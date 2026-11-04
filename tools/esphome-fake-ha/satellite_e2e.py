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

from aioesphomeapi import (  # noqa: E402
    BinarySensorInfo,
    BinarySensorState,
    ButtonInfo,
    MediaPlayerCommand,
    MediaPlayerInfo,
    NumberInfo,
    NumberState,
    SelectInfo,
    SelectState,
    SwitchInfo,
    SwitchState,
    TextSensorInfo,
    TextSensorState,
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
        info, entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("S1 Noise handshake, device info from the config (default flags 31: announce + timers)",
              info.name == "e2e-satellite" and info.friendly_name == "E2E Satellite"
              and info.mac_address == "02:00:00:00:00:E2" and flags == 31,
              f"{info.name} {info.mac_address} flags={flags}")
        bins = {e.object_id: e for e in entities if isinstance(e, BinarySensorInfo)}
        mute_sw = [e for e in entities if isinstance(e, SwitchInfo)][0]
        check("S1a listening indicator entities listed (binary sensors, no category)",
              {"listening", "microphone_muted", "timer_alarm"} <= set(bins)
              and set(bins) - {"listening", "microphone_muted", "timer_alarm"} == {"capture_problem", "firmware_changed", "firmware_verified"}
              and bins["listening"].entity_category.name == "NONE" and bins["microphone_muted"].entity_category.name == "NONE"
              and bins["listening"].name == "Listening" and bins["microphone_muted"].name == "Microphone muted",
              str(sorted(bins)))
        bstate: dict[int, BinarySensorState] = {}

        def on_state(st) -> None:
            if isinstance(st, BinarySensorState):
                bstate[st.key] = st

        def shown(obj: str):
            st = bstate.get(bins[obj].key)
            return None if st is None or st.missing_state else st.state

        cli.subscribe_states(on_state)
        ha.subscribe_voice()
        ok = await until_true(lambda: shown("listening") is True and shown("microphone_muted") is False)
        check("S1b on connect: Listening ON (PCM open) and Microphone muted OFF", ok,
              f"listening={shown('listening')} muted={shown('microphone_muted')}")
        conv, flags, _ = await ha.next_start()
        check("S2 wake mode: start request with USE_WAKE_WORD", bool(flags & USE_WAKE_WORD), f"flags={flags}")
        ok = await until_true(lambda: ha.audio.qsize() >= 15, TIMEOUT)
        chunks = ha.drain_audio()
        check("S3 file mic streamed: 1024-byte chunks, non-silent, after the front-end",
              ok and all(len(c) == 1024 for c in chunks) and any(any(c) for c in chunks),
              f"{len(chunks)} chunks")

        # Mute from HA: the PCM closes, so Listening goes OFF and muted ON;
        # unmute reopens it. The indicator follows the capture, never the intent.
        cli.switch_command(mute_sw.key, True)
        ok = await until_true(lambda: shown("listening") is False and shown("microphone_muted") is True)
        check("S3a mute: Listening OFF and Microphone muted ON after the PCM closed",
              ok and "capture: microphone closed, PCM released (muted)" in sat.text(),
              f"listening={shown('listening')} muted={shown('microphone_muted')}")
        cli.switch_command(mute_sw.key, False)
        ok = await until_true(lambda: shown("listening") is True and shown("microphone_muted") is False)
        check("S3b unmute: Listening ON again, Microphone muted OFF", ok,
              f"listening={shown('listening')} muted={shown('microphone_muted')}")
        await ha.next_start()  # the wake word run restarts after the unmute
        while not ha.stops.empty():
            ha.stops.get_nowait()  # the mute aborted the first run
        ha.drain_audio()

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


def alarm_config(tmp: str, name: str, port: int, mac: str, extra: str = "") -> str:
    config = os.path.join(tmp, f"{name}.toml")
    wav = os.path.join(tmp, f"wav-{name}")
    os.makedirs(wav, exist_ok=True)
    with open(config, "w") as f:
        f.write(f"""
[device]
name = "{name}"
mac = "{mac}"
bind = "127.0.0.1"
port = {port}
noise_psk_file = "psk"
start_conversation = true
{extra}
[mic]
source = "file"
path = "mic.raw"
[sink]
type = "wav"
dir = "wav-{name}"
[log]
level = "info"
""")
    return config


def alarm_files(tmp: str, name: str) -> list[str]:
    d = os.path.join(tmp, f"wav-{name}")
    return sorted(f for f in os.listdir(d) if f.startswith("alarm-"))


def wav_peak(path: str) -> int:
    with open(path, "rb") as f:
        data = f.read()[44:]
    n = len(data) // 2
    return max((abs(v) for v in struct.unpack(f"<{n}h", data[: n * 2])), default=0)


async def scenario_timers(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: timers on by default, alarm (start, stop, queue, cancel), sensors ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    with open(os.path.join(tmp, "psk"), "w") as f:
        f.write(psk + "\n")
    os.chmod(os.path.join(tmp, "psk"), 0o600)
    write_mic(os.path.join(tmp, "mic.raw"), 30.0)
    port = free_port()
    name = "e2e-timers"
    config = alarm_config(tmp, name, port, "02:00:00:00:00:e3")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite-timers.log"))
    await sat.start()
    ha = FakeHA(port, psk)
    try:
        cli = await ha.connect()
        info, entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("T1 timers are on by default: TIMERS + START_CONVERSATION advertised (63)", flags == 63, f"flags={flags}")
        btns = {e.object_id: e for e in entities if isinstance(e, ButtonInfo)}
        texts = {e.object_id: e for e in entities if isinstance(e, TextSensorInfo)}
        bins = {e.object_id: e for e in entities if isinstance(e, BinarySensorInfo)}
        mp = [e for e in entities if isinstance(e, MediaPlayerInfo)][0]
        check("T1a entities: Stop timer alarm button, Timers sensor, Timer alarm binary sensor",
              "stop_timer_alarm" in btns and btns["stop_timer_alarm"].name == "Stop timer alarm"
              and "timers" in texts and "timer_alarm" in bins, f"{sorted(btns)} {sorted(texts)} {sorted(bins)}")
        shown: dict[str, object] = {}

        def on_state(st) -> None:
            for obj, info_ in (("timers", texts.get("timers")), ("timer_alarm", bins.get("timer_alarm"))):
                if info_ is not None and st.key == info_.key and not getattr(st, "missing_state", False):
                    shown[obj] = st.state

        cli.subscribe_states(on_state)
        ha.subscribe_voice()
        ok = await until_true(lambda: shown.get("timers") == "none" and shown.get("timer_alarm") is False)
        check("T1b initial states: Timers none, Timer alarm off", ok, str(shown))
        await ha.next_start()

        tname = "timer-name-sentinel"
        T = TimerEv
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_STARTED, "t-1", tname, 300, 300, True)
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_UPDATED, "t-1", tname, 360, 342, True)
        ok = await until_true(lambda: shown.get("timers") == "1 timer, next in 6 min")
        check("T2 Timers sensor follows started/updated (rounded up to minutes)", ok, str(shown))

        # Finished: the alarm rings, the sensors say so, a chime reaches the sink.
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_FINISHED, "t-1", tname, 360, 0, False)
        ok = await sat.wait_log("timer finished id=t-1 seconds_left=0")
        ok = ok and await sat.wait_log("timer alarm: ringing for at most 300 s")
        ok = ok and await until_true(lambda: shown.get("timer_alarm") is True and shown.get("timers") == "ringing")
        files = alarm_files(tmp, name)
        check("T3 FINISHED rings the alarm: log, sensors, chime written to the sink",
              ok and len(files) == 1, f"{shown} {files}")
        peak = wav_peak(os.path.join(tmp, f"wav-{name}", files[0]))
        # 0.7 full scale at the default reply volume 40 % (-30 dB): about 725.
        check("T3a chime is audible-level audio at the reply volume (not silent, not full scale)",
              300 < peak < 1500, f"peak {peak}")

        # The button stops it.
        cli.button_command(btns["stop_timer_alarm"].key)
        ok = await sat.wait_log("timer alarm stopped (button in Home Assistant")
        ok = ok and await until_true(lambda: shown.get("timer_alarm") is False and shown.get("timers") == "none")
        check("T4 Stop timer alarm button stops it and the sensors clear", ok, str(shown))

        # Media player stop stops it too.
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_FINISHED, "t-2", "", 10, 0, False)
        ok = await until_true(lambda: shown.get("timer_alarm") is True)
        cli.media_player_command(mp.key, command=MediaPlayerCommand.STOP)
        ok = ok and await sat.wait_log("timer alarm stopped (media player stop")
        ok = ok and await until_true(lambda: shown.get("timer_alarm") is False)
        check("T5 media player stop stops it", ok, str(shown))

        # The wake word stops it before the pipeline streams.
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_FINISHED, "t-3", "", 10, 0, False)
        ok = await until_true(lambda: shown.get("timer_alarm") is True)
        ha.event(Ev.VOICE_ASSISTANT_RUN_START)
        ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_START)
        ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_END)
        ok = ok and await sat.wait_log("timer alarm stopped (wake word")
        ok = ok and await until_true(lambda: shown.get("timer_alarm") is False)
        check("T6 the wake word stops it", ok, str(shown))

        # A timer that finishes during a conversation waits for it to end.
        n_files = len(alarm_files(tmp, name))
        rings = sat.text().count("timer alarm: ringing")
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_FINISHED, "t-4", "", 10, 0, False)
        ok = await sat.wait_log("timer alarm: timer t-4 finished")
        await asyncio.sleep(1.0)
        queued = (ok and len(alarm_files(tmp, name)) == n_files
                  and sat.text().count("timer alarm: ringing") == rings)
        check("T7 a timer finishing mid-conversation does not ring over it (queued)", queued,
              f"{n_files} -> {len(alarm_files(tmp, name))}")
        ha.event(Ev.VOICE_ASSISTANT_STT_VAD_END)
        ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "x"})
        ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "c9", "speech": "", "continue_conversation": "0"})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        ok = await until_true(lambda: len(alarm_files(tmp, name)) == n_files + 1, 8)
        check("T8 the queued alarm rings once the run ended", ok, f"{len(alarm_files(tmp, name))} files")

        # HA changing the timer that rings stops it (a finished timer is gone
        # in HA, so this is a defensive path).
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_CANCELLED, "t-4", "", 10, 0, False)
        ok = await sat.wait_log("timer alarm stopped: Home Assistant changed timer t-4")
        ok = ok and await until_true(lambda: shown.get("timer_alarm") is False)
        check("T9 HA cancelling the finished timer stops the alarm", ok, str(shown))

        log = sat.text()
        check("T10 timer name never logged", tname not in log)
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        check("T11 clean exit", rc == 0, f"rc={rc}")
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()


async def scenario_alarm_cap(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: alarm time cap and the off switches ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    with open(os.path.join(tmp, "psk"), "w") as f:
        f.write(psk + "\n")
    port = free_port()
    config = alarm_config(tmp, "e2e-cap", port, "02:00:00:00:00:e4", "timer_alarm_max_s = 2")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite-cap.log"))
    await sat.start()
    ha = FakeHA(port, psk)
    try:
        cli = await ha.connect()
        await cli.device_info_and_list_entities()
        cli.send_voice_assistant_timer_event(TimerEv.VOICE_ASSISTANT_TIMER_FINISHED, "c-1", "", 5, 0, False)
        t0 = time.monotonic()
        ok = await sat.wait_log("timer alarm: ringing for at most 2 s")
        ok = ok and await sat.wait_log("timer alarm: stopped at the time cap", 6)
        dt = time.monotonic() - t0
        check("C1 an alarm nobody stops ends at its cap", ok and 1.5 < dt < 4.5, f"{dt:.1f} s")
        sat.proc.send_signal(signal.SIGTERM)
        await asyncio.wait_for(sat.proc.wait(), 5)
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()

    # timer_alarm_max_s = 0: timers advertised and tracked, no sound, no stop entities.
    port = free_port()
    config = alarm_config(tmp, "e2e-nosound", port, "02:00:00:00:00:e5", "timer_alarm_max_s = 0")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite-nosound.log"))
    await sat.start()
    ha = FakeHA(port, psk)
    try:
        cli = await ha.connect()
        info, entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        names = {e.object_id for e in entities}
        cli.send_voice_assistant_timer_event(TimerEv.VOICE_ASSISTANT_TIMER_FINISHED, "n-1", "", 5, 0, False)
        ok = await sat.wait_log("timer alarm: sound is off")
        await asyncio.sleep(0.5)
        check("C2 timer_alarm_max_s = 0: TIMERS still advertised, no button or alarm sensor, no sound",
              ok and flags == 63 and "timers" in names and "stop_timer_alarm" not in names
              and "timer_alarm" not in names and not alarm_files(tmp, "e2e-nosound"),
              f"flags={flags} {sorted(names)}")
        sat.proc.send_signal(signal.SIGTERM)
        await asyncio.wait_for(sat.proc.wait(), 5)
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()

    # timers = false: the flag is gone (HA then answers that timers are not supported here).
    port = free_port()
    config = alarm_config(tmp, "e2e-notimers", port, "02:00:00:00:00:e6", "timers = false")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite-notimers.log"))
    await sat.start()
    ha = FakeHA(port, psk)
    try:
        cli = await ha.connect()
        info, entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        names = {e.object_id for e in entities}
        check("C3 timers = false: flags 55 (no TIMERS), no timer entities",
              flags == 55 and not names & {"timers", "timer_alarm", "stop_timer_alarm"}, f"flags={flags} {sorted(names)}")
        sat.proc.send_signal(signal.SIGTERM)
        await asyncio.wait_for(sat.proc.wait(), 5)
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
              and info2.voice_assistant_feature_flags_compat(cli2.api_version) == 31)
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


FAKE_LUNA = r"""#!/bin/sh
# A fake luna-send for the TV control e2e: synthetic replies from state files in
# $D, every call appended to $D/calls. Needs a terminal on stdout, like the TV.
D="@D@"
[ -t 1 ] || echo "NOT-A-TTY" >> "$D/calls"
snap() { cat "$D/vol" "$D/mute" "$D/fg" "$D/power" "$D/points" 2>/dev/null | tr -d '\n'; }
vol_json() { printf '{"returnValue":true,"volumeStatus":{"volume":%s,"maxVolume":60,"muteStatus":%s,"soundOutput":"external_arc"}}' "$(cat "$D/vol")" "$(cat "$D/mute")"; }
fg_json() { printf '{"returnValue":true,"appId":"%s"}' "$(cat "$D/fg")"; }
if [ "$1" = "-i" ]; then
  echo "SUB $2" >> "$D/calls"
  last=""
  while :; do
    cur=$(snap)
    if [ "$cur" != "$last" ]; then
      last="$cur"
      case "$2" in
        */getPowerState) printf '{"returnValue":true,"state":"%s","subscribed":true}\n' "$(cat "$D/power")" ;;
        */getVolume) vol_json; echo ;;   # no "subscribed" flag, like both TVs
        */getForegroundAppInfo) fg_json | sed 's/{"returnValue":true,/{"returnValue":true,"subscribed":true,/'; echo ;;
        *) printf '{"returnValue":true,"subscribed":false}\n' ;;
      esac
    fi
    sleep 0.2
  done
fi
uri="$5"; payload="$6"
echo "$uri $payload" >> "$D/calls"
case "$uri" in
  *tv.systemproperty/getSystemInfo) echo '{"returnValue":true,"modelName":"OLED42C34LA","firmwareVersion":"03.30.74","serialNumber":"CANARYSERIAL0123456789","sdkVersion":"8.3.0"}' ;;
  */getPowerState) printf '{"returnValue":true,"state":"%s"}\n' "$(cat "$D/power")" ;;
  */getVolume) vol_json; echo ;;
  */getForegroundAppInfo) case "$uri" in *media*) echo '{"returnValue":true,"playState":"playing"}' ;; *) fg_json; echo ;; esac ;;
  */getExternalInputList) echo '{"returnValue":true,"devices":[{"id":"HDMI_1","label":"CanaryInput 1","connected":true,"appId":"app.hdmi1"},{"id":"HDMI_2","label":"CanaryInput 2","connected":false,"appId":"app.hdmi2"}]}' ;;
  */listLaunchPoints) cat "$D/points"; echo ;;
  */setVolume) sed 's/.*"volume":\([0-9]*\).*/\1/' <<EOF2 > "$D/vol"
$payload
EOF2
    echo '{"returnValue":true,"volume":1,"soundOutput":""}' ;;
  */volumeUp) echo $(( $(cat "$D/vol") + 1 )) > "$D/vol"; echo '{"returnValue":true}' ;;
  */volumeDown) echo $(( $(cat "$D/vol") - 1 )) > "$D/vol"; echo '{"returnValue":true}' ;;
  */setMute) case "$payload" in *true*) echo true > "$D/mute" ;; *) echo false > "$D/mute" ;; esac
    echo '{"returnValue":true}' ;;
  */switchInput) n=$(echo "$payload" | sed 's/.*HDMI_\([0-9]\).*/\1/'); echo "app.hdmi$n" > "$D/fg"; echo '{"returnValue":true}' ;;
  *applicationManager/launch) sed 's/.*"id":"\([^"]*\)".*/\1/' <<EOF3 > "$D/fg"
$payload
EOF3
    echo '{"returnValue":true}' ;;
  */turnOffScreen) echo "Screen Off" > "$D/power"; echo '{"returnValue":true,"state":"Screen Off"}' ;;
  */turnOnScreen|*/powerOn) echo "Active" > "$D/power"; echo '{"returnValue":true,"state":"Active"}' ;;
  *) echo '{"returnValue":true}' ;;
esac
"""

POINTS = (
    '{"returnValue":true,"launchPoints":[{"id":"app.a","title":"CanaryApp A","icon":"/secret.png"},'
    '{"id":"io.github.pando85.voicesatellite","title":"Voice Satellite"},'
    '{"id":"app.hdmi1","title":"CanaryInput 1"},{"id":"app.b","title":"CanaryApp B"}]}'
)


async def scenario_tv_control(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: TV control through a fake luna-send ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    with open(os.path.join(tmp, "psk-tv"), "w") as f:
        f.write(psk + "\n")
    os.chmod(os.path.join(tmp, "psk-tv"), 0o600)
    write_mic(os.path.join(tmp, "mic-tv.raw"), 30.0)
    tv = os.path.join(tmp, "tv")
    os.mkdir(tv)
    for name, text in (("vol", "12"), ("mute", "false"), ("fg", "app.a"), ("power", "Active"), ("points", POINTS)):
        with open(os.path.join(tv, name), "w") as f:
            f.write(text + "\n")
    luna = os.path.join(tmp, "luna-send")
    with open(luna, "w") as f:
        f.write(FAKE_LUNA.replace("@D@", tv))
    os.chmod(luna, 0o755)
    os_info = os.path.join(tmp, "os_info.json")
    with open(os_info, "w") as f:
        f.write('{"webos_manufacturing_version": "99.99.99"}')
    port = free_port()
    config = os.path.join(tmp, "sat-tv.toml")
    with open(config, "w") as f:
        f.write(f"""
mode = "wake"
[device]
name = "e2e-tv"
mac = "02:00:00:00:00:e4"
bind = "127.0.0.1"
port = {port}
noise_psk_file = "psk-tv"
[mic]
source = "file"
path = "mic-tv.raw"
[sink]
type = "null"
[log]
level = "info"
[power]
luna_send = "{luna}"
[tv_control]
power_off = true
play_state = true
volume_limit = 40
[safety]
validated_firmware = ["03.30.74"]
firmware_info = "{os_info}"
""")
    calls_path = os.path.join(tv, "calls")

    def calls() -> str:
        return open(calls_path).read() if os.path.exists(calls_path) else ""

    async def called(needle: str, timeout: float = TIMEOUT) -> bool:
        return await until_true(lambda: needle in calls(), timeout)

    sat = Satellite(binary, config, os.path.join(tmp, "satellite-tv.log"))
    await sat.start()
    ha = FakeHA(port, psk)
    try:
        cli = await ha.connect()
        _info, ents, _ = await cli.device_info_and_list_entities()
        by = {e.object_id: e for e in ents}
        want = {"tv_screen", "tv_power", "tv_mute", "tv_volume", "tv_volume_up", "tv_volume_down", "tv_turn_on",
                "tv_input", "tv_app", "tv_refresh_lists", "tv_sound_output", "tv_play_state", "tv_power_state",
                "remote_power_off", "listening", "microphone_muted", "mute_microphone"}
        check("V1 entity list: every TV control entity, once", want <= set(by) and len(by) == len(ents),
              f"missing {want - set(by)}")
        check("V2 volume range ends at the configured cap (40; the TV reports 60), no unit; selects have no category",
              isinstance(by["tv_volume"], NumberInfo) and by["tv_volume"].max_value == 40.0
              and by["tv_volume"].unit_of_measurement == ""
              and list(by["tv_input"].options) == ["CanaryInput 1", "CanaryInput 2", "Other"]
              and list(by["tv_app"].options) == ["CanaryApp A", "CanaryApp B", "Other"]
              and by["tv_refresh_lists"].entity_category.name == "CONFIG"
              and by["tv_sound_output"].entity_category.name == "DIAGNOSTIC", "")
        states: dict[int, object] = {}
        cli.subscribe_states(lambda st: states.__setitem__(st.key, st))
        ha.subscribe_voice()

        def st(obj: str):
            v = states.get(by[obj].key)
            return None if v is None or getattr(v, "missing_state", False) else v.state

        ok = await until_true(lambda: st("tv_volume") == 12.0 and st("tv_mute") is False and st("tv_screen") is True
                              and st("tv_app") == "CanaryApp A" and st("tv_input") == "Other"
                              and st("tv_sound_output") == "external_arc" and st("tv_play_state") == "playing")
        check("V3 initial states from the TV", ok,
              " ".join(f"{o}={st(o)}" for o in ("tv_volume", "tv_mute", "tv_screen", "tv_app", "tv_input")))
        check("V4 tv_power reads unknown while the remote power-off select is on \"Turn off normally\"",
              st("tv_power") is None)
        check("V5 getters ran on a terminal, startup wrote nothing, getSystemInfo never called",
              "NOT-A-TTY" not in calls() and "apiadapter/system/getSystemInfo" not in calls()
              and "CANARYSERIAL" not in sat.text()
              and not any(w in calls().split("SUB")[0] for w in ("setVolume", "setMute", "switchInput", "turnOff")),
              calls()[:300])
        log = sat.text()
        check("V6 firmware not verified: warning logged, TV control still on",
              "tv_control: firmware 99.99.99 is not verified; TV control runs anyway" in log
              and "SETTERS ARE OFF" not in log, "")

        # tv_power off while not in keep-listening: refused, no key.
        cli.switch_command(by["tv_power"].key, False)
        await asyncio.sleep(0.6)
        check("V7 power-off refused without the always-on mode: no key sent",
              "controls/power" not in calls() and await sat.wait_log("tv_power: refused"), calls())
        cli.select_command(by["remote_power_off"].key, "Keep listening (screen off)")
        ok = await until_true(lambda: st("tv_power") is True)
        check("V8 select on Keep listening: tv_power shows on", ok, str(st("tv_power")))
        cli.switch_command(by["tv_power"].key, False)
        check("V9 power-off sends the remote's power key", await called("luna://com.webos.service.networkinput/controls/power {}"),
              calls())

        # Volume: coalesced set, clamp, read back.
        cli.number_command(by["tv_volume"].key, 20.0)
        check("V10 volume set calls setVolume with the value", await called('master/setVolume {"volume":20}'), calls())
        ok = await until_true(lambda: st("tv_volume") == 20.0)
        check("V11 the state follows the TV's own value", ok, str(st("tv_volume")))
        await asyncio.sleep(1.2)
        # With a cap the slider ends at the cap and the server ignores a value
        # above it (the controller's own clamp is a second line, unit-tested).
        cli.number_command(by["tv_volume"].key, 55.0)
        await asyncio.sleep(1.0)
        check("V12a a value above the configured volume_limit (40) is ignored", 'setVolume {"volume":55}' not in calls()
              and "outside 0..=40" in sat.text(), calls())
        cli.number_command(by["tv_volume"].key, 40.0)
        check("V12 the cap itself can be set", await called('master/setVolume {"volume":40}'), calls())
        await asyncio.sleep(1.2)
        cli.button_command(by["tv_volume_up"].key)
        await asyncio.sleep(0.6)
        check("V13 volume up at the limit is refused (no call)", "master/volumeUp" not in calls(), calls())
        cli.button_command(by["tv_volume_down"].key)
        ok = await called("master/volumeDown") and await until_true(lambda: st("tv_volume") == 39.0)
        check("V14 volume down works", ok, str(st("tv_volume")))

        cli.switch_command(by["tv_mute"].key, True)
        ok = await called('apiadapter/audio/setMute {"mute":true}') and await until_true(lambda: st("tv_mute") is True)
        check("V15 mute", ok, calls())

        # Input and app.
        cli.select_command(by["tv_input"].key, "Other")
        await asyncio.sleep(0.5)
        check("V16 choosing Other is refused", "switchInput" not in calls(), calls())
        cli.select_command(by["tv_input"].key, "CanaryInput 2")
        ok = await called('apiadapter/tv/switchInput {"inputId":"HDMI_2"}') and await until_true(
            lambda: st("tv_input") == "CanaryInput 2")
        check("V17 input switch, state follows the foreground app", ok, str(st("tv_input")))
        await asyncio.sleep(2.2)
        cli.select_command(by["tv_app"].key, "CanaryApp B")
        ok = await called('applicationManager/launch {"id":"app.b"}') and await until_true(
            lambda: st("tv_app") == "CanaryApp B")
        check("V18 app launch, state follows", ok, str(st("tv_app")))

        # Screen and turn on.
        cli.switch_command(by["tv_screen"].key, False)
        ok = await called("turnOffScreen") and await until_true(lambda: st("tv_screen") is False)
        check("V19 screen off, state follows the power state", ok, str(st("tv_screen")))
        check("V20 volume still readable and settable in Screen Off", st("tv_volume") == 39.0, str(st("tv_volume")))
        cli.button_command(by["tv_turn_on"].key)
        ok = await called("turnOnScreen") and await until_true(lambda: st("tv_screen") is True)
        check("V21 turn on from Screen Off", ok, str(st("tv_screen")))

        # Refresh: a new app is installed, HA lists again.
        with open(os.path.join(tv, "points"), "w") as f:
            f.write(POINTS.replace('{"id":"app.b"', '{"id":"app.c","title":"CanaryApp C"},{"id":"app.b"') + "\n")
        n_before = sat.text().count("HA connected")
        cli.button_command(by["tv_refresh_lists"].key)
        ok = await until_true(lambda: sat.text().count("HA connected") > n_before or "HA disconnected" in sat.text()[-3000:], 15)
        await asyncio.sleep(0.5)
        ha2 = FakeHA(port, psk)
        cli2 = await ha2.connect()
        _i, ents2, _ = await cli2.device_info_and_list_entities()
        app2 = [e for e in ents2 if e.object_id == "tv_app"][0]
        check("V22 list refresh: HA reconnects and lists the new app", ok and "CanaryApp C" in list(app2.options),
              str(list(app2.options)))
        await cli2.disconnect()

        log = sat.text()
        check("V22b refresh logs a one-line summary with counts",
              "tv_refresh_lists: done: 2 inputs, 3 apps" in log, log[-600:])
        check("V22c a getVolume subscription without the flag is kept and confirmed by a pushed update",
              "reply has no subscribed flag; keeping the subscription" in log
              and "update was pushed on the volume subscription" in log, log[-1500:])
        leaks = [w for w in ("CanaryApp", "CanaryInput", "app.hdmi", "app.a", "app.b", "app.c", "/secret.png") if w in log]
        check("V23 no app title, input label or app id in the log at info", not leaks, f"{leaks}")
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 8)
        left = await until_true(lambda: True)
        check("V24 clean exit", rc == 0 and left, f"rc={rc}")
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()


COMPAT_LUNA = r"""#!/bin/sh
# A fake luna-send for the compatibility e2e: the model query and a power
# state; every call is appended to $D/calls. Needs a terminal on stdout.
D="@D@"
if [ "$1" = "-i" ]; then
  printf '{"returnValue":true,"state":"Active","subscribed":true}\n'
  exec sleep 600
fi
echo "$5 $6" >> "$D/calls"
case "$5" in
  *tv.systemproperty/getSystemInfo)
    printf '{"returnValue":true,"modelName":"%s","firmwareVersion":"03.30.74","serialNumber":"CANARYSERIAL0123456789","sdkVersion":"8.3.0"}\n' "$(cat "$D/model")" ;;
  *) echo '{"returnValue":true}' ;;
esac
"""


def write_const_mic(path: str, seconds: float) -> None:
    """A stuck signal: every sample the same (the self-check must fail it)."""
    with open(path, "wb") as f:
        f.write(struct.pack("<ii", 1000, 1000) * int(16000 * seconds))


async def scenario_compat(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: firmware status, self-check, report files (ADR 0006) ==")
    import json as _json
    import urllib.parse as _up

    cdir = os.path.join(tmp, "compat")
    os.mkdir(cdir)
    run_dir = os.path.join(cdir, "run")
    with open(os.path.join(cdir, "model"), "w") as f:
        f.write("OLED42C34LA\n")
    luna = os.path.join(cdir, "luna-send")
    with open(luna, "w") as f:
        f.write(COMPAT_LUNA.replace("@D@", cdir))
    os.chmod(luna, 0o755)
    write_mic(os.path.join(cdir, "mic-good.raw"), 30.0)
    write_const_mic(os.path.join(cdir, "mic-stuck.raw"), 30.0)
    os_info = os.path.join(cdir, "os_info.json")
    fw_seen = os.path.join(cdir, "fw_seen")

    def set_fw(v: str) -> None:
        with open(os_info, "w") as f:
            f.write('{"webos_manufacturing_version": "%s", "webos_release": "8.3.0", "webos_build_datetime": "20240226022140"}' % v)

    async def boot(tag: str, mic: str, fw: str, model: str, policy: str = "warn", validated: str = "[]"):
        set_fw(fw)
        with open(os.path.join(cdir, "model"), "w") as f:
            f.write(model + "\n")
        psk = base64.b64encode(os.urandom(32)).decode()
        with open(os.path.join(cdir, "psk"), "w") as f:
            f.write(psk + "\n")
        os.chmod(os.path.join(cdir, "psk"), 0o600)
        port = free_port()
        config = os.path.join(cdir, f"sat-{tag}.toml")
        with open(config, "w") as f:
            f.write(f"""
mode = "wake"
[device]
name = "e2e-compat"
friendly_name = "Grandma Living Room TV"
mac = "02:00:00:00:00:c1"
bind = "127.0.0.1"
port = {port}
noise_psk_file = "psk"
[mic]
source = "file"
path = "{mic}"
loop = true
pause_when_idle = false
[sink]
type = "null"
[log]
level = "info"
[power]
luna_send = "{luna}"
[tv_control]
enabled = false
[safety]
kill_switch = "{cdir}/kill"
mute_file = "{cdir}/muted"
firmware_policy = "{policy}"
validated_firmware = {validated}
firmware_info = "{os_info}"
firmware_seen_file = "{fw_seen}"
self_check_window_s = 2
[report]
dir = "{run_dir}"
""")
        sat = Satellite(binary, config, os.path.join(tmp, f"satellite-compat-{tag}.log"))
        await sat.start()
        ha = FakeHA(port, psk)
        cli = await ha.connect()
        _i, ents, _ = await cli.device_info_and_list_entities()
        by = {e.object_id: e for e in ents}
        states: dict[int, object] = {}
        cli.subscribe_states(lambda st: states.__setitem__(st.key, st))

        def st(obj: str):
            v = states.get(by[obj].key)
            return None if v is None or getattr(v, "missing_state", False) else v.state

        return sat, ha, cli, by, st, port

    async def stop(sat: Satellite) -> int:
        sat.proc.send_signal(signal.SIGTERM)
        rc = await asyncio.wait_for(sat.proc.wait(), 8)
        sat.log.close()
        return rc

    def read(name: str) -> str:
        with open(os.path.join(run_dir, name)) as f:
            return f.read()

    sat = None
    try:
        # ---- run 1: a verified TV, a good microphone ----
        sat, ha, cli, by, st, port = await boot("ok", os.path.join(cdir, "mic-good.raw"), "03.30.74", "OLED42C34LA")
        want = {"firmware_verified", "capture_problem", "firmware_changed", "tv_firmware", "tv_model", "compat_status",
                "selfcheck", "report_status", "selfcheck_run", "firmware_ack"}
        check("K1 every compatibility entity is listed once; no report_compat button",
              want <= set(by) and "report_compat" not in by, f"missing {want - set(by)}")
        check("K2 categories: sensors diagnostic, buttons config, problem class",
              all(by[o].entity_category.name == "DIAGNOSTIC" for o in want - {"selfcheck_run", "firmware_ack"})
              and all(by[o].entity_category.name == "CONFIG" for o in ("selfcheck_run", "firmware_ack"))
              and by["capture_problem"].device_class == "problem" and isinstance(by["selfcheck_run"], ButtonInfo), "")
        ok = await until_true(lambda: (st("selfcheck") or "").startswith("passed"), 12)
        check("K3 the self-check passes on the normal stream (sine at -30 dBFS)", ok, str(st("selfcheck")))
        check("K4 status: model and firmware read, verified, no problem, no change",
              (st("tv_firmware"), st("tv_model"), st("compat_status"), st("firmware_verified"),
               st("capture_problem"), st("firmware_changed"))
              == ("03.30.74", "OLED42C34LA", "verified", True, False, False),
              str([st(o) for o in sorted(want)]))
        ok = await until_true(lambda: (st("report_status") or "").startswith("ready"), 5)
        check("K5 report_status says where the report is", ok, str(st("report_status")))
        check("K6 the firmware is remembered at the first start",
              open(fw_seen).read().strip() == "03.30.74", "")

        files = {n: read(n) for n in ("report.json", "report.short.txt", "report.full.txt", "report.url", "compat.json")}
        url = files["report.url"].strip()
        parsed = _up.urlparse(url)
        q = _up.parse_qs(parsed.query)
        check("K7 the link targets the issue form of the repository, with title and report",
              parsed.netloc == "github.com" and parsed.path == "/pando85/webos-voice-satellite/issues/new"
              and q.get("template") == ["compatibility-report.yml"] and len(url) <= 1500
              and q["report"][0].strip() == files["report.short.txt"].strip()
              and q["title"][0].startswith("[Compat] OLED42C34LA fw 03.30.74: "), url[:200])
        rj = _json.loads(files["report.json"])
        cj = _json.loads(files["compat.json"])
        check("K8 report.json and compat.json parse and agree with the entities",
              rj["url"] == url and rj["verdict"] in ("works", "partly works") and cj["compat_status"] == "verified"
              and cj["tv_model"] == "OLED42C34LA" and cj["firmware_verified"] is True and cj["policy"] == "warn",
              str(cj))
        check("K9 the report has the checks, levels and the power state from the TV",
              "checks: C1=pass" in files["report.full.txt"] and "levels: window=2s" in files["report.full.txt"]
              and "states=Active" in files["report.full.txt"], files["report.full.txt"][:600])
        secrets = ["CANARYSERIAL", "Grandma", "e2e-compat", "02:00:00:00:00:c1", "02%3A00", "127.0.0.1", "psk"]
        everything = "\n".join(files.values())
        check("K10 no serial, device name, MAC, address or key in the report files; no serial in the log",
              not [s for s in secrets if s in everything] and "CANARYSERIAL" not in sat.text(),
              str([s for s in secrets if s in everything]))
        check("K11 only modelName was asked of the model query; apiadapter/system/getSystemInfo never called",
              '{"keys":["modelName"]}' in open(os.path.join(cdir, "calls")).read()
              and "apiadapter/system/getSystemInfo" not in open(os.path.join(cdir, "calls")).read(), "")

        # The HA button reruns on the open stream (no second open).
        opens = sat.text().count("microphone open")
        cli.button_command(by["selfcheck_run"].key)
        ok = await until_true(lambda: st("selfcheck") == "checking", 4)
        check("K12 the Re-run button starts a new measurement (checking)", ok, str(st("selfcheck")))
        ok = await until_true(lambda: (st("selfcheck") or "").startswith("passed"), 12)
        check("K13 ...and it passes again without opening the PCM a second time",
              ok and sat.text().count("microphone open") == opens and "re-run requested (Home Assistant)" in sat.text(),
              str(st("selfcheck")))
        # ctl.sh's trigger file.
        with open(os.path.join(run_dir, "selfcheck-run"), "w"):
            pass
        ok = await until_true(lambda: "re-run requested (ctl.sh)" in sat.text(), 4)
        check("K14 the selfcheck-run trigger file is acted on and deleted",
              ok and not os.path.exists(os.path.join(run_dir, "selfcheck-run")), "")
        await until_true(lambda: (st("selfcheck") or "").startswith("passed"), 12)
        rc = await stop(sat)
        await ha.cli.disconnect()
        check("K15 clean exit", rc == 0, f"rc={rc}")

        # ---- run 2: the firmware changed under the daemon ----
        sat, ha, cli, by, st, port = await boot("chg", os.path.join(cdir, "mic-good.raw"), "03.30.80", "OLED42C34LA")
        ok = await until_true(lambda: st("compat_status") == "firmware changed" and st("firmware_changed") is True, 8)
        check("K16 a firmware that differs from the remembered one: status firmware changed, marker on",
              ok and "FIRMWARE CHANGED: 03.30.74 -> 03.30.80" in sat.text(),
              str(st("compat_status")))
        ok = await until_true(lambda: (st("selfcheck") or "").startswith("passed"), 12)
        check("K17 the microphone kept running and the self-check ran", ok and st("firmware_verified") is False,
              str(st("selfcheck")))
        check("K18 the marker is not written by the change itself", open(fw_seen).read().strip() == "03.30.74", "")
        cli.button_command(by["firmware_ack"].key)
        ok = await until_true(lambda: st("firmware_changed") is False
                              and st("compat_status") == "series verified (firmware not yet reported)", 6)
        check("K19 Acknowledge firmware change clears it; same series, firmware not yet reported",
              ok and open(fw_seen).read().strip() == "03.30.80", str(st("compat_status")))
        with open(os.path.join(run_dir, "firmware-ack"), "w"):
            pass
        ok = await until_true(lambda: not os.path.exists(os.path.join(run_dir, "firmware-ack")), 4)
        check("K20 the firmware-ack trigger file is consumed too", ok, "")
        rc = await stop(sat)
        await ha.cli.disconnect()

        # ---- run 3: not verified at all, and a stuck microphone ----
        sat, ha, cli, by, st, port = await boot("bad", os.path.join(cdir, "mic-stuck.raw"), "99.99.99", "SOME-OTHER-TV")
        listening_seen = await until_true(lambda: st("listening") is True, 6)
        check("K21 an unverified firmware does not keep the microphone off: it opens (listening)",
              listening_seen and "FIRMWARE GUARD" not in sat.text()
              and "not verified; the microphone starts anyway" in sat.text(), str(st("listening")))
        ok = await until_true(lambda: st("capture_problem") is True and st("compat_status") == "capture problem", 12)
        check("K22 a stuck signal fails the self-check: capture problem, status, failed text",
              ok and (st("selfcheck") or "").startswith("failed:"), str(st("selfcheck")))
        ok = await until_true(lambda: st("microphone_muted") is True and st("listening") is False, 4)
        check("K23 the microphone is closed (muted on, listening off); HA's own mute switch is untouched",
              ok and st("mute_microphone") is not True, str(st("microphone_muted")))
        ok = await until_true(lambda: os.path.exists(os.path.join(run_dir, "report.json")), 4)
        rj = _json.loads(read("report.json"))
        check("K24 the report says it does not work and names the model and firmware in the title",
              ok and rj["verdict"] == "does not work" and rj["title"] == "[Compat] SOME-OTHER-TV fw 99.99.99: does not work",
              str(rj.get("title")))
        check("K25 the log names the problem and the way to report it",
              "capture problem" in sat.text() and "Report compatibility" in sat.text(), "")
        rc = await stop(sat)
        await ha.cli.disconnect()

        # ---- run 4: strict policy is the old gate (a fresh install: no marker) ----
        os.remove(fw_seen)
        sat, ha, cli, by, st, port = await boot("strict", os.path.join(cdir, "mic-good.raw"), "99.99.99", "SOME-OTHER-TV",
                                                policy="strict", validated='["03.30.74"]')
        # A file mic is never gated (the gate is for the ALSA microphone; the
        # unit test unverified_firmware_does_not_gate_the_mic_unless_strict
        # covers the closed mic): the strict policy shows in the log and status.
        ok = await until_true(lambda: st("compat_status") == "not verified", 5)
        check("K26 firmware_policy = strict: not verified, policy strict, the gate named in the log",
              ok and '"policy":"strict"' in read("compat.json").replace(" ", "")
              and "firmware gate (not applied to a file mic)" in sat.text(), str(st("compat_status")))
        check("K27 ...and no FIRMWARE GUARD error for a file mic",
              "FIRMWARE GUARD" not in sat.text(), "")
        rc = await stop(sat)
        await ha.cli.disconnect()
        check("K28 clean exit after the strict run", rc == 0, f"rc={rc}")
    finally:
        if sat is not None and sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, help="path to the webos-satellite binary")
    args = parser.parse_args()
    failed = False
    with tempfile.TemporaryDirectory(prefix="webos-sat-e2e-") as tmp:
        try:
            await scenario(args.binary, tmp)
            await scenario_timers(args.binary, tmp)
            await scenario_alarm_cap(args.binary, tmp)
            await scenario_pairing(args.binary, tmp)
            await scenario_pairing_window(args.binary, tmp)
            await scenario_aec(args.binary, tmp)
            await scenario_tv_control(args.binary, tmp)
            await scenario_compat(args.binary, tmp)
        except Exception as err:  # noqa: BLE001
            failed = True
            print(f"FAIL  {type(err).__name__}: {err}", flush=True)
            for name in ("satellite.log", "satellite-timers.log", "satellite-cap.log",
                         "satellite-nosound.log", "satellite-notimers.log", "satellite-pairing.log",
                         "satellite-pairing.log.2", "satellite-window.log", "satellite-aec.log", "satellite-tv.log",
                         "satellite-compat-ok.log", "satellite-compat-chg.log",
                         "satellite-compat-bad.log", "satellite-compat-strict.log"):
                log = os.path.join(tmp, name)
                if os.path.exists(log):
                    print(f"---- {name} ----")
                    print(open(log).read())
    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print(f"\n{passed} checks passed, {'FAILED' if failed else 'all OK'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
