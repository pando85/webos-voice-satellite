#!/usr/bin/env python3
"""Fake media servers for the satellite's music playback (milestone M1).

Two models of where a `play_media` URL points, both stdlib-only asyncio HTTP
servers on 127.0.0.1 (design: docs/design/music-playback.md, sections B and F):

  ha-proxy  `HaProxy`: Home Assistant's ffmpeg proxy for ESPHome media players
            (`/api/esphome/ffmpeg_proxy/<device>/<random>.<format>`).
  ma-flow   `MaFlow`: Music Assistant's flow stream, which bypasses HA's proxy:
            one continuous stream for the whole queue, on its own host and port.

Behaviour reproduced (documented, from the plan's 2026-11-05 review of HA core
2026.10.4 and Music Assistant server 2.10.9; re-read `ffmpeg_proxy.py`,
`media_player.py` and MA's `hass_players/player.py` before relying on a detail,
and record the versions; the real check is M2b):

  ha-proxy  plain 200, `Transfer-Encoding: chunked`, 2048-byte chunks, no
            `Content-Length`, no auth, `Range` ignored (always the whole body
            from the start), real ffmpeg (`-i SRC -f FMT -ar R -ac C
            -sample_fmt s16 -map_metadata -1 pipe:`) from a generated test
            signal, ffmpeg blocks when the client is slow. A new request for
            the same URL restarts the conversion from the start (not strictly
            one-time). At most 2 conversions per device: creating a third URL
            kills the oldest unfinished one, and a URL that was never fetched
            holds a slot. A conversion is finished when its body ended or the
            client went away (the slot is then free; the URL still answers
            and restarts). A killed URL answers 404.
  ma-flow   no `Content-Length`; one continuous stream made of several
            tracks at one codec, rate and channel count; endless or ending at
            close; every GET starts a fresh stream (not one-time, no slot
            limit, no auth); the format follows `ma_pick_format`.

Assumptions that are NOT verified and that M2b must check: the exact ffmpeg
flags of HA (here: the ones above), MA's chunk size and framing (`framing`
selects `chunked` or close-delimited), and MA's rate rule (`ma_pick_format`).

Test signal: sines (one frequency per track and channel) plus deterministic
noise, generated here; no music files exist. `backend="python"` serves WAV
without ffmpeg (CI has no ffmpeg guarantee); `backend="ffmpeg"` serves
whatever ffmpeg can encode (flac, wav, mp3).

Failure injection (`Fault`): stall, early close, status code, first-byte
delay. Everything stays on 127.0.0.1 unless a caller binds another address.

CLI for manual use (prints `URL <url>` and runs until interrupted):
    python3 tools/esphome-fake-ha/fake_proxy.py --mode ha-proxy --format flac
    python3 tools/esphome-fake-ha/fake_proxy.py --mode ma-flow --rate 44100 --endless
"""

from __future__ import annotations

import argparse
import array
import asyncio
import dataclasses
import functools
import math
import random
import secrets
import shutil
import socket
import ssl
import struct
import sys
import tempfile
import time
from collections.abc import AsyncIterator, Iterator, Sequence
from dataclasses import dataclass, field

HA_CHUNK = 2048  # bytes per body chunk of HA's proxy
MAX_CONVERSIONS_PER_DEVICE = 2
MIME = {"flac": "audio/flac", "wav": "audio/x-wav", "mp3": "audio/mpeg"}
PURPOSE_DEFAULT = 0
PURPOSE_ANNOUNCEMENT = 1


# ---------------------------------------------------------------------------
# Test signal
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Track:
    seconds: float
    freq: float = 440.0
    amp: float = 0.5  # of full scale


@dataclass(frozen=True)
class Signal:
    """Deterministic multi-track test audio (16-bit PCM).

    Channel c of a track plays `freq * (1 + 0.5 * c)` so a swapped or dropped
    channel is audible to a test. `noise` is uniform noise of that amplitude
    (fraction of full scale), which also keeps FLAC output large enough that a
    slow client blocks the encoder.
    """

    tracks: tuple[Track, ...] = (Track(10.0),)
    rate: int = 44100
    channels: int = 2
    noise: float = 0.05
    seed: int = 1
    endless: bool = False  # the track list repeats forever

    def with_format(self, rate: int, channels: int) -> "Signal":
        return dataclasses.replace(self, rate=rate, channels=channels)

    def frames_per_pass(self) -> int:
        return sum(round(t.seconds * self.rate) for t in self.tracks)

    def track_bounds(self) -> list[tuple[int, int]]:
        """(start, end) frame of each track in one pass."""
        out, pos = [], 0
        for t in self.tracks:
            n = round(t.seconds * self.rate)
            out.append((pos, pos + n))
            pos += n
        return out

    def chunks(self, chunk_frames: int = 4096) -> Iterator[bytes]:
        """s16le interleaved PCM in chunks of `chunk_frames` frames."""
        while True:
            for idx, t in enumerate(self.tracks):
                n = round(t.seconds * self.rate)
                rng = random.Random(self.seed * 1000 + idx)
                w = [2 * math.pi * t.freq * (1 + 0.5 * c) / self.rate for c in range(self.channels)]
                for start in range(0, n, chunk_frames):
                    buf = array.array("h")
                    for i in range(start, min(n, start + chunk_frames)):
                        for c in range(self.channels):
                            v = t.amp * math.sin(w[c] * i) + self.noise * (2 * rng.random() - 1)
                            buf.append(max(-32768, min(32767, int(v * 32767))))
                    if sys.byteorder == "big":
                        buf.byteswap()
                    yield buf.tobytes()
            if not self.endless:
                return

    def pcm(self) -> bytes:
        """The whole finite signal (cached). Raises for an endless signal."""
        if self.endless:
            raise ValueError("an endless signal has no PCM image")
        return _pcm_cached(self)

    def wav(self) -> bytes:
        """A finite WAV file (real sizes), for ffmpeg input and tests."""
        pcm = self.pcm()
        return wav_header(self.rate, self.channels, 2, len(pcm)) + pcm


@functools.lru_cache(maxsize=16)
def _pcm_cached(sig: Signal) -> bytes:
    return b"".join(sig.chunks())


def wav_header(rate: int, channels: int, width: int = 2, data_len: int = 0xFFFFFFFF) -> bytes:
    """RIFF/WAVE header; the default is the unknown-length form ffmpeg writes to a pipe."""
    riff = 0xFFFFFFFF if data_len == 0xFFFFFFFF else 36 + data_len
    return (
        b"RIFF" + struct.pack("<I", riff) + b"WAVEfmt "
        + struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * channels * width, channels * width, width * 8)
        + b"data" + struct.pack("<I", data_len)
    )


# ---------------------------------------------------------------------------
# Format negotiation as HA and Music Assistant do it (duck-typed formats:
# any object with .format .sample_rate .num_channels .purpose .sample_bytes,
# e.g. aioesphomeapi.MediaPlayerSupportedFormat)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class AdvertisedFormat:
    """A `supported_formats` entry (same attribute names as aioesphomeapi's
    MediaPlayerSupportedFormat), for building formats without that library."""

    format: str
    sample_rate: int
    num_channels: int
    purpose: int = PURPOSE_DEFAULT
    sample_bytes: int = 2


def _purpose(fmt) -> int:
    return int(getattr(fmt, "purpose", PURPOSE_DEFAULT) or 0)


def ha_pick_format(formats: Sequence, announce: bool = False):
    """HA's choice for `play_media`: the first DEFAULT entry, or the first
    ANNOUNCEMENT entry when announcing. None when there is none (HA then does
    not proxy the URL)."""
    want = PURPOSE_ANNOUNCEMENT if announce else PURPOSE_DEFAULT
    return next((f for f in formats if _purpose(f) == want), None)


def ma_pick_format(formats: Sequence, source_rate: int = 44100) -> tuple[str, int, int, int]:
    """Music Assistant's choice (review, unverified, M2b re-checks): the codec
    from the first format after sorting by purpose (DEFAULT before ANNOUNCEMENT,
    stable), the sample rate from the set of ALL advertised rates (the source
    rate when advertised, else the lowest advertised rate above it, else the
    highest). Returns (format, rate, channels, sample_bytes); channels and
    width come from the same first format."""
    ordered = sorted(formats, key=_purpose)
    if not ordered:
        raise ValueError("no formats advertised")
    first = ordered[0]
    rates = sorted({f.sample_rate for f in formats})
    if source_rate in rates:
        rate = source_rate
    else:
        above = [r for r in rates if r > source_rate]
        rate = above[0] if above else rates[-1]
    return first.format, rate, first.num_channels, first.sample_bytes or 2


# ---------------------------------------------------------------------------
# Faults
# ---------------------------------------------------------------------------


@dataclass
class Fault:
    """Per-stream failure injection, applied to the body bytes."""

    status: int | None = None  # answer with this status and an empty body (404, 500 ...)
    close_after: int | None = None  # abort the connection after this many body bytes
    stall_after: int | None = None  # stop sending (connection stays open) after this many body bytes
    first_byte_delay: float = 0.0


# ---------------------------------------------------------------------------
# Encoding
# ---------------------------------------------------------------------------


def ffmpeg_available() -> bool:
    return shutil.which("ffmpeg") is not None and shutil.which("ffprobe") is not None


async def _drain_stderr(proc, sink: list) -> None:
    try:
        while data := await proc.stderr.read(1024):
            sink.append(data)
            del sink[:-4]
    except (asyncio.CancelledError, ConnectionError):
        pass


async def _pace(frames_sent: int, rate: int, start: float, factor: float) -> None:
    """Sleeps so that audio is produced at `factor` times real time."""
    due = start + frames_sent / rate / factor
    delay = due - time.monotonic()
    await asyncio.sleep(delay if delay > 0 else 0)


async def encode(
    signal: Signal,
    fmt: str,
    rate: int,
    channels: int,
    width: int = 2,
    *,
    backend: str = "auto",
    src_file: str | None = None,
    realtime: float = 0.0,
    stderr_sink: list | None = None,
) -> AsyncIterator[bytes]:
    """Encoded audio bytes in arbitrary pieces; stops its processes on close.

    `src_file` (a finite WAV) selects HA's model: ffmpeg reads `-i src_file`
    and resamples to `rate`/`channels`. Without it the signal is synthesised
    at the target rate and channels and fed to ffmpeg's stdin (or written as
    WAV by the python backend). `realtime` > 0 paces production at that many
    times real time (0: as fast as the client reads).
    """
    if backend == "auto":
        backend = "ffmpeg" if ffmpeg_available() else "python"
    if backend == "python":
        if fmt != "wav" or width != 2:
            raise ValueError(f"the python backend serves 16-bit wav only, not {fmt}/{width}")
        yield wav_header(rate, channels)
        sig = signal.with_format(rate, channels)
        t0, frames = time.monotonic(), 0
        for chunk in sig.chunks():
            yield chunk
            frames += len(chunk) // (2 * channels)
            if realtime:
                await _pace(frames, rate, t0, realtime)
            else:
                await asyncio.sleep(0)
        return

    args = ["ffmpeg", "-hide_banner", "-loglevel", "error"]
    if src_file:
        if realtime:
            args += ["-re"]
        args += ["-nostdin", "-vn", "-i", src_file]
    else:
        args += ["-f", "s16le", "-ar", str(rate), "-ac", str(channels), "-i", "pipe:0"]
    args += ["-f", fmt, "-ar", str(rate), "-ac", str(channels)]
    if width == 2:
        args += ["-sample_fmt", "s16"]
    args += ["-map_metadata", "-1", "-vn", "pipe:1"]
    proc = await asyncio.create_subprocess_exec(
        *args,
        stdin=asyncio.subprocess.DEVNULL if src_file else asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    tasks = [asyncio.create_task(_drain_stderr(proc, stderr_sink if stderr_sink is not None else []))]
    try:
        if not src_file:
            tasks.append(asyncio.create_task(_feed(proc, signal.with_format(rate, channels), realtime)))
        while True:
            data = await proc.stdout.read(HA_CHUNK * 4)
            if not data:
                break
            yield data
    finally:
        # The kill is synchronous on purpose: a second cancellation (server
        # stop) must not leave a running ffmpeg behind; the reaping that
        # follows survives it (`shield`) and `_Server.stop` waits for it.
        for t in tasks:
            t.cancel()
        if proc.returncode is None:
            try:
                proc.kill()
            except ProcessLookupError:
                pass
        reaper = asyncio.ensure_future(_reap(proc))
        _REAPERS.add(reaper)
        reaper.add_done_callback(_REAPERS.discard)
        await asyncio.shield(reaper)


_REAPERS: set[asyncio.Future] = set()


async def _reap(proc) -> None:
    """Waits for a killed ffmpeg and closes its pipes (no zombie, no ResourceWarning)."""
    await proc.wait()
    try:
        await asyncio.wait_for(proc.stdout.read(), 2)  # to EOF: the transport closes
    except (TimeoutError, ConnectionError):
        pass


async def _feed(proc, sig: Signal, realtime: float) -> None:
    """Writes the signal's PCM to ffmpeg's stdin, then closes it."""
    t0, frames = time.monotonic(), 0
    try:
        for chunk in sig.chunks():
            proc.stdin.write(chunk)
            await proc.stdin.drain()
            frames += len(chunk) // (2 * sig.channels)
            if realtime:
                await _pace(frames, sig.rate, t0, realtime)
            else:
                await asyncio.sleep(0)
        proc.stdin.close()
    except (BrokenPipeError, ConnectionResetError):
        pass


async def _rechunk(source: AsyncIterator[bytes], size: int) -> AsyncIterator[bytes]:
    """Re-slices a byte stream to exactly `size`-byte pieces (the last may be shorter)."""
    buf = bytearray()
    try:
        async for piece in source:
            buf += piece
            while len(buf) >= size:
                yield bytes(buf[:size])
                del buf[:size]
        if buf:
            yield bytes(buf)
    finally:
        await source.aclose()  # stops the encoder (and ffmpeg) at once


# ---------------------------------------------------------------------------
# HTTP server
# ---------------------------------------------------------------------------


@dataclass
class Request:
    method: str
    path: str
    version: str
    headers: dict[str, str]  # lower-case names
    peer: tuple
    at: float


class _Server:
    """Minimal asyncio HTTP/1.1 server; subclasses implement `route`."""

    def __init__(self, host: str = "127.0.0.1", port: int = 0, *, sndbuf: int = 65536,
                 tls: tuple[str, str] | None = None, backend: str = "auto") -> None:
        self.host, self.port = host, port
        self.sndbuf = sndbuf
        self.tls = tls
        self.backend = backend
        self.requests: list[Request] = []
        self.events: list[dict] = []
        self._server: asyncio.base_events.Server | None = None
        self._handlers: set[asyncio.Task] = set()

    @property
    def scheme(self) -> str:
        return "https" if self.tls else "http"

    @property
    def base_url(self) -> str:
        return f"{self.scheme}://{self.host}:{self.port}"

    def log(self, ev: str, **kw) -> None:
        self.events.append({"ev": ev, "t": time.monotonic(), **kw})

    def count(self, ev: str, **match) -> int:
        return sum(1 for e in self.events if e["ev"] == ev and all(e.get(k) == v for k, v in match.items()))

    async def start(self) -> "_Server":
        ctx = None
        if self.tls:
            ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            ctx.load_cert_chain(*self.tls)
        self._server = await asyncio.start_server(self._handle, self.host, self.port, ssl=ctx)
        self.port = self._server.sockets[0].getsockname()[1]
        return self

    async def stop(self) -> None:
        if self._server:
            self._server.close()
        for t in list(self._handlers):
            t.cancel()
        if self._handlers:
            await asyncio.gather(*self._handlers, return_exceptions=True)
        if self._server:
            await self._server.wait_closed()
            self._server = None
        if _REAPERS:
            await asyncio.gather(*_REAPERS, return_exceptions=True)

    async def __aenter__(self):
        return await self.start()

    async def __aexit__(self, *exc) -> None:
        await self.stop()

    async def _handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        task = asyncio.current_task()
        self._handlers.add(task)
        sock = writer.get_extra_info("socket")
        try:
            if sock is not None and self.sndbuf:
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, self.sndbuf)
            writer.transport.set_write_buffer_limits(high=HA_CHUNK * 4)
            head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 10)
            lines = head.decode("latin-1").split("\r\n")
            method, path, version = (lines[0].split(" ") + ["", ""])[:3]
            headers = {}
            for ln in lines[1:]:
                if ":" in ln:
                    k, v = ln.split(":", 1)
                    headers[k.strip().lower()] = v.strip()
            req = Request(method, path, version, headers, writer.get_extra_info("peername"), time.monotonic())
            self.requests.append(req)
            await self.route(req, reader, writer)
        except (asyncio.CancelledError, ConnectionError, asyncio.IncompleteReadError,
                asyncio.LimitOverrunError, TimeoutError, ssl.SSLError, OSError):
            pass
        finally:
            self._handlers.discard(task)
            writer.transport.abort()

    async def route(self, req: Request, reader, writer) -> None:  # pragma: no cover
        raise NotImplementedError

    @staticmethod
    async def reply(writer, status: int, reason: str = "", body: bytes = b"") -> None:
        writer.write(
            f"HTTP/1.1 {status} {reason}\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)
        await writer.drain()

    async def stream(self, req: Request, reader, writer, source: AsyncIterator[bytes], *,
                     content_type: str, framing: str, end: str, fault: Fault, info: dict) -> bool:
        """Sends the response and the body; returns True when the body ended normally.

        `framing`: "chunked" (Transfer-Encoding) or "close" (no length, ends at
        close; always used for HTTP/1.0). `end`: "clean" finishes the body
        (terminal chunk, then close); "abort" closes without a terminal chunk.
        Returns False when the client went away or a fault cut the stream.
        The body is sent as one HTTP chunk per source piece.
        """
        if fault.status is not None:
            await self.reply(writer, fault.status, "Fault")
            return False
        if req.version == "HTTP/1.0":
            framing = "close"
        hdr = f"HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\n"
        hdr += "Transfer-Encoding: chunked\r\n\r\n" if framing == "chunked" else "\r\n"
        if fault.first_byte_delay:
            await asyncio.sleep(fault.first_byte_delay)
        writer.write(hdr.encode())
        await writer.drain()

        sent = 0

        async def pump() -> bool:
            nonlocal sent
            async for piece in source:
                if fault.close_after is not None and sent + len(piece) > fault.close_after:
                    piece = piece[: max(0, fault.close_after - sent)]
                    cut = True
                else:
                    cut = False
                if piece:
                    writer.write(b"%x\r\n%b\r\n" % (len(piece), piece) if framing == "chunked" else piece)
                    await writer.drain()
                    sent += len(piece)
                    info["bytes_sent"] = sent
                if cut:
                    return False
                if fault.stall_after is not None and sent >= fault.stall_after:
                    await asyncio.Event().wait()  # until cancelled
            if framing == "chunked" and end == "clean":
                writer.write(b"0\r\n\r\n")
                await writer.drain()
            return end == "clean"

        async def watch_eof() -> None:
            while await reader.read(1024):
                pass

        pump_t = asyncio.create_task(pump())
        eof_t = asyncio.create_task(watch_eof())
        try:
            await asyncio.wait({pump_t, eof_t}, return_when=asyncio.FIRST_COMPLETED)
            if pump_t.done():
                try:
                    return pump_t.result()
                except (ConnectionError, OSError):
                    return False
            return False  # client closed first
        finally:
            for t in (pump_t, eof_t):
                t.cancel()
            await asyncio.gather(pump_t, eof_t, return_exceptions=True)
            aclose = getattr(source, "aclose", None)
            if aclose:
                await aclose()


# ---------------------------------------------------------------------------
# ha-proxy
# ---------------------------------------------------------------------------


@dataclass
class Conversion:
    id: str
    device_id: str
    fmt: str
    rate: int
    channels: int
    width: int
    signal: Signal
    created: int  # creation order
    fetches: int = 0  # requests received (a restart counts)
    restarts: int = 0
    bytes_sent: int = 0  # of the latest run
    finished: bool = False  # body ended, or the client went away
    killed: bool = False  # evicted by a third URL; its URL answers 404
    active: asyncio.Task | None = None  # handler task of the running stream
    stderr: list = field(default_factory=list)

    @property
    def holds_slot(self) -> bool:
        return not self.finished and not self.killed


class HaProxy(_Server):
    """HA's ffmpeg proxy model (mode `ha-proxy`)."""

    def __init__(self, host: str = "127.0.0.1", port: int = 0, *, max_conversions: int = MAX_CONVERSIONS_PER_DEVICE,
                 realtime: float = 0.0, fault: Fault | None = None, **kw) -> None:
        super().__init__(host, port, **kw)
        self.max_conversions = max_conversions
        self.realtime = realtime
        self.fault = fault or Fault()
        self.conversions: dict[str, Conversion] = {}
        self._counter = 0
        self._tmp = tempfile.TemporaryDirectory(prefix="fake-proxy-")
        self._src_files: dict[Signal, str] = {}

    async def stop(self) -> None:
        await super().stop()
        self._tmp.cleanup()

    # -- what HA's integration does ---------------------------------------

    def create_url(self, device_id: str, fmt: str = "flac", rate: int = 48000, channels: int = 2,
                   width: int = 2, signal: Signal | None = None) -> str:
        """HA's `async_create_proxy_url`: registers a conversion and returns its URL.
        Creating a third unfinished conversion for a device kills the oldest one."""
        sig = signal or Signal()
        if sig.endless:
            raise ValueError("HA's proxy converts a finite media file")
        self._counter += 1
        conv = Conversion(secrets.token_urlsafe(12), device_id, fmt, rate, channels, width, sig, self._counter)
        self.conversions[conv.id] = conv
        self.log("create", id=conv.id, device=device_id, fmt=fmt, rate=rate, channels=channels)
        self._enforce(device_id, keep=conv)
        return f"{self.base_url}/api/esphome/ffmpeg_proxy/{device_id}/{conv.id}.{fmt}"

    def url_for_play_media(self, device_id: str, formats: Sequence, announce: bool = False,
                           signal: Signal | None = None) -> str:
        """What HA's `async_play_media` does: the format comes from the device's
        `supported_formats` (`ha_pick_format`), then a proxy URL is created."""
        f = ha_pick_format(formats, announce)
        if f is None:
            raise LookupError("no matching supported format: HA would pass the URL through unproxied")
        return self.create_url(device_id, f.format, f.sample_rate, f.num_channels, f.sample_bytes or 2, signal)

    def slots(self, device_id: str) -> list[Conversion]:
        return sorted((c for c in self.conversions.values() if c.device_id == device_id and c.holds_slot),
                      key=lambda c: c.created)

    def slot_count(self, device_id: str) -> int:
        return len(self.slots(device_id))

    def conversion_of(self, url: str) -> Conversion:
        return self.conversions[url.rsplit("/", 1)[1].rsplit(".", 1)[0]]

    def _enforce(self, device_id: str, keep: Conversion) -> None:
        while len(slots := self.slots(device_id)) > self.max_conversions:
            victim = next(c for c in slots if c is not keep)
            self._kill(victim)

    def _kill(self, conv: Conversion) -> None:
        conv.killed = True
        self.log("kill", id=conv.id, running=conv.active is not None)
        if conv.active is not None:
            conv.active.cancel()  # the handler aborts the connection and stops ffmpeg

    # -- HTTP --------------------------------------------------------------

    def _src(self, sig: Signal) -> str:
        if sig not in self._src_files:
            path = f"{self._tmp.name}/src{len(self._src_files)}.wav"
            with open(path, "wb") as f:
                f.write(sig.wav())
            self._src_files[sig] = path
        return self._src_files[sig]

    async def route(self, req: Request, reader, writer) -> None:
        parts = req.path.split("?", 1)[0].split("/")
        # ['', 'api', 'esphome', 'ffmpeg_proxy', device, 'token.fmt']
        conv = None
        if req.method == "GET" and len(parts) == 6 and parts[1:4] == ["api", "esphome", "ffmpeg_proxy"]:
            conv = self.conversions.get(parts[5].rsplit(".", 1)[0])
            if conv and (conv.device_id != parts[4] or parts[5].rsplit(".", 1)[-1] != conv.fmt):
                conv = None
        if req.method != "GET":
            return await self.reply(writer, 405, "Method Not Allowed")
        if conv is None or conv.killed:
            self.log("fetch-404", path=req.path)
            return await self.reply(writer, 404, "Not Found")
        # A new request restarts the conversion from the start: the running
        # run (if any) is stopped and its client cut off. `Range` is ignored.
        if conv.active is not None:
            conv.restarts += 1
            self.log("restart", id=conv.id)
            old = conv.active
            old.cancel()
            await asyncio.gather(old, return_exceptions=True)
        conv.fetches += 1
        conv.active = asyncio.current_task()
        conv.finished = False
        conv.bytes_sent = 0
        self._enforce(conv.device_id, keep=conv)
        self.log("fetch", id=conv.id, n=conv.fetches, range=req.headers.get("range"))
        info: dict = {}
        source = _rechunk(encode(conv.signal, conv.fmt, conv.rate, conv.channels, conv.width,
                                 backend=self.backend, src_file=self._src(conv.signal) if self._use_ffmpeg() else None,
                                 realtime=self.realtime, stderr_sink=conv.stderr), HA_CHUNK)
        try:
            ok = await self.stream(req, reader, writer, source, content_type=MIME.get(conv.fmt, "application/octet-stream"),
                                   framing="chunked", end="clean", fault=self.fault, info=info)
        finally:
            conv.bytes_sent = info.get("bytes_sent", 0)
            # Only the run that is still current finishes the conversion
            # (a cancelled run was restarted or killed by someone else).
            if conv.active is asyncio.current_task():
                conv.active = None
                conv.finished = True
                self.log("finish", id=conv.id, bytes=conv.bytes_sent)
        if not ok:
            self.log("cut", id=conv.id, bytes=conv.bytes_sent)

    def _use_ffmpeg(self) -> bool:
        return self.backend == "ffmpeg" or (self.backend == "auto" and ffmpeg_available())


# ---------------------------------------------------------------------------
# ma-flow
# ---------------------------------------------------------------------------


@dataclass
class FlowStream:
    id: str
    queue: str
    fmt: str
    rate: int
    channels: int
    signal: Signal
    framing: str
    end: str
    fault: Fault
    fetches: int = 0
    open: int = 0  # connections streaming right now
    max_open: int = 0
    bytes_sent: int = 0  # of the latest run


class MaFlow(_Server):
    """Music Assistant's flow mode (mode `ma-flow`): its own host and port, no
    `Content-Length`, one continuous multi-track stream per request, no
    one-time rule, no slot limit."""

    def __init__(self, host: str = "127.0.0.1", port: int = 0, *, chunk: int = 4096,
                 realtime: float = 0.0, **kw) -> None:
        super().__init__(host, port, **kw)
        self.chunk = chunk
        self.realtime = realtime
        self.streams: dict[str, FlowStream] = {}

    def create_stream(self, queue: str = "queue1", *, fmt: str = "flac", rate: int = 44100, channels: int = 2,
                      tracks: Sequence[Track] | None = None, endless: bool = False, framing: str = "chunked",
                      end: str = "clean", fault: Fault | None = None, noise: float = 0.05) -> str:
        """Returns the stream URL. `tracks` defaults to three short tracks with
        different pitches; `endless` repeats them forever (no end at all)."""
        tracks = tuple(tracks or (Track(1.0, 330.0), Track(1.0, 440.0), Track(1.0, 550.0)))
        sig = Signal(tracks, rate, channels, noise, endless=endless)
        s = FlowStream(secrets.token_urlsafe(12), queue, fmt, rate, channels, sig, framing, end, fault or Fault())
        self.streams[s.id] = s
        self.log("create", id=s.id, fmt=fmt, rate=rate, channels=channels, endless=endless)
        return f"{self.base_url}/flow/{s.id}/{queue}.{fmt}"

    def url_for_play_media(self, formats: Sequence, source_rate: int = 44100, queue: str = "queue1",
                           **kw) -> str:
        """What MA does with `bypass_proxy`: its own URL, in the format of
        `ma_pick_format`."""
        fmt, rate, channels, _width = ma_pick_format(formats, source_rate)
        return self.create_stream(queue, fmt=fmt, rate=rate, channels=channels, **kw)

    def stream_of(self, url: str) -> FlowStream:
        return self.streams[url.rsplit("/", 2)[1]]

    async def route(self, req: Request, reader, writer) -> None:
        parts = req.path.split("?", 1)[0].split("/")  # ['', 'flow', id, 'queue.fmt']
        s = self.streams.get(parts[2]) if len(parts) == 4 and parts[1] == "flow" else None
        if req.method != "GET":
            return await self.reply(writer, 405, "Method Not Allowed")
        if s is None:
            self.log("fetch-404", path=req.path)
            return await self.reply(writer, 404, "Not Found")
        s.fetches += 1
        s.open += 1
        s.max_open = max(s.max_open, s.open)
        self.log("fetch", id=s.id, n=s.fetches)
        info: dict = {}
        source = _rechunk(encode(s.signal, s.fmt, s.rate, s.channels, 2, backend=self.backend,
                                 realtime=self.realtime), self.chunk)
        try:
            ok = await self.stream(req, reader, writer, source, content_type=MIME.get(s.fmt, "application/octet-stream"),
                                   framing=s.framing, end=s.end, fault=s.fault, info=info)
        finally:
            s.open -= 1
            s.bytes_sent = info.get("bytes_sent", 0)
            self.log("close", id=s.id, bytes=s.bytes_sent)
        if not ok:
            self.log("cut", id=s.id, bytes=s.bytes_sent)


# ---------------------------------------------------------------------------
# A raw client for tests and scenarios (sees chunk framing; the satellite's
# own client is ureq, this one reports what a client could observe)
# ---------------------------------------------------------------------------


@dataclass
class Fetched:
    status: int = 0
    reason: str = ""
    headers: dict[str, str] = field(default_factory=dict)
    chunks: list[int] = field(default_factory=list)  # sizes of the HTTP chunks (chunked framing)
    body: bytes = b""
    complete: bool = False  # the body ended by its framing (terminal chunk) or by a clean close
    truncated: bool = False  # the connection ended inside the body (or without a terminal chunk)


async def fetch(url: str, *, headers: dict[str, str] | None = None, max_bytes: int | None = None,
                timeout: float = 15.0, ca_file: str | None = None, version: str = "1.1") -> Fetched:
    """GET `url`; reads the body until its end or `max_bytes` (then closes)."""
    from urllib.parse import urlsplit

    u = urlsplit(url)
    ctx = None
    if u.scheme == "https":
        ctx = ssl.create_default_context(cafile=ca_file)
    reader, writer = await asyncio.open_connection(u.hostname, u.port or (443 if ctx else 80), ssl=ctx)
    out = Fetched()
    try:
        path = (u.path or "/") + (f"?{u.query}" if u.query else "")
        hdrs = {"Host": f"{u.hostname}:{u.port}", **(headers or {})}
        writer.write((f"GET {path} HTTP/{version}\r\n" + "".join(f"{k}: {v}\r\n" for k, v in hdrs.items()) + "\r\n").encode())
        await writer.drain()

        async def run() -> None:
            head = await reader.readuntil(b"\r\n\r\n")
            lines = head.decode("latin-1").split("\r\n")
            _v, code, *reason = lines[0].split(" ", 2)
            out.status, out.reason = int(code), " ".join(reason)
            for ln in lines[1:]:
                if ":" in ln:
                    k, v = ln.split(":", 1)
                    out.headers[k.strip().lower()] = v.strip()
            body = bytearray()
            try:
                if out.headers.get("transfer-encoding") == "chunked":
                    while True:
                        size = int((await reader.readuntil(b"\r\n")).strip().split(b";")[0], 16)
                        if size == 0:
                            await reader.readuntil(b"\r\n")
                            out.complete = True
                            break
                        data = await reader.readexactly(size)
                        await reader.readexactly(2)
                        out.chunks.append(size)
                        body += data
                        if max_bytes is not None and len(body) >= max_bytes:
                            break
                elif "content-length" in out.headers:
                    n = int(out.headers["content-length"])
                    body += await reader.readexactly(n)
                    out.complete = True
                else:  # close-delimited
                    while data := await reader.read(65536):
                        body += data
                        if max_bytes is not None and len(body) >= max_bytes:
                            break
                    else:
                        out.complete = True
            except (asyncio.IncompleteReadError, ConnectionError):
                out.truncated = True
            out.body = bytes(body)

        await asyncio.wait_for(run(), timeout)
    finally:
        writer.transport.abort()
    return out


def make_private_ca(directory: str) -> tuple[str, str, str]:
    """A private CA and a server certificate for 127.0.0.1, made with the
    `openssl` CLI (the HTTPS variant with a `ca_file`). Returns (ca_cert,
    server_cert, server_key). Test material only, generated per run."""
    import subprocess

    def sh(*a: str) -> None:
        subprocess.run(a, cwd=directory, check=True, capture_output=True)

    sh("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "ca.key", "-out", "ca.pem",
       "-days", "2", "-subj", "/CN=fake-proxy test CA",
       "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign",
       "-addext", "subjectKeyIdentifier=hash")
    sh("openssl", "req", "-newkey", "rsa:2048", "-nodes", "-keyout", "srv.key", "-out", "srv.csr", "-subj", "/CN=127.0.0.1")
    with open(f"{directory}/ext.cnf", "w") as f:
        f.write("subjectAltName=IP:127.0.0.1\nauthorityKeyIdentifier=keyid\n")
    sh("openssl", "x509", "-req", "-in", "srv.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial",
       "-out", "srv.pem", "-days", "2", "-extfile", "ext.cnf")
    return f"{directory}/ca.pem", f"{directory}/srv.pem", f"{directory}/srv.key"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


async def _cli(args: argparse.Namespace) -> None:
    if args.mode == "ha-proxy":
        srv: _Server = HaProxy(args.host, args.port, realtime=args.realtime, backend=args.backend)
        await srv.start()
        url = srv.create_url("dev", args.format, args.rate, args.channels, signal=Signal((Track(args.seconds),)))
    else:
        srv = MaFlow(args.host, args.port, realtime=args.realtime, backend=args.backend)
        await srv.start()
        url = srv.create_stream(fmt=args.format, rate=args.rate, channels=args.channels, endless=args.endless,
                                framing=args.framing)
    print(f"URL {url}", flush=True)
    try:
        await asyncio.Event().wait()
    finally:
        await srv.stop()


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--mode", choices=("ha-proxy", "ma-flow"), required=True)
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=0)
    p.add_argument("--format", default="flac")
    p.add_argument("--rate", type=int, default=48000)
    p.add_argument("--channels", type=int, default=2)
    p.add_argument("--seconds", type=float, default=10.0, help="ha-proxy source length")
    p.add_argument("--endless", action="store_true", help="ma-flow: never end")
    p.add_argument("--framing", choices=("chunked", "close"), default="chunked", help="ma-flow body framing")
    p.add_argument("--realtime", type=float, default=0.0, help="produce audio at this many times real time (0: unpaced)")
    p.add_argument("--backend", choices=("auto", "ffmpeg", "python"), default="auto")
    try:
        asyncio.run(_cli(p.parse_args()))
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
