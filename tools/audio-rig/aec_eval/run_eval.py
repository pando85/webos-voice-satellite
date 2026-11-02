#!/usr/bin/env python3
"""Run AEC candidate binaries over synthetic scenarios and report ERLE / near-end distortion / RTF.

SYNTHETIC results: they say how the algorithms behave on a modelled echo path, not how they
behave on a TV. Candidate binaries are the throwaway `bench-*` programs from ./bench (built by
bench/build.sh); each reads `<mic.wav> <ref.wav> <out.wav>`
(16 kHz mono, f32 or s16) and prints a RESULT line on stderr.

usage: run_eval.py --scen-dir DIR --work-dir DIR --bin LABEL=PATH[,MODE] ... [--repeat N] [--json OUT]
"""
import argparse
import json
import os
import re
import subprocess
import sys

import numpy as np
from scipy import signal
from scipy.io import wavfile

FS = 16000
BAND = (200.0, 4000.0)


def rd(p):
    fs, x = wavfile.read(p)
    assert fs == FS
    return x.astype(np.float64) / (32768.0 if x.dtype.kind == "i" else 1.0)


def bp(x, band=BAND):
    sos = signal.butter(4, band, btype="bandpass", fs=FS, output="sos")
    return signal.sosfiltfilt(sos, x)


def pw(x):
    return float(np.mean(np.asarray(x) ** 2) + 1e-30)


def db(p):
    return 10 * np.log10(p)


def seg(x, a, b):
    return x[int(a * FS) : int(b * FS)]


def lag_of(out, ref_sig, max_ms=250):
    """Samples by which `out` lags `ref_sig` (>= 0), by cross-correlation."""
    n = min(len(out), len(ref_sig))
    L = int(max_ms * FS / 1000)
    c = signal.fftconvolve(out[:n], ref_sig[:n][::-1], mode="full")
    mid = n - 1
    c = c[mid : mid + L]
    return int(np.argmax(np.abs(c)))


def coh_decomp(out, near, nperseg=1024):
    """Split `out` into the part linearly explained by `near` and the rest, per frequency bin
    (magnitude-squared coherence, 200-4000 Hz, speech-energy weighted).

    Unlike a single least-squares gain this ignores benign linear filtering (a high-pass's
    phase shift, a fixed delay) and counts as "unexplained" whatever is not a linear time-invariant
    function of the near-end speech: residual echo, noise, nonlinear or time-varying suppression.
    Returns (gain_db, sdr_db, unexplained_variance): gain = explained out power / near power,
    sdr = explained / unexplained out power.
    """
    n = min(len(out), len(near))
    f, pxx = signal.welch(near[:n], fs=FS, nperseg=nperseg)
    _, pyy = signal.welch(out[:n], fs=FS, nperseg=nperseg)
    _, pxy = signal.csd(near[:n], out[:n], fs=FS, nperseg=nperseg)
    sel = (f >= BAND[0]) & (f <= BAND[1])
    coh = np.abs(pxy) ** 2 / (pxx * pyy + 1e-40)
    expl = float(np.sum(pyy[sel] * coh[sel]))
    unex = float(np.sum(pyy[sel] * (1 - coh[sel]))) + 1e-30
    df = f[1] - f[0]  # PSD sum -> variance
    return db(expl / float(np.sum(pxx[sel]))), db(expl / unex), unex * df


def metrics(mic, out, near, echo, ph):
    n = min(len(mic), len(out), len(near), len(echo))
    mic, out, near, echo = mic[:n], out[:n], near[:n], echo[:n]
    # align out to the mic time base using the near-only phase (clean near-end speech dominates)
    a, b = ph["C"]
    lag = lag_of(seg(out, a, b), seg(near, a, b))
    if lag:
        out = np.concatenate([out[lag:], np.zeros(lag)])
    m = {"lag_ms": 1000.0 * lag / FS}
    A0, A1 = ph["A"]
    # ERLE far-end only, converged (last 7 s of phase A) and early (first 5 s), 200-4000 Hz
    m["erle_early_db"] = db(pw(bp(seg(mic, A0 + 0.5, A0 + 5)))) - db(pw(bp(seg(out, A0 + 0.5, A0 + 5))))
    m["erle_ss_db"] = db(pw(bp(seg(mic, A1 - 7, A1)))) - db(pw(bp(seg(out, A1 - 7, A1))))
    # ceiling set by the noise floor present in mic: (echo + noise) / noise
    noise_floor = pw(bp(seg(mic - echo - near, A1 - 7, A1)))
    m["erle_ceiling_db"] = db(pw(bp(seg(echo, A1 - 7, A1))) + noise_floor) - db(noise_floor)
    # double talk (B): echo power vs what is left of `out` that is not near-end speech
    B0, B1 = ph["B"]
    g, sdr, unex = coh_decomp(seg(out, B0 + 2, B1), seg(near, B0 + 2, B1))
    m["dt_erle_db"] = db(pw(bp(seg(echo, B0 + 2, B1)))) - db(unex)
    m["dt_near_gain_db"], m["dt_sdr_db"] = g, sdr
    g, sdr, _ = coh_decomp(seg(mic, B0 + 2, B1), seg(near, B0 + 2, B1))
    m["dt_sdr_mic_db"] = sdr
    # near-only (C): what the processing does to speech when there is nothing to cancel
    C0, C1 = ph["C"]
    m["c_near_gain_db"], m["c_sdr_db"], _ = coh_decomp(seg(out, C0 + 0.5, C1 - 0.5), seg(near, C0 + 0.5, C1 - 0.5))
    _, m["c_sdr_mic_db"], _ = coh_decomp(seg(mic, C0 + 0.5, C1 - 0.5), seg(near, C0 + 0.5, C1 - 0.5))
    m["c_noise_floor_change_db"] = db(pw(bp(seg(out, C1 - 0.4, C1)))) - db(pw(bp(seg(mic, C1 - 0.4, C1))))
    return m


def run_bin(path, mode, mic, ref, outp, repeat):
    cmd = [path, mic, ref, outp, "--mode", mode, "--repeat", str(repeat)]
    r = subprocess.run(cmd, capture_output=True, text=True)
    mm = re.search(r"RESULT (.*)", r.stderr)
    kv = dict(x.split("=", 1) for x in mm.group(1).split()) if mm else {}
    if r.returncode != 0 or not mm:
        raise RuntimeError(f"{cmd} failed: {r.stderr[-500:]}")
    return kv


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--scen-dir", required=True)
    ap.add_argument("--work-dir", required=True)
    ap.add_argument("--bin", action="append", required=True, help="LABEL=PATH[,MODE]")
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--json")
    a = ap.parse_args()
    os.makedirs(a.work_dir, exist_ok=True)
    names = sorted(f[:-5] for f in os.listdir(a.scen_dir) if f.endswith(".json"))
    res = {}
    for nm in names:
        meta = json.load(open(os.path.join(a.scen_dir, nm + ".json")))
        ph = {k: tuple(v) for k, v in meta["phases"].items()}
        mic, ref = (os.path.join(a.scen_dir, f"{nm}_{t}.wav") for t in ("mic", "ref"))
        near, echo = rd(os.path.join(a.scen_dir, nm + "_near.wav")), rd(os.path.join(a.scen_dir, nm + "_echo.wav"))
        micx = rd(mic)
        res[nm] = {"meta": {k: meta[k] for k in ("delay_ms", "drift_ppm", "nonlinear", "ref_source")}}
        res[nm]["mic_only"] = metrics(micx, micx, near, echo, ph)
        for spec in a.bin:
            label, rest = spec.split("=", 1)
            path, _, mode = rest.partition(",")
            outp = os.path.join(a.work_dir, f"{nm}__{label}.wav")
            kv = run_bin(path, mode or "aec", mic, ref, outp, a.repeat)
            m = metrics(micx, rd(outp), near, echo, ph)
            m["rtf"] = float(kv["rtf_best"])
            res[nm][label] = m
            os.remove(outp)
    if a.json:
        json.dump(res, open(a.json, "w"), indent=1)
    cols = [("erle_early_db", "ERLE 0.5-5s"), ("erle_ss_db", "ERLE ss"), ("erle_ceiling_db", "ceil"), ("dt_erle_db", "DT ERLE"),
            ("dt_near_gain_db", "DT gain"), ("dt_sdr_db", "DT SDR"), ("c_sdr_db", "C SDR"), ("c_near_gain_db", "C gain"), ("rtf", "RTF")]
    for nm, d in res.items():
        print(f"\n## {nm}  {d['meta']}")
        print("| engine | " + " | ".join(c[1] for c in cols) + " | lag ms |")
        print("|---|" + "---|" * (len(cols) + 1))
        for label, m in d.items():
            if label == "meta":
                continue
            print(f"| {label} | " + " | ".join((f"{m[c]:.4f}" if c == "rtf" else f"{m[c]:.1f}") if c in m else "" for c, _ in cols) + f" | {m['lag_ms']:.0f} |")


if __name__ == "__main__":
    main()
