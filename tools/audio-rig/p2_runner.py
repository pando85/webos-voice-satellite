#!/usr/bin/env python3
"""Play P2 corpus items one at a time to a live satellite and record what Home Assistant did.

For the Phase 2 wake/STT accuracy runs through HA's real integration. Before each item the
runner waits until the satellite is idle **and** its next wake word run is armed (the newest
run of the selected pipeline is in the `wake_word` stage), plus a settle time, so pipeline
time of one item never eats the next. It then plays the item on the named sink and watches
the satellite entity: a change from `idle` to `listening` is a wake detection, timed with the
PC clock. When a run happened it waits for it to end and for the re-arm, then fetches the
pipeline run from `assist_pipeline/pipeline_debug` (wake word id, STT text, intent response)
and asks the built-in agent's recognition-only endpoint for the intent of the transcript and of
the reference text (nothing is executed by that call).

Level: `--gain-db G` follows `play_record.py` on a corpus session file, whose loudest item peak
is -6 dBFS: every item is scaled by (G - 24) dB, so `--gain-db 12` is the owner-approved
reference level (-18 dBFS peak for the loudest item). Sink volumes are never changed here.

    p2_runner.py --url https://ha.example --token-file tok.json --pipeline "test pipeline" \
        --entity assist_satellite.x --corpus devices/private/corpus/p2-v1 --sink <jbl-sink> \
        --gain-db 12 --condition A-quiet --out results.jsonl [--kind pos] [--per-voice 5]

Output: one JSON line per item in the format `p2_score.py` reads. Detection times are in the
item's own timeline, assuming the sink adds `--sink-latency` seconds (state it in the report;
the P2-corpus window of -0.2/+1.5 s absorbs the rest). Replies should be silent during accuracy
runs (satellite `sink.gain = 0`), since they would overlap the next item.
"""
import argparse
import asyncio
import os
import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import numpy as np
import soundfile as sf

import ha_ws
from p2_digital import item_fields, load_items, stratified  # noqa: F401 (stratified re-exported)

CORPUS_PEAK_DBFS = -6.0


class Watch:
    """Satellite entity state changes with PC receive times."""

    def __init__(self, entity):
        self.entity = entity
        self.state = None
        self.changes = []  # (t_pc, old, new)
        self.event = asyncio.Event()

    def __call__(self, ev):
        d = ev.get("data", {})
        if d.get("entity_id") != self.entity:
            return
        new = (d.get("new_state") or {}).get("state")
        old = (d.get("old_state") or {}).get("state")
        self.state = new
        self.changes.append((time.time(), old, new))
        self.event.set()


async def armed(ha, pid):
    runs = await ha.pipeline_runs(pid)
    if not runs:
        return False
    newest = max(runs, key=lambda r: r.get("timestamp", ""))
    ev = (await ha.pipeline_run(pid, newest["pipeline_run_id"])).get("events", [])
    types = [e.get("type") for e in ev]
    return "wake_word-start" in types and "run-end" not in types and "wake_word-end" not in types


async def wait_ready(ha, pid, watch, settle, timeout=120):
    t_end = time.monotonic() + timeout
    while time.monotonic() < t_end:
        if watch.state == "idle" and await armed(ha, pid):
            await asyncio.sleep(settle)
            if watch.state == "idle" and await armed(ha, pid):
                return True
        await asyncio.sleep(0.5)
    return False


def play(sink, wav):
    return subprocess.run(["pw-play", "--target", sink, str(wav)], check=False).returncode


async def main_async(a):
    items = load_items(a.corpus, a.kind, a.id)
    if a.per_voice:
        items = stratified([i for i in items if i["kind"] == "pos"], a.per_voice) + \
            [i for i in items if i["kind"] != "pos"]
    if a.limit:
        items = items[:a.limit]
    factor = 10 ** ((a.gain_db - 30 - CORPUS_PEAK_DBFS) / 20)
    print(f"{len(items)} items, scale {20 * np.log10(factor):+.1f} dB", file=sys.stderr)
    out = open(a.out, "a", buffering=1)
    tmpdir = tempfile.mkdtemp(prefix="p2run-", dir=a.tmp_dir)
    async with ha_ws.HaWs(a.url, a.token_file) as ha:
        pid = await ha.pipeline_id(a.pipeline)
        watch = Watch(a.entity)
        await ha.subscribe({"type": "subscribe_events", "event_type": "state_changed"}, watch)
        st = await ha.call({"type": "get_states"})
        watch.state = next((s["state"] for s in st if s["entity_id"] == a.entity), None)
        seen = {r["pipeline_run_id"] for r in await ha.pipeline_runs(pid)}
        for n, it in enumerate(items, 1):
            ok = await wait_ready(ha, pid, watch, a.settle)
            if not ok:
                print(f"satellite not ready before {it['id']}; stopping", file=sys.stderr)
                break
            seen |= {r["pipeline_run_id"] for r in await ha.pipeline_runs(pid)}
            x, sr = sf.read(str(Path(a.corpus) / it["file_48k_stereo"]), dtype="float64", always_2d=True)
            lead = np.zeros((int(a.lead * sr), x.shape[1]))
            wav = Path(tmpdir) / "item.wav"
            sf.write(str(wav), np.concatenate([lead, x * factor, np.zeros((int(0.3 * sr), x.shape[1]))]), sr,
                     subtype="PCM_16")
            n_changes = len(watch.changes)
            t_play = time.time()
            rc = await asyncio.to_thread(play, a.sink, wav)
            t_item0 = t_play + a.lead + a.sink_latency
            item_end = t_item0 + it["duration_ms"] / 1000
            # wait for a detection until item end + post, then for any run to finish
            while time.time() < item_end + a.post:
                await asyncio.sleep(0.1)
            if any(c[2] not in ("idle", None) for c in watch.changes[n_changes:]) or watch.state != "idle":
                t_lim = time.monotonic() + a.max_run
                while watch.state != "idle" and time.monotonic() < t_lim:
                    await asyncio.sleep(0.2)
            dets = [round(c[0] - t_item0, 3) for c in watch.changes[n_changes:]
                    if c[2] == "listening" and c[1] == "idle"]
            # pipeline runs that ended since the item started
            runs = []
            for _ in range(20):
                runs = []
                for r in await ha.pipeline_runs(pid):
                    if r["pipeline_run_id"] in seen:
                        continue
                    full = await ha.pipeline_run(pid, r["pipeline_run_id"])
                    if any(e.get("type") == "run-end" for e in full.get("events", [])):
                        runs.append(ha_ws.summarize_run(full))
                if len(runs) >= len(dets):
                    break
                await asyncio.sleep(0.5)
            stt = next((r["stt_text"] for r in runs if r["wake"] and r["stt_text"] is not None), None)
            row = {"condition": a.condition, "item": item_fields(it), "detections_s": dets,
                   "stt_text": stt, "runs": [{k: v for k, v in r.items() if k != "events"} for r in runs],
                   "t_play": round(t_play, 3), "lead_s": a.lead, "sink_latency_s": a.sink_latency,
                   "gain_db": a.gain_db, "sink": a.sink, "play_rc": rc}
            if it["kind"] == "pos" and it["subkind"] == "cmd" and stt is not None:
                res = await ha.recognize([stt, it["expected_command_text"]], "es")
                row["recog"], row["recog_ref"] = res[0], res[1]
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
            print(f"{n}/{len(items)} {it['id']}: det={dets} stt={stt!r}", file=sys.stderr)
    try:
        os.remove(Path(tmpdir) / "item.wav")
        os.rmdir(tmpdir)
    except OSError:
        pass


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--url", required=True)
    ap.add_argument("--token-file", required=True)
    ap.add_argument("--pipeline", required=True, help="the pipeline selected on the satellite")
    ap.add_argument("--entity", required=True, help="assist_satellite.* entity id")
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--sink", required=True, help="exact sink name for pw-play --target")
    ap.add_argument("--gain-db", type=float, default=12.0, help="as play_record.py on a session file")
    ap.add_argument("--condition", required=True, help="label stored with every row")
    ap.add_argument("--out", required=True)
    ap.add_argument("--kind", action="append", help="pos, neg, nearmiss (repeatable; default all)")
    ap.add_argument("--id", action="append", help="item id (repeatable)")
    ap.add_argument("--per-voice", type=int, default=0, help="stratified subset of positives per voice")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--lead", type=float, default=1.0, help="silence played before each item (s)")
    ap.add_argument("--sink-latency", type=float, default=0.0, help="assumed sink output latency (s)")
    ap.add_argument("--settle", type=float, default=2.0, help="wait after the re-arm before playing (s)")
    ap.add_argument("--post", type=float, default=3.0, help="wait for a detection after the item (s)")
    ap.add_argument("--max-run", type=float, default=60.0, help="longest pipeline run to wait for (s)")
    ap.add_argument("--tmp-dir", default=None, help="where the scaled item WAV is written (deleted after)")
    a = ap.parse_args()
    if not -60 <= a.gain_db <= 24:
        sys.exit("--gain-db must be within [-60, +24]")
    asyncio.run(main_async(a))
    return 0


if __name__ == "__main__":
    sys.exit(main())
