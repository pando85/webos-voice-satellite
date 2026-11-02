#!/usr/bin/env python3
"""Offline A/B of the satellite's front-end variants on semi-synthetic near-end speech.

Each corpus item is mixed into the RAW mic of a real TV recording (`tv_dualcap.py` capture:
mic.raw S24_LE 2 ch 16 kHz, ref.raw SoundEngineCapture S32_LE 2 ch 48 kHz, linked) exactly as
P2 session 2 mixed it for Home Assistant (`p2_digital.py --bed`: same bed offset per item, 2 s
lead, 3 s tail, the item at `speech_gain_db` re full scale, added to the mic before any
processing). The reference is left untouched: it holds only the TV program, as in reality.

Every variant then runs the daemon's exact front-end through the `aec_offline` example of the
capture crate (HPF, optional canceller, gain, S16 saturation) on the mic + reference segment
starting `--warmup-s` before the item, so the canceller has converged as it would have during
continuous TV playback. The output is cut to the item's segment (the canceller's latency is
compensated) and scored with `oww_wyoming.WyomingOww` (HA's Wyoming openWakeWord 2.1.0 loop,
fresh state per item, as HA's pipeline starts a new wake word stream per item).

Special variants (not aec_offline flags):
- `@py`: session 2's own Python emulation of the front-end on the same mix (no aec_offline),
  for calibration against session 2's HA results;
- `@digital`: the item with zero lead and tail, no bed and no front-end (the digital baseline);
- `@oracle FLAGS`: diagnostic. The variant FLAGS run on the bed WITHOUT the speech (at 0 dB
  gain), and the high-passed speech is added to that output afterwards, then the gain
  (`--gain-db` in FLAGS, default 24) and S16 saturation. The speech never passes the
  canceller, so this shows the residual echo's effect alone, without near-end distortion.

    semisyn_ab.py --corpus devices/private/corpus/p2-v1 \
        --session-rows devices/private/c3-a/P2/session2/semisyn/content-v10.jsonl \
        --capdir devices/private/c3-a/P2/session2/aec/content-v10 \
        --tool target/release/examples/aec_offline --work-dir <private dir> --out res.jsonl \
        --variant noaec=--no-aec --variant aec= --variant aec+30=--gain-db 30 [--keep-wavs] [--jobs 8]

`--session-rows` is a `p2_digital.py` result file: it fixes the items, their order, the bed
offset and the speech gain (and supplies HA's detections for the calibration columns).
Outputs one JSON line per item and variant: detections (seconds in the item timeline, as in
session 2), max score, HA's detections, clipping and canceller metrics. Audio stays in the
private work dir (`--keep-wavs` keeps the cut 16 kHz outputs for STT); never commit it.
"""
import argparse
import json
import os
import shlex
import subprocess
import sys
import tempfile
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

RATE = 16000
REF_RATE = 48000
LEAD_S = 2.0
TAIL_S = 3.0
BED_SKIP_S = 1.0
# Output samples by which the front-end delays the mic (sonora's latency, see crates/dsp).
LATENCY = {"noaec": 0, "aec": 128, "ns": 224, "linear": 64, "linear+ns": 160}


def bed_offset(index, bed_len, item_len, lead=LEAD_S, tail=TAIL_S, skip_s=BED_SKIP_S):
    """p2_digital.mix_on_bed's deterministic bed offset (samples) for the 1-based index."""
    n = int((lead + tail) * RATE) + item_len
    usable = bed_len - int(skip_s * RATE) - n
    if usable <= 0:
        raise ValueError("bed too short")
    return int(skip_s * RATE) + (index * 7919 * RATE // 1000) % usable


def s24(raw):
    v = raw & 0xFFFFFF
    return np.where(v & 0x800000, v - 0x1000000, v).astype(np.int32)


def speech_s24(item16, gain_db):
    """Item samples on the S24 scale at gain_db re full scale (float)."""
    return item16.astype(np.float64) / 32768 * 10 ** (gain_db / 20) * 2 ** 23


def parse_variant(spec):
    label, _, flags = spec.partition("=")
    return label, flags.strip()


def latency_of(flags):
    f = shlex.split(flags)
    if "--no-aec" in f:
        return LATENCY["noaec"]
    lin = "--output" in f and f[f.index("--output") + 1] == "linear"  # nearend: as full
    key = ("linear" if lin else "aec") if "--ns" not in f else ("linear+ns" if lin else "ns")
    return LATENCY[key]


class Bed:
    """Memory-mapped capture: mic (frames x 2 int32) and reference (frames x 2 int32)."""

    def __init__(self, capdir):
        self.capdir = Path(capdir)
        mic = np.memmap(self.capdir / "mic.raw", dtype="<i4", mode="r")
        self.mic = mic[: len(mic) // 2 * 2].reshape(-1, 2)
        ref = np.memmap(self.capdir / "ref.raw", dtype="<i4", mode="r")
        self.ref = ref[: len(ref) // 2 * 2].reshape(-1, 2)

    def __len__(self):
        return len(self.mic)


def build_segment(bed, off, n, warmup, sp24, lead=LEAD_S):
    """Mic (int32 S24, 2 ch, speech added to both channels) and reference for
    [off - warmup, off + n). Returns (mic, ref, pre) with pre the warm-up actually used."""
    start = max(0, off - warmup)
    pre = off - start
    end = off + n
    mic = s24(np.asarray(bed.mic[start:end], dtype=np.int64)).astype(np.float64)
    a = pre + int(lead * RATE)
    mic[a:a + len(sp24), :] += sp24[:, None]
    mic = np.clip(np.round(mic), -(2 ** 23), 2 ** 23 - 1).astype("<i4")
    ref = np.asarray(bed.ref[3 * start:3 * end], dtype="<i4")
    return mic, ref, pre


def hpf(x, hz=120.0):
    """The front-end's 2nd-order Butterworth high-pass (oww_offline.frontend's filter)."""
    import math

    from scipy.signal import lfilter
    w0 = 2 * math.pi * hz / RATE
    alpha = math.sin(w0) / (2 / math.sqrt(2))
    c = math.cos(w0)
    b = np.array([(1 + c) / 2, -(1 + c), (1 + c) / 2])
    a = np.array([1 + alpha, -2 * c, 1 - alpha])
    return lfilter(b / a[0], a / a[0], x)


def split_gain(flags):
    """(flags without --gain-db, gain in dB)."""
    f = shlex.split(flags)
    g = 24.0
    if "--gain-db" in f:
        i = f.index("--gain-db")
        g = float(f[i + 1])
        del f[i:i + 2]
    return " ".join(shlex.quote(x) for x in f), g


def run_tool(tool, mic_path, ref_path, out_path, flags):
    cmd = [tool, "--mic", mic_path, "--ref", ref_path, "--out", out_path, "--skip-ms", "0",
           "--ref-format", "s32_le", "--ref-channels", "2", "--ref-rate", str(REF_RATE)] + shlex.split(flags)
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}: {r.stderr.strip()}")
    return json.loads(r.stdout)


def read_wav16(path):
    from scipy.io import wavfile
    fs, x = wavfile.read(path)
    assert fs == RATE
    return x


_DET = None


def _detector(threshold):
    global _DET
    if _DET is None:
        from oww_wyoming import WyomingOww
        _DET = WyomingOww(threshold=threshold)
    return _DET


def process_item(job):
    """One item through every variant. Returns a list of result rows."""
    (a, idx, row, variants) = job
    import soundfile as sf
    from oww_offline import frontend
    det = _detector(a["threshold"])
    it = row["item"]
    x, sr = sf.read(os.path.join(a["corpus"], it["_file"]), dtype="int16")
    assert sr == RATE and x.ndim == 1
    n = int((LEAD_S + TAIL_S) * RATE) + len(x)
    out = []
    bed = Bed(a["capdir"]) if a["capdir"] else None
    gain_db = a["speech_gain_db"] if a.get("speech_gain_db") is not None else row.get("speech_gain_db", -39.0)
    off = None
    if bed is not None:
        off = bed_offset(idx, len(bed), len(x))
        if "bed_offset_s" in row and abs(off / RATE - row["bed_offset_s"]) > 0.001:
            raise RuntimeError(f"{it['id']}: bed offset {off / RATE} != session {row['bed_offset_s']}")
        sp24 = speech_s24(x, gain_db)
        mic, ref, pre = build_segment(bed, off, n, int(a["warmup_s"] * RATE), sp24)
        need_bare = any(f.startswith("@oracle") for _, f in variants)
    tmp = tempfile.mkdtemp(dir=a["work_dir"])
    try:
        if bed is not None:
            mp, rp = os.path.join(tmp, "mic.raw"), os.path.join(tmp, "ref.raw")
            mic.tofile(mp)
            ref.tofile(rp)
            if need_bare:
                bp = os.path.join(tmp, "bare.raw")
                build_segment(bed, off, n, int(a["warmup_s"] * RATE), np.zeros(0))[0].tofile(bp)
        for label, flags in variants:
            metrics = {}
            if flags == "@digital":
                pcm = np.concatenate([np.zeros(int(LEAD_S * RATE), np.int16), x, np.zeros(int(TAIL_S * RATE), np.int16)])
                clipped = 0
            elif flags.startswith("@py"):
                if bed is None:
                    raise RuntimeError("@py needs a bed")
                g = float(flags.split()[1]) if len(flags.split()) > 1 else 24.0
                seg = s24(np.asarray(bed.mic[off:off + n, 0], dtype=np.int64)).astype(np.float64) / 2 ** 23
                spf = x.astype(np.float64) / 32768 * 10 ** (gain_db / 20)
                seg[int(LEAD_S * RATE):int(LEAD_S * RATE) + len(spf)] += spf
                pcm, clipped = frontend(seg * 2 ** 23, g, 120.0)
            elif flags.startswith("@oracle"):
                fl, g = split_gain(flags[len("@oracle"):].strip())
                op = os.path.join(tmp, f"{label}.wav")
                m = run_tool(a["tool"], bp, rp, op, fl + " --gain-db 0")
                y = read_wav16(op).astype(np.float64) / 32768
                lat = latency_of(fl)
                y = y[pre + lat:pre + lat + n]
                if len(y) < n:
                    y = np.concatenate([y, np.zeros(n - len(y))])
                spf = np.zeros(n)
                spf[int(LEAD_S * RATE):int(LEAD_S * RATE) + len(x)] = x.astype(np.float64) / 32768 * 10 ** (gain_db / 20)
                yy = (y + hpf(spf)) * 10 ** (g / 20) * 32768
                clipped = int(np.sum(np.abs(np.round(yy)) > 32767))
                pcm = np.clip(np.round(yy), -32768, 32767).astype(np.int16)
                mm = m.get("metrics") or {}
                metrics = {"erle_db": mm.get("erle_db"), "delay_ms": mm.get("delay_ms")}
            else:
                op = os.path.join(tmp, f"{label}.wav")
                m = run_tool(a["tool"], mp, rp, op, flags)
                y = read_wav16(op)
                lat = latency_of(flags)
                pcm = y[pre + lat:pre + lat + n]
                if len(pcm) < n:
                    pcm = np.concatenate([pcm, np.zeros(n - len(pcm), np.int16)])
                clipped = int(np.sum((pcm == 32767) | (pcm == -32768)))
                mm = m.get("metrics") or {}
                metrics = {"erle_db": mm.get("erle_db"), "delay_ms": mm.get("delay_ms"),
                           "ref_underruns": mm.get("ref_underruns"), "tool_clipped": m.get("clipped")}
            dets, mx = det.run(pcm)
            rel = [round(d["t"] - LEAD_S, 3) for d in dets]
            sp_a, sp_b = int(LEAD_S * RATE), int(LEAD_S * RATE) + len(x)
            f = pcm.astype(np.float64)
            r = {"condition": a["condition"], "variant": label, "flags": flags, "item": it,
                 "detections_s": rel, "max_score": round(mx, 3), "ha_detections_s": row.get("detections_s"),
                 "bed_offset_s": None if off is None else round(off / RATE, 3), "speech_gain_db": gain_db,
                 "warmup_s": None if bed is None else round(pre / RATE, 2), "clipped": clipped,
                 "rms_dbfs_item": round(20 * np.log10(max(np.sqrt(np.mean(f[sp_a:sp_b] ** 2)), 1e-9) / 32768), 1),
                 **metrics}
            if a["keep_wavs"]:
                kd = Path(a["work_dir"]) / "wavs" / f"{a['condition']}@{gain_db:g}" / label
                kd.mkdir(parents=True, exist_ok=True)
                from scipy.io import wavfile
                wavfile.write(kd / f"{it['id']}.wav", RATE, pcm.astype(np.int16))
                r["wav"] = str(kd / f"{it['id']}.wav")
            out.append(r)
    finally:
        for fn in os.listdir(tmp):
            os.unlink(os.path.join(tmp, fn))
        os.rmdir(tmp)
    return out


def load_rows(path, kinds):
    rows = [json.loads(line) for line in open(path)]
    # The 1-based index p2_digital used for the bed offset is the line number in the run.
    return [(i, r) for i, r in enumerate(rows, 1) if not kinds or r["item"]["kind"] in kinds]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--session-rows", required=True, help="p2_digital.py result JSONL of the session run")
    ap.add_argument("--capdir", help="tv_dualcap capture (mic.raw + ref.raw); omit for @digital only")
    ap.add_argument("--condition", help="label (default: the capdir name, or 'digital')")
    ap.add_argument("--tool", default="target/release/examples/aec_offline")
    ap.add_argument("--work-dir", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--variant", action="append", default=[], help="LABEL=FLAGS (aec_offline flags, @py, @digital)")
    ap.add_argument("--warmup-s", type=float, default=10.0)
    ap.add_argument("--kind", action="append", default=["pos"], help="item kinds (default pos)")
    ap.add_argument("--id", action="append", help="only these item ids")
    ap.add_argument("--threshold", type=float, default=0.5)
    ap.add_argument("--speech-gain-db", type=float,
                    help="override the session rows' speech gain (dB re full scale; -39 is the rig reference)")
    ap.add_argument("--keep-wavs", action="store_true")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    a = ap.parse_args()
    os.makedirs(a.work_dir, exist_ok=True)
    variants = [parse_variant(v) for v in a.variant] or [("aec", "")]
    rows = load_rows(a.session_rows, set(a.kind))
    if a.id:
        rows = [(i, r) for i, r in rows if r["item"]["id"] in a.id]
    manifest = {}
    import csv
    for m in csv.DictReader(open(Path(a.corpus) / "manifest.csv", newline="")):
        manifest[m["id"]] = m["file_16k_mono"]
    cond = a.condition or (Path(a.capdir).name if a.capdir else "digital")
    cfg = {"corpus": a.corpus, "capdir": a.capdir, "condition": cond, "tool": os.path.abspath(a.tool),
           "work_dir": os.path.abspath(a.work_dir), "warmup_s": a.warmup_s, "threshold": a.threshold,
           "keep_wavs": a.keep_wavs, "speech_gain_db": a.speech_gain_db}
    jobs = []
    for i, r in rows:
        r = dict(r)
        r["item"] = dict(r["item"], _file=manifest[r["item"]["id"]])
        jobs.append((cfg, i, r, variants))
    with open(a.out, "a", buffering=1) as fo, ProcessPoolExecutor(max_workers=a.jobs) as ex:
        for res in ex.map(process_item, jobs):
            for r in res:
                fo.write(json.dumps(r, ensure_ascii=False) + "\n")
            print(f"{res[0]['item']['id']}: " + " ".join(f"{r['variant']}={len(r['detections_s'])}" for r in res),
                  file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
