#!/usr/bin/env python3
"""End-to-end check of the real `webos-satellite` binary against a fake HA.

Starts the daemon with a synthetic file microphone (S24_LE stereo sine), the
WAV sink and a fresh Noise key, then plays HA with aioesphomeapi 46.6.0:
handshake, device info, a wake word run with a streamed TTS reply, a
push-to-talk run triggered by SIGUSR1, and a SIGTERM shutdown. It also checks
the log policy (no transcript or reply text at info level, no key). The AEC
scenario runs the daemon with `[aec] enabled` on a synthetic mic + reference
file pair (the mic holds a delayed echo of the reference) and checks the delay
lock and that the streamed audio has the echo removed.

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
from fake_ha import (  # noqa: E402
    RESULTS,
    TIMEOUT,
    USE_WAKE_WORD,
    FakeHA,
    check,
    clear_key_like_ha,
    fetch_device_info_like_ha,
    provision_over_zero_psk,
    until_true,
)

from aioesphomeapi import VoiceAssistantEventType as Ev  # noqa: E402
from aioesphomeapi import VoiceAssistantTimerEventType as TimerEv  # noqa: E402

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


async def scenario_timers(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: [device] timers + start_conversation, timer events ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    with open(os.path.join(tmp, "psk"), "w") as f:
        f.write(psk + "\n")
    os.chmod(os.path.join(tmp, "psk"), 0o600)
    write_mic(os.path.join(tmp, "mic.raw"), 5.0)
    port = free_port()
    config = os.path.join(tmp, "sat-timers.toml")
    with open(config, "w") as f:
        f.write(f"""
[device]
name = "e2e-timers"
mac = "02:00:00:00:00:e3"
bind = "127.0.0.1"
port = {port}
noise_psk_file = "psk"
timers = true
start_conversation = true
[mic]
source = "file"
path = "mic.raw"
[sink]
type = "null"
[log]
level = "info"
""")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite-timers.log"))
    await sat.start()
    ha = FakeHA(port, psk)
    try:
        cli = await ha.connect()
        info, _entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("T1 config keys advertise TIMERS and START_CONVERSATION (63)", flags == 63, f"flags={flags}")
        name = "timer-name-sentinel"
        cli.send_voice_assistant_timer_event(TimerEv.VOICE_ASSISTANT_TIMER_STARTED, "t-1", name, 300, 300, True)
        cli.send_voice_assistant_timer_event(TimerEv.VOICE_ASSISTANT_TIMER_UPDATED, "t-1", name, 360, 342, True)
        cli.send_voice_assistant_timer_event(TimerEv.VOICE_ASSISTANT_TIMER_FINISHED, "t-1", name, 360, 0, False)
        ok = await sat.wait_log("timer finished id=t-1 seconds_left=0")
        log = sat.text()
        check("T2 timer started/updated/finished logged with id and seconds",
              ok and "timer started id=t-1 seconds_left=300" in log
              and "timer updated id=t-1 seconds_left=342" in log)
        check("T3 timer name never logged", name not in log)
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        check("T4 clean exit", rc == 0, f"rc={rc}")
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()


def pairing_config(tmp: str, name: str, port: int, mac: str, window_s: int) -> str:
    config = os.path.join(tmp, f"{name}.toml")
    with open(config, "w") as f:
        f.write(f"""
mode = "wake"
[device]
name = "{name}"
mac = "{mac}"
bind = "127.0.0.1"
port = {port}
[mic]
source = "file"
path = "mic.raw"
loop = true
[sink]
type = "null"
[log]
level = "debug"
[pairing]
enabled = true
state_dir = "{name}-state"
window_s = {window_s}
""")
    return config


async def scenario_pairing(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: [pairing] HA-driven key provisioning ==")
    write_mic(os.path.join(tmp, "mic.raw"), 5.0)
    port = free_port()
    config = pairing_config(tmp, "e2e-pairing", port, "02:00:00:00:00:e4", 600)
    state = os.path.join(tmp, "e2e-pairing-state")
    key_file = os.path.join(state, "noise_psk")
    log_path = os.path.join(tmp, "satellite-pairing.log")
    sat = Satellite(binary, config, log_path)
    await sat.start()
    new_key = base64.b64encode(os.urandom(32))
    try:
        ok = await sat.wait_log("PAIRING WINDOW OPEN for 600 s") and await sat.wait_log(
            "capture: microphone off (not paired)")
        check("P1 unpaired start: window open, microphone closed (not paired)",
              ok and "capture: microphone open" not in sat.text()
              and (os.stat(state).st_mode & 0o777) == 0o700)
        ha = FakeHA(port)
        cli = await ha.connect()
        info, entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        ha.subscribe_voice()
        no_start = await ha.expect_no_start(1.0)
        check("P2 keyless HA session: provisionable, no voice, no pipeline, no audio",
              info.api_encryption_provisionable and flags == 0 and entities == [] and no_start
              and ha.audio.empty() and "HA connected" not in sat.text(), f"flags={flags}")
        ok = await provision_over_zero_psk(port, new_key)
        check("P3 HA's zero-PSK provisioning accepted", ok is True)
        ok = await sat.wait_log("PAIRED by") and await sat.wait_log("capture: microphone open")
        mode = os.stat(key_file).st_mode & 0o777
        check("P4 key stored (0600), keyless session closed, microphone opens",
              ok and mode == 0o600 and await until_true(lambda: ha.stopped.is_set()), f"mode={mode:o}")

        ha2 = FakeHA(port, new_key.decode())
        cli2 = await ha2.connect()
        info2 = await cli2.device_info()
        ha2.subscribe_voice()
        _conv, vflags, _ = await ha2.next_start()
        ok = await until_true(lambda: ha2.audio.qsize() >= 5, TIMEOUT)
        check("P5 reconnect with the provisioned key: wake word run streams the file mic",
              ok and bool(vflags & USE_WAKE_WORD) and not info2.api_encryption_provisionable
              and info2.voice_assistant_feature_flags_compat(cli2.api_version) == 23)
        await cli2.disconnect()
        res = [await fetch_device_info_like_ha(port, k) for k in (None, base64.b64encode(os.urandom(32)).decode())]
        check("P6 no key / wrong key refused", res == ["requires_encryption_key", "invalid_psk"], str(res))

        sat.proc.send_signal(signal.SIGTERM)
        await asyncio.wait_for(sat.proc.wait(), 5)
        sat.log.close()
        sat = Satellite(binary, config, log_path + ".2")
        await sat.start()
        ok = await sat.wait_log("esphome-api: paired (stored key)")
        check("P7 restart: paired from the stored key, no window",
              ok and "PAIRING WINDOW OPEN" not in sat.text()
              and await fetch_device_info_like_ha(port, new_key.decode()) == "ok")

        ha3 = FakeHA(port, new_key.decode())
        await ha3.connect()
        with open(os.path.join(state, "reset-pairing"), "w"):
            pass
        ok = await sat.wait_log("PAIRING RESET") and await sat.wait_log("PAIRING WINDOW OPEN")
        ok = ok and await sat.wait_log("PCM released (not paired)")
        check("P8 reset-pairing file: key deleted, session dropped, window open, microphone closed",
              ok and not os.path.exists(key_file) and not os.path.exists(os.path.join(state, "reset-pairing"))
              and await until_true(lambda: ha3.stopped.is_set())
              and await fetch_device_info_like_ha(port, new_key.decode()) == "invalid_psk")
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        logs = open(log_path).read() + sat.text()
        check("P9 clean exit; the key is never logged (debug level)",
              rc == 0 and new_key.decode() not in logs, f"rc={rc}")
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()


async def scenario_pairing_window(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: [pairing] window timeout, pair trigger file, key clear ==")
    write_mic(os.path.join(tmp, "mic.raw"), 5.0)
    port = free_port()
    config = pairing_config(tmp, "e2e-window", port, "02:00:00:00:00:e5", 2)
    state = os.path.join(tmp, "e2e-window-state")
    key_file = os.path.join(state, "noise_psk")
    log_path = os.path.join(tmp, "satellite-window.log")
    sat = Satellite(binary, config, log_path)
    await sat.start()
    key = base64.b64encode(os.urandom(32))
    try:
        ok = await sat.wait_log("pairing window closed without a key", timeout=6)
        res = await fetch_device_info_like_ha(port, None)
        check("W1 window timeout: HA's config flow gets provisioning_closed", ok and res == "provisioning_closed", res)
        with open(os.path.join(state, "pair"), "w"):
            pass
        ok = await sat.wait_log("found: opening the pairing window")
        res = await fetch_device_info_like_ha(port, None)
        check("W2 pair trigger file reopens the window", ok and res == "ok", res)
        ok = await provision_over_zero_psk(port, key)
        ok = ok and await sat.wait_log("PAIRED by") and await sat.wait_log("capture: microphone open")
        check("W3 paired in the reopened window, microphone open", ok)
        await asyncio.sleep(1.1)  # one SetKey attempt per second
        ok = await clear_key_like_ha(port, key.decode())
        ok = ok and await sat.wait_log("pairing CLEARED by") and await sat.wait_log("PCM released (not paired)")
        res = await fetch_device_info_like_ha(port, None)
        check("W4 HA's clear on entry removal: key deleted, microphone closed, window stays closed",
              ok and not os.path.exists(key_file) and res == "provisioning_closed", res)
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        sat.log.close()
        sat = Satellite(binary, config, log_path + ".2")
        await sat.start()
        ok = await sat.wait_log("the key holder cleared the pairing key")
        res = await fetch_device_info_like_ha(port, None)
        check("W5 restart after the clear: window still closed", rc == 0 and ok and res == "provisioning_closed"
              and "PAIRING WINDOW OPEN" not in sat.text(), res)
        with open(os.path.join(state, "pair"), "w"):
            pass
        ok = await sat.wait_log("found: opening the pairing window")
        res = await fetch_device_info_like_ha(port, None)
        check("W6 pair trigger file reopens it", ok and res == "ok", res)
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        check("W7 clean exit; the key is never logged",
              rc == 0 and key.decode() not in open(log_path).read() + sat.text(), f"rc={rc}")
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()


def write_echo_pair(mic_path: str, ref_path: str, seconds: float) -> float:
    """mic: S24_LE stereo 16 kHz, the echo (15 ms, -10 dB) of a band-limited
    noise; ref: that noise as S32_LE stereo 48 kHz (each sample held three
    times). Returns the echo RMS in dBFS."""
    import random

    rnd = random.Random(7)
    n = int(16000 * seconds)
    white = [rnd.uniform(-0.1, 0.1) for _ in range(n + 3)]
    base = [sum(white[i:i + 4]) / 4 for i in range(n)]
    echo = [0.3 * base[i - 240] if i >= 240 else 0.0 for i in range(n)]
    with open(mic_path, "wb") as f:
        for v in echo:
            s24 = int(v * 0x7FFFFF) & 0xFFFFFF
            f.write(struct.pack("<ii", s24, s24))
    with open(ref_path, "wb") as f:
        for v in base:
            s32 = int(v * 0x7FFFFFFF)
            f.write(struct.pack("<ii", s32, s32) * 3)
    ms = sum(v * v for v in echo[16000:]) / (n - 16000)
    return 10 * math.log10(ms)


async def scenario_aec(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: [aec] with a file mic + reference pair ==")
    echo_db = write_echo_pair(os.path.join(tmp, "aec-mic.raw"), os.path.join(tmp, "aec-ref.raw"), 10.0)
    port = free_port()
    config = os.path.join(tmp, "sat-aec.toml")
    with open(config, "w") as f:
        f.write(f"""
mode = "wake"
[device]
name = "e2e-aec"
mac = "02:00:00:00:00:ae"
bind = "127.0.0.1"
port = {port}
allow_plaintext = true
[mic]
source = "file"
path = "aec-mic.raw"
[dsp]
gain_db = 0.0
[aec]
enabled = true
reference_path = "aec-ref.raw"
[sink]
type = "null"
[log]
level = "debug"
""")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite-aec.log"))
    await sat.start()
    ha = FakeHA(port, None)
    try:
        cli = await ha.connect()
        await cli.device_info_and_list_entities()
        cli.subscribe_states(lambda s: None)
        ha.subscribe_voice()
        await ha.next_start()
        ok = await sat.wait_log("aec: echo cancellation on: reference file")
        line = next((l for l in sat.text().splitlines() if "echo cancellation on" in l), "missing")
        check("A1 canceller on with the file reference", ok, line.split("aec: ", 1)[-1].strip())
        ok = await sat.wait_log("aec: echo delay ", 8)
        line = next((l for l in sat.text().splitlines() if "aec: echo delay " in l), "")
        try:
            delay = float(line.split("aec: echo delay ")[1].split(" ms")[0])
        except (IndexError, ValueError):
            delay = float("nan")
        check("A2 delay locked at the true 15 ms (mic behind reference)",
              ok and abs(delay - 15.0) < 0.5, line.strip())
        # Stream for 6 s more (real time), then compare the last 2 s with the echo level.
        samples: list[int] = []
        t = time.monotonic()
        while time.monotonic() - t < 6.0:
            for c in ha.drain_audio():
                samples.extend(struct.unpack(f"<{len(c) // 2}h", c))
            await asyncio.sleep(0.1)
        tail = samples[-32000:]
        out_db = 10 * math.log10(sum(v * v for v in tail) / max(len(tail), 1) / 32768.0 ** 2 + 1e-20)
        check("A3 streamed audio has the echo removed (at least 10 dB below it)",
              len(tail) == 32000 and echo_db - out_db > 10.0,
              f"echo {echo_db:.1f} dBFS, streamed {out_db:.1f} dBFS, {len(samples)} samples")
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        check("A4 clean exit", rc == 0, f"rc={rc}")
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
            await scenario_timers(args.binary, tmp)
            await scenario_pairing(args.binary, tmp)
            await scenario_pairing_window(args.binary, tmp)
            await scenario_aec(args.binary, tmp)
        except Exception as err:  # noqa: BLE001
            failed = True
            print(f"FAIL  {type(err).__name__}: {err}", flush=True)
            for name in ("satellite.log", "satellite-timers.log", "satellite-pairing.log",
                         "satellite-pairing.log.2", "satellite-window.log", "satellite-aec.log"):
                log = os.path.join(tmp, name)
                if os.path.exists(log):
                    print(f"---- {name} ----")
                    print(open(log).read())
    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print(f"\n{passed} checks passed, {'FAILED' if failed else 'all OK'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
