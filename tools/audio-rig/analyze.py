#!/usr/bin/env python3
"""Detect a known stimulus in every channel of a recording.

Per channel it reports RMS/peak (dBFS), the noise floor from the lead-in, the
matched-filter (cross-correlation) detection peak, the stimulus start position
(delay) in the recording, detection SNR and a pass/fail verdict. For sweeps it
also estimates the impulse response (regularised deconvolution) and latency.
Pilot-tone stimuli additionally get per-tone narrow-band level and SNR.

Recording input: any WAV that libsndfile reads (any channel count, 16/24/32-bit
or float, any rate) or raw PCM with explicit --rate/--channels/--format
(S16_LE, S24_3LE, S24_LE, S32_LE, F32_LE, U8). The reference is resampled to
the recording rate. Nothing here is committed: outputs go to a gitignored dir.

Examples:
  analyze.py rec.wav --stimulus sweep --spectrogram
  analyze.py tv.raw --raw --rate 16000 --channels 4 --format S32_LE --stimulus speech_full
"""
import argparse
import json
import math
import sys
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import fftconvolve, resample_poly

ROOT = Path(__file__).resolve().parents[2]
STIM_DIR = ROOT / "artifacts" / "audio-rig" / "stimuli"
OUT_DIR = ROOT / "artifacts" / "audio-rig" / "analysis"
RAW_FORMATS = ("S16_LE", "S24_3LE", "S24_LE", "S32_LE", "F32_LE", "U8")
EPS = 1e-12


# ------------------------------------------------------------------ I/O

def decode_raw(data, fmt, channels):
    fmt = fmt.upper()
    if fmt == "S16_LE":
        x = np.frombuffer(data[: len(data) // 2 * 2], "<i2").astype(np.float64) / 32768.0
    elif fmt == "S32_LE":
        x = np.frombuffer(data[: len(data) // 4 * 4], "<i4").astype(np.float64) / 2147483648.0
    elif fmt == "F32_LE":
        x = np.frombuffer(data[: len(data) // 4 * 4], "<f4").astype(np.float64)
    elif fmt == "U8":
        x = (np.frombuffer(data, "u1").astype(np.float64) - 128.0) / 128.0
    elif fmt == "S24_LE":  # 24 significant bits in the low bytes of a 32-bit word
        v = np.frombuffer(data[: len(data) // 4 * 4], "<i4").astype(np.int64)
        v = ((v & 0xFFFFFF) ^ 0x800000) - 0x800000
        x = v.astype(np.float64) / 8388608.0
    elif fmt == "S24_3LE":
        b = np.frombuffer(data[: len(data) // 3 * 3], "u1").reshape(-1, 3).astype(np.int64)
        v = b[:, 0] | (b[:, 1] << 8) | (b[:, 2] << 16)
        v = (v ^ 0x800000) - 0x800000
        x = v.astype(np.float64) / 8388608.0
    else:
        raise SystemExit(f"unsupported raw format {fmt}; choose from {RAW_FORMATS}")
    n = len(x) // channels * channels
    return x[:n].reshape(-1, channels)


def load_recording(path, raw, rate, channels, fmt):
    if raw or path.suffix.lower() in (".raw", ".pcm"):
        if not (rate and channels and fmt):
            raise SystemExit("raw PCM needs --rate, --channels and --format")
        return decode_raw(path.read_bytes(), fmt, channels), rate
    x, fs = sf.read(str(path), dtype="float64", always_2d=True)
    return x, fs


def resample(x, fs_in, fs_out):
    if fs_in == fs_out:
        return x
    g = gcd(int(fs_in), int(fs_out))
    return resample_poly(x, int(fs_out) // g, int(fs_in) // g)


def db(v):
    return 20 * math.log10(max(v, EPS))


def find_manifest_entry(ref_path):
    mf = ref_path.parent / "manifest.json"
    if not mf.exists():
        return None
    data = json.loads(mf.read_text())
    for rate in data.get("rates", {}).values():
        for name, ent in rate.get("stimuli", {}).items():
            if ent["file"] == ref_path.name:
                return {"name": name, **ent}
    return None


# ------------------------------------------------------------------ detection

def band_ms(x, fs, f0, half_bw=40.0):
    """Mean-square contributed by a narrow band (Hann periodogram)."""
    n = len(x)
    w = np.hanning(n)
    spec = np.fft.rfft((x - x.mean()) * w)
    f = np.fft.rfftfreq(n, 1 / fs)
    sel = (f >= f0 - half_bw) & (f <= f0 + half_bw)
    return float(2 * np.sum(np.abs(spec[sel]) ** 2) / (n * np.sum(w ** 2)))


def band_track(x, fs, freqs, frame_s=0.05, hop_s=0.01, half_bw=50.0):
    """Hann-STFT band energy (mean square) per frame for each tone band: (n_tones, n_frames), frame hop."""
    n = int(frame_s * fs)
    hop = max(int(hop_s * fs), 1)
    w = np.hanning(n)
    nfr = 1 + (len(x) - n) // hop
    idx = np.arange(n)[None, :] + hop * np.arange(nfr)[:, None]
    spec = np.fft.rfft(x[idx] * w, axis=1)
    f = np.fft.rfftfreq(n, 1 / fs)
    out = np.empty((len(freqs), nfr))
    for i, f0 in enumerate(freqs):
        sel = (f >= f0 - half_bw) & (f <= f0 + half_bw)
        out[i] = 2 * np.sum(np.abs(spec[:, sel]) ** 2, axis=1) / (n * np.sum(w ** 2))
    return out, hop / fs, frame_s


def detect_pilots(x, fs, params, lead_in, tone_snr_db, min_contrast_db=6.0):
    """Per-band energy detector for the pilot-tone sequence (a matched filter is a poor detector
    for periodic tones). Finds the start that maximises the in-slot vs other-slot contrast of each
    tone, then reports per-tone level, SNR over the noise in the same band, and timing."""
    tones, tone_s, gap_s = params["tones_hz"], params["tone_s"], params["gap_s"]
    nt = len(tones)
    E, hop_s, frame_s = band_track(x, fs, tones)
    nfr = E.shape[1]
    floor = 1e-11  # mean-square floor (about 16-bit quantisation noise): digital-silence lead-ins stay finite
    # noise per band: median over the lead-in frames, or the 25th percentile of the whole recording
    # (whichever is lower: tolerant to a contaminated or missing lead-in)
    lead_fr = int(max(lead_in - frame_s, 0) / hop_s)
    noise = np.percentile(E, 25, axis=1)
    if lead_fr >= 10:
        noise = np.minimum(noise, np.median(E[:, :lead_fr], axis=1))
    noise = np.maximum(noise, floor)
    D = np.clip(10 * np.log10(np.maximum(E, floor) / noise[:, None]), -10, 60)
    cs = np.concatenate([np.zeros((nt, 1)), np.cumsum(D, axis=1)], axis=1)
    step = (tone_s + gap_s) / hop_s
    half = frame_s / 2 / hop_s  # frame centre offset in frames
    lo, hi = 0.1 * tone_s / hop_s, 0.9 * tone_s / hop_s

    def slot_mean(i, j, t0_fr):
        a = int(round(t0_fr + j * step + lo - half)); b = int(round(t0_fr + j * step + hi - half))
        if a < 0 or b > nfr or b - a < 2:
            return None
        return (cs[i, b] - cs[i, a]) / (b - a)

    need = math.ceil(0.75 * nt)
    best, best_t = -1e9, None
    for t0 in range(0, nfr):  # tolerate a truncated recording: slots outside it are skipped
        total, navail = 0.0, 0
        for i in range(nt):
            m = slot_mean(i, i, t0)
            others = [v for v in (slot_mean(i, j, t0) for j in range(nt) if j != i) if v is not None]
            if m is None or not others:
                continue
            total += m - float(np.mean(others))
            navail += 1
        if navail >= need and total > best:
            best, best_t = total, t0
    if best_t is None:
        return {"detected": False, "verdict": "not detected", "score_db": float("nan")}
    rows = []
    for i, f0 in enumerate(tones):
        m = slot_mean(i, i, best_t)
        others = [v for v in (slot_mean(i, j, best_t) for j in range(nt) if j != i) if v is not None]
        if m is None or not others:
            rows.append({"hz": f0, "level_dbfs": float("nan"), "snr_db": float("nan"),
                         "contrast_db": float("nan"), "ok": False})
            continue
        a = int(round(best_t + i * step + lo - half)); b = int(round(best_t + i * step + hi - half))
        lvl = 10 * math.log10(max(2 * float(np.mean(E[i, a:b])), floor))
        rows.append({"hz": f0, "level_dbfs": lvl,
                     "snr_db": float(10 * math.log10(max(np.mean(E[i, a:b]), floor) / noise[i])),
                     "contrast_db": float(m - float(np.mean(others)))})
    ok = [r.get("ok", True) and r["snr_db"] >= tone_snr_db and r["contrast_db"] >= min_contrast_db for r in rows]
    for r, o in zip(rows, ok):
        r["ok"] = bool(o)
    detected = sum(ok) >= need
    return {"detected": bool(detected), "pilot_tones": rows, "start_coarse_s": best_t * hop_s,
            "score_db": float(np.nanmedian([r["snr_db"] for r in rows]))}


def matched_filter(x, r, guard):
    """Valid-mode cross-correlation; returns (lag, frac, c, ncc, psr_db)."""
    L = len(r)
    c = fftconvolve(x, r[::-1], mode="valid")
    ac = np.abs(c)
    k0 = int(np.argmax(ac))
    cs = np.concatenate([[0.0], np.cumsum(x ** 2)])
    e = cs[L:] - cs[:-L]
    ncc = c[k0] / (math.sqrt(max(e[k0], 0.0)) * np.linalg.norm(r) + EPS)
    frac = 0.0
    if 0 < k0 < len(ac) - 1:  # parabolic peak interpolation
        a, b, d = ac[k0 - 1], ac[k0], ac[k0 + 1]
        den = a - 2 * b + d
        frac = 0.5 * (a - d) / den if den != 0 else 0.0
    mask = np.ones(len(ac), bool)
    mask[max(k0 - guard, 0): k0 + guard + 1] = False
    out = ac[mask]
    rms_out = math.sqrt(float(np.mean(out ** 2))) if out.size else 0.0
    psr = db(ac[k0] / rms_out) if rms_out > 0 else float("inf")
    return k0, frac, float(ac[k0]), float(ncc), psr


def impulse_response(x, r, k0, fs, pre_s=0.1, post_s=0.5):
    s0 = max(k0 - int(pre_s * fs), 0)
    seg = x[s0: k0 + len(r) + int(post_s * fs)]
    n = len(seg)
    X = np.fft.rfft(r, n)
    Y = np.fft.rfft(seg)
    p = np.abs(X) ** 2
    H = Y * np.conj(X) / (p + 1e-3 * p.max())
    h = np.fft.irfft(H, n)
    ah = np.abs(h)
    pk = int(np.argmax(ah))
    pre_n = max(int((k0 - s0) - 0.01 * fs), 0)
    noise = float(np.sqrt(np.mean(h[:pre_n] ** 2))) if pre_n > 200 else float("nan")
    first = int(np.argmax(ah > 0.2 * ah[pk]))
    return {"ir_peak_value": float(h[pk]),
            "ir_peak_ms": (s0 + pk) / fs * 1000,
            "ir_first_arrival_ms": (s0 + first) / fs * 1000,
            "ir_peak_to_prenoise_db": db(ah[pk] / noise) if noise == noise and noise > 0 else float("nan")}


def analyze_channel(x, ref, fs, args, lead_in, entry, ref_start_ms, kind):
    res = {}
    res["rms_dbfs"] = db(float(np.sqrt(np.mean(x ** 2))))
    res["peak_dbfs"] = db(float(np.max(np.abs(x))))
    if np.max(np.abs(x)) == 0:
        res.update(detected=False, verdict="SILENT (digital zero)")
        return res
    guard = int(args.guard_ms / 1000 * fs)
    pil = None
    if kind == "pilots":
        params = entry["params"] if entry and "tones_hz" in entry.get("params", {}) else \
            {"tones_hz": [500, 1000, 2000, 3000], "tone_s": 0.5, "gap_s": 0.1}
        pil = detect_pilots(x, fs, params, lead_in, args.tone_snr_db)
        res["method"] = "pilot band energy"
        res["score_db"] = pil["score_db"]
        if "pilot_tones" not in pil:
            res.update(detected=False, verdict="not detected")
            return res
        # sub-band timing: refine with a matched filter in +-25 ms around the band-energy estimate
        c0 = max(int((pil["start_coarse_s"] - 0.05) * fs), 0)
        win = x[c0: c0 + len(ref) + int(0.1 * fs)]
        k0, frac, peak, ncc, _ = matched_filter(win, ref, guard) if len(win) >= len(ref) else (int(0.05 * fs), 0, 0, 0, 0)
        k0 += c0
        psr = float("nan")
    else:
        k0, frac, peak, ncc, psr = matched_filter(x, ref, guard)
        res["method"] = "matched filter"
        res["score_db"] = psr
    start_s = (k0 + frac) / fs
    res.update(mf_peak=peak, ncc=ncc, mf_psr_db=psr, start_ms=start_s * 1000)
    # noise floor from the lead-in, ending before the detected start
    lead_n = int(min(lead_in, start_s - 0.1) * fs)
    noise = None
    if lead_n >= int(0.2 * fs):
        seg = x[:lead_n]
        noise = float(np.sqrt(np.mean((seg - seg.mean()) ** 2)))
    res["noise_floor_dbfs"] = db(noise) if noise is not None else float("nan")
    sig = x[k0: k0 + len(ref)]
    res["stim_rms_dbfs"] = db(float(np.sqrt(np.mean(sig ** 2))))
    res["snr_db"] = res["stim_rms_dbfs"] - res["noise_floor_dbfs"] if noise is not None else float("nan")
    if ref_start_ms is not None:
        res["delay_vs_ref_ms"] = res["start_ms"] - ref_start_ms
    if args.expected_start is not None:
        res["delay_vs_expected_ms"] = res["start_ms"] - args.expected_start * 1000
    detected = pil["detected"] if pil else bool(psr >= args.threshold_db)
    res["detected"] = detected
    res["verdict"] = "DETECTED" if detected else "not detected"
    if kind == "sweep" and detected:
        res.update(impulse_response(x, ref, k0, fs))
    if pil:
        res["pilot_tones"] = pil["pilot_tones"]
    return res


def save_spectrogram(x, fs, start_ms, dur_s, path, title):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from scipy.signal import spectrogram
    nper = 512 if fs <= 16000 else 1024
    f, t, S = spectrogram(x, fs, nperseg=nper, noverlap=nper * 3 // 4)
    fig, ax = plt.subplots(figsize=(9, 4))
    ax.pcolormesh(t, f, 10 * np.log10(S + 1e-14), shading="auto", vmin=-120, vmax=-30)
    ax.set_ylim(0, min(fs / 2, 10000))
    if start_ms is not None:
        ax.axvline(start_ms / 1000, color="w", ls="--", lw=0.8)
        ax.axvline(start_ms / 1000 + dur_s, color="w", ls=":", lw=0.8)
    ax.set_xlabel("time (s)")
    ax.set_ylabel("Hz")
    ax.set_title(title)
    fig.tight_layout()
    fig.savefig(path, dpi=110)
    plt.close(fig)


# ------------------------------------------------------------------ main

def fmt(v, spec=".1f"):
    return "n/a" if v is None or (isinstance(v, float) and v != v) else format(v, spec)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    ap.add_argument("recording", type=Path)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--ref", type=Path, help="reference stimulus WAV")
    g.add_argument("--stimulus", help="name in the stimuli dir, e.g. sweep, pilots, noise, speech_full")
    ap.add_argument("--stim-rate", choices=["16k", "48k"], default="48k")
    ap.add_argument("--raw", action="store_true", help="recording is raw PCM (implied by .raw/.pcm)")
    ap.add_argument("--rate", type=int)
    ap.add_argument("--channels", type=int)
    ap.add_argument("--format", choices=RAW_FORMATS)
    ap.add_argument("--leadin", type=float, help="seconds of silence at the start (default: sidecar JSON or 1.0)")
    ap.add_argument("--threshold-db", type=float, default=20.0,
                    help="matched-filter peak-to-sidelobe-rms threshold for a detection (default 20 dB)")
    ap.add_argument("--tone-snr-db", type=float, default=10.0,
                    help="pilots: minimum per-tone SNR over the same-band noise (default 10 dB; also needs >= 6 dB "
                         "contrast between the tone's own slot and the other tones' slots, in >= 3 of 4 tones)")
    ap.add_argument("--guard-ms", type=float, default=50.0, help="exclusion around the peak for the sidelobe rms")
    ap.add_argument("--kind", choices=["sweep", "pilots", "noise", "speech"], help="override stimulus kind")
    ap.add_argument("--ref-recording", type=Path,
                    help="recording of a reference channel (e.g. the sink monitor) for relative delay")
    ap.add_argument("--spectrogram", action="store_true", help="save a spectrogram PNG per channel")
    ap.add_argument("--out-dir", type=Path, default=OUT_DIR)
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    ref_path = args.ref or (STIM_DIR / f"{args.stimulus}_{args.stim_rate}.wav")
    if not ref_path.exists():
        sys.exit(f"reference not found: {ref_path}")
    ref, ref_fs = sf.read(str(ref_path), dtype="float64", always_2d=True)
    ref = ref[:, 0]
    entry = find_manifest_entry(ref_path)
    kind = args.kind or (entry["kind"] if entry else ("sweep" if ref_path.name.startswith("sweep") else "speech"))

    X, fs = load_recording(args.recording, args.raw, args.rate, args.channels, args.format)
    sidecar = args.recording.with_suffix(".json")
    meta = json.loads(sidecar.read_text()) if sidecar.exists() else {}
    lead_in = args.leadin if args.leadin is not None else float(meta.get("lead_in_s", 1.0))
    args.expected_start = meta.get("expected_start_s")
    refx = resample(ref, ref_fs, fs)
    if len(X) < len(refx):
        sys.exit(f"recording ({len(X)/fs:.2f} s) is shorter than the stimulus ({len(refx)/fs:.2f} s)")

    ref_start_ms = None
    ref_dur_ms = None
    end_aligned = False
    if args.ref_recording is None and meta and "__monitor" not in args.recording.stem:
        cand = list(args.recording.parent.glob(f"{args.recording.stem.split('__')[0]}__monitor.wav"))
        args.ref_recording = cand[0] if cand else None
    if args.ref_recording and args.ref_recording.exists() and args.ref_recording != args.recording:
        RX, rfs = load_recording(args.ref_recording, False, None, None, None)
        rref = resample(ref, ref_fs, rfs)
        if len(RX) >= len(rref):
            k0, frac, _, _, rpsr = matched_filter(RX[:, 0], rref, int(args.guard_ms / 1000 * rfs))
            if rpsr >= args.threshold_db:
                ref_start_ms = (k0 + frac) / rfs * 1000
                ref_dur_ms = len(RX) / rfs * 1000
                rside = args.ref_recording.with_suffix(".json")
                rmeta = json.loads(rside.read_text()) if rside.exists() else {}
                # recorders of one play_record session are stopped together, so aligning the file ENDS
                # removes the (unknown, per-device) capture start offsets
                end_aligned = bool(meta.get("player_start_unix")) and \
                    meta.get("player_start_unix") == rmeta.get("player_start_unix")

    results = []
    for ch in range(X.shape[1]):
        r = analyze_channel(X[:, ch], refx, fs, args, lead_in, entry, ref_start_ms, kind)
        r["channel"] = ch
        if ref_start_ms is not None and "start_ms" in r:
            r["delay_vs_ref_startaligned_ms"] = r["delay_vs_ref_ms"]
            if end_aligned:
                r["delay_vs_ref_ms"] = (r["start_ms"] - len(X) / fs * 1000) - (ref_start_ms - ref_dur_ms)
        results.append(r)

    args.out_dir.mkdir(parents=True, exist_ok=True)
    stem = args.recording.stem
    if args.spectrogram:
        for r in results:
            png = args.out_dir / f"{stem}_ch{r['channel']}_spec.png"
            save_spectrogram(X[:, r["channel"]], fs, r.get("start_ms"), len(refx) / fs, png,
                             f"{stem} ch{r['channel']} ({r['verdict']})")
            r["spectrogram"] = str(png)
    summary = {"recording": str(args.recording), "rate": fs, "channels": X.shape[1],
               "duration_s": len(X) / fs, "reference": ref_path.name, "kind": kind,
               "lead_in_s": lead_in, "threshold_db": args.threshold_db,
               "ref_recording": str(args.ref_recording) if ref_start_ms is not None else None,
               "ref_start_ms": ref_start_ms, "delay_vs_ref_alignment": "end" if end_aligned else "start", "results": results}
    (args.out_dir / f"{stem}__analysis.json").write_text(json.dumps(summary, indent=2, default=str) + "\n")

    if not args.quiet:
        print(f"{args.recording.name}: {fs} Hz, {X.shape[1]} ch, {len(X)/fs:.2f} s | ref {ref_path.name} "
              f"({kind}) | lead-in {lead_in:.2f} s | threshold {args.threshold_db:.0f} dB")
        print(f"{'ch':>2} {'RMS':>7} {'peak':>7} {'noise':>7} {'score':>7} {'NCC':>6} {'start ms':>9} "
              f"{'vs ref':>8} {'SNR':>6}  verdict")
        for r in results:
            print(f"{r['channel']:>2} {fmt(r['rms_dbfs']):>7} {fmt(r['peak_dbfs']):>7} "
                  f"{fmt(r.get('noise_floor_dbfs')):>7} {fmt(r.get('score_db')):>7} "
                  f"{fmt(r.get('ncc'), '.3f'):>6} {fmt(r.get('start_ms')):>9} "
                  f"{fmt(r.get('delay_vs_ref_ms')):>8} {fmt(r.get('snr_db')):>6}  {r['verdict']}")
            if "ir_peak_ms" in r:
                print(f"     IR: peak {r['ir_peak_value']:+.3f} at {r['ir_peak_ms']:.2f} ms, first arrival "
                      f"{r['ir_first_arrival_ms']:.2f} ms, peak/pre-noise {fmt(r['ir_peak_to_prenoise_db'])} dB")
            for t in r.get("pilot_tones", []):
                print(f"     tone {t['hz']:>5} Hz: {fmt(t['level_dbfs'])} dBFS, SNR {fmt(t['snr_db'])} dB, "
                      f"contrast {fmt(t['contrast_db'])} dB {'ok' if t['ok'] else 'weak'}")
        print("units: levels dBFS; score = matched-filter peak vs rms of correlation outside +-guard (dB), "
              "or median per-tone SNR for pilots; "
              "start = stimulus start inside the recording; vs ref = delay relative to the monitor recording (files aligned by their ends when from one session; includes sink/Bluetooth latency + acoustic path); SNR = stimulus-window RMS minus lead-in RMS")
    return 0 if any(r["detected"] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
