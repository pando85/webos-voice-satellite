#!/usr/bin/env python3
"""Offline wake word detector that follows Home Assistant's Wyoming openWakeWord 2.1.0 server.

HA's openWakeWord add-on 2.1.0 runs `wyoming-openwakeword` 2.1.0, which uses `pyopen-wakeword`
1.x (TFLite models and a bundled `libtensorflowlite_c`, not the `openwakeword` package with
ONNX). This module repeats its handler loop (`wyoming_openwakeword/handler.py` at tag v2.1.0)
on a 16 kHz mono S16 stream:

- every audio stream starts with `OpenWakeWordFeatures.reset()` and `OpenWakeWord.reset()`
  (the features start with 8 s of zero "autofill" audio, as in the server);
- audio goes in chunks (1024 samples, the size `p2_digital.py` streamed to HA);
- a probability counts when it is strictly above the threshold (default 0.5); a detection
  fires after `trigger_level` such probabilities (default 1) and is timestamped with the
  audio time before the chunk that triggered it, as the server does;
- after a detection, further detections are suppressed for `refractory_s` (default 2 s;
  the server uses wall-clock time, here audio time, which is the same at real-time pace).

Install (in a separate venv; nothing is committed):
    pip install numpy scipy soundfile pyopen-wakeword==1.1.0

    oww_wyoming.py file.wav [--threshold 0.5] [--json]
"""
import argparse
import json
import sys

import numpy as np

RATE = 16000
CHUNK = 1024


class WyomingOww:
    """One Wyoming openWakeWord client session per `run` call (fresh state each time)."""

    def __init__(self, model="alexa", threshold=0.5, trigger_level=1, refractory_s=2.0):
        from pyopen_wakeword import Model, OpenWakeWord, OpenWakeWordFeatures
        self.features = OpenWakeWordFeatures.from_builtin()
        self.model = OpenWakeWord.from_builtin(Model(model))
        self.threshold = threshold
        self.trigger_level = trigger_level
        self.refractory_s = refractory_s

    def run(self, pcm16, chunk=CHUNK, first_only=False):
        """Streams `pcm16` (int16 array) as one audio stream. Returns (detections, max_score)
        where detections are dicts {"t": seconds, "score": probability}."""
        pcm16 = np.ascontiguousarray(pcm16, dtype=np.int16)
        self.features.reset()
        self.model.reset()
        triggers_left = self.trigger_level
        last = None
        ts_ms = 0.0
        dets = []
        max_score = 0.0
        for i in range(0, len(pcm16), chunk):
            block = pcm16[i:i + chunk]
            for feats in self.features.process_streaming(block.tobytes()):
                skip = last is not None and (ts_ms / 1000.0 - last) < self.refractory_s
                for prob in self.model.process_streaming(feats):
                    max_score = max(max_score, float(prob))
                    if skip or prob <= self.threshold:
                        continue
                    triggers_left -= 1
                    if triggers_left > 0:
                        continue
                    last = ts_ms / 1000.0
                    dets.append({"t": round(last, 3), "score": round(float(prob), 3)})
                    if first_only:
                        return dets, max_score
            ts_ms += len(block) * 1000.0 / RATE
        return dets, max_score

    def trace(self, pcm16, chunk=CHUNK, min_prob=0.1):
        """Streams `pcm16` as one audio stream and returns (trace, max_score): every model
        probability of at least `min_prob` as `(t, prob)`, with `t` the audio time before the
        chunk that produced it (the server's timestamp). `detect_from_trace` turns a trace into
        the detections `run` would give for any threshold of at least `min_prob`, trigger level
        and refractory time."""
        pcm16 = np.ascontiguousarray(pcm16, dtype=np.int16)
        self.features.reset()
        self.model.reset()
        ts_ms = 0.0
        out = []
        max_score = 0.0
        for i in range(0, len(pcm16), chunk):
            block = pcm16[i:i + chunk]
            for feats in self.features.process_streaming(block.tobytes()):
                for prob in self.model.process_streaming(feats):
                    p = float(prob)
                    max_score = max(max_score, p)
                    if p >= min_prob:
                        out.append((round(ts_ms / 1000.0, 4), round(p, 4)))
            ts_ms += len(block) * 1000.0 / RATE
        return out, max_score

    def run_rearm(self, pcm16, rearm_s=0.0, chunk=CHUNK):
        """Long-stream mode for false wakes: after each detection a new stream starts
        `rearm_s` later (HA ends the wake word stage on a detection; the satellite re-arms
        with a new run). Returns (detections with absolute times, max_score, armed_s)."""
        pos, dets, max_score, armed = 0, [], 0.0, 0
        n = len(pcm16)
        while pos < n:
            d, m = self.run(pcm16[pos:], chunk, first_only=True)
            max_score = max(max_score, m)
            if not d:
                armed += n - pos
                break
            t_abs = pos / RATE + d[0]["t"]
            dets.append({"t": round(t_abs, 3), "score": d[0]["score"]})
            end = int((t_abs + rearm_s) * RATE) + chunk
            armed += int(t_abs * RATE) - pos
            pos = max(end, pos + chunk)
        return dets, max_score, armed / RATE


def detect_from_trace(trace, threshold=0.5, trigger_level=1, refractory_s=2.0):
    """The detections (times in s) of the server loop for one audio stream, from a `trace`
    (see `WyomingOww.trace`): a probability counts when strictly above `threshold`; the
    `trigger_level`-th counting probability since the stream started fires a detection (the
    count is never reset within a stream, as in wyoming-openwakeword 2.1.0); probabilities
    less than `refractory_s` after a detection are skipped."""
    left = trigger_level
    last = None
    dets = []
    for t, p in trace:
        if last is not None and t - last < refractory_s:
            continue
        if p <= threshold:
            continue
        left -= 1
        if left > 0:
            continue
        last = t
        dets.append(t)
    return dets


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("wav")
    ap.add_argument("--model", default="alexa")
    ap.add_argument("--threshold", type=float, default=0.5)
    ap.add_argument("--trigger-level", type=int, default=1)
    ap.add_argument("--refractory-s", type=float, default=2.0)
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    import soundfile as sf
    x, sr = sf.read(a.wav, dtype="int16")
    if sr != RATE or x.ndim != 1:
        print("need a 16 kHz mono WAV", file=sys.stderr)
        return 2
    det = WyomingOww(a.model, a.threshold, a.trigger_level, a.refractory_s)
    dets, mx = det.run(x)
    res = {"input": a.wav, "seconds": round(len(x) / RATE, 2), "max_score": round(mx, 3), "detections": dets}
    print(json.dumps(res) if a.json else json.dumps(res, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
