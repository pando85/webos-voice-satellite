#!/usr/bin/env python3
"""Analysis of TV mic + reference captures made with `tv_dualcap.py`.

Library and CLI (numpy + scipy; run with the rig venv). Everything works on a
capture directory written by `tv_dualcap_demux.py` (`mic.raw`, `ref.raw`,
`meta.jsonl`); formats come from the meta "open" record.

Subcommands:
  levels DIR            windowed RMS per channel (quick look)
  locate DIR STIM.wav   where a stimulus sits in the mic and the reference, and
                        its level (RMS/peak, optional band) in each
  echo DIR              reference-to-mic delay, coherence, coherence-bound and
                        adaptive-filter (PBFDAF NLMS) ERLE, windowed drift

Levels are dBFS of each PCM's own full scale (24-bit for the mic, 32-bit for
the reference), so mic and reference levels are not comparable in absolute
terms. Delay sign: positive = the mic lags the reference.
"""
import argparse
import json
import os
import sys

import numpy as np
from scipy import signal

SCALE = {"S16_LE": 2.0**15, "S24_LE": 2.0**23, "S32_LE": 2.0**31}
DTYPE = {"S16_LE": "<i2", "S24_LE": "<i4", "S32_LE": "<i4"}


# ---------------------------------------------------------------- loading


def read_meta(path):
    with open(os.path.join(path, "meta.jsonl")) as f:
        return [json.loads(line) for line in f if line.strip()]


def load(path):
    """Return dict(mic=(N,C) float, fs_mic, ref=(M,K) float or None, fs_ref, meta)."""
    metas = read_meta(path)
    opened = next(m for m in metas if m.get("kind") == "open")
    out = {"meta": metas, "link_rc": opened.get("link_rc"), "mic": None, "ref": None}
    for p in opened["pcms"]:
        key = "mic" if p["sid"] == 0 else "ref"
        raw = os.path.join(path, key + ".raw")
        if not os.path.exists(raw):
            continue
        x = np.fromfile(raw, dtype=DTYPE[p["format"]]).astype(np.float64) / SCALE[p["format"]]
        n = len(x) // p["channels"]
        out[key] = x[: n * p["channels"]].reshape(n, p["channels"])
        out["fs_" + key] = p["rate"]
    return out


# ---------------------------------------------------------------- basics


def db(power):
    return 10.0 * np.log10(np.maximum(power, 1e-30))


def rms_db(x):
    return float(db(np.mean(np.asarray(x, dtype=np.float64) ** 2)))


def peak_db(x):
    return float(20.0 * np.log10(max(float(np.max(np.abs(x))), 1e-15)))


def bandpass(x, fs, lo, hi, order=6):
    sos = signal.butter(order, [lo, hi], btype="bandpass", fs=fs, output="sos")
    return signal.sosfiltfilt(sos, x)


def windowed_db(x, fs, win_s=0.5):
    n = max(1, int(fs * win_s))
    k = len(x) // n
    return db(np.mean(x[: k * n].reshape(k, n) ** 2, axis=1))


def resample(x, fs_from, fs_to):
    if fs_from == fs_to:
        return np.asarray(x, dtype=np.float64)
    g = np.gcd(int(fs_from), int(fs_to))
    return signal.resample_poly(x, int(fs_to) // g, int(fs_from) // g)


def load_wav_mono(path):
    import soundfile as sf

    x, fs = sf.read(path, always_2d=True)
    return x[:, 0].astype(np.float64), fs


# ---------------------------------------------------------------- correlation


def xcorr(a, b):
    """Full cross-correlation c[k] = sum a[n+k] b[n], returned with lags."""
    n = len(a) + len(b) - 1
    nfft = 1 << (n - 1).bit_length()
    c = np.fft.irfft(np.fft.rfft(a, nfft) * np.conj(np.fft.rfft(b, nfft)), nfft)
    c = np.concatenate([c[-(len(b) - 1):], c[: len(a)]]) if len(b) > 1 else c[: len(a)]
    lags = np.arange(-(len(b) - 1), len(a))
    return c, lags


def find_template(x, fs, template, fs_t, band=None):
    """Locate `template` in `x`. Returns (start_s, normalized_peak, psr_db)."""
    t = resample(template, fs_t, fs)
    if band:
        x = bandpass(x, fs, *band)
        t = bandpass(t, fs, *band)
    c, lags = xcorr(x, t)
    valid = lags >= 0
    c, lags = c[valid], lags[valid]
    i = int(np.argmax(np.abs(c)))
    norm = np.abs(c[i]) / (np.linalg.norm(t) * np.linalg.norm(x[lags[i]: lags[i] + len(t)]) + 1e-30)
    guard = int(0.05 * fs)
    side = np.abs(np.concatenate([c[: max(0, i - guard)], c[i + guard:]]))
    psr = float(db(c[i] ** 2) - db(np.max(side) ** 2)) if len(side) else float("inf")
    return lags[i] / fs, float(norm), psr


def delay_estimate(mic, ref, fs, max_lag_s=0.5, phat=True):
    """Delay of `mic` relative to `ref` (same fs), sub-sample by parabolic fit.

    Returns (delay_s, peak_value). GCC-PHAT by default (robust to coloration).
    """
    n = len(mic) + len(ref)
    nfft = 1 << (n - 1).bit_length()
    X = np.fft.rfft(mic, nfft) * np.conj(np.fft.rfft(ref, nfft))
    if phat:
        X = X / (np.abs(X) + 1e-12)
    c = np.fft.irfft(X, nfft)
    m = int(max_lag_s * fs)
    seg = np.concatenate([c[-m:], c[: m + 1]])
    i = int(np.argmax(np.abs(seg)))
    frac = 0.0
    if 0 < i < len(seg) - 1:
        y0, y1, y2 = np.abs(seg[i - 1: i + 2])
        den = y0 - 2 * y1 + y2
        frac = 0.5 * (y0 - y2) / den if den != 0 else 0.0
    return (i - m + frac) / fs, float(np.abs(seg[i]))


def align(ref, delay_samples, n):
    """Shift `ref` by an integer delay (mic lags by `delay_samples`), length n."""
    d = int(round(delay_samples))
    out = np.zeros(n)
    if d >= 0:
        seg = ref[: max(0, n - d)]
        out[d: d + len(seg)] = seg
    else:
        seg = ref[-d: -d + n]
        out[: len(seg)] = seg
    return out


# ---------------------------------------------------------------- echo metrics


def coherence_stats(mic, ref_aligned, fs, band=(200.0, 4000.0), nperseg=8192):
    f, C = signal.coherence(mic, ref_aligned, fs=fs, nperseg=nperseg)
    _, Pdd = signal.welch(mic, fs=fs, nperseg=nperseg)
    m = (f >= band[0]) & (f <= band[1])
    mean_c = float(np.mean(C[m]))
    # Best linear (any FIR, unlimited length) residual: Pdd * (1 - C) per bin.
    bound = float(db(np.sum(Pdd[m])) - db(np.sum(Pdd[m] * (1.0 - C[m]))))
    return {"mean_coherence": mean_c, "median_coherence": float(np.median(C[m])),
            "erle_bound_db": bound, "band_hz": list(band)}


def pbfdaf(mic, ref, block=256, partitions=16, mu=0.5, eps=1e-10):
    """Partitioned-block frequency-domain NLMS echo canceller (constrained).

    Filter length = block * partitions. Returns the error signal (mic - echo
    estimate), same length as the input truncated to whole blocks.
    """
    N, P = block, partitions
    nblocks = len(mic) // N
    W = np.zeros((P, N + 1), dtype=complex)
    Xh = np.zeros((P, N + 1), dtype=complex)
    pw = np.full(N + 1, 1e-6)
    prev = np.zeros(N)
    e_out = np.zeros(nblocks * N)
    for k in range(nblocks):
        cur = ref[k * N: (k + 1) * N]
        Xh = np.roll(Xh, 1, axis=0)
        Xh[0] = np.fft.rfft(np.concatenate([prev, cur]))
        prev = cur
        y = np.fft.irfft(np.sum(W * Xh, axis=0))[N:]
        e = mic[k * N: (k + 1) * N] - y
        e_out[k * N: (k + 1) * N] = e
        E = np.fft.rfft(np.concatenate([np.zeros(N), e]))
        pw = 0.9 * pw + 0.1 * np.abs(Xh[0]) ** 2
        G = mu * np.conj(Xh) * E / (P * pw + eps)
        g = np.fft.irfft(G, axis=1)
        g[:, N:] = 0.0
        W += np.fft.rfft(g, axis=1)
    return e_out


def erle_db(mic, err, fs, skip_s=0.0, band=None):
    n = len(err)
    s = int(skip_s * fs)
    d, e = mic[s:n], err[s:n]
    if band:
        d, e = bandpass(d, fs, *band), bandpass(e, fs, *band)
    return float(db(np.mean(d ** 2)) - db(np.mean(e ** 2)))


def drift(mic, ref, fs, win_s=4.0, hop_s=10.0, max_lag_s=0.3, min_peak=0.0, active=None):
    """Delay per window and a linear fit. Returns dict(times, delays_ms, ppm, ...).

    `active`: optional boolean mask of windows to keep (for example where the
    reference carries signal).
    """
    w, h = int(win_s * fs), int(hop_s * fs)
    times, delays, peaks = [], [], []
    for s in range(0, len(mic) - w, h):
        r = ref[s: s + w]
        if rms_db(r) < -100:
            continue
        d, p = delay_estimate(mic[s: s + w], r, fs, max_lag_s)
        if p < min_peak:
            continue
        times.append((s + w / 2) / fs)
        delays.append(d * 1000.0)
        peaks.append(p)
    res = {"times_s": times, "delays_ms": delays, "peaks": peaks}
    if len(times) >= 3:
        slope, icpt = np.polyfit(times, delays, 1)
        res.update(ppm=float(slope * 1000.0), intercept_ms=float(icpt),
                   spread_ms=float(np.max(delays) - np.min(delays)), std_ms=float(np.std(delays)))
    return res


# ---------------------------------------------------------------- CLI


def ref_mono(cap, channels=(0, 1)):
    return np.mean(cap["ref"][:, list(channels)], axis=1)


def cmd_levels(a):
    cap = load(a.dir)
    for name in ("mic", "ref"):
        x = cap[name]
        if x is None:
            continue
        for c in range(x.shape[1]):
            v = windowed_db(x[:, c], cap["fs_" + name], a.win)
            print("%s%d %s" % (name, c, " ".join("%4.0f" % max(-199, y) for y in v)))


def cmd_locate(a):
    cap = load(a.dir)
    tpl, fs_t = load_wav_mono(a.stim)
    seg = [float(v) for v in a.segment] if a.segment else [0.0, len(tpl) / fs_t]
    band = [float(v) for v in a.band] if a.band else None
    out = {}
    for name, chans in (("mic", range(cap["mic"].shape[1]) if cap["mic"] is not None else []),
                        ("ref", a.ref_channels if cap["ref"] is not None else [])):
        fs = cap["fs_" + name] if chans else None
        for c in chans:
            x = cap[name][:, c]
            skip = int(0.25 * fs) if name == "mic" else 0
            st, norm, psr = find_template(x[skip:], fs, tpl, fs_t, band=a.locate_band)
            st += skip / fs
            s0, s1 = int((st + seg[0]) * fs), int((st + seg[1]) * fs)
            y = x[s0:s1]
            yb = bandpass(y, fs, *band) if band else y
            pre = x[max(skip, int((st - 1.0) * fs)): max(skip + 1, int((st - 0.1) * fs))]
            preb = bandpass(pre, fs, *band) if (band and len(pre) > 64) else pre
            out["%s%d" % (name, c)] = dict(start_s=round(st, 4), norm=round(norm, 3), psr_db=round(psr, 1),
                                          rms_db=round(rms_db(yb), 1), peak_db=round(peak_db(y), 1),
                                          pre_rms_db=round(rms_db(preb), 1) if len(preb) else None,
                                          clipped=int(np.sum(np.abs(y) >= 0.999)))
    print(json.dumps(out))


def cmd_echo(a):
    cap = load(a.dir)
    fs = cap["fs_mic"]
    mic = cap["mic"][:, a.mic_channel]
    r = resample(ref_mono(cap, a.ref_channels), cap["fs_ref"], fs)
    n = min(len(mic), len(r))
    s0 = int(a.start * fs)
    s1 = int(a.end * fs) if a.end else n
    s0 = max(s0, int(0.25 * fs))
    mic, r = mic[s0:s1], r[s0:s1]
    d, pk = delay_estimate(mic, r, fs, max_lag_s=a.max_lag)
    ra = align(r, d * fs, len(mic))
    res = {"segment_s": [s0 / fs, s1 / fs], "delay_ms": round(d * 1000, 3), "gcc_peak": round(pk, 4),
           "mic_rms_db": round(rms_db(mic), 1), "ref_rms_db": round(rms_db(r), 1)}
    res.update({k: (round(v, 3) if isinstance(v, float) else v) for k, v in coherence_stats(mic, ra, fs, nperseg=a.coh_nperseg).items()})
    if not a.no_aec:
        lead = int(a.lead_ms * fs / 1000)
        ra_lead = align(r, d * fs - lead, len(mic))  # leave room for a non-causal part of the path
        e = pbfdaf(mic, ra_lead, block=a.block, partitions=a.partitions, mu=a.mu)
        res["pbfdaf"] = {"taps": a.block * a.partitions, "mu": a.mu,
                         "erle_db_after_converge": round(erle_db(mic, e, fs, skip_s=a.converge), 1),
                         "erle_db_200_4k": round(erle_db(mic, e, fs, skip_s=a.converge, band=(200, 4000)), 1)}
    if a.drift:
        res["drift"] = {k: (np.round(v, 3).tolist() if isinstance(v, list) else v)
                        for k, v in drift(mic, r, fs, win_s=a.drift_win, hop_s=a.drift_hop).items()}
    if a.digital:
        res["digital"] = digital_reference_metrics(cap, a, mic, s0, fs)
    print(json.dumps(res))


def locate_digital(cap, wav_path, excerpt_s=8.0, skip_s=2.0):
    """Place a played file in the capture's mic timeline (via the reference PCM).

    Returns the file resampled to the mic rate and shifted so that sample i
    corresponds to mic sample i (zeros outside the file).
    """
    x, fs_x = load_wav_mono(wav_path)
    fs_r, fs = cap["fs_ref"], cap["fs_mic"]
    a = int(skip_s * fs_x)
    ex = x[a: a + int(excerpt_s * fs_x)]
    st, _, _ = find_template(cap["ref"][:, 0], fs_r, ex, fs_x)
    start_in_ref = st - skip_s  # file start, seconds of the capture timeline
    x16 = resample(x, fs_x, fs)
    out = np.zeros(len(cap["mic"]))
    off = int(round(start_in_ref * fs))
    src0 = max(0, -off)
    dst0 = max(0, off)
    n = min(len(x16) - src0, len(out) - dst0)
    if n > 0:
        out[dst0: dst0 + n] = x16[src0: src0 + n]
    return out, start_in_ref


def digital_reference_metrics(cap, a, mic, s0, fs):
    xd, start = locate_digital(cap, a.digital)
    xd = xd[s0: s0 + len(mic)]
    d, pk = delay_estimate(mic, xd, fs, max_lag_s=a.max_lag)
    xa = align(xd, d * fs, len(mic))
    out = {"file_start_s": round(start, 4), "delay_ms": round(d * 1000, 3), "gcc_peak": round(pk, 4)}
    out.update({k: (round(v, 3) if isinstance(v, float) else v) for k, v in coherence_stats(mic, xa, fs, nperseg=a.coh_nperseg).items()})
    if not a.no_aec:
        lead = int(a.lead_ms * fs / 1000)
        e = pbfdaf(mic, align(xd, d * fs - lead, len(mic)), block=a.block, partitions=a.partitions, mu=a.mu)
        out["pbfdaf"] = {"erle_db_after_converge": round(erle_db(mic, e, fs, skip_s=a.converge), 1),
                         "erle_db_200_4k": round(erle_db(mic, e, fs, skip_s=a.converge, band=(200, 4000)), 1)}
    return out


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("levels")
    p.add_argument("dir")
    p.add_argument("--win", type=float, default=0.5)
    p.set_defaults(fn=cmd_levels)
    p = sub.add_parser("locate")
    p.add_argument("dir")
    p.add_argument("stim")
    p.add_argument("--segment", nargs=2, metavar=("START", "END"), help="seconds within the stimulus")
    p.add_argument("--band", nargs=2, metavar=("LO", "HI"), help="band for the RMS")
    p.add_argument("--locate-band", nargs=2, type=float, default=None)
    p.add_argument("--ref-channels", type=int, nargs="*", default=[0, 1])
    p.set_defaults(fn=cmd_locate)
    p = sub.add_parser("echo")
    p.add_argument("dir")
    p.add_argument("--mic-channel", type=int, default=0)
    p.add_argument("--ref-channels", type=int, nargs="+", default=[0, 1])
    p.add_argument("--start", type=float, default=0.0)
    p.add_argument("--end", type=float, default=None)
    p.add_argument("--max-lag", type=float, default=0.5)
    p.add_argument("--block", type=int, default=256)
    p.add_argument("--partitions", type=int, default=16)
    p.add_argument("--mu", type=float, default=0.5)
    p.add_argument("--lead-ms", type=float, default=4.0)
    p.add_argument("--converge", type=float, default=3.0, help="seconds skipped before ERLE")
    p.add_argument("--no-aec", action="store_true")
    p.add_argument("--drift", action="store_true")
    p.add_argument("--drift-win", type=float, default=4.0)
    p.add_argument("--drift-hop", type=float, default=10.0)
    p.add_argument("--coh-nperseg", type=int, default=8192, help="Welch segment for coherence")
    p.add_argument("--digital", help="the played WAV: also evaluate it as an oracle reference")
    p.set_defaults(fn=cmd_echo)
    a = ap.parse_args(argv)
    a.fn(a)


if __name__ == "__main__":
    sys.exit(main())
