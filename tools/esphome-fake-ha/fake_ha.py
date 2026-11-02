#!/usr/bin/env python3
"""Fake Home Assistant for the webos-esphome-api crate.

Drives the `fake_device` example with aioesphomeapi 46.6.0 (the library HA
2026.10.4 uses) the way HA's ESPHome integration does, and checks both the
protocol side (what HA would see) and the app events the device emits.

Usage (from the repository root):
    tools/esphome-fake-ha/run.sh
or, with a prebuilt binary:
    tools/esphome-fake-ha/.venv/bin/python tools/esphome-fake-ha/fake_ha.py --device PATH

No real HA, network or TV is used: the device listens on 127.0.0.1 on an
ephemeral port. Noise keys are generated per run and never written to disk.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import os
import re
import sys
import time
from dataclasses import dataclass, field

from aioesphomeapi import (
    APIClient,
    InvalidEncryptionKeyAPIError,
    MediaPlayerInfo,
    MediaPlayerFormatPurpose,
    MediaPlayerEntityState,
    RequiresEncryptionAPIError,
    SwitchInfo,
    SwitchState,
    VoiceAssistantEventType as Ev,
    VoiceAssistantTimerEventType as TimerEv,
)
from aioesphomeapi.model import APIVersion, VoiceAssistantCommandFlag

USE_WAKE_WORD = int(VoiceAssistantCommandFlag.USE_WAKE_WORD)
TIMEOUT = 5.0
RESULTS: list[tuple[str, bool, str]] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    RESULTS.append((name, ok, detail))
    print(f"{'PASS' if ok else 'FAIL'}  {name}{('  -- ' + detail) if detail else ''}", flush=True)
    if not ok:
        raise AssertionError(f"{name}: {detail}")


class Device:
    """The fake_device process; collects its `EVENT ...` lines."""

    def __init__(self, binary: str, args: list[str], psk: str | None) -> None:
        self.binary = binary
        self.args = args
        self.psk = psk
        self.events: list[str] = []
        self.cursor = 0
        self.port = 0
        self._changed = asyncio.Event()

    async def start(self) -> None:
        env = dict(os.environ)
        env.pop("WEBOS_ESPHOME_PSK", None)
        if self.psk:
            env["WEBOS_ESPHOME_PSK"] = self.psk
        env.setdefault("FAKE_DEVICE_LOG", "info")
        self.log = open(os.path.join(LOG_DIR, f"device-{len(os.listdir(LOG_DIR))}.log"), "w")
        self.proc = await asyncio.create_subprocess_exec(
            self.binary, "--port", "0", *self.args,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=self.log, env=env,
        )
        line = (await asyncio.wait_for(self.proc.stdout.readline(), TIMEOUT)).decode()
        m = re.match(r"LISTENING .*:(\d+)", line)
        assert m, f"unexpected first line {line!r}"
        self.port = int(m.group(1))
        self._reader = asyncio.create_task(self._read())

    async def _read(self) -> None:
        while line := await self.proc.stdout.readline():
            text = line.decode().rstrip()
            if text.startswith("EVENT "):
                self.events.append(text[6:])
                self._changed.set()

    async def wait_event(self, prefix: str, timeout: float = TIMEOUT) -> str:
        """Waits for the next event (after the cursor) starting with prefix."""
        deadline = time.monotonic() + timeout
        while True:
            for i in range(self.cursor, len(self.events)):
                if self.events[i].startswith(prefix):
                    self.cursor = i + 1
                    return self.events[i]
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"no event {prefix!r}; got {self.events[self.cursor:]}")
            self._changed.clear()
            try:
                await asyncio.wait_for(self._changed.wait(), remaining)
            except TimeoutError:
                pass

    def count_since(self, prefix: str, start: int) -> int:
        return sum(1 for e in self.events[start:] if e.startswith(prefix))

    async def command(self, cmd: str) -> None:
        self.proc.stdin.write(f"{cmd}\n".encode())
        await self.proc.stdin.drain()

    async def stop(self) -> None:
        if self.proc.returncode is None:
            self.proc.terminate()
            await self.proc.wait()
        self.log.close()


@dataclass
class FakeHA:
    """An APIClient plus the voice-assistant callbacks HA registers."""

    port: int
    psk: str | None = None
    starts: asyncio.Queue = field(default_factory=asyncio.Queue)
    stops: asyncio.Queue = field(default_factory=asyncio.Queue)
    audio: asyncio.Queue = field(default_factory=asyncio.Queue)
    announce_finished: asyncio.Queue = field(default_factory=asyncio.Queue)
    states: list = field(default_factory=list)
    stopped: asyncio.Event = field(default_factory=asyncio.Event)

    async def connect(self) -> APIClient:
        self.cli = APIClient("127.0.0.1", self.port, "", client_info="fake-ha", noise_psk=self.psk)

        async def on_stop(expected: bool) -> None:
            self.stopped.set()

        await asyncio.wait_for(self.cli.connect(on_stop=on_stop, login=True), TIMEOUT)
        return self.cli

    def subscribe_voice(self) -> None:
        async def handle_start(conversation_id, flags, audio_settings, wake_word_phrase):
            await self.starts.put((conversation_id, flags, wake_word_phrase))
            return 0  # API audio (no UDP port)

        async def handle_stop(abort: bool) -> None:
            await self.stops.put(abort)

        async def handle_audio(data: bytes, data2: bytes | None) -> None:
            await self.audio.put(data)

        async def handle_announcement_finished(msg) -> None:
            await self.announce_finished.put(msg.success)

        self.cli.subscribe_voice_assistant(
            handle_start=handle_start,
            handle_stop=handle_stop,
            handle_audio=handle_audio,
            handle_announcement_finished=handle_announcement_finished,
        )

    def event(self, ev: Ev, data: dict[str, str] | None = None) -> None:
        self.cli.send_voice_assistant_event(ev, data or {})

    async def next_start(self, timeout: float = TIMEOUT):
        return await asyncio.wait_for(self.starts.get(), timeout)

    async def expect_no_start(self, seconds: float) -> bool:
        try:
            await asyncio.wait_for(self.starts.get(), seconds)
            return False
        except TimeoutError:
            return True

    def drain_audio(self) -> list[bytes]:
        out = []
        while not self.audio.empty():
            out.append(self.audio.get_nowait())
        return out

    async def stream_tts(self, chunks: int) -> None:
        """What HA's _stream_tts_audio does: STREAM_START, PCM chunks, STREAM_END."""
        self.event(Ev.VOICE_ASSISTANT_TTS_STREAM_START)
        for i in range(chunks):
            self.cli.send_voice_assistant_audio(bytes([i % 256]) * 1024)
            await asyncio.sleep(0.005)
        self.event(Ev.VOICE_ASSISTANT_TTS_STREAM_END)


async def until_true(pred, timeout: float = TIMEOUT) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if pred():
            return True
        await asyncio.sleep(0.02)
    return pred()


async def scenario_plaintext_speaker(binary: str) -> None:
    """Plaintext, default features (SPEAKER: TTS streamed over the API), wake word mode."""
    print("\n== scenario A: plaintext, SPEAKER streaming, server-side wake word ==")
    dev = Device(binary, ["--mode", "wake", "--mac", "02:00:00:00:00:0a"], None)
    await dev.start()
    ha = FakeHA(dev.port)
    try:
        cli = await ha.connect()
        check("A1 hello: API version 1.14", cli.api_version == APIVersion(1, 14), str(cli.api_version))
        await dev.wait_event("HaConnected { client_info: \"fake-ha\"")

        info, entities, _services = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("A2 device info", info.name == "fake-tv" and info.mac_address == "02:00:00:00:00:0A"
              and info.friendly_name == "Fake TV Satellite" and info.manufacturer == "webos-voice-satellite"
              and not info.uses_password and not info.api_encryption_supported,
              f"{info.name} {info.mac_address} {info.manufacturer}")
        check("A3 voice feature flags = VOICE_ASSISTANT|SPEAKER|API_AUDIO|ANNOUNCE", flags == 23, str(flags))
        mps = [e for e in entities if isinstance(e, MediaPlayerInfo)]
        sws = [e for e in entities if isinstance(e, SwitchInfo)]
        fmt = mps[0].supported_formats[0] if mps and mps[0].supported_formats else None
        check("A4 media_player with ANNOUNCEMENT format flac/22050/1ch/2B",
              len(mps) == 1 and fmt is not None and fmt.purpose == MediaPlayerFormatPurpose.ANNOUNCEMENT
              and (fmt.format, fmt.sample_rate, fmt.num_channels, fmt.sample_bytes) == ("flac", 22050, 1, 2),
              str(fmt))
        check("A5 mute switch entity", len(sws) == 1 and sws[0].name == "Mute microphone", str(sws))
        mute_key = sws[0].key

        cli.subscribe_states(ha.states.append)
        cli.subscribe_home_assistant_states_and_services(
            on_state=lambda *a: None, on_service_call=lambda *a: None,
            on_state_sub=lambda *a: None, on_state_request=lambda *a: None)
        ok = await until_true(lambda: any(isinstance(s, MediaPlayerEntityState) for s in ha.states)
                              and any(isinstance(s, SwitchState) for s in ha.states))
        check("A6 initial states (media player idle, switch off)", ok, str(ha.states))

        conf = await cli.get_voice_assistant_configuration(5)
        check("A7 voice config: no on-device wake words",
              conf.available_wake_words == [] and conf.active_wake_words == [] and conf.max_active_wake_words == 0)

        ha.subscribe_voice()
        conv, flags, phrase = await ha.next_start()
        check("A8 device requests pipeline with USE_WAKE_WORD", flags & USE_WAKE_WORD and conv == "" and phrase is None,
              f"flags={flags} conv={conv!r}")
        await dev.wait_event("StartStreaming { wake_word: true")
        await asyncio.sleep(0.6)
        chunks = ha.drain_audio()
        check("A9 mic audio streamed: 1024-byte chunks, non-silent",
              len(chunks) >= 10 and all(len(c) == 1024 for c in chunks) and any(any(c) for c in chunks),
              f"{len(chunks)} chunks")

        # Full run, as HA's pipeline reports it.
        ha.event(Ev.VOICE_ASSISTANT_RUN_START)
        ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_START)
        ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_END)
        await dev.wait_event("WakeWordDetected")
        ha.event(Ev.VOICE_ASSISTANT_STT_START)
        ha.event(Ev.VOICE_ASSISTANT_STT_VAD_START)
        ha.event(Ev.VOICE_ASSISTANT_STT_VAD_END)
        abort = await asyncio.wait_for(ha.stops.get(), TIMEOUT)
        check("A10 STT_VAD_END: device ends audio gracefully (end=true)", abort is False)
        await dev.wait_event("StopStreaming")
        await asyncio.sleep(0.2)
        ha.drain_audio()
        await asyncio.sleep(0.2)
        check("A11 no mic audio after end of speech", ha.drain_audio() == [])
        ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "enciende la luz"})
        ha.event(Ev.VOICE_ASSISTANT_INTENT_START)
        ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "conv-1", "continue_conversation": "0",
                                                 "speech": "Hecho"})
        ha.event(Ev.VOICE_ASSISTANT_TTS_START, {"text": "Hecho"})
        ha.event(Ev.VOICE_ASSISTANT_TTS_END, {"url": "http://10.42.0.99:8123/api/tts_proxy/abc.wav"})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        start_idx = dev.cursor
        await dev.wait_event('Transcript("enciende la luz")')
        await dev.wait_event('IntentEnd { speech: "Hecho", continue_conversation: false }')
        await dev.wait_event("RunEnd")
        await ha.stream_tts(20)
        await dev.wait_event("TtsStreamStart { sample_rate: 16000, channels: 1, sample_width: 2 }")
        await dev.wait_event("TtsStreamEnd")
        n_audio = dev.count_since("TtsAudio(len=1024)", start_idx)
        check("A12 SPEAKER: TTS stream forwarded to app (20 x 1024 B), URL not used",
              n_audio == 20 and dev.count_since("TtsUrl", start_idx) == 0, f"{n_audio} chunks")
        conv, flags, _ = await ha.next_start()
        check("A13 re-armed after playback with wake word + conversation id",
              flags & USE_WAKE_WORD and conv == "conv-1", f"flags={flags} conv={conv!r}")
        check("A14 SPEAKER: no AnnounceFinished for streamed TTS (HA finishes it itself)",
              ha.announce_finished.empty())

        # Error from HA -> backoff, no hot loop.
        await dev.wait_event("StartStreaming")
        t0 = time.monotonic()
        ha.event(Ev.VOICE_ASSISTANT_RUN_START)
        ha.event(Ev.VOICE_ASSISTANT_ERROR, {"code": "wake-engine-missing", "message": "No wake word engine"})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        await dev.wait_event('Error { code: "wake-engine-missing"')
        conv, flags, _ = await ha.next_start(timeout=10)
        delay = time.monotonic() - t0
        check("A15 wake-engine-missing: retry after backoff (>= 1.8 s), not immediately",
              delay >= 1.8 and flags & USE_WAKE_WORD, f"{delay:.2f} s")

        # continue_conversation -> follow-up without wake word, same conversation.
        await dev.wait_event("StartStreaming")
        ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_END)
        ha.event(Ev.VOICE_ASSISTANT_STT_VAD_END)
        ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "pon un temporizador"})
        ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "conv-1", "continue_conversation": "1",
                                                 "speech": "Cuantos minutos?"})
        ha.event(Ev.VOICE_ASSISTANT_TTS_END, {"url": "http://x/y.wav"})
        await ha.stream_tts(3)
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        conv, flags, _ = await ha.next_start()
        check("A16 continue_conversation: follow-up starts at STT with same conversation id",
              not flags & USE_WAKE_WORD and conv == "conv-1", f"flags={flags} conv={conv!r}")
        await dev.wait_event("StartStreaming { wake_word: false")
        ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "cinco"})
        ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "conv-1", "continue_conversation": "0",
                                                 "speech": ""})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)  # no TTS ("nevermind")
        conv, flags, _ = await ha.next_start()
        check("A17 run without TTS re-arms the wake word immediately", bool(flags & USE_WAKE_WORD), f"flags={flags}")
        await dev.wait_event("StartStreaming")
        while not ha.stops.empty():
            ha.stops.get_nowait()

        # Announcement while armed: abort the wake word run, play, re-arm.
        res = await cli.send_voice_assistant_announcement_await_response(
            "http://127.0.0.1:9/chime.flac", 10, "Hola", preannounce_media_id="")
        check("A18 announce: AnnounceFinished(success)", res.success is True)
        await dev.wait_event('Announce { url: "http://127.0.0.1:9/chime.flac", preannounce_url: None, text: "Hola"')
        abort = await asyncio.wait_for(ha.stops.get(), TIMEOUT)
        check("A19 announce aborts the streaming wake word run (start=false)", abort is True)
        conv, flags, _ = await ha.next_start()
        check("A20 re-armed after announcement", bool(flags & USE_WAKE_WORD))
        await dev.wait_event("StartStreaming")

        res = await cli.send_voice_assistant_announcement_await_response(
            "http://127.0.0.1:9/q.flac", 10, "Pregunta", start_conversation=True)
        conv, flags, _ = await ha.next_start()
        check("A21 start_conversation: run starts at STT after the announcement",
              res.success and not flags & USE_WAKE_WORD, f"flags={flags}")
        await dev.wait_event("StartStreaming { wake_word: false")
        ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "si"})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        await ha.next_start()
        await dev.wait_event("StartStreaming { wake_word: true")
        while not ha.stops.empty():
            ha.stops.get_nowait()

        # TIMERS is not advertised (A3), so HA would never forward timers; a
        # stray timer event must be ignored.
        before = len(dev.events)
        cli.send_voice_assistant_timer_event(TimerEv.VOICE_ASSISTANT_TIMER_STARTED, "stray", "x", 30, 30, True)
        await asyncio.sleep(0.4)
        check("A21b timer event without TIMERS is ignored", dev.count_since("Timer", before) == 0)

        # Mute switch from HA.
        ha.states.clear()
        cli.switch_command(mute_key, True)
        abort = await asyncio.wait_for(ha.stops.get(), TIMEOUT)
        await dev.wait_event("MuteChanged(true)")
        ok = await until_true(lambda: any(isinstance(s, SwitchState) and s.state for s in ha.states))
        check("A22 mute: run aborted, switch state on, no restart", abort is True and ok
              and await ha.expect_no_start(1.0))
        cli.switch_command(mute_key, False)
        conv, flags, _ = await ha.next_start()
        check("A23 unmute re-arms the wake word", bool(flags & USE_WAKE_WORD))

        await cli.disconnect()
        await dev.wait_event("HaDisconnected")
        check("A24 disconnect reported to app", True)
    finally:
        await dev.stop()


async def scenario_noise_url(binary: str) -> None:
    """Noise, URL mode (no SPEAKER), push-to-talk, client replacement."""
    print("\n== scenario B: Noise, URL TTS mode, push-to-talk ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    dev = Device(binary, ["--mode", "ptt", "--features", "53"], psk)  # VA|API_AUDIO|ANNOUNCE|START_CONV
    await dev.start()
    try:
        try:
            await FakeHA(dev.port).connect()
            check("B1 plaintext client refused", False, "connected")
        except RequiresEncryptionAPIError:
            check("B1 plaintext client refused with RequiresEncryptionAPIError", True)
        try:
            await FakeHA(dev.port, base64.b64encode(os.urandom(32)).decode()).connect()
            check("B2 wrong key refused", False, "connected")
        except InvalidEncryptionKeyAPIError:
            check("B2 wrong key refused with InvalidEncryptionKeyAPIError", True)

        ha = FakeHA(dev.port, psk)
        cli = await ha.connect()
        info = await cli.device_info()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("B3 noise session: device info", flags == 53 and info.api_encryption_supported, f"flags={flags}")
        ha.subscribe_voice()
        check("B4 push-to-talk mode: no automatic start", await ha.expect_no_start(1.0))
        await dev.command("ptt")
        conv, flags, _ = await ha.next_start()
        check("B5 push-to-talk start without USE_WAKE_WORD", not flags & USE_WAKE_WORD, f"flags={flags}")
        await dev.wait_event("StartStreaming { wake_word: false")
        await asyncio.sleep(0.3)
        check("B6 encrypted mic audio received", len(ha.drain_audio()) >= 5)
        ha.event(Ev.VOICE_ASSISTANT_RUN_START)
        ha.event(Ev.VOICE_ASSISTANT_STT_START)
        ha.event(Ev.VOICE_ASSISTANT_STT_VAD_END)
        ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "hola"})
        ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "c2", "continue_conversation": "0",
                                                 "speech": "Hola"})
        ha.event(Ev.VOICE_ASSISTANT_TTS_END, {"url": "http://ha.local:8123/api/tts_proxy/t.flac"})
        ha.event(Ev.VOICE_ASSISTANT_RUN_END)
        await dev.wait_event('TtsUrl("http://ha.local:8123/api/tts_proxy/t.flac")')
        success = await asyncio.wait_for(ha.announce_finished.get(), TIMEOUT)
        check("B7 URL mode: TtsUrl to app, AnnounceFinished(success) after playback", success is True)
        check("B8 push-to-talk is one-shot", await ha.expect_no_start(1.0))

        # A second client replaces the first.
        ha2 = FakeHA(dev.port, psk)
        await ha2.connect()
        await dev.wait_event("HaDisconnected")
        await dev.wait_event("HaConnected")
        ok = await until_true(lambda: ha.stopped.is_set())
        check("B9 new connection replaces the old one", ok)
        await ha2.cli.disconnect()
    finally:
        await dev.stop()


async def scenario_timers(binary: str) -> None:
    """TIMERS enabled (31 = default | TIMERS): HA's timer events reach the app."""
    print("\n== scenario C: TIMERS feature, timer events from HA ==")
    dev = Device(binary, ["--mode", "none", "--features", "31", "--mac", "02:00:00:00:00:0c"], None)
    os.environ["FAKE_DEVICE_LOG"] = "debug"  # debug lines print every AppEvent via Debug
    try:
        await dev.start()
    finally:
        del os.environ["FAKE_DEVICE_LOG"]
    ha = FakeHA(dev.port)
    try:
        cli = await ha.connect()
        info = await cli.device_info()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("C1 device info advertises TIMERS", flags == 31 and flags & 8, str(flags))
        ha.subscribe_voice()
        await dev.wait_event("HaConnected")

        # What HA's handle_timer_event sends for: set a timer, add a minute,
        # pause, resume, finish; and a second timer that is cancelled.
        T = TimerEv
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_STARTED, "t-1", "pasta", 300, 300, True)
        ev = await dev.wait_event("Timer ")
        check("C2 STARTED reaches the app with all fields",
              ev == 'Timer { event: started, id: "t-1", name: "pasta", total_seconds: 300, '
                    'seconds_left: 300, is_active: true }', ev)
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_UPDATED, "t-1", "pasta", 360, 342, True)
        ev = await dev.wait_event("Timer ")
        check("C3 UPDATED (time added)", "event: updated" in ev and "total_seconds: 360" in ev
              and "seconds_left: 342" in ev, ev)
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_UPDATED, "t-1", "pasta", 360, 330, False)
        ev = await dev.wait_event("Timer ")
        check("C4 UPDATED (paused) carries is_active=false", "is_active: false" in ev and "seconds_left: 330" in ev, ev)
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_UPDATED, "t-1", "pasta", 360, 330, True)
        await dev.wait_event("Timer ")
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_FINISHED, "t-1", "pasta", 360, 0, False)
        ev = await dev.wait_event("Timer ")
        check("C5 FINISHED", "event: finished" in ev and "seconds_left: 0" in ev and '"t-1"' in ev, ev)

        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_STARTED, "t-2", None, 60, 60, True)
        ev = await dev.wait_event("Timer ")
        check("C6 a timer without a name", 'name: ""' in ev, ev)
        cli.send_voice_assistant_timer_event(T.VOICE_ASSISTANT_TIMER_CANCELLED, "t-2", None, 60, 41, True)
        ev = await dev.wait_event("Timer ")
        check("C7 CANCELLED", "event: cancelled" in ev and "seconds_left: 41" in ev, ev)

        # The device log shows ids and seconds but never the timer name.
        await cli.disconnect()
        await dev.wait_event("HaDisconnected")
        await dev.stop()
        log_text = open(dev.log.name).read()
        check("C8 device log never contains the timer name", "pasta" not in log_text and "t-1" in log_text,
              "debug log checked")
    finally:
        await dev.stop()


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--device", required=True, help="path to the fake_device binary")
    args = parser.parse_args()
    failed = False
    for scenario in (scenario_plaintext_speaker, scenario_noise_url, scenario_timers):
        try:
            await scenario(args.device)
        except Exception as err:  # noqa: BLE001
            failed = True
            print(f"FAIL  {scenario.__name__}: {type(err).__name__}: {err}", flush=True)
    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print(f"\n{passed} checks passed, {'FAILED' if failed else 'all scenarios OK'} (device logs: {LOG_DIR})")
    return 1 if failed else 0


LOG_DIR = os.environ.get("FAKE_HA_LOG_DIR") or os.path.join(
    os.environ.get("TMPDIR", "/tmp"), f"esphome-fake-ha-{os.getpid()}")
os.makedirs(LOG_DIR, exist_ok=True)

if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
