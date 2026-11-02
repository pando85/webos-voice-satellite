"""Unit tests for the item selection in p2_runner.py (no audio, no network).

Run: .venv/bin/python -m unittest discover -s tools/audio-rig -p 'test_p2_*.py'
"""
import collections
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p2_runner as r  # noqa: E402


def fake_items():
    items = []
    for v, n_alone, n_cmd in (("es_A", 3, 10), ("es_B", 3, 10), ("en_C", 3, 0)):
        for i in range(n_alone):
            items.append({"id": f"pos_alone_{v}_{i}", "voice": v, "speaker": "s", "kind": "pos", "subkind": "alone"})
        for i in range(n_cmd):
            items.append({"id": f"pos_cmd_{v}_{i:02d}", "voice": v, "speaker": "s", "kind": "pos", "subkind": "cmd"})
    return items


class Stratified(unittest.TestCase):
    def test_every_voice_and_both_subkinds(self):
        s = r.stratified(fake_items(), 5)
        by = collections.Counter(i["voice"] for i in s)
        self.assertEqual(by, {"es_A": 5, "es_B": 5, "en_C": 3})
        sub = collections.Counter((i["voice"], i["subkind"]) for i in s)
        self.assertEqual(sub[("es_A", "cmd")], 3)
        self.assertEqual(sub[("es_A", "alone")], 2)

    def test_deterministic_and_unique(self):
        a = [i["id"] for i in r.stratified(fake_items(), 7)]
        b = [i["id"] for i in r.stratified(list(reversed(fake_items())), 7)]
        self.assertEqual(a, b)
        self.assertEqual(len(a), len(set(a)))


if __name__ == "__main__":
    unittest.main()


class BedMix(unittest.TestCase):
    def test_level_offset_and_frontend(self):
        import numpy as np
        import p2_digital as d
        bed = np.zeros(16000 * 30)
        tone = (np.sin(2 * np.pi * 1000 * np.arange(16000) / 16000) * 32767 * 0.5).astype(np.int16)
        out, off, clipped = d.mix_on_bed(tone, bed, index=3, lead=2.0, tail=3.0, speech_gain_db=-39.0)
        self.assertEqual(len(out), 16000 * 6)
        self.assertGreaterEqual(off, 1.0)
        self.assertEqual(clipped, 0)
        seg = out[2 * 16000 + 1600:3 * 16000 - 1600].astype(float) / 32768
        rms_db = 20 * np.log10(np.sqrt(np.mean(seg ** 2)))
        # -6 dBFS peak sine = -9 dBFS RMS, -39 dB, +24 dB front-end gain
        self.assertAlmostEqual(rms_db, -9.03 - 39 + 24, delta=0.3)
        self.assertEqual(d.mix_on_bed(tone, bed, 3, 2.0, 3.0, -39.0)[1], off)


class BedLoad(unittest.TestCase):
    def test_s24_container_decode(self):
        import tempfile
        import numpy as np
        import p2_digital as d
        vals = np.array([[0x7FFFFF, 1], [0x800000, 2], [0xFFFFFF, 3], [0x000010, 4]], dtype="<i4")
        with tempfile.NamedTemporaryFile(suffix=".raw") as f:
            vals.tofile(f.name)
            x = d.load_bed(f.name)
        self.assertAlmostEqual(x[0], 0x7FFFFF / 2 ** 23)
        self.assertEqual(x[1], -1.0)
        self.assertAlmostEqual(x[2], -1 / 2 ** 23)
        self.assertAlmostEqual(x[3], 16 / 2 ** 23)
