"""Unit tests for dogfood_report.py. Synthetic fixtures only, built by hand from the daemon's
and the scripts' format strings (crates/satellite/src/daemon.rs, packaging/webos/supervise.sh,
autostart.sh); the dates are fictional.

Run: python3 -m unittest discover -s tools -p 'test_dogfood*.py'
"""
import base64
import contextlib
import hashlib
import io
import json
import os
import socket
import struct
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dogfood_report as d  # noqa: E402

SENTINEL = "CANARY-SENTENCE-DO-NOT-LEAK"


def L(hms, msg, level="INFO ", day="2030-05-06"):
    """A daemon log line: `UTC-timestamp LEVEL message`, level padded to 5 like the logger."""
    return f"{day}T{hms}Z {level} {msg}"


def S(hms, msg, day="2030-05-06"):
    return f"{day}T{hms[:8]}Z SUPER {msg}"


def A(hms, msg, day="2030-05-06"):
    return f"{day}T{hms[:8]}Z autostart: {msg}"


START = "webos-satellite 9.9.9 (aarch64-unknown-linux-musl) ESPHome API on 0.0.0.0:6053"
LINES = [
    S("10:00:00", "supervisor started (pid 100; give up after 5 exits within 600s)"),
    L("10:00:00.500", START),
    L("10:00:00.600", "config: name=sat port=6053 voice_features=23 mode=Wake encryption=noise mic=[alsa] dsp=[Left] aec=[off] x=1"),
    L("10:00:00.700", "power: state Active"),
    L("10:00:00.800", "capture: microphone open (2 ch, 16000 Hz, Alsa)"),
    L("10:00:05.000", "HA connected: Home Assistant 2030.1.1 from 192.0.2.7"),
    L("10:00:05.100", "streaming mic audio to HA (wake word stage)"),
    # run 1: completed
    L("10:10:00.000", "wake word detected by HA"),
    L("10:10:01.000", "stopped streaming mic audio"),
    L("10:10:02.000", "speech-to-text done (14 chars)"),
    L("10:10:02.500", "intent done (reply 20 chars, continue_conversation=false)"),
    L("10:10:02.600", "TTS stream start (22050 Hz, 1 ch, 2 B)"),
    L("10:10:02.700", "pipeline run end"),
    L("10:10:05.000", "playback 1 done: 1800 ms of audio, 79000 bytes, 79000 non-zero samples"),
    L("10:10:05.000", "run 1 latency ms (from wake word): stt_end=2000 intent_end=2500 tts_start=2600 playback_end=5000 wake_after_stream_start=595000 reply_audio=1800"),
    L("10:10:05.200", "streaming mic audio to HA (wake word stage)"),
    # run 2: intent error, DEBUG line with a canary must be ignored
    L("10:20:00.000", "wake word detected by HA"),
    L("10:20:02.000", "speech-to-text done (9 chars)"),
    L("10:20:02.100", f"transcript: \"{SENTINEL}\"", level="DEBUG"),
    L("10:20:02.500", "voice: HA error intent-failed"),
    L("10:20:02.500", "HA pipeline error intent-failed", level="WARN "),
    L("10:20:02.600", "pipeline run end"),
    L("10:20:02.600", "run 2 latency ms (from wake word): stt_end=2000 intent_end=- tts_start=- playback_end=- wake_after_stream_start=594000 error=intent-failed"),
    L("10:20:02.800", "streaming mic audio to HA (wake word stage)"),
    # run 3: idle wake-stage cycle ended without a wake
    L("10:27:00.000", "voice: run failed (1 in a row); retry in 250 ms"),
    L("10:27:00.000", "stopped streaming mic audio"),
    L("10:27:00.000", "pipeline run end"),
    L("10:27:00.000", "run 3 latency ms (from wake word): stt_end=- intent_end=- tts_start=- playback_end=- wake_after_stream_start=-"),
    L("10:27:00.300", "streaming mic audio to HA (wake word stage)"),
    # run 4: empty STT
    L("10:30:00.000", "wake word detected by HA"),
    L("10:30:03.000", "HA pipeline error stt-no-text-recognized", level="WARN "),
    L("10:30:03.100", "pipeline run end"),
    L("10:30:03.100", "run 4 latency ms (from wake word): stt_end=- intent_end=- tts_start=- playback_end=- wake_after_stream_start=1000 error=stt-no-text-recognized"),
    L("10:30:03.300", "streaming mic audio to HA (wake word stage)"),
    # run 5: wake then disconnect (retry follows within 30 s)
    L("10:30:20.000", "wake word detected by HA"),
    L("10:30:21.000", "esphome-api: HA connection 192.0.2.7:1 closed: i/o error: Connection reset by peer (os error 104)"),
    L("10:30:21.000", "HA disconnected", level="WARN "),
    L("10:30:21.000", "run 5 latency ms (from wake word): stt_end=- intent_end=- tts_start=- playback_end=- wake_after_stream_start=2000"),
    L("10:30:30.000", "HA connected: Home Assistant 2030.1.1 from 192.0.2.7"),
    L("10:30:30.100", "streaming mic audio to HA (wake word stage)"),
    # run 6: wake, STT never ends (no error)
    L("10:30:40.000", "wake word detected by HA"),
    L("10:31:10.000", "pipeline run end"),
    L("10:31:10.000", "run 6 latency ms (from wake word): stt_end=- intent_end=- tts_start=- playback_end=- wake_after_stream_start=1500"),
    L("10:31:10.100", "streaming mic audio to HA (speech-to-text)"),
    # run 7: push to talk, completed
    L("10:31:12.000", "speech-to-text done (5 chars)"),
    L("10:31:12.500", "intent done (reply 0 chars, continue_conversation=false)"),
    L("10:31:12.600", "pipeline run end"),
    L("10:31:12.600", "run 7 latency ms (from start): stt_end=1900 intent_end=2400 tts_start=- playback_end=-"),
    # mic, mute, power
    L("10:40:00.000", "microphone MUTED by Home Assistant"),
    L("10:40:00.001", "capture: microphone closed, PCM released (muted)"),
    L("10:50:00.000", "microphone unmuted by Home Assistant"),
    L("10:50:00.010", "capture: microphone open (2 ch, 16000 Hz, Alsa)"),
    L("10:55:00.000", "power: state Active Standby, processing Prepare Active Standby, reason remoteKey, onOff off"),
    L("11:05:00.000", "power: state Active, processing Screen On, reason localKey, onOff on"),
    L("11:10:00.000", "memory: resident size 12000 KiB (last reported 0 KiB)"),
    L("11:20:00.000", "SIGTERM: shutting down"),
    L("11:20:00.500", "stopped"),
    S("11:20:00", "stopped by signal (daemon exit 0)"),
    # a crash restart after a watchdog exit
    S("11:30:00", "supervisor started (pid 200; give up after 5 exits within 600s)"),
    L("11:30:00.400", START),
    L("11:35:00.000", "watchdog: event loop made no progress for 30 s; exiting with 70 for a restart", level="ERROR"),
    S("11:35:00", "daemon exited with status 70 after 300s (1 of 5 within 600s)"),
    S("11:35:00", "restarting in 1s"),
    L("11:35:01.500", START),
    "thread 'main' panicked at src/x.rs:1:1:",
    L("11:36:00.000", "stopped streaming mic audio"),
]
AUTOSTART = [
    A("09:59:30", "firmware 99.1 (webOS 9.9.9, build 1): validated; starting"),
    A("09:59:31", "start.sh exited with status 0"),
]


def model(lines=LINES, auto=AUTOSTART, running=False):
    P = d.build_model(lines, auto, {"running": running}, d.dt.timezone.utc, 0, 1e12, d.to_epoch(d.TS_LINE.match(lines[-1])))
    return P


def ts(hms, day="2030-05-06"):
    return d.to_epoch(d.TS_LINE.match(f"{day}T{hms}Z INFO  x"))


class Parse(unittest.TestCase):
    def setUp(self):
        self.P = model()
        self.by = {r["n"]: r for r in self.P["runs"]}

    def test_outcomes(self):
        want = {1: "completed", 2: "intent_error", 3: "idle", 4: "stt_empty", 5: "disconnect",
                6: "stt_timeout_or_no_speech_end", 7: "completed_no_reply"}
        got = {n: d.classify(r) for n, r in self.by.items()}
        self.assertEqual(got, want)

    def test_run_fields_and_anchor(self):
        r = self.by[1]
        self.assertEqual((r["stt_ms"], r["intent_ms"], r["tts_ms"], r["play_ms"]), (2000, 2500, 2600, 5000))
        self.assertEqual(r["kind"], "wake")
        self.assertAlmostEqual(d.anchor(r), ts("10:10:00"), places=3)
        self.assertEqual(self.by[7]["kind"], "ptt")
        self.assertEqual(self.by[2]["error"], "intent-failed")
        # idle cycles anchor at the stream start, not at the line time
        self.assertAlmostEqual(d.anchor(self.by[3]), ts("10:20:02.800"), places=3)

    def test_debug_lines_are_discarded_unread(self):
        self.assertEqual(self.P["stats"]["debug_lines_discarded"], 1)
        self.assertNotIn(SENTINEL, json.dumps(self.P, default=str))

    def test_unknown_error_code_is_not_kept_verbatim(self):
        line = L("10:00:00.000", "run 1 latency ms (from wake word): stt_end=- intent_end=- tts_start=- "
                 "playback_end=- wake_after_stream_start=5 error=" + "x" * 80)
        P = d.parse_log([line])
        self.assertEqual(P["runs"][0]["error"], "other")

    def test_untimestamped_panic_line(self):
        self.assertEqual(sum(1 for _, k, _ in self.P["events"] if k == "panic"), 1)


class Health(unittest.TestCase):
    def setUp(self):
        self.P = model()

    def test_sessions_and_start_causes(self):
        causes = [(s["cause"], s["status"]) for s in self.P["starts"]]
        self.assertEqual(causes, [("boot (autostart)", None), ("manual or ctl", None), ("crash restart", 70)])
        self.assertEqual(len(self.P["sessions"]), 3)
        self.assertEqual(self.P["sessions"][0][3], "stopped")

    def test_window_counts(self):
        w = d.summarize_window(self.P, self.P["first_ts"], 1e12, d.dt.timezone.utc)
        dm = w["daemon"]
        self.assertEqual(dm["starts"], 3)
        self.assertEqual(dm["supervisor_exits"], {"watchdog: event loop stall": 1})
        self.assertEqual(dm["supervisor_restarts"], 1)
        self.assertEqual(dm["watchdog"]["event_loop"], 1)
        self.assertEqual(dm["panics"], 1)
        self.assertEqual(dm["boot_starts"], 1)
        self.assertEqual(dm["rss_kib"]["last"], 12000)
        self.assertEqual(w["ha"]["connects"], 2)
        self.assertEqual(w["ha"]["disconnects"], 1)
        self.assertEqual(w["ha"]["disconnect_reasons"], {"reset": 1})
        self.assertEqual(w["mic"]["mute_by_ha"], 1)
        self.assertEqual(w["mic"]["seconds_by_state"]["off: muted"], 600)
        self.assertEqual(w["power"]["standby_entries"], 1)
        self.assertEqual(w["power"]["seconds_by_state"]["Active Standby"], 600)
        self.assertEqual(w["power"]["remote_power_off_requests"], 0)

    def test_pipeline_block(self):
        w = d.summarize_window(self.P, self.P["first_ts"], 1e12, d.dt.timezone.utc)["pipeline"]
        self.assertEqual(w["wakes_detected"], 5)
        self.assertEqual(w["attempts"], 6)
        self.assertEqual(w["wake_stage_cycles_without_wake"], 1)
        self.assertEqual(w["push_to_talk_runs"], 1)
        self.assertEqual(w["outcomes"]["completed"], 1)
        self.assertEqual(w["ok_pct"], round(100 * 2 / 6, 1))
        self.assertEqual(w["error_codes"], {"intent-failed": 1, "stt-no-text-recognized": 1})
        self.assertEqual(w["failed_then_retried_within_30s"], 2)  # run 4 -> 5 (17 s), run 5 -> 6 (19 s)
        self.assertEqual(w["latency_ms_from_wake"]["stt_end"]["n"], 2)  # runs 1 and 2 (run 7 is push-to-talk)
        self.assertEqual(w["latency_ms_from_wake"]["playback_end"]["p50"], 5000)
        self.assertEqual(w["by_power_state"]["Active"]["attempts"], 6)
        hours = {x["hour"]: x for x in w["by_hour"]}
        self.assertEqual(hours[10]["wakes"], 5)

    def test_window_clipping_by_day(self):
        days = list(d.local_days(ts("00:00:00") - 3600, ts("00:00:00") + 2 * 86400, d.dt.timezone.utc))
        self.assertEqual([x[0] for x in days], ["2030-05-05", "2030-05-06", "2030-05-07"])
        w = d.summarize_window(self.P, ts("00:00:00"), ts("00:00:00") + 86400, d.dt.timezone.utc)
        self.assertEqual(w["daemon"]["starts"], 3)
        w2 = d.summarize_window(self.P, ts("00:00:00") + 86400, ts("00:00:00") + 2 * 86400, d.dt.timezone.utc)
        self.assertEqual(w2["daemon"]["starts"], 0)

    def test_crash_loop_and_exit_codes(self):
        lines = [S("10:00:00", "supervisor started (pid 1; give up after 5 exits within 600s)"),
                 L("10:00:00.1", START),
                 S("10:01:00", "daemon exited with status 72 after 60s (1 of 5 within 600s)"),
                 S("10:01:00", "restarting in 1s"), L("10:01:01.1", START),
                 S("10:02:00", "daemon exited with status 1 after 60s (2 of 5 within 600s)"),
                 S("10:02:00", "CRASH LOOP: giving up; wrote /x/crashloop")]
        P = model(lines)
        w = d.summarize_window(P, P["first_ts"], 1e12, d.dt.timezone.utc)["daemon"]
        self.assertEqual(w["supervisor_exits"], {"watchdog: memory ceiling": 1, "exit status 1": 1})
        self.assertEqual(w["crash_loop_giveups"], 1)

    def test_running_session_extends_to_now(self):
        P = d.build_model(LINES, AUTOSTART, {"running": True}, d.dt.timezone.utc, 0, 1e12, ts("12:00:00"))
        self.assertEqual(P["sessions"][-1][3], "running")
        self.assertEqual(P["sessions"][-1][1], ts("12:00:00"))


class Stats(unittest.TestCase):
    def test_percentiles(self):
        self.assertEqual(d.pctl([], 50), None)
        self.assertEqual(d.pctl([5], 95), 5)
        self.assertEqual(d.pctl([1000, 2000, 3000, 4000, 5000], 50), 3000)
        self.assertEqual(d.pctl([0, 100], 90), 90)

    def test_autostart_parse(self):
        got = d.parse_autostart(AUTOSTART + [A("10:00:00", "kill switch /tmp/x present; not starting")])
        self.assertEqual([k for _, k in got], ["start", "refused"])


class Archive(unittest.TestCase):
    def test_sanitize_and_merge(self):
        raw = [L("10:00:00.000", "HA connected: Home Assistant 1.2.3 from 192.0.2.7"),
               L("10:00:01.000", f"transcript: {SENTINEL}", level="DEBUG"), "garbage"]
        san = d.sanitize_for_archive(raw)
        self.assertEqual(len(san), 1)
        self.assertNotIn("192.0.2.7", san[0])
        new = d.sanitize_for_archive(raw + [L("10:00:02.000", "stopped streaming mic audio")])
        merged = d.merge_archive(san, new)
        self.assertEqual(len(merged), 2)
        self.assertEqual(d.merge_archive(merged, new), merged)  # idempotent

    def test_ignored_path_check(self):
        self.assertTrue(d.under_ignored_path("/nonexistent-elsewhere/x"))
        self.assertTrue(d.under_ignored_path(os.path.join(d.REPO, "devices", "private", "dogfood")))
        self.assertFalse(d.under_ignored_path(os.path.join(d.REPO, "tools", "out")))


# ---------------------------------------------------------------- HA

HA_EVENTS = [
    {"type": "run-start", "timestamp": "2030-05-06T10:20:00+00:00", "data": {"pipeline": "p", "language": "es"}},
    {"type": "wake_word-start", "timestamp": "2030-05-06T10:20:00.100+00:00", "data": {}},
    {"type": "wake_word-end", "timestamp": "2030-05-06T10:20:00.200+00:00",
     "data": {"wake_word_output": {"wake_word_id": "alexa", "timestamp": 1}}},
    {"type": "stt-start", "timestamp": "2030-05-06T10:20:00.300+00:00", "data": {"engine": "stt.x"}},
    {"type": "stt-vad-start", "timestamp": "2030-05-06T10:20:00.900+00:00", "data": {}},
    {"type": "stt-end", "timestamp": "2030-05-06T10:20:02+00:00", "data": {"stt_output": {"text": SENTINEL}}},
    {"type": "intent-start", "timestamp": "2030-05-06T10:20:02.100+00:00", "data": {"intent_input": SENTINEL}},
    {"type": "intent-end", "timestamp": "2030-05-06T10:20:02.500+00:00", "data": {
        "processed_locally": False,
        "intent_output": {"response": {"response_type": "query_answer", "speech": {"plain": {"speech": SENTINEL + "!"}}}}}},
    {"type": "tts-end", "timestamp": "2030-05-06T10:20:03+00:00", "data": {"tts_output": {"url": "http://x/" + SENTINEL}}},
    {"type": "run-end", "timestamp": "2030-05-06T10:20:03.100+00:00", "data": None},
]


class HaSummary(unittest.TestCase):
    def test_whitelist_and_timings(self):
        o = d.summarize_ha_run("run-abc", "2030-05-06T10:20:00+00:00", HA_EVENTS)
        self.assertNotIn(SENTINEL, json.dumps(o))
        self.assertTrue(o["wake_detected"] and o["stt_text"] and o["intent"] and o["tts"] and o["ended"])
        self.assertEqual(o["stt_chars"], len(SENTINEL))
        self.assertEqual(o["reply_chars"], len(SENTINEL) + 1)
        self.assertEqual(o["intent_type"], "query_answer")
        self.assertEqual(o["ms_from_wake_or_start"]["stt_end"], 1800)

    def test_empty_stt_error_message_is_dropped(self):
        ev = [{"type": "run-start", "timestamp": "2030-05-06T10:30:00+00:00", "data": {}},
              {"type": "error", "timestamp": "2030-05-06T10:30:03+00:00",
               "data": {"code": "stt-no-text-recognized", "message": SENTINEL}}]
        o = d.summarize_ha_run("r2", "2030-05-06T10:30:00+00:00", ev)
        self.assertTrue(o["stt_empty"])
        self.assertEqual(o["error"], "stt-no-text-recognized")
        self.assertNotIn(SENTINEL, json.dumps(o))
        self.assertFalse(o["stt_text"])

    def test_weird_values_are_not_copied(self):
        ev = [{"type": "error", "timestamp": "2030-05-06T10:30:03+00:00", "data": {"code": SENTINEL + " with spaces"}},
              {"type": "intent-end", "timestamp": "2030-05-06T10:30:03+00:00",
               "data": {"intent_output": {"response": {"response_type": "error", "data": {"code": "no intent " + SENTINEL}}}}}]
        o = d.summarize_ha_run("r3 " + SENTINEL, "2030-05-06T10:30:00+00:00", ev)
        self.assertNotIn(SENTINEL, json.dumps(o))

    def test_section_matches_tv_attempts(self):
        P = model()
        runs = [d.summarize_ha_run("a", "2030-05-06T10:10:05.3+00:00", HA_EVENTS),  # TV run 2 streamed from 10:10:05.2
                d.summarize_ha_run("b", "2030-05-06T10:27:00.4+00:00",
                                   [{"type": "error", "timestamp": "2030-05-06T10:30:03+00:00",
                                     "data": {"code": "stt-no-text-recognized", "message": SENTINEL}}])]
        runs[0]["ts"] = ts("10:10:05.3")
        runs[1]["ts"] = ts("10:27:00.4")
        sec = d.ha_section({"pipeline": "Kiwi", "runs": runs}, P["runs"], 0, 1e12)
        self.assertNotIn(SENTINEL, json.dumps(sec))
        self.assertEqual(sec["counts"]["stt_empty"], 1)
        pairs = {(x["tv_outcome"], x["ha_stt"]): x["n"] for x in sec["matched_with_tv_attempts"]}
        self.assertEqual(pairs.get(("intent_error", "stt_text")), 1)  # TV run 2 vs HA run a
        self.assertEqual(pairs.get(("stt_empty", "stt_empty")), 1)  # TV run 4 (stream 10:27:00.3) vs HA run b

    def test_cache_merges_by_id_and_is_private(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "c.json")
            o = d.summarize_ha_run("a", "2030-05-06T10:20:00+00:00", HA_EVENTS)
            d.merge_ha_cache(p, [o])
            runs = d.merge_ha_cache(p, [o, d.summarize_ha_run("b", "2030-05-06T10:21:00+00:00", [])])
            self.assertEqual([r["id"] for r in runs], ["a", "b"])
            self.assertEqual(os.stat(p).st_mode & 0o777, 0o600)
            with open(p) as f:
                self.assertNotIn(SENTINEL, f.read())


class FakeHa(threading.Thread):
    """A plain-TCP websocket server speaking HA's auth and the assist_pipeline commands."""

    def __init__(self, token):
        super().__init__(daemon=True)
        self.token = token
        self.srv = socket.socket()
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(1)
        self.port = self.srv.getsockname()[1]
        self.seen = []

    def _recv_frame(self, c):
        def rd(n):
            b = b""
            while len(b) < n:
                x = c.recv(n - len(b))
                if not x:
                    raise EOFError
                b += x
            return b
        b0, b1 = rd(2)
        n = b1 & 0x7F
        if n == 126:
            n = struct.unpack(">H", rd(2))[0]
        mask = rd(4)
        data = bytes(x ^ mask[i % 4] for i, x in enumerate(rd(n)))
        return json.loads(data)

    def _send(self, c, obj):
        data = json.dumps(obj).encode()
        hdr = bytes([0x81]) + (bytes([len(data)]) if len(data) < 126 else bytes([126]) + struct.pack(">H", len(data)))
        c.sendall(hdr + data)

    def run(self):
        c, _ = self.srv.accept()
        req = b""
        while b"\r\n\r\n" not in req:
            req += c.recv(4096)
        key = [ln.split(b": ")[1] for ln in req.split(b"\r\n") if ln.lower().startswith(b"sec-websocket-key")][0].decode()
        self.path = req.split(b" ")[1].decode()
        acc = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        c.sendall(f"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {acc}\r\n\r\n".encode())
        self._send(c, {"type": "auth_required"})
        auth = self._recv_frame(c)
        if auth.get("access_token") != self.token:
            self._send(c, {"type": "auth_invalid"})
            c.close()
            self.srv.close()
            return
        self._send(c, {"type": "auth_ok"})
        try:
            while True:
                m = self._recv_frame(c)
                self.seen.append(m["type"])
                if m["type"] == "assist_pipeline/pipeline/list":
                    res = {"pipelines": [{"id": "p1", "name": "Kiwi"}, {"id": "p2", "name": "Other"}], "preferred_pipeline": "p1"}
                elif m["type"] == "assist_pipeline/pipeline_debug/list":
                    res = {"pipeline_runs": [{"pipeline_run_id": "run1", "timestamp": "2030-05-06T10:20:00+00:00"}]}
                elif m["type"] == "assist_pipeline/pipeline_debug/get":
                    res = {"events": HA_EVENTS}
                else:
                    self._send(c, {"id": m["id"], "type": "result", "success": False, "error": {"code": "unknown_command"}})
                    continue
                self._send(c, {"id": m["id"], "type": "event", "event": {}})  # noise the client must skip
                self._send(c, {"id": m["id"], "type": "result", "success": True, "result": res})
        except (EOFError, OSError):
            pass
        finally:
            c.close()
            self.srv.close()


class HaWebsocket(unittest.TestCase):
    def test_collect(self):
        srv = FakeHa("tok-secret")
        srv.start()
        out = d.collect_ha(f"http://127.0.0.1:{srv.port}", "tok-secret", "kiwi", None)
        self.assertEqual(out["pipeline"], "Kiwi")
        self.assertEqual(len(out["runs"]), 1)
        self.assertEqual(out["runs"][0]["intent_type"], "query_answer")
        self.assertNotIn(SENTINEL, json.dumps(out))
        self.assertEqual(srv.path, "/api/websocket")
        self.assertEqual(srv.seen, ["assist_pipeline/pipeline/list", "assist_pipeline/pipeline_debug/list",
                                    "assist_pipeline/pipeline_debug/get"])

    def test_bad_token_message_has_no_token(self):
        srv = FakeHa("right")
        srv.start()
        with self.assertRaises(d.WsError) as cm:
            d.collect_ha(f"http://127.0.0.1:{srv.port}", "wrong-token-value", None, None)
        self.assertNotIn("wrong-token-value", str(cm.exception))

    def test_unknown_pipeline(self):
        srv = FakeHa("t")
        srv.start()
        with self.assertRaises(d.WsError):
            d.collect_ha(f"http://127.0.0.1:{srv.port}", "t", "nope", None)

    def test_read_token_formats(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "tok")
            with open(p, "w") as f:
                f.write('{"access_token": "abc", "refresh_token": "zzz"}')
            os.chmod(p, 0o600)
            self.assertEqual(d.read_token(p), ("abc", False))
            with open(p, "w") as f:
                f.write("plain\n")
            os.chmod(p, 0o644)
            self.assertEqual(d.read_token(p), ("plain", True))


# ---------------------------------------------------------------- end to end

BUNDLE = "\n".join(["@@now", str(int(ts("12:00:00"))), "@@uptime", "5000.00 4000.00", "@@clk", "100", "@@ctl",
                    "webos-satellite: installed: true 9.9.9 in /var/lib/webos-satellite (webos-satellite 9.9.9 (aarch64))",
                    "webos-satellite: running: pid 4321",
                    "webos-satellite: start at boot: on (/x)",
                    "webos-satellite: firmware: firmware 99.1 (webOS 9.9.9, build 1): validated",
                    "@@markers", "@@proc", "pid 4321",
                    "4321 (webos-satellite) S 1 4321 4321 0 -1 0 0 0 0 0 0 0 0 0 20 0 4 0 100000 0 0 0 0",
                    "VmRSS:\t   1280 kB", "Threads:\t4", "@@log1", "@@log"] + LINES + ["@@autostart"] + AUTOSTART + ["@@end"])


class EndToEnd(unittest.TestCase):
    def run_main(self, *extra):
        with tempfile.TemporaryDirectory() as t:
            b = os.path.join(t, "bundle.txt")
            with open(b, "w") as f:
                f.write(BUNDLE)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                d.main(["--bundle", b, "--utc", "--no-ha", "--since", "2030-05-06", "--until", "2030-05-07", "--config", os.devnull,
                        *extra])
            return buf.getvalue()

    def test_markdown(self):
        out = self.run_main()
        for needle in ("# Satellite dogfood report", "intent_error", "crash restart (status 70)", "Not recorded",
                       "Skipped: --no-ha", "pid 4321", "up 1h 6m"):
            self.assertIn(needle, out)
        self.assertNotIn(SENTINEL, out)
        self.assertNotIn("192.0.2.7", out)

    def test_json(self):
        out = self.run_main("--json")
        rep = json.loads(out)
        self.assertEqual(rep["live"]["pid"], 4321)
        self.assertEqual(rep["live"]["rss_kib"], 1280)
        self.assertEqual(rep["live"]["daemon_uptime_s"], 5000 - 1000)
        self.assertEqual(rep["total"]["pipeline"]["wakes_detected"], 5)
        self.assertEqual(list(rep["days"]), ["2030-05-06"])
        self.assertNotIn(SENTINEL, out)
        self.assertNotIn("192.0.2.7", out)

    def test_no_tv_is_an_error(self):
        with self.assertRaises(SystemExit):
            d.main(["--config", os.devnull])


if __name__ == "__main__":
    unittest.main()
