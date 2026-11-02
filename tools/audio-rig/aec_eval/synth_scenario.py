#!/usr/bin/env python3
"""Synthetic echo scenarios for offline AEC comparison (SYNTHETIC, not device evidence).

Builds, per scenario, 16 kHz mono float32 WAVs in --out-dir:
  <name>_ref.wav   far-end reference (what the TV "plays", e.g. the SoundEngineCapture tap)
  <name>_mic.wav   microphone = echo (reference through a room response, delay, clock drift,
                   optional speaker nonlinearity) + near-end speech + background noise
  <name>_near.wav  clean near-end speech (ground truth for distortion metrics)
  <name>_echo.wav  clean echo component (ground truth for ERLE during double talk)
  <name>.json      scenario parameters and phase boundaries

Timeline (40 s): A 0-15 s far-end only; B 15-30 s far-end + near-end speech (double talk);
C 30-40 s near-end only (reference silent).

Inputs are read from the gitignored devices/private/ tree (content/p2-v1, corpus/p2-v1,
stimuli/p2-v1); nothing is written there. Needs numpy + scipy.
"""
import argparse
import csv
import json
import os

import numpy as np
from scipy import signal
from scipy.interpolate import CubicSpline
from scipy.io import wavfile

FS = 16000
DUR = 40.0
PHASES = {"A": (0.0, 15.0), "B": (15.0, 30.0), "C": (30.0, 40.0)}


def read_mono16k(path, start_s=0.0, dur_s=None):
    fs, x = wavfile.read(path)
    x = x.astype(np.float64)
    if x.dtype.kind != "f":
        pass
    if np.max(np.abs(x)) > 2.0:
        x = x / 32768.0
    if x.ndim == 2:
        x = x.mean(axis=1)
    if fs != FS:
        g = np.gcd(fs, FS)
        x = signal.resample_poly(x, FS // g, fs // g)
    s = int(start_s * FS)
    e = len(x) if dur_s is None else s + int(dur_s * FS)
    return x[s:e]


def rms_active(x, margin_db=25.0):
    """RMS over 20 ms frames within `margin_db` of the 95th-percentile frame power."""
    x = np.asarray(x)
    w = FS // 50
    k = len(x) // w
    p = np.mean(x[: k * w].reshape(k, w) ** 2, axis=1)
    m = p > np.percentile(p, 95) * 10 ** (-margin_db / 10)
    return float(np.sqrt(np.mean(p[m]))) if m.any() else float(np.sqrt(np.mean(p)))


def set_active_rms(x, target_dbfs):
    return x * (10 ** (target_dbfs / 20) / max(rms_active(x), 1e-12))


def make_rir(rng, delay_ms, rt_scale=1.0):
    """Plausible TV-to-mic response: dominant direct path (~78 % of energy), ~10 % in 5-50 ms,
    ~8 % in 50-250 ms (the split measured on c3-a in P2 session 1), plus speaker/mic coloring."""
    n = int(0.45 * FS)
    h = np.zeros(n)
    d0 = int(round(delay_ms * FS / 1000))
    h[d0] = 1.0
    # a few close reflections 1-5 ms
    for k in range(3):
        h[d0 + int(rng.uniform(0.001, 0.005) * FS)] += rng.uniform(-0.12, 0.12)

    def tail(t0, t1, tau, energy):
        i0, i1 = d0 + int(t0 * FS), min(n, d0 + int(t1 * FS))
        t = np.arange(i1 - i0) / FS
        x = rng.standard_normal(i1 - i0) * np.exp(-t / (tau * rt_scale))
        x *= np.sqrt(energy / np.sum(x**2))
        h[i0:i1] += x

    tail(0.005, 0.05, 0.020, 0.13)
    tail(0.05, 0.25, 0.080, 0.10)
    # speaker + small-mic coloring: high-pass ~150 Hz, gentle lift above 3 kHz
    sos = signal.butter(2, 150, "highpass", fs=FS, output="sos")
    h = signal.sosfilt(sos, h)
    b, a = signal.iirpeak(4500, 0.7, fs=FS)
    h = h + 0.5 * signal.lfilter(b, a, h)
    return h / np.sqrt(np.sum(h**2))  # unit power gain


def delay_and_drift(x, ppm, n_out):
    """echo[n] = x(n * (1 - ppm*1e-6)); ppm > 0 means the capture clock runs faster."""
    if ppm == 0:
        return x[:n_out]
    t = np.arange(n_out) * (1.0 - ppm * 1e-6)
    cs = CubicSpline(np.arange(len(x)), x)
    return cs(np.clip(t, 0, len(x) - 1))


def near_track(root, rng, n):
    """Near-end speech: TTS corpus utterances (positives and neutral) with pauses, phases B and C."""
    items = []
    with open(os.path.join(root, "corpus/p2-v1/manifest.csv")) as f:
        for r in csv.DictReader(f):
            if r["kind"] in ("pos", "nearmiss"):
                items.append(r)
    rng.shuffle(items)
    out = np.zeros(n)
    t = PHASES["B"][0] + 0.5
    for it in items:
        x = read_mono16k(os.path.join(root, "corpus/p2-v1", it["file_16k_mono"]))
        s = int(t * FS)
        if s + len(x) >= n - FS // 2:
            continue
        out[s : s + len(x)] += x
        t += len(x) / FS + rng.uniform(0.3, 0.9)
    return out


def build(root, name, ref_src, delay_ms, ppm, nonlinear, rt_scale, seed, out_dir, ref_level=-33.0, near_level=-43.0, noise_level=-66.0):
    rng = np.random.default_rng(seed)
    n = int(DUR * FS)
    kind, path, start = ref_src
    if kind == "pink":
        ref = read_mono16k(path, 0, DUR + 1)
    else:
        ref = read_mono16k(path, start, DUR + 1)
    if len(ref) < n + FS:
        ref = np.tile(ref, int(np.ceil((n + FS) / len(ref))))
    ref = ref[: n + FS]
    ref = set_active_rms(ref, ref_level)
    ref[int(PHASES["C"][0] * FS) :] = 0.0  # reference silent in phase C
    ref = ref[:n]
    ref_pl = ref.copy()
    if nonlinear:
        # mild speaker compression/clipping: tanh at a drive that gives roughly -25 dBc distortion
        pk = 10 ** ((ref_level + 12) / 20)
        ref_pl = pk * np.tanh(ref_pl / pk * 1.5) / 1.5
    h = make_rir(rng, delay_ms, rt_scale)
    echo = signal.fftconvolve(ref_pl, h)[: n + FS]
    echo = delay_and_drift(echo, ppm, n)
    near = near_track(root, rng, n)
    near = set_active_rms(near, near_level)
    noise = signal.lfilter([1], [1, -0.6], rng.standard_normal(n))  # slightly tilted room noise
    noise = noise / np.sqrt(np.mean(noise**2)) * 10 ** (noise_level / 20)
    mic = echo + near + noise
    peak = max(np.max(np.abs(mic)), np.max(np.abs(ref)))
    assert peak < 0.95, peak
    for tag, x in (("ref", ref), ("mic", mic), ("near", near), ("echo", echo)):
        wavfile.write(os.path.join(out_dir, f"{name}_{tag}.wav"), FS, x.astype(np.float32))
    meta = dict(name=name, delay_ms=delay_ms, drift_ppm=ppm, nonlinear=nonlinear, rt_scale=rt_scale, seed=seed,
                ref_active_rms_dbfs=ref_level, near_active_rms_dbfs=near_level, noise_rms_dbfs=noise_level,
                phases=PHASES, ref_source=os.path.basename(path))
    with open(os.path.join(out_dir, name + ".json"), "w") as f:
        json.dump(meta, f, indent=1)
    return meta


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", default="devices/private", help="private data root (default devices/private)")
    ap.add_argument("--out-dir", required=True)
    a = ap.parse_args()
    os.makedirs(a.out_dir, exist_ok=True)
    c = os.path.join(a.root, "content/p2-v1")
    speech = ("film", os.path.join(c, "speech_en__tears_of_steel_dialogue.wav"), 30.0)
    music = ("film", os.path.join(c, "music_vocals__woodward_effortless.wav"), 20.0)
    pink = ("pink", os.path.join(a.root, "stimuli/p2-v1/pink_noise_16k_mono.wav"), 0.0)
    scen = [
        ("s1_speech_d17", speech, 17, 0, False, 1.0),
        ("s2_speech_d17_drift100", speech, 17, 100, False, 1.0),
        ("s3_speech_d80", speech, 80, 0, False, 1.0),
        ("s4_music_d40", music, 40, 0, False, 1.0),
        ("s5_pink_d40_drift200", pink, 40, 200, False, 1.0),
        ("s6_speech_d40_nonlinear", speech, 40, 0, True, 1.0),
    ]
    for i, (nm, src, d, ppm, nl, rt) in enumerate(scen):
        m = build(a.root, nm, src, d, ppm, nl, rt, 1000 + i, a.out_dir)
        print(nm, "ok")


if __name__ == "__main__":
    main()
