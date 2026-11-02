#!/usr/bin/env python3
"""Split a `tv_dualcap.py` stream into raw PCM files and a JSON-lines meta file.

Writes `<prefix>mic.raw`, `<prefix>ref.raw` (interleaved, as captured) and
`<prefix>meta.jsonl` into --out-dir. Prints a one-line summary. Stdlib only.
"""
import argparse
import json
import os
import struct
import sys

NAMES = {0: "mic.raw", 1: "ref.raw"}


def demux(stream, out_dir, prefix=""):
    os.makedirs(out_dir, exist_ok=True)
    files = {}
    metas = []
    counts = {0: 0, 1: 0}
    try:
        while True:
            hdr = stream.read(7)
            if not hdr:
                break
            if len(hdr) < 7 or hdr[:2] != b"TC":
                raise ValueError("bad frame header at stream offset (truncated or corrupt input)")
            sid, n = struct.unpack("<BI", hdr[2:])
            payload = stream.read(n)
            if len(payload) < n:
                raise ValueError("truncated payload")
            if sid == 2:
                metas.append(json.loads(payload))
                continue
            if sid not in files:
                files[sid] = open(os.path.join(out_dir, prefix + NAMES[sid]), "wb")
            files[sid].write(payload)
            counts[sid] += n
    finally:
        for f in files.values():
            f.close()
        with open(os.path.join(out_dir, prefix + "meta.jsonl"), "w") as f:
            for m in metas:
                f.write(json.dumps(m) + "\n")
    return counts, metas


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("input", help="captured stream file, or - for stdin")
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--prefix", default="")
    a = ap.parse_args()
    if a.input == "-":
        counts, metas = demux(sys.stdin.buffer, a.out_dir, a.prefix)
    else:
        with open(a.input, "rb") as src:
            counts, metas = demux(src, a.out_dir, a.prefix)
    opened = next((m for m in metas if m.get("kind") == "open"), {})
    end = next((m for m in metas if m.get("kind") == "end"), None)
    xruns = sum(1 for m in metas if m.get("kind") == "xrun")
    print(json.dumps({"bytes": counts, "link_rc": opened.get("link_rc"), "xrun_records": xruns,
                      "complete": end is not None, "end": end}))


if __name__ == "__main__":
    main()
