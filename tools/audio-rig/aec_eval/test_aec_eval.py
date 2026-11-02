#!/usr/bin/env python3
"""Unit tests for the offline front-end A/B tools (no recordings, no models needed).

    python3 -m unittest discover -s tools/audio-rig/aec_eval -p 'test_*.py'
"""
import os
import sys
import tempfile
import unittest

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, ".."))

import ab_report  # noqa: E402
import semisyn_ab  # noqa: E402
import stt_eval  # noqa: E402
from oww_wyoming import WyomingOww  # noqa: E402


class SemisynTest(unittest.TestCase):
    def test_bed_offset_reproduces_session_2(self):
        # pos_alone_sharM_v0 (1264 ms) was the first item on the 600 s content-v10 bed in
        # session 2 and was mixed at 8.919 s.
        off = semisyn_ab.bed_offset(1, 600 * 16000, int(1.264 * 16000))
        self.assertEqual(off, 142704)
        self.assertAlmostEqual(off / 16000, 8.919, places=3)
        with self.assertRaises(ValueError):
            semisyn_ab.bed_offset(1, 16000 * 5, 16000)

    def test_s24_sign_extension_ignores_the_top_byte(self):
        raw = np.array([0x7FFFFF, 0x800000, 0xFF800000, 0x00FFFFFF, 0x12000001], dtype=np.int64)
        self.assertEqual(semisyn_ab.s24(raw).tolist(), [8388607, -8388608, -8388608, -1, 1])

    def test_speech_lands_after_warmup_and_lead_on_both_channels(self):
        class Bed:
            pass
        bed = Bed()
        bed.mic = np.zeros((40 * 16000, 2), dtype="<i4")
        bed.ref = np.arange(3 * 40 * 16000 * 2, dtype="<i4").reshape(-1, 2)
        sp = np.full(160, 1000.0)
        mic, ref, pre = semisyn_ab.build_segment(bed, off=5 * 16000, n=6 * 16000, warmup=10 * 16000, sp24=sp)
        self.assertEqual(pre, 5 * 16000)  # clipped at the start of the recording
        a = pre + 2 * 16000
        self.assertTrue(np.all(mic[a:a + 160] == 1000))
        self.assertEqual(int(np.abs(mic).sum()), 2 * 160 * 1000)
        self.assertEqual(len(ref), 3 * len(mic))
        self.assertEqual(ref[0, 0], 0)  # reference starts at the same instant (3 x mic frames)

    def test_variant_flags(self):
        self.assertEqual(semisyn_ab.parse_variant("ns+30=--ns --gain-db 30"), ("ns+30", "--ns --gain-db 30"))
        self.assertEqual(semisyn_ab.split_gain("--ns --gain-db 30"), ("--ns", 30.0))
        self.assertEqual(semisyn_ab.split_gain(""), ("", 24.0))
        lat = semisyn_ab.latency_of
        self.assertEqual((lat("--no-aec"), lat(""), lat("--ns"), lat("--output linear"),
                          lat("--output linear --ns"), lat("--output nearend")), (0, 128, 224, 64, 160, 128))

    def test_hpf_matches_the_front_end_emulation(self):
        from oww_offline import frontend
        x = np.random.default_rng(1).normal(0, 1e5, 4000)
        y16, _ = frontend(x, 0.0, 120.0)
        y = semisyn_ab.hpf(x) / 256.0
        self.assertLess(np.max(np.abs(np.round(y) - y16)), 1.01)


class FakeDetector(WyomingOww):
    """run() without models: detections at fixed absolute sample positions."""

    def __init__(self, at):
        self.at = at

    def run(self, pcm16, chunk=1024, first_only=False):
        start = self.offset
        hits = [p for p in self.at if start <= p < start + len(pcm16)]
        if not hits:
            return [], 0.1
        return [{"t": (hits[0] - start) / 16000, "score": 0.9}], 0.9


class RearmTest(unittest.TestCase):
    def test_rearm_starts_a_new_stream_after_each_detection(self):
        det = FakeDetector([16000 * 10, 16000 * 11, 16000 * 30])
        calls = []
        orig = FakeDetector.run

        def run(self, pcm16, chunk=1024, first_only=False):
            self.offset = 16000 * 60 - len(pcm16)
            calls.append(self.offset)
            return orig(self, pcm16, chunk, first_only)
        det.run = run.__get__(det)
        dets, mx, armed = det.run_rearm(np.zeros(16000 * 60, np.int16), rearm_s=2.0)
        # The detection at 11 s falls in the 2 s re-arm gap after the one at 10 s.
        self.assertEqual([d["t"] for d in dets], [10.0, 30.0])
        self.assertAlmostEqual(armed, 60 - 2 * (2 + 1024 / 16000), places=2)
        self.assertEqual(mx, 0.9)


class ReportTest(unittest.TestCase):
    def test_kappa_and_mcnemar(self):
        self.assertEqual(ab_report.kappa([(True, True), (False, False)] * 5), 1.0)
        self.assertAlmostEqual(ab_report.kappa([(True, False), (False, True)] * 5), -1.0)
        self.assertEqual(ab_report.mcnemar(0, 0), 1.0)
        self.assertAlmostEqual(ab_report.mcnemar(0, 5), 2 / 32)
        self.assertAlmostEqual(ab_report.mcnemar(3, 3), 1.0)

    def test_tables_score_with_the_p2_window(self):
        item = {"id": "p1", "kind": "pos", "subkind": "alone", "voice": "es_ES-davefx-medium", "speaker": "",
                "wake_start_ms": 300, "wake_end_ms": 800}
        rows = [{"condition": "c", "variant": "a", "item": item, "detections_s": [0.9], "ha_detections_s": [0.95]},
                {"condition": "c", "variant": "b", "item": item, "detections_s": [3.0], "ha_detections_s": [0.95]}]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
            import json
            for r in rows:
                f.write(json.dumps(r) + "\n")
        try:
            loaded = ab_report.load([f.name])
        finally:
            os.unlink(f.name)
        self.assertEqual([r["detected"] for r in loaded], [True, False])  # 3.0 s is past wake end + 1.5 s
        self.assertIn("1/1 = 100 %", ab_report.table(loaded))
        self.assertIn("lost", ab_report.paired(loaded, "a"))
        cal = ab_report.calibration(loaded)
        self.assertIn("100 %", cal.splitlines()[1])


class SttTest(unittest.TestCase):
    def test_command_segment(self):
        x = np.arange(16000 * 8)
        it = {"command_start_ms": 1000, "duration_ms": 3000}
        seg = stt_eval.segment(x, it, lead=2.0)
        self.assertEqual(seg[0], int(2.5 * 16000))
        self.assertEqual(len(seg), int(5.3 * 16000) - int(2.5 * 16000))


if __name__ == "__main__":
    unittest.main()
