#!/usr/bin/env python3
"""Generate the Phase 2 echo/AEC measurement stimuli (synthetic signals, nothing downloaded).

Output (default devices/private/stimuli/p2-v1/, gitignored; never commit audio), each as
<name>_48k_stereo.wav (PCM_16, 48 kHz, 2 ch, identical channels) and <name>_16k_mono.wav (PCM_16, 16 kHz, 1 ch;
the inverse filter is float32):
  sweep           10 s exponential (log) sweep 50 Hz-8 kHz, 20 ms fades (16 kHz variant: stops at 0.95 x Nyquist)
  sweep_inverse   matched inverse filter (Farina), 32-bit float WAV, analysis only: IR = conv(recording, inverse)
  pink_noise      30 s pink noise (1/f power), 20 Hz-Nyquist, seeded, 100 ms fades
  tone_1khz       5 s 1 kHz sine, -20 dBFS peak, 20 ms fades
  align_train     alignment train: 12 events (alternating 4 ms noise click and 80 ms 300 Hz-7 kHz chirp) with
                  irregular spacing, 1 s silence before and after; event times are in manifest.json
manifest.json     per file: parameters, duration, peak/RMS dBFS, event times, sha256

Levels: all files peak at -6 dBFS except tone_1khz (-20 dBFS), as asked for the echo measurement. Note that
play_record.py rescales whatever it plays so that the file PEAK is -30 dBFS + gain; use --gain-db accordingly
(for the tone, +10 dB gain gives -20 dBFS, as does any other file with the same peak target).
Generated with a fixed seed; the files are bit-reproducible with the same numpy/scipy versions.
"""
import argparse
import datetime
import hashlib
import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import fftconvolve

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = ROOT / "devices" / "private" / "stimuli" / "p2-v1"
PEAK_DBFS = -6.0
TONE_DBFS = -20.0
SEED = 20261102
FS48, FS16 = 48000, 16000


def fade(x, fs, ms):
    n = min(int(fs * ms / 1000), len(x) // 2)
    w = 0.5 - 0.5 * np.cos(np.pi * np.arange(n) / n)
    x = x.copy()
    x[:n] *= w
    x[len(x) - n:] *= w[::-1]
    return x


def scale_peak(x, dbfs):
    return x * (10 ** (dbfs / 20) / np.max(np.abs(x)))


def dbfs(v):
    return round(float(20 * np.log10(max(v, 1e-12))), 2)


def sweep(fs, dur=10.0, f1=50.0, f2_req=8000.0):
    f2 = min(f2_req, 0.95 * fs / 2)
    t = np.arange(int(dur * fs)) / fs
    k = np.log(f2 / f1)
    x = fade(np.sin(2 * np.pi * f1 * dur / k * (np.exp(t * k / dur) - 1)), fs, 20)
    inv = x[::-1] * np.exp(-t * k / dur)  # time reversal with -6 dB/octave amplitude correction
    conv = fftconvolve(x, inv)
    inv = inv / np.max(np.abs(conv))  # unit-peak impulse after convolution
    x = scale_peak(x, PEAK_DBFS)
    return x, inv, {"f1_hz": f1, "f2_hz": f2, "duration_s": dur}


def pink(fs, dur=30.0, rng=None):
    n = int(dur * fs)
    spec = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1 / fs)
    g = np.zeros_like(f)
    g[f >= 20] = 1 / np.sqrt(f[f >= 20])  # 1/f power -> 1/sqrt(f) amplitude
    x = np.fft.irfft(spec * g, n)
    return scale_peak(fade(x, fs, 100), PEAK_DBFS)


def tone(fs, dur=5.0, f=1000.0):
    t = np.arange(int(dur * fs)) / fs
    return scale_peak(fade(np.sin(2 * np.pi * f * t), fs, 20), TONE_DBFS)


def align_train(fs, rng):
    gaps = [1.0, 1.3, 0.9, 1.7, 1.1, 1.5, 0.8, 1.9, 1.2, 1.4, 1.0]  # irregular -> unambiguous alignment
    click = fade(rng.standard_normal(int(0.004 * fs)), fs, 2)
    n = int(0.08 * fs)
    t = np.arange(n) / fs
    f0, f1 = 300.0, min(7000.0, 0.95 * fs / 2)
    chirp = fade(np.sin(2 * np.pi * (f0 * t + (f1 - f0) / (2 * 0.08) * t ** 2)), fs, 8)
    events = [click if i % 2 == 0 else chirp for i in range(len(gaps) + 1)]
    names = ["click" if i % 2 == 0 else "chirp" for i in range(len(gaps) + 1)]
    pos = int(1.0 * fs)
    total = pos + int(sum(gaps) * fs) + len(chirp) + int(1.0 * fs)
    x = np.zeros(total)
    times = []
    for i, e in enumerate(events):
        x[pos:pos + len(e)] += e / np.max(np.abs(e))
        times.append({"event": names[i], "start_s": round(pos / fs, 4), "duration_s": round(len(e) / fs, 4)})
        if i < len(gaps):
            pos += int(gaps[i] * fs)
    return scale_peak(x, PEAK_DBFS), times


def write(out, name, x48, x16, meta, manifest, subtype="PCM_16"):
    for suffix, x, fs, ch in (("48k_stereo", x48, FS48, 2), ("16k_mono", x16, FS16, 1)):
        data = np.stack([x] * ch, axis=1) if ch > 1 else x
        p = out / f"{name}_{suffix}.wav"
        sf.write(str(p), data, fs, subtype=subtype)
        r, _ = sf.read(str(p), dtype="float64", always_2d=True)
        manifest["files"].append({
            "file": p.name, "signal": name, "sample_rate": fs, "channels": ch,
            "duration_s": round(len(x) / fs, 3), "peak_dbfs": dbfs(np.max(np.abs(r))),
            "rms_dbfs": dbfs(np.sqrt(np.mean(r[:, 0] ** 2))), "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
            **meta})


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = ap.parse_args()
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(SEED)
    man = {"stimuli": "p2-v1", "generator": "tools/audio-rig/make_p2_stimuli.py", "seed": SEED,
           "generated_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
           "files": []}

    s48, i48, m48 = sweep(FS48)
    s16, i16, m16 = sweep(FS16)
    write(out, "sweep", s48, s16, {"description": "exponential sweep 50 Hz-8 kHz, 10 s (16 kHz variant ends at "
                                   f"{m16['f2_hz']:.0f} Hz)"}, man)
    write(out, "sweep_inverse", i48, i16, {"description": "Farina inverse filter of the sweep with the same "
                                           "sample rate; stored as 32-bit float (its peak is about -90 dBFS, too small for PCM_16); "
                                           "conv(sweep, inverse) has a unit-peak impulse; analysis only, do not play"},
          man, subtype="FLOAT")
    write(out, "pink_noise", pink(FS48, rng=rng), pink(FS16, rng=rng),
          {"description": "pink noise 30 s, 20 Hz-Nyquist, independent noise per variant"}, man)
    write(out, "tone_1khz", tone(FS48), tone(FS16), {"description": "1 kHz sine 5 s at -20 dBFS peak"}, man)
    a48, ev48 = align_train(FS48, rng)
    a16, ev16 = align_train(FS16, rng)
    write(out, "align_train", a48, a16, {"description": "alternating 4 ms click and 80 ms 300 Hz-7 kHz chirp"}, man)
    man["align_train_events_48k"] = ev48
    man["align_train_events_16k"] = ev16
    (out / "manifest.json").write_text(json.dumps(man, indent=2) + "\n", encoding="utf-8")
    for f in man["files"]:
        print(f"{f['file']:28s} {f['duration_s']:7.2f} s  peak {f['peak_dbfs']:6.2f}  rms {f['rms_dbfs']:6.2f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
