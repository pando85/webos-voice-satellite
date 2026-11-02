#!/usr/bin/env python3
"""Play TV-content clips one after another on a named sink and log when each one played.

For the Phase 2 false-wake and echo runs: the content set from `make_content.py` goes from
the PC over HDMI to the TV. Each clip is played with `pw-play --target <sink>` at the file's
own level (the content is normalized to -23 LUFS); sink and TV volume are set by the
operator and recorded in the run notes, never here. One JSON line per clip with PC start
and end times (`t_start`, `t_end`, Unix seconds) and the category from `manifest.csv`, so
detections logged with PC time can be attributed to a category.

    content_play.py --sink <hdmi-sink> --manifest devices/private/content/p2-v1/manifest.csv \
        --out playlog.jsonl [--only sampler] [--exclude sampler] [--repeat 1]

Stop with SIGTERM/SIGINT (the current pw-play is stopped too).
"""
import argparse
import csv
import json
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path


def load_playlist(manifest, only=(), exclude=()):
    rows = list(csv.DictReader(open(manifest, newline="")))
    base = Path(manifest).parent
    out = []
    for r in rows:
        if only and r["category"] not in only:
            continue
        if r["category"] in exclude:
            continue
        out.append({"file": str(base / r["file"]), "category": r["category"],
                    "duration_s": float(r["duration_s"])})
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--sink", required=True, help="exact PipeWire/PulseAudio sink name")
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", action="append", default=[], help="category to include (repeatable)")
    ap.add_argument("--exclude", action="append", default=[], help="category to skip (repeatable)")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--gap", type=float, default=0.0, help="silence between clips (s)")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    if not shutil.which("pw-play"):
        sys.exit("pw-play not found")
    plist = load_playlist(a.manifest, a.only, a.exclude) * a.repeat
    total = sum(p["duration_s"] for p in plist)
    print(f"{len(plist)} clips, {total / 60:.1f} min", file=sys.stderr)
    if a.dry_run:
        for p in plist:
            print(p["category"], p["file"])
        return 0
    cur = {"proc": None}

    def stop(*_):
        if cur["proc"] and cur["proc"].poll() is None:
            cur["proc"].terminate()
        raise SystemExit(130)

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    with open(a.out, "a", buffering=1) as log:
        for i, p in enumerate(plist):
            t0 = time.time()
            cur["proc"] = subprocess.Popen(["pw-play", "--target", a.sink, p["file"]])
            rc = cur["proc"].wait()
            t1 = time.time()
            log.write(json.dumps({"i": i, "file": Path(p["file"]).name, "category": p["category"],
                                  "duration_s": p["duration_s"], "t_start": round(t0, 3),
                                  "t_end": round(t1, 3), "rc": rc}) + "\n")
            if a.gap:
                time.sleep(a.gap)
    return 0


if __name__ == "__main__":
    sys.exit(main())
