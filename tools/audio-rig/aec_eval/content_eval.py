#!/usr/bin/env python3
"""Content-only runs of the front-end variants: false-wake proxy and in-band ERLE per content segment.

For each `tv_dualcap.py` capture (no near-end speech: TV content or a quiet room) and each
variant, the whole recording runs through the daemon's front-end (`aec_offline`, the daemon's
250 ms startup skip) once at the variant's gain (the stream HA would receive) and once at 0 dB
(for ERLE). Then:

- **False-wake proxy:** `oww_wyoming.WyomingOww` over the gained stream as one long wake word
  run; after a detection a new run starts `--rearm-s` later (HA ends the wake stage on a
  detection). Every detection is a false wake (there is no "alexa" in the content). Reported
  with the armed time and an exact Poisson 95 % interval (rule of three for zero).
- **ERLE** (200-4000 Hz, reference-active frames, `real_eval.erle`) per content segment of
  the capture's manifest entry (`aec/manifest.json` of P2 session 2), against the same chain
  with the canceller off.

    content_eval.py --tool target/release/examples/aec_offline --work-dir <private dir> \
        --manifest devices/private/c3-a/P2/session2/aec/manifest.json \
        --variant noaec=--no-aec --variant aec= [...] CAPDIR [...] [--json out.jsonl]

Recordings and outputs stay in the gitignored private tree; only numbers are printed.
"""
import argparse
import json
import os
import shlex
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "analysis"))

from semisyn_ab import latency_of, split_gain  # noqa: E402

FS = 16000
SKIP_MS = 250


def ref_mono16(capdir):
    import tvcap
    cap = tvcap.load(capdir)
    return tvcap.resample(tvcap.ref_mono(cap), cap["fs_ref"], FS)[SKIP_MS * FS // 1000:]


def read(path):
    from scipy.io import wavfile
    fs, x = wavfile.read(path)
    assert fs == FS
    return x


def segments_for(manifest, capdir):
    name = Path(capdir).name
    for r in manifest.get("recordings", []):
        if Path(r["dir"]).name == name:
            return (r.get("content") or {}).get("segments", [])
    return []


def run_tool_cap(tool_path, capdir, out, flags):
    """aec_offline on a whole capture with the daemon's startup skip."""
    import subprocess
    cmd = [tool_path, "--mic", os.path.join(capdir, "mic.raw"), "--ref", os.path.join(capdir, "ref.raw"),
           "--out", out, "--ref-format", "s32_le", "--ref-channels", "2", "--ref-rate", "48000",
           "--skip-ms", str(SKIP_MS)] + shlex.split(flags)
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}: {r.stderr.strip()}")
    return json.loads(r.stdout)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--tool", default="target/release/examples/aec_offline")
    ap.add_argument("--work-dir", required=True)
    ap.add_argument("--manifest", help="aec/manifest.json with the content segments")
    ap.add_argument("--variant", action="append", default=[], help="LABEL=FLAGS (aec_offline flags)")
    ap.add_argument("--rearm-s", type=float, default=2.0)
    ap.add_argument("--threshold", type=float, default=0.5)
    ap.add_argument("--json", help="append one JSON line per capture and variant")
    ap.add_argument("caps", nargs="+")
    a = ap.parse_args()
    from oww_wyoming import WyomingOww
    from p2_score import poisson_rate_ci
    from real_eval import erle
    os.makedirs(a.work_dir, exist_ok=True)
    det = WyomingOww(threshold=a.threshold)
    manifest = json.load(open(a.manifest)) if a.manifest else {}
    variants = [v.split("=", 1) for v in a.variant] or [["noaec", "--no-aec"], ["aec", ""]]
    fo = open(a.json, "a") if a.json else None
    for capdir in a.caps:
        name = Path(capdir).name
        ref = ref_mono16(capdir)
        base = os.path.join(a.work_dir, f"{name}__base.wav")
        run_tool_cap(a.tool, capdir, base, "--no-aec --gain-db 0")
        mic = read(base).astype(np.float64) / 32768
        segs = segments_for(manifest, capdir)
        print(f"\n## {name}")
        for label, flags in variants:
            fl, g = split_gain(flags)
            lat = latency_of(fl)
            o0 = os.path.join(a.work_dir, f"{name}__{label}__0.wav")
            og = os.path.join(a.work_dir, f"{name}__{label}__g.wav")
            run_tool_cap(a.tool, capdir, o0, fl + " --gain-db 0")
            m = run_tool_cap(a.tool, capdir, og, fl + f" --gain-db {g}")
            y0 = read(o0).astype(np.float64) / 32768
            yg = read(og)
            n = min(len(mic), len(ref), len(y0))
            # Align the outputs to the input timeline (the canceller's latency).
            y0a = np.concatenate([y0[lat:], np.zeros(lat)])[:n]
            dets, mx, armed = det.run_rearm(yg[lat:], a.rearm_s)
            lo, hi = poisson_rate_ci(len(dets), armed / 3600)
            per = []
            for s in segs:
                s0 = max(0, int((s["start_s"] - SKIP_MS / 1000) * FS))
                s1 = min(n, int((s["end_s"] - SKIP_MS / 1000) * FS))
                if s1 - s0 < FS:
                    continue
                if "--no-aec" in shlex.split(fl):
                    e = 0.0
                else:
                    e, _ = erle(mic[:n], y0a, ref[:n], s0, s1)
                fw = sum(1 for d in dets if s0 / FS <= d["t"] < s1 / FS)
                per.append({"category": s["category"], "clip": s["clip"], "start_s": s["start_s"],
                            "erle_db": round(float(e), 1), "false_wakes": fw})
            row = {"capture": name, "variant": label, "flags": flags, "gain_db": g, "armed_s": round(armed, 1),
                   "false_wakes": len(dets), "detections": dets, "rate_per_h": round(len(dets) / (armed / 3600), 2),
                   "ci_per_h": [round(lo, 2), round(hi, 2)], "max_score": round(mx, 3),
                   "clipped": m.get("clipped"), "segments": per,
                   "delay_ms": (m.get("metrics") or {}).get("delay_ms")}
            seg_txt = " ".join(f"{p['category'][:8]}:{p['erle_db']:.1f}" for p in per)
            print(f"{label:14s} false wakes {len(dets)} in {armed / 60:.1f} min "
                  f"({row['rate_per_h']}/h [{lo:.1f}, {hi:.1f}]) max {mx:.2f} clipped {m.get('clipped')} | ERLE {seg_txt}")
            if fo:
                fo.write(json.dumps(row) + "\n")
                fo.flush()
            for p in (o0, og):
                os.remove(p)
        os.remove(base)
    return 0


if __name__ == "__main__":
    sys.exit(main())
