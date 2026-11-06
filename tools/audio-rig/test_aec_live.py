"""Unit tests for the live AEC A/B tools (plan, schedule, config editing, safety, log parsing, CSV, McNemar).

No audio, no network, no device. Run: python3 -m unittest discover -s tools/audio-rig -p 'test_aec_live.py'
"""
import asyncio
import collections
import contextlib
import csv
import io
import datetime
import json
import os
import re
import sys
import tempfile
import time
import unittest
import unittest.mock
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import aec_live_ab as ab  # noqa: E402
import aec_live_convert as conv  # noqa: E402
import aec_live_lib as L  # noqa: E402
import aec_live_report as rep  # noqa: E402

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
PLANS = HERE / "aec_plans"

BASE = """# synthetic config
mode = "wake"

[device]
name = "test-tv"

[log]
level = "info"

[safety]
mute_file = "muted"
"""

BASE_WITH_AEC = """mode = "wake"

[log]
level = "info"
# comment about the aec section below
[aec]
enabled = true
render_channels = 1

[aec.aec3]
"linear_mix" = 0.9

# the music section follows
[music]
enabled = false
"""

SNIP_B = '[aec]\nenabled = true\nrender_channels = 2\nns = true\n\n[aec.aec3]\n"linear_mix" = 0.4\n'
SNIP_DEF = "[aec]\nenabled = true\n"


class ConfigEditor(unittest.TestCase):
    def test_off_on_a_config_without_aec_is_byte_identical(self):
        self.assertEqual(L.render_config(BASE, ""), BASE)

    def test_apply_then_off_restores_exactly(self):
        on = L.render_config(BASE, SNIP_B)
        self.assertIn("[aec.aec3]", on)
        self.assertEqual(L.render_config(on, ""), BASE)

    def test_outside_managed_unchanged_for_every_arm(self):
        for text in (BASE, BASE_WITH_AEC):
            for snip in ("", SNIP_DEF, SNIP_B):
                out = L.render_config(text, snip, "debug")
                self.assertEqual(L.outside_managed(out), L.outside_managed(text))

    def test_idempotent_arm_switching(self):
        a = L.render_config(L.render_config(BASE, SNIP_B), SNIP_DEF)
        self.assertEqual(a, L.render_config(BASE, SNIP_DEF))

    def test_existing_aec_tables_replaced_in_place_comments_kept(self):
        out = L.render_config(BASE_WITH_AEC, SNIP_DEF)
        self.assertNotIn("render_channels = 1", out)
        self.assertNotIn('"linear_mix" = 0.9', out)
        self.assertIn("# the music section follows\n[music]\nenabled = false\n", out)
        # the new section sits where the old one was: before the music comment
        self.assertLess(out.index("enabled = true"), out.index("# the music section follows"))
        self.assertEqual(len(re.findall(r"(?m)^\[aec\]$", out)), 1)

    def test_every_other_line_survives_byte_for_byte(self):
        out = L.render_config(BASE_WITH_AEC, SNIP_B)
        keep = [l for l in BASE_WITH_AEC.splitlines(keepends=True)
                if l.strip() and not re.match(r'\s*(\[aec|enabled = true$|render_channels|"linear_mix")', l)]
        for l in keep:
            self.assertIn(l, out)

    def test_no_trailing_newline_and_crlf(self):
        out = L.render_config("mode = \"wake\"\r\n[log]\r\nlevel = \"info\"", SNIP_DEF)
        self.assertIn('level = "info"\n' + L.BEGIN_MARK, out)
        crlf = 'a = 1\r\n[log]\r\nlevel = "info"\r\n'
        self.assertEqual(L.set_log_level(crlf, "debug"), 'a = 1\r\n[log]\r\nlevel = "debug"\r\n')

    def test_log_level(self):
        self.assertIn('[log]\nlevel = "debug"\n', L.set_log_level(BASE, "debug"))
        self.assertEqual(L.set_log_level(BASE, ""), BASE)
        self.assertIn('[log]\nlevel = "debug"', L.set_log_level('[device]\nname = "x"\n', "debug"))
        no_key = '[log]\n# no level\n[safety]\nx = 1\n'
        out = L.set_log_level(no_key, "debug")
        self.assertEqual(out, '[log]\nlevel = "debug"\n# no level\n[safety]\nx = 1\n')

    def test_rendered_toml_parses(self):
        import tomllib
        d = tomllib.loads(L.render_config(BASE, SNIP_B, "debug"))
        self.assertEqual(d["aec"]["render_channels"], 2)
        self.assertEqual(d["aec"]["aec3"]["linear_mix"], 0.4)
        self.assertEqual(d["log"]["level"], "debug")
        self.assertEqual(d["device"]["name"], "test-tv")


class Plans(unittest.TestCase):
    def test_repo_plans_load(self):
        for name in ("exp018.toml", "exp019-off-vs-bns.toml"):
            plan = L.load_plan(PLANS / name)
            self.assertTrue(plan["blocks"])

    def test_exp019_design(self):
        plan = L.load_plan(PLANS / "exp019-off-vs-bns.toml")
        self.assertEqual(set(plan["arms"]), {"OFF", "B+NS"})
        self.assertEqual(plan["arms"]["OFF"]["aec"], "[aec]\nenabled = false\n")
        self.assertIn("ns = true", plan["arms"]["B+NS"]["aec"])
        # Since 0.4.3 a missing [aec] section is the B+NS default (on): OFF must say enabled = false.
        self.assertFalse(L.arm_aec_enabled(plan["arms"]["OFF"]["aec"]))
        self.assertTrue(L.arm_aec_enabled(plan["arms"]["B+NS"]["aec"]))
        self.assertTrue(L.arm_aec_enabled(""))
        self.assertEqual({c["tv_volume"] for c in plan["conditions"].values()}, {10, 15})
        self.assertEqual({t["sink_pct"] for n, t in plan["talkers"].items() if n == "natural"}, {82})
        per = collections.Counter((b["condition"], b["arm"]) for b in plan["blocks"])
        self.assertEqual(set(per.values()), {4})                 # 4 blocks of 10 per arm and condition
        self.assertEqual(sum(len(b["items"]) for b in plan["blocks"]), 160)
        for cond in ("C10", "C15"):                              # 40 distinct paired items per cell
            for arm in ("OFF", "B+NS"):
                ids = [i for b in plan["blocks"] if (b["condition"], b["arm"]) == (cond, arm) for i in b["items"]]
                self.assertEqual(len(ids), 40)
                self.assertEqual(len(set(ids)), 40)
        harmless = ("alone", "_02", "_00", "_04", "_05", "_07", "_09", "_01", "_03", "_08")
        for i in {i for s in plan["sets"].values() for i in s}:   # only the voice-ids of the corpus
            self.assertTrue(i.startswith("pos_"))

    def test_abba_order_and_alternation(self):
        plan = L.load_plan(PLANS / "exp019-off-vs-bns.toml")
        for cond in ("C10", "C15"):
            seq = [b["arm"] for b in plan["blocks"] if b["condition"] == cond]
            first = seq[0]
            other = "B+NS" if first == "OFF" else "OFF"
            self.assertEqual(seq, [first, other, other, first, other, first, first, other])

    def test_deterministic_from_seed(self):
        a = L.load_plan(PLANS / "exp019-off-vs-bns.toml")["blocks"]
        b = L.load_plan(PLANS / "exp019-off-vs-bns.toml")["blocks"]
        self.assertEqual(a, b)
        raw = _raw_plan(seed=19)
        self.assertEqual(L.parse_plan(raw)["blocks"], L.parse_plan(_raw_plan(seed=19))["blocks"])
        self.assertNotEqual([x["items"] for x in L.parse_plan(raw)["blocks"]],
                            [x["items"] for x in L.parse_plan(_raw_plan(seed=20))["blocks"]])

    def test_counterbalanced_orders(self):
        self.assertEqual(L.counterbalanced_orders(["A", "B"]), [["A", "B"], ["B", "A"], ["B", "A"], ["A", "B"]])
        o3 = L.counterbalanced_orders(["A", "B", "C"])
        self.assertEqual(len(o3), 6)
        for pos in range(3):
            self.assertEqual(collections.Counter(o[pos] for o in o3), {"A": 2, "B": 2, "C": 2})

    def test_exp018_explicit_order_is_the_recorded_one(self):
        plan = L.load_plan(PLANS / "exp018.toml")
        self.assertEqual(len(plan["blocks"]), 20)
        self.assertEqual([(b["arm"], b["condition"], b["set"]) for b in plan["blocks"][:3]],
                         [("DEFAULT", "Q", "X"), ("DEFAULT", "C10", "X"), ("DEFAULT", "C15", "X")])
        self.assertEqual([b["arm"] for b in plan["blocks"][-2:]], ["B+NS", "B+NS"])

    def test_arm_snippets_are_aec_only(self):
        with self.assertRaises(L.PlanError):
            L.validate_arm_snippet('[mic]\nsource = "alsa"\n')
        with self.assertRaises(L.PlanError):
            L.validate_arm_snippet("[aec]\nenabled = true\n[music]\nenabled = true\n")
        with self.assertRaises(L.PlanError):
            L.validate_arm_snippet("not toml [[[")
        self.assertEqual(L.validate_arm_snippet(""), "")

    def test_plans_hold_no_secrets_or_addresses(self):
        for p in PLANS.glob("*.toml"):
            text = p.read_text()
            self.assertNotRegex(text, r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}")
            self.assertNotRegex(text, r"(?i)(token|psk|password|bluez_output|alsa_output|root@|https?://)")

    def test_unknown_references_refused(self):
        raw = _raw_plan()
        raw["schedule"]["arm_order"] = ["OFF", "NOPE"]
        with self.assertRaises(L.PlanError):
            L.parse_plan(raw)

    def test_dry_run_summary_and_text(self):
        plan = L.load_plan(PLANS / "exp019-off-vs-bns.toml")
        s = L.schedule_summary(plan)
        self.assertEqual(s["trials"], 160)
        self.assertEqual(s["restarts"], 12)
        self.assertGreater(s["total_minutes"], s["ab_minutes"])
        text = L.format_schedule(plan)
        self.assertIn("estimated sound", text)
        self.assertIn("block 16", text)

    def test_dry_run_touches_nothing(self):
        # no ssh/pactl/HA: dry_run must not call the subprocess helper
        calls = []
        orig = ab.sh
        ab.sh = lambda *a, **k: calls.append(a) or (0, b"")
        try:
            ns = ab.parse_args([str(PLANS / "exp019-off-vs-bns.toml"), "--dry-run", "--corpus", "/nonexistent"])[1]
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.assertEqual(ab.dry_run(ns, L.load_plan(ns.plan)), 0)
            self.assertIn("dry run: no device", out.getvalue())
        finally:
            ab.sh = orig
        self.assertEqual(calls, [])


def _raw_plan(seed=19):
    return {"plan": {"id": "t", "seed": seed},
            "talkers": {"natural": {"sink_pct": 82}},
            "arms": {"OFF": {"aec": ""}, "B": {"aec": "[aec]\nenabled = true\n"}},
            "conditions": {"C10": {"tv_volume": 10, "content": True, "talker": "natural"}},
            "sets": {"S1": [f"i{n}" for n in range(10)], "S2": [f"j{n}" for n in range(10)]},
            "schedule": {"mode": "generated", "arm_order": ["OFF", "B"], "conditions": ["C10"], "sets": ["S1", "S2"]}}


class FakeRun:
    """Stands in for ssh/pactl: records every command in order and answers the few we use."""

    def __init__(self, vol=89):
        self.calls, self.vol = [], vol

    def __call__(self, cmd, input_bytes=None, timeout=120):
        self.calls.append(" ".join(cmd))
        remote = cmd[-1]
        if "getVolume" in remote:
            return 0, b'{"returnValue":true,"volume":%d,"muteStatus":false}' % self.vol
        if "setVolume" in remote:
            self.vol = int(re.search(r'"volume":(\d+)', remote).group(1))
            return 0, b""
        return 0, b""


class FakeContent:
    def __init__(self, log):
        self.log = log

    def start(self):
        self.log.append("content.start")

    def stop(self):
        self.log.append("content.stop")


class TvConfig(unittest.TestCase):
    def _run(self, md5_ok=True):
        calls = []

        def run(cmd, input_bytes=None, timeout=120):
            calls.append((cmd[-1], input_bytes))
            if "md5sum" in cmd[-1]:
                return 0, (L.md5(input_bytes) if md5_ok else "0" * 32).encode() + b"  config.toml.new\n"
            if "date -u" in cmd[-1]:
                return 0, b"1000\n"
            return 0, b""
        return run, calls

    def test_write_config_checks_md5_before_moving(self):
        run, calls = self._run()
        ab.Tv("root@tv", run=run).write_config(b"abc\n")
        self.assertIn("mv /var/lib/webos-satellite/etc/config.toml.new", calls[-1][0])
        run, calls = self._run(md5_ok=False)
        with self.assertRaises(RuntimeError):
            ab.Tv("root@tv", run=run).write_config(b"abc\n")
        self.assertFalse(any(c[0].startswith("mv ") for c in calls))

    def test_arm_apply_writes_only_managed_parts_then_restarts(self):
        plan = L.load_plan(PLANS / "exp019-off-vs-bns.toml")
        run, calls = self._run()
        ctx = ab.Ctx()
        ctx.plan, ctx.base_text, ctx.config_changed, ctx.cur_arm = plan, BASE, False, None
        ctx.tv = ab.Tv("root@tv", run=run)
        t = ab.tv_arm_apply(ctx, "B+NS")
        self.assertEqual(t, 1000)
        written = [c[1] for c in calls if c[1]][0].decode()
        self.assertEqual(L.outside_managed(written), L.outside_managed(BASE))
        self.assertIn("ns = true", written)
        self.assertTrue(ctx.config_changed)
        self.assertEqual(ctx.cur_arm, "B+NS")
        self.assertIn("ctl.sh restart", calls[-1][0])


class Safety(unittest.TestCase):
    def _t(self, h, m=0):
        return datetime.datetime(2026, 11, 5, h, m).astimezone()

    def test_clock_window(self):
        L.check_clock(self._t(8, 0))
        L.check_clock(self._t(20, 59))
        for h, m in ((7, 59), (21, 0), (23, 30), (3, 0)):
            with self.assertRaises(L.SafetyError):
                L.check_clock(self._t(h, m))
        L.check_clock(self._t(23, 30), allow_any_time=True)

    def test_tv_volume_above_20_refused(self):
        L.check_tv_volume(20)
        for v in (21, 89, -1):
            with self.assertRaises(L.SafetyError):
                L.check_tv_volume(v)
        raw = _raw_plan()
        raw["conditions"]["C10"]["tv_volume"] = 25
        with self.assertRaises(L.SafetyError):
            L.parse_plan(raw)
        raw = _raw_plan()
        raw["sweep"] = {"enabled": True, "tv_volume": 30}
        with self.assertRaises(L.SafetyError):
            L.parse_plan(raw)

    def test_tv_set_volume_refuses_above_20_but_restore_may_go_back(self):
        run = FakeRun()
        tv = ab.Tv("root@tv", run=run)
        with self.assertRaises(L.SafetyError):
            tv.set_volume(21)
        self.assertEqual(run.calls, [])
        tv.set_volume(15)
        self.assertEqual(run.vol, 15)
        tv.set_volume(89, restore=True)
        self.assertEqual(run.vol, 89)

    def test_hdmi_sink_rules(self):
        L.check_sinks("alsa_output.pci-0000_09_00.1.hdmi-stereo-extra1", "bluez_output.XX.1")
        with self.assertRaises(L.SafetyError):
            L.check_sinks(None, "bluez_output.XX.1")            # content cell, no HDMI sink named
        L.check_sinks(None, "bluez_output.XX.1", content_needed=False)
        with self.assertRaises(L.SafetyError):
            L.check_sinks("hdmi-x", "alsa_output.pci.hdmi-stereo")   # talker on an HDMI sink
        with self.assertRaises(L.SafetyError):
            L.check_sinks("same", "same")
        with self.assertRaises(L.SafetyError):
            L.check_sinks("a_hdmi", "talker", known={"a_hdmi"})      # unknown talker sink
        with self.assertRaises(L.SafetyError):
            L.check_sinks("a_hdmi", None)

    def test_token_file_mode(self):
        with tempfile.NamedTemporaryFile() as f:
            os.chmod(f.name, 0o644)
            with self.assertRaises(L.SafetyError):
                L.check_token_file(f.name)
            os.chmod(f.name, 0o600)
            L.check_token_file(f.name)

    def test_condition_order_lowers_tv_volume_before_raising_the_pc_sink(self):
        plan = L.load_plan(PLANS / "exp019-off-vs-bns.toml")
        run = FakeRun(vol=89)
        events = []
        content = FakeContent(events)
        tv, pc = ab.Tv("root@tv", run=run), ab.Pc(run=run)
        ab.apply_condition(tv, pc, content, plan, "C15", "talker-sink", "hdmi-sink")
        text = "\n".join(run.calls)
        self.assertLess(text.index("setVolume"), text.index("set-sink-volume hdmi-sink 100%"))
        self.assertIn("set-sink-volume talker-sink 82%", text)
        self.assertEqual(run.vol, 15)
        self.assertEqual(events, ["content.stop", "content.start"])

    def test_restore_order_pc_sinks_before_tv_volume_and_continues_after_a_failure(self):
        run = FakeRun(vol=15)
        events = []
        content = FakeContent(events)
        tv, pc = ab.Tv("root@tv", run=run), ab.Pc(run=run)
        orig = {"tv_volume": 89, "pc": {"hdmi-sink": 14417, "talker-sink": 53667}}
        errs = ab.restore_levels(tv, pc, content, orig)
        self.assertEqual(errs, [])
        text = "\n".join(run.calls)
        self.assertLess(text.index("set-sink-volume hdmi-sink 14417"), text.index("setVolume"))
        self.assertEqual(run.vol, 89)
        self.assertEqual(events, ["content.stop"])

        class Boom(FakeContent):
            def stop(self):
                raise OSError("x")
        run2 = FakeRun(vol=15)
        errs = ab.restore_levels(ab.Tv("t", run=run2), ab.Pc(run=run2), Boom([]), orig)
        self.assertEqual(len(errs), 1)
        self.assertEqual(run2.vol, 89)     # the TV volume was still restored


LINES = """2000-01-01T10:00:00.000Z INFO  aec: echo cancellation on: reference PCM "SoundEngineCapture" (2 ch, 48000 Hz, s32_le), render 2 ch, output Full, linked start, delay auto, ns=true, bypass_when_silent=false, aec3=[ep_strength.default_len=0,linear_mix=0.4]
2000-01-01T10:00:09.100Z INFO  aec: echo delay 16.5 ms (mic behind reference, confidence 11)
2000-01-01T10:01:00.000Z DEBUG capture: last 60 s: in rms -53.8 dBFS peak -29.9 dBFS, out rms -37.0 dBFS peak -14.0 dBFS, clipped 3, xruns 0, chunks sent 554 dropped 0
2000-01-01T10:01:00.001Z DEBUG aec: last 60 s: delay 16.5 ms (confidence 12.0), ERLE 8.6 dB (sonora n/a dB) over 3285 active-reference frames, sonora 345 us/frame mean 9004 max (3.45 % of a core), bypassed 0/5976 frames, reference underruns 2
2000-01-01T10:01:30.000Z INFO  aec: echo delay changed 16.5 -> 15.2 ms (confidence 11)
2000-01-01T10:02:00.001Z DEBUG aec: last 60 s: delay 15.2 ms (confidence 17.1), ERLE 14.8 dB (sonora 5.9 dB) over 3709 active-reference frames, sonora 373 us/frame mean 4851 max (3.73 % of a core), bypassed 0/6000 frames, reference underruns 0
2000-01-01T10:03:00.001Z DEBUG aec: last 60 s: delay n/a ms (confidence n/a), ERLE n/a dB (sonora 0.2 dB) over 0 active-reference frames, sonora 295 us/frame mean 7592 max (2.95 % of a core), bypassed 0/5976 frames, reference underruns 0
2000-01-01T10:03:05.000Z INFO  streaming mic audio to HA (wake word stage)
2000-01-01T10:03:06.000Z INFO  wake word detected by HA
"""


class LogParsing(unittest.TestCase):
    def setUp(self):
        self.ents = L.parse_aec_lines(LINES)

    def test_kinds(self):
        self.assertEqual([e["kind"] for e in self.ents],
                         ["on", "lock", "capture", "window", "change", "window", "window", "streaming", "wake"])

    def test_window_fields(self):
        w = self.ents[3]
        self.assertEqual((w["delay_ms"], w["erle_db"], w["underruns"], w["active_frames"]), (16.5, 8.6, 2, 3285))
        self.assertIsNone(w["sonora_erle_db"])
        self.assertAlmostEqual(w["sonora_core_pct"], 3.45)
        w2 = self.ents[5]
        self.assertEqual(w2["sonora_erle_db"], 5.9)

    def test_na_values(self):
        w = self.ents[6]
        self.assertIsNone(w["delay_ms"])
        self.assertIsNone(w["erle_db"])
        self.assertEqual(w["active_frames"], 0)

    def test_lock_change_on_capture(self):
        self.assertEqual(self.ents[1]["delay_ms"], 16.5)
        self.assertEqual((self.ents[4]["old_ms"], self.ents[4]["delay_ms"]), (16.5, 15.2))
        self.assertEqual((self.ents[0]["render"], self.ents[0]["ns"]), (2, True))
        self.assertEqual(self.ents[2]["clipped"], 3)

    def test_timestamps_and_window_lookup(self):
        t0 = L.parse_ts("2000-01-01T10:00:00.000Z")
        self.assertEqual(L.parse_ts("2000-01-01T10:00:00Z"), t0)
        self.assertEqual(L.window_for(t0 + 30, self.ents)["erle_db"], 8.6)
        self.assertEqual(L.window_for(t0 + 90, self.ents)["erle_db"], 14.8)
        self.assertIsNone(L.window_for(t0 - 5, self.ents))
        self.assertIsNone(L.window_for(t0 + 500, self.ents))

    def test_unrelated_lines_ignored(self):
        self.assertEqual(L.parse_aec_lines("2000-01-01T10:00:00Z INFO  something else\ngarbage aec: x\n"), [])

    def test_trial_row_takes_the_window_with_clock_skew_and_skips_off_arm(self):
        plan = L.load_plan(PLANS / "exp019-off-vs-bns.toml")
        t0 = L.parse_ts("2000-01-01T10:00:00.000Z")
        item = {"id": "pos_cmd_sharF_00", "voice": "es_ES-sharvard-medium", "speaker": "F", "subkind": "cmd",
                "expected_command_text": "¿qué hora es?", "duration_ms": 2000}
        rec = {"arm": "B+NS", "condition": "C10", "block": 3, "item": item, "detections_s": [1.1], "stt_text": "¿Qué hora es?",
               "t_play": t0 + 100 - 5, "lead_s": 1.0, "sink_latency_s": 0.2, "valid": True, "notes": []}
        row = L.trial_row(plan, "r1", rec, self.ents, {"daemon_cpu_pct": 6.6, "notes": ["lock_ms=16.5"]}, skew_s=5.0)
        self.assertEqual((row["detected"], row["wake_latency_ms"], row["stt_match"], row["stt_wer"]), (1, 1100, 1, 0.0))
        self.assertEqual(row["erle_db"], 14.8)          # trial mid-point t0 + 97 + skew 5 falls in the window ending at t0 + 120
        self.assertEqual(row["tv_volume"], 10)
        self.assertEqual(row["daemon_cpu_pct"], 6.6)
        self.assertIn("block=3", row["notes"])
        rec_off = dict(rec, arm="OFF")
        self.assertIsNone(L.trial_row(plan, "r1", rec_off, self.ents, None)["erle_db"])
        bad = dict(rec, valid=False)
        r = L.trial_row(plan, "r1", bad, self.ents, None)
        self.assertIsNone(r["detected"])
        self.assertIn("invalid", r["notes"])


class DetectionAndCsv(unittest.TestCase):
    def test_detection_window(self):
        self.assertEqual(L.is_detected([1.05], 1264), (1, 1050))
        self.assertEqual(L.is_detected([-0.3], 1264), (0, None))
        self.assertEqual(L.is_detected([3.0], 1264), (0, None))     # 1.264 + 1.5 = 2.764
        self.assertEqual(L.is_detected([], 1264), (0, None))
        self.assertEqual(L.is_detected([2.5, 1.2], 1264), (1, 1200))

    def test_stt_scores_keep_no_text(self):
        cmd = {"subkind": "cmd", "expected_command_text": "¿qué hora es?"}
        self.assertEqual(L.stt_scores(cmd, " ¿Qué hora es?"), (1, 0.0))
        m, w = L.stt_scores(cmd, "¡Qué gola es!")
        self.assertEqual((m, w), (0, 0.333))
        self.assertEqual(L.stt_scores({"subkind": "alone"}, "gracias"), (None, None))
        self.assertEqual(L.stt_scores(cmd, None), (None, None))

    def test_phrase_and_voice_labels(self):
        self.assertEqual(L.phrase_id({"subkind": "alone"}), "alone")
        self.assertEqual(L.phrase_id({"subkind": "cmd", "expected_command_text": "¿qué temperatura hay en el salón?"}),
                         "cmd_que_temperatura_hay_en_el_salon")
        self.assertEqual(L.voice_label({"voice": "es_ES-sharvard-medium", "speaker": "M"}), "es_ES-sharvard-medium:M")
        self.assertEqual(L.voice_label({"voice": "es_MX-ald-medium", "speaker": "ald"}), "es_MX-ald-medium")

    def test_privacy_guard(self):
        ok = {c: "" for c in L.CSV_COLUMNS}
        L.check_row_privacy(ok)
        for bad in ("192.168.1.20", "44:D6:42:6D:2F:F7", "http://x", "eyJhbGciOiJIUzI1NiIsInR5cCI6"):
            with self.assertRaises(ValueError):
                L.check_row_privacy(dict(ok, notes=bad))

    def test_csv_roundtrip(self):
        row = {c: "" for c in L.CSV_COLUMNS}
        row.update(run_id="r", plan="p", arm="OFF", condition="C10", tv_volume=10, talker_level="natural", item_id="i",
                   voice="v", phrase_id="alone", detected=1, wake_latency_ms=1050, erle_db=8.0, delay_ms=16.5)
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "t.csv"
            L.write_rows(p, [row])
            L.write_rows(p, [dict(row, arm="B+NS")], append=True)
            rows = L.read_rows([p])
            self.assertEqual([r["arm"] for r in rows], ["OFF", "B+NS"])
            self.assertEqual(rows[0]["erle_db"], "8")
            self.assertEqual(open(p).readline().strip().split(","), L.CSV_COLUMNS)


def _fake_private_dir(d):
    """A miniature EXP-018 private directory: two arms, one item each, one cmd item with STT."""
    d = Path(d)
    t = datetime.datetime(2026, 11, 5, 12, 49, 58, tzinfo=datetime.timezone(datetime.timedelta(hours=1))).timestamp()
    item = {"id": "pos_cmd_sharF_00", "voice": "es_ES-sharvard-medium", "speaker": "F", "kind": "pos", "subkind": "cmd",
            "lang": "es", "text": "alexa ¿qué hora es?", "expected_command_text": "¿qué hora es?",
            "expected_intent": "time_query", "wake_start_ms": 300, "wake_end_ms": 800, "command_start_ms": 1200,
            "duration_ms": 2500}
    alone = dict(item, id="pos_alone_sharM_v0", speaker="M", subkind="alone", expected_command_text="", duration_ms=1264)
    rows = [{"condition": "B-C10-p1", "item": item, "detections_s": [1.1], "stt_text": None, "t_play": t + 5, "lead_s": 1.0,
             "sink_latency_s": 0.2},
            {"condition": "OFF-C10-p1", "item": alone, "detections_s": [], "stt_text": None, "t_play": t + 100, "lead_s": 1.0,
             "sink_latency_s": 0.2}]
    (d / "ab.jsonl").write_text("\n".join(json.dumps(r) for r in rows) + "\n")
    t_det = t + 5 + 1.0 + 0.2 + 1.1
    at = datetime.datetime.fromtimestamp(t_det, datetime.timezone.utc).isoformat()
    (d / "watch-all.jsonl").write_text(json.dumps({"kind": "run", "wake": {"id": "alexa", "at": at},
                                                    "stt_text": " ¿Qué hora es?", "t_pc": t_det}) + "\n")
    (d / "cells.log").write_text("12:49:58 cell B C10 pass 1 start\n12:51:00 cell B C10 pass 1 end\n"
                                 "12:51:30 cell OFF C10 pass 1 start\n12:53:00 cell OFF C10 pass 1 end\n")
    ts = datetime.datetime.fromtimestamp(t + 30, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.000Z")
    (d / "tvlog-aec.txt").write_text(LINES.split("\n")[3].replace("2000-01-01T10:01:00.001Z", ts) + "\n")


class Conversion(unittest.TestCase):
    def test_convert_synthetic_directory(self):
        with tempfile.TemporaryDirectory() as d:
            _fake_private_dir(d)
            rows = conv.convert(d)
        self.assertEqual(len(rows), 2)
        by = {r["arm"]: r for r in rows}
        b = by["B"]
        self.assertEqual((b["detected"], b["stt_match"], b["stt_wer"], b["erle_db"], b["delay_ms"], b["underruns"]),
                         (1, 1, 0.0, 8.6, 16.5, 2))
        self.assertEqual((b["tv_volume"], b["talker_level"], b["phrase_id"]), (10, "natural", "cmd_que_hora_es"))
        self.assertIn("cell=1", b["notes"])
        o = by["OFF"]
        self.assertEqual((o["detected"], o["erle_db"], o["wake_latency_ms"]), (0, None, None))
        for r in rows:
            L.check_row_privacy(r)

    def test_committed_exp018_csv_matches_the_experiment_table_and_is_clean(self):
        csv_path = ROOT / "docs" / "experiments" / "data" / "EXP-018-trials.csv"
        if not csv_path.exists():
            self.skipTest("EXP-018-trials.csv not present")
        rows = L.read_rows([csv_path])
        self.assertEqual(list(rows[0]), L.CSV_COLUMNS)
        for r in rows:
            L.check_row_privacy(r)
        text = csv_path.read_text()
        self.assertNotRegex(text, r"(?i)bluez|alsa_output|root@|https?://|token|psk")
        cells, skipped = rep.analyze(rows)
        got = {(c["condition"], c["arm"]): (c["k"], c["n"]) for c in cells}
        want = {("Q", "OFF"): (15, 20), ("Q", "DEFAULT"): (11, 20), ("Q", "B"): (14, 20),
                ("C10", "OFF"): (8, 20), ("C10", "DEFAULT"): (8, 20), ("C10", "B"): (11, 20), ("C10", "B+NS"): (15, 20),
                ("C15", "OFF"): (6, 20), ("C15", "DEFAULT"): (12, 20), ("C15", "B"): (13, 20)}
        self.assertEqual(got, want)
        self.assertEqual(skipped, 0)
        p = {(c["condition"], c["arm"]): c["paired"] for c in cells if "paired" in c}
        self.assertEqual((p[("C10", "B+NS")]["arm_only"], p[("C10", "B+NS")]["base_only"]), (10, 3))
        self.assertAlmostEqual(p[("C10", "B+NS")]["p"], 0.092, places=3)
        self.assertAlmostEqual(p[("C15", "B")]["p"], 0.118, places=3)
        self.assertAlmostEqual(p[("Q", "DEFAULT")]["p"], 0.344, places=3)


class LiveRunHelpers(unittest.TestCase):
    def test_prior_runs_keeps_the_armed_run_out(self):
        # the newest run is the armed wake word run: a detection continues it, so it must not count as seen
        runs = [{"pipeline_run_id": "a", "timestamp": "2026-11-05T13:00:00"},
                {"pipeline_run_id": "c", "timestamp": "2026-11-05T13:02:00"},
                {"pipeline_run_id": "b", "timestamp": "2026-11-05T13:01:00"}]
        self.assertEqual(ab.prior_runs(runs), {"a", "b"})
        self.assertEqual(ab.prior_runs([]), set())

    def test_collect_log_keeps_lines_across_rotations(self):
        w1 = ("2026-11-05T11:41:47.262Z DEBUG aec: last 60 s: delay 16.5 ms (confidence 11), ERLE 9.8 dB "
              "(sonora n/a dB) over 4000 active-reference frames, sonora 400 us/frame mean 9000 max "
              "(4.00 % of a core), bypassed 0/6000 frames, reference underruns 0")
        w2 = w1.replace("11:41:47", "11:42:47")
        served = [[w1, "2026-11-05T11:41:50Z INFO  config: name=secret-host x", "noise"], [w2]]

        class T:
            def aec_log_lines(self):
                return served.pop(0)
        with tempfile.TemporaryDirectory() as d:
            ctx = ab.Ctx()
            ctx.out, ctx.tv = Path(d), T()
            ab.collect_log(ctx)
            ab.collect_log(ctx)     # the first lines have rotated away by now
            text = (Path(d) / "tvlog-aec.txt").read_text()
        self.assertIn(w1, text)
        self.assertIn(w2, text)
        self.assertNotIn("secret-host", text)
        self.assertNotIn("noise", text)
        self.assertEqual(len([e for e in L.parse_aec_lines(text) if e["kind"] == "window"]), 2)

    def test_log_reads_include_the_rotated_file(self):
        run = FakeRun()
        tv = ab.Tv("root@tv", run=run)
        tv.log_entries(10)
        tv.aec_log_lines()
        self.assertTrue(all("webos-satellite.log.1" in c for c in run.calls))


class FakeHa:
    """HA's pipeline debug list (newest last, at most 10 like HA) and the runs' events."""

    def __init__(self, runs):
        self.runs = runs            # rid -> (satellite_id, [event types])
        self.order = list(runs)
        self.gets = collections.Counter()

    async def pipeline_runs(self, pid):
        return [{"pipeline_run_id": r, "timestamp": f"2026-11-05T19:00:{i:02d}"} for i, r in enumerate(self.order[-10:])]

    async def pipeline_run(self, pid, rid):
        self.gets[rid] += 1
        sat, types = self.runs[rid]
        ev = [{"type": "run-start", "data": {"satellite_id": sat}}] if types else []
        return {"events": ev + [{"type": t} for t in types if t != "run-start"]}


def _ctx(ha, state="idle", entity="assist_satellite.c3_a"):
    ctx = ab.Ctx()
    ctx.ha, ctx.pid, ctx.run_owner = ha, "p", {}
    ctx.args = type("A", (), {"entity": entity})()
    ctx.watch = type("W", (), {"state": state})()
    ctx.plan = {"timing": {"log_every_s": 3600.0}}
    ctx.log_t = time.monotonic()        # no log collection in these tests
    return ctx


ARMED = ["run-start", "wake_word-start"]
DONE = ["run-start", "wake_word-start", "wake_word-end", "stt-start", "stt-end", "run-end"]
RELISTEN_RUN = ["run-start", "stt-start", "stt-end", "run-end"]


class SatelliteRuns(unittest.TestCase):
    """Fix (a): only the target satellite's runs count (EXP-021: c3-b's runs pushed c3-a's out)."""

    def test_other_satellites_runs_are_ignored(self):
        runs = {"a1": ("assist_satellite.c3_a", ARMED)}
        runs.update({f"b{i}": ("assist_satellite.c3_b", RELISTEN_RUN) for i in range(5)})
        ha = FakeHa(runs)
        ctx = _ctx(ha)
        own = asyncio.run(ab.own_runs(ctx))
        self.assertEqual([r["pipeline_run_id"] for r in own], ["a1"])
        # the newest run of the pipeline is c3-b's finished run; ours is armed
        self.assertTrue(asyncio.run(ab.armed(ctx)))
        asyncio.run(ab.own_runs(ctx))
        self.assertEqual(ha.gets["b0"], 1)          # the owner is fetched once per run, then cached

    def test_own_finished_run_is_not_armed(self):
        ctx = _ctx(FakeHa({"a1": ("assist_satellite.c3_a", DONE), "b1": ("assist_satellite.c3_b", ARMED)}))
        self.assertFalse(asyncio.run(ab.armed(ctx)))

    def test_pushed_out_counts_as_armed_only_when_idle(self):
        runs = {f"b{i}": ("assist_satellite.c3_b", RELISTEN_RUN) for i in range(10)}
        ctx = _ctx(FakeHa(runs))
        self.assertEqual(asyncio.run(ab.own_runs(ctx)), [])
        self.assertTrue(asyncio.run(ab.armed(ctx)))
        self.assertTrue(asyncio.run(ab.wait_ready(ctx, 0.0, timeout=1)))
        busy = _ctx(FakeHa(runs), state="listening")
        self.assertFalse(asyncio.run(ab.wait_ready(busy, 0.0, timeout=0.2)))

    def test_run_without_events_is_decided_later(self):
        ha = FakeHa({"a1": ("assist_satellite.c3_a", [])})
        ctx = _ctx(ha)
        self.assertEqual(asyncio.run(ab.own_runs(ctx)), [])
        self.assertNotIn("a1", ctx.run_owner)
        ha.runs["a1"] = ("assist_satellite.c3_a", ARMED)
        self.assertEqual(len(asyncio.run(ab.own_runs(ctx))), 1)

    def test_run_satellite_and_run_armed(self):
        self.assertIsNone(L.run_satellite({"events": []}))
        self.assertEqual(L.run_satellite({"events": [{"type": "run-start", "data": {"satellite_id": "x"}}]}), "x")
        self.assertTrue(L.run_armed({"events": [{"type": t} for t in ARMED]}))
        self.assertFalse(L.run_armed({"events": [{"type": t} for t in DONE]}))
        self.assertFalse(L.run_armed({"events": [{"type": t} for t in RELISTEN_RUN]}))


class FalseWakes(unittest.TestCase):
    """Fix (b): continued-conversation re-listens are not wake word false wakes."""

    def test_relistens_after_a_reply_are_separated(self):
        ch = [(100.0, "idle", "listening"),                       # a real false wake
              (101.0, "listening", "processing"), (103.0, "processing", "responding"),
              (110.0, "responding", "idle"), (110.4, "idle", "listening"),   # re-listen, 0.4 s
              (115.0, "listening", "idle"),
              (120.0, "idle", "listening"), (121.0, "listening", "responding"),
              (125.0, "responding", "idle"), (126.9, "idle", "listening"),   # re-listen, 1.9 s (EXP-021 block 8)
              (130.0, "listening", "idle"),
              (140.0, "responding", "idle"), (145.0, "idle", "listening"),   # 5 s of idle: a wake
              (200.0, "idle", "listening")]                       # inside a trial window: ignored
        self.assertEqual(L.classify_wakes(ch, [(199.0, 205.0)], 0, 1000), (3, 2))
        self.assertEqual(L.classify_wakes(ch, [], 0, 1000, gap_s=0.3), (6, 0))
        self.assertEqual(L.classify_wakes(ch, [], 105, 128), (1, 2))

    def test_log_wakes_count_only_wake_word_lines(self):
        text = "\n".join([
            "2026-11-05T19:00:00.000Z INFO  streaming mic audio to HA (wake word stage)",
            "2026-11-05T19:00:10.000Z INFO  wake word detected by HA",
            "2026-11-05T19:00:20.000Z INFO  streaming mic audio to HA (speech-to-text)",
            "2026-11-05T19:00:40.000Z INFO  wake word detected by HA"])
        ents = L.parse_aec_lines(text)
        self.assertEqual([e["kind"] for e in ents], ["streaming", "wake", "relisten", "wake"])
        t0 = L.parse_ts("2026-11-05T19:00:00Z")
        # TV clock 5 s ahead of the PC: the TV's 19:00:40 is the PC's t0 + 35; the trial window covers it
        self.assertEqual(L.log_wakes(ents, [], t0 - 10, t0 + 60, skew_s=5.0), 2)
        self.assertEqual(L.log_wakes(ents, [(t0 + 34, t0 + 36)], t0 - 10, t0 + 60, skew_s=5.0), 1)
        self.assertIsNone(L.log_wakes(ents, [], t0 + 100, t0 + 200))      # no log for that span: unknown

    def test_runner_block_record_and_blocks_csv(self):
        plan = L.parse_plan(_raw_plan())
        self.assertEqual(plan["timing"]["relisten_gap_s"], L.RELISTEN_GAP_S)
        t0 = L.parse_ts("2026-11-05T19:00:00Z")
        with tempfile.TemporaryDirectory() as d:
            out = Path(d)
            item = {"id": "i0", "duration_ms": 1000, "subkind": "alone", "voice": "v"}
            rec = {"arm": "OFF", "condition": "C10", "block": 1, "set": "S1", "item": item, "detections_s": [],
                   "stt_text": None, "valid": True, "notes": [], "t_play": t0 + 50, "lead_s": 1.0,
                   "sink_latency_s": 0.2, "item_end": t0 + 52}
            (out / "trials.jsonl").write_text(json.dumps(rec) + "\n")
            b = {"block": 1, "arm": "OFF", "condition": "C10", "set": "S1", "trials": 1, "t_start": t0,
                 "t_end": t0 + 100, "content_s": 100, "false_wakes": 1, "relistens": 4, "daemon_cpu_pct": 1.2,
                 "notes": []}
            (out / "blocks.jsonl").write_text(json.dumps(b) + "\n")
            ents = L.parse_aec_lines("2026-11-05T19:00:10.000Z INFO  wake word detected by HA\n"
                                     "2026-11-05T19:00:51.000Z INFO  wake word detected by HA\n")
            ab.rebuild_csv(plan, "r", out, ents, 0.0)
            rows = list(csv.DictReader(open(out / "blocks.csv")))
        self.assertEqual((rows[0]["false_wakes"], rows[0]["relistens"], rows[0]["false_wakes_log"]), ("1", "4", "1"))


class RestartWait(unittest.TestCase):
    """Fix (d): after a restart, only the new process's lines count."""

    OLD = ["2026-11-05T19:00:00.000Z INFO  webos-satellite 0.4.3 (armv7-unknown-linux-musleabi) ESPHome API on 0.0.0.0:6053",
           "2026-11-05T19:00:03.000Z INFO  streaming mic audio to HA (wake word stage)",
           "2026-11-05T19:10:00.400Z INFO  streaming mic audio to HA (wake word stage)"]   # old process, restart second
    NEW = ["2026-11-05T19:10:01.200Z INFO  webos-satellite 0.4.3 (armv7-unknown-linux-musleabi) ESPHome API on 0.0.0.0:6053",
           "2026-11-05T19:10:02.000Z INFO  aec: echo delay 16.5 ms (mic behind reference, confidence 11.0)",
           "2026-11-05T19:10:04.000Z INFO  streaming mic audio to HA (wake word stage)"]

    def test_old_process_streaming_line_is_not_accepted(self):
        t_restart = L.parse_ts("2026-11-05T19:10:00Z")
        old = L.parse_aec_lines("\n".join(self.OLD))
        self.assertEqual(old[0]["kind"], "start")
        self.assertEqual(old[0]["version"], "0.4.3")
        self.assertFalse(L.streaming_after_start(old, t_restart))
        self.assertFalse(L.streaming_after_start(L.parse_aec_lines("\n".join(self.OLD + self.NEW[:2])), t_restart))
        both = L.parse_aec_lines("\n".join(self.OLD + self.NEW))
        self.assertTrue(L.streaming_after_start(both, t_restart))
        self.assertEqual([e["kind"] for e in L.since_start(both, t_restart)], ["lock", "streaming"])

    def _tv(self, pids, logs):
        def run(cmd, input_bytes=None, timeout=120):
            remote = cmd[-1]
            if "/proc/$P" in remote:
                p = pids.pop(0) if len(pids) > 1 else pids[0]
                return 0, (b"%d\n" % p) if p else b""
            if "date -u" in remote:
                return 0, b"%d\n" % int(L.parse_ts("2026-11-05T19:10:00Z"))
            if "webos-satellite.log" in remote:
                return 0, "\n".join(logs).encode()
            return 0, b""
        return ab.Tv("root@tv", run=run)

    def test_wait_streaming_needs_a_new_pid_and_its_own_line(self):
        sleeps = []
        with unittest.mock.patch.object(ab.time, "sleep", sleeps.append):
            tv = self._tv([111], self.OLD)
            t = tv.restart()
            self.assertEqual(tv.old_pid, 111)
            self.assertFalse(tv.wait_streaming(t, timeout=0.05))           # same PID: the old process
            tv = self._tv([111, 0, 222], self.OLD)                         # restart, gap, new PID
            t = tv.restart()
            self.assertFalse(tv.wait_streaming(t, timeout=0.05))           # new PID, but only old lines yet
            tv = self._tv([111, 222], self.OLD + self.NEW)
            t = tv.restart()
            self.assertTrue(tv.wait_streaming(t, timeout=5))
            self.assertEqual(tv.wait_lock(t, timeout=5), 16.5)
            tv = self._tv([111, 222], self.OLD)
            self.assertIsNone(tv.wait_lock(tv.restart(), timeout=0.05))


class LogCollection(unittest.TestCase):
    """Fix (c): the TV log is collected during long waits, not only after a block."""

    def _ctx(self, every):
        lines = []

        class T:
            def aec_log_lines(self):
                lines.append(1)
                return ["2026-11-05T19:10:01.200Z INFO  webos-satellite 0.4.3 (armv7-unknown-linux-musleabi) "
                        "ESPHome API on 192.168.1.5:6053",
                        "2026-11-05T19:10:05.000Z INFO  wake word detected by HA"]
        ctx = ab.Ctx()
        ctx.tv, ctx.plan = T(), {"timing": {"log_every_s": every}}
        return ctx, lines

    def test_throttled_and_forced(self):
        with tempfile.TemporaryDirectory() as d:
            ctx, calls = self._ctx(3600.0)
            ctx.out = Path(d)
            self.assertTrue(asyncio.run(ab.maybe_collect_log(ctx)))
            self.assertFalse(asyncio.run(ab.maybe_collect_log(ctx)))
            self.assertTrue(asyncio.run(ab.maybe_collect_log(ctx, force=True)))
            self.assertEqual(len(calls), 2)
            text = (Path(d) / "tvlog-aec.txt").read_text()
        self.assertIn("wake word detected by HA", text)
        self.assertIn("ESPHome API on (redacted)", text)
        self.assertNotIn("192.168", text)

    def test_long_waits_collect(self):
        with tempfile.TemporaryDirectory() as d:
            ctx, calls = self._ctx(0.0)
            ctx.out = Path(d)
            asyncio.run(ab.sleep_collecting(ctx, 0.05))
            self.assertGreaterEqual(len(calls), 1)
            n = len(calls)
            ctx.ha, ctx.pid, ctx.run_owner = FakeHa({}), "p", {}
            ctx.args = type("A", (), {"entity": "assist_satellite.c3_a"})()
            ctx.watch = type("W", (), {"state": "responding"})()        # busy: wait_ready keeps waiting
            self.assertFalse(asyncio.run(ab.wait_ready(ctx, 0.0, timeout=0.3)))
            self.assertGreater(len(calls), n)

    def test_collect_errors_are_not_raised(self):
        class Broken:
            def aec_log_lines(self):
                raise RuntimeError("ssh failed")
        with tempfile.TemporaryDirectory() as d:
            ctx = ab.Ctx()
            ctx.tv, ctx.plan, ctx.out = Broken(), {"timing": {}}, Path(d)
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertFalse(asyncio.run(ab.maybe_collect_log(ctx)))


class Stats(unittest.TestCase):
    def test_mcnemar_exact(self):
        self.assertEqual(L.mcnemar_exact(0, 0), 1.0)
        self.assertEqual(L.mcnemar_exact(4, 4), 1.0)
        self.assertAlmostEqual(L.mcnemar_exact(5, 2), 0.453125)
        self.assertAlmostEqual(L.mcnemar_exact(10, 3), 0.0923, places=4)
        self.assertAlmostEqual(L.mcnemar_exact(11, 4), 0.1185, places=4)
        self.assertAlmostEqual(L.mcnemar_exact(10, 0), 2 / 1024)
        self.assertEqual(L.mcnemar_exact(3, 7), L.mcnemar_exact(7, 3))

    def test_report_pools_runs_and_pairs_by_run_and_item(self):
        def row(run, arm, item, det, cond="C10", vol="10", tl="natural"):
            r = {c: "" for c in L.CSV_COLUMNS}
            r.update(run_id=run, arm=arm, condition=cond, tv_volume=vol, talker_level=tl, item_id=item, detected=str(det))
            return r
        rows = []
        for run in ("r1", "r2"):             # the same item ids in two runs are different pairs
            for i in range(5):
                rows.append(row(run, "OFF", f"i{i}", 0))
                rows.append(row(run, "B+NS", f"i{i}", 1 if i < 4 else 0))
        rows.append(row("r1", "B+NS", "x", ""))      # invalid trial: skipped
        rows.append(row("r1", "OFF", "i0", 1, tl="loud", vol="15", cond="C15"))   # another cell
        cells, skipped = rep.analyze(rows)
        self.assertEqual(skipped, 1)
        c10 = {c["arm"]: c for c in cells if c["condition"] == "C10"}
        self.assertEqual((c10["OFF"]["k"], c10["OFF"]["n"]), (0, 10))
        self.assertEqual((c10["B+NS"]["k"], c10["B+NS"]["n"]), (8, 10))
        self.assertEqual(c10["B+NS"]["paired"]["arm_only"], 8)
        self.assertEqual(c10["B+NS"]["paired"]["base_only"], 0)
        self.assertAlmostEqual(c10["B+NS"]["paired"]["p"], 2 / 256)
        self.assertEqual(c10["B+NS"]["runs"], ["r1", "r2"])
        self.assertEqual(len([c for c in cells if c["talker_level"] == "loud"]), 1)
        self.assertIn("B+NS", rep.format_table(cells))


if __name__ == "__main__":
    unittest.main()
