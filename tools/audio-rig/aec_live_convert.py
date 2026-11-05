#!/usr/bin/env python3
"""Convert the private EXP-018 trial files into the committed per-trial CSV.

Reads, from the private directory of EXP-018 (never committed): `ab.jsonl` and `ab-supplement.jsonl`
(one JSON line per played item from the scratch runner), `cells.log` (cell start/end times),
`watch-all.jsonl` (`ha_assist_watch.py`: pipeline runs, for the STT of the question items) and
`tvlog-aec.txt` (the satellite's `aec:` debug lines, for delay/ERLE/underruns per 60 s window).
Writes the schema of `aec_live_lib.CSV_COLUMNS`: no transcripts, no audio, no addresses, no
timestamps; STT is reduced to a match flag and a word error rate against the synthetic reference.

    aec_live_convert.py devices/private/c3-a/EXP-018 docs/experiments/data/EXP-018-trials.csv

Mapping: arm BNS -> "B+NS"; conditions Q (quiet, TV volume 10, owner-natural talker), C10 (content,
volume 10, owner-natural), C15 (content, volume 15, loud talker); the pass number (item set) and the
cell number go to `notes`. `daemon_cpu_pct` is blank: EXP-018 measured the CPU in separate windows
(see the experiment's section 2), not per trial.
"""
import argparse
import datetime
import json
import re
import sys
from pathlib import Path

import aec_live_lib as L

RUN_ID = "exp018-20261105"
PLAN = "exp018"
ARM_NAMES = {"BNS": "B+NS"}
COND = {"Q": (10, "natural"), "C10": (10, "natural"), "C15": (15, "loud")}
DATE = "2026-11-05"
LOCAL_TZ_NOTE = "cells.log times are local (CET, UTC+1)"
CET = datetime.timezone(datetime.timedelta(hours=1))


def read_jsonl(path):
    return [json.loads(l) for l in open(path) if l.strip()]


def cell_times(cells_log):
    """[(index, arm, cond, pass, t_start, t_end)] from `HH:MM:SS cell ARM COND pass N start|end` lines."""
    cells, open_ = [], {}
    for line in open(cells_log):
        m = re.match(r"(\d\d:\d\d:\d\d) cell (\S+) (\S+) pass (\d) (start|end)", line)
        if not m:
            continue
        t = datetime.datetime.strptime(f"{DATE} {m.group(1)}", "%Y-%m-%d %H:%M:%S").replace(tzinfo=CET).timestamp()
        key = (m.group(2), m.group(3), m.group(4))
        if m.group(5) == "start":
            open_[key] = t
        elif key in open_:
            cells.append((len(cells) + 1, key[0], key[1], int(key[2]), open_.pop(key), t))
    return cells


def stt_for_detection(runs, t_det_pc):
    """STT text of the pipeline run whose wake word fired within 4 s of the detection (PC time)."""
    best = None
    for r in runs:
        at = r.get("wake", {}).get("at") if r.get("wake") else None
        if not at:
            continue
        d = abs(datetime.datetime.fromisoformat(at).timestamp() - t_det_pc)
        if d < 4 and (best is None or d < best[0]):
            best = (d, r.get("stt_text"))
    return best[1] if best else None


def convert(src):
    src = Path(src)
    ab = read_jsonl(src / "ab.jsonl")
    sup = src / "ab-supplement.jsonl"
    if sup.exists():
        ab += [dict(r, _reconstructed=True) for r in read_jsonl(sup)]
    watch = read_jsonl(src / "watch-all.jsonl")
    runs = [w for w in watch if w.get("kind") == "run" and w.get("wake")]
    log = L.parse_aec_lines((src / "tvlog-aec.txt").read_text(errors="replace"))
    cells = cell_times(src / "cells.log")
    rows = []
    for r in sorted(ab, key=lambda r: r["t_play"]):
        arm_raw, cond, ps = r["condition"].split("-")
        it = r["item"]
        dur_ms = it["duration_ms"]
        det, lat = L.is_detected(r["detections_s"], dur_ms)
        t0 = r["t_play"] + r["lead_s"] + r["sink_latency_s"]
        stt_match = wer = None
        if det and it.get("subkind") == "cmd":
            text = stt_for_detection(runs, t0 + min(d for d in r["detections_s"]
                                                    if -0.2 <= d <= dur_ms / 1000 + 1.5))
            stt_match, wer = L.stt_scores(it, text)
        t_mid = t0 + dur_ms / 2000
        w = L.window_for(t_mid, log) if arm_raw != "OFF" else None
        cell = next((c for c in cells if c[4] - 20 <= r["t_play"] <= c[5] and c[1] == arm_raw and c[2] == cond
                     and c[3] == int(ps[1:])), None)
        notes = [f"pass={ps[1:]}"]
        if cell:
            notes.append(f"cell={cell[0]}")
        if r.get("_reconstructed"):
            notes.append("reconstructed")
        if w is None and arm_raw != "OFF":
            notes.append("no_aec_window")
        tv, tl = COND[cond]
        rows.append({"run_id": RUN_ID, "plan": PLAN, "arm": ARM_NAMES.get(arm_raw, arm_raw), "condition": cond,
                     "tv_volume": tv, "talker_level": tl, "item_id": it["id"], "voice": L.voice_label(it),
                     "phrase_id": L.phrase_id(it), "detected": det, "wake_latency_ms": lat,
                     "stt_match": stt_match, "stt_wer": wer,
                     "delay_ms": w["delay_ms"] if w else None, "erle_db": w["erle_db"] if w else None,
                     "underruns": w["underruns"] if w else None, "daemon_cpu_pct": None,
                     "notes": ";".join(notes)})
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("private_dir", help="devices/private/c3-a/EXP-018")
    ap.add_argument("out_csv", help="docs/experiments/data/EXP-018-trials.csv")
    a = ap.parse_args()
    rows = convert(a.private_dir)
    L.write_rows(a.out_csv, rows)
    print(f"{len(rows)} trials -> {a.out_csv}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
