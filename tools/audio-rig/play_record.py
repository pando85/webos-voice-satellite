#!/usr/bin/env python3
"""Play a stimulus on a named sink while recording named sources simultaneously.

Uses paplay/parecord (work on PipeWire via pipewire-pulse). Device names are
always passed explicitly to the tools: default devices and system/sink volume
are never changed. Recordings are written at each source's native rate and
channel count, with a JSON sidecar (device, rate, channels, timestamps).

Example:
  play_record.py --stimulus sweep --sink SINK --source webcam --source usb
Aliases: speaker, webcam, usb, monitor (see discover.py) or an exact/unique-substring name.
Output (gitignored): artifacts/audio-rig/recordings/<session>/<label>__<kind>.wav + .json
"""
import argparse
import datetime
import json
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf

import discover

ROOT = Path(__file__).resolve().parents[2]
STIM_DIR = ROOT / "artifacts" / "audio-rig" / "stimuli"
REC_DIR = ROOT / "artifacts" / "audio-rig" / "recordings"
FMT_OK = {"s16le", "s32le", "float32le", "s24le", "u8"}


def find_stimulus(arg, rate_label):
    p = Path(arg)
    if p.exists():
        return p
    cand = STIM_DIR / f"{arg}_{rate_label}.wav"
    if cand.exists():
        return cand
    sys.exit(f"stimulus not found: {arg} (looked for {cand}); run make_stimuli.py")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--stimulus", required=True, help="WAV path, or name such as sweep/pilots/noise/speech_full/sequence")
    ap.add_argument("--stim-rate", choices=["16k", "48k"], default="48k")
    ap.add_argument("--sink", required=True, help="sink name/alias; no default device is used")
    ap.add_argument("--source", action="append", required=True, help="source name/alias (repeatable)")
    ap.add_argument("--lead-in", type=float, default=2.0,
                    help="silence (s) at the start of the played stream, before the stimulus; also absorbs "
                         "Bluetooth/sink wake-up losses")
    ap.add_argument("--tail", type=float, default=2.5,
                    help="recording time after playback returns (s); keep >= 2 s for Bluetooth latency")
    ap.add_argument("--no-prewake", action="store_true",
                    help="skip the silent pre-roll that wakes a suspended sink before recording starts")
    ap.add_argument("--gain-db", type=float, default=0.0,
                    help="playback level: the stimulus is rescaled so its PEAK is -30 dBFS + gain_db "
                         "(default 0 -> -30 dBFS peak; max +24 -> -6 dBFS). The sink/system volume is "
                         "never touched and the paplay stream volume stays at 100%%.")
    ap.add_argument("--no-monitor", action="store_true",
                    help="do not also record the sink monitor (digital reference channel)")
    ap.add_argument("--allow-hdmi-sink", action="store_true",
                    help="permit an HDMI/DP sink (may be a TV/monitor, not a speaker)")
    ap.add_argument("--label", default="")
    ap.add_argument("--out-dir", type=Path, default=REC_DIR)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    if not -60 <= args.gain_db <= 24:
        sys.exit("--gain-db must be within [-60, +24] (peak -90..-6 dBFS)")
    for tool in ("paplay", "parecord", "pactl"):
        if not shutil.which(tool):
            sys.exit(f"{tool} not found")

    rep = discover.discover()
    sink_name = discover.resolve(args.sink, rep["sinks"])
    sink = next(s for s in rep["sinks"] if s["name"] == sink_name)
    sources = [discover.resolve(s, rep["sources"]) for s in args.source]
    if not args.no_monitor and sink["monitor_source"] not in sources:
        sources.append(sink["monitor_source"])
    src_info = {s["name"]: s for s in rep["sources"]}

    if sink["kind"] == "hdmi" and not args.allow_hdmi_sink:
        sys.exit(f"refusing: sink {sink_name} is HDMI/DisplayPort (may be a TV or monitor, not a speaker). "
                 "Pass --allow-hdmi-sink only after confirming where the sound will come out.")
    if sink["mute"]:
        sys.exit("sink is muted; unmute it yourself (this tool never changes mute/volume)")

    stim = find_stimulus(args.stimulus, args.stim_rate)
    info = sf.info(stim)
    stim_dur = info.frames / info.samplerate
    session = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    outdir = args.out_dir / session
    label = args.label or stim.stem
    plan = {"stimulus": str(stim), "stimulus_duration_s": round(stim_dur, 3), "sink": sink_name,
            "sources": sources, "lead_in_s": args.lead_in, "tail_s": args.tail,
            "gain_db": args.gain_db, "playback_peak_dbfs": -30 + args.gain_db,
            "sink_volume_percent": sink["volume_percent"]}
    print(json.dumps(plan, indent=2))
    if args.dry_run:
        return 0
    outdir.mkdir(parents=True, exist_ok=True)
    data, sr = sf.read(str(stim), dtype="float64", always_2d=True)
    scale = 10 ** ((-30 + args.gain_db) / 20) / max(float(abs(data).max()), 1e-9)
    play_wav = outdir / f"{label}__played.wav"  # gitignored copy at the requested level
    pad = lambda sec: np.zeros((int(sec * sr), data.shape[1]))
    # silence is part of the played stream so the sink stays active and any wake-up loss hits silence
    sf.write(str(play_wav), np.concatenate([pad(args.lead_in), data * scale, pad(0.5)]), sr, subtype="PCM_16")

    if not args.no_prewake:  # resume a suspended (e.g. Bluetooth) sink before the recorders start
        wake_wav = outdir / f"{label}__wake.wav"
        sf.write(str(wake_wav), np.zeros((int(0.4 * sr), data.shape[1])), sr, subtype="PCM_16")
        subprocess.run(["paplay", f"--device={sink_name}", "--volume=65536", str(wake_wav)], check=True)
        for _ in range(30):
            state = next(x for x in discover.pactl_json("sinks") if x["name"] == sink_name)["state"]
            if state != "SUSPENDED":
                break
            time.sleep(0.1)
        else:
            print("WARNING: sink still SUSPENDED after pre-wake", file=sys.stderr)
        time.sleep(0.3)

    recs = []
    for name in sources:
        si = src_info[name]
        fmt = si["format"] if si["format"] in FMT_OK else "s16le"
        wav = outdir / f"{label}__{si['kind']}.wav"
        # small --latency-msec: the default fragment is ~1 s and the unflushed tail is lost on SIGINT
        cmd = ["parecord", "--latency-msec=20", f"--device={name}", f"--rate={si['rate']}", f"--channels={si['channels']}",
               f"--format={fmt}", "--file-format=wav", str(wav)]
        recs.append({"name": name, "info": si, "wav": wav, "cmd": cmd})

    for r in recs:
        r["t_launch"] = time.time()
        r["proc"] = subprocess.Popen(r["cmd"], stderr=subprocess.PIPE)
    deadline = time.monotonic() + 5  # wait until every recorder is really capturing
    while time.monotonic() < deadline and not all(r["wav"].exists() and r["wav"].stat().st_size > 2048 for r in recs):
        time.sleep(0.05)
    time.sleep(0.2)
    play_cmd = ["paplay", f"--device={sink_name}", "--volume=65536", str(play_wav)]
    t_play = time.time()
    subprocess.run(play_cmd, check=True)
    t_play_end = time.time()
    time.sleep(args.tail)
    t_stop = time.time()
    for r in recs:
        r["proc"].send_signal(signal.SIGINT)
    for r in recs:
        r["proc"].wait(timeout=10)

    for r in recs:
        si = r["info"]
        meta = {"recording": r["wav"].name, "source": r["name"], "source_kind": si["kind"],
                "description": si["description"], "rate": si["rate"], "channels": si["channels"],
                "format": si["format"], "stimulus": stim.name, "stimulus_duration_s": stim_dur,
                "sink": sink_name, "gain_db": args.gain_db,
                "playback_peak_dbfs": -30 + args.gain_db, "sink_volume_percent": sink["volume_percent"],
                "recorder_launch_unix": r["t_launch"], "player_start_unix": t_play,
                "player_end_unix": t_play_end, "nominal_lead_in_s": args.lead_in,
                "launch_to_play_s": t_play - r["t_launch"], "tail_s": args.tail,
                "expected_start_s": t_play - r["t_launch"] + args.lead_in,  # stimulus start excl. sink/BT latency
                "recording_expected_min_s": t_stop - r["t_launch"] - 0.5,
                "lead_in_s": max(args.lead_in - 0.3, 0.2),  # conservative quiet window for noise floor
                "note": "recorder launch != first captured sample; use the monitor recording as timing reference"}
        got = sf.info(str(r["wav"])).duration
        meta["recording_duration_s"] = got
        r["wav"].with_suffix(".json").write_text(json.dumps(meta, indent=2) + "\n")
        flag = "" if got >= meta["recording_expected_min_s"] else (
            f"  WARNING: TRUNCATED (expected >= {meta['recording_expected_min_s']:.2f} s)")
        print(f"recorded {r['wav']} ({si['kind']}, {si['rate']} Hz, {si['channels']} ch, {got:.2f} s){flag}")
    (outdir / f"{label}__session.json").write_text(json.dumps(plan, indent=2) + "\n")
    print(f"session dir: {outdir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
