"""Pure logic of the live AEC A/B tools (no network, no audio, standard library only).

Shared by `aec_live_ab.py` (the runner), `aec_live_convert.py` (EXP-018 converter) and
`aec_live_report.py` (analysis). Nothing here touches a device.

Contents: plan files and the counterbalanced schedule, the config-section editor, the safety
rails, the satellite's `aec:` debug-line parser, the per-trial CSV schema and McNemar.
"""
import csv
import datetime
import hashlib
import math
import re
import tomllib
import unicodedata
from pathlib import Path

import p2_score

# ---- safety constants ------------------------------------------------------------------------

MAX_TV_VOLUME = 20          # the owner's rule for every AEC session
DAY_START_H, DAY_END_H = 8, 21   # sound only from 08:00 up to (not including) 21:00 local time
MAX_CLOCK_SKEW_S = 120      # PC against TV clock, for the log alignment
HDMI_RE = re.compile(r"hdmi|displayport|\.dp[-_.]|display[-_]port", re.I)

CSV_COLUMNS = ["run_id", "plan", "arm", "condition", "tv_volume", "talker_level", "item_id", "voice",
               "phrase_id", "detected", "wake_latency_ms", "stt_match", "stt_wer", "delay_ms", "erle_db",
               "underruns", "daemon_cpu_pct", "notes"]


class PlanError(ValueError):
    pass


class SafetyError(RuntimeError):
    pass


# ---- deterministic ordering ------------------------------------------------------------------

def seeded_order(seq, seed, salt=""):
    """A permutation of `seq` fixed by (seed, salt): sorted by a hash, so it does not depend on
    the Python version's random module."""
    def key(x):
        return hashlib.sha256(f"{seed}:{salt}:{x}".encode()).hexdigest()
    return sorted(seq, key=key)


def counterbalanced_orders(arms):
    """Arm orders for successive item sets: every rotation of `arms`, then every rotation reversed.
    Two arms give A B / B A / B A / A B (ABBA); three give six orders covering every position."""
    n = len(arms)
    rot = [list(arms[i:] + arms[:i]) for i in range(n)]
    return rot + [list(reversed(r)) for r in rot]


# ---- plan ------------------------------------------------------------------------------------

def _need(d, key, where):
    if key not in d:
        raise PlanError(f"{where}: missing `{key}`")
    return d[key]


def load_plan(path):
    """Parse and validate a plan file. Returns a dict with the validated sections."""
    with open(path, "rb") as f:
        raw = tomllib.load(f)
    return parse_plan(raw, source=str(path))


def parse_plan(raw, source="plan"):
    p = _need(raw, "plan", source)
    plan = {"id": _need(p, "id", "[plan]"), "title": p.get("title", ""), "seed": int(_need(p, "seed", "[plan]")),
            "log_level": p.get("log_level", "debug"), "gain_db": float(p.get("gain_db", 24.0)),
            "trial_seconds": float(p.get("trial_seconds", 13.0)),
            "restart_seconds": float(p.get("restart_seconds", 60.0)),
            "content_lead_s": float(p.get("content_lead_s", 15.0)),
            "corpus_note": p.get("corpus_note", ""), "source": source}
    t = raw.get("timing", {})
    plan["timing"] = {"sink_latency_s": float(t.get("sink_latency_s", 0.2)), "settle_s": float(t.get("settle_s", 1.5)),
                      "post_s": float(t.get("post_s", 3.0)), "max_run_s": float(t.get("max_run_s", 40.0)),
                      "lead_s": float(t.get("lead_s", 1.0))}
    if not re.fullmatch(r"[A-Za-z0-9._-]+", plan["id"]):
        raise PlanError("[plan] id: letters, digits, . _ - only")
    c = raw.get("content", {})
    plan["content"] = {"category": c.get("category", "sampler"), "pc_sink_pct": int(c.get("pc_sink_pct", 100)),
                       "repeat": int(c.get("repeat", 10))}
    plan["talkers"] = {}
    for name, t in raw.get("talkers", {}).items():
        plan["talkers"][name] = {"sink_pct": int(_need(t, "sink_pct", f"[talkers.{name}]")),
                                 "label": t.get("label", name)}
    plan["arms"] = {}
    for name, a in raw.get("arms", {}).items():
        plan["arms"][name] = {"aec": validate_arm_snippet(a.get("aec", ""), name), "note": a.get("note", "")}
    plan["conditions"] = {}
    for name, c_ in raw.get("conditions", {}).items():
        talker = _need(c_, "talker", f"[conditions.{name}]")
        plan["conditions"][name] = {"tv_volume": int(_need(c_, "tv_volume", f"[conditions.{name}]")),
                                    "content": bool(c_.get("content", False)), "talker": talker}
    plan["sets"] = {n: list(v) for n, v in raw.get("sets", {}).items()}
    plan["schedule"] = raw.get("schedule", {})
    plan["dualcap"] = raw.get("dualcap", {"enabled": False})
    plan["sweep"] = raw.get("sweep", {"enabled": False})
    validate_plan(plan)
    plan["blocks"] = build_schedule(plan)
    return plan


def validate_arm_snippet(text, name="arm"):
    """An arm is only `[aec]` and `[aec.*]` tables (empty = no [aec] section = the satellite's built-in
    default: B+NS on since 0.4.3, off before; write `enabled = false` for an OFF arm)."""
    if not text.strip():
        return ""
    try:
        d = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        raise PlanError(f"arm {name}: not valid TOML: {e}")
    extra = set(d) - {"aec"}
    if extra or "aec" not in d:
        raise PlanError(f"arm {name}: only [aec] / [aec.aec3] tables are allowed (found {sorted(set(d) - {'aec'}) or 'no [aec]'})")
    return text if text.endswith("\n") else text + "\n"


def arm_aec_enabled(text):
    """Whether an arm runs the canceller: its `[aec] enabled`, which defaults to true since 0.4.3
    (B+NS), so an empty arm (no [aec] section) is on too."""
    if not text.strip():
        return True
    return bool(tomllib.loads(text).get("aec", {}).get("enabled", True))


def validate_plan(plan):
    if not plan["arms"]:
        raise PlanError("no [arms.*]")
    if not plan["conditions"]:
        raise PlanError("no [conditions.*]")
    if not plan["sets"]:
        raise PlanError("no [sets]")
    for n, c in plan["conditions"].items():
        check_tv_volume(c["tv_volume"], f"condition {n}")
        if c["talker"] not in plan["talkers"]:
            raise PlanError(f"condition {n}: unknown talker {c['talker']!r}")
    for n, t in plan["talkers"].items():
        if not 0 < t["sink_pct"] <= 100:
            raise PlanError(f"talker {n}: sink_pct must be 1 to 100")
    if not 0 < plan["content"]["pc_sink_pct"] <= 100:
        raise PlanError("content.pc_sink_pct must be 1 to 100")
    if not -60 <= plan["gain_db"] <= 24:
        raise PlanError("gain_db must be within [-60, +24]")
    for k in ("dualcap", "sweep"):
        sec = plan[k]
        if sec.get("enabled") and "tv_volume" in sec:
            check_tv_volume(sec["tv_volume"], k)
    if plan["sweep"].get("enabled"):
        check_tv_volume(_need(plan["sweep"], "tv_volume", "[sweep]"), "sweep")
    for s, ids in plan["sets"].items():
        if len(ids) != len(set(ids)):
            raise PlanError(f"set {s}: duplicate item ids")


def check_tv_volume(v, where="tv volume"):
    if not isinstance(v, int) or v < 0 or v > MAX_TV_VOLUME:
        raise SafetyError(f"{where}: TV volume {v!r} refused (allowed 0 to {MAX_TV_VOLUME})")


def build_schedule(plan):
    """Blocks of trials: dicts with index, arm, condition, set, items (item ids in play order)."""
    sch = plan["schedule"]
    mode = sch.get("mode", "generated")
    seed = plan["seed"]
    blocks = []
    if mode == "explicit":
        for b in sch.get("block", []):
            for key, table in (("arm", plan["arms"]), ("condition", plan["conditions"]), ("set", plan["sets"])):
                if b.get(key) not in table:
                    raise PlanError(f"explicit block {b}: unknown {key}")
            blocks.append({"arm": b["arm"], "condition": b["condition"], "set": b["set"],
                           "items": list(plan["sets"][b["set"]])})
    elif mode == "generated":
        layout = sch.get("layout", "cond-set-arm")
        if layout != "cond-set-arm":
            raise PlanError(f"unknown schedule layout {layout!r}")
        arms = list(sch.get("arm_order", list(plan["arms"])))
        conds = list(sch.get("conditions", list(plan["conditions"])))
        sets = list(sch.get("sets", list(plan["sets"])))
        for name, table in (("arm", plan["arms"]),):
            for x in arms:
                if x not in table:
                    raise PlanError(f"schedule: unknown arm {x}")
        for x in conds:
            if x not in plan["conditions"]:
                raise PlanError(f"schedule: unknown condition {x}")
        for x in sets:
            if x not in plan["sets"]:
                raise PlanError(f"schedule: unknown set {x}")
        if sch.get("condition_order", "listed") == "seeded":
            conds = seeded_order(conds, seed, "conditions")
        for cond in conds:
            base = seeded_order(arms, seed, f"arms:{cond}") if sch.get("arm_order_seeded", True) else arms
            orders = counterbalanced_orders(base)
            for i, s in enumerate(sets):
                for arm in orders[i % len(orders)]:
                    items = plan["sets"][s]
                    if sch.get("shuffle_items", True):
                        items = seeded_order(items, seed, f"items:{cond}:{s}:{arm}")
                    blocks.append({"arm": arm, "condition": cond, "set": s, "items": list(items)})
    else:
        raise PlanError(f"unknown schedule mode {mode!r}")
    for i, b in enumerate(blocks, 1):
        b["index"] = i
    return blocks


def schedule_summary(plan, item_seconds=None):
    """Numbers for `--dry-run`: trials, arm switches (restarts) and the estimated sound minutes.
    Sound = the trials, the restarts that happen while content plays, and the content lead-in each time
    the condition (or, with `content_each_block`, each block) starts content."""
    blocks = plan["blocks"]
    each = bool(plan["schedule"].get("content_each_block", False))
    trials = sum(len(b["items"]) for b in blocks)
    restarts, cur, cur_cond = 0, None, None
    ab = 0.0
    for b in blocks:
        cond = plan["conditions"][b["condition"]]
        if (b["condition"] != cur_cond or each) and cond["content"]:
            ab += plan["content_lead_s"]
        cur_cond = b["condition"]
        if b["arm"] != cur:
            restarts += 1
            cur = b["arm"]
            if cond["content"]:          # a restart is silent in a quiet condition
                ab += plan["restart_seconds"]
        ab += len(b["items"]) * plan["trial_seconds"]
    extra = {}
    dc = plan["dualcap"]
    if dc.get("enabled"):
        ids = [i for s in dc.get("sets", list(plan["sets"])) for i in plan["sets"][s]]
        avg = (sum(item_seconds.get(i, 3.0) for i in ids) / len(ids)) if (ids and item_seconds) else 3.0
        n_cond = len(dc.get("conditions", list(plan["conditions"])))
        per = plan["content_lead_s"] + 1.0 + len(ids) * (avg + float(dc.get("gap_s", 2.5))) + 5.0
        extra["dualcap"] = per * n_cond / 60
    sw = plan["sweep"]
    if sw.get("enabled"):
        extra["sweep"] = (float(sw.get("lead_s", 3.0)) + int(sw.get("repeats", 3)) *
                          (float(sw.get("sweep_s", 10.0)) + float(sw.get("gap_s", 2.0))) + 3.0) / 60
    total = ab / 60 + sum(extra.values())
    return {"trials": trials, "blocks": len(blocks), "restarts": restarts, "ab_minutes": ab / 60,
            "extra_minutes": extra, "total_minutes": total}


def format_schedule(plan, item_seconds=None):
    """Human-readable schedule for `--dry-run`."""
    lines = [f"plan {plan['id']}: {plan['title']}", f"seed {plan['seed']}, log level {plan['log_level'] or '(unchanged)'}", ""]
    lines.append("arms:")
    for n, a in plan["arms"].items():
        body = " ".join(l.strip() for l in a["aec"].splitlines() if l.strip()) or "(no [aec] section: the built-in default, B+NS since 0.4.3)"
        lines.append(f"  {n}: {body}")
    lines.append("conditions:")
    for n, c in plan["conditions"].items():
        t = plan["talkers"][c["talker"]]
        cont = f"on (PC HDMI sink {plan['content']['pc_sink_pct']} %)" if c["content"] else "off"
        lines.append(f"  {n}: TV volume {c['tv_volume']}, content {cont}, talker {c['talker']} "
                     f"(talker sink {t['sink_pct']} %, gain {plan['gain_db']:+.0f} dB)")
    lines.append("")
    lines.append("schedule (a restart is needed whenever the arm changes):")
    cur = None
    for b in plan["blocks"]:
        sw = "restart" if b["arm"] != cur else "same arm"
        cur = b["arm"]
        lines.append(f"  block {b['index']:2d}  {b['condition']:4s} {b['arm']:8s} set {b['set']:3s} "
                     f"{len(b['items'])} trials  ({sw})")
    s = schedule_summary(plan, item_seconds)
    lines.append("")
    lines.append(f"{s['trials']} trials in {s['blocks']} blocks, {s['restarts']} daemon restarts")
    for k, v in s["extra_minutes"].items():
        lines.append(f"  {k}: about {v:.1f} min of sound")
    lines.append(f"estimated sound: about {s['ab_minutes']:.1f} min for the A/B "
                 f"({plan['trial_seconds']:.0f} s per trial, {plan['restart_seconds']:.0f} s per restart, "
                 f"{plan['content_lead_s']:.0f} s content lead-in), about {s['total_minutes']:.0f} min in all")
    return "\n".join(lines)


# ---- config section editor -------------------------------------------------------------------

BEGIN_MARK = "# >>> aec_live_ab managed [aec] section (begin)"
END_MARK = "# <<< aec_live_ab managed [aec] section (end)"
_HEADER = re.compile(r"^\s*\[\[?\s*([^\[\]]+?)\s*\]\]?\s*(?:#.*)?$")


def _header_name(line):
    m = _HEADER.match(line.rstrip("\r\n"))
    return m.group(1) if m else None


def _is_aec_header(name):
    return name is not None and name.split(".")[0].strip().strip('"') == "aec"


def _lines(text):
    # split on "\n" only (str.splitlines also splits on form feeds and other separators)
    return re.findall(r"[^\n]*\n|[^\n]+", text)


def _drop_marked(lines):
    out, skip = [], False
    for l in lines:
        s = l.rstrip("\r\n")
        if s == BEGIN_MARK:
            skip = True
            continue
        if skip:
            if s == END_MARK:
                skip = False
            continue
        out.append(l)
    return out


def _split_blocks(lines):
    """[(header_name or None, [lines])]: the lines before the first header form block None.
    Trailing blank/comment lines of a block go to the next block (they describe it); the block's
    name stays that of its header."""
    heads = [i for i, l in enumerate(lines) if _header_name(l) is not None]
    starts = ([0] if (not heads or heads[0] != 0) else []) + heads
    names = ([None] if (not heads or heads[0] != 0) else []) + [_header_name(lines[i]) for i in heads]
    has_header = [n is not None for n in names]
    ends = starts[1:] + [len(lines)]
    spans = [[s, e] for s, e in zip(starts, ends)]
    for bi in range(len(spans) - 1):
        s, e = spans[bi]
        floor = s + 1 if has_header[bi] else s
        while e > floor and (lines[e - 1].strip() == "" or lines[e - 1].lstrip().startswith("#")):
            e -= 1
        spans[bi][1] = e
        spans[bi + 1][0] = e
    return [(n, lines[s:e]) for n, (s, e) in zip(names, spans)]


def _leading(blk):
    """(lines before the block's header, the rest): the blank/comment run that _split_blocks attached."""
    i = next(k for k, l in enumerate(blk) if _header_name(l) is not None)
    return blk[:i], blk[i:]


def replace_aec_section(text, snippet):
    """`text` with every [aec] / [aec.*] table replaced by `snippet` (empty: none), wrapped in
    marker comments. Everything else is byte-for-byte the same. The snippet goes where the old
    [aec] tables were, or at the end of the file."""
    lines = _drop_marked(_lines(text))
    out, pos = [], None
    for name, blk in _split_blocks(lines):
        if _is_aec_header(name):
            lead, _ = _leading(blk)      # comments above the header are not part of the table
            out.extend(lead)
            if pos is None:
                pos = len(out)
            continue
        out.extend(blk)
    ins = []
    if snippet.strip():
        sn = snippet if snippet.endswith("\n") else snippet + "\n"
        ins = [BEGIN_MARK + "\n"] + _lines(sn) + [END_MARK + "\n"]
    if pos is None:
        pos = len(out)
        if ins and out and not out[-1].endswith("\n"):
            out[-1] += "\n"
    out[pos:pos] = ins
    return "".join(out)


def set_log_level(text, level):
    """Set `level` in [log] (add the key or the table if missing). Empty `level` = unchanged."""
    if not level:
        return text
    lines = _lines(text)
    blocks = _split_blocks(lines)
    new = []
    done = False
    for name, blk in blocks:
        if name == "log" and not done:
            done = True
            hit = False
            for i, l in enumerate(blk):
                if re.match(r"^\s*level\s*=", l):
                    eol = l[len(l.rstrip("\r\n")):]
                    blk = blk[:i] + [f'level = "{level}"{eol or chr(10)}'] + blk[i + 1:]
                    hit = True
                    break
            if not hit:
                if not blk[-1].endswith("\n"):
                    blk[-1] += "\n"
                blk = blk[:1] + [f'level = "{level}"\n'] + blk[1:]
        new.extend(blk)
    out = "".join(new)
    if not done:
        if out and not out.endswith("\n"):
            out += "\n"
        out += f'\n[log]\nlevel = "{level}"\n'
    return out


def render_config(base_text, snippet, log_level=""):
    return set_log_level(replace_aec_section(base_text, snippet), log_level)


def outside_managed(text):
    """The text with the managed parts removed ([aec] tables, our markers, the [log] level line),
    for the check that a rendering changed nothing else."""
    lines = _drop_marked(_lines(text))
    out = []
    for name, blk in _split_blocks(lines):
        if _is_aec_header(name):
            out.extend(_leading(blk)[0])
            continue
        if name == "log":
            blk = [l for l in blk if not re.match(r"^\s*level\s*=", l)]
        out.extend(blk)
    return "".join(out)


def md5(data):
    return hashlib.md5(data if isinstance(data, bytes) else data.encode()).hexdigest()


# ---- safety rails ----------------------------------------------------------------------------

def check_clock(now, allow_any_time=False):
    """`now` is a timezone-aware local datetime. Sound only 08:00 to 21:00 local time."""
    if allow_any_time:
        return
    if not (DAY_START_H <= now.hour < DAY_END_H):
        raise SafetyError(f"local time is {now:%H:%M}: no sound outside {DAY_START_H:02d}:00 to {DAY_END_H:02d}:00 "
                          f"(use --allow-any-time only with the owner's agreement)")


def is_hdmi_sink(name):
    return bool(HDMI_RE.search(name or ""))


def check_sinks(content_sink, talker_sink, content_needed=True, talker_needed=True, known=None):
    """The HDMI (TV) sink must be named explicitly and is never the talker sink."""
    if content_needed and not content_sink:
        raise SafetyError("a content cell needs the HDMI sink named explicitly (--content-sink NAME); no default is used")
    if talker_needed and not talker_sink:
        raise SafetyError("the talker sink must be named explicitly (--talker-sink NAME)")
    if talker_sink and is_hdmi_sink(talker_sink):
        raise SafetyError(f"talker sink {talker_sink!r} looks like an HDMI/DisplayPort sink (the TV): refused")
    if content_sink and content_sink == talker_sink:
        raise SafetyError("talker sink and content sink are the same")
    if known is not None:
        for n in (content_sink, talker_sink):
            if n and n not in known:
                raise SafetyError(f"sink {n!r} does not exist (pactl list short sinks)")


def check_token_file(path):
    import os
    import stat
    st = os.stat(path)
    if stat.S_IMODE(st.st_mode) & 0o077:
        raise SafetyError(f"token file {path} must be mode 0600 (it is {oct(stat.S_IMODE(st.st_mode))})")


# ---- the satellite's log lines ---------------------------------------------------------------

_TS = r"(?P<ts>\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z)"
_NUM = r"(?:-?[0-9.]+|n/a)"
AEC_WINDOW = re.compile(
    _TS + r"\s+DEBUG\s+aec: last 60 s: delay (?P<delay>" + _NUM + r") ms \(confidence (?P<conf>" + _NUM + r")\), "
    r"ERLE (?P<erle>" + _NUM + r") dB \(sonora (?P<serle>" + _NUM + r") dB\) over (?P<active>\d+) active-reference frames, "
    r"sonora (?P<us>\d+) us/frame mean (?P<usmax>\d+) max \((?P<core>[0-9.]+) % of a core\), "
    r"bypassed (?P<byp>\d+)/(?P<frames>\d+) frames, reference underruns (?P<under>\d+)")
AEC_LOCK = re.compile(_TS + r"\s+INFO\s+aec: echo delay (?P<delay>[0-9.]+) ms \(mic behind reference, confidence (?P<conf>[0-9.]+)\)")
AEC_CHANGE = re.compile(_TS + r"\s+INFO\s+aec: echo delay changed (?P<old>[0-9.]+) -> (?P<new>[0-9.]+) ms \(confidence (?P<conf>[0-9.]+)\)")
AEC_ON = re.compile(_TS + r"\s+INFO\s+aec: echo cancellation on: .*?render (?P<render>\d+) ch.*?ns=(?P<ns>true|false)")
CAPTURE_WINDOW = re.compile(
    _TS + r"\s+DEBUG\s+capture: last 60 s: in rms (?P<in_rms>-?[0-9.]+) dBFS peak (?P<in_peak>-?[0-9.]+) dBFS, "
    r"out rms (?P<out_rms>-?[0-9.]+) dBFS peak (?P<out_peak>-?[0-9.]+) dBFS, clipped (?P<clipped>\d+), "
    r"xruns (?P<xruns>\d+), chunks sent (?P<sent>\d+) dropped (?P<dropped>\d+)")
STREAMING = re.compile(_TS + r"\s+INFO\s+streaming mic audio to HA \(wake word stage\)")


def parse_ts(s):
    """Epoch seconds of a log timestamp like 2026-11-05T11:41:47.262Z."""
    s = s.rstrip("Z")
    fmt = "%Y-%m-%dT%H:%M:%S.%f" if "." in s else "%Y-%m-%dT%H:%M:%S"
    return datetime.datetime.strptime(s, fmt).replace(tzinfo=datetime.timezone.utc).timestamp()


def _f(x):
    return None if x in (None, "n/a") else float(x)


def parse_aec_lines(text):
    """Structured `aec:` / `capture:` / streaming lines of a satellite log, in file order.
    Each is a dict with kind in {window, lock, change, on, capture, streaming} and `t` (epoch)."""
    out = []
    for line in text.splitlines():
        if "aec:" not in line and "capture:" not in line and "streaming mic" not in line:
            continue
        m = AEC_WINDOW.search(line)
        if m:
            g = m.groupdict()
            out.append({"kind": "window", "t": parse_ts(g["ts"]), "delay_ms": _f(g["delay"]), "confidence": _f(g["conf"]),
                        "erle_db": _f(g["erle"]), "sonora_erle_db": _f(g["serle"]), "active_frames": int(g["active"]),
                        "sonora_us_mean": int(g["us"]), "sonora_us_max": int(g["usmax"]), "sonora_core_pct": float(g["core"]),
                        "bypassed": int(g["byp"]), "frames": int(g["frames"]), "underruns": int(g["under"])})
            continue
        m = AEC_LOCK.search(line)
        if m:
            g = m.groupdict()
            out.append({"kind": "lock", "t": parse_ts(g["ts"]), "delay_ms": float(g["delay"]), "confidence": float(g["conf"])})
            continue
        m = AEC_CHANGE.search(line)
        if m:
            g = m.groupdict()
            out.append({"kind": "change", "t": parse_ts(g["ts"]), "old_ms": float(g["old"]), "delay_ms": float(g["new"]),
                        "confidence": float(g["conf"])})
            continue
        m = AEC_ON.search(line)
        if m:
            g = m.groupdict()
            out.append({"kind": "on", "t": parse_ts(g["ts"]), "render": int(g["render"]), "ns": g["ns"] == "true"})
            continue
        m = CAPTURE_WINDOW.search(line)
        if m:
            g = m.groupdict()
            out.append({"kind": "capture", "t": parse_ts(g["ts"]), "in_rms_dbfs": float(g["in_rms"]),
                        "out_rms_dbfs": float(g["out_rms"]), "clipped": int(g["clipped"]), "xruns": int(g["xruns"]),
                        "dropped": int(g["dropped"])})
            continue
        m = STREAMING.search(line)
        if m:
            out.append({"kind": "streaming", "t": parse_ts(m.group("ts"))})
    return out


def window_for(t, entries, kind="window", span=60.0):
    """The first `kind` entry whose span of `span` seconds (ending at its timestamp) contains t."""
    best = None
    for e in entries:
        if e["kind"] == kind and e["t"] - span < t <= e["t"]:
            if best is None or e["t"] < best["t"]:
                best = e
    return best


# ---- trials and rows -------------------------------------------------------------------------

def phrase_id(item):
    """A stable id for the spoken phrase of a synthetic corpus item (never a transcript of the room)."""
    if item.get("subkind") == "alone" or not item.get("expected_command_text"):
        return "alone"
    t = unicodedata.normalize("NFD", item["expected_command_text"])
    t = "".join(c for c in t if not unicodedata.combining(c)).lower()
    return "cmd_" + re.sub(r"[^a-z0-9]+", "_", t).strip("_")


def voice_label(item):
    v = item.get("voice", "")
    sp = item.get("speaker")
    return f"{v}:{sp}" if sp in ("M", "F") else v


def is_detected(detections_s, duration_ms, pre=0.2, post=1.5):
    """Detection inside [-pre, item duration + post] of the item's own timeline (the EXP-018 window).
    Returns (detected, first latency in ms or None)."""
    inside = sorted(d for d in detections_s if -pre <= d <= duration_ms / 1000 + post)
    return (1, round(inside[0] * 1000)) if inside else (0, None)


def stt_scores(item, stt_text):
    """(stt_match 0/1 or None, wer fraction or None). Only for command items with a transcript;
    the transcript itself is never kept."""
    if item.get("subkind") != "cmd" or stt_text is None:
        return None, None
    e, n, _ = p2_score.wer_counts(item.get("expected_command_text", ""), stt_text)
    return (1 if e == 0 else 0), (round(e / n, 3) if n else None)


_BAD_FIELD = re.compile(r"(\b\d{1,3}(?:\.\d{1,3}){3}\b)|([0-9A-Fa-f]{2}(?:[:_-][0-9A-Fa-f]{2}){5})|(https?://)|(eyJ[A-Za-z0-9_-]{10,})")


def check_row_privacy(row):
    """Refuse rows that carry an IP address, a MAC address, a URL or a token-like string."""
    for k, v in row.items():
        s = "" if v is None else str(v)
        if _BAD_FIELD.search(s) or len(s) > 120:
            raise ValueError(f"row field {k} looks sensitive or free text: {s[:30]!r}")


def _fmt(v):
    if v is None:
        return ""
    if isinstance(v, float):
        return f"{v:.3f}".rstrip("0").rstrip(".") if v != int(v) else str(int(v))
    return str(v)


def write_rows(path, rows, append=False):
    path = Path(path)
    new = not path.exists() or not append
    with open(path, "a" if append else "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        if new:
            w.writerow(CSV_COLUMNS)
        for r in rows:
            check_row_privacy(r)
            w.writerow([_fmt(r.get(c)) for c in CSV_COLUMNS])


def read_rows(paths):
    rows = []
    for p in paths:
        with open(p, newline="") as f:
            rd = csv.DictReader(f)
            missing = set(CSV_COLUMNS) - set(rd.fieldnames or [])
            if missing:
                raise ValueError(f"{p}: missing columns {sorted(missing)}")
            for r in rd:
                rows.append(r)
    return rows


# ---- statistics ------------------------------------------------------------------------------

def mcnemar_exact(only_a, only_b):
    """Two-sided exact McNemar p (binomial, p = 0.5) from the discordant counts."""
    n = only_a + only_b
    if n == 0:
        return 1.0
    k = min(only_a, only_b)
    return min(1.0, 2 * sum(math.comb(n, i) for i in range(k + 1)) / 2 ** n)


# ---- raw trial records to CSV rows -----------------------------------------------------------

def trial_row(plan, run_id, rec, log_entries, block, skew_s=0.0):
    """One CSV row from a raw trial record (`aec_live_ab.py` writes them to trials.jsonl).

    rec: arm, condition, block, item (corpus fields), detections_s, stt_text, t_play, lead_s,
    sink_latency_s, valid (bool), notes (list). block: the block record (daemon_cpu_pct, notes).
    skew_s: TV clock minus PC clock, to place the PC trial time in the TV log's timeline."""
    cond = plan["conditions"][rec["condition"]]
    it = rec["item"]
    notes = list(rec.get("notes", [])) + list((block or {}).get("notes", []))
    notes.insert(0, f"block={rec['block']}")
    row = {"run_id": run_id, "plan": plan["id"], "arm": rec["arm"], "condition": rec["condition"],
           "tv_volume": cond["tv_volume"], "talker_level": cond["talker"], "item_id": it["id"],
           "voice": voice_label(it), "phrase_id": phrase_id(it), "detected": None, "wake_latency_ms": None,
           "stt_match": None, "stt_wer": None, "delay_ms": None, "erle_db": None, "underruns": None,
           "daemon_cpu_pct": (block or {}).get("daemon_cpu_pct")}
    if rec.get("valid", True):
        det, lat = is_detected(rec["detections_s"], it["duration_ms"])
        row["detected"], row["wake_latency_ms"] = det, lat
        if det:
            row["stt_match"], row["stt_wer"] = stt_scores(it, rec.get("stt_text"))
    else:
        notes.append("invalid")
    if arm_aec_enabled(plan["arms"][rec["arm"]]["aec"]):
        t_mid = rec["t_play"] + rec["lead_s"] + rec["sink_latency_s"] + it["duration_ms"] / 2000 + skew_s
        w = window_for(t_mid, log_entries)
        if w:
            row["delay_ms"], row["erle_db"], row["underruns"] = w["delay_ms"], w["erle_db"], w["underruns"]
    row["notes"] = ";".join(notes)
    return row
