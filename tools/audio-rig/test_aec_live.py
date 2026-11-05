"""Unit tests for the live AEC A/B tools (plan, schedule, config editing, safety, log parsing, CSV, McNemar).

No audio, no network, no device. Run: python3 -m unittest discover -s tools/audio-rig -p 'test_aec_live.py'
"""
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
import unittest
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
        self.assertEqual(plan["arms"]["OFF"]["aec"], "")
        self.assertIn("ns = true", plan["arms"]["B+NS"]["aec"])
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
                         ["on", "lock", "capture", "window", "change", "window", "window", "streaming"])

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
