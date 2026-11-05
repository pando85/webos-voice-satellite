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
import logging
import math
import os
import re
import sys
import time
from dataclasses import dataclass, field

from aioesphomeapi import (
    ZERO_NOISE_PSK,
    APIClient,
    APIConnectionError,
    BinarySensorInfo,
    BinarySensorState,
    ButtonInfo,
    EntityCategory,
    NumberInfo,
    NumberMode,
    NumberState,
    SelectInfo,
    SelectState,
    SwitchInfo,
    SwitchState,
    TextSensorInfo,
    TextSensorState,
    ConnectionClosedEvent,
    DisconnectReason,
    InvalidEncryptionKeyAPIError,
    APIVersion,
    MediaPlayerCommand,
    MediaPlayerEntityFeature,
    MediaPlayerInfo,
    MediaPlayerFormatPurpose,
    MediaPlayerEntityState,
    MediaPlayerState,
    RequiresEncryptionAPIError,
    SwitchInfo,
    SwitchState,
    VoiceAssistantEventType as Ev,
    VoiceAssistantTimerEventType as TimerEv,
)
from aioesphomeapi.model import APIVersion, VoiceAssistantCommandFlag

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fake_proxy  # noqa: E402  (HA's ffmpeg proxy and Music Assistant's flow stream; milestone M1)

USE_WAKE_WORD = int(VoiceAssistantCommandFlag.USE_WAKE_WORD)
TIMEOUT = 5.0
RESULTS: list[tuple[str, bool, str]] = []
# Checks that need satellite features that do not exist yet (music playback,
# milestones M2 to M5 of docs/design/music-playback.md). Printed, never failed.
PENDING: list[tuple[str, str]] = []

# Feature flags of the satellite's media player (design section B).
MEDIA_FLAGS_MUSIC_OFF = 5124  # STOP + VOLUME_SET + VOLUME_STEP (today, and `[music] enabled = false`)
MEDIA_FLAGS_MUSIC_ON = 153093  # + PLAY_MEDIA, PAUSE, PLAY, BROWSE_MEDIA
MEDIA_FLAGS_MUSIC_ANNOUNCE = 1201669  # + MEDIA_ANNOUNCE (`[music] announce = true`)
# What HA 2026.10.4's ESPHome media player maps (everything else is unmapped).
HA_MEDIA_STATES = {
    MediaPlayerState.IDLE: "idle",
    MediaPlayerState.PLAYING: "playing",
    MediaPlayerState.PAUSED: "paused",
    MediaPlayerState.ON: "on",
    MediaPlayerState.OFF: "off",
}


def pending(name: str, why: str) -> None:
    PENDING.append((name, why))
    print(f"PEND  {name}  -- {why}", flush=True)


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
    # What HA's assist_satellite holds for the device: a pipeline task that is
    # alive from a start request until a stop/abort or RUN_END, and the number
    # of audio chunks received so far.
    session_open: bool = False
    audio_count: int = 0
    # The satellite's media player entity and the states HA received for it
    # (filled by `load_media_player` and `track_media_state`).
    media: MediaPlayerInfo | None = None
    media_states: list = field(default_factory=list)
    device_name: str = "fake-device"

    async def connect(self) -> APIClient:
        self.cli = APIClient("127.0.0.1", self.port, "", client_info="fake-ha", noise_psk=self.psk)

        async def on_stop(expected: bool) -> None:
            self.stopped.set()

        await asyncio.wait_for(self.cli.connect(on_stop=on_stop, login=True), TIMEOUT)
        return self.cli

    def subscribe_voice(self) -> None:
        async def handle_start(conversation_id, flags, audio_settings, wake_word_phrase):
            self.session_open = True
            await self.starts.put((conversation_id, flags, wake_word_phrase))
            return 0  # API audio (no UDP port)

        async def handle_stop(abort: bool) -> None:
            self.session_open = False
            await self.stops.put(abort)

        async def handle_audio(data: bytes, data2: bytes | None) -> None:
            self.audio_count += 1
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
        if ev == Ev.VOICE_ASSISTANT_RUN_END:
            self.session_open = False
        self.cli.send_voice_assistant_event(ev, data or {})

    async def announce_like_ha(self, url: str, text: str = "", hang_after: float = 1.5,
                               timeout: float = TIMEOUT, **kw):
        """What HA's assist_satellite does for assist_satellite.announce.

        `async_internal_announce` first awaits `_cancel_running_pipeline()`.
        With a wake word pipeline open, the Wyoming wake word client's cancel
        runs `finally: await audio_task`, which waits for the NEXT audio chunk
        from the device (HA 2026.10.4, read from its source). A device that
        keeps such a session open while sending no audio therefore blocks the
        announcement (seen on c3-b with the microphone busy). Returns the
        announcement result, or None when HA would hang (no chunk within
        `hang_after` seconds): the harness reports that as a failure.
        """
        if self.session_open:
            seen = self.audio_count
            if not await until_true(lambda: self.audio_count > seen, hang_after):
                return None
        return await asyncio.wait_for(
            self.cli.send_voice_assistant_announcement_await_response(
                url, timeout, text, preannounce_media_id="", **kw),
            timeout + 1)

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

    # -- media player: what HA's ESPHome integration reads and sends ----------

    async def load_media_player(self, object_id: str = "media_player") -> MediaPlayerInfo:
        """Lists the entities like HA does and keeps the satellite's media player."""
        info, entities, _ = await self.cli.device_info_and_list_entities()
        self.device_name = info.name
        self.media = next(e for e in entities if isinstance(e, MediaPlayerInfo) and e.object_id == object_id)
        return self.media

    def media_flags(self) -> MediaPlayerEntityFeature:
        """The feature flags as HA 2026.10.4 (API 1.14) computes them."""
        return MediaPlayerEntityFeature(self.media.feature_flags_compat(APIVersion(1, 14)))

    def media_formats(self) -> list:
        """`supported_formats` in the advertised order (format, sample_rate,
        num_channels, purpose, sample_bytes)."""
        return list(self.media.supported_formats)

    def track_media_state(self) -> None:
        """Records every state of the satellite player in `media_states`."""
        def on_state(st) -> None:
            if isinstance(st, MediaPlayerEntityState) and st.key == self.media.key:
                self.media_states.append(st)

        self.cli.subscribe_states(on_state)

    @property
    def media_state(self) -> MediaPlayerEntityState | None:
        return self.media_states[-1] if self.media_states else None

    async def wait_media_state(self, state: MediaPlayerState, timeout: float = TIMEOUT) -> bool:
        return await until_true(lambda: self.media_state is not None and self.media_state.state == state, timeout)

    @staticmethod
    def ha_state_name(state: MediaPlayerState) -> str | None:
        """HA's entity state for an ESPHome state; None where HA has no mapping
        (`ANNOUNCING` is never sent by the satellite)."""
        return HA_MEDIA_STATES.get(state)

    def play_media(self, url: str, announce: bool | None = None) -> None:
        """`media_player.play_media`; `announce` None leaves the field out like HA does."""
        self.cli.media_player_command(self.media.key, media_url=url, announcement=announce)

    def enqueue_media(self, url: str, announce: bool | None = None) -> None:
        """`media_player.play_media` with `enqueue: add`: HA sends ENQUEUE plus the URL
        even when MEDIA_ENQUEUE is not offered (it only checks PLAY_MEDIA)."""
        self.cli.media_player_command(self.media.key, command=MediaPlayerCommand.ENQUEUE, media_url=url,
                                      announcement=announce)

    def media_pause(self) -> None:
        self.cli.media_player_command(self.media.key, command=MediaPlayerCommand.PAUSE)

    def media_resume(self) -> None:
        self.cli.media_player_command(self.media.key, command=MediaPlayerCommand.PLAY)

    def media_stop(self) -> None:
        self.cli.media_player_command(self.media.key, command=MediaPlayerCommand.STOP)

    def volume_set(self, level: float) -> None:
        self.cli.media_player_command(self.media.key, volume=level)

    def volume_up(self) -> None:
        self.cli.media_player_command(self.media.key, command=MediaPlayerCommand.VOLUME_UP)

    def volume_down(self) -> None:
        self.cli.media_player_command(self.media.key, command=MediaPlayerCommand.VOLUME_DOWN)

    def send_unadvertised(self) -> list[MediaPlayerCommand]:
        """Mute, unmute, toggle, repeat and clear-playlist: not offered, HA does not
        send them; sent once to prove the satellite ignores them."""
        sent = [MediaPlayerCommand.MUTE, MediaPlayerCommand.UNMUTE, MediaPlayerCommand.TOGGLE,
                MediaPlayerCommand.REPEAT_ONE, MediaPlayerCommand.REPEAT_OFF, MediaPlayerCommand.CLEAR_PLAYLIST]
        for c in sent:
            self.cli.media_player_command(self.media.key, command=c)
        return sent

    def play_media_like_ha(self, proxy: "fake_proxy.HaProxy", announce: bool = False,
                           signal: "fake_proxy.Signal | None" = None) -> str:
        """`media_player.play_media` from the HA UI or a script: the format comes from the
        advertised list (first DEFAULT entry, or first ANNOUNCEMENT when announcing), HA's
        proxy gets a conversion (and a slot), the satellite gets the proxy URL."""
        url = proxy.url_for_play_media(self.device_name, self.media_formats(), announce, signal)
        self.play_media(url, announce=True if announce else None)
        return url

    def play_media_like_ma(self, flow: "fake_proxy.MaFlow", source_rate: int = 44100, **kw) -> str:
        """Music Assistant's `play_media`: its own flow-mode URL (`bypass_proxy`) in the format it
        picks from the advertised list; `kw` goes to `MaFlow.create_stream` (tracks, endless ...)."""
        url = flow.url_for_play_media(self.media_formats(), source_rate, **kw)
        self.play_media(url)
        return url


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


async def scenario_capture_live(binary: str) -> None:
    """A microphone that delivers no audio (busy PCM) must not leave a session open in HA."""
    print("\n== scenario L: no audio from the microphone, no wake word session, announcements work ==")
    dev = Device(binary, ["--mode", "wake", "--mac", "02:00:00:00:00:1c", "--no-capture"], None)
    await dev.start()
    ha = FakeHA(dev.port)
    try:
        cli = await ha.connect()
        await dev.wait_event("HaConnected { client_info: \"fake-ha\"")
        await cli.device_info_and_list_entities()
        ha.subscribe_voice()
        check("L1 busy mic from the start: no pipeline start is requested",
              await ha.expect_no_start(1.5) and not ha.session_open)

        res = await ha.announce_like_ha("http://127.0.0.1:9/a.flac", "hola")
        check("L2 announce with no session open completes promptly", res is not None and res.success is True,
              str(res))
        await dev.wait_event('Announce { url: "http://127.0.0.1:9/a.flac"')
        check("L3 nothing is aborted (there was no run) and no session opens after the announcement",
              ha.stops.empty() and await ha.expect_no_start(1.0))

        await dev.command("capture")
        conv, flags, _ = await ha.next_start()
        check("L4 audio flows: the wake word session opens (USE_WAKE_WORD)", bool(flags & USE_WAKE_WORD))
        await dev.wait_event("StartStreaming { wake_word: true")
        await asyncio.sleep(0.4)
        check("L5 audio reaches HA", len(ha.drain_audio()) >= 5)
        res = await ha.announce_like_ha("http://127.0.0.1:9/b.flac", "hola")
        check("L6 announce while capturing completes (the next chunk lets HA's cancel finish)",
              res is not None and res.success is True, str(res))
        abort = await asyncio.wait_for(ha.stops.get(), TIMEOUT)
        check("L7 the run is aborted for the announcement", abort is True)
        await ha.next_start()  # re-armed after the announcement
        await dev.wait_event("StartStreaming { wake_word: true")
        while not ha.stops.empty():
            ha.stops.get_nowait()

        # The microphone stops delivering mid-session (LG hands-free grabs the PCM).
        await dev.command("nocapture")
        abort = await asyncio.wait_for(ha.stops.get(), TIMEOUT)
        check("L8 capture lost: the session is aborted at once (HA ends its audio stream)",
              abort is True and not ha.session_open)
        await dev.wait_event("StopStreaming")
        await asyncio.sleep(0.3)
        ha.drain_audio()
        await asyncio.sleep(0.3)
        check("L9 no audio and no new session while capture is lost",
              ha.drain_audio() == [] and await ha.expect_no_start(1.0))
        res = await ha.announce_like_ha("http://127.0.0.1:9/c.flac", "hola")
        check("L10 announce after the loss completes promptly", res is not None and res.success is True, str(res))
        check("L11 announce with start_conversation without audio opens no session",
              (await ha.announce_like_ha("http://127.0.0.1:9/d.flac", "q?", start_conversation=True)) is not None
              and await ha.expect_no_start(1.0))
        await dev.command("capture")
        conv, flags, _ = await ha.next_start()
        check("L12 capture back: the wake word session returns", bool(flags & USE_WAKE_WORD))
        await dev.wait_event("StartStreaming { wake_word: true")

        # Many announcements in a row: HA cancels the idle wake word run (it ends
        # with RUN_END, no error) and announces. Each one must leave the wake
        # word session back within a moment: an announcement is no failed run,
        # so the retry backoff never grows (it used to: 250 ms doubling to 60 s).
        worst = 0.0
        for n in range(12):
            while not ha.starts.empty():
                ha.starts.get_nowait()
            ha.event(Ev.VOICE_ASSISTANT_RUN_END)
            t = time.monotonic()
            res = await ha.announce_like_ha("http://127.0.0.1:9/e.flac", "hola")
            if res is None or not res.success:
                check(f"L13 announcement {n} completes", False, str(res))
            _conv, flags, _ = await ha.next_start(timeout=3)
            worst = max(worst, time.monotonic() - t)
            if not flags & USE_WAKE_WORD:
                check(f"L13 announcement {n} re-arms with the wake word", False, f"flags={flags}")
            await dev.wait_event("StartStreaming { wake_word: true")
        check("L13 12 announcements in a row: the wake word session is back within 1 s each time",
              worst < 1.0, f"worst {worst:.2f} s")

        await cli.disconnect()
        await dev.wait_event("HaDisconnected")
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

        # The media player's stop is reported to the app even with nothing to
        # stop in the voice layer, so the app can stop its own sounds (the
        # timer alarm).
        _i, ents, _ = await cli.device_info_and_list_entities()
        mp = [e for e in ents if isinstance(e, MediaPlayerInfo)][0]
        cli.media_player_command(mp.key, command=MediaPlayerCommand.STOP)
        ev = await dev.wait_event("MediaStop")
        check("C7a media player STOP reaches the app as MediaStop", ev == "MediaStop", ev)

        # The device log shows ids and seconds but never the timer name.
        await cli.disconnect()
        await dev.wait_event("HaDisconnected")
        await dev.stop()
        log_text = open(dev.log.name).read()
        check("C8 device log never contains the timer name", "pasta" not in log_text and "t-1" in log_text,
              "debug log checked")
    finally:
        await dev.stop()


async def scenario_entities(binary: str) -> None:
    """number, button and binary_sensor entities: listing, states, commands."""
    print("\n== scenario H: number / button / binary_sensor entities ==")
    dev = Device(binary, ["--mode", "none", "--demo-entities", "--mac", "02:00:00:00:00:0d"], None)
    await dev.start()
    ha = FakeHA(dev.port)
    try:
        cli = await ha.connect()
        _info, entities, _ = await cli.device_info_and_list_entities()
        nums = [e for e in entities if isinstance(e, NumberInfo)]
        btns = {e.object_id: e for e in entities if isinstance(e, ButtonInfo)}
        bins = [e for e in entities if isinstance(e, BinarySensorInfo)]
        n = nums[0] if nums else None
        check("H1 number listed with range, step, unit, class, mode",
              len(nums) == 1 and (n.object_id, n.min_value, n.max_value, n.step, n.unit_of_measurement,
                                  n.device_class, n.mode)
              == ("demo_volume", 0.0, 100.0, 1.0, "%", "volume", NumberMode.SLIDER)
              and n.entity_category == EntityCategory.NONE, str(n))
        check("H2 buttons listed with their category and class",
              set(btns) == {"demo_press", "demo_config_press"}
              and btns["demo_config_press"].entity_category == EntityCategory.CONFIG
              and btns["demo_config_press"].device_class == "restart"
              and btns["demo_press"].entity_category == EntityCategory.NONE, str(btns))
        check("H3 binary sensor listed (diagnostic, class running)",
              len(bins) == 1 and bins[0].object_id == "demo_state" and bins[0].device_class == "running"
              and bins[0].entity_category == EntityCategory.DIAGNOSTIC, str(bins))
        states: dict[int, object] = {}
        cli.subscribe_states(lambda s: states.__setitem__(s.key, s))
        ok = await until_true(lambda: n.key in states and bins[0].key in states)
        check("H4 initial states are missing_state (unknown), not 0 or off",
              ok and states[n.key].missing_state and states[bins[0].key].missing_state,
              f"{states.get(n.key)} {states.get(bins[0].key)}")

        await dev.command("number 37")
        await dev.command("binary on")
        ok = await until_true(lambda: isinstance(states[n.key], NumberState) and not states[n.key].missing_state
                              and states[n.key].state == 37.0
                              and isinstance(states[bins[0].key], BinarySensorState)
                              and states[bins[0].key].state is True and not states[bins[0].key].missing_state)
        check("H5 states pushed on change (number 37, binary on)", ok,
              f"{states[n.key]} {states[bins[0].key]}")
        await dev.command("binary off")
        ok = await until_true(lambda: states[bins[0].key].state is False)
        check("H6 binary sensor off pushed", ok)
        await dev.command("number unknown")
        ok = await until_true(lambda: states[n.key].missing_state)
        check("H7 a state can go back to unknown", ok)

        cli.number_command(n.key, 55.0)
        ev = await dev.wait_event("NumberChanged")
        check("H8 number command reaches the app", ev == 'NumberChanged { object_id: "demo_volume", value: 55.0 }', ev)
        await asyncio.sleep(0.2)
        check("H9 the command is not echoed as a state (the app owns the truth)",
              states[n.key].missing_state, str(states[n.key]))
        cli.number_command(n.key, 250.0)
        await asyncio.sleep(0.3)
        check("H10 an out-of-range command is rejected (no event)",
              dev.count_since("NumberChanged", 0) == 1, str(dev.events))
        cli.button_command(btns["demo_press"].key)
        ev = await dev.wait_event("ButtonPressed")
        check("H11 button press reaches the app", ev == 'ButtonPressed { object_id: "demo_press" }', ev)
        cli.button_command(btns["demo_config_press"].key)
        ev = await dev.wait_event("ButtonPressed")
        check("H12 second button press", "demo_config_press" in ev, ev)

        # A reconnect gets the current states again.
        await dev.command("number 12")
        await dev.command("binary on")
        await until_true(lambda: states[n.key].state == 12.0 and states[bins[0].key].state is True)
        await cli.disconnect()
        await dev.wait_event("HaDisconnected")
        ha2 = FakeHA(dev.port)
        cli2 = await ha2.connect()
        _i, entities2, _ = await cli2.device_info_and_list_entities()
        n2 = [e for e in entities2 if isinstance(e, NumberInfo)][0]
        b2 = [e for e in entities2 if isinstance(e, BinarySensorInfo)][0]
        states2: dict[int, object] = {}
        cli2.subscribe_states(lambda s: states2.__setitem__(s.key, s))
        ok = await until_true(lambda: n2.key in states2 and b2.key in states2)
        check("H13 states re-sent on reconnect",
              ok and states2[n2.key].state == 12.0 and states2[b2.key].state is True
              and n2.key == n.key and b2.key == bins[0].key, f"{states2}")
        await cli2.disconnect()
    finally:
        await dev.stop()


async def scenario_generic_entities(binary: str) -> None:
    """switch, text_sensor and an application-owned select: listing, states, commands, options."""
    print("\n== scenario I: switch / text_sensor / app-owned select entities ==")
    dev = Device(binary, ["--mode", "none", "--demo-entities", "--mac", "02:00:00:00:00:0e"], None)
    await dev.start()
    ha = FakeHA(dev.port)
    try:
        cli = await ha.connect()
        _info, entities, _ = await cli.device_info_and_list_entities()
        sws = {e.object_id: e for e in entities if isinstance(e, SwitchInfo)}
        txt = [e for e in entities if isinstance(e, TextSensorInfo) and e.object_id == "demo_text"]
        sels = [e for e in entities if isinstance(e, SelectInfo) and e.object_id == "demo_select"]
        sw = sws.get("demo_switch")
        check("I1 generic switch listed next to the mute switch (no category, class switch)",
              sw is not None and "mute_microphone" in sws and sw.entity_category == EntityCategory.NONE
              and sw.device_class == "switch" and sws["mute_microphone"].entity_category == EntityCategory.CONFIG,
              str(sws))
        check("I2 text sensor listed as a diagnostic",
              len(txt) == 1 and txt[0].entity_category == EntityCategory.DIAGNOSTIC, str(txt))
        check("I3 select listed with its options, no category",
              len(sels) == 1 and list(sels[0].options) == ["App A", "App B", "Other"]
              and sels[0].entity_category == EntityCategory.NONE, str(sels))
        sel = sels[0]
        states: dict[int, object] = {}
        cli.subscribe_states(lambda st: states.__setitem__(st.key, st))
        ok = await until_true(lambda: sw.key in states and txt[0].key in states and sel.key in states)
        check("I4 initial states are missing_state (unknown) for all three",
              ok and states[sw.key].missing_state and states[txt[0].key].missing_state
              and states[sel.key].missing_state, f"{states}")

        await dev.command("switch on")
        await dev.command("text external_arc")
        await dev.command("select App B")
        ok = await until_true(lambda: isinstance(states[sw.key], SwitchState) and states[sw.key].state is True
                              and not states[sw.key].missing_state
                              and isinstance(states[txt[0].key], TextSensorState)
                              and states[txt[0].key].state == "external_arc"
                              and isinstance(states[sel.key], SelectState) and states[sel.key].state == "App B")
        check("I5 states pushed on change (switch on, text, select)", ok, f"{states}")

        cli.switch_command(sw.key, False)
        ev = await dev.wait_event("SwitchChanged")
        check("I6 switch command reaches the app", ev == 'SwitchChanged { object_id: "demo_switch", state: false }', ev)
        await asyncio.sleep(0.2)
        check("I7 the command is not echoed as a state (the app owns the truth)",
              states[sw.key].state is True, str(states[sw.key]))
        cli.select_command(sel.key, "App A")
        ev = await dev.wait_event("SelectChanged")
        check("I8 select command reaches the app", 'object_id: "demo_select"' in ev and "App A" in ev, ev)
        await asyncio.sleep(0.2)
        check("I9 the select command is not echoed either", states[sel.key].state == "App B", str(states[sel.key]))
        cli.select_command(sel.key, "No such app")
        await asyncio.sleep(0.3)
        check("I10 an invalid option is rejected (no event) and the state is re-sent",
              dev.count_since("SelectChanged", 0) == 1 and states[sel.key].state == "App B", str(dev.events))
        await dev.command("switch unknown")
        await dev.command("select unknown")
        await dev.command("text unknown")
        ok = await until_true(lambda: states[sw.key].missing_state and states[sel.key].missing_state
                              and states[txt[0].key].missing_state)
        check("I11 all three can go back to unknown", ok, f"{states}")

        # New options: HA is disconnected so it lists again.
        await dev.command("switch on")
        await dev.command("options App A, App B, App C, Other")
        await dev.wait_event("HaDisconnected")
        ha2 = FakeHA(dev.port)
        cli2 = await ha2.connect()
        _i, entities2, _ = await cli2.device_info_and_list_entities()
        sel2 = [e for e in entities2 if isinstance(e, SelectInfo) and e.object_id == "demo_select"][0]
        check("I12 a changed option list closes the connection; the next listing has the new options and the same key",
              list(sel2.options) == ["App A", "App B", "App C", "Other"] and sel2.key == sel.key, str(sel2))
        states2: dict[int, object] = {}
        cli2.subscribe_states(lambda st: states2.__setitem__(st.key, st))
        ok = await until_true(lambda: sel2.key in states2 and sw.key in states2)
        check("I13 after the change the select has no state until the app sets one; the switch state is kept",
              ok and states2[sel2.key].missing_state and states2[sw.key].state is True, f"{states2}")
        await cli2.disconnect()
    finally:
        await dev.stop()


async def provision_over_zero_psk(port: int, key: bytes) -> bool:
    """What HA 2026.10.4's _async_provision_key_over_noise does."""
    cli = APIClient("127.0.0.1", port, "", client_info="fake-ha-provision", noise_psk=ZERO_NOISE_PSK)
    try:
        await asyncio.wait_for(cli.connect(login=True), TIMEOUT)
        return await asyncio.wait_for(cli.noise_encryption_set_key(key), TIMEOUT)
    finally:
        await cli.disconnect(force=True)


async def fetch_device_info_like_ha(port: int, psk: str | None) -> str:
    """HA 2026.10.4 config_flow._fetch_device_info: returns an error key or "ok"."""
    cli = APIClient("127.0.0.1", port, "", noise_psk=psk)
    closed = False

    def on_closed(event: ConnectionClosedEvent) -> None:
        nonlocal closed
        if event.reason is DisconnectReason.PROVISIONING_CLOSED:
            closed = True

    cli.add_connection_closed_callback(on_closed)
    try:
        await asyncio.wait_for(cli.connect(), TIMEOUT)
        await asyncio.wait_for(cli.device_info(), TIMEOUT)
        return "ok"
    except RequiresEncryptionAPIError:
        return "requires_encryption_key"
    except InvalidEncryptionKeyAPIError:
        return "invalid_psk"
    except (APIConnectionError, TimeoutError):
        return "provisioning_closed" if closed else "connection_error"
    finally:
        await cli.disconnect(force=True)


async def clear_key_like_ha(port: int, psk: str) -> bool:
    """HA 2026.10.4 esphome/__init__.py _async_clear_dynamic_encryption_key
    (run when the config entry is deleted): connect with the entry's key,
    send an empty key, then disconnect() (DisconnectRequest, wait for the
    response)."""
    cli = APIClient("127.0.0.1", port, "", client_info="fake-ha-remove", noise_psk=psk)
    try:
        await asyncio.wait_for(cli.connect(), TIMEOUT)
        return await asyncio.wait_for(cli.noise_encryption_set_key(b""), TIMEOUT)
    finally:
        await cli.disconnect()


class ErrorLog(logging.Handler):
    """Collects aioesphomeapi log records at ERROR and above (HA logs
    "disconnect request failed" with a traceback when its disconnect races
    one from the device)."""

    def __init__(self) -> None:
        super().__init__(logging.ERROR)
        self.records: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.records.append(record.getMessage())

    def __enter__(self) -> "ErrorLog":
        logging.getLogger("aioesphomeapi").addHandler(self)
        return self

    def __exit__(self, *exc) -> None:
        logging.getLogger("aioesphomeapi").removeHandler(self)


class CoalescingProxy:
    """TCP proxy that hands the device's bytes to the client in batches:
    everything the device sends within `hold` seconds after a first byte is
    written in one go, so the client reads it in one `data_received`. That is
    what HA 2026.10.4 in its Kubernetes pod saw behind a router (P3 on c3-a):
    a HelloResponse and a DisconnectRequest sent back to back arrived
    together, and aioesphomeapi 46.6.0 then dropped the disconnect reason.
    Client-to-device bytes pass at once."""

    def __init__(self, target_port: int, hold: float = 0.05) -> None:
        self.target_port = target_port
        self.hold = hold
        self.port = 0
        self.batches = 0

    async def __aenter__(self) -> "CoalescingProxy":
        self.server = await asyncio.start_server(self._handle, "127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        return self

    async def __aexit__(self, *exc) -> None:
        self.server.close()
        await self.server.wait_closed()

    async def _handle(self, cr: asyncio.StreamReader, cw: asyncio.StreamWriter) -> None:
        dr, dw = await asyncio.open_connection("127.0.0.1", self.target_port)
        loop = asyncio.get_running_loop()

        async def up() -> None:
            try:
                while data := await cr.read(65536):
                    dw.write(data)
                    await dw.drain()
            finally:
                dw.close()

        async def down() -> None:
            try:
                eof = False
                while not eof and (data := await dr.read(65536)):
                    buf = bytearray(data)
                    deadline = loop.time() + self.hold
                    while (left := deadline - loop.time()) > 0:
                        try:
                            more = await asyncio.wait_for(dr.read(65536), left)
                        except TimeoutError:
                            break
                        if not more:
                            eof = True
                            break
                        buf += more
                    self.batches += 1
                    cw.write(bytes(buf))
                    await cw.drain()
            finally:
                cw.close()

        await asyncio.gather(up(), down(), return_exceptions=True)


async def scenario_pairing(binary: str) -> None:
    """Unpaired device; HA's provisioning flow sets a key over zero-PSK Noise."""
    print("\n== scenario D: pairing (HA-driven key provisioning) ==")
    key_dir = os.path.join(LOG_DIR, "pairing-d")
    os.makedirs(key_dir, exist_ok=True)
    key_file = os.path.join(key_dir, "noise_psk")
    dev = Device(binary, ["--mode", "wake", "--mac", "02:00:00:00:00:0d", "--pair-key-file", key_file], None)
    os.environ["FAKE_DEVICE_LOG"] = "trace"  # every message name and event
    try:
        await dev.start()
    finally:
        del os.environ["FAKE_DEVICE_LOG"]
    try:
        await dev.wait_event("Pairing(WindowOpened { seconds: 600 })")
        # HA's main connection: no key yet.
        ha = FakeHA(dev.port)
        cli = await ha.connect()
        info, entities, _ = await cli.device_info_and_list_entities()
        flags = info.voice_assistant_feature_flags_compat(cli.api_version)
        check("D1 unpaired: encryption supported + provisionable, no voice features, no entities",
              info.api_encryption_supported and info.api_encryption_provisionable and flags == 0
              and entities == [], f"flags={flags} entities={len(entities)}")
        ha.subscribe_voice()
        cli.subscribe_states(ha.states.append)
        ok = await ha.expect_no_start(1.0)
        check("D2 keyless session: no pipeline start, no audio, no states, app not told HA connected",
              ok and ha.audio.empty() and ha.states == [] and dev.count_since("StartStreaming", 0) == 0
              and dev.count_since("HaConnected", 0) == 0)
        new_key = base64.b64encode(os.urandom(32))
        ok = await asyncio.wait_for(cli.noise_encryption_set_key(new_key), TIMEOUT)
        check("D3 SetKey over plaintext refused (zero-PSK Noise only)",
              ok is False and not os.path.exists(key_file))
        await asyncio.sleep(1.1)  # one SetKey attempt per second
        ok = await provision_over_zero_psk(dev.port, new_key)
        check("D4 HA's zero-PSK provisioning: SetKey accepted", ok is True)
        await dev.wait_event('Pairing(Paired { client_info: "fake-ha-provision"')
        ok = await until_true(lambda: ha.stopped.is_set())
        check("D5 the keyless main session is closed after activation (HA reconnects)", ok)
        mode = os.stat(key_file).st_mode & 0o777
        with open(key_file, "rb") as f:
            stored = f.read()
        check("D6 key stored with mode 0600", mode == 0o600 and stored == new_key + b"\n", f"mode={mode:o}")

        ha2 = FakeHA(dev.port, new_key.decode())
        cli2 = await ha2.connect()
        info2, entities2, _ = await cli2.device_info_and_list_entities()
        flags2 = info2.voice_assistant_feature_flags_compat(cli2.api_version)
        check("D7 reconnect with the new key: full device, no longer provisionable",
              info2.api_encryption_supported and not info2.api_encryption_provisionable
              and flags2 == 23 and len(entities2) == 2, f"flags={flags2}")
        ha2.subscribe_voice()
        _conv, vflags, _ = await ha2.next_start()
        await asyncio.sleep(0.4)
        check("D8 audio flows only after pairing", bool(vflags & USE_WAKE_WORD) and len(ha2.drain_audio()) >= 5)
        await cli2.disconnect()
        await dev.wait_event("HaDisconnected")

        results = [await fetch_device_info_like_ha(dev.port, k) for k in
                   (None, base64.b64encode(os.urandom(32)).decode(), ZERO_NOISE_PSK)]
        check("D9 no key / wrong key / zero PSK refused as HA expects",
              results == ["requires_encryption_key", "invalid_psk", "invalid_psk"], str(results))
        try:
            await provision_over_zero_psk(dev.port, base64.b64encode(os.urandom(32)))
            check("D10 provisioning after pairing refused", False, "connected")
        except InvalidEncryptionKeyAPIError:
            check("D10 provisioning after pairing refused (HA: 'already has an encryption key')", True)

        await dev.command("reset-pairing")
        await dev.wait_event("Pairing(Reset)")
        res = await fetch_device_info_like_ha(dev.port, new_key.decode())
        check("D11 reset-pairing: key file deleted, old key refused, keyless allowed again",
              not os.path.exists(key_file) and res == "invalid_psk"
              and await fetch_device_info_like_ha(dev.port, None) == "ok", res)
        await dev.stop()
        log_text = open(dev.log.name).read()
        check("D12 device log never contains the key", new_key.decode() not in log_text
              and "key set by" in log_text)
    finally:
        await dev.stop()


async def scenario_pairing_window(binary: str) -> None:
    """The pairing window closes on timeout; HA's flow sees PROVISIONING_CLOSED."""
    print("\n== scenario E: pairing window timeout ==")
    key_file = os.path.join(LOG_DIR, "pairing-e", "noise_psk")
    os.makedirs(os.path.dirname(key_file), exist_ok=True)
    dev = Device(binary, ["--mode", "wake", "--mac", "02:00:00:00:00:0e", "--pair-key-file", key_file,
                          "--pair-window", "1.5"], None)
    await dev.start()
    try:
        check("E1 open window: HA's config flow reads device info", await fetch_device_info_like_ha(dev.port, None) == "ok")
        await dev.wait_event("Pairing(WindowClosed)", timeout=5)
        res = [await fetch_device_info_like_ha(dev.port, k) for k in (None, ZERO_NOISE_PSK)]
        check("E2 closed window: plaintext and zero-PSK get PROVISIONING_CLOSED",
              res == ["provisioning_closed", "provisioning_closed"], str(res))
        try:
            await provision_over_zero_psk(dev.port, base64.b64encode(os.urandom(32)))
            ok = False
        except APIConnectionError:
            ok = True
        check("E3 closed window: no key can be set", ok and not os.path.exists(key_file))
        await dev.command("pair")
        await dev.wait_event("Pairing(WindowOpened")
        check("E4 explicit pair command reopens the window",
              await fetch_device_info_like_ha(dev.port, None) == "ok")
    finally:
        await dev.stop()


async def scenario_window_behind_router(binary: str) -> None:
    """HA's config flow against a closed window with HA's network timing: the
    device's frames are delivered in batches (CoalescingProxy). With the
    DisconnectRequest right behind the HelloResponse, this returned
    connection_error (as the owner's HA did 4 of 4 on c3-a); the device now
    sends it after the client's next message."""
    print("\n== scenario F: closed window, HA's timing behind a router ==")
    key_file = os.path.join(LOG_DIR, "pairing-f", "noise_psk")
    os.makedirs(os.path.dirname(key_file), exist_ok=True)
    dev = Device(binary, ["--mode", "wake", "--mac", "02:00:00:00:00:0f", "--pair-key-file", key_file,
                          "--pair-closed"], None)
    await dev.start()
    try:
        async with CoalescingProxy(dev.port) as proxy:
            res = [await fetch_device_info_like_ha(proxy.port, k) for k in (None, ZERO_NOISE_PSK) * 3]
            check("F1 closed window through the batching proxy: provisioning_closed 6 of 6 (plaintext, zero-PSK)",
                  res == ["provisioning_closed"] * 6 and proxy.batches > 0, str(res))
        res = [await fetch_device_info_like_ha(dev.port, k) for k in (None, ZERO_NOISE_PSK)]
        check("F2 closed window, direct: provisioning_closed", res == ["provisioning_closed"] * 2, str(res))
    finally:
        await dev.stop()


async def scenario_key_clear(binary: str) -> None:
    """HA deletes the entry and clears the key it provisioned: accepted from
    the key holder only; unpaired with the window closed, also after a
    restart, until a local open."""
    print("\n== scenario G: key clear on entry deletion ==")
    key_file = os.path.join(LOG_DIR, "pairing-g", "noise_psk")
    os.makedirs(os.path.dirname(key_file), exist_ok=True)
    args = ["--mode", "wake", "--mac", "02:00:00:00:00:10", "--pair-key-file", key_file]
    dev = Device(binary, args, None)
    await dev.start()
    try:
        await dev.wait_event("Pairing(WindowOpened")
        key = base64.b64encode(os.urandom(32))
        check("G1 provisioned over zero-PSK", await provision_over_zero_psk(dev.port, key) is True)
        await dev.wait_event("Pairing(Paired")
        ha = FakeHA(dev.port, key.decode())
        await ha.connect()
        await ha.cli.disconnect()  # the entry is unloaded before it is removed
        await asyncio.sleep(1.1)  # one SetKey attempt per second
        with ErrorLog() as errors:
            ok = await clear_key_like_ha(dev.port, key.decode())
        check("G2 HA's clear on entry removal accepted (HA then drops its stored key)", ok is True)
        await dev.wait_event('Pairing(Cleared { client_info: "fake-ha-remove"')
        check("G3 key file deleted, marker written",
              not os.path.exists(key_file) and os.path.exists(key_file + ".cleared"))
        check("G4 HA's disconnect() after the reply answered cleanly (no error logged)",
              errors.records == [], str(errors.records))
        res = [await fetch_device_info_like_ha(dev.port, k) for k in (None, ZERO_NOISE_PSK, key.decode())]
        check("G5 unpaired, window closed: keyless refused, old key refused",
              res == ["provisioning_closed", "provisioning_closed", "invalid_psk"], str(res))
        await dev.stop()

        dev = Device(binary, args, None)
        await dev.start()
        await asyncio.sleep(0.3)
        res = await fetch_device_info_like_ha(dev.port, None)
        check("G6 restart (window opens at start otherwise): still closed",
              res == "provisioning_closed" and dev.count_since("Pairing(WindowOpened", 0) == 0, res)
        await dev.command("pair")
        await dev.wait_event("Pairing(WindowOpened")
        ok = await provision_over_zero_psk(dev.port, key)
        await dev.wait_event("Pairing(Paired")
        check("G7 local pair command: HA re-pairs with its stored key, marker removed",
              ok is True and not os.path.exists(key_file + ".cleared"))
        await dev.stop()

        fixed = base64.b64encode(os.urandom(32)).decode()
        dev = Device(binary, ["--mode", "wake", "--mac", "02:00:00:00:00:11"], fixed)
        await dev.start()
        ok = await clear_key_like_ha(dev.port, fixed)
        await asyncio.sleep(0.2)
        check("G8 a key fixed in the configuration is not cleared",
              ok is False and await fetch_device_info_like_ha(dev.port, fixed) == "ok")
    finally:
        await dev.stop()


async def scenario_two_media_players(binary: str) -> None:
    """The satellite's media player and an application-owned, control-only one (the TV)."""
    print("\n== scenario J: two media players (satellite reply volume + TV controls) ==")
    dev = Device(binary, ["--mode", "none", "--demo-entities", "--mac", "02:00:00:00:00:0f"], None)
    await dev.start()
    ha = FakeHA(dev.port)
    try:
        cli = await ha.connect()
        _info, entities, _ = await cli.device_info_and_list_entities()
        mps = {e.object_id: e for e in entities if isinstance(e, MediaPlayerInfo)}
        sat, tv = mps.get("media_player"), mps.get("demo_tv")
        check("J1 two media players with distinct keys; the satellite one keeps its object id",
              len(mps) == 2 and sat is not None and tv is not None and sat.key != tv.key, str(list(mps)))
        # What HA 2026.10.4's ESPHome media player does with the flags (API 1.14).
        feat = MediaPlayerEntityFeature
        tv_flags = feat(tv.feature_flags_compat(APIVersion(1, 14)))
        sat_flags = feat(sat.feature_flags_compat(APIVersion(1, 14)))
        check("J2 TV player: volume set/step, mute, turn on/off; no play media, announce, browse or source",
              tv_flags == (feat.VOLUME_SET | feat.VOLUME_STEP | feat.VOLUME_MUTE | feat.TURN_ON | feat.TURN_OFF)
              and not tv_flags & (feat.PLAY_MEDIA | feat.MEDIA_ANNOUNCE | feat.BROWSE_MEDIA | feat.SELECT_SOURCE),
              repr(tv_flags))
        check("J3 satellite player unchanged: stop + volume set/step, one ANNOUNCEMENT format",
              sat_flags == (feat.STOP | feat.VOLUME_SET | feat.VOLUME_STEP)
              and len(sat.supported_formats) == 1, repr(sat_flags))
        # HA's assist satellite takes the first ANNOUNCEMENT format of any
        # media player of the device (_update_tts_format): only the satellite has one.
        fmts = [f for p in (sat, tv) for f in p.supported_formats
                if f.purpose == MediaPlayerFormatPurpose.ANNOUNCEMENT]
        check("J4 the TV player has no audio format: HA's TTS/announcement format comes from the satellite player",
              not tv.supported_formats and len(fmts) == 1 and fmts[0].format == "flac", str(fmts))

        states: dict[int, object] = {}
        cli.subscribe_states(lambda st: states.__setitem__(st.key, st))
        ok = await until_true(lambda: sat.key in states)
        await asyncio.sleep(0.3)
        check("J5 the satellite player has a state; the TV player none until the app reports one",
              ok and tv.key not in states, str(states))
        await dev.command("mp playing 0.2 unmuted")
        ok = await until_true(lambda: tv.key in states and states[tv.key].state == MediaPlayerState.PLAYING)
        check("J6 TV player state pushed (playing, volume 0.2, not muted)",
              ok and abs(states[tv.key].volume - 0.2) < 1e-6 and states[tv.key].muted is False, str(states.get(tv.key)))
        await dev.command("mp off unknown muted")
        ok = await until_true(lambda: states[tv.key].state == MediaPlayerState.OFF)
        check("J7 off with an unknown volume (NaN, HA shows no level) and muted",
              ok and math.isnan(states[tv.key].volume) and states[tv.key].muted is True, str(states[tv.key]))

        # Commands to the TV player reach the app, routed by key, not echoed.
        start = len(dev.events)
        cli.media_player_command(tv.key, volume=0.5)
        ev = await dev.wait_event("MediaPlayerCommand")
        check("J8 volume set on the TV player", ev == 'MediaPlayerCommand { object_id: "demo_tv", command: SetVolume(0.5) }', ev)
        for cmd, want in ((MediaPlayerCommand.VOLUME_DOWN, "VolumeDown"), (MediaPlayerCommand.MUTE, "Mute(true)"),
                          (MediaPlayerCommand.UNMUTE, "Mute(false)"), (MediaPlayerCommand.TURN_ON, "TurnOn"),
                          (MediaPlayerCommand.TURN_OFF, "TurnOff")):
            cli.media_player_command(tv.key, command=cmd)
            ev = await dev.wait_event("MediaPlayerCommand")
            check(f"J9 {cmd.name} on the TV player", ev.endswith(f"command: {want} }}"), ev)
        await asyncio.sleep(0.2)
        check("J10 TV player commands are not echoed as a state", states[tv.key].state == MediaPlayerState.OFF,
              str(states[tv.key]))
        # play_media / an announcement aimed at the TV player: ignored.
        cli.media_player_command(tv.key, media_url="http://127.0.0.1:1/a.flac", announcement=True)
        cli.media_player_command(tv.key, command=MediaPlayerCommand.STOP)
        await asyncio.sleep(0.4)
        check("J11 play media, an announcement and a stop (not advertised) to the TV player do nothing",
              dev.count_since("MediaPlayerCommand", start) == 6 and dev.count_since("Announce", start) == 0
              and dev.count_since("MediaStop", start) == 0, str(dev.events[start:]))
        # The satellite player is untouched by all of that and still works.
        cli.media_player_command(sat.key, volume=0.3)
        ev = await dev.wait_event("VolumeChanged")
        ok = await until_true(lambda: abs(states[sat.key].volume - 0.3) < 1e-6)
        check("J12 the satellite player's volume is the reply volume (VolumeChanged), its state follows",
              ev == "VolumeChanged(30)" and ok, ev)
        cli.media_player_command(sat.key, command=MediaPlayerCommand.STOP)
        ev = await dev.wait_event("MediaStop")
        check("J13 the satellite player's stop is MediaStop, not a TV command",
              ev == "MediaStop" and dev.count_since("MediaPlayerCommand", start) == 6, ev)
        # A reconnect gets both states again, same keys.
        await cli.disconnect()
        await dev.wait_event("HaDisconnected")
        ha2 = FakeHA(dev.port)
        cli2 = await ha2.connect()
        _i, ents2, _ = await cli2.device_info_and_list_entities()
        keys2 = {e.object_id: e.key for e in ents2 if isinstance(e, MediaPlayerInfo)}
        states2: dict[int, object] = {}
        cli2.subscribe_states(lambda st: states2.__setitem__(st.key, st))
        ok = await until_true(lambda: sat.key in states2 and tv.key in states2)
        check("J14 reconnect: same keys, both states re-sent",
              ok and keys2 == {"media_player": sat.key, "demo_tv": tv.key}
              and states2[tv.key].state == MediaPlayerState.OFF, f"{keys2} {states2}")
        await cli2.disconnect()
    finally:
        await dev.stop()


async def scenario_music(binary: str) -> None:
    """Music on the satellite's media player (design M2): flags, formats, commands, states."""
    print("\n== scenario K: music on the satellite media player (flags, formats, commands, state) ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    dev = Device(binary, ["--mode", "none", "--music", "--mac", "02:00:00:00:00:10"], psk)
    await dev.start()
    feat = MediaPlayerEntityFeature
    try:
        ha = FakeHA(dev.port, psk)
        cli = await ha.connect()
        _info, entities, _ = await cli.device_info_and_list_entities()
        sat = next(e for e in entities if isinstance(e, MediaPlayerInfo))
        flags = feat(sat.feature_flags_compat(APIVersion(1, 14)))
        want = (feat.PAUSE | feat.VOLUME_SET | feat.PLAY_MEDIA | feat.VOLUME_STEP | feat.STOP
                | feat.PLAY | feat.BROWSE_MEDIA | feat.MEDIA_ANNOUNCE)
        check("K1 flags 1201669: pause, volume set/step, play media, stop, play, browse, announce; no mute or enqueue",
              sat.feature_flags == 1201669 and flags == want
              and not flags & (feat.VOLUME_MUTE | feat.MEDIA_ENQUEUE | feat.TURN_ON | feat.TURN_OFF),
              f"{sat.feature_flags} {flags!r}")
        fm = sat.supported_formats
        check("K2 formats: ANNOUNCEMENT flac/22050/mono first, then DEFAULT flac/48000/stereo/16-bit",
              len(fm) == 2
              and fm[0].purpose == MediaPlayerFormatPurpose.ANNOUNCEMENT
              and (fm[0].format, fm[0].sample_rate, fm[0].num_channels) == ("flac", 22050, 1)
              and fm[1].purpose == MediaPlayerFormatPurpose.DEFAULT
              and (fm[1].format, fm[1].sample_rate, fm[1].num_channels, fm[1].sample_bytes) == ("flac", 48000, 2, 2),
              str(fm))

        states: list = []
        cli.subscribe_states(lambda st: states.append(st) if st.key == sat.key else None)
        await until_true(lambda: bool(states))
        check("K3 initial state idle", states[-1].state == MediaPlayerState.IDLE, str(states[-1]))

        url = "http://ma.example.invalid:8097/flow/x.flac"
        cli.media_player_command(sat.key, media_url=url)
        ev = await dev.wait_event("MusicPlay")
        check("K4 play_media becomes MusicPlay with the URL, announce false",
              ev == f'MusicPlay {{ url: "{url}", announce: false }}', ev)
        cli.media_player_command(sat.key, media_url=url, announcement=True)
        ev = await dev.wait_event("MusicPlay")
        check("K5 play_media with announce: true", ev.endswith("announce: true }"), ev)
        cli.media_player_command(sat.key, command=MediaPlayerCommand.PAUSE)
        check("K6 pause", await dev.wait_event("MusicPause") == "MusicPause")
        cli.media_player_command(sat.key, command=MediaPlayerCommand.PLAY)
        check("K7 play resumes", await dev.wait_event("MusicResume") == "MusicResume")
        cli.media_player_command(sat.key, command=MediaPlayerCommand.STOP)
        check("K8 stop is MediaStop", await dev.wait_event("MediaStop") == "MediaStop")
        cli.media_player_command(sat.key, volume=0.35)
        check("K9 volume set is VolumeChanged", await dev.wait_event("VolumeChanged") == "VolumeChanged(35)")

        # What HA sends for enqueue: ENQUEUE plus the URL, although the flag is not offered.
        n = len(states)
        cli.media_player_command(sat.key, media_url=url, command=MediaPlayerCommand.ENQUEUE)
        ev = await dev.wait_event("MusicEnqueueRefused")
        ok = await until_true(lambda: len(states) > n)
        check("K10 ENQUEUE is refused: the URL is handed up and the state echoed",
              ev == f'MusicEnqueueRefused {{ url: Some("{url}") }}' and ok, ev)

        for state, want_state in (("playing", MediaPlayerState.PLAYING), ("paused", MediaPlayerState.PAUSED),
                                  ("idle", MediaPlayerState.IDLE)):
            await dev.command(f"music {state}")
            ok = await until_true(lambda: states[-1].state == want_state)
            check(f"K11 SetMusicState({state}) reaches HA with the volume",
                  ok and abs(states[-1].volume - 0.35) < 1e-6, str(states[-1]))
        await cli.disconnect()

        # A plaintext session never gets a media URL through (no key, no allow_plaintext).
        dev2 = Device(binary, ["--mode", "none", "--music", "--mac", "02:00:00:00:00:11"], None)
        await dev2.start()
        try:
            cli2 = await FakeHA(dev2.port).connect()
            _i, ents2, _ = await cli2.device_info_and_list_entities()
            sat2 = next(e for e in ents2 if isinstance(e, MediaPlayerInfo))
            cli2.media_player_command(sat2.key, media_url=url)
            cli2.media_player_command(sat2.key, media_url=url, command=MediaPlayerCommand.ENQUEUE)
            cli2.media_player_command(sat2.key, command=MediaPlayerCommand.PAUSE)
            await dev2.wait_event("MusicPause")
            check("K12 a plaintext session: play_media and ENQUEUE refused, pause still arrives",
                  dev2.count_since("MusicPlay", 0) == 0 and dev2.count_since("MusicEnqueue", 0) == 0,
                  str(dev2.events))
            await cli2.disconnect()
        finally:
            await dev2.stop()
    finally:
        await dev.stop()


async def scenario_music_harness(binary: str) -> None:
    """The M1 helpers (media player commands, flags, formats, the fake proxies) against
    fake_device: first music off (flags 5124, one ANNOUNCEMENT format), then `--music` over
    an encrypted session (M2). What needs the satellite fetching the URL is in
    satellite_music_e2e.py (M5, S1 to S15)."""
    print("\n== scenario P: music harness helpers (music off, then --music) ==")
    dev = Device(binary, ["--mode", "none", "--mac", "02:00:00:00:00:10"], None)
    await dev.start()
    ha = FakeHA(dev.port)
    try:
        cli = await ha.connect()
        await ha.load_media_player()
        ha.track_media_state()
        feat = MediaPlayerEntityFeature
        flags = ha.media_flags()
        check("P1 the helper reads the entity flags: 5124 = STOP + VOLUME_SET + VOLUME_STEP, no PLAY_MEDIA",
              int(flags) == MEDIA_FLAGS_MUSIC_OFF and not flags & (feat.PLAY_MEDIA | feat.PAUSE | feat.PLAY
                                                                    | feat.BROWSE_MEDIA | feat.MEDIA_ANNOUNCE),
              repr(flags))
        fmts = ha.media_formats()
        check("P2 supported_formats: one ANNOUNCEMENT entry (flac 22050 Hz mono 16 bit), no DEFAULT",
              [(f.format, f.sample_rate, f.num_channels, int(f.purpose), f.sample_bytes) for f in fmts]
              == [("flac", 22050, 1, int(MediaPlayerFormatPurpose.ANNOUNCEMENT), 2)], str(fmts))
        check("P3 HA's choice: announce takes that entry, play_media finds no DEFAULT entry",
              fake_proxy.ha_pick_format(fmts, announce=True) is fmts[0] and fake_proxy.ha_pick_format(fmts) is None)
        check("P4 the initial state is idle and HA maps it to 'idle'",
              await ha.wait_media_state(MediaPlayerState.IDLE)
              and ha.ha_state_name(ha.media_state.state) == "idle"
              and ha.ha_state_name(MediaPlayerState.ANNOUNCING) is None)

        start = len(dev.events)
        ha.volume_set(0.4)
        ev = await dev.wait_event("VolumeChanged")
        ok = await until_true(lambda: ha.media_state is not None and abs(ha.media_state.volume - 0.4) < 1e-6)
        check("P5 volume set reaches the app (VolumeChanged) and the state follows", ev == "VolumeChanged(40)" and ok, ev)
        ha.volume_up()
        ev = await dev.wait_event("VolumeChanged")
        ha.volume_down()
        ev2 = await dev.wait_event("VolumeChanged")
        check("P6 volume up and down are 5 % steps of the same volume", ev == "VolumeChanged(45)" and ev2 == "VolumeChanged(40)",
              f"{ev} {ev2}")
        ha.media_stop()
        ev = await dev.wait_event("MediaStop")
        check("P7 stop is MediaStop", ev == "MediaStop", ev)

        # Music off: everything else is ignored. The proxy side still behaves like HA's.
        async with fake_proxy.HaProxy() as proxy:
            mark = len(dev.events)
            url = ha.play_media_like_ha(proxy, announce=True, signal=fake_proxy.Signal((fake_proxy.Track(1.0),)))
            ha.enqueue_media(url)
            ha.media_pause()
            ha.media_resume()
            sent = ha.send_unadvertised()
            await asyncio.sleep(0.5)
            check("P8 play_media (announce), ENQUEUE, pause, play and the unadvertised commands are ignored with music off",
                  len(dev.events) == mark and len(sent) == 6, str(dev.events[mark:]))
            conv = proxy.conversion_of(url)
            check("P9 the proxy URL HA created was never fetched and holds a slot (the satellite must open and close it: M5 S13)",
                  conv.fetches == 0 and proxy.slot_count(ha.device_name) == 1)
            ha.play_media_like_ha(proxy, announce=True)
            ha.play_media_like_ha(proxy, announce=True)
            check("P10 HA's proxy kills the oldest conversion when a third URL is created",
                  conv.killed and proxy.slot_count(ha.device_name) == 2)
        async with fake_proxy.MaFlow() as flow:
            url = flow.url_for_play_media(fmts + [fake_proxy.AdvertisedFormat("flac", 48000, 2)], 44100, endless=True)
            s = flow.stream_of(url)
            check("P11 Music Assistant's URL is on another port than HA's proxy and follows MA's format rule",
                  (s.fmt, s.rate, s.channels) == ("flac", 48000, 2) and "/ffmpeg_proxy/" not in url)
        await cli.disconnect()
    finally:
        await dev.stop()
    # Music on (M2): the same helpers against `fake_device --music` over a Noise session
    # (a plaintext session never gets a media URL through).
    psk = base64.b64encode(os.urandom(32)).decode()
    dev = Device(binary, ["--mode", "none", "--music", "--mac", "02:00:00:00:00:12"], psk)
    await dev.start()
    try:
        ha = FakeHA(dev.port, psk)
        cli = await ha.connect()
        await ha.load_media_player()
        ha.track_media_state()
        fmts = ha.media_formats()
        default, ann = fake_proxy.ha_pick_format(fmts), fake_proxy.ha_pick_format(fmts, announce=True)
        check("P12 music on: flags 1201669 (helper constant), DEFAULT format flac 48000 Hz stereo, ANNOUNCEMENT unchanged",
              int(ha.media_flags()) == MEDIA_FLAGS_MUSIC_ANNOUNCE and default is not None and ann is not None
              and (default.format, default.sample_rate, default.num_channels) == ("flac", 48000, 2)
              and (ann.format, ann.sample_rate, ann.num_channels) == ("flac", 22050, 1), str(fmts))
        async with fake_proxy.HaProxy() as proxy, fake_proxy.MaFlow() as flow:
            sig = fake_proxy.Signal((fake_proxy.Track(1.0),))
            url = ha.play_media_like_ha(proxy, signal=sig)
            ev = await dev.wait_event("MusicPlay")
            c = proxy.conversion_of(url)
            check("P13 play_media: HA's proxy URL (format from the DEFAULT entry) reaches the app",
                  ev == f'MusicPlay {{ url: "{url}", announce: false }}' and (c.fmt, c.rate, c.channels) == ("flac", 48000, 2), ev)
            url2 = ha.play_media_like_ha(proxy, announce=True, signal=sig)
            ev = await dev.wait_event("MusicPlay")
            c2 = proxy.conversion_of(url2)
            check("P14 announce: the proxy URL has the ANNOUNCEMENT format and the flag reaches the app",
                  ev == f'MusicPlay {{ url: "{url2}", announce: true }}' and (c2.fmt, c2.rate, c2.channels) == ("flac", 22050, 1), ev)
            url3 = proxy.url_for_play_media(ha.device_name, fmts, False, sig)  # a third URL: the first is evicted
            ha.enqueue_media(url3)
            ev = await dev.wait_event("MusicEnqueueRefused")
            check("P15 enqueue: ENQUEUE plus the URL arrives and is refused; HA's proxy had evicted the oldest conversion",
                  ev == f'MusicEnqueueRefused {{ url: Some("{url3}") }}' and c.killed and proxy.slot_count(ha.device_name) == 2, ev)
            ha.media_pause()
            ev1 = await dev.wait_event("MusicPause")
            ha.media_resume()
            ev2 = await dev.wait_event("MusicResume")
            ha.volume_set(0.5)
            ev3 = await dev.wait_event("VolumeChanged")
            ha.media_stop()
            ev4 = await dev.wait_event("MediaStop")
            check("P16 pause, resume, volume and stop reach the app", (ev1, ev2, ev3, ev4) ==
                  ("MusicPause", "MusicResume", "VolumeChanged(50)", "MediaStop"), str((ev1, ev2, ev3, ev4)))
            mark = len(dev.events)
            ha.send_unadvertised()
            await asyncio.sleep(0.4)
            check("P17 mute, unmute, toggle, repeat and clear-playlist are ignored", len(dev.events) == mark, str(dev.events[mark:]))
            url4 = ha.play_media_like_ma(flow, endless=True)
            ev = await dev.wait_event("MusicPlay")
            check("P18 Music Assistant's URL (its own port, MA's format rule, no HA proxy path) reaches the app unchanged",
                  ev == f'MusicPlay {{ url: "{url4}", announce: false }}' and f":{flow.port}/" in url4
                  and f":{proxy.port}/" not in url4 and flow.stream_of(url4).rate == 48000, ev)
        await cli.disconnect()
    finally:
        await dev.stop()
    # What needs the satellite fetching and playing the URLs (the proxy slot count back to its
    # start, MA's endless stream, a refused ENQUEUE URL opened and closed) runs against the real
    # daemon: satellite_music_e2e.py, scenarios S1 to S15 (M5).


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--device", required=True, help="path to the fake_device binary")
    args = parser.parse_args()
    failed = False
    for scenario in (scenario_plaintext_speaker, scenario_capture_live, scenario_noise_url, scenario_timers,
                     scenario_pairing, scenario_pairing_window, scenario_window_behind_router,
                     scenario_key_clear, scenario_entities, scenario_generic_entities,
                     scenario_two_media_players, scenario_music, scenario_music_harness):
        try:
            await scenario(args.device)
        except Exception as err:  # noqa: BLE001
            failed = True
            print(f"FAIL  {scenario.__name__}: {type(err).__name__}: {err}", flush=True)
    passed = sum(1 for _, ok, _ in RESULTS if ok)
    if PENDING:
        print(f"\n{len(PENDING)} checks pending (need unbuilt satellite features): " + "; ".join(n for n, _ in PENDING))
    print(f"\n{passed} checks passed, {'FAILED' if failed else 'all scenarios OK'} (device logs: {LOG_DIR})")
    return 1 if failed else 0


LOG_DIR = os.environ.get("FAKE_HA_LOG_DIR") or os.path.join(
    os.environ.get("TMPDIR", "/tmp"), f"esphome-fake-ha-{os.getpid()}")
os.makedirs(LOG_DIR, exist_ok=True)

if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
