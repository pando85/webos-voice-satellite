"""Unit tests for p2_score.py (synthetic inputs only).

Run: .venv/bin/python -m unittest discover -s tools/audio-rig -p 'test_p2_*.py'
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p2_score as s  # noqa: E402


class Normalize(unittest.TestCase):
    def test_accents_punctuation_and_case(self):
        self.assertEqual(s.normalize("¿Qué hora es?")[0], ["que", "hora", "es"])
        self.assertEqual(s.normalize("Enciende la luz del Salón.")[0], ["enciende", "la", "luz", "del", "salon"])
        self.assertEqual(s.normalize("año, pingüino")[0], ["ano", "pinguino"])

    def test_numerals_spelled_in_hypothesis_only(self):
        words, n = s.normalize("pon un temporizador de 5 minutos", spell_numbers=True)
        self.assertEqual(words, ["pon", "un", "temporizador", "de", "cinco", "minutos"])
        self.assertEqual(n, 1)
        self.assertEqual(s.normalize("al 50%", spell_numbers=True)[0], ["al", "cincuenta"])
        self.assertEqual(s.normalize("a 21 grados", spell_numbers=True)[0], ["a", "veintiuno", "grados"])
        self.assertEqual(s.normalize("5", spell_numbers=False)[0], ["5"])

    def test_spell_es(self):
        self.assertEqual(s.spell_es(0), "cero")
        self.assertEqual(s.spell_es(16), "dieciseis")
        self.assertEqual(s.spell_es(30), "treinta")
        self.assertEqual(s.spell_es(45), "cuarenta y cinco")
        with self.assertRaises(ValueError):
            s.spell_es(100)


class Wer(unittest.TestCase):
    def test_counts(self):
        self.assertEqual(s.wer_counts("¿Qué hora es?", "Qué hora es"), (0, 3, 0))
        self.assertEqual(s.wer_counts("a b c d", "a x c"), (2, 4, 0))  # 1 S + 1 D
        self.assertEqual(s.wer_counts("a b", "a b c"), (1, 2, 0))  # 1 I
        self.assertEqual(s.wer_counts("pon un temporizador de diez minutos",
                                      "Pon un temporizador de 10 minutos."), (0, 6, 1))

    def test_v11_number_formats(self):
        ref = "baja la luz del salón al cincuenta por ciento"
        self.assertEqual(s.wer_counts(ref, "Baja la luz del salón al 50%.")[0], 2)
        self.assertEqual(s.wer_counts(ref, "Baja la luz del salón al 50%.", v11=True)[0], 0)
        ref = "sube la temperatura del termostato a veintiún grados"
        self.assertEqual(s.wer_counts(ref, "Sube la temperatura del termostato a 21 grados.")[0], 1)
        self.assertEqual(s.wer_counts(ref, "Sube la temperatura del termostato a 21 grados.", v11=True)[0], 0)

    def test_corpus_wer_is_pooled(self):
        rows = [{"condition": "c", "kind": "pos", "subkind": "cmd", "detected": True, "latency_s": 0.3,
                 "repeats": 0, "outside": 0, "wer_errors": e, "wer_words": n, "intent_ok": True}
                for e, n in ((1, 2), (0, 8))]
        t = s.summarize(rows)[0]
        self.assertTrue(t["wer"].startswith("1/10 = 10.0 %"))


class Intervals(unittest.TestCase):
    def test_wilson_known_values(self):
        lo, hi = s.wilson(97, 100)
        self.assertAlmostEqual(lo, 0.9154, places=3)
        self.assertAlmostEqual(hi, 0.9897, places=3)
        lo, hi = s.wilson(0, 10)
        self.assertEqual(lo, 0.0)
        self.assertAlmostEqual(hi, 0.2775, places=3)

    def test_poisson(self):
        self.assertEqual(s.poisson_rate_ci(0, 1.5), (0.0, 2.0))
        lo, hi = s.poisson_rate_ci(1, 1.0)
        self.assertAlmostEqual(lo, 0.0253, places=3)
        self.assertAlmostEqual(hi, 5.5716, places=3)
        lo, hi = s.poisson_rate_ci(10, 2.0)
        self.assertAlmostEqual(lo, 4.795 / 2, places=2)
        self.assertAlmostEqual(hi, 18.39 / 2, places=2)


class WakeMatch(unittest.TestCase):
    pos = {"id": "p", "kind": "pos", "wake_start_ms": 300, "wake_end_ms": 760}

    def test_inside_window(self):
        m = s.match_wake(self.pos, [0.9])
        self.assertTrue(m["detected"])
        self.assertAlmostEqual(m["latency_s"], 0.14)

    def test_window_edges(self):
        self.assertTrue(s.match_wake(self.pos, [0.1])["detected"])  # 0.3 - 0.2
        self.assertTrue(s.match_wake(self.pos, [2.26])["detected"])  # 0.76 + 1.5
        self.assertFalse(s.match_wake(self.pos, [2.27])["detected"])
        self.assertFalse(s.match_wake(self.pos, [0.09])["detected"])

    def test_repeats_and_outside(self):
        m = s.match_wake(self.pos, [0.8, 1.2, 5.0])
        self.assertEqual((m["repeats"], m["outside"]), (1, 1))

    def test_negative_counts_everything(self):
        m = s.match_wake({"id": "n", "kind": "neg"}, [1.0, 2.0])
        self.assertEqual((m["detected"], m["outside"]), (False, 2))


class Intent(unittest.TestCase):
    def test_mapping(self):
        r = {"intent": {"name": "HassTurnOn"}, "slots": {"area": "salón", "domain": "light"}}
        self.assertTrue(s.score_intent("light_on", "enciende la luz del salón", r))
        self.assertFalse(s.score_intent("light_on", "enciende la luz del dormitorio", r))
        r = {"intent": {"name": "HassTurnOn"}, "slots": {}, "unmatched_slots": {"area": "dormitorio"}}
        self.assertTrue(s.score_intent("light_on", "enciende la luz del dormitorio", r))
        r = {"intent": {"name": "HassStartTimer"}, "slots": {"minutes": "5"}}
        self.assertTrue(s.score_intent("timer_start", "pon un temporizador de cinco minutos", r))
        self.assertFalse(s.score_intent("timer_start", "pon un temporizador de diez minutos", r))
        r = {"intent": {"name": "HassSetPosition"}, "slots": {"area": "salón", "position": "cincuenta"}}
        self.assertFalse(s.score_intent("light_set", "baja la luz del salón al cincuenta por ciento", r))
        self.assertFalse(s.score_intent("time_query", "¿qué hora es?", None))
        r = {"intent": {"name": "HassClimateSetTemperature"}, "slots": {"temperature": "veintiún"}}
        self.assertTrue(s.score_intent("climate_set", "sube la temperatura del termostato a veintiún grados", r))

    def test_every_class_has_a_mapping(self):
        for k in ("light_on", "light_off", "light_off_all", "light_set", "time_query", "timer_start",
                  "timer_cancel", "timer_status", "temperature_query", "weather_query", "climate_set"):
            self.assertIn(k, s.INTENT_MAP)


class FalseWakes(unittest.TestCase):
    def test_attribution_and_dead_time(self):
        play = [{"category": "speech_es", "t_start": 0.0, "t_end": 3600.0},
                {"category": "music", "t_start": 3600.0, "t_end": 5400.0}]
        watch = [{"kind": "state", "old": "idle", "new": "listening", "t_pc": 100.0},
                 {"kind": "state", "old": "listening", "new": "processing", "t_pc": 105.0},
                 {"kind": "state", "old": "responding", "new": "idle", "t_pc": 117.7},
                 {"kind": "state", "old": "idle", "new": "listening", "t_pc": 4000.0},
                 {"kind": "state", "old": "listening", "new": "idle", "t_pc": 4010.0}]
        rows, info = s.false_wakes(watch, play)
        by = {r["category"]: r for r in rows}
        self.assertEqual(by["speech_es"]["false_wakes"], 1)
        self.assertAlmostEqual(by["speech_es"]["hours_armed"], (3600 - 18.0) / 3600, places=3)
        self.assertEqual(by["music"]["false_wakes"], 1)
        self.assertEqual(by["total"]["false_wakes"], 2)
        self.assertEqual(by["total"]["hours"], 1.5)
        self.assertEqual(info, {"detections": 2, "outside_playlog": 0})

    def test_zero_uses_rule_of_three(self):
        rows, _ = s.false_wakes([], [{"category": "x", "t_start": 0.0, "t_end": 1800.0}])
        self.assertEqual(rows[0]["ci95"], [0.0, 6.0])


if __name__ == "__main__":
    unittest.main()
