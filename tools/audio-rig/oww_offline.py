#!/usr/bin/env python3
"""Offline openWakeWord check of TV-mic recordings, with the satellite's front-end emulated.

Runs the python `openwakeword` package (0.6.x, ONNX) with one model (default `alexa_v0.1`)
over a recording and prints every detection (score >= threshold, with a refractory time so one
utterance counts once). The input is what `tv_dualcap_demux.py` writes (`mic.raw`, S24_LE in
4-byte containers, 2 ch, 16 kHz) or a 16 kHz WAV. By default the satellite's front-end is
applied first, as in `crates/dsp`: channel 0, 2nd-order Butterworth high-pass at 120 Hz,
+24 dB, hard saturation to S16. This approximates the audio HA's openWakeWord receives; it is
not the same server (HA runs the Wyoming openWakeWord service), so treat it as a baseline.

    oww_offline.py capdir/mic.raw --models-dir <dir with alexa_v0.1.onnx and the feature models> \
        [--gain-db 24] [--raw-input] [--manifest timeline.json] [--json]

Install in a separate venv (tflite-runtime has no wheel for new Pythons):
    pip install onnxruntime soundfile scipy tqdm requests scikit-learn
    pip install --no-deps openwakeword==0.6.0
    python -c "from openwakeword.utils import download_models as d; d(['alexa'], target_directory='<dir>')"

With `--manifest` (a JSON list of {"id", "kind", "t_wake_start", "t_wake_end"} in seconds of the
recording), each positive is scored with the P2-corpus window (-0.2 s / +1.5 s) and the rest
of the detections are counted as false wakes.
"""
import argparse
import json
import math
import os
import sys

import numpy as np

RATE = 16000
FRAME = 1280  # 80 ms, openWakeWord's step


def load(path, channels, channel):
    if path.endswith(".wav"):
        import soundfile as sf
        x, sr = sf.read(path, dtype="int32", always_2d=True)
        assert sr == RATE, "need 16 kHz"
        return x[:, channel].astype(np.float64) / 2 ** 31 * 2 ** 23  # as S24 sample values
    raw = np.fromfile(path, dtype="<i4")
    raw = raw[: len(raw) // channels * channels].reshape(-1, channels)
    # ALSA S24_LE: 24-bit value in the low 3 bytes of a 4-byte container, sign in bit 23
    v = raw[:, channel] & 0xFFFFFF
    v = np.where(v & 0x800000, v - 0x1000000, v)
    return v.astype(np.float64)


def frontend(s24, gain_db=24.0, hpf_hz=120.0):
    """Emulates crates/dsp: HPF (RBJ Butterworth biquad), gain, S24 -> S16 with saturation."""
    from scipy.signal import lfilter
    x = s24
    if hpf_hz > 0:
        w0 = 2 * math.pi * hpf_hz / RATE
        q = 1 / math.sqrt(2)
        alpha = math.sin(w0) / (2 * q)
        c = math.cos(w0)
        b = np.array([(1 + c) / 2, -(1 + c), (1 + c) / 2])
        a = np.array([1 + alpha, -2 * c, 1 - alpha])
        x = lfilter(b / a[0], a / a[0], x)
    y = x / 256.0 * 10 ** (gain_db / 20)
    clipped = int(np.sum(np.abs(y) > 32767))
    return np.clip(np.round(y), -32768, 32767).astype(np.int16), clipped


def detect(pcm16, models_dir, model="alexa_v0.1", threshold=0.5, refractory=1.5):
    from openwakeword.model import Model
    m = Model(wakeword_models=[os.path.join(models_dir, model + ".onnx")], inference_framework="onnx",
              melspec_model_path=os.path.join(models_dir, "melspectrogram.onnx"),
              embedding_model_path=os.path.join(models_dir, "embedding_model.onnx"))
    dets, last = [], -1e9
    maxscore = 0.0
    for i in range(0, len(pcm16) - FRAME + 1, FRAME):
        sc = m.predict(pcm16[i:i + FRAME])[model]
        maxscore = max(maxscore, float(sc))
        t = (i + FRAME) / RATE
        if sc >= threshold and t - last >= refractory:
            dets.append({"t": round(t, 3), "score": round(float(sc), 3)})
            last = t
    return dets, maxscore


def score(dets, manifest, pre=0.2, post=1.5):
    used = set()
    pos = [m for m in manifest if m["kind"] == "pos"]
    hit = 0
    for m in pos:
        for j, d in enumerate(dets):
            if j not in used and m["t_wake_start"] - pre <= d["t"] <= m["t_wake_end"] + post:
                used.add(j)
                hit += 1
                break
    return {"positives": len(pos), "detected": hit, "false_wakes": len(dets) - len(used)}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("input")
    ap.add_argument("--models-dir", required=True)
    ap.add_argument("--model", default="alexa_v0.1")
    ap.add_argument("--channels", type=int, default=2)
    ap.add_argument("--channel", type=int, default=0)
    ap.add_argument("--gain-db", type=float, default=24.0)
    ap.add_argument("--hpf-hz", type=float, default=120.0)
    ap.add_argument("--skip-s", type=float, default=0.25, help="startup burst to drop")
    ap.add_argument("--threshold", type=float, default=0.5)
    ap.add_argument("--manifest")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    s24 = load(a.input, a.channels, a.channel)[int(a.skip_s * RATE):]
    pcm, clipped = frontend(s24, a.gain_db, a.hpf_hz)
    dets, maxscore = detect(pcm, a.models_dir, a.model, a.threshold)
    for d in dets:
        d["t"] = round(d["t"] + a.skip_s, 3)
    res = {"input": os.path.basename(a.input), "seconds": round(len(s24) / RATE + a.skip_s, 1),
           "clipped_samples": clipped, "clipped_share": round(clipped / max(len(pcm), 1), 5),
           "rms_dbfs_after_frontend": round(20 * math.log10(max(np.sqrt(np.mean(pcm.astype(np.float64) ** 2)), 1e-9)
                                                           / 32768), 1),
           "max_score": round(maxscore, 3), "detections": dets}
    if a.manifest:
        res["score"] = score(dets, json.load(open(a.manifest)))
    print(json.dumps(res) if a.json else json.dumps(res, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
