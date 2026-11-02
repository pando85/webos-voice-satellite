#!/usr/bin/env python3
"""Offline tests for tvcap.py and tv_dualcap_demux.py (synthetic data only).

Run with the rig venv (numpy, scipy, soundfile):
  .venv/bin/python -m unittest discover -s tools/audio-rig/analysis -v
"""
import io
import json
import os
import struct
import sys
import tempfile
import unittest

import numpy as np
from scipy import signal

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))

import tv_dualcap_demux
import tvcap

FS = 16000


def echo_fixture(seconds=20, delay=22, seed=1):
    rng = np.random.default_rng(seed)
    ref = signal.lfilter([1], [1, -0.9], rng.standard_normal(FS * seconds)) * 0.01
    t = np.arange(int(0.2 * FS))
    h = rng.standard_normal(len(t)) * np.exp(-t / (0.04 * FS)) * 0.3
    h[:delay] = 0
    h[delay] = 3.0
    mic = signal.lfilter(h, [1], ref) * 0.1 + rng.standard_normal(len(ref)) * 1e-6
    return mic, ref


class TestCorrelation(unittest.TestCase):
    def test_delay_estimate_integer_and_sign(self):
        mic, ref = echo_fixture()
        d, _ = tvcap.delay_estimate(mic, ref, FS, max_lag_s=0.1)
        self.assertAlmostEqual(d * FS, 22, delta=0.3)
        d2, _ = tvcap.delay_estimate(ref, mic, FS, max_lag_s=0.1)
        self.assertAlmostEqual(d2 * FS, -22, delta=0.3)

    def test_find_template(self):
        rng = np.random.default_rng(3)
        tpl = rng.standard_normal(4800)
        x = np.concatenate([rng.standard_normal(12345) * 0.01, tpl, rng.standard_normal(5000) * 0.01])
        st, norm, psr = tvcap.find_template(x, 48000, tpl, 48000)
        self.assertAlmostEqual(st * 48000, 12345, delta=1)
        self.assertGreater(norm, 0.99)
        self.assertGreater(psr, 20)

    def test_align(self):
        x = np.arange(10, dtype=float)
        np.testing.assert_array_equal(tvcap.align(x, 2, 10)[:4], [0, 0, 0, 1])
        np.testing.assert_array_equal(tvcap.align(x, -2, 10)[:3], [2, 3, 4])


class TestEchoMetrics(unittest.TestCase):
    def test_pbfdaf_cancels_linear_echo(self):
        mic, ref = echo_fixture()
        e = tvcap.pbfdaf(mic, ref, block=256, partitions=16, mu=0.5)
        self.assertGreater(tvcap.erle_db(mic, e, FS, skip_s=5), 25)

    def test_pbfdaf_fails_on_unrelated_reference(self):
        mic, _ = echo_fixture(seed=1)
        _, other = echo_fixture(seed=2)
        e = tvcap.pbfdaf(mic, other, partitions=8, mu=0.5)
        self.assertLess(abs(tvcap.erle_db(mic, e, FS, skip_s=5)), 3)

    def test_coherence_bound_drops_with_independent_noise(self):
        mic, ref = echo_fixture()
        d, _ = tvcap.delay_estimate(mic, ref, FS)
        ra = tvcap.align(ref, d * FS, len(mic))
        clean = tvcap.coherence_stats(mic, ra, FS)
        rng = np.random.default_rng(9)
        noisy_ref = ra + rng.standard_normal(len(ra)) * np.std(ra)  # 0 dB junk in the reference
        noisy = tvcap.coherence_stats(mic, noisy_ref, FS)
        self.assertGreater(clean["mean_coherence"], 0.8)
        self.assertLess(noisy["mean_coherence"], 0.6)
        self.assertGreater(clean["erle_bound_db"], noisy["erle_bound_db"] + 5)

    def test_drift_slope(self):
        rng = np.random.default_rng(4)
        n = FS * 60
        ref = rng.standard_normal(n)
        ppm = 50.0
        t = np.arange(n) / FS
        mic = np.interp(t * (1 - ppm * 1e-6) - 0.001, t, ref)  # 1 ms delay, growing by 50 ppm
        res = tvcap.drift(mic, ref, FS, win_s=2.0, hop_s=5.0, max_lag_s=0.05)
        self.assertAlmostEqual(res["ppm"], ppm, delta=5)
        self.assertAlmostEqual(res["delays_ms"][0], 1.0, delta=0.1)


class TestDemuxAndLoad(unittest.TestCase):
    def test_round_trip(self):
        mic = (np.arange(64, dtype=np.int32) - 32) * 1000          # S24 values, 2 ch
        ref = (np.arange(8 * 96, dtype=np.int32) - 400) * 1000000  # S32 values, 8 ch
        meta_open = {"kind": "open", "link_rc": 0, "pcms": [
            {"sid": 0, "name": "m", "format": "S24_LE", "channels": 2, "rate": 16000},
            {"sid": 1, "name": "r", "format": "S32_LE", "channels": 8, "rate": 48000}]}
        buf = io.BytesIO()

        def frame(sid, payload):
            buf.write(b"TC" + struct.pack("<BI", sid, len(payload)) + payload)

        frame(2, json.dumps(meta_open).encode())
        frame(0, mic[:32].astype("<i4").tobytes())
        frame(1, ref.astype("<i4").tobytes())
        frame(0, mic[32:].astype("<i4").tobytes())
        frame(2, json.dumps({"kind": "end"}).encode())
        buf.seek(0)
        with tempfile.TemporaryDirectory() as d:
            counts, metas = tv_dualcap_demux.demux(buf, d)
            self.assertEqual(counts, {0: 256, 1: ref.nbytes})
            self.assertEqual([m["kind"] for m in metas], ["open", "end"])
            cap = tvcap.load(d)
            self.assertEqual(cap["mic"].shape, (32, 2))
            self.assertEqual(cap["ref"].shape, (96, 8))
            self.assertAlmostEqual(cap["mic"][0, 0], -32000 / 2.0**23)
            self.assertEqual(cap["fs_ref"], 48000)

    def test_truncated_input_raises(self):
        buf = io.BytesIO(b"TC" + struct.pack("<BI", 0, 10) + b"abc")
        with tempfile.TemporaryDirectory() as d, self.assertRaises(ValueError):
            tv_dualcap_demux.demux(buf, d)


if __name__ == "__main__":
    unittest.main()
