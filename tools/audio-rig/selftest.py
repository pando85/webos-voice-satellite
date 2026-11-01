#!/usr/bin/env python3
"""Offline self-test of analyze.py with synthetic recordings (no audio hardware, no playback).

Builds multi-channel recordings (stimulus + fake room + noise on ch0, noise only on ch1,
digital silence on ch2) in several rates/formats, runs analyze.py and checks the verdicts
and delay estimate. Requires the stimuli from make_stimuli.py.
"""
import json
import subprocess
import sys
import tempfile
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

HERE = Path(__file__).resolve().parent
STIM = HERE.parents[1] / "artifacts" / "audio-rig" / "stimuli"
DELAY_S = 0.0373
LEAD_S = 1.5


def make(ref48, fs, snr_db, rng):
    g = gcd(48000, fs)
    ref = resample_poly(ref48, fs // g, 48000 // g)
    n = int((LEAD_S + len(ref) / fs + 1.0) * fs)
    ch0 = np.zeros(n)
    start = int((LEAD_S + DELAY_S) * fs)
    ch0[start: start + len(ref)] += ref
    ir = np.zeros(int(0.03 * fs)); ir[0] = 1; ir[int(0.011 * fs)] = 0.4; ir[int(0.027 * fs)] = -0.2
    ch0 = np.convolve(ch0, ir)[:n]
    sig_rms = np.sqrt(np.mean(ref ** 2))
    if snr_db is None:  # digital loopback: perfect copy, no room; ch1 is very quiet noise only
        ch0 = np.zeros(n)
        ch0[start: start + len(ref)] = ref
        noise = lambda: rng.standard_normal(n) * 1e-5
        return np.stack([ch0, noise(), np.zeros(n)], 1), fs
    sigma = sig_rms / 10 ** (snr_db / 20)
    noise = lambda: rng.standard_normal(n) * sigma
    return np.stack([ch0 + noise(), noise(), np.zeros(n)], 1), fs


def run(args):
    out = subprocess.run([sys.executable, str(HERE / "analyze.py"), *args, "--quiet"], capture_output=True, text=True)
    return out.returncode


def main():
    rng = np.random.default_rng(1)
    fails = 0
    with tempfile.TemporaryDirectory() as d:
        d = Path(d)
        cases = [("sweep", 16000, "wav", None), ("sweep", 44100, "wav", None), ("pilots", 16000, "wav", None),
                 ("noise", 48000, "wav", None), ("speech_full", 16000, "raw", "S16_LE"),
                 ("speech_full", 32000, "raw", "S32_LE"), ("sweep", 16000, "raw", "S24_3LE"),
                 ("sweep", 16000, "raw", "F32_LE")]
        cases += [(k, 48000, "wav", "loopback") for k in ("sweep", "pilots", "noise", "speech_wake", "speech_full")]
        for name, fs, kind, fmt in cases:
            ref48, _ = sf.read(STIM / f"{name}_48k.wav")
            loop = fmt == "loopback"
            x, _ = make(ref48, fs, snr_db=None if loop else 0, rng=rng)  # 0 dB wideband SNR: deliberately modest
            x *= 0.1 / np.abs(x).max()
            fmt = None if loop else fmt
            path = d / f"{name}_{fs}.{ 'wav' if kind == 'wav' else 'raw'}"
            extra = []
            if kind == "wav":
                sf.write(path, x, fs, subtype="PCM_32")
            else:
                extra = ["--raw", "--rate", str(fs), "--channels", "3", "--format", fmt]
                if fmt == "S16_LE":
                    path.write_bytes((x * 32767).astype("<i2").tobytes())
                elif fmt == "S32_LE":
                    path.write_bytes((x * 2147483647).astype("<i4").tobytes())
                elif fmt == "F32_LE":
                    path.write_bytes(x.astype("<f4").tobytes())
                else:
                    v = (x * 8388607).astype(np.int32).reshape(-1)
                    path.write_bytes(np.stack([(v >> s) & 0xFF for s in (0, 8, 16)], 1).astype("u1").tobytes())
            rc = run([str(path), "--stimulus", name, "--leadin", str(LEAD_S - 0.2), "--out-dir", str(d / "o"), *extra,
                      "--stim-rate", "48k"])
            js = json.loads((d / "o" / f"{path.stem}__analysis.json").read_text())["results"]
            ok = (js[0]["detected"] and not js[1]["detected"] and not js[2]["detected"]
                  and abs(js[0]["start_ms"] - (LEAD_S + DELAY_S) * 1000) < (5.0 if name == "pilots" else 1.0))
            fails += not ok
            print(f"{'PASS' if ok else 'FAIL'} {name:12} {fs:>6} Hz {kind}{'/' + fmt if fmt else ('/loopback' if loop else '')}: "
                  f"ch0 score {js[0]['score_db']:.1f} dB start {js[0]['start_ms']:.2f} ms | "
                  f"ch1 score {js[1]['score_db']:.1f} dB ({js[1]['verdict']}) | ch2 {js[2]['verdict']}")
    print("selftest:", "OK" if not fails else f"{fails} FAILED")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
