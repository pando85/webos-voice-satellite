#!/usr/bin/env python3
"""Run AEC candidate binaries on REAL paired recordings (mic + reference tap) from
`tv_dualcap.py` captures and report ERLE in 200-4000 Hz. Device recordings stay in the
gitignored devices/private/ tree; only numbers are reported.

Mic = channel 0 (S24, 16 kHz); reference = mean of the 2 reference channels, resampled 48 -> 16 kHz.
The candidates get mic and reference *as captured* (no manual delay alignment): the delay
(13-19 ms on the SoundEngineCapture tap, 48-50 ms on MixerCapture) must be found by the
canceller itself, as it would have to be on the TV. `--ref-gain-db` scales the reference
before feeding it (WebRTC AEC3 ignores references it considers inactive, a level threshold
near -50 dBFS, so a quiet post-volume tap may need digital gain).

An oracle baseline is printed too: the 4096-tap PBFDAF NLMS of tools/audio-rig/analysis/tvcap.py
with the reference pre-aligned using GCC-PHAT, and the coherence-limited bound.

usage: real_eval.py --work-dir D --bin LABEL=PATH,MODE ... CAPDIR:START:END [CAPDIR:START:END ...]
"""
import argparse
import os
import re
import subprocess
import sys

import numpy as np
from scipy import signal
from scipy.io import wavfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "analysis"))
import tvcap  # noqa: E402

FS = 16000
BAND = (200.0, 4000.0)


def bp(x):
    sos = signal.butter(4, BAND, btype="bandpass", fs=FS, output="sos")
    return signal.sosfiltfilt(sos, x)


def active_mask(ref, margin_db=30.0, w=FS // 50):
    k = len(ref) // w
    p = np.mean(ref[: k * w].reshape(k, w) ** 2, axis=1)
    m = p > np.percentile(p, 95) * 10 ** (-margin_db / 10)
    return np.repeat(m, w)


def lag_of(out, mic, max_ms=100):
    n = min(len(out), len(mic))
    c = signal.fftconvolve(out[:n], mic[:n][::-1], mode="full")[n - 1 : n - 1 + int(max_ms * FS / 1000)]
    return int(np.argmax(c))


def erle(mic, out, ref, s0, s1):
    lag = lag_of(out[s0:s1], mic[s0:s1])
    out = np.concatenate([out[lag:], np.zeros(lag)])
    n = min(len(mic), len(out), len(ref))
    d, e, r = bp(mic[s0:s1]), bp(out[s0:s1]), ref[s0:s1]
    m = active_mask(r)
    m = m[: min(len(m), len(d))]
    d, e = d[: len(m)][m], e[: len(m)][m]
    return 10 * np.log10(np.mean(d**2) / np.mean(e**2)), lag * 1000.0 / FS


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--work-dir", required=True)
    ap.add_argument("--bin", action="append", required=True)
    ap.add_argument("--ref-gain-db", type=float, action="append")
    ap.add_argument("--converge-s", type=float, default=0.0, help="skip this many seconds at the start of each segment (also reported without)")
    ap.add_argument("caps", nargs="+", help="CAPDIR:START_S:END_S")
    a = ap.parse_args()
    os.makedirs(a.work_dir, exist_ok=True)
    gains = a.ref_gain_db or [0.0]
    for spec in a.caps:
        d, s, e = spec.rsplit(":", 2)
        s, e = float(s), float(e)
        cap = tvcap.load(d)
        mic = cap["mic"][:, 0]
        ref = tvcap.resample(tvcap.ref_mono(cap), cap["fs_ref"], FS)
        n = min(len(mic), len(ref))
        mic, ref = mic[:n], ref[:n]
        s0, s1 = int(s * FS), min(int(e * FS), n)
        name = os.path.basename(d.rstrip("/"))
        ref_db = 10 * np.log10(np.mean(ref[s0:s1] ** 2) + 1e-30)
        mic_db = 10 * np.log10(np.mean(mic[s0:s1] ** 2) + 1e-30)
        # oracle baselines
        dl, _ = tvcap.delay_estimate(mic[s0:s1], ref[s0:s1], FS, max_lag_s=0.3)
        ra = tvcap.align(ref[s0:s1], dl * FS - int(0.01 * FS), s1 - s0)
        err = tvcap.pbfdaf(mic[s0:s1], ra)
        ob = tvcap.erle_db(mic[s0:s1], err, FS, skip_s=2.0, band=BAND)
        coh = tvcap.coherence_stats(mic[s0:s1], tvcap.align(ref[s0:s1], dl * FS, s1 - s0), FS)
        print(f"\n## {name} segment {s:.1f}-{e:.1f} s: mic {mic_db:.1f} dBFS, ref {ref_db:.1f} dBFS, delay mic-ref {dl*1000:.1f} ms, "
              f"coherence {coh['mean_coherence']:.2f}, coherence bound {coh['erle_bound_db']:.1f} dB, oracle PBFDAF ERLE {ob:.1f} dB (after 2 s)")
        print("| engine | ref gain dB | ERLE all (200-4000) | ERLE after converge-skip | est. output lag ms |")
        print("|---|---|---|---|---|")
        for g in gains:
            rg = ref * 10 ** (g / 20)
            assert np.max(np.abs(rg)) < 1.0, "reference would clip; lower --ref-gain-db"
            mp, rp, op = (os.path.join(a.work_dir, f"{name}_{t}.wav") for t in ("mic", "ref", "out"))
            wavfile.write(mp, FS, mic.astype(np.float32))
            wavfile.write(rp, FS, rg.astype(np.float32))
            for bspec in a.bin:
                label, rest = bspec.split("=", 1)
                path, _, mode = rest.partition(",")
                r = subprocess.run([path, mp, rp, op, "--mode", mode or "aec"], capture_output=True, text=True)
                if r.returncode != 0:
                    print(f"| {label} | {g:+.0f} | FAILED: {r.stderr[-120:]!r} | | |")
                    continue
                out = wavfile.read(op)[1].astype(np.float64)
                e_all, lag = erle(mic, out, ref, s0, s1)
                e_c, _ = erle(mic, out, ref, s0 + int(a.converge_s * FS), s1) if a.converge_s else (float("nan"), 0)
                print(f"| {label} | {g:+.0f} | {e_all:.1f} | {e_c:.1f} | {lag:.0f} |")
                os.remove(op)


if __name__ == "__main__":
    main()
