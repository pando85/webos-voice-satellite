#!/usr/bin/env python3
"""Digital baseline for the P2 corpus: stream each item into Home Assistant's pipeline (no speaker, no mic).

P2-corpus.md asks for this baseline in every report, so that losses of the acoustic path can be
told apart from losses of the model and the voices. Each item (16 kHz mono) is sent over the
WebSocket API as `assist_pipeline/run` from `wake_word` to `stt`, with lead-in and tail silence.
The run therefore never reaches the conversation agent: nothing is executed in HA. The intent
is scored separately with the built-in agent's recognition-only debug endpoint
(`conversation/agent/homeassistant/debug`), for the transcript and for the reference text.

    p2_digital.py --url https://ha.example --token-file tok.json --pipeline "test pipeline" \
        --corpus devices/private/corpus/p2-v1 --out digital.jsonl [--kind pos] [--limit N]

Output: one JSON line per item in the format `p2_score.py` reads (`condition` "digital").

Semi-synthetic conditions (`--bed mic.raw`): each item is added, at `--speech-gain-db` (default
-39 dB: the rig reference level of -34 dBFS active speech times the -27 dB digital-to-mic gain
measured in D3, so about -61 dBFS active RMS at the mic), to a segment of a real TV-mic
recording (`tv_dualcap` mic.raw, S24_LE 2 ch: a quiet room, or TV content playing). The sum
passes through the satellite's front-end emulation (ch0, 120 Hz high-pass, +24 dB, S16
saturation) before it is streamed. Bed segments are chosen deterministically per item. This
injects clean synthetic speech digitally: no room, no speaker; label results accordingly.
"""
import argparse
import asyncio
import csv
import json
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf

import ha_ws

RATE = 16000
INT_FIELDS = ("wake_start_ms", "wake_end_ms", "command_start_ms", "duration_ms")


def load_items(corpus, kinds=None, ids=None):
    rows = list(csv.DictReader(open(Path(corpus) / "manifest.csv", newline="")))
    out = []
    for r in rows:
        if kinds and r["kind"] not in kinds:
            continue
        if ids and r["id"] not in ids:
            continue
        for k in INT_FIELDS:
            r[k] = int(r[k]) if r.get(k) not in (None, "") else None
        out.append(r)
    return out


def stratified(items, per_voice):
    """Up to per_voice positives per voice, alternating `alone` and `cmd`, in id order."""
    by = {}
    for it in items:
        by.setdefault(it["voice"] + "/" + it["speaker"], []).append(it)
    out = []
    for key in sorted(by):
        alone = sorted((i for i in by[key] if i["subkind"] == "alone"), key=lambda i: i["id"])
        cmd = sorted((i for i in by[key] if i["subkind"] == "cmd"), key=lambda i: i["id"])
        pick = []
        while len(pick) < per_voice and (alone or cmd):
            for src in (cmd, alone):
                if src and len(pick) < per_voice:
                    pick.append(src.pop(0))
        out.extend(pick)
    return out


def item_fields(r):
    keep = ("id", "voice", "speaker", "kind", "subkind", "lang", "gender_est", "text", "expected_command_text",
            "expected_intent", "wake_start_ms", "wake_end_ms", "command_start_ms", "duration_ms")
    return {k: r.get(k) for k in keep}


def load_bed(path, channels=2, channel=0):
    raw = np.fromfile(path, dtype="<i4")
    raw = raw[: len(raw) // channels * channels].reshape(-1, channels)
    v = raw[:, channel] & 0xFFFFFF
    v = np.where(v & 0x800000, v - 0x1000000, v)
    return v.astype(np.float64) / 2 ** 23


def mix_on_bed(item16, bed, index, lead, tail, speech_gain_db, skip_s=1.0, gain_db=24.0, hpf_hz=120.0):
    """Item (int16, 16 kHz) at speech_gain_db (dB re full scale) on a bed segment; returns
    (int16 after the front-end emulation, bed offset in seconds, clipped samples)."""
    from oww_offline import frontend
    n = int((lead + tail) * RATE) + len(item16)
    usable = len(bed) - int(skip_s * RATE) - n
    if usable <= 0:
        raise ValueError("bed too short")
    off = int(skip_s * RATE) + (index * 7919 * RATE // 1000) % usable
    seg = bed[off:off + n].copy()
    sp = item16.astype(np.float64) / 32768 * 10 ** (speech_gain_db / 20)
    seg[int(lead * RATE):int(lead * RATE) + len(sp)] += sp
    out, clipped = frontend(seg * 2 ** 23, gain_db, hpf_hz)
    return out, off / RATE, clipped


async def run_item(ha, pipeline_id, pcm, lead_s, speed):
    """Stream one item; return (events, t_lead) where events are the pipeline events."""
    ha._id += 1
    rid = ha._id
    events = []
    done = asyncio.Event()
    stop_audio = asyncio.Event()  # HA closed the audio handler: sending more logs an error in HA
    started = asyncio.get_running_loop().create_future()

    def on_ev(ev):
        events.append(ev)
        if ev.get("type") == "run-start" and not started.done():
            started.set_result(ev["data"]["runner_data"]["stt_binary_handler_id"])
        if ev.get("type") in ("stt-end", "error", "run-end"):
            stop_audio.set()
        if ev.get("type") == "run-end":
            done.set()

    ha._subs[rid] = on_ev
    fut = asyncio.get_running_loop().create_future()
    ha._pending[rid] = fut
    await ha.ws.send_json({"id": rid, "type": "assist_pipeline/run", "start_stage": "wake_word",
                           "end_stage": "stt", "pipeline": pipeline_id, "timeout": 120,
                           "input": {"sample_rate": RATE, "timeout": 60}})
    r = await asyncio.wait_for(fut, 30)
    if not r.get("success"):
        raise RuntimeError(f"run refused: {r.get('error')}")
    hid = await asyncio.wait_for(started, 30)
    prefix = bytes([hid])
    chunk = 1024
    data = pcm.astype("<i2").tobytes()
    step = chunk * 2
    for i in range(0, len(data), step):
        if stop_audio.is_set():
            break
        await ha.ws.send_bytes(prefix + data[i:i + step])
        await asyncio.sleep(chunk / RATE / speed)
    if not stop_audio.is_set():
        await ha.ws.send_bytes(prefix)  # end of audio
    try:
        await asyncio.wait_for(done.wait(), 60)
    except asyncio.TimeoutError:
        events.append({"type": "timeout"})
    ha._subs.pop(rid, None)
    return events


async def main_async(a):
    items = load_items(a.corpus, a.kind, a.id)
    if a.per_voice:
        items = stratified([i for i in items if i["kind"] == "pos"], a.per_voice) + \
            [i for i in items if i["kind"] != "pos"]
    if a.limit:
        items = items[:a.limit]
    out = open(a.out, "a", buffering=1)
    bed = load_bed(a.bed) if a.bed else None
    async with ha_ws.HaWs(a.url, a.token_file) as ha:
        pid = await ha.pipeline_id(a.pipeline)
        for n, it in enumerate(items, 1):
            x, sr = sf.read(str(Path(a.corpus) / it["file_16k_mono"]), dtype="int16")
            assert sr == RATE and x.ndim == 1
            extra = {}
            if bed is not None:
                pcm, bed_off, clipped = mix_on_bed(x, bed, n, a.lead, a.tail, a.speech_gain_db)
                extra = {"bed": Path(a.bed).parent.name, "bed_offset_s": round(bed_off, 3),
                         "speech_gain_db": a.speech_gain_db, "clipped": clipped}
            else:
                pcm = np.concatenate([np.zeros(int(a.lead * RATE), np.int16), x,
                                      np.zeros(int(a.tail * RATE), np.int16)])
            t0 = time.time()
            evs = await run_item(ha, pid, pcm, a.lead, a.speed)
            summ = ha_ws.summarize_run({"events": evs})
            dets = []
            if summ["wake"] and summ["wake"].get("audio_ms") is not None:
                dets.append(summ["wake"]["audio_ms"] / 1000 - a.lead)
            row = {"condition": a.condition, "item": item_fields(it), "detections_s": dets,
                   "wake_id": (summ["wake"] or {}).get("id"), "stt_text": summ["stt_text"],
                   "errors": summ["errors"], "t_pc": round(t0, 3), **extra}
            if it["kind"] == "pos" and it["subkind"] == "cmd" and summ["stt_text"] is not None:
                res = await ha.recognize([summ["stt_text"], it["expected_command_text"]], "es")
                row["recog"], row["recog_ref"] = res[0], res[1]
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
            print(f"{n}/{len(items)} {it['id']}: wake={dets} stt={summ['stt_text']!r} err={summ['errors']}",
                  file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--url", required=True)
    ap.add_argument("--token-file", required=True)
    ap.add_argument("--pipeline", required=True)
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--kind", action="append", help="pos, neg, nearmiss (repeatable; default all)")
    ap.add_argument("--id", action="append", help="item id (repeatable)")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--per-voice", type=int, default=0, help="stratified subset of positives per voice")
    ap.add_argument("--lead", type=float, default=2.0, help="silence before the item (s)")
    ap.add_argument("--tail", type=float, default=3.0, help="silence after the item (s)")
    ap.add_argument("--speed", type=float, default=1.0,
                    help="send rate as a multiple of real time; keep 1: faster sending loses the audio "
                         "between the detection and the start of STT")
    ap.add_argument("--condition", default="digital")
    ap.add_argument("--bed", help="TV-mic recording (tv_dualcap mic.raw) to mix the items into")
    ap.add_argument("--speech-gain-db", type=float, default=-39.0,
                    help="item gain re full scale when mixing on a bed (default: rig reference at the mic)")
    asyncio.run(main_async(ap.parse_args()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
