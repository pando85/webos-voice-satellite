#!/usr/bin/env python3
"""Log an Assist satellite's state changes and its pipeline runs from Home Assistant (JSON lines).

For the Phase 2 false-wake runs: while content plays and nobody speaks, every wake
detection shows up as the satellite entity leaving `idle`. Each line carries the PC
receive time (`t_pc`, Unix seconds) so detections can be mapped onto the content
playlist. After the entity returns to `idle`, the pipeline runs that ended since the
last check are fetched from `assist_pipeline/pipeline_debug` and logged in condensed
form (wake word id and audio timestamp, STT text, intent response type, errors).
HA keeps only the last few runs per pipeline, so the fetch happens after every run.

    ha_assist_watch.py --url https://ha.example --token-file tok.json \
        --entity assist_satellite.x --pipeline "test pipeline" --out events.jsonl --duration 4200

STT text is logged because the material is synthetic or licensed content; do not run
this while people talk in the room unless they agreed (AGENTS.md privacy rules).
"""
import argparse
import asyncio
import json
import sys
import time

import ha_ws


async def run(a):
    out = open(a.out, "a", buffering=1)

    def emit(**kw):
        kw.setdefault("t_pc", round(time.time(), 3))
        out.write(json.dumps(kw, ensure_ascii=False) + "\n")

    async with ha_ws.HaWs(a.url, a.token_file) as ha:
        pid = await ha.pipeline_id(a.pipeline)
        seen = {r["pipeline_run_id"] for r in await ha.pipeline_runs(pid)}
        emit(kind="start", entity=a.entity, pipeline_id=pid, preexisting_runs=len(seen))
        idle = asyncio.Event()

        def on_event(ev):
            d = ev.get("data", {})
            if d.get("entity_id") != a.entity:
                return
            new = (d.get("new_state") or {}).get("state")
            old = (d.get("old_state") or {}).get("state")
            emit(kind="state", old=old, new=new, time_fired=ev.get("time_fired"))
            if new == "idle" and old not in (None, "idle"):
                idle.set()

        await ha.subscribe({"type": "subscribe_events", "event_type": "state_changed"}, on_event)
        deadline = time.monotonic() + a.duration if a.duration else None
        while deadline is None or time.monotonic() < deadline:
            try:
                await asyncio.wait_for(idle.wait(), timeout=a.poll)
            except asyncio.TimeoutError:
                pass
            idle.clear()
            for r in await ha.pipeline_runs(pid):
                rid = r["pipeline_run_id"]
                if rid in seen:
                    continue
                run_ = await ha.pipeline_run(pid, rid)
                if not any(e.get("type") == "run-end" for e in run_.get("events", [])):
                    continue  # still running; picked up later
                seen.add(rid)
                emit(kind="run", run_id=rid, started=r.get("timestamp"), **ha_ws.summarize_run(run_))
        emit(kind="end")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--url", required=True)
    ap.add_argument("--token-file", required=True)
    ap.add_argument("--entity", required=True, help="assist_satellite.* entity id")
    ap.add_argument("--pipeline", required=True, help="pipeline name or id selected on the satellite")
    ap.add_argument("--out", required=True)
    ap.add_argument("--duration", type=float, default=0, help="seconds; 0 = until interrupted")
    ap.add_argument("--poll", type=float, default=20, help="pipeline poll interval when nothing happens (s)")
    a = ap.parse_args()
    try:
        asyncio.run(run(a))
    except KeyboardInterrupt:
        return 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
