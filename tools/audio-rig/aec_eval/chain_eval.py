#!/usr/bin/env python3
"""Run the satellite's integrated front-end (HPF -> sonora AEC3 -> gain) on REAL mic +
reference pairs and report in-band ERLE, the delay lock and the CPU per frame.

Uses the `aec_offline` example of the capture crate, which reads the pair exactly as the
daemon's file mode does (FilePair, 256-frame reads, reference pushed before each mic read)
and runs webos_dsp::Frontend with the EchoCanceller. ERLE is computed as in real_eval.py
(200-4000 Hz, frames where the reference is active, output lag compensated) so the numbers
compare with docs/research/aec-candidates.md. The "mic" side of the ratio is the same chain
with the canceller off (high-pass filtered mic at 0 dB gain).

Device recordings stay in the gitignored devices/private/ tree; only numbers are printed.
Processed WAVs go to --work-dir (also private; never commit them). With --keep-wavs the
16 kHz mono S16 output of each variant at --gain-db (default 24, what HA receives) is kept
for offline wake word (openWakeWord) and STT scoring.

usage:
  chain_eval.py --tool target/release/examples/aec_offline --work-dir DIR \
      [--variant LABEL=FLAGS ...] [--keep-wavs] CAPDIR:START_S:END_S [...]

A CAPDIR holds mic.raw (S24_LE 2 ch 16 kHz) and ref.raw plus meta.jsonl from
tv_dualcap.py; the reference format, channels and rate come from meta.jsonl.
FLAGS are aec_offline flags separated by spaces, e.g. 'default=' or 'ns=--ns'. The defaults of
aec_offline are the daemon's [aec] defaults (delay hint, no NS, no bypass).
"""
import argparse
import json
import os
import shlex
import subprocess
import sys

import numpy as np
from scipy.io import wavfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "analysis"))
sys.path.insert(0, HERE)
import tvcap  # noqa: E402
from real_eval import erle  # noqa: E402

FS = 16000
SKIP_MS = 250  # the daemon's mic.skip_ms; aec_offline skips both streams by this much

DEFAULT_VARIANTS = [
    "default=",
    "aec3-only=--apply none",
    "predelay=--apply predelay",
    "bypass=--bypass",
    "ns=--ns",
    "ref+20dB=--ref-gain-db 20",
]


def ref_params(capdir):
    opened = next(m for m in tvcap.read_meta(capdir) if m.get("kind") == "open")
    p = next(p for p in opened["pcms"] if p["sid"] == 1)
    fmt = {"S32_LE": "s32_le", "S24_LE": "s24_le"}[p["format"]]
    return fmt, p["channels"], p["rate"]


def run_tool(tool, capdir, out, flags):
    fmt, ch, rate = ref_params(capdir)
    cmd = [tool, "--mic", os.path.join(capdir, "mic.raw"), "--ref", os.path.join(capdir, "ref.raw"),
           "--out", out, "--ref-format", fmt, "--ref-channels", str(ch), "--ref-rate", str(rate),
           "--skip-ms", str(SKIP_MS)] + flags
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}: {r.stderr.strip()}")
    return json.loads(r.stdout)


def read_wav(path):
    fs, x = wavfile.read(path)
    assert fs == FS
    return x.astype(np.float64) / 32768.0


def fmt(v, spec=".1f"):
    return "n/a" if v is None or (isinstance(v, float) and np.isnan(v)) else format(v, spec)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--tool", required=True, help="path to the aec_offline example binary")
    ap.add_argument("--work-dir", required=True)
    ap.add_argument("--variant", action="append", help="LABEL=FLAGS (repeatable); default: a fixed set")
    ap.add_argument("--converge-s", type=float, default=2.0, help="ERLE also reported after skipping this much")
    ap.add_argument("--keep-wavs", action="store_true", help="keep the gain-applied outputs (HA stream)")
    ap.add_argument("--gain-db", type=float, default=24.0, help="front-end gain for the kept WAVs")
    ap.add_argument("caps", nargs="+", help="CAPDIR:START_S:END_S")
    a = ap.parse_args()
    os.makedirs(a.work_dir, exist_ok=True)
    variants = [v.split("=", 1) for v in (a.variant or DEFAULT_VARIANTS)]
    for spec in a.caps:
        d, s, e = spec.rsplit(":", 2)
        s, e = float(s), float(e)
        name = os.path.basename(d.rstrip("/"))
        cap = tvcap.load(d)
        ref = tvcap.resample(tvcap.ref_mono(cap), cap["fs_ref"], FS)[SKIP_MS * FS // 1000:]
        base = os.path.join(a.work_dir, f"{name}_noaec.wav")
        run_tool(a.tool, d, base, ["--no-aec", "--gain-db", "0"])
        mic = read_wav(base)
        n = min(len(mic), len(ref))
        mic, ref = mic[:n], ref[:n]
        s0 = max(0, int((s - SKIP_MS / 1000) * FS))
        s1 = min(int((e - SKIP_MS / 1000) * FS), n)
        mic_db = 10 * np.log10(np.mean(mic[s0:s1] ** 2) + 1e-30)
        ref_db = 10 * np.log10(np.mean(ref[s0:s1] ** 2) + 1e-30)
        print(f"\n## {name} segment {s:.1f}-{e:.1f} s (file time): HPF mic {mic_db:.1f} dBFS, "
              f"reference {ref_db:.1f} dBFS (16 kHz mono), clipped at +24 dB: see 'clip@gain'")
        print(f"| variant | ERLE 200-4000 Hz (dB) | ERLE after {a.converge_s:.0f} s | first 1 s | delay lock (t s: ms, conf) "
              f"| sonora us/frame mean / max | CPU % (host) | bypassed frames | clip@gain |")
        print("|---|---|---|---|---|---|---|---|---|")
        for label, flags in variants:
            out = os.path.join(a.work_dir, f"{name}_{label}.wav")
            fl = shlex.split(flags)
            res = run_tool(a.tool, d, out, fl + ["--gain-db", "0"])
            y = read_wav(out)[:n]
            if len(y) < n:
                y = np.concatenate([y, np.zeros(n - len(y))])
            e_all, _ = erle(mic, y, ref, s0, s1)
            e_conv, _ = erle(mic, y, ref, min(s0 + int(a.converge_s * FS), s1 - FS), s1)
            e_first, _ = erle(mic, y, ref, s0, min(s0 + FS, s1))
            lock = next((ev for ev in res["events"] if ev["event"] == "locked"), None)
            lock_s = f"{lock['t'] + SKIP_MS / 1000:.2f}: {lock['delay_ms']:.2f}, {lock['confidence']:.0f}" if lock else "none"
            m = res["metrics"] or {}
            kept = os.path.join(a.work_dir, f"{name}_{label}_g{a.gain_db:g}.wav")
            r2 = run_tool(a.tool, d, kept, fl + ["--gain-db", str(a.gain_db)])
            clip = str(r2["clipped"])
            if not a.keep_wavs:
                os.remove(kept)
            print(f"| {label} | {e_all:.1f} | {e_conv:.1f} | {e_first:.1f} | {lock_s} | "
                  f"{fmt(m.get('cpu_us_mean'))} / {fmt(m.get('cpu_us_max'))} | {fmt(m.get('cpu_pct'), '.2f')} | "
                  f"{m.get('bypassed', 'n/a')} | {clip} |")
            os.remove(out)
        kept = os.path.join(a.work_dir, f"{name}_noaec_g{a.gain_db:g}.wav")
        r0 = run_tool(a.tool, d, kept, ["--no-aec", "--gain-db", str(a.gain_db)])
        if not a.keep_wavs:
            os.remove(kept)
        print(f"| (no AEC) | 0 | | | | | | | {r0['clipped']} |")
        os.remove(base)


if __name__ == "__main__":
    main()
