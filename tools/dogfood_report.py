#!/usr/bin/env python3
"""Real-use health report for a webos-satellite on a TV (PC side, Python 3 stdlib only).

Reads the daemon, supervisor and autostart logs over ssh (read-only: cat, date and
`ctl.sh status`, nothing is written on the TV), parses them, and prints per-day and
whole-window numbers: daemon health, HA connection, microphone and power periods,
pipeline outcomes, latency percentiles, runs per hour. Optionally adds Home Assistant's
Assist pipeline debug runs (websocket API).

Privacy: only counts, booleans, lengths, timings and fixed error codes leave the parsers.
Log lines at debug level (which may hold transcripts) are discarded unread. HA event data
is reduced to a whitelist of fields on the spot; transcript, reply and error message
text is never kept, printed or cached. No token or address is printed.

Examples (see docs/development/dogfood-report.md):

  DOGFOOD_TV_HOST=tv.example tools/dogfood_report.py --days 3 --no-ha
  tools/dogfood_report.py --host tv.example --ha-url https://ha.example \\
      --ha-token-file "$TOKEN_FILE" --ha-pipeline Kiwi --json

Why the tool exists: the log records what the daemon saw (wake detected by HA, STT, intent,
playback). It cannot record a wake word HA never detected, nor whether the TV was playing
sound. The "Not recorded" section of the report lists the gaps.
"""
import argparse
import base64
import calendar
import collections
import datetime as dt
import hashlib
import json
import os
import re
import socket
import ssl
import struct
import subprocess
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
DEFAULT_CONFIG = os.path.join(REPO, "devices", "private", "dogfood", "config.json")
DEFAULT_TV_DIR = "/var/lib/webos-satellite"
DEFAULT_CTL = "/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite/payload/ctl.sh"
# assist_pipeline/run.py STORED_PIPELINE_RUNS (checked in HA 2026.10.4); the tool reports
# what it actually sees as well.
HA_STORED_RUNS = 10
HA_MATCH_S = 12.0
RETRY_S = 30.0

SUPERVISOR_STATUS = {70: "watchdog: event loop stall", 71: "watchdog: capture stall", 72: "watchdog: memory ceiling",
                     137: "killed (SIGKILL)", 139: "segmentation fault", 134: "abort"}

# ---------------------------------------------------------------- log parsing

TS_LINE = re.compile(r"^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(?:\.(\d+))?Z\s+(\S+)\s*(.*)$")
RUN_LINE = re.compile(r"^run (\d+) latency ms \(from (wake word|start)\): stt_end=(\S+) intent_end=(\S+) "
                      r"tts_start=(\S+) playback_end=(\S+)(.*)$")
IPV4 = re.compile(r"\b\d{1,3}(?:\.\d{1,3}){3}\b")
CODE_OK = re.compile(r"^[A-Za-z0-9_.-]{1,48}$")

# (kind, regex on the message) for the daemon's INFO/WARN/ERROR lines. Captured groups
# are the only text that survives parsing.
DAEMON_RULES = [
    ("start", r"^webos-satellite (\S+) \((\S+)\) ESPHome API on "),
    ("ha_connect", r"^HA connected: Home Assistant (\S+)"),
    ("ha_close", r"^esphome-api: HA connection \S+ closed: (.*)$"),
    ("ha_disc", r"^HA disconnected$"),
    ("stream_start", r"^streaming mic audio to HA \((wake word stage|speech-to-text)\)"),
    ("stream_stop", r"^stopped streaming mic audio$"),
    ("wake", r"^wake word detected by HA$"),
    ("stt_done", r"^speech-to-text done \((\d+) chars\)"),
    ("intent_done", r"^intent done \(reply (\d+) chars"),
    ("tts_start", r"^TTS (?:stream start|URL)"),
    ("run_end", r"^pipeline run end$"),
    ("pipeline_error", r"^HA pipeline error (\S+)$"),
    ("run_failed", r"^voice: run failed \((\d+) in a row\)"),
    ("playback_done", r"^playback \d+ done: (\d+) ms of audio"),
    ("playback_fail", r"^playback \d+ failed"),
    ("ptt_request", r"^push-to-talk run requested"),
    ("mic_open", r"^capture: microphone open"),
    ("mic_off", r"^capture: microphone (?:off|closed, PCM released) \((.*)\)$"),
    ("mic_busy", r"^capture: MIC BUSY"),
    ("mic_stall", r"^capture: no audio from the device for (\d+) ms"),
    ("mic_error", r"^capture: (?:stream failed|cannot open the microphone)"),
    ("mute_on", r"^microphone MUTED by Home Assistant"),
    ("mute_off", r"^microphone unmuted by Home Assistant"),
    ("unmute_refused", r"^unmute refused"),
    ("kill_on", r"^KILL SWITCH "),
    ("kill_off", r"^kill switch removed"),
    ("capture_stats", r"^capture released \(chunks sent (\d+), dropped (\d+), xruns (\d+)\)"),
    ("power_sub", r"^power: subscribed to the TV power state"),
    ("power", r"^power: state (.*)$"),
    ("rpo_request", r"^remote power-off: request "),
    ("resume", r"^RESUMED from a system suspend"),
    ("wd_loop", r"^watchdog: event loop made no progress"),
    ("wd_capture", r"^watchdog: capture thread made no progress"),
    ("wd_rss", r"^watchdog: resident memory (\d+) KiB is above"),
    ("wd_suspended", r"^watchdog: the process did not run"),
    ("rss", r"^memory: resident size (\d+) KiB"),
    ("shutdown", r"^SIG(?:TERM|INT): shutting down"),
    ("stopped", r"^stopped$"),
    ("config", r"^config: .*?\bmode=(\w+).*?\baec=\[([^\]\s]*)"),
    ("aec_on", r"^aec: echo cancellation on"),
    ("aec_without", r"^aec: .*running WITHOUT echo cancellation"),
    ("aec_delay", r"^aec: echo delay"),
]
DAEMON_RULES = [(k, re.compile(r)) for k, r in DAEMON_RULES]

SUPER_RULES = [
    ("sup_start", re.compile(r"^supervisor started")),
    ("sup_exit", re.compile(r"^daemon exited with status (\d+) after (\d+)s")),
    ("sup_restart", re.compile(r"^restarting in (\d+)s")),
    ("sup_stopped", re.compile(r"^stopped by signal \(daemon exit (\d+)\)")),
    ("sup_normal", re.compile(r"^daemon exited normally")),
    ("crashloop", re.compile(r"^CRASH LOOP")),
    ("sup_refused", re.compile(r"not starting")),
]
PANIC = re.compile(r"panicked at|Segmentation fault|SIGSEGV|stack overflow")


def to_epoch(m):
    y, mo, d, h, mi, s = (int(m.group(i)) for i in range(1, 7))
    frac = float("0." + m.group(7)) if m.group(7) else 0.0
    return calendar.timegm((y, mo, d, h, mi, s)) + frac


def parse_log(lines):
    """Turn daemon log lines into events [(ts, kind, data)] and runs. Debug/trace lines are
    discarded unread; only whitelisted captures are kept."""
    events = []
    runs = []
    stats = collections.Counter()
    last_ts = None
    curs = {}  # stream index within the daemon session -> {start, wake, ended}
    nstreams = 0
    disc_since = None
    close_reason = None
    shutdown_seen = False
    for raw in lines:
        raw = raw.rstrip("\r\n")
        m = TS_LINE.match(raw)
        if not m:
            if PANIC.search(raw):
                events.append((last_ts or 0.0, "panic", {}))
            stats["lines_unparsed"] += 1
            continue
        ts = to_epoch(m)
        last_ts = ts
        level, msg = m.group(8), m.group(9)
        if level in ("DEBUG", "TRACE"):
            stats["debug_lines_discarded"] += 1
            continue
        if level == "SUPER":
            for kind, rx in SUPER_RULES:
                mm = rx.search(msg)
                if mm:
                    events.append((ts, kind, {"g": mm.groups()}))
                    break
            else:
                stats["lines_unparsed"] += 1
            continue
        stats["lines"] += 1
        rl = RUN_LINE.match(msg)
        if rl:
            extra = rl.group(7)
            wa = re.search(r"wake_after_stream_start=(\S+)", extra)
            er = re.search(r"error=(\S+)", extra)
            ra = re.search(r"reply_audio=(\d+)", extra)

            def num(s):
                return int(s) if s.isdigit() else None
            n = int(rl.group(1))
            c = curs.pop(n, {})
            nstreams = max(nstreams, n)
            run = {
                "n": n, "ts": ts, "kind": "wake" if rl.group(2) == "wake word" else "ptt",
                "stt_ms": num(rl.group(3)), "intent_ms": num(rl.group(4)), "tts_ms": num(rl.group(5)),
                "play_ms": num(rl.group(6)), "woke": bool(wa and wa.group(1).isdigit()),
                "error": (er.group(1) if er and CODE_OK.match(er.group(1)) else ("other" if er else None)),
                "reply_audio_ms": int(ra.group(1)) if ra else None,
                "start_ts": c.get("start", ts),
                "wake_ts": c.get("wake"),
                "disconnected": bool(disc_since is not None and disc_since >= c.get("start", ts) and not c.get("ended")),
                "shutdown": shutdown_seen and not c.get("ended"),
            }
            runs.append(run)
            continue
        for kind, rx in DAEMON_RULES:
            mm = rx.search(msg)
            if not mm:
                continue
            g = mm.groups()
            data = {}
            if kind == "start":
                data = {"version": g[0] if CODE_OK.match(g[0]) else "?"}
                curs, nstreams = {}, 0
                disc_since = None
                shutdown_seen = False
            elif kind == "stream_start":
                data = {"stage": "wake" if g[0].startswith("wake") else "stt"}
                nstreams += 1
                curs[nstreams] = {"start": ts}
            elif kind == "wake":
                curs.setdefault(nstreams, {})["wake"] = ts
            elif kind == "run_end":
                curs.setdefault(nstreams, {})["ended"] = True
            elif kind == "ha_close":
                w = g[0].lower()
                close_reason = ("requested" if "requested" in w else "reset" if "reset" in w else
                                "timeout" if "time" in w else "other")
            elif kind == "ha_disc":
                disc_since = ts
                data = {"reason": close_reason or "unknown"}
                close_reason = None
            elif kind == "ha_connect":
                data = {"ha_version": g[0] if CODE_OK.match(g[0]) else "?"}
            elif kind in ("stt_done", "intent_done", "playback_done", "rss", "mic_stall", "wd_rss", "run_failed"):
                data = {"n": int(g[0])}
            elif kind == "pipeline_error":
                data = {"code": g[0] if CODE_OK.match(g[0]) else "other"}
            elif kind == "mic_off":
                why = g[0].lower()
                data = {"why": why if re.match(r"^[a-z -]{1,24}$", why) else "other"}
            elif kind == "capture_stats":
                data = {"sent": int(g[0]), "dropped": int(g[1]), "xruns": int(g[2])}
            elif kind == "config":
                data = {"mode": g[0], "aec": g[1] or "?"}
            elif kind == "power":
                parts = [p.strip() for p in g[0].split(", ")]
                data = {"state": parts[0] if parts and re.match(r"^[A-Za-z0-9 _.-]{1,40}$", parts[0]) else "?"}
                for p in parts[1:]:
                    for key, name in (("processing ", "processing"), ("reason ", "reason"), ("onOff ", "onoff")):
                        if p.startswith(key):
                            v = p[len(key):]
                            data[name] = v if re.match(r"^[A-Za-z0-9 _.-]{1,40}$", v) else "?"
            elif kind == "shutdown":
                shutdown_seen = True
            events.append((ts, kind, data))
            break
        else:
            stats["lines_other"] += 1
    return {"events": events, "runs": runs, "stats": dict(stats), "first_ts": events[0][0] if events else None,
            "last_ts": last_ts}


def parse_autostart(lines):
    """Boot starts from autostart.log: [(ts, kind)] kind in start, refused, failed, update."""
    out = []
    for raw in lines:
        m = TS_LINE.match(raw.rstrip("\r\n"))
        if not m or m.group(8) != "autostart:":
            continue
        ts, msg = to_epoch(m), m.group(9)
        if msg.endswith("; starting"):
            out.append((ts, "start"))
        elif "not starting" in msg:
            out.append((ts, "refused"))
        elif msg.startswith("start.sh exited with status") and not msg.endswith(" 0"):
            out.append((ts, "failed"))
        elif msg.startswith("package update pending"):
            out.append((ts, "update"))
    return out


def sessions_from(events, data_end, running):
    """Daemon sessions [(start, end, partial, end_reason)] from start/stopped/supervisor events."""
    out = []
    cur = None
    last = None
    for ts, kind, _ in events:
        last = ts
        if kind == "start":
            if cur:
                out.append((cur[0], ts_prev, cur[1], "died"))
            cur = [ts, False]
        elif cur is None and kind not in ("sup_start", "sup_exit", "sup_restart", "sup_stopped", "sup_normal",
                                          "crashloop", "sup_refused", "panic"):
            cur = [ts, True]  # log starts mid-session
        if kind in ("stopped", "sup_exit", "sup_normal", "sup_stopped") and cur:
            out.append((cur[0], ts, cur[1], "stopped" if kind != "sup_exit" else "exit"))
            cur = None
        ts_prev = ts
    if cur:
        end = data_end if running and data_end else (last or cur[0])
        out.append((cur[0], max(end, cur[0]), cur[1], "running" if running else "ended"))
    return out


def attribute_starts(events, autostart):
    """Cause of each daemon start: boot, crash restart (exit status), manual/ctl, unknown."""
    boots = [t for t, k in autostart if k == "start"]
    res = []
    for i, (ts, kind, data) in enumerate(events):
        if kind != "start":
            continue
        cause, detail = "unknown", None
        j = i - 1
        while j >= 0 and events[j][1] not in ("sup_start", "sup_restart") and ts - events[j][0] < 10:
            j -= 1
        if j >= 0 and ts - events[j][0] < 10:
            if events[j][1] == "sup_restart":
                k = j
                while k >= 0 and events[k][1] != "sup_exit":
                    k -= 1
                st = int(events[k][2]["g"][0]) if k >= 0 else None
                cause, detail = "crash restart", st
            elif any(-5 <= ts - b <= 120 for b in boots):
                cause = "boot (autostart)"
            else:
                cause = "manual or ctl"
        res.append({"ts": ts, "cause": cause, "status": detail, "version": data.get("version")})
    return res


# ---------------------------------------------------------------- run classification

def classify(run):
    """One outcome label per run. 'idle' is a wake-stage cycle that ended without a wake."""
    err = run["error"]
    if run["kind"] == "wake" and not run["woke"] and not err:
        return "idle"
    if err:
        if "no-text" in err or "empty" in err:
            return "stt_empty"
        if "timeout" in err:
            return "timeout"
        if "intent" in err:
            return "intent_error"
        if "tts" in err:
            return "tts_error"
        if "stt" in err:
            return "stt_other_error"
        return "other_error"
    if run["disconnected"]:
        return "disconnect"
    if run["shutdown"]:
        return "shutdown"
    if run["stt_ms"] is None:
        return "stt_timeout_or_no_speech_end"
    if run["intent_ms"] is None:
        return "intent_incomplete"
    if run["tts_ms"] is None:
        return "completed_no_reply"
    if run["play_ms"] is None:
        return "playback_unfinished"
    return "completed"


GOOD = ("completed", "completed_no_reply")


def anchor(run):
    return run["wake_ts"] or run["start_ts"] if run["woke"] or run["kind"] == "ptt" else run["start_ts"]


def pctl(vals, p):
    if not vals:
        return None
    s = sorted(vals)
    k = (len(s) - 1) * p / 100.0
    lo = int(k)
    hi = min(lo + 1, len(s) - 1)
    return round(s[lo] + (s[hi] - s[lo]) * (k - lo))


def lat_stats(vals):
    return {"n": len(vals), "p50": pctl(vals, 50), "p90": pctl(vals, 90), "p95": pctl(vals, 95),
            "max": max(vals) if vals else None}


# ---------------------------------------------------------------- timelines

def clip(a, b, lo, hi):
    return max(0.0, min(b, hi) - max(a, lo))


def timeline(events, sessions, pick, initial=None):
    """Intervals [(t0, t1, label)] from events where pick(kind, data) returns a label (or None
    for no change); a session end closes the interval."""
    out = []
    for s_start, s_end, _, _ in sessions:
        label, t0 = initial, s_start
        for ts, kind, data in events:
            if ts < s_start or ts > s_end:
                continue
            new = pick(kind, data)
            if new is None or new == label:
                continue
            if label is not None:
                out.append((t0, ts, label))
            label, t0 = new, ts
        if label is not None:
            out.append((t0, s_end, label))
    return out


def label_at(intervals, ts):
    for a, b, lab in intervals:
        if a <= ts < b:
            return lab
    return None


def pick_power(kind, d):
    return d.get("state") if kind == "power" else None


def pick_ha(kind, d):
    return "up" if kind == "ha_connect" else "down" if kind == "ha_disc" else None


MIC_OFF_LABEL = {"muted": "muted", "kill switch": "kill switch", "firmware guard": "firmware guard",
                 "not paired": "not paired"}


def pick_mic(kind, d):
    if kind == "mic_open":
        return "open"
    if kind == "mic_off":
        return "off: " + MIC_OFF_LABEL.get(d.get("why"), d.get("why", "other"))
    if kind == "mic_busy":
        return "busy (another process)"
    if kind in ("mic_stall", "mic_error"):
        return "error"
    return None


def seconds_by_label(intervals, lo, hi):
    out = collections.defaultdict(float)
    for a, b, lab in intervals:
        c = clip(a, b, lo, hi)
        if c:
            out[lab] += c
    return {k: round(v) for k, v in sorted(out.items(), key=lambda kv: -kv[1])}


# ---------------------------------------------------------------- aggregation

def summarize_window(P, lo, hi, tz, live_now=None):
    ev = [e for e in P["events"] if lo <= e[0] < hi]
    cnt = collections.Counter(k for _, k, _ in ev)
    sess = P["sessions"]
    first = P["first_ts"] or hi
    end = P["data_end"]
    covered = clip(max(first, lo), end, lo, hi) if end else 0
    up = sum(clip(a, b, lo, hi) for a, b, _, _ in sess)
    starts = [s for s in P["starts"] if lo <= s["ts"] < hi]
    sup_exits = [e for e in ev if e[1] == "sup_exit"]
    exits = collections.Counter()
    for _, _, d in sup_exits:
        st = int(d["g"][0])
        exits[SUPERVISOR_STATUS.get(st, f"exit status {st}")] += 1
    rss = [d["n"] for _, k, d in ev if k == "rss"]
    disc = collections.Counter(d.get("reason", "unknown") for _, k, d in ev if k == "ha_disc")
    ha_secs = seconds_by_label(P["tl_ha"], lo, hi)
    power_secs = seconds_by_label(P["tl_power"], lo, hi)
    mic_secs = seconds_by_label(P["tl_mic"], lo, hi)
    out = {
        "window": {"from": iso(lo, tz), "to": iso(min(hi, end or hi), tz), "covered_s": round(covered)},
        "daemon": {
            "up_s": round(up), "up_pct": round(100 * up / covered, 1) if covered else None,
            "starts": len(starts), "starts_by_cause": dict(collections.Counter(
                s["cause"] + (f" (status {s['status']})" if s["status"] is not None else "") for s in starts)),
            "supervisor_exits": dict(exits), "supervisor_restarts": cnt["sup_restart"],
            "crash_loop_giveups": cnt["crashloop"],
            "watchdog": {"event_loop": cnt["wd_loop"], "capture": cnt["wd_capture"], "memory": cnt["wd_rss"],
                         "process_suspended": cnt["wd_suspended"]},
            "panics": cnt["panic"],
            "boot_starts": sum(1 for s in starts if s["cause"].startswith("boot")),
            "autostart_refusals_or_failures": sum(1 for t, k in P["autostart"] if lo <= t < hi and k in ("refused", "failed")),
            "rss_kib": {"n": len(rss), "min": min(rss), "max": max(rss), "last": rss[-1]} if rss else None,
            "resumes_from_suspend": cnt["resume"], "capture_stalls": cnt["mic_stall"],
            "capture_errors": cnt["mic_error"],
        },
        "ha": {"connects": cnt["ha_connect"], "disconnects": cnt["ha_disc"], "disconnect_reasons": dict(disc),
               "connected_s": ha_secs.get("up", 0), "disconnected_s": ha_secs.get("down", 0)},
        "mic": {"seconds_by_state": mic_secs, "mute_by_ha": cnt["mute_on"], "unmute_refused": cnt["unmute_refused"],
                "kill_switch_on": cnt["kill_on"], "busy_events": cnt["mic_busy"]},
        "power": {"seconds_by_state": power_secs,
                  "transitions": sum(1 for a, b in zip(P["tl_power"], P["tl_power"][1:])
                                     if lo <= b[0] < hi and a[2] != b[2] and a[1] == b[0]),
                  "standby_entries": sum(1 for a, b in zip(P["tl_power"], P["tl_power"][1:])
                                         if lo <= b[0] < hi and "Standby" in b[2] and "Standby" not in a[2]),
                  "remote_power_off_requests": cnt["rpo_request"]},
    }
    out["pipeline"] = summarize_runs(P, lo, hi, tz, cnt)
    return out


def summarize_runs(P, lo, hi, tz, cnt):
    runs = [r for r in P["runs"] if lo <= anchor(r) < hi]
    outcomes = collections.Counter(classify(r) for r in runs)
    attempts = [r for r in runs if classify(r) != "idle"]
    wake_runs = [r for r in runs if r["kind"] == "wake" and r["woke"]]
    codes = collections.Counter(r["error"] for r in runs if r["error"])
    by_state = collections.defaultdict(collections.Counter)
    hours = [collections.Counter() for _ in range(24)]
    for r in attempts:
        t = anchor(r)
        o = classify(r)
        st = label_at(P["tl_power"], t) or "unknown"
        by_state[st]["attempts"] += 1
        by_state[st]["ok" if o in GOOD else "failed"] += 1
        h = dt.datetime.fromtimestamp(t, tz).hour
        hours[h]["attempts"] += 1
        hours[h]["ok" if o in GOOD else "failed"] += 1
    for ts, k, _ in P["events"]:
        if k == "wake" and lo <= ts < hi:
            hours[dt.datetime.fromtimestamp(ts, tz).hour]["wakes"] += 1
    retries = 0
    ordered = sorted(attempts, key=anchor)
    for a, b in zip(ordered, ordered[1:]):
        if classify(a) not in GOOD and b["woke"] and (b["wake_ts"] or b["start_ts"]) - a["ts"] <= RETRY_S:
            retries += 1
    wake_lat = [r for r in wake_runs]
    return {
        "wakes_detected": cnt["wake"],
        "attempts": len(attempts), "push_to_talk_runs": sum(1 for r in runs if r["kind"] == "ptt"),
        "wake_stage_cycles_without_wake": outcomes.get("idle", 0),
        "outcomes": {k: v for k, v in sorted(outcomes.items(), key=lambda kv: -kv[1]) if k != "idle"},
        "ok_pct": round(100 * sum(1 for r in attempts if classify(r) in GOOD) / len(attempts), 1) if attempts else None,
        "error_codes": dict(codes),
        "failed_then_retried_within_30s": retries,
        "latency_ms_from_wake": {
            "stt_end": lat_stats([r["stt_ms"] for r in wake_lat if r["stt_ms"] is not None]),
            "intent_end": lat_stats([r["intent_ms"] for r in wake_lat if r["intent_ms"] is not None]),
            "tts_start": lat_stats([r["tts_ms"] for r in wake_lat if r["tts_ms"] is not None]),
            "playback_end": lat_stats([r["play_ms"] for r in wake_lat if r["play_ms"] is not None]),
        },
        "by_power_state": {k: dict(v) for k, v in sorted(by_state.items())},
        "by_hour": [{"hour": h, **dict(hours[h])} for h in range(24) if hours[h]],
    }


def iso(ts, tz):
    d = dt.datetime.fromtimestamp(ts, tz)
    return (d if tz else d.astimezone()).strftime("%Y-%m-%d %H:%M:%S %Z")


def local_days(lo, hi, tz):
    d = dt.datetime.fromtimestamp(lo, tz).replace(hour=0, minute=0, second=0, microsecond=0)
    while d.timestamp() < hi:
        nxt = (d + dt.timedelta(days=1, hours=3)).replace(hour=0, minute=0, second=0, microsecond=0)
        yield d.strftime("%Y-%m-%d"), d.timestamp(), nxt.timestamp()
        d = nxt


def build_model(log_lines, autostart_lines, live, tz, lo, hi, now):
    P = parse_log(log_lines)
    P["autostart"] = parse_autostart(autostart_lines)
    last = P["last_ts"]
    P["data_end"] = now if live.get("running") and last else last
    P["sessions"] = sessions_from(P["events"], P["data_end"], bool(live.get("running")))
    P["starts"] = attribute_starts(P["events"], P["autostart"])
    P["tl_power"] = timeline(P["events"], P["sessions"], pick_power)
    P["tl_ha"] = timeline(P["events"], P["sessions"], pick_ha, "down")
    P["tl_mic"] = timeline(P["events"], P["sessions"], pick_mic)
    cfg = [d for _, k, d in P["events"] if k == "config"]
    P["config"] = cfg[-1] if cfg else None
    P["aec_state"] = ("on" if any(k == "aec_on" for _, k, _ in P["events"]) else
                      "running without a reference" if any(k == "aec_without" for _, k, _ in P["events"]) else
                      "off" if P["config"] and P["config"]["aec"] == "off" else "not reported")
    return P


# ---------------------------------------------------------------- TV (ssh) and archive

REMOTE = """
D=%(dir)s
echo '@@now'; date -u +%%s
echo '@@uptime'; cat /proc/uptime 2>/dev/null
echo '@@clk'; getconf CLK_TCK 2>/dev/null || echo 100
echo '@@ctl'; sh %(ctl)s status 2>&1
echo '@@markers'
for f in "$D/disable" "$D/crashloop" /tmp/webos-satellite.disable; do [ -e "$f" ] && echo "$(basename "$f")"; done
echo '@@proc'
pid=$(cat "$D/run/webos-satellite.pid" 2>/dev/null)
if [ -n "$pid" ] && [ -r "/proc/$pid/stat" ]; then echo "pid $pid"; cat "/proc/$pid/stat"; grep -E '^(VmRSS|Threads):' "/proc/$pid/status"; fi
sp=$(cat "$D/run/supervise.pid" 2>/dev/null)
if [ -n "$sp" ] && tr '\0' ' ' <"/proc/$sp/cmdline" 2>/dev/null | grep -q supervise.sh; then echo "supervisor $sp"; fi
echo '@@log1'; cat "$D/log/webos-satellite.log.1" 2>/dev/null
echo '@@log'; cat "$D/log/webos-satellite.log" 2>/dev/null
echo '@@autostart'; cat "$D/log/autostart.log" 2>/dev/null
echo '@@end'
"""


def fetch_tv(host, user, tv_dir, ctl, ssh_opts, timeout):
    for v in (tv_dir, ctl):
        if not re.match(r"^[A-Za-z0-9_./-]+$", v):
            raise SystemExit("unsupported characters in --tv-dir/--ctl")
    cmd = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"] + ssh_opts + [f"{user}@{host}", "sh -s"]
    try:
        r = subprocess.run(cmd, input=REMOTE % {"dir": tv_dir, "ctl": ctl}, capture_output=True, text=True,
                           timeout=timeout, errors="replace")
    except (subprocess.TimeoutExpired, FileNotFoundError) as e:
        raise SystemExit(f"ssh failed: {type(e).__name__}")
    if r.returncode != 0 or "@@end" not in r.stdout:
        first = next((ln for ln in r.stderr.splitlines() if ln and not ln.startswith("**")), "")
        raise SystemExit(f"ssh to the TV failed (status {r.returncode}): {first[:120]}")
    return r.stdout


def split_bundle(text):
    sec, cur = {}, None
    for ln in text.splitlines():
        if ln.startswith("@@") and ln[2:] in ("now", "uptime", "clk", "ctl", "markers", "proc", "log1", "log", "autostart", "end"):
            cur = ln[2:]
            sec[cur] = []
        elif cur:
            sec[cur].append(ln)
    return sec


def firmware_status(ctl):
    """Firmware status from the `ctl.sh status` text (the soft policy, ADR 0006).

    Returns "verified", "series verified", "not verified", "changed" or "unknown", or None when
    the status has no firmware line. The verdict is the text that `--check-firmware` prints after
    the firmware info ("firmware <version> (...): <verdict>"). The older wording ("validated", "NOT in
    safety.validated_firmware") is still read: "validated" counts as verified, the strict gate
    text as not verified. "changed" comes from the compatibility line (`compat_status`).
    """
    m = re.search(r"^(?:webos-satellite: )?firmware: (.*)$", ctl, re.M)
    if not m:
        return None
    line = m.group(1).strip()
    # "firmware <version> (webOS ..., build ...): <verdict>", or the unreadable text on its own.
    rest = line.split("): ", 1)[1].strip() if line.startswith("firmware ") and "): " in line else line
    c = re.search(r"^(?:webos-satellite: )?compatibility: ([^;\n]*)", ctl, re.M)
    if c and c.group(1).strip() == "firmware changed":
        return "changed"
    if rest.startswith("verified") or rest.startswith("validated"):
        return "verified"
    if "is a verified series" in rest:
        return "series verified"
    if rest.startswith("not verified") or rest.startswith("not validated") or "NOT in safety.validated_firmware" in rest:
        return "not verified"
    return "unknown"


def live_from(sec, now_local, offset=True):
    """Live state of the daemon from the status/proc sections."""
    live = {"running": False}
    try:
        tv_now = float(sec["now"][0])
        if offset:
            live["clock_offset_s"] = round(tv_now - now_local, 1)
        live["now"] = tv_now
    except (KeyError, IndexError, ValueError):
        live["now"] = now_local
    try:
        up = float(sec["uptime"][0].split()[0])
        live["tv_uptime_s"] = round(up)
    except (KeyError, IndexError, ValueError):
        up = None
    ctl = "\n".join(sec.get("ctl", []))
    m = re.search(r"running: pid (\d+)", ctl)
    live["running"] = bool(m)
    live["pid"] = int(m.group(1)) if m else None
    m = re.search(r"installed: true (\S+) in \S+ \(webos-satellite (\S+)", ctl)
    live["version"] = m.group(1) if m else None
    m = re.search(r"start at boot: (on|off)", ctl)
    live["autostart"] = m.group(1) if m else None
    live["firmware_status"] = firmware_status(ctl)
    live["markers"] = [x for x in sec.get("markers", []) if x in ("disable", "crashloop", "webos-satellite.disable")]
    proc = sec.get("proc", [])
    # The supervisor: its pid and a supervise.sh cmdline in the /proc probe (the same test as
    # supervisor_alive() in packaging/webos/lib.sh). ctl.sh status text does not name it, so the
    # text match ("supervisor running", as status.sh prints it) is only a fallback.
    # The remote script prints `supervisor PID` only after its cmdline check passed.
    sup_pids = [ln.split()[1] for ln in proc if re.match(r"^supervisor \d+$", ln)]
    live["supervisor_running"] = bool(sup_pids) or bool(re.search(r"supervisor\W{1,3}running", ctl))
    live["supervisor_pid"] = int(sup_pids[0]) if sup_pids else None
    clk = 100
    try:
        clk = int(sec["clk"][0])
    except (KeyError, IndexError, ValueError):
        pass
    for ln in proc:
        if ln.startswith("VmRSS:"):
            live["rss_kib"] = int(ln.split()[1])
        elif ln.startswith("Threads:"):
            live["threads"] = int(ln.split()[1])
        elif re.match(r"^\d+ \(", ln) and up is not None:
            tail = ln.rsplit(")", 1)[1].split()
            try:
                live["daemon_uptime_s"] = round(up - int(tail[19]) / clk)
            except (IndexError, ValueError):
                pass
    return live


def sanitize_for_archive(lines):
    out = []
    for ln in lines:
        m = TS_LINE.match(ln)
        if not m or m.group(8) in ("DEBUG", "TRACE"):
            continue
        out.append(IPV4.sub("x.x.x.x", ln.rstrip("\r\n")))
    return out


def merge_archive(old, new):
    """Append the lines of `new` that are newer than the last line of `old` (equal-second
    lines already present are not repeated)."""
    def key(ln):
        m = TS_LINE.match(ln)
        return to_epoch(m) if m else None
    keys = [key(ln) for ln in old]
    last = max((k for k in keys if k is not None), default=None)
    have = {ln for ln, k in zip(old, keys) if k == last}
    merged = list(old)
    for ln in new:
        k = key(ln)
        if k is None:
            continue
        if last is None or k > last or (k == last and ln not in have):
            merged.append(ln)
    return merged


def under_ignored_path(path):
    """True when `path` is outside the repo or under a gitignored place."""
    ap = os.path.abspath(path)
    if not ap.startswith(REPO + os.sep):
        return True
    r = subprocess.run(["git", "-C", REPO, "check-ignore", "-q", ap + os.sep + "x"], capture_output=True)
    return r.returncode == 0


# ---------------------------------------------------------------- Home Assistant

class WsError(Exception):
    pass


class MiniWs:
    """Just enough of RFC 6455 for HA's websocket API (text frames, no extensions)."""

    def __init__(self, url, timeout=20, cafile=None):
        u = urllib.parse.urlparse(url)
        secure = u.scheme in ("https", "wss")
        host, port = u.hostname, u.port or (443 if secure else 80)
        sock = socket.create_connection((host, port), timeout=timeout)
        if secure:
            sock = ssl.create_default_context(cafile=cafile).wrap_socket(sock, server_hostname=host)
        self.sock = sock
        key = base64.b64encode(os.urandom(16)).decode()
        path = (u.path.rstrip("/") or "") + "/api/websocket"
        req = (f"GET {path} HTTP/1.1\r\nHost: {u.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
        sock.sendall(req.encode())
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = sock.recv(4096)
            if not chunk:
                raise WsError("connection closed during the handshake")
            head += chunk
            if len(head) > 65536:
                raise WsError("handshake too long")
        head, self.buf = head.split(b"\r\n\r\n", 1)
        if b" 101 " not in head.split(b"\r\n", 1)[0] + b" ":
            raise WsError("websocket upgrade refused")
        acc = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
        if acc not in head:
            raise WsError("bad Sec-WebSocket-Accept")

    def _read(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise WsError("connection closed")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def send_json(self, obj, opcode=1):
        data = json.dumps(obj).encode() if opcode == 1 else obj
        hdr = bytes([0x80 | opcode])
        n = len(data)
        hdr += bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n) if n < 65536 \
            else bytes([0x80 | 127]) + struct.pack(">Q", n)
        mask = os.urandom(4)
        self.sock.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def recv_json(self):
        msg = b""
        while True:
            b0, b1 = self._read(2)
            op, n = b0 & 0x0F, b1 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._read(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._read(8))[0]
            payload = self._read(n)
            if op == 8:
                raise WsError("closed by the server")
            if op == 9:
                self.send_json(payload, opcode=10)
                continue
            if op == 10:
                continue
            msg += payload
            if b0 & 0x80:
                return json.loads(msg)

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


class HaClient:
    def __init__(self, url, token, cafile=None):
        self.ws = MiniWs(url, cafile=cafile)
        self.n = 0
        try:
            first = self.ws.recv_json()
            if first.get("type") != "auth_required":
                raise WsError("unexpected first message")
            self.ws.send_json({"type": "auth", "access_token": token})
            if self.ws.recv_json().get("type") != "auth_ok":
                raise WsError("Home Assistant rejected the token")
        except BaseException:
            self.ws.close()
            raise

    def call(self, typ, **kw):
        self.n += 1
        self.ws.send_json({"id": self.n, "type": typ, **kw})
        while True:
            m = self.ws.recv_json()
            if m.get("id") == self.n and m.get("type") == "result":
                if not m.get("success"):
                    code = (m.get("error") or {}).get("code", "error")
                    raise WsError(f"{typ} failed: {code if CODE_OK.match(str(code)) else 'error'}")
                return m.get("result")

    def close(self):
        self.ws.close()


def read_token(path):
    st = os.stat(path)
    warn = bool(st.st_mode & 0o077)
    with open(path) as f:
        raw = f.read().strip()
    if raw.startswith("{"):
        raw = json.loads(raw)["access_token"]
    return raw, warn


def parse_iso(s):
    try:
        d = dt.datetime.fromisoformat(s.replace("Z", "+00:00"))
        if d.tzinfo is None:
            d = d.replace(tzinfo=dt.timezone.utc)
        return d.timestamp()
    except (ValueError, AttributeError):
        return None


def summarize_ha_run(run_id, run_ts, events):
    """Whitelist reduction of one pipeline debug run: booleans, lengths, fixed codes, timings.
    Never copies stt text, intent input, reply speech or error messages."""
    o = {"id": run_id if CODE_OK.match(str(run_id)) else hashlib.sha1(str(run_id).encode()).hexdigest()[:16],
         "ts": parse_iso(run_ts), "wake_detected": False, "stt_text": False, "stt_chars": None,
         "stt_empty": False, "vad_start": False, "intent": False, "intent_type": None, "intent_code": None,
         "processed_locally": None, "reply_chars": None, "tts": False, "error": None, "ended": False}
    t0 = tw = None
    marks = {}
    for e in events or []:
        typ, d, ts = e.get("type"), e.get("data") or {}, parse_iso(e.get("timestamp"))
        if t0 is None and ts is not None:
            t0 = ts
        if typ == "wake_word-end":
            if d.get("wake_word_output"):
                o["wake_detected"] = True
                tw = ts
        elif typ == "stt-vad-start":
            o["vad_start"] = True
        elif typ == "stt-end":
            txt = (d.get("stt_output") or {}).get("text")
            o["stt_text"] = bool(txt)
            o["stt_chars"] = len(txt) if isinstance(txt, str) else None
            marks["stt_end"] = ts
        elif typ == "intent-end":
            o["intent"] = True
            o["processed_locally"] = bool(d.get("processed_locally"))
            resp = ((d.get("intent_output") or {}).get("response") or {})
            rt = resp.get("response_type")
            o["intent_type"] = rt if isinstance(rt, str) and CODE_OK.match(rt) else None
            code = (resp.get("data") or {}).get("code")
            o["intent_code"] = code if isinstance(code, str) and CODE_OK.match(code) else None
            sp = ((resp.get("speech") or {}).get("plain") or {}).get("speech")
            o["reply_chars"] = len(sp) if isinstance(sp, str) else None
            marks["intent_end"] = ts
        elif typ == "tts-end":
            o["tts"] = True
            marks["tts_end"] = ts
        elif typ == "error":
            code = d.get("code")
            o["error"] = code if isinstance(code, str) and CODE_OK.match(code) else "other"
            o["stt_empty"] = o["stt_empty"] or code == "stt-no-text-recognized"
        elif typ == "run-end":
            o["ended"] = True
    base = tw or t0
    o["ms_from_wake_or_start"] = {k: round((v - base) * 1000) for k, v in marks.items() if v and base}
    return o


def collect_ha(url, token, pipeline, cafile):
    cli = HaClient(url, token, cafile)
    try:
        pl = cli.call("assist_pipeline/pipeline/list")
        pipes = pl.get("pipelines", [])
        want = (pipeline or "preferred").lower()
        sel = None
        for p in pipes:
            if want == "preferred" and p.get("id") == pl.get("preferred_pipeline"):
                sel = p
            elif want in (str(p.get("id", "")).lower(), str(p.get("name", "")).lower()):
                sel = p
        if not sel:
            raise WsError("pipeline not found (use --ha-pipeline NAME)")
        runs = cli.call("assist_pipeline/pipeline_debug/list", pipeline_id=sel["id"]).get("pipeline_runs", [])
        out = []
        for r in runs:
            res = cli.call("assist_pipeline/pipeline_debug/get", pipeline_id=sel["id"], pipeline_run_id=r["pipeline_run_id"])
            out.append(summarize_ha_run(r["pipeline_run_id"], r.get("timestamp"), (res or {}).get("events")))
        name = sel.get("name") if CODE_OK.match(str(sel.get("name", "")).replace(" ", "_")) else "pipeline"
        return {"pipeline": name, "runs": out}
    finally:
        cli.close()


def ha_section(ha, tv_runs, lo, hi):
    runs = [r for r in ha["runs"] if r["ts"] and lo <= r["ts"] < hi]
    c = collections.Counter()
    for r in runs:
        c["runs"] += 1
        c["wake_detected"] += r["wake_detected"]
        c["stt_text_present"] += r["stt_text"]
        c["stt_empty"] += r["stt_empty"]
        c["intent_done"] += r["intent"]
        c["intent_handled"] += r["intent_type"] in ("action_done", "query_answer")
        c["intent_error_type"] += r["intent_type"] == "error"
        c["tts_done"] += r["tts"]
        c["pipeline_error"] += bool(r["error"])
    errs = collections.Counter(r["error"] for r in runs if r["error"])
    icodes = collections.Counter(r["intent_code"] for r in runs if r["intent_code"])
    chars = sorted(r["stt_chars"] for r in runs if r["stt_chars"] is not None)
    # Join with TV runs by start time (nearest within HA_MATCH_S, each used once).
    used, pairs = set(), collections.Counter()
    for tr in sorted(tv_runs, key=lambda r: r["start_ts"]):
        if classify(tr) == "idle":
            continue
        best = min(((abs(h["ts"] - tr["start_ts"]), i) for i, h in enumerate(runs) if i not in used and h["ts"]),
                   default=None)
        if best and best[0] <= HA_MATCH_S:
            used.add(best[1])
            h = runs[best[1]]
            hv = ("stt_empty" if h["stt_empty"] else "stt_text" if h["stt_text"] else "no_stt")
            iv = h["intent_type"] or ("intent_error" if h["error"] and "intent" in h["error"] else "no_intent")
            pairs[(classify(tr), hv, iv)] += 1
    return {
        "pipeline": ha["pipeline"], "runs_in_window": len(runs), "runs_available": len(ha["runs"]),
        "oldest": ha.get("oldest"), "newest": ha.get("newest"),
        "counts": dict(c), "error_codes": dict(errs), "intent_error_codes": dict(icodes),
        "stt_text_chars_p50": chars[len(chars) // 2] if chars else None,
        "matched_with_tv_attempts": [{"tv_outcome": a, "ha_stt": b, "ha_intent": i, "n": n}
                                     for (a, b, i), n in sorted(pairs.items())],
    }


def merge_ha_cache(path, fresh):
    """Merge sanitized HA run summaries into a JSON cache (one entry per run id)."""
    old = {}
    if path and os.path.exists(path):
        try:
            with open(path) as f:
                old = {r["id"]: r for r in json.load(f).get("runs", [])}
        except (ValueError, KeyError, OSError):
            old = {}
    for r in fresh:
        old[r["id"]] = r
    runs = sorted(old.values(), key=lambda r: r["ts"] or 0)
    if path:
        os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump({"note": "sanitized Assist debug run summaries (no text)", "runs": runs}, f)
    return runs


# ---------------------------------------------------------------- rendering

NOT_RECORDED = [
    "A wake word that HA never detected leaves no trace in the daemon log or in HA's debug runs: misses are "
    "invisible. Only failed or repeated attempts are visible (\"failed_then_retried_within_30s\").",
    "Whether the TV (or the PC) was playing sound is not logged. The power state tells screen/standby only; "
    "\"misses with sound\" cannot be separated from \"misses in quiet\" from these logs.",
    "The microphone level, echo-reference level and AEC metrics are logged at debug only, every 60 s, "
    "and never per run.",
    "The TV volume/mute and the foreground app are not logged.",
    "The log is size-rotated (256 KiB, then one .1 copy; about three days at light use), so a long window needs "
    "--archive-dir with a regular run. Proposed additions: docs/development/dogfood-report.md.",
]


def fmt_s(s):
    if s is None:
        return "-"
    s = int(s)
    d, s = divmod(s, 86400)
    h, s = divmod(s, 3600)
    m, s = divmod(s, 60)
    return (f"{d}d " if d else "") + (f"{h}h " if h or d else "") + f"{m}m"


def table(head, rows):
    if not rows:
        return "(none)\n"
    out = ["| " + " | ".join(head) + " |", "|" + "|".join("---" for _ in head) + "|"]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(out) + "\n"


def kv(d):
    return ", ".join(f"{k} {v}" for k, v in d.items()) if d else "none"


def render_window(w, title):
    d, h, mic, pw, p = w["daemon"], w["ha"], w["mic"], w["power"], w["pipeline"]
    L = [f"### {title}", ""]
    L.append(f"- Coverage: {w['window']['from']} to {w['window']['to']} ({fmt_s(w['window']['covered_s'])} of log)")
    L.append(f"- Daemon: up {fmt_s(d['up_s'])} ({d['up_pct']}% of covered time), {d['starts']} starts ({kv(d['starts_by_cause'])}), "
             f"supervisor restarts {d['supervisor_restarts']}, exits: {kv(d['supervisor_exits'])}, crash-loop give-ups {d['crash_loop_giveups']}")
    wd = d["watchdog"]
    L.append(f"- Watchdog trips: loop {wd['event_loop']}, capture {wd['capture']}, memory {wd['memory']}; panics {d['panics']}; "
             f"capture stalls {d['capture_stalls']}, capture errors {d['capture_errors']}; suspend resumes {d['resumes_from_suspend']}; "
             f"boot starts {d['boot_starts']}, refusals/failures {d['autostart_refusals_or_failures']}")
    if d["rss_kib"]:
        r = d["rss_kib"]
        L.append(f"- RSS (logged on change): min {r['min']} KiB, max {r['max']} KiB, last {r['last']} KiB ({r['n']} samples)")
    L.append(f"- HA connection: {h['connects']} connects, {h['disconnects']} disconnects ({kv(h['disconnect_reasons'])}); "
             f"connected {fmt_s(h['connected_s'])}, down {fmt_s(h['disconnected_s'])}")
    L.append(f"- Microphone time: " + ", ".join(f"{k} {fmt_s(v)}" for k, v in mic["seconds_by_state"].items()) +
             f"; HA mutes {mic['mute_by_ha']}, kill switch {mic['kill_switch_on']}, busy events {mic['busy_events']}")
    L.append(f"- TV power state time: " + ", ".join(f"{k} {fmt_s(v)}" for k, v in pw["seconds_by_state"].items()) +
             f"; transitions {pw['transitions']}, standby entries {pw['standby_entries']}, remote power-off requests {pw['remote_power_off_requests']}")
    L += ["", f"Pipeline: {p['wakes_detected']} wakes, {p['attempts']} attempts ({p['ok_pct']}% ok), "
          f"{p['push_to_talk_runs']} push-to-talk, {p['wake_stage_cycles_without_wake']} wake-stage cycles without a wake, "
          f"{p['failed_then_retried_within_30s']} failures followed by a retry within 30 s.", ""]
    L.append(table(["outcome", "runs"], list(p["outcomes"].items())))
    if p["error_codes"]:
        L.append("HA error codes: " + kv(p["error_codes"]) + "\n")
    lat = p["latency_ms_from_wake"]
    L.append(table(["latency ms from wake", "n", "p50", "p90", "p95", "max"],
                   [[k, v["n"], v["p50"], v["p90"], v["p95"], v["max"]] for k, v in lat.items()]))
    if p["by_power_state"]:
        L.append(table(["TV power state at wake", "attempts", "ok", "failed"],
                       [[k, v.get("attempts", 0), v.get("ok", 0), v.get("failed", 0)] for k, v in p["by_power_state"].items()]))
    if p["by_hour"]:
        L.append(table(["hour (local)", "wakes", "attempts", "ok", "failed"],
                       [[f"{x['hour']:02d}", x.get("wakes", 0), x.get("attempts", 0), x.get("ok", 0), x.get("failed", 0)]
                        for x in p["by_hour"]]))
    return "\n".join(L)


def render(rep):
    L = [f"# Satellite dogfood report", "",
         f"Generated {rep['generated']}; window {rep['window']['from']} to {rep['window']['to']}; times in {rep['window']['tz']}.", ""]
    live = rep["live"]
    if live:
        L += ["## Now (read from the TV)", ""]
        L.append(f"- Daemon: {'running' if live.get('running') else 'NOT running'}"
                 + (f", pid {live['pid']}" if live.get("pid") else "")
                 + (f", version {live['version']}" if live.get("version") else "")
                 + (f", up {fmt_s(live.get('daemon_uptime_s'))}" if live.get("daemon_uptime_s") is not None else "")
                 + (f"; TV up {fmt_s(live.get('tv_uptime_s'))}" if live.get("tv_uptime_s") is not None else ""))
        L.append(f"- RSS {live.get('rss_kib', '-')} KiB, threads {live.get('threads', '-')}; supervisor "
                 f"{'running' if live.get('supervisor_running') else 'not running'}; start at boot: {live.get('autostart')}; "
                 f"markers: {', '.join(live.get('markers') or []) or 'none'}; firmware status: {live.get('firmware_status') or 'no data'}")
        if live.get("clock_offset_s") is not None:
            L.append(f"- TV clock minus PC clock: {live['clock_offset_s']} s")
        L.append("")
    cov = rep["coverage"]
    L += ["## Log coverage", "",
          f"- Logs hold {cov['first']} to {cov['last']} ({cov['lines']} info+ lines, {cov['debug_lines_discarded']} debug lines discarded unread). "
          f"Config: mode {cov['mode']}, AEC {cov['aec_config']} (log says: {cov['aec_state']}).",
          ] + ([f"- WARNING: the window starts before the log does; earlier days are not in the TV's logs."] if cov["window_before_log"] else []) + [""]
    if rep["days"]:
        L += ["## Per day", ""]
        L.append(table(["day", "covered", "daemon up", "starts", "crash restarts", "HA disc.", "wakes", "attempts", "ok %",
                        "stt_end p50", "playback_end p50"],
                       [[k, fmt_s(v["window"]["covered_s"]), (f"{v['daemon']['up_pct']}%" if v["daemon"]["up_pct"] is not None else "-"), v["daemon"]["starts"],
                         sum(1 for c in v["daemon"]["starts_by_cause"] if c.startswith("crash")),
                         v["ha"]["disconnects"], v["pipeline"]["wakes_detected"], v["pipeline"]["attempts"],
                         "-" if v["pipeline"]["ok_pct"] is None else v["pipeline"]["ok_pct"], v["pipeline"]["latency_ms_from_wake"]["stt_end"]["p50"],
                         v["pipeline"]["latency_ms_from_wake"]["playback_end"]["p50"]] for k, v in rep["days"].items()]))
    L += ["## Window", "", render_window(rep["total"], "Whole window"), ""]
    ha = rep.get("ha")
    L += ["## Home Assistant Assist debug runs", ""]
    if ha is None:
        L.append(f"Skipped: {rep.get('ha_note', 'not requested')}.")
    else:
        L.append(f"Pipeline \"{ha['pipeline']}\": {ha['runs_available']} debug runs available, {ha['runs_in_window']} in the window"
                 + (f" ({ha['oldest']} to {ha['newest']})" if ha.get("oldest") else "") + ".")
        L.append(f"HA keeps at most {HA_STORED_RUNS} runs per pipeline, in memory (lost when HA restarts), shared by every device on "
                 f"the pipeline, and each wake-stage cycle counts as a run. So HA covers only the recent past (about an hour when idle "
                 f"cycles last several minutes); use --archive-dir with a regular run (for example hourly) to accumulate.")
        L.append("")
        L.append("Counts: " + kv(ha["counts"]))
        if ha["error_codes"]:
            L.append("Error codes: " + kv(ha["error_codes"]))
        if ha["intent_error_codes"]:
            L.append("Intent error codes: " + kv(ha["intent_error_codes"]))
        L.append("")
        if ha["matched_with_tv_attempts"]:
            L.append(table(["TV outcome", "HA STT", "HA intent", "runs"],
                           [[x["tv_outcome"], x["ha_stt"], x["ha_intent"], x["n"]] for x in ha["matched_with_tv_attempts"]]))
    L += ["", "## Not recorded (needed to separate misses with sound from misses in quiet)", ""]
    L += [f"- {x}" for x in NOT_RECORDED]
    return "\n".join(L) + "\n"


# ---------------------------------------------------------------- main

def load_config(path):
    if path and os.path.exists(path):
        try:
            with open(path) as f:
                return json.load(f) if os.path.getsize(path) else {}
        except (ValueError, OSError):
            raise SystemExit("cannot read the config file")
    return {}


def parse_when(s, tz):
    s = s.strip()
    try:
        d = dt.datetime.strptime(s, "%Y-%m-%d") if re.match(r"^\d{4}-\d\d-\d\d$", s) else dt.datetime.fromisoformat(s)
        return (d if d.tzinfo or tz is None else d.replace(tzinfo=tz)).timestamp()
    except ValueError:
        raise SystemExit(f"cannot parse the time: {s}")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                 epilog="Config file (JSON, gitignored; flags and env win): keys host, user, ha_url, "
                                        "ha_token_file, ha_pipeline. Env: DOGFOOD_TV_HOST, DOGFOOD_TV_USER, HA_URL, "
                                        "HA_TOKEN_FILE, DOGFOOD_HA_PIPELINE.")
    ap.add_argument("--host", help="TV hostname or address (env DOGFOOD_TV_HOST; no default)")
    ap.add_argument("--user", help="ssh user (default root)")
    ap.add_argument("--ssh-opt", action="append", default=[], help="extra ssh option, e.g. -o and value as one arg: -oStrictHostKeyChecking=yes")
    ap.add_argument("--tv-dir", default=DEFAULT_TV_DIR, help="install dir on the TV")
    ap.add_argument("--ctl", default=DEFAULT_CTL, help="ctl.sh on the TV")
    ap.add_argument("--days", type=int, help="last N local days including today (default 7)")
    ap.add_argument("--since", help="window start: YYYY-MM-DD (local midnight) or ISO time")
    ap.add_argument("--until", help="window end (default now)")
    ap.add_argument("--utc", action="store_true", help="days and hours in UTC instead of the PC's timezone")
    ap.add_argument("--json", action="store_true", help="print JSON instead of markdown")
    ap.add_argument("--no-ha", action="store_true", help="skip the Home Assistant part")
    ap.add_argument("--ha-url", help="HA base URL (env HA_URL)")
    ap.add_argument("--ha-token-file", help="file with a long-lived or short-lived access token (env HA_TOKEN_FILE); "
                                          "never minted or stored by this tool")
    ap.add_argument("--ha-pipeline", help="pipeline name or id (default: HA's preferred pipeline)")
    ap.add_argument("--ha-cafile", help="CA bundle for HA's TLS (default: system store; certificates are always verified)")
    ap.add_argument("--archive-dir", help="accumulate sanitized log lines and HA run summaries here across runs "
                                           "(must be gitignored, e.g. devices/private/dogfood)")
    ap.add_argument("--bundle", help="read a saved TV bundle instead of ssh (offline analysis)")
    ap.add_argument("--save-bundle", help="save the raw TV bundle (gitignored path only; holds addresses)")
    ap.add_argument("--config", default=DEFAULT_CONFIG, help="gitignored JSON config")
    a = ap.parse_args(argv)

    cfg = load_config(a.config)
    host = a.host or os.environ.get("DOGFOOD_TV_HOST") or cfg.get("host")
    user = a.user or os.environ.get("DOGFOOD_TV_USER") or cfg.get("user") or "root"
    tz = dt.timezone.utc if a.utc else None  # None: the PC's local zone, DST-aware
    now = dt.datetime.now(dt.timezone.utc).timestamp()
    for p in (a.archive_dir, a.save_bundle):
        if p and not under_ignored_path(p):
            raise SystemExit(f"refusing {p}: inside the repository but not gitignored (use devices/private/...)")

    if a.bundle:
        with open(a.bundle) as f:
            sec = split_bundle(f.read())
    elif host:
        text = fetch_tv(host, user, a.tv_dir, a.ctl, a.ssh_opt, 90)
        if a.save_bundle:
            fd = os.open(a.save_bundle, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(text)
        sec = split_bundle(text)
    else:
        raise SystemExit("no TV: give --host (or DOGFOOD_TV_HOST, or the config file) or --bundle")

    live = live_from(sec, now, offset=not a.bundle)
    now_eff = live.get("now", now) if a.bundle else now
    log_lines = sec.get("log1", []) + sec.get("log", [])
    as_lines = sec.get("autostart", [])
    if a.archive_dir:
        os.makedirs(a.archive_dir, exist_ok=True)
        for name, new in (("webos-satellite.log", log_lines), ("autostart.log", as_lines)):
            p = os.path.join(a.archive_dir, name)
            old = []
            if os.path.exists(p):
                with open(p) as f:
                    old = f.read().splitlines()
            merged = merge_archive(old, sanitize_for_archive(new))
            fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write("\n".join(merged) + "\n")
            if name == "webos-satellite.log":
                log_lines = merged
            else:
                as_lines = merged

    hi = parse_when(a.until, tz) if a.until else now_eff
    if a.since:
        lo = parse_when(a.since, tz)
    else:
        days = a.days or 7
        d0 = dt.datetime.fromtimestamp(hi, tz).replace(hour=0, minute=0, second=0, microsecond=0) - dt.timedelta(days=days - 1)
        lo = d0.timestamp()
    P = build_model(log_lines, as_lines, live, tz, lo, hi, now_eff)
    if not P["events"]:
        raise SystemExit("no parsable log lines")
    lo_eff = max(lo, P["first_ts"])
    total = summarize_window(P, lo_eff, hi, tz)
    days = {k: summarize_window(P, max(s, lo_eff), min(e, hi), tz) for k, s, e in local_days(lo_eff, hi, tz)}
    stats = P["stats"]
    rep = {
        "generated": iso(now, tz), "window": {"from": iso(lo_eff, tz), "to": iso(min(hi, P["data_end"] or hi), tz), "tz": "UTC" if a.utc else dt.datetime.now().astimezone().tzname()},
        "live": live if (host or a.bundle) else None,
        "coverage": {"first": iso(P["first_ts"], tz), "last": iso(P["last_ts"], tz), "lines": stats.get("lines", 0),
                     "debug_lines_discarded": stats.get("debug_lines_discarded", 0),
                     "mode": (P["config"] or {}).get("mode", "?"), "aec_config": (P["config"] or {}).get("aec", "?"),
                     "aec_state": P["aec_state"], "window_before_log": lo < P["first_ts"] - 3600},
        "total": total, "days": days,
    }
    if a.no_ha:
        rep["ha"], rep["ha_note"] = None, "--no-ha"
    else:
        url = a.ha_url or os.environ.get("HA_URL") or cfg.get("ha_url")
        tokf = a.ha_token_file or os.environ.get("HA_TOKEN_FILE") or cfg.get("ha_token_file")
        pipe = a.ha_pipeline or os.environ.get("DOGFOOD_HA_PIPELINE") or cfg.get("ha_pipeline")
        if not url or not tokf:
            rep["ha"], rep["ha_note"] = None, "no HA URL or token file given (--ha-url, --ha-token-file)"
        else:
            try:
                token, warn = read_token(tokf)
                if warn:
                    print("warning: the token file is readable by other users", file=sys.stderr)
                ha = collect_ha(url, token, pipe, a.ha_cafile)
                cache = os.path.join(a.archive_dir, "ha_runs.json") if a.archive_dir else None
                runs = merge_ha_cache(cache, ha["runs"])
                ts = [r["ts"] for r in runs if r["ts"]]
                ha["runs"] = runs
                ha["oldest"], ha["newest"] = (iso(min(ts), tz), iso(max(ts), tz)) if ts else (None, None)
                rep["ha"] = ha_section(ha, [r for r in P["runs"] if lo_eff <= anchor(r) < hi], lo_eff, hi)
            except (WsError, OSError, ValueError, KeyError) as e:
                rep["ha"], rep["ha_note"] = None, "failed (%s)" % (str(e) if isinstance(e, WsError) else type(e).__name__)
    if a.json:
        json.dump(rep, sys.stdout, indent=2, sort_keys=False)
        print()
    else:
        sys.stdout.write(render(rep))
    return 0


if __name__ == "__main__":
    sys.exit(main())
