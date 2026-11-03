#!/usr/bin/env python3
"""AEC tuning and wake-threshold evaluation on semi-synthetic near-end speech (P2 AEC tuning).

Extends `semisyn_ab.py` (same mixing, same `aec_offline` front-end, same Wyoming openWakeWord
2.1.0 replica) for parameter searches and DET curves:

- `items`: corpus items mixed into a bed (quiet room or TV content) at a talker level, run
  through every variant, scored as a probability **trace** (`WyomingOww.trace`), so detections
  for any threshold >= 0.1 and trigger level are computed afterwards without re-running
  (`oww_wyoming.detect_from_trace`). Rows are compatible with `ab_report.py` (`detections_s`
  at `--threshold`/`--trigger-level`) and with `stt_eval.py` (`--keep-wavs`).
- `content`: whole content-only recordings (no talker) per variant: in-band ERLE (200-4000 Hz,
  reference-active frames, per content segment and over the whole recording, against the same
  chain without canceller) and, with `--wake`, false wakes as consecutive Wyoming streams
  (a new stream `--rearm-s` after each detection) for every threshold and trigger level asked.

Items are selected by kind and voice: `--split tune` / `--split held` are the fixed voice
splits of the P2 tuning experiment (`SPLITS`), so parameters are chosen on some voices and
reported on others.

    tune_eval.py items --corpus devices/private/corpus/p2-v1 --bed quiet=CAPDIR:ROWS \
        --bed content-v10=CAPDIR:ROWS --gain -39 --gain -29 --split tune \
        --variant aec= --variant "mask4=--aec3 suppressor.normal_tuning.mask_lf.enr_suppress=1.6" \
        --work-dir <private> --out res.jsonl
    tune_eval.py content --manifest .../aec/manifest.json --variant aec= --wake \
        --threshold 0.3 --threshold 0.5 --trigger-level 1 --trigger-level 2 CAPDIR [...] --out c.jsonl

Recordings, audio and result files stay in the gitignored private tree.
"""
import argparse
import csv
import itertools
import json
import os
import shlex
import sys
import tempfile
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "analysis"))

from oww_wyoming import detect_from_trace  # noqa: E402
from semisyn_ab import (LEAD_S, RATE, TAIL_S, Bed, bed_offset, build_segment, latency_of,  # noqa: E402
                        parse_variant, read_wav16, run_tool, speech_s24, split_gain)

# Voice splits (speaker key = voice + speaker). The two MLS voices (near 0 % even digitally)
# are held out, so they never steer the tuning; each split has both genders and both corpora.
SPLITS = {
    "tune": {"es_ES-davefx-medium/davefx", "es_ES-sharvard-medium/F", "es_MX-ald-medium/ald",
             "en_US-lessac-medium/lessac"},
    "held": {"es_ES-sharvard-medium/M", "es_MX-claude-high/claude", "en_US-ryan-medium/ryan",
             "es_ES-mls_10246-low/mls_10246", "es_ES-mls_9972-low/mls_9972"},
}
MLS = ("es_ES-mls_9972-low", "es_ES-mls_10246-low")
MIN_PROB = 0.1


def speaker_key(item):
    return f"{item['voice']}/{item.get('speaker') or ''}"


def in_split(item, split):
    return split in (None, "all") or speaker_key(item) in SPLITS[split]


def detections(row, threshold, trigger_level=1, refractory_s=2.0):
    """Detections (s, item timeline) of a trace row for a threshold and trigger level."""
    return [round(t - LEAD_S, 3) for t in detect_from_trace(row["trace"], threshold, trigger_level, refractory_s)]


def tool_flags(flags):
    """aec_offline flags of a variant (drops nothing; @-variants are not supported here)."""
    if flags.startswith("@"):
        raise ValueError("tune_eval supports aec_offline flags only")
    return flags


def process_item(job):
    """One item in one bed at one talker level through every variant."""
    (a, idx, row, bed_name, capdir, gain_db, variants) = job
    import soundfile as sf
    from oww_wyoming import WyomingOww
    global _DET
    try:
        det = _DET
    except NameError:
        det = _DET = WyomingOww()
    it = row["item"]
    x, sr = sf.read(os.path.join(a["corpus"], it["_file"]), dtype="int16")
    assert sr == RATE and x.ndim == 1
    n = int((LEAD_S + TAIL_S) * RATE) + len(x)
    bed = Bed(capdir)
    off = bed_offset(idx, len(bed), len(x))
    if "bed_offset_s" in row and abs(off / RATE - row["bed_offset_s"]) > 0.001:
        raise RuntimeError(f"{it['id']}: bed offset {off / RATE} != session {row['bed_offset_s']}")
    mic, ref, pre = build_segment(bed, off, n, int(a["warmup_s"] * RATE), speech_s24(x, gain_db))
    out = []
    tmp = tempfile.mkdtemp(dir=a["work_dir"])
    cond = f"{bed_name}@{gain_db:g}"
    try:
        mp, rp = os.path.join(tmp, "mic.raw"), os.path.join(tmp, "ref.raw")
        mic.tofile(mp)
        ref.tofile(rp)
        for label, flags in variants:
            op = os.path.join(tmp, "out.wav")
            m = run_tool(a["tool"], mp, rp, op, tool_flags(flags))
            y = read_wav16(op)
            lat = latency_of(flags)
            pcm = y[pre + lat:pre + lat + n]
            if len(pcm) < n:
                pcm = np.concatenate([pcm, np.zeros(n - len(pcm), np.int16)])
            trace, mx = det.trace(pcm, min_prob=MIN_PROB)
            mm = m.get("metrics") or {}
            r = {"condition": bed_name, "cond": cond, "variant": label, "flags": flags, "item": it,
                 "speech_gain_db": gain_db, "bed_offset_s": round(off / RATE, 3), "warmup_s": round(pre / RATE, 2),
                 "trace": trace, "max_score": round(mx, 4),
                 "detections_s": detections({"trace": trace}, a["threshold"], a["trigger_level"]),
                 "clipped": int(np.sum((pcm == 32767) | (pcm == -32768))),
                 "erle_db": mm.get("erle_db"), "cpu_us_mean": mm.get("cpu_us_mean")}
            if a["keep_wavs"]:
                from scipy.io import wavfile
                kd = Path(a["work_dir"]) / "wavs" / cond / label
                kd.mkdir(parents=True, exist_ok=True)
                wavfile.write(kd / f"{it['id']}.wav", RATE, pcm.astype(np.int16))
                r["wav"] = str(kd / f"{it['id']}.wav")
            out.append(r)
    finally:
        for fn in os.listdir(tmp):
            os.unlink(os.path.join(tmp, fn))
        os.rmdir(tmp)
    return out


def load_bed_rows(rows_path, kinds, split, ids=None):
    rows = [json.loads(line) for line in open(rows_path)]
    sel = []
    for i, r in enumerate(rows, 1):
        it = r["item"]
        if kinds and it["kind"] not in kinds:
            continue
        if not in_split(it, split) or (ids and it["id"] not in ids):
            continue
        sel.append((i, r))
    return sel


def cmd_items(a):
    os.makedirs(a.work_dir, exist_ok=True)
    variants = [parse_variant(v) for v in a.variant]
    if a.variants_file:
        variants += [tuple(v) for v in json.load(open(a.variants_file))]
    manifest = {m["id"]: m["file_16k_mono"] for m in csv.DictReader(open(Path(a.corpus) / "manifest.csv", newline=""))}
    cfg = {"corpus": a.corpus, "tool": os.path.abspath(a.tool), "work_dir": os.path.abspath(a.work_dir),
           "warmup_s": a.warmup_s, "keep_wavs": a.keep_wavs, "threshold": a.threshold,
           "trigger_level": a.trigger_level}
    done = set()
    if os.path.exists(a.out):  # resume: skip (condition, item) jobs already complete
        for line in open(a.out):
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            done.add((r["cond"], r["item"]["id"], r["variant"]))
    jobs = []
    for spec in a.bed:
        name, _, rest = spec.partition("=")
        capdir, _, rows_path = rest.rpartition(":")
        for gain in a.gain:
            for i, r in load_bed_rows(rows_path, set(a.kind), a.split, set(a.id or [])):
                r = dict(r)
                r["item"] = dict(r["item"], _file=manifest[r["item"]["id"]])
                if r.get("bed_offset_s") is None:
                    r.pop("bed_offset_s", None)
                cond = f"{name}@{gain:g}"
                if all((cond, r["item"]["id"], v) in done for v, _ in variants):
                    continue
                jobs.append((cfg, i, r, name, capdir, gain, variants))
    with open(a.out, "a", buffering=1) as fo, ProcessPoolExecutor(max_workers=a.jobs) as ex:
        for res in ex.map(process_item, jobs, chunksize=1):
            for r in res:
                fo.write(json.dumps(r, ensure_ascii=False) + "\n")
    return 0


def _content_job(job):
    """One variant on one content capture: ERLE per segment and, optionally, false wakes."""
    (a, capdir, label, flags) = job
    import content_eval
    from real_eval import erle
    name = Path(capdir).name
    wd = tempfile.mkdtemp(dir=a["work_dir"])
    try:
        base = os.path.join(wd, "base.wav")
        content_eval.run_tool_cap(a["tool"], capdir, base, "--no-aec --gain-db 0")
        mic = content_eval.read(base).astype(np.float64) / 32768
        ref = content_eval.ref_mono16(capdir)
        fl, g = split_gain(flags)
        f = shlex.split(fl)
        lat = latency_of(fl)
        noaec = "--no-aec" in f
        agc = any(x.startswith("--agc2") for x in f)
        row = {"capture": name, "variant": label, "flags": flags}
        n = min(len(mic), len(ref))
        if not noaec and not agc:
            o0 = os.path.join(wd, "o0.wav")
            content_eval.run_tool_cap(a["tool"], capdir, o0, fl + " --gain-db 0")
            y0 = content_eval.read(o0).astype(np.float64) / 32768
            y0a = np.concatenate([y0[lat:], np.zeros(lat)])[:n]
            n = min(n, len(y0a))
            segs = content_eval.segments_for(a["manifest"], capdir)
            per = []
            for s in segs:
                s0 = max(0, int((s["start_s"] - content_eval.SKIP_MS / 1000) * RATE))
                s1 = min(n, int((s["end_s"] - content_eval.SKIP_MS / 1000) * RATE))
                if s1 - s0 < RATE:
                    continue
                e, _ = erle(mic[:n], y0a, ref[:n], s0, s1)
                per.append({"category": s["category"], "start_s": s["start_s"], "erle_db": round(float(e), 2)})
            whole = None
            if per:
                whole = float(erle(mic[:n], y0a, ref[:n], 0, n)[0])
            row.update({"segments": per, "erle_mean_db": round(float(np.mean([p["erle_db"] for p in per])), 2)
                        if per else None, "erle_whole_db": None if whole is None else round(whole, 2)})
        if a["wake"]:
            from oww_wyoming import WyomingOww
            og = os.path.join(wd, "og.wav")
            m = content_eval.run_tool_cap(a["tool"], capdir, og, fl + f" --gain-db {g}")
            yg = content_eval.read(og)[lat:]
            row["clipped"] = m.get("clipped")
            fw = {}
            for thr, lvl in itertools.product(a["thresholds"], a["levels"]):
                det = WyomingOww(threshold=thr, trigger_level=lvl)
                dets, mx, armed = det.run_rearm(yg, a["rearm_s"])
                fw[f"{thr:g}/{lvl}"] = {"n": len(dets), "t": [d["t"] for d in dets], "armed_s": round(armed, 1)}
                row["max_score"] = round(mx, 4)
            row["false_wakes"] = fw
        return row
    finally:
        for fn in os.listdir(wd):
            os.unlink(os.path.join(wd, fn))
        os.rmdir(wd)


def cmd_content(a):
    os.makedirs(a.work_dir, exist_ok=True)
    variants = [parse_variant(v) for v in a.variant]
    if a.variants_file:
        variants += [tuple(v) for v in json.load(open(a.variants_file))]
    cfg = {"tool": os.path.abspath(a.tool), "work_dir": os.path.abspath(a.work_dir),
           "manifest": json.load(open(a.manifest)) if a.manifest else {}, "wake": a.wake,
           "thresholds": a.threshold or [0.5], "levels": a.trigger_level or [1], "rearm_s": a.rearm_s}
    jobs = [(cfg, c, label, flags) for c in a.caps for label, flags in variants]
    with open(a.out, "a", buffering=1) as fo, ProcessPoolExecutor(max_workers=a.jobs) as ex:
        for row in ex.map(_content_job, jobs, chunksize=1):
            fo.write(json.dumps(row) + "\n")
            print(f"{row['capture']} {row['variant']}: ERLE {row.get('erle_mean_db')} "
                  f"fw {sum(v['n'] for v in row.get('false_wakes', {}).values())}", file=sys.stderr)
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--tool", default="target/release/examples/aec_offline")
    common.add_argument("--work-dir", required=True)
    common.add_argument("--out", required=True)
    common.add_argument("--variant", action="append", default=[], help="LABEL=FLAGS (aec_offline flags)")
    common.add_argument("--variants-file", help="JSON list of [label, flags]")
    common.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    it = sub.add_parser("items", parents=[common])
    it.add_argument("--corpus", required=True)
    it.add_argument("--bed", action="append", required=True, help="NAME=CAPDIR:ROWS.jsonl (session rows)")
    it.add_argument("--gain", action="append", type=float, required=True, help="talker level, dB re full scale")
    it.add_argument("--kind", action="append", default=[], help="item kinds (default: all in the rows)")
    it.add_argument("--split", choices=["tune", "held", "all"], default="all")
    it.add_argument("--id", action="append")
    it.add_argument("--warmup-s", type=float, default=10.0)
    it.add_argument("--threshold", type=float, default=0.5, help="for the detections_s column")
    it.add_argument("--trigger-level", type=int, default=1)
    it.add_argument("--keep-wavs", action="store_true")
    co = sub.add_parser("content", parents=[common])
    co.add_argument("--manifest")
    co.add_argument("--wake", action="store_true")
    co.add_argument("--threshold", type=float, action="append")
    co.add_argument("--trigger-level", type=int, action="append")
    co.add_argument("--rearm-s", type=float, default=2.0)
    co.add_argument("caps", nargs="+")
    a = ap.parse_args()
    return cmd_items(a) if a.cmd == "items" else cmd_content(a)


if __name__ == "__main__":
    sys.exit(main())
