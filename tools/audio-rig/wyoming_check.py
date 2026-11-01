#!/usr/bin/env python3
"""Offline check: does a recording work with Wyoming wake-word and STT services?

Loads a WAV or raw PCM recording, selects a channel, trims, optionally peak-normalizes,
converts to 16 kHz mono S16_LE and then:

  1. streams the whole signal to a Wyoming wake-word service (Detect, AudioStart,
     AudioChunk..., AudioStop) and prints every Detection (name, audio timestamp);
  2. sends the segment after the wake word to a Wyoming STT service (Transcribe,
     AudioStart, AudioChunk..., AudioStop) and prints the transcript, plus a word
     error rate when --expected is given.

Timing and expected text come from --reference (a stimulus WAV with an entry in the
sibling manifest.json): the stimulus start is found by cross-correlation, the first
segment is taken as the wake word and the remaining segments as the STT expectation.
Without --reference, use --stt-start/--stt-end (seconds in the processed signal).

Examples:
  wyoming_check.py artifacts/audio-rig/stimuli/speech_full_16k.wav \\
      --wake-host localhost --wake-port 10400 --stt-host localhost --stt-port 10300 \\
      --reference artifacts/audio-rig/stimuli/speech_full_16k.wav
  wyoming_check.py tv.raw --rate 16000 --channels 2 --format S24_LE --channel mean \\
      --trim-start 2 --normalize-db -3 --reference .../speech_full_16k.wav \\
      --wake-host localhost --stt-host localhost

Privacy: the signal is sent only to the hosts you name; nothing is saved unless
--save-wav is given. Do not commit recordings or outputs.
"""
import argparse
import asyncio
import json
import math
import re
import sys
import unicodedata
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import butter, fftconvolve, sosfiltfilt

sys.path.insert(0, str(Path(__file__).resolve().parent))
from analyze import RAW_FORMATS, load_recording, resample  # noqa: E402

TARGET_RATE = 16000


# ------------------------------------------------------------------ signal prep

def select_channel(x, channel):
    """x: (n, channels) float. channel: 'mean' or a channel index."""
    if channel == "mean":
        return x.mean(axis=1)
    idx = int(channel)
    if not 0 <= idx < x.shape[1]:
        raise SystemExit(f"--channel {idx} out of range (recording has {x.shape[1]} channels)")
    return x[:, idx]


def dbfs(v):
    return 20 * math.log10(max(float(v), 1e-12))


def prepare(args):
    x, fs = load_recording(Path(args.input), args.raw, args.rate, args.channels, args.format)
    mono = select_channel(x, args.channel)
    mono = resample(mono, fs, TARGET_RATE)
    start = int(round(args.trim_start * TARGET_RATE))
    mono = mono[start:]
    if args.highpass:
        mono = sosfiltfilt(butter(4, args.highpass, "highpass", fs=TARGET_RATE, output="sos"), mono)
    if args.normalize_db is not None:
        peak = float(np.max(np.abs(mono)))
        if peak > 0:
            mono = mono * (10 ** (args.normalize_db / 20) / peak)
    applied_gain = None
    if args.gain_db:
        peak = float(np.max(np.abs(mono)))
        limit = 10 ** (-1.0 / 20) / peak if peak > 0 else 1.0  # keep the peak at or below -1 dBFS
        applied_gain = min(10 ** (args.gain_db / 20), limit)
        mono = mono * applied_gain
    mono = np.clip(mono, -1.0, 1.0)
    pcm = np.round(mono * 32767).astype("<i2")
    stats = {
        "peak_dbfs": round(dbfs(np.max(np.abs(mono))), 1),
        "rms_dbfs": round(dbfs(np.sqrt(np.mean(mono**2))), 1),
        "duration_s": round(len(pcm) / TARGET_RATE, 2),
    }
    if applied_gain is not None:
        stats["applied_gain_db"] = round(dbfs(applied_gain), 1)
    return pcm, stats


# ------------------------------------------------------------------ reference / alignment

def load_reference(path):
    """Return (signal at 16 kHz, segments list from the sibling manifest or None)."""
    ref, fs = sf.read(str(path), dtype="float64", always_2d=True)
    ref = resample(ref.mean(axis=1), fs, TARGET_RATE)
    segments = None
    manifest = Path(path).parent / "manifest.json"
    if manifest.exists():
        stem = Path(path).name.rsplit("_", 1)[0]
        for rate in json.loads(manifest.read_text()).get("rates", {}).values():
            params = rate.get("stimuli", {}).get(stem, {}).get("params", {})
            if "segments" in params:
                segments = params["segments"]
    return ref, segments


def locate(signal_i16, ref):
    """Start (s) of ref in signal by normalized cross-correlation of the envelope-free signals."""
    x = signal_i16.astype(np.float64) / 32768.0
    if len(x) < len(ref):
        return None, 0.0
    corr = fftconvolve(x, ref[::-1], mode="valid")
    energy = np.sqrt(np.maximum(fftconvolve(x**2, np.ones(len(ref)), mode="valid"), 1e-18))
    score = corr / (energy * np.linalg.norm(ref))
    i = int(np.argmax(np.abs(score)))
    return i / TARGET_RATE, float(abs(score[i]))


# ------------------------------------------------------------------ text scoring

def normalize_text(text):
    text = unicodedata.normalize("NFKD", text.lower())
    text = "".join(c for c in text if not unicodedata.combining(c))
    return re.sub(r"[^a-z0-9 ]+", " ", text).split()


def wer(expected, hypothesis):
    ref, hyp = normalize_text(expected), normalize_text(hypothesis)
    if not ref:
        return None
    d = list(range(len(hyp) + 1))
    for i, r in enumerate(ref, 1):
        prev, d[0] = d[0], i
        for j, h in enumerate(hyp, 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (r != h))
    return d[len(hyp)] / len(ref)


# ------------------------------------------------------------------ Wyoming

async def stream_audio(client, pcm, chunk_ms, pace):
    from wyoming.audio import AudioChunk, AudioStart, AudioStop

    step = int(TARGET_RATE * chunk_ms / 1000)
    await client.write_event(AudioStart(rate=TARGET_RATE, width=2, channels=1).event())
    for pos in range(0, len(pcm), step):
        chunk = AudioChunk(
            rate=TARGET_RATE, width=2, channels=1,
            audio=pcm[pos:pos + step].tobytes(),
            timestamp=int(pos * 1000 / TARGET_RATE),
        )
        await client.write_event(chunk.event())
        if pace:
            await asyncio.sleep(chunk_ms / 1000)
    await client.write_event(AudioStop().event())


async def run_wake(host, port, pcm, name, chunk_ms, pace, tail_s, timeout, settle):
    from wyoming.client import AsyncTcpClient
    from wyoming.wake import Detect, Detection, NotDetected

    pcm = np.concatenate([pcm, np.zeros(int(tail_s * TARGET_RATE), dtype="<i2")])
    detections = []
    async with AsyncTcpClient(host, port) as client:
        await client.write_event(Detect(names=[name] if name else None).event())
        sender = asyncio.create_task(stream_audio(client, pcm, chunk_ms, pace))
        try:
            while True:
                # after AudioStop the server sends NotDetected if nothing fired; after a
                # Detection it may stay silent, so only wait `settle` seconds for more events
                wait = settle if sender.done() else timeout
                event = await asyncio.wait_for(client.read_event(), wait)
                if event is None or NotDetected.is_type(event.type):
                    break
                if Detection.is_type(event.type):
                    d = Detection.from_event(event)
                    detections.append({
                        "name": d.name,
                        "timestamp_s": None if d.timestamp is None else d.timestamp / 1000,
                    })
        except asyncio.TimeoutError:
            pass
        finally:
            if sender.done():
                sender.result()
            else:
                sender.cancel()
    return detections


async def run_stt(host, port, pcm, language, chunk_ms, timeout):
    from wyoming.asr import Transcribe, Transcript
    from wyoming.client import AsyncTcpClient

    async with AsyncTcpClient(host, port) as client:
        await client.write_event(Transcribe(language=language).event())
        await stream_audio(client, pcm, chunk_ms, pace=False)
        while True:
            event = await asyncio.wait_for(client.read_event(), timeout)
            if event is None:
                return None
            if Transcript.is_type(event.type):
                return Transcript.from_event(event).text


# ------------------------------------------------------------------ CLI

def parse_args(argv):
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("input", help="WAV file, or raw PCM (.raw/.pcm or --raw)")
    p.add_argument("--raw", action="store_true", help="treat input as raw PCM")
    p.add_argument("--rate", type=int, help="raw: sample rate")
    p.add_argument("--channels", type=int, help="raw: channel count")
    p.add_argument("--format", choices=RAW_FORMATS, help="raw: sample format (S24_LE = 24-bit LSB-justified in int32)")
    p.add_argument("--channel", default="mean", help="channel index or 'mean' (default mean)")
    p.add_argument("--trim-start", type=float, default=0.0, help="drop the first N seconds")
    p.add_argument("--highpass", type=float, default=0.0, metavar="HZ",
                   help="4th-order zero-phase high-pass before normalization (default off)")
    p.add_argument("--gain-db", type=float, default=0.0, metavar="DB",
                   help="fixed gain, reduced if needed so the peak stays <= -1 dBFS (default off)")
    p.add_argument("--normalize-db", type=float, default=None, metavar="DBFS",
                   help="peak-normalize to this level (default: keep natural level)")
    p.add_argument("--save-wav", help="write the processed 16 kHz mono signal here (stays local)")
    p.add_argument("--reference", help="stimulus WAV with a manifest.json entry (alignment and expected text)")
    p.add_argument("--wake-host")
    p.add_argument("--wake-port", type=int, default=10400)
    p.add_argument("--wake-name", default="alexa",
                   help="wake word model name as listed by the server's Info (default alexa; "
                        "unknown names such as alexa_v0.1 silently give no detections)")
    p.add_argument("--stt-host")
    p.add_argument("--stt-port", type=int, default=10300)
    p.add_argument("--language", default="es")
    p.add_argument("--stt-start", type=float, help="STT segment start (s, processed signal)")
    p.add_argument("--stt-end", type=float, help="STT segment end (s); default end of signal")
    p.add_argument("--stt-lead", type=float, default=0.3, help="seconds before the first phrase to include")
    p.add_argument("--stt-tail", type=float, default=0.5, help="seconds after the last phrase to include")
    p.add_argument("--expected", help="expected STT text (overrides the manifest)")
    p.add_argument("--chunk-ms", type=int, default=80)
    p.add_argument("--pace", action="store_true", help="stream wake audio in real time (default: as fast as possible)")
    p.add_argument("--tail-s", type=float, default=1.0, help="silence appended to the wake stream")
    p.add_argument("--settle", type=float, default=3.0, help="seconds to wait for wake events after AudioStop")
    p.add_argument("--timeout", type=float, default=60.0)
    p.add_argument("--label", default="", help="free-text label echoed in the output")
    p.add_argument("--json", action="store_true", help="print one JSON object instead of text")
    args = p.parse_args(argv)
    if not (args.wake_host or args.stt_host):
        p.error("give --wake-host and/or --stt-host")
    return args


def main(argv=None):
    args = parse_args(argv)
    pcm, stats = prepare(args)
    result = {"label": args.label, "input": Path(args.input).name, "channel": args.channel,
              "trim_start_s": args.trim_start, "highpass_hz": args.highpass,
              "normalize_db": args.normalize_db, **stats}
    if args.save_wav:
        sf.write(args.save_wav, pcm, TARGET_RATE, subtype="PCM_16")

    expected, stt_start, stt_end = args.expected, args.stt_start, args.stt_end
    if args.reference:
        ref, segments = load_reference(args.reference)
        offset, score = locate(pcm, ref)
        result["stimulus_start_s"] = None if offset is None else round(offset, 2)
        result["align_score"] = round(score, 2)
        if offset is not None and segments:
            result["expected_wake_s"] = round(offset + segments[0]["start_s"], 2)
            if stt_start is None:
                stt_start = offset + segments[1]["start_s"] - args.stt_lead
            if stt_end is None:
                stt_end = offset + segments[-1]["end_s"] + args.stt_tail
            if expected is None:
                expected = " ".join(s["text"] for s in segments[1:])
    if stt_start is None:
        stt_start = 0.0

    if args.wake_host:
        det = asyncio.run(run_wake(args.wake_host, args.wake_port, pcm, args.wake_name,
                                   args.chunk_ms, args.pace, args.tail_s, args.timeout, args.settle))
        result["wake_detections"] = det
    if args.stt_host:
        a = max(int(stt_start * TARGET_RATE), 0)
        b = len(pcm) if stt_end is None else int(stt_end * TARGET_RATE)
        result["stt_segment_s"] = [round(a / TARGET_RATE, 2), round(b / TARGET_RATE, 2)]
        text = asyncio.run(run_stt(args.stt_host, args.stt_port, pcm[a:b], args.language,
                                   args.chunk_ms, args.timeout))
        result["transcript"] = text
        if expected:
            result["expected"] = expected
            score = wer(expected, text or "")
            result["wer"] = None if score is None else round(score, 2)

    if args.json:
        print(json.dumps(result, ensure_ascii=False))
    else:
        for key, value in result.items():
            print(f"{key}: {value}")


if __name__ == "__main__":
    main()
