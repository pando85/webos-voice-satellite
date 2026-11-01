#!/usr/bin/env python3
"""End-to-end check of the real `webos-satellite` binary against a fake HA.

Starts the daemon with a synthetic file microphone (S24_LE stereo sine), the
WAV sink and a fresh Noise key, then plays HA with aioesphomeapi 46.6.0:
handshake, device info, a wake word run with a streamed TTS reply, a
push-to-talk run triggered by SIGUSR1, and a SIGTERM shutdown. It also checks
the log policy (no transcript or reply text at info level, no key).

Usage (from the repository root):
    tools/esphome-fake-ha/run.sh --satellite-only
or, with a prebuilt binary:
    tools/esphome-fake-ha/.venv/bin/python tools/esphome-fake-ha/satellite_e2e.py --binary PATH

Everything stays on 127.0.0.1 and in a temporary directory.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import math
import os
import signal
import socket
import struct
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fake_ha import RESULTS, TIMEOUT, USE_WAKE_WORD, FakeHA, check, until_true  # noqa: E402

from aioesphomeapi import VoiceAssistantEventType as Ev  # noqa: E402

TRANSCRIPT = "e2e transcript sentinel"
REPLY = "e2e reply sentinel"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def write_mic(path: str, seconds: float) -> None:
    """Raw S24_LE stereo 16 kHz: a 440 Hz sine at about -30 dBFS."""
    frames = int(16000 * seconds)
    amp = int(0x7FFFFF * 0.03)
    with open(path, "wb") as f:
        for n in range(frames):
            v = int(amp * math.sin(2 * math.pi * 440 * n / 16000)) & 0xFFFFFF
            f.write(struct.pack("<ii", v, v))


class Satellite:
    def __init__(self, binary: str, config: str, log_path: str) -> None:
        self.binary, self.config, self.log_path = binary, config, log_path

    async def start(self) -> None:
        self.log = open(self.log_path, "w")
        self.proc = await asyncio.create_subprocess_exec(
            self.binary, "--config", self.config,
            stdin=asyncio.subprocess.DEVNULL, stdout=self.log, stderr=self.log,
        )
        ok = await until_true(lambda: "ESPHome API on" in self.text(), TIMEOUT)
        assert ok, f"satellite did not start:\n{self.text()}"

    def text(self) -> str:
        with open(self.log_path) as f:
            return f.read()

    async def wait_log(self, needle: str, timeout: float = TIMEOUT) -> bool:
        return await until_true(lambda: needle in self.text(), timeout)


async def scenario(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: Noise, file mic, wav sink, wake + push-to-talk ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    psk_file = os.path.join(tmp, "psk")
    with open(psk_file, "w") as f:
        f.write(psk + "\n")
    os.chmod(psk_file, 0o600)
    write_mic(os.path.join(tmp, "mic.raw"), 30.0)
    wav_dir = os.path.join(tmp, "wav")
    os.mkdir(wav_dir)
    port = free_port()
    config = os.path.join(tmp, "sat.toml")
    with open(config, "w") as f:
        f.write(f"""
mode = "wake"
[device]
name = "e2e-satellite"
friendly_name = "E2E Satellite"
mac = "02:00:00:00:00:e2"
bind = "127.0.0.1"
port = {port}
noise_psk_file = "psk"
[mic]
source = "file"
path = "mic.raw"
[sink]
type = "wav"
dir = "wav"
[log]
level = "info"
""")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite.log"))
    await sat.start()
    ha = FakeHA(port, psk)
    try:
        cli = await ha.connect()
        info, _entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("S1 Noise handshake, device info from the config",
              info.name == "e2e-satellite" and info.friendly_name == "E2E Satellite"
              and info.mac_address == "02:00:00:00:00:E2" and flags == 23,
              f"{info.name} {info.mac_address} flags={flags}")
        cli.subscribe_states(lambda s: None)
        ha.subscribe_voice()
        conv, flags, _ = await ha.next_start()
        check("S2 wake mode: start request with USE_WAKE_WORD", bool(flags & USE_WAKE_WORD), f"flags={flags}")
        ok = await until_true(lambda: ha.audio.qsize() >= 15, TIMEOUT)
        chunks = ha.drain_audio()
        check("S3 file mic streamed: 1024-byte chunks, non-silent, after the front-end",
              ok and all(len(c) == 1024 for c in chunks) and any(any(c) for c in chunks),
              f"{len(chunks)} chunks")

        ha.event(Ev.VOICE_ASSISTANT_RUN_START)
        ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_START)
        ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_END)
        ha.event(Ev.VOICE_ASSISTANT_STT_START)
        ha.event(Ev.VOICE_ASSISTANT_STT_VAD_END)
        abort = await asyncio.wait_for(ha.stops.get(), TIMEOUT)
        check("S4 end of speech: device ends its audio stream gracefully", abort is False, f"abort={abort}")
        await asyncio.sleep(0.3)
        ha.drain_audio()
        await asyncio.sleep(0.3)
        check("S5 no mic audio after the stream ended", ha.audio.empty(), f"{ha.audio.qsize()} chunks")
        ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": TRANSCRIPT})
        ha.event(Ev.VOICE_ASSISTANT_INTENT_START)
        ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "c1", "speech": REPLY,
                                                 "continue_conversation": "0"})
        ha.event(Ev.VOICE_ASSISTANT_TTS_START, {"text": REPLY})
        ha.event(Ev.VOICE_ASSISTANT_TTS_END, {"url": "http://127.0.0.1:1/api/tts_proxy/secret-token.wav"})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        await ha.stream_tts(20)
        ok = await until_true(lambda: any(f.endswith(".wav") for f in os.listdir(wav_dir)), TIMEOUT)
        files = [os.path.join(wav_dir, f) for f in os.listdir(wav_dir)]
        size = os.path.getsize(files[0]) if files else 0
        check("S6 streamed TTS written to the wav sink (20 x 1024 bytes + header)",
              ok and size == 44 + 20 * 1024, f"{files} {size}")
        check("S7 latency line logged", await sat.wait_log("run 1 latency ms (from wake word)"))
        conv, flags, _ = await ha.next_start()
        check("S8 re-armed after playback, reusing the conversation id",
              bool(flags & USE_WAKE_WORD) and conv == "c1", f"flags={flags} conv={conv!r}")

        sat.proc.send_signal(signal.SIGUSR1)
        abort = await asyncio.wait_for(ha.stops.get(), TIMEOUT)
        conv, flags, _ = await ha.next_start()
        check("S9 SIGUSR1: wake run aborted, push-to-talk run starts at STT",
              abort is True and not flags & USE_WAKE_WORD, f"abort={abort} flags={flags}")
        ok = await until_true(lambda: ha.audio.qsize() >= 5, TIMEOUT)
        check("S10 push-to-talk run streams audio", ok, f"{ha.audio.qsize()} chunks")
        ha.event(Ev.VOICE_ASSISTANT_RUN_START)
        ha.event(Ev.VOICE_ASSISTANT_ERROR, {"code": "stt-no-text-recognized", "message": "No text"})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        check("S11 error run logged with its code",
              await sat.wait_log("error=stt-no-text-recognized"))

        t = time.monotonic()
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        await until_true(lambda: ha.stopped.is_set(), 2)
        check("S12 SIGTERM: clean exit, HA told to disconnect",
              rc == 0 and ha.stopped.is_set() and "stopped" in sat.text(),
              f"rc={rc} in {time.monotonic() - t:.2f}s")
        log = sat.text()
        check("S13 log policy: no transcript, reply text, token URL or key at info level",
              TRANSCRIPT not in log and REPLY not in log and "secret-token" not in log and psk not in log)
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, help="path to the webos-satellite binary")
    args = parser.parse_args()
    failed = False
    with tempfile.TemporaryDirectory(prefix="webos-sat-e2e-") as tmp:
        try:
            await scenario(args.binary, tmp)
        except Exception as err:  # noqa: BLE001
            failed = True
            print(f"FAIL  {type(err).__name__}: {err}", flush=True)
            log = os.path.join(tmp, "satellite.log")
            if os.path.exists(log):
                print("---- satellite log ----")
                print(open(log).read())
    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print(f"\n{passed} checks passed, {'FAILED' if failed else 'all OK'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
