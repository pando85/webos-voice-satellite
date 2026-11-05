#!/usr/bin/env python3
"""Live echo-cancellation A/B on a TV, driven by a plan file (`aec_plans/*.toml`).

For each block of the plan: switch the satellite's `[aec]` arm on the TV (only the `[aec]` /
`[aec.*]` tables and the `[log] level` key of its config change; the original bytes are backed up
and restored on exit, Ctrl-C or error), restart it with the supported `ctl.sh restart`, wait for
HA to take the stream and for the delay lock, play TV content from the PC (HDMI sink) and the
synthetic talker (loudspeaker sink) one item at a time, read the wake detections and STT from Home
Assistant, and write one CSV row per trial (schema in README.md, `aec_live_lib.CSV_COLUMNS`).

    aec_live_ab.py PLAN --dry-run                       # print the schedule and the sound minutes, touch nothing
    aec_live_ab.py PLAN --tv-ssh root@TV --ha-url URL --token-file F --pipeline NAME --entity ID \\
        --corpus devices/private/corpus/p2-v1 --content-manifest devices/private/content/p2-v1/manifest.csv \\
        --talker-sink SINK --content-sink HDMI_SINK [--stages ab,dualcap,sweep]

Safety rails (all enforced before and during the run): TV volume above 20 is refused; sound only
08:00 to 21:00 local time unless --allow-any-time (checked again before every block); the HDMI
sink must be named with --content-sink (there is no default) and an HDMI-looking name is refused
as the talker sink; the token file must be mode 0600 and is never printed. Sink volumes and the TV
volume are restored at the end (PC sinks first, then the TV). Nothing here is run on a TV without
the owner's authorization for the session (AGENTS.md).

Private outputs (never committed) go to --out-dir (default devices/private/<device>/<run-id>/):
config backup, trials.jsonl, blocks.jsonl, tvlog-aec.txt, trials.csv, blocks.csv, the optional
mic + reference captures (`dualcap-*`, `echo-path/`). Copy `trials.csv` to docs/experiments/data/
once it has been checked.

`--rebuild OUT_DIR` rebuilds trials.csv from trials.jsonl and tvlog-aec.txt (after a crash).
"""
import argparse
import asyncio
import datetime
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import aec_live_lib as L

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SSH_OPTS = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "LogLevel=ERROR"]
AEC_LOG_RE = r"aec:|capture:|streaming mic|config: name"


def sh(cmd, input_bytes=None, timeout=120):
    """Run a command, return (rc, stdout bytes). Never raises on a non-zero exit."""
    p = subprocess.run(cmd, input=input_bytes, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=timeout)
    return p.returncode, p.stdout


# ---- the TV (over ssh) -----------------------------------------------------------------------

class Tv:
    def __init__(self, target, tv_dir="/var/lib/webos-satellite", run=sh):
        self.target, self.dir, self.run = target, tv_dir.rstrip("/"), run
        self.cfg = f"{self.dir}/etc/config.toml"
        self.ctl = f"{self.dir}/bin/ctl.sh"
        self.log = f"{self.dir}/log/webos-satellite.log"
        self.pidf = f"{self.dir}/run/webos-satellite.pid"

    def ssh(self, remote, input_bytes=None, tty=False, timeout=120):
        cmd = ["ssh"] + SSH_OPTS + (["-tt"] if tty else []) + [self.target, remote]
        return self.run(cmd, input_bytes, timeout)

    def now(self):
        rc, out = self.ssh("date -u +%s")
        return int(out.split()[0]) if rc == 0 and out.strip() else None

    def get_volume(self):
        rc, out = self.ssh("luna-send -n 1 -w 3000 luna://com.webos.service.audio/master/getVolume '{}'", tty=True)
        m = re.search(rb'"volume":\s*(\d+)', out)
        if rc != 0 or not m:
            raise RuntimeError("cannot read the TV volume")
        return int(m.group(1))

    def set_volume(self, n, restore=False):
        if not restore:
            L.check_tv_volume(n, "set TV volume")
        self.ssh("luna-send -n 1 -w 3000 luna://com.webos.service.audio/master/setVolume "
                 f"'{{\"volume\":{int(n)}}}' >/dev/null", tty=True)
        got = self.get_volume()
        if got != n:
            raise RuntimeError(f"TV volume read back {got}, expected {n}")
        if not restore:
            L.check_tv_volume(got, "TV volume read back")
        return got

    def read_config(self):
        rc, out = self.ssh(f"cat {self.cfg}")
        if rc != 0 or not out:
            raise RuntimeError("cannot read the TV config")
        return out

    def write_config(self, data):
        rc, out = self.ssh(f"cat > {self.cfg}.new && chmod 600 {self.cfg}.new && md5sum {self.cfg}.new", input_bytes=data)
        got = out.split()[0].decode() if out.split() else ""
        if rc != 0 or got != L.md5(data):
            self.ssh(f"rm -f {self.cfg}.new")
            raise RuntimeError("config upload failed its md5 check; nothing was replaced")
        rc, _ = self.ssh(f"mv {self.cfg}.new {self.cfg}")
        if rc != 0:
            raise RuntimeError("could not move the new config into place")

    def restart(self):
        """Returns the TV clock (epoch s) just before the restart, for the log filter."""
        t = self.now()
        self.ssh(f"sh {self.ctl} restart 2>&1 | tail -2", timeout=90)
        return t

    def stop(self):
        self.ssh(f"sh {self.ctl} stop 2>&1 | tail -2", timeout=60)

    def log_entries(self, tail=6000):
        # the daemon rotates its log (about 256 KB, under 20 min at debug level): read the rotated file too
        rc, out = self.ssh(f"cat {self.log}.1 {self.log} 2>/dev/null | tail -n {tail}", timeout=60)
        return L.parse_aec_lines(out.decode(errors="replace")), out.decode(errors="replace")

    def aec_log_lines(self):
        """The `aec:` / `capture:` / streaming / config lines of the current and the rotated log."""
        rc, out = self.ssh(f"cat {self.log}.1 {self.log} 2>/dev/null | grep -E '{AEC_LOG_RE}'", timeout=60)
        return out.decode(errors="replace").splitlines()

    def wait_streaming(self, t_restart, timeout=90):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            ents, _ = self.log_entries(600)
            if any(e["kind"] == "streaming" and e["t"] >= t_restart - 1 for e in ents):
                return True
            time.sleep(2)
        return False

    def wait_lock(self, t_restart, timeout=60):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            ents, _ = self.log_entries(600)
            locks = [e for e in ents if e["kind"] in ("lock", "change") and e["t"] >= t_restart - 1]
            if locks:
                return locks[-1]["delay_ms"]
            time.sleep(2)
        return None

    def cpu_sample(self):
        rc, out = self.ssh(f"P=$(cat {self.pidf}); echo $(cut -d' ' -f14,15 /proc/$P/stat) $(cut -d' ' -f1 /proc/uptime)")
        f = out.split()
        if rc != 0 or len(f) != 3:
            return None
        return (int(f[0]) + int(f[1])) / 100.0, float(f[2])

    @staticmethod
    def cpu_pct(a, b):
        if not a or not b or b[1] <= a[1]:
            return None
        return round(100 * (b[0] - a[0]) / (b[1] - a[1]), 1)


# ---- the PC (PipeWire/PulseAudio) ------------------------------------------------------------

class Pc:
    def __init__(self, run=sh):
        self.run = run

    def sinks(self):
        rc, out = self.run(["pactl", "list", "short", "sinks"])
        return {l.split("\t")[1] for l in out.decode().splitlines() if "\t" in l}

    def get_volume(self, sink):
        rc, out = self.run(["pactl", "get-sink-volume", sink])
        m = re.search(rb"(\d+) / +\d+%", out)
        if rc != 0 or not m:
            raise RuntimeError(f"cannot read the volume of sink {sink}")
        return int(m.group(1))

    def set_volume(self, sink, value):
        """value: '82%' or a raw integer."""
        self.run(["pactl", "set-sink-volume", sink, str(value)])


class Content:
    """content_play.py in a subprocess (the PC's HDMI sink -> the TV)."""

    def __init__(self, sink, manifest, category, repeat, playlog, popen=subprocess.Popen):
        self.args = [sys.executable, str(HERE / "content_play.py"), "--sink", sink, "--manifest", str(manifest),
                     "--only", category, "--repeat", str(repeat), "--out", str(playlog)]
        self.popen, self.proc = popen, None

    def start(self):
        self.proc = self.popen(self.args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.proc = None

    @property
    def running(self):
        return self.proc is not None and self.proc.poll() is None


# ---- condition and restore order (the safety-critical sequences) ------------------------------

def apply_condition(tv, pc, content, plan, cname, talker_sink, content_sink):
    """Content off, TV volume to the condition's (at most 20), then the PC sink levels, then content on.
    The TV volume is lowered before the PC HDMI sink is raised."""
    cond = plan["conditions"][cname]
    content.stop()
    tv.set_volume(cond["tv_volume"])
    if cond["content"]:
        pc.set_volume(content_sink, f"{plan['content']['pc_sink_pct']}%")
    pc.set_volume(talker_sink, f"{plan['talkers'][cond['talker']]['sink_pct']}%")
    if cond["content"]:
        content.start()


def restore_levels(tv, pc, content, orig):
    """Content off, PC sinks back (HDMI first), then the TV volume back. Returns the list of failures."""
    errs = []
    steps = [("content", content.stop)]
    for sink, raw in orig.get("pc", {}).items():
        steps.append((f"sink {sink}", lambda s=sink, r=raw: pc.set_volume(s, r)))
    if orig.get("tv_volume") is not None:
        steps.append(("TV volume", lambda: tv.set_volume(orig["tv_volume"], restore=True)))
    for name, fn in steps:
        try:
            fn()
        except Exception as e:  # keep restoring the rest
            errs.append(f"{name}: {e}")
    return errs


# ---- audio helpers (numpy/soundfile, only for the capture stages) ----------------------------

def build_train(parts, out_wav, rate=48000):
    """Concatenate ('wav', path, gain_factor) and ('silence', seconds) parts into one 16-bit stereo WAV.
    Returns [{name, t0, dur}] for the wav parts."""
    import numpy as np
    import soundfile as sf
    chunks, sched, t = [], [], 0.0
    for p in parts:
        if p[0] == "silence":
            chunks.append(np.zeros((int(p[1] * rate), 2)))
            t += p[1]
        else:
            x, sr = sf.read(str(p[1]), dtype="float64", always_2d=True)
            if sr != rate:
                raise RuntimeError(f"{p[1]} is {sr} Hz, expected {rate}")
            if x.shape[1] == 1:
                x = np.repeat(x, 2, axis=1)
            x = x[:, :2] * p[2]
            sched.append({"name": Path(p[1]).stem, "t0": round(t, 3), "dur": round(len(x) / rate, 3)})
            chunks.append(x)
            t += len(x) / rate
    sf.write(str(out_wav), np.concatenate(chunks), rate, subtype="PCM_16")
    return sched


# ---- the live run ----------------------------------------------------------------------------

class Ctx:
    pass


def log(msg):
    print(f"{datetime.datetime.now():%H:%M:%S} {msg}", file=sys.stderr, flush=True)


def ensure_daytime(a):
    L.check_clock(datetime.datetime.now().astimezone(), a.allow_any_time)


def tv_arm_apply(ctx, arm):
    """Render the arm into the TV config (backup is already on the PC) and restart the daemon."""
    snippet = ctx.plan["arms"][arm]["aec"]
    new = L.render_config(ctx.base_text, snippet, ctx.plan["log_level"])
    if L.outside_managed(new) != L.outside_managed(ctx.base_text):
        raise RuntimeError("rendered config differs outside the managed parts; refusing to write it")
    ctx.tv.write_config(new.encode())
    ctx.config_changed = True
    t = ctx.tv.restart()
    ctx.cur_arm = arm
    return t


def prior_runs(runs):
    """Run ids to ignore when looking for the trial's run: every listed run except the newest, which is
    the armed wake word run (wait_ready checked it) that a detection continues into STT."""
    if not runs:
        return set()
    newest = max(runs, key=lambda r: r.get("timestamp", ""))
    return {r["pipeline_run_id"] for r in runs if r is not newest}


async def run_trial(ctx, item, blk):
    """One played item. Returns the raw trial record (see aec_live_lib.trial_row)."""
    pr = ctx.p2r
    a, plan = ctx.args, ctx.plan
    tm = plan["timing"]
    rec = {"arm": blk["arm"], "condition": blk["condition"], "block": blk["index"], "set": blk["set"],
           "item": ctx.item_fields(item), "detections_s": [], "stt_text": None, "valid": True, "notes": []}
    if not await pr.wait_ready(ctx.ha, ctx.pid, ctx.watch, tm["settle_s"]):
        rec.update(valid=False, t_play=time.time(), lead_s=tm["lead_s"], sink_latency_s=tm["sink_latency_s"])
        rec["notes"].append("satellite_not_ready")
        return rec
    seen = ctx.seen | prior_runs(await ctx.ha.pipeline_runs(ctx.pid))
    ctx.seen = seen
    sf, np = ctx.sf, ctx.np
    x, sr = sf.read(str(Path(a.corpus) / item["file_48k_stereo"]), dtype="float64", always_2d=True)
    factor = 10 ** ((plan["gain_db"] - 24) / 20)
    wav = Path(ctx.tmpdir) / "item.wav"
    sf.write(str(wav), np.concatenate([np.zeros((int(tm["lead_s"] * sr), x.shape[1])), x * factor,
                                       np.zeros((int(0.3 * sr), x.shape[1]))]), sr, subtype="PCM_16")
    n_changes = len(ctx.watch.changes)
    t_play = time.time()
    rc = await asyncio.to_thread(pr.play, a.talker_sink, wav)
    t_item0 = t_play + tm["lead_s"] + tm["sink_latency_s"]
    item_end = t_item0 + item["duration_ms"] / 1000
    while time.time() < item_end + tm["post_s"]:
        await asyncio.sleep(0.1)
    if any(c[2] not in ("idle", None) for c in ctx.watch.changes[n_changes:]) or ctx.watch.state != "idle":
        t_lim = time.monotonic() + tm["max_run_s"]
        while ctx.watch.state != "idle" and time.monotonic() < t_lim:
            await asyncio.sleep(0.2)
    dets = [round(c[0] - t_item0, 3) for c in ctx.watch.changes[n_changes:] if c[2] == "listening" and c[1] == "idle"]
    runs = []
    for _ in range(20):
        runs = []
        for r in await ctx.ha.pipeline_runs(ctx.pid):
            if r["pipeline_run_id"] in seen:
                continue
            full = await ctx.ha.pipeline_run(ctx.pid, r["pipeline_run_id"])
            if any(e.get("type") in ("run-end", "error") for e in full.get("events", [])):
                runs.append(ctx.ha_ws.summarize_run(full))
        if len(runs) >= len(dets):
            break
        await asyncio.sleep(0.5)
    stt = next((r["stt_text"] for r in runs if r["wake"] and r["stt_text"] is not None), None)
    rec.update(detections_s=dets, stt_text=stt, t_play=round(t_play, 3), lead_s=tm["lead_s"],
               sink_latency_s=tm["sink_latency_s"], play_rc=rc, item_end=round(item_end, 3),
               runs=[{k: v for k, v in r.items() if k != "events"} for r in runs])
    return rec


async def reconnect(ctx):
    for attempt in range(1, 6):
        try:
            try:
                await ctx.ha.__aexit__(None, None, None)
            except Exception:
                pass
            ctx.ha = ctx.ha_ws.HaWs(ctx.args.ha_url, ctx.args.token_file)
            await ctx.ha.__aenter__()
            ctx.pid = await ctx.ha.pipeline_id(ctx.args.pipeline)
            ctx.watch.state = None
            await ctx.ha.subscribe({"type": "subscribe_events", "event_type": "state_changed"}, ctx.watch)
            st = await ctx.ha.call({"type": "get_states"})
            ctx.watch.state = next((s["state"] for s in st if s["entity_id"] == ctx.args.entity), None)
            return True
        except Exception as e:
            log(f"HA reconnect {attempt} failed: {type(e).__name__}")
            await asyncio.sleep(10)
    return False


async def run_ab(ctx):
    a, plan = ctx.args, ctx.plan
    items = {i["id"]: i for i in ctx.load_items(a.corpus, None, None)}
    missing = sorted({i for b in plan["blocks"] for i in b["items"]} - set(items))
    if missing:
        raise RuntimeError(f"items not in the corpus: {missing[:5]}")
    cur_cond, t_restart = None, None
    for blk in plan["blocks"]:
        ensure_daytime(a)
        cname = blk["condition"]
        cond = plan["conditions"][cname]
        log(f"block {blk['index']}/{len(plan['blocks'])}: {cname} {blk['arm']} set {blk['set']}")
        bnotes = []
        if cname != cur_cond or plan["schedule"].get("content_each_block"):
            apply_condition(ctx.tv, ctx.pc, ctx.content, plan, cname, a.talker_sink, a.content_sink)
            ctx.touched_levels = True
            cur_cond = cname
            if cond["content"]:
                await asyncio.sleep(plan["content_lead_s"])
        restarted = False
        if blk["arm"] != ctx.cur_arm:
            t_restart = tv_arm_apply(ctx, blk["arm"])
            restarted = True
            if t_restart is None:
                raise RuntimeError("cannot read the TV clock")
            if not ctx.tv.wait_streaming(t_restart):
                bnotes.append("no_streaming_after_restart")
                log("WARNING: the satellite did not report streaming within 90 s")
            if L.arm_aec_enabled(plan["arms"][blk["arm"]]["aec"]) and cond["content"]:
                lock = ctx.tv.wait_lock(t_restart)
                if lock is None:
                    bnotes.append("no_delay_lock")
                    log("WARNING: no delay lock within 60 s of the restart")
                else:
                    bnotes.append(f"lock_ms={lock}")
        else:
            bnotes.append("no_restart")
        cpu0 = ctx.tv.cpu_sample()
        t_blk0 = time.time()
        for item_id in blk["items"]:
            ensure_daytime(a)
            rec = None
            for attempt in (1, 2):
                try:
                    rec = await run_trial(ctx, items[item_id], blk)
                    if attempt == 2:
                        rec["notes"].append("retried_after_ha_error")
                    break
                except (ConnectionError, OSError, asyncio.TimeoutError, RuntimeError) as e:
                    log(f"trial {item_id}: {type(e).__name__}; reconnecting to HA")
                    ok = await reconnect(ctx)
                    if not ok:
                        raise RuntimeError("HA connection lost and not recovered")
            if rec is None:
                rec = {"arm": blk["arm"], "condition": cname, "block": blk["index"], "set": blk["set"],
                       "item": ctx.item_fields(items[item_id]), "detections_s": [], "stt_text": None, "valid": False,
                       "notes": ["ha_connection_lost"], "t_play": time.time(), "lead_s": 0.0, "sink_latency_s": 0.0}
            ctx.trials_f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            ctx.trials_f.flush()
            ctx.trial_recs.append(rec)
            log(f"  {item_id}: detected={'yes' if rec['detections_s'] else 'no'}")
        cpu1 = ctx.tv.cpu_sample()
        t_blk1 = time.time()
        wins = [(r["t_play"], r.get("item_end", r["t_play"]) + 3.0) for r in ctx.trial_recs if r["block"] == blk["index"]]
        fw = 0
        if cond["content"]:
            fw = sum(1 for c in ctx.watch.changes if c[1] == "idle" and c[2] == "listening" and t_blk0 <= c[0] <= t_blk1
                     and not any(w0 - 0.5 <= c[0] <= w1 for w0, w1 in wins))
        brec = {"block": blk["index"], "arm": blk["arm"], "condition": cname, "set": blk["set"],
                "trials": len(blk["items"]), "t_start": round(t_blk0, 3), "t_end": round(t_blk1, 3),
                "content_s": round(t_blk1 - t_blk0, 1) if cond["content"] else 0, "false_wakes": fw,
                "daemon_cpu_pct": Tv.cpu_pct(cpu0, cpu1), "restarted": restarted, "notes": bnotes}
        ctx.block_recs[blk["index"]] = brec
        ctx.blocks_f.write(json.dumps(brec) + "\n")
        ctx.blocks_f.flush()
        try:
            collect_log(ctx)        # before the log rotates the block's `aec:` lines away
        except Exception as e:
            log(f"could not fetch the TV log after block {blk['index']}: {e}")
    ctx.content.stop()


def collect_log(ctx):
    """Add the TV log's new `aec:` / `capture:` / streaming lines to tvlog-aec.txt (kept across rotations)."""
    out = Path(ctx.out) / "tvlog-aec.txt"
    if not hasattr(ctx, "log_lines"):
        ctx.log_lines = out.read_text().splitlines() if out.exists() else []
    have = set(ctx.log_lines)
    for l in ctx.tv.aec_log_lines():
        if not re.search(AEC_LOG_RE, l):
            continue
        l = re.sub(r"config: name=\S+.*", "config: (redacted)", l)
        if l not in have:
            have.add(l)
            ctx.log_lines.append(l)
    out.write_text("\n".join(ctx.log_lines) + "\n")
    return ctx.log_lines


def finalize_outputs(ctx):
    """tvlog-aec.txt, trials.csv and blocks.csv from what was recorded (safe to call more than once)."""
    out = Path(ctx.out)
    try:
        collect_log(ctx)
    except Exception as e:
        log(f"could not fetch the TV log: {e}")
    ents = L.parse_aec_lines((out / "tvlog-aec.txt").read_text()) if (out / "tvlog-aec.txt").exists() else []
    rebuild_csv(ctx.plan, ctx.run_id, out, ents, ctx.skew)


def rebuild_csv(plan, run_id, out, entries, skew):
    out = Path(out)
    recs = [json.loads(l) for l in open(out / "trials.jsonl") if l.strip()]
    blocks = {}
    if (out / "blocks.jsonl").exists():
        for l in open(out / "blocks.jsonl"):
            if l.strip():
                b = json.loads(l)
                blocks[b["block"]] = b
    rows = [L.trial_row(plan, run_id, r, entries, blocks.get(r["block"]), skew) for r in recs]
    L.write_rows(out / "trials.csv", rows)
    import csv
    with open(out / "blocks.csv", "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(["run_id", "plan", "block", "arm", "condition", "trials", "content_s", "false_wakes",
                    "daemon_cpu_pct", "notes"])
        for b in blocks.values():
            w.writerow([run_id, plan["id"], b["block"], b["arm"], b["condition"], b["trials"], b["content_s"],
                        b["false_wakes"], "" if b["daemon_cpu_pct"] is None else b["daemon_cpu_pct"],
                        ";".join(b["notes"])])
    return rows


# ---- optional stages: mic + reference capture and the echo-path sweep -------------------------

def _run_capture(ctx, name, seconds, play_fn, tag):
    """Satellite stopped; tv_dualcap.py streams mic + reference to a file on the PC while play_fn plays."""
    a = ctx.args
    out = Path(ctx.out) / tag
    out.mkdir(parents=True, exist_ok=True)
    tc = out / f"{name}.tc"
    script = (HERE / "tv_dualcap.py").read_bytes()
    ctx.tv.ssh("cat > /tmp/tv_dualcap.py", input_bytes=script)
    ctx.tmp_tv_files.append("/tmp/tv_dualcap.py")
    dc = ctx.plan["dualcap"]
    cmd = (f"python3 /tmp/tv_dualcap.py --seconds {int(seconds)} --ref {dc.get('ref', 'hw:LGSIC,3')} "
           f"--ref-channels {dc.get('ref_channels', 2)} --ref-format {dc.get('ref_format', 'S32_LE')}")
    with open(tc, "wb") as f:
        p = subprocess.Popen(["ssh"] + SSH_OPTS + [a.tv_ssh, cmd], stdout=f, stderr=subprocess.DEVNULL)
        time.sleep(3)
        play_fn()
        try:
            p.wait(timeout=seconds + 30)
        except subprocess.TimeoutExpired:
            p.terminate()
    cap = out / name
    subprocess.run([sys.executable, str(HERE / "tv_dualcap_demux.py"), str(tc), "--out-dir", str(cap)], check=False)
    return cap


def stage_dualcap(ctx):
    a, plan = ctx.args, ctx.plan
    dc = plan["dualcap"]
    ctx.tv.stop()
    ctx.satellite_stopped = True
    ids = [i for s in dc.get("sets", list(plan["sets"])) for i in plan["sets"][s]]
    ids = L.seeded_order(ids, plan["seed"], "dualcap")
    for cname in dc.get("conditions", list(plan["conditions"])):
        ensure_daytime(a)
        cond = plan["conditions"][cname]
        log(f"dualcap {cname}: {len(ids)} items, satellite stopped")
        apply_condition(ctx.tv, ctx.pc, ctx.content, plan, cname, a.talker_sink, a.content_sink)
        ctx.touched_levels = True
        gap = float(dc.get("gap_s", 2.5))
        parts = [("silence", plan["content_lead_s"] + 1.0)]
        factor = 10 ** ((plan["gain_db"] - 24) / 20)
        for i in ids:
            parts += [("wav", Path(a.corpus) / "items" / "48k_stereo" / f"{i}.wav", factor), ("silence", gap)]
        parts.append(("silence", 3.0))
        wav = Path(ctx.out) / "dualcap" / f"{cname}-session.wav"
        wav.parent.mkdir(parents=True, exist_ok=True)
        sched = build_train(parts, wav)
        dur = sum(p[1] for p in parts if p[0] == "silence") + sum(s["dur"] for s in sched)
        (wav.parent / f"{cname}-session.json").write_text(json.dumps(sched, indent=1))

        def play():
            subprocess.run(["pw-play", "--target", a.talker_sink, str(wav)], check=False)
        _run_capture(ctx, cname, dur + 10, play, "dualcap")
        ctx.content.stop()


def stage_sweep(ctx):
    a, plan = ctx.args, ctx.plan
    sw = plan["sweep"]
    ctx.tv.stop()
    ctx.satellite_stopped = True
    ensure_daytime(a)
    ctx.content.stop()
    ctx.tv.set_volume(int(sw["tv_volume"]))
    ctx.pc.set_volume(a.content_sink, f"{plan['content']['pc_sink_pct']}%")
    ctx.touched_levels = True
    stim = Path(a.stimuli_dir) / sw.get("stimulus", "sweep_48k_stereo.wav")
    parts = [("silence", float(sw.get("lead_s", 3.0)))]
    for _ in range(int(sw.get("repeats", 3))):
        parts += [("wav", stim, 1.0), ("silence", float(sw.get("gap_s", 2.0)))]
    parts.append(("silence", 3.0))
    out = Path(ctx.out) / "echo-path"
    out.mkdir(parents=True, exist_ok=True)
    wav = out / "sweep-train.wav"
    sched = build_train(parts, wav)
    (out / "sweep-train.json").write_text(json.dumps(sched, indent=1))
    dur = sum(p[1] for p in parts if p[0] == "silence") + sum(s["dur"] for s in sched)

    def play():
        subprocess.run(["pw-play", "--target", a.content_sink, str(wav)], check=False)
    log(f"echo-path sweep: {dur:.0f} s through the TV, satellite stopped")
    _run_capture(ctx, "sweep", dur + 8, play, "echo-path")


# ---- main ------------------------------------------------------------------------------------

def parse_args(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("plan", nargs="?", help="plan file (aec_plans/*.toml)")
    ap.add_argument("--dry-run", action="store_true", help="print the schedule and the sound minutes; touch nothing")
    ap.add_argument("--allow-any-time", action="store_true", help="allow sound outside 08:00 to 21:00 local time")
    ap.add_argument("--stages", default="", help="comma list of ab,dualcap,sweep (default: the stages the plan enables)")
    ap.add_argument("--run-id", default="")
    ap.add_argument("--device", default="c3-a", help="alias for the private output directory")
    ap.add_argument("--out-dir", default="")
    ap.add_argument("--tv-ssh", help="ssh target of the TV, for example root@host (kept out of the repo)")
    ap.add_argument("--tv-dir", default="/var/lib/webos-satellite")
    ap.add_argument("--ha-url")
    ap.add_argument("--token-file", help="HA token file, mode 0600 (never printed)")
    ap.add_argument("--pipeline", help="the pipeline selected on the satellite (name or id)")
    ap.add_argument("--entity", help="assist_satellite.* entity id")
    ap.add_argument("--corpus", default=str(ROOT / "devices/private/corpus/p2-v1"))
    ap.add_argument("--content-manifest", default=str(ROOT / "devices/private/content/p2-v1/manifest.csv"))
    ap.add_argument("--stimuli-dir", default=str(ROOT / "devices/private/stimuli/p2-v1"))
    ap.add_argument("--talker-sink", help="exact sink name of the loudspeaker (never an HDMI sink)")
    ap.add_argument("--content-sink", help="exact sink name of the HDMI sink that feeds the TV (no default)")
    ap.add_argument("--public-csv", default="", help="also write trials.csv here (for example docs/experiments/data/NAME.csv)")
    ap.add_argument("--rebuild", metavar="OUT_DIR", help="rebuild trials.csv/blocks.csv of an earlier run and exit")
    return ap, ap.parse_args(argv)


def item_seconds_from_corpus(corpus):
    try:
        import csv
        return {r["id"]: int(r["duration_ms"]) / 1000 for r in csv.DictReader(open(Path(corpus) / "manifest.csv"))}
    except OSError:
        return None


def dry_run(a, plan):
    now = datetime.datetime.now().astimezone()
    print(f"local date/time now: {now:%Y-%m-%d %H:%M:%S %Z}")
    try:
        L.check_clock(now, a.allow_any_time)
        print(f"clock check: sound is allowed now ({L.DAY_START_H:02d}:00 to {L.DAY_END_H:02d}:00 local)")
    except L.SafetyError as e:
        print(f"clock check: WOULD REFUSE: {e}")
    if a.talker_sink or a.content_sink:
        try:
            L.check_sinks(a.content_sink, a.talker_sink, content_needed=bool(a.content_sink))
            print("sink check: ok")
        except L.SafetyError as e:
            print(f"sink check: WOULD REFUSE: {e}")
    print()
    print(L.format_schedule(plan, item_seconds_from_corpus(a.corpus)))
    stages = [s for s in ("ab", "dualcap", "sweep") if s == "ab" or plan[s].get("enabled")]
    print(f"stages: {', '.join(stages)}")
    print("dry run: no device, sink, TV or Home Assistant was touched")
    return 0


def main(argv=None):
    ap, a = parse_args(argv)
    if a.rebuild:
        if not a.plan:
            ap.error("--rebuild needs the plan file too")
        plan = L.load_plan(a.plan)
        out = Path(a.rebuild)
        ents = L.parse_aec_lines((out / "tvlog-aec.txt").read_text()) if (out / "tvlog-aec.txt").exists() else []
        meta = json.loads((out / "run.json").read_text()) if (out / "run.json").exists() else {}
        rows = rebuild_csv(plan, meta.get("run_id", plan["id"]), out, ents, meta.get("skew_s", 0.0))
        print(f"{len(rows)} rows -> {out / 'trials.csv'}")
        return 0
    if not a.plan:
        ap.error("a plan file is required")
    try:
        plan = L.load_plan(a.plan)
    except (L.PlanError, L.SafetyError) as e:
        print(f"plan refused: {e}", file=sys.stderr)
        return 2
    if a.dry_run:
        return dry_run(a, plan)
    return asyncio.run(live(a, plan))


async def live(a, plan):
    stages = [s for s in a.stages.split(",") if s] or [s for s in ("ab", "dualcap", "sweep")
                                                      if s == "ab" or plan[s].get("enabled")]
    need = {"ab": ["tv_ssh", "ha_url", "token_file", "pipeline", "entity", "talker_sink"],
            "dualcap": ["tv_ssh", "talker_sink"], "sweep": ["tv_ssh", "content_sink"]}
    for s in stages:
        for k in need[s]:
            if not getattr(a, k):
                print(f"--{k.replace('_', '-')} is required for stage {s}", file=sys.stderr)
                return 2
    run_id = a.run_id or f"{plan['id']}-{datetime.date.today():%Y%m%d}"
    out = Path(a.out_dir) if a.out_dir else ROOT / "devices" / "private" / a.device / run_id
    # --- rails that need no device ---
    try:
        ensure_daytime(a)
        any_content = any(c["content"] for c in plan["conditions"].values()) or "sweep" in stages
        talker_needed = bool({"ab", "dualcap"} & set(stages))
        L.check_sinks(a.content_sink, a.talker_sink, content_needed=any_content, talker_needed=talker_needed)
        if "ab" in stages:
            L.check_token_file(a.token_file)
    except L.SafetyError as e:
        print(f"refused: {e}", file=sys.stderr)
        return 2
    print(f"local date/time: {datetime.datetime.now().astimezone():%Y-%m-%d %H:%M:%S %Z}", file=sys.stderr)

    import soundfile
    import numpy
    ctx = Ctx()
    ctx.args, ctx.plan, ctx.run_id, ctx.out = a, plan, run_id, out
    ctx.sf, ctx.np = soundfile, numpy
    ctx.tv, ctx.pc = Tv(a.tv_ssh, a.tv_dir), Pc()
    known = ctx.pc.sinks()
    try:
        L.check_sinks(a.content_sink, a.talker_sink, content_needed=any_content, talker_needed=talker_needed, known=known)
    except L.SafetyError as e:
        print(f"refused: {e}", file=sys.stderr)
        return 2
    t_tv = ctx.tv.now()
    t_pc = time.time()
    if t_tv is None or abs(t_tv - t_pc) > L.MAX_CLOCK_SKEW_S:
        print(f"refused: TV clock {t_tv} and PC clock differ by more than {L.MAX_CLOCK_SKEW_S} s", file=sys.stderr)
        return 2
    ctx.skew = round(t_tv - t_pc, 1)
    out.mkdir(parents=True, exist_ok=True)
    os.chmod(out, 0o700)
    ctx.content = Content(a.content_sink or "", a.content_manifest, plan["content"]["category"],
                          plan["content"]["repeat"], out / "content-playlog.jsonl")
    ctx.cur_arm, ctx.config_changed, ctx.satellite_stopped, ctx.touched_levels = None, False, False, False
    ctx.tmp_tv_files, ctx.trial_recs, ctx.block_recs, ctx.seen = [], [], {}, set()
    ctx.tmpdir = tempfile.mkdtemp(prefix="aeclive-", dir=out)
    # --- snapshot the originals before anything changes ---
    orig_cfg = ctx.tv.read_config()
    ctx.base_text = orig_cfg.decode()
    fd = os.open(out / "config.toml.orig", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "wb") as f:
        f.write(orig_cfg)
    orig = {"tv_volume": ctx.tv.get_volume(), "pc": {}}
    for s in (a.content_sink, a.talker_sink):
        if s:
            orig["pc"][s] = ctx.pc.get_volume(s)
    ordered = {}
    for s in (a.content_sink, a.talker_sink):   # HDMI sink restored first
        if s:
            ordered[s] = orig["pc"][s]
    orig["pc"] = ordered
    (out / "run.json").write_text(json.dumps({"run_id": run_id, "plan": plan["id"], "skew_s": ctx.skew,
                                              "config_md5": L.md5(orig_cfg), "stages": stages,
                                              "started": datetime.datetime.now().astimezone().isoformat()}, indent=1))
    log(f"original config md5 {L.md5(orig_cfg)}, TV volume {orig['tv_volume']}, backup in {out}")
    ctx.trials_f = open(out / "trials.jsonl", "a", buffering=1)
    ctx.blocks_f = open(out / "blocks.jsonl", "a", buffering=1)

    def on_term(*_):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, on_term)
    failures = []
    ctx.ha = None
    try:
        if "ab" in stages:
            import ha_ws
            import p2_runner
            from p2_digital import item_fields, load_items
            ctx.ha_ws, ctx.p2r, ctx.item_fields, ctx.load_items = ha_ws, p2_runner, item_fields, load_items
            ctx.watch = p2_runner.Watch(a.entity)
            ctx.ha = ha_ws.HaWs(a.ha_url, a.token_file)
            await ctx.ha.__aenter__()
            ctx.pid = await ctx.ha.pipeline_id(a.pipeline)
            await ctx.ha.subscribe({"type": "subscribe_events", "event_type": "state_changed"}, ctx.watch)
            st = await ctx.ha.call({"type": "get_states"})
            ctx.watch.state = next((s["state"] for s in st if s["entity_id"] == a.entity), None)
            ctx.seen = {r["pipeline_run_id"] for r in await ctx.ha.pipeline_runs(ctx.pid)}
            await run_ab(ctx)
        if "dualcap" in stages:
            stage_dualcap(ctx)
        if "sweep" in stages:
            stage_sweep(ctx)
    except (KeyboardInterrupt, asyncio.CancelledError):
        log("interrupted: restoring")
        failures.append("interrupted")
    except Exception as e:
        log(f"error: {type(e).__name__}: {e}; restoring")
        failures.append(f"{type(e).__name__}: {e}")
    finally:
        # a second Ctrl-C or SIGTERM must not abort the restore (EXP-020: a repeated SIGTERM did)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if ctx.ha:
            try:
                await ctx.ha.__aexit__(None, None, None)
            except Exception:
                pass
        if ctx.trial_recs:
            try:
                finalize_outputs(ctx)
            except Exception as e:
                log(f"could not write the CSV ({e}); rebuild with --rebuild")
        failures += restore_all(ctx, orig, orig_cfg)
        signal.signal(signal.SIGINT, signal.default_int_handler)
        signal.signal(signal.SIGTERM, signal.SIG_DFL)
    if a.public_csv and (out / "trials.csv").exists():
        Path(a.public_csv).write_bytes((out / "trials.csv").read_bytes())
        log(f"trials.csv copied to {a.public_csv}: check it, then commit")
    print(f"done: {len(ctx.trial_recs)} trials, outputs in {out}" + (f"; PROBLEMS: {failures}" if failures else ""),
          file=sys.stderr)
    return 1 if failures else 0


def restore_all(ctx, orig, orig_cfg):
    errs = []
    if ctx.touched_levels:
        errs += restore_levels(ctx.tv, ctx.pc, ctx.content, orig)
    else:
        ctx.content.stop()
    try:
        if ctx.config_changed or ctx.satellite_stopped:
            if ctx.config_changed:
                ctx.tv.write_config(orig_cfg)
                if L.md5(ctx.tv.read_config()) != L.md5(orig_cfg):
                    errs.append("config md5 differs after the restore")
            ctx.tv.restart()
        for f in ctx.tmp_tv_files:
            ctx.tv.ssh(f"rm -f {f}")
    except Exception as e:
        errs.append(f"config restore: {e}")
    try:
        for f in Path(ctx.tmpdir).glob("*"):
            f.unlink()
        os.rmdir(ctx.tmpdir)
    except OSError:
        pass
    log("restore finished" + (f" with problems: {errs}" if errs else ": TV volume, PC sinks and config are back"))
    return errs


if __name__ == "__main__":
    sys.exit(main())
