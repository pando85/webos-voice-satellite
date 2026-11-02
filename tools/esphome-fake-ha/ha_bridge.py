#!/usr/bin/env python3
"""Relay between the satellite and a real Home Assistant Assist pipeline.

Plays the part of HA's ESPHome integration when HA itself cannot open a
connection to the satellite (for example a firewall between HA and the TV):

    satellite (ESPHome API, Noise) <--aioesphomeapi-- this bridge
        --HA WebSocket API (assist_pipeline/run, binary audio)--> HA pipeline

It mirrors what `homeassistant/components/esphome/assist_satellite.py`
(HA 2026.10.4) does for a device with API_AUDIO + SPEAKER:
- a start request with USE_WAKE_WORD starts the run at the wake word stage,
  otherwise at STT; the run ends at TTS;
- microphone audio from the device goes to the pipeline as binary frames;
- pipeline events become voice assistant events (WAKE_WORD_END with no output
  becomes ERROR no_wake_word, as in HA);
- the TTS result is fetched from HA's tts_proxy, converted to 16 kHz mono s16
  with ffmpeg and streamed to the device between TTS_STREAM_START/END in 512
  sample chunks, paced 384 ms ahead of real time.

What it does NOT cover: HA's config flow, the assist_satellite entity, the
pipeline/VAD selects, HA's own reconnect logic, announcements.

Usage:
    .venv/bin/python -I ha_bridge.py --device HOST:6053 --psk-file FILE \
        --ha-url https://ha.example --token-file FILE [--pipeline NAME_OR_ID] \
        [--runs N] [--duration SECS] [--json-log FILE] [--reply-on-intent-failure TEXT]
        [--synthetic-reply-every SECS --reply-on-intent-failure TEXT]

`--synthetic-reply-every SECS` is a soak aid, not a pipeline test: when the
device starts a run the bridge opens no HA wake word run and ignores the
microphone audio; after SECS it plays a "wake, speech, intent" event sequence
with a fixed text and the reply is a real TTS-only run of the
`--reply-on-intent-failure` text in HA's pipeline (labelled `synthetic: true`
in the run summary). It makes a device in wake mode play a streamed TTS reply
every SECS seconds with no human voice involved.

The access token is read from a file and never printed. Transcripts and
replies are printed (they are the test's own phrases); keep the output local.
Requires aioesphomeapi (requirements.txt), aiohttp and ffmpeg.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
import time

import aiohttp
from aioesphomeapi import APIClient, MediaPlayerEntityState, MediaPlayerState, VoiceAssistantEventType as Ev
from aioesphomeapi.model import VoiceAssistantCommandFlag

USE_WAKE_WORD = int(VoiceAssistantCommandFlag.USE_WAKE_WORD)
SAMPLES_PER_CHUNK = 512
BYTES_PER_CHUNK = SAMPLES_PER_CHUNK * 2

PLAIN = {
    "run-start": Ev.VOICE_ASSISTANT_RUN_START,
    "wake_word-start": Ev.VOICE_ASSISTANT_WAKE_WORD_START,
    "stt-start": Ev.VOICE_ASSISTANT_STT_START,
    "stt-vad-start": Ev.VOICE_ASSISTANT_STT_VAD_START,
    "stt-vad-end": Ev.VOICE_ASSISTANT_STT_VAD_END,
    "intent-start": Ev.VOICE_ASSISTANT_INTENT_START,
}


def log(msg: str) -> None:
    print(f"{time.strftime('%H:%M:%S')}.{int(time.time() * 1000) % 1000:03d} {msg}", flush=True)


class HaWs:
    """One authenticated HA WebSocket connection with id-based dispatch."""

    def __init__(self, url: str, token: str) -> None:
        self.url = url.rstrip("/").replace("https://", "wss://").replace("http://", "ws://") + "/api/websocket"
        self.token = token
        self.next_id = 0
        self.queues: dict[int, asyncio.Queue] = {}

    async def connect(self, session: aiohttp.ClientSession) -> None:
        self.ws = await session.ws_connect(self.url, max_msg_size=0, heartbeat=30)
        await self.ws.receive_json()
        await self.ws.send_json({"type": "auth", "access_token": self.token})
        r = await self.ws.receive_json()
        if r.get("type") != "auth_ok":
            raise RuntimeError(f"HA auth failed: {r.get('type')}")
        self.reader = asyncio.create_task(self._read())

    async def _read(self) -> None:
        async for msg in self.ws:
            if msg.type != aiohttp.WSMsgType.TEXT:
                continue
            data = json.loads(msg.data)
            for m in data if isinstance(data, list) else [data]:
                q = self.queues.get(m.get("id"))
                if q is not None:
                    q.put_nowait(m)
        for q in self.queues.values():
            q.put_nowait({"type": "closed"})

    async def call(self, payload: dict) -> dict:
        i, q = self.open()
        await self.ws.send_json(dict(payload, id=i))
        while True:
            m = await q.get()
            if m.get("type") in ("result", "closed"):
                self.queues.pop(i, None)
                return m

    def open(self) -> tuple[int, asyncio.Queue]:
        self.next_id += 1
        q: asyncio.Queue = asyncio.Queue()
        self.queues[self.next_id] = q
        return self.next_id, q

    async def send(self, payload: dict) -> None:
        await self.ws.send_json(payload)

    async def send_bytes(self, data: bytes) -> None:
        await self.ws.send_bytes(data)


class Run:
    """One pipeline run started by a device start request."""

    def __init__(self, msg_id: int, wake: bool) -> None:
        self.msg_id = msg_id
        self.wake = wake
        self.handler_id: int | None = None
        self.audio_open = True
        self.pending: list[bytes] = []
        self.t0 = time.monotonic()
        self.marks: dict[str, float] = {}
        self.info: dict = {"start_stage": "wake_word" if wake else "stt"}
        self.audio_bytes = 0
        self.substitute: asyncio.Task | None = None

    def mark(self, name: str) -> None:
        self.marks.setdefault(name, round((time.monotonic() - self.t0) * 1000))


class Bridge:
    def __init__(self, args: argparse.Namespace, token: str, psk: str) -> None:
        self.args = args
        self.ha = HaWs(args.ha_url, token)
        host, _, port = args.device.rpartition(":")
        self.cli = APIClient(host, int(port), "", client_info="webos-ha-bridge", noise_psk=psk)
        self.run: Run | None = None
        self.runs_done: list[dict] = []
        self.pipeline_id: str | None = None
        self.finished = asyncio.Event()
        self.tts_task: asyncio.Task | None = None
        self.media_state = None
        self.media_changed = asyncio.Event()

    # ---- device side ----

    async def handle_start(self, conversation_id, flags, audio_settings, wake_word_phrase):
        wake = bool(flags & USE_WAKE_WORD)
        if self.args.synthetic_reply_every:
            run = Run(-1, wake)
            run.info["synthetic"] = True
            self.run = run
            log(f"device start request: synthetic reply in {self.args.synthetic_reply_every} s (no HA run)")
            send = self.cli.send_voice_assistant_event
            send(Ev.VOICE_ASSISTANT_RUN_START, {})
            if wake:
                send(Ev.VOICE_ASSISTANT_WAKE_WORD_START, {})
            asyncio.create_task(self.synthetic_cycle(run))
            return 0
        msg_id, q = self.ha.open()
        run = Run(msg_id, wake)
        self.run = run
        payload = {
            "id": msg_id,
            "type": "assist_pipeline/run",
            "start_stage": "wake_word" if wake else "stt",
            "end_stage": "tts",
            "input": {"sample_rate": 16000},
            "conversation_id": conversation_id or None,
        }
        if wake:
            # HA's default wake word timeout for WS runs is 3 s; the device
            # streams continuously, like an ESPHome satellite.
            payload["input"]["timeout"] = 3600
            # The whole run is capped too (HA default 300 s).
            payload["timeout"] = 3600
        if self.pipeline_id:
            payload["pipeline"] = self.pipeline_id
        log(f"device start request: {'wake word' if wake else 'push-to-talk (stt)'}"
            f" conversation_id={'set' if conversation_id else 'none'}")
        await self.ha.send(payload)
        asyncio.create_task(self.pump_events(run, q))
        return 0  # API audio, no UDP port

    async def handle_audio(self, data: bytes, data2: bytes | None) -> None:
        run = self.run
        if run is None or not run.audio_open or run.info.get("synthetic"):
            return
        run.audio_bytes += len(data)
        if run.handler_id is None:
            run.pending.append(data)
        else:
            await self.ha.send_bytes(bytes([run.handler_id]) + data)

    async def handle_stop(self, abort: bool) -> None:
        run = self.run
        if run is None:
            return
        log(f"device stop request (abort={abort})")
        await self.end_audio(run)
        if abort:
            try:
                await self.ha.call({"type": "unsubscribe_events", "subscription": run.msg_id})
            except Exception as err:  # noqa: BLE001
                log(f"unsubscribe failed: {err}")

    async def end_audio(self, run: Run) -> None:
        if run.audio_open and run.handler_id is not None:
            run.audio_open = False
            await self.ha.send_bytes(bytes([run.handler_id]))
        run.audio_open = False

    # ---- HA side ----

    async def pump_events(self, run: Run, q: asyncio.Queue) -> None:
        while True:
            m = await q.get()
            if m.get("type") == "closed":
                log("HA connection closed")
                self.finished.set()
                return
            if m.get("type") == "result":
                if not m.get("success"):
                    err = m.get("error", {})
                    log(f"HA refused the run: {err}")
                    run.info["error"] = err.get("code")
                    self.cli.send_voice_assistant_event(
                        Ev.VOICE_ASSISTANT_ERROR, {"code": str(err.get("code")), "message": str(err.get("message"))})
                    self.cli.send_voice_assistant_event(Ev.VOICE_ASSISTANT_RUN_END, {})
                    self.finish(run)
                    return
                continue
            ev = m.get("event", {})
            etype, data = ev.get("type"), ev.get("data") or {}
            run.mark(etype)
            if await self.on_event(run, etype, data):
                self.ha.queues.pop(run.msg_id, None)
                return

    async def on_event(self, run: Run, etype: str, data: dict) -> bool:
        send = self.cli.send_voice_assistant_event
        if etype == "run-start":
            run.handler_id = data.get("runner_data", {}).get("stt_binary_handler_id")
            run.info["pipeline"] = data.get("pipeline")
            for chunk in run.pending:
                await self.ha.send_bytes(bytes([run.handler_id]) + chunk)
            run.pending.clear()
            send(Ev.VOICE_ASSISTANT_RUN_START, {})
        elif etype == "wake_word-start":
            run.info["wake_word_entity"] = data.get("entity_id")
            run.info["wake_word_metadata"] = data.get("metadata")
            send(Ev.VOICE_ASSISTANT_WAKE_WORD_START, {})
        elif etype == "wake_word-end":
            out = data.get("wake_word_output") or {}
            run.info["wake_word_output"] = out
            log(f"wake_word-end: {out}")
            if not out:
                send(Ev.VOICE_ASSISTANT_ERROR, {"code": "no_wake_word", "message": "No wake word detected"})
            else:
                send(Ev.VOICE_ASSISTANT_WAKE_WORD_END, {})
        elif etype == "stt-vad-end":
            send(Ev.VOICE_ASSISTANT_STT_VAD_END, {})
        elif etype == "stt-end":
            text = (data.get("stt_output") or {}).get("text", "")
            run.info["transcript"] = text
            log(f"stt-end: {text!r}")
            await self.end_audio(run)
            send(Ev.VOICE_ASSISTANT_STT_END, {"text": text})
        elif etype == "intent-progress":
            if data.get("tts_start_streaming"):
                send(Ev.VOICE_ASSISTANT_INTENT_PROGRESS, {"tts_start_streaming": "1"})
        elif etype == "intent-end":
            io = data.get("intent_output") or {}
            speech = ((io.get("response") or {}).get("speech") or {}).get("plain", {}).get("speech", "")
            run.info["reply"] = speech
            run.info["response_type"] = (io.get("response") or {}).get("response_type")
            log(f"intent-end: {speech!r} (continue={io.get('continue_conversation')})")
            send(Ev.VOICE_ASSISTANT_INTENT_END, {
                "conversation_id": io.get("conversation_id") or "",
                "continue_conversation": str(int(bool(io.get("continue_conversation")))),
                "speech": speech,
            })
        elif etype == "tts-start":
            send(Ev.VOICE_ASSISTANT_TTS_START, {"text": data.get("tts_input", "")})
        elif etype == "tts-end":
            out = data.get("tts_output") or {}
            path = out.get("url", "")
            run.info["tts_mime"] = out.get("mime_type")
            url = path if path.startswith("http") else self.args.ha_url.rstrip("/") + path
            # The device runs in SPEAKER mode: it only needs the stream.
            send(Ev.VOICE_ASSISTANT_TTS_END, {"url": url})
            self.tts_task = asyncio.create_task(self.stream_tts(run, url))
        elif etype == "error":
            code = str(data.get("code"))
            run.info["error"] = code
            log(f"pipeline error: {code}: {data.get('message')}")
            await self.end_audio(run)
            if code.startswith("intent") and self.args.reply_on_intent_failure:
                # Labelled substitute: HA's conversation agent failed; get a
                # TTS reply for a fixed text from the same pipeline instead.
                run.info["intent_substituted"] = True
                run.substitute = asyncio.create_task(self.tts_only(run))
            else:
                send(Ev.VOICE_ASSISTANT_ERROR, {"code": code, "message": str(data.get("message"))})
        elif etype in PLAIN:
            send(PLAIN[etype], {})
        elif etype == "run-end":
            # HA has unregistered the binary handler; send nothing more.
            run.audio_open = False
            if run.substitute is not None:
                await run.substitute
            send(Ev.VOICE_ASSISTANT_RUN_END, {})
            if self.tts_task is not None:
                await self.tts_task
                self.tts_task = None
                # Wait until the device reports its media player idle again
                # (it plays out its buffer first), at most 15 s.
                await self.wait_media_idle(15.0)
            self.finish(run)
            return True
        return False

    async def synthetic_cycle(self, run: Run) -> None:
        send = self.cli.send_voice_assistant_event
        await asyncio.sleep(self.args.synthetic_reply_every)
        if self.run is not run:
            return  # the device aborted this run meanwhile
        if run.wake:
            send(Ev.VOICE_ASSISTANT_WAKE_WORD_END, {})
        send(Ev.VOICE_ASSISTANT_STT_START, {})
        send(Ev.VOICE_ASSISTANT_STT_END, {"text": "prueba"})
        send(Ev.VOICE_ASSISTANT_INTENT_START, {})
        await self.tts_only(run)
        send(Ev.VOICE_ASSISTANT_RUN_END, {})
        if self.tts_task is not None:
            await self.tts_task
            self.tts_task = None
            await self.wait_media_idle(15.0)
        self.finish(run)

    async def tts_only(self, run: Run) -> None:
        text = self.args.reply_on_intent_failure
        msg_id, q = self.ha.open()
        await self.ha.send({"id": msg_id, "type": "assist_pipeline/run", "start_stage": "tts",
                            "end_stage": "tts", "input": {"text": text}, "pipeline": self.pipeline_id})
        url = None
        while True:
            m = await q.get()
            if m.get("type") == "closed" or (m.get("type") == "result" and not m.get("success")):
                log(f"substitute TTS run failed: {m.get('error')}")
                break
            ev = m.get("event") or {}
            if ev.get("type") == "tts-end":
                url = ((ev.get("data") or {}).get("tts_output") or {}).get("url")
            if ev.get("type") in ("run-end", "error"):
                break
        self.ha.queues.pop(msg_id, None)
        run.mark("substitute-tts-end")
        send = self.cli.send_voice_assistant_event
        send(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "", "continue_conversation": "0", "speech": text})
        if url:
            url = url if url.startswith("http") else self.args.ha_url.rstrip("/") + url
            send(Ev.VOICE_ASSISTANT_TTS_START, {"text": text})
            send(Ev.VOICE_ASSISTANT_TTS_END, {"url": url})
            self.tts_task = asyncio.create_task(self.stream_tts(run, url))

    async def stream_tts(self, run: Run, url: str) -> None:
        t = time.monotonic()
        async with aiohttp.ClientSession() as s, s.get(url) as r:
            body = await r.read()
            status = r.status
        if status != 200:
            log(f"TTS fetch failed: HTTP {status}")
            return
        proc = await asyncio.create_subprocess_exec(
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-i", "pipe:0",
            "-f", "s16le", "-acodec", "pcm_s16le", "-ar", "16000", "-ac", "1", "pipe:1",
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE)
        pcm, _ = await proc.communicate(body)
        run.info["tts_fetch_convert_ms"] = round((time.monotonic() - t) * 1000)
        run.info["tts_audio_ms"] = round(len(pcm) / 32)
        run.mark("tts-stream-start")
        self.cli.send_voice_assistant_event(Ev.VOICE_ASSISTANT_TTS_STREAM_START, {})
        start = asyncio.get_running_loop().time()
        sent = 0.0
        for i in range(0, len(pcm), BYTES_PER_CHUNK):
            self.cli.send_voice_assistant_audio(pcm[i:i + BYTES_PER_CHUNK])
            sent += SAMPLES_PER_CHUNK / 16000
            wait = (sent - 0.384) - (asyncio.get_running_loop().time() - start)
            if wait > 0:
                await asyncio.sleep(wait)
        self.cli.send_voice_assistant_event(Ev.VOICE_ASSISTANT_TTS_STREAM_END, {})
        run.mark("tts-stream-end")
        log(f"TTS streamed: {run.info['tts_audio_ms']} ms of audio ({len(pcm)} bytes)")

    def on_state(self, state) -> None:
        if isinstance(state, MediaPlayerEntityState):
            self.media_state = state.state
            self.media_changed.set()

    async def wait_media_idle(self, timeout: float) -> None:
        t = time.monotonic()
        deadline = t + timeout
        # The ANNOUNCING state may still be on its way; give it a moment.
        await asyncio.sleep(0.2)
        while self.media_state != MediaPlayerState.IDLE and time.monotonic() < deadline:
            self.media_changed.clear()
            try:
                await asyncio.wait_for(self.media_changed.wait(), deadline - time.monotonic())
            except TimeoutError:
                break
        if self.run is not None:
            self.run.mark("device-media-idle")
        log(f"device media player {self.media_state.name if self.media_state is not None else '?'} "
            f"{round((time.monotonic() - t) * 1000)} ms after the TTS stream ended")

    def finish(self, run: Run) -> None:
        run.info["marks_ms"] = run.marks
        run.info["mic_audio_bytes"] = run.audio_bytes
        self.runs_done.append(run.info)
        log(f"run done: {json.dumps(run.info, ensure_ascii=False)}")
        if self.run is run:
            self.run = None
        if self.args.runs and len(self.runs_done) >= self.args.runs:
            self.finished.set()

    # ---- main ----

    async def main(self) -> int:
        async with aiohttp.ClientSession() as session:
            await self.ha.connect(session)
            pl = await self.ha.call({"type": "assist_pipeline/pipeline/list"})
            pipelines = pl["result"]["pipelines"]
            want = self.args.pipeline or pl["result"]["preferred_pipeline"]
            match = [p for p in pipelines if want in (p["id"], p["name"])]
            if not match:
                log(f"pipeline {want!r} not found")
                return 2
            p = match[0]
            self.pipeline_id = p["id"]
            log(f"HA pipeline {p['name']!r}: wake_word_entity={p.get('wake_word_entity')} "
                f"wake_word_id={p.get('wake_word_id')} stt={p.get('stt_engine')} "
                f"conversation={p.get('conversation_engine')} tts={p.get('tts_engine')}")

            stopped = asyncio.Event()

            async def on_stop(expected: bool) -> None:
                stopped.set()
                self.finished.set()

            await asyncio.wait_for(self.cli.connect(on_stop=on_stop, login=True), 15)
            info = await self.cli.device_info()
            flags = info.voice_assistant_feature_flags_compat(self.cli.api_version)
            log(f"device: name={info.name} api={self.cli.api_version} voice_flags={flags} "
                f"esphome_version={info.esphome_version}")
            self.cli.subscribe_states(self.on_state)
            self.cli.subscribe_voice_assistant(
                handle_start=self.handle_start, handle_stop=self.handle_stop,
                handle_audio=self.handle_audio)
            try:
                await asyncio.wait_for(self.finished.wait(), self.args.duration)
            except TimeoutError:
                pass
            if self.tts_task is not None:
                await self.tts_task
            if not stopped.is_set():
                await self.cli.disconnect()
        if self.args.json_log:
            with open(self.args.json_log, "w") as f:
                json.dump(self.runs_done, f, ensure_ascii=False, indent=1)
        log(f"bridge done: {len(self.runs_done)} runs")
        return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--device", required=True, help="satellite HOST:PORT")
    ap.add_argument("--psk-file", required=True)
    ap.add_argument("--ha-url", required=True)
    ap.add_argument("--token-file", required=True)
    ap.add_argument("--pipeline", help="pipeline name or id (default: HA's preferred)")
    ap.add_argument("--runs", type=int, default=0, help="stop after N finished runs (0: no limit)")
    ap.add_argument("--duration", type=float, default=120.0, help="stop after SECS")
    ap.add_argument("--json-log", help="write the run summaries here")
    ap.add_argument("--reply-on-intent-failure", metavar="TEXT",
                    help="if HA's intent stage fails, reply with TTS of TEXT from the same pipeline "
                         "(labelled 'intent_substituted' in the run summary)")
    ap.add_argument("--synthetic-reply-every", type=float, metavar="SECS",
                    help="soak aid: answer every device run after SECS with a TTS-only reply of "
                         "--reply-on-intent-failure TEXT (no wake word run, mic audio ignored)")
    args = ap.parse_args()
    if args.synthetic_reply_every and not args.reply_on_intent_failure:
        ap.error("--synthetic-reply-every needs --reply-on-intent-failure TEXT")
    token = open(args.token_file).read().strip()
    psk = open(args.psk_file).read().strip()

    async def run() -> int:
        # APIClient needs a running loop at construction.
        return await Bridge(args, token, psk).main()

    return asyncio.run(run())


if __name__ == "__main__":
    sys.exit(main())
