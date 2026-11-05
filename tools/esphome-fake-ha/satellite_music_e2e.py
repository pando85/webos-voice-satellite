#!/usr/bin/env python3
"""Music end-to-end scenarios S1 to S15 (docs/design/music-playback.md, M5).

The real `webos-satellite` daemon with `[music] enabled`, a file microphone,
the WAV sinks (music is written to a growing `music-<epoch>-<n>.wav` per
track, in real time; no sound) and a fake luna-send for the power state,
against the fake HA (aioesphomeapi 46.6.0) and `fake_proxy.py` in its two
modes: `ha-proxy` (HA's ffmpeg proxy: 2048-byte chunks, no length, at most 2
conversions per device) and `ma-flow` (Music Assistant's endless or
close-ended multi-track stream on its own port).

Run from satellite_e2e.py (`run.sh --satellite-only`) or alone:
    tools/esphome-fake-ha/.venv/bin/python -I tools/esphome-fake-ha/satellite_music_e2e.py --binary PATH

Levels are read from the WAV the music slot writes: the position in the file
is the engine's clock (at most `write_ahead_ms` ahead of real time), so a
duck is located by the file position when the event was sent. Everything
stays on 127.0.0.1 (plus this host's own address for the refused-target
check, which must never be connected to) and in a temporary directory.
"""

from __future__ import annotations

import argparse
import array
import asyncio
import base64
import math
import os
import re
import socket
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fake_proxy  # noqa: E402
from fake_ha import (  # noqa: E402
    MEDIA_FLAGS_MUSIC_ANNOUNCE,
    MEDIA_FLAGS_MUSIC_OFF,
    RESULTS,
    TIMEOUT,
    USE_WAKE_WORD,
    FakeHA,
    check,
    until_true,
)
from satellite_e2e import FAKE_LUNA, POINTS, Satellite, free_port, write_mic  # noqa: E402

from aioesphomeapi import MediaPlayerState  # noqa: E402
from aioesphomeapi import VoiceAssistantEventType as Ev  # noqa: E402
from aioesphomeapi import VoiceAssistantTimerEventType as TimerEv  # noqa: E402

PLAYING, PAUSED, IDLE = MediaPlayerState.PLAYING, MediaPlayerState.PAUSED, MediaPlayerState.IDLE
WIN = 0.02  # seconds per level window


# ---------------------------------------------------------------- WAV reading


def music_files(d: str) -> list[str]:
    """The music slot's files in track order (`music-<epoch>-<n>.wav`)."""
    out = []
    for f in os.listdir(d):
        m = re.fullmatch(r"music-\d+-(\d+)\.wav", f)
        if m:
            out.append((int(m.group(1)), os.path.join(d, f)))
    return [p for _, p in sorted(out)]


def wav_fmt(path: str) -> tuple[int, int]:
    with open(path, "rb") as f:
        h = f.read(44)
    return int.from_bytes(h[24:28], "little"), int.from_bytes(h[22:24], "little")


def frames_of(path: str) -> int:
    _rate, ch = wav_fmt(path)
    return max(0, os.path.getsize(path) - 44) // (2 * ch)


def samples(path: str) -> array.array:
    with open(path, "rb") as f:
        data = f.read()[44:]
    a = array.array("h")
    a.frombytes(data[: len(data) // 2 * 2])
    return a


def level_db(path: str, start_s: float, end_s: float) -> float:
    """RMS in dBFS of the frames between two times of the file."""
    rate, ch = wav_fmt(path)
    a = samples(path)
    lo, hi = int(start_s * rate) * ch, int(end_s * rate) * ch
    seg = a[max(0, lo):max(0, hi)]
    if not seg:
        return -200.0
    rms = math.sqrt(sum(s * s for s in seg) / len(seg))
    return 20 * math.log10(rms / 32768) if rms > 0 else -200.0


def envelope(path: str, start_s: float, end_s: float) -> list[tuple[float, float]]:
    """(time, dBFS) per WIN window."""
    out, t = [], start_s
    while t + WIN <= end_s:
        out.append((t, level_db(path, t, t + WIN)))
        t += WIN
    return out


def own_address() -> str | None:
    """This host's outbound address (never connected to: the guard refuses it)."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("192.0.2.1", 9))
            ip = s.getsockname()[0]
        return None if ip.startswith("127.") else ip
    except OSError:
        return None


# ---------------------------------------------------------------- the daemon


def write_config(tmp: str, name: str, port: int, mac: str, *, music: str, device: str = "",
                 luna: str | None = None) -> str:
    wav = os.path.join(tmp, f"wav-{name}")
    os.makedirs(wav, exist_ok=True)
    os.makedirs(os.path.join(tmp, f"run-{name}"), exist_ok=True)
    config = os.path.join(tmp, f"{name}.toml")
    power = f'[power]\nluna_send = "{luna}"\n' if luna else "[power]\nwatch = false\n"
    with open(config, "w") as f:
        f.write(f"""
mode = "wake"
[device]
name = "{name}"
mac = "{mac}"
bind = "127.0.0.1"
port = {port}
{device}
[mic]
source = "file"
path = "mic-music.raw"
loop = true
[sink]
type = "wav"
dir = "{wav}"
volume = 100
volume_file = "{name}-volume"
[log]
level = "info"
{power}
[report]
dir = "{tmp}/run-{name}"
[music]
{music}
""")
    return config


async def media_ha(port: int, psk: str | None) -> FakeHA:
    ha = FakeHA(port, psk)
    await ha.connect()
    await ha.load_media_player()
    ha.track_media_state()
    return ha


def states_since(ha: FakeHA, mark: int) -> list:
    """The states HA received since `mark`, repeats collapsed (a volume change
    is answered with the unchanged state)."""
    out: list = []
    for s in ha.media_states[mark:]:
        if not out or out[-1] != s.state:
            out.append(s.state)
    return out


def token(url: str) -> str:
    """The secret part of a proxy or flow URL (the random id), for the log check."""
    return url.rstrip("/").rsplit("/", 2)[-2] if "/flow/" in url else url.rsplit("/", 1)[1].split(".")[0]


async def grows(path: str, seconds: float) -> int:
    """Frames the file grew by in `seconds`."""
    a = frames_of(path)
    await asyncio.sleep(seconds)
    return frames_of(path) - a


async def new_file(d: str, count: int, timeout: float = TIMEOUT) -> str | None:
    ok = await until_true(lambda: len(music_files(d)) > count, timeout)
    return music_files(d)[-1] if ok else None


# ---------------------------------------------------------------- scenarios


async def scenario_music(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: music (M5 S1 to S13), Noise, wav sinks, fake proxy and fake luna ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    with open(os.path.join(tmp, "psk-music"), "w") as f:
        f.write(psk + "\n")
    os.chmod(os.path.join(tmp, "psk-music"), 0o600)
    write_mic(os.path.join(tmp, "mic-music.raw"), 20.0)
    tv = os.path.join(tmp, "tv-music")
    os.mkdir(tv)
    for n, text in (("vol", "12"), ("mute", "false"), ("fg", "app.a"), ("power", "Active"), ("points", POINTS)):
        with open(os.path.join(tv, n), "w") as f:
            f.write(text + "\n")
    luna = os.path.join(tmp, "luna-send-music")
    with open(luna, "w") as f:
        f.write(FAKE_LUNA.replace("@D@", tv))
    os.chmod(luna, 0o755)
    port = free_port()
    name = "e2e-music"
    config = write_config(tmp, name, port, "02:00:00:00:00:e7", luna=luna,
                          device='noise_psk_file = "psk-music"',
                          music="enabled = true\nidle_timeout_s = 2\ndisconnect_stop_s = 3\n")
    wav = os.path.join(tmp, f"wav-{name}")
    sat = Satellite(binary, config, os.path.join(tmp, "satellite-music.log"))
    await sat.start()
    secrets_seen: list[str] = []
    try:
        ha = await media_ha(port, psk)
        cli = ha.cli
        ha.subscribe_voice()
        check("S0 music on: flags 1201669, DEFAULT flac 48000 Hz stereo after the ANNOUNCEMENT entry",
              int(ha.media_flags()) == MEDIA_FLAGS_MUSIC_ANNOUNCE
              and [(f.format, f.sample_rate, f.num_channels) for f in ha.media_formats()]
              == [("flac", 22050, 1), ("flac", 48000, 2)], str(ha.media_formats()))
        await ha.next_start()  # the armed wake word run
        voice = os.path.join(tmp, f"run-{name}", "voice.json")
        ok = await until_true(lambda: os.path.exists(voice) and '"music":{"enabled":true,"state":"idle"' in open(voice).read())
        check("S0a run/voice.json carries the music block (idle)", ok, open(voice).read() if os.path.exists(voice) else "")

        async with fake_proxy.HaProxy() as proxy, fake_proxy.MaFlow() as flow:
            dev = ha.device_name
            # ---- S1 play, finish, idle
            mark, n = len(ha.media_states), len(music_files(wav))
            url = ha.play_media_like_ha(proxy, signal=fake_proxy.Signal((fake_proxy.Track(1.5),)))
            secrets_seen.append(token(url))
            conv = proxy.conversion_of(url)
            ok = await ha.wait_media_state(PLAYING)
            ok2 = await until_true(lambda: ha.media_state.state == IDLE, 6)
            path = music_files(wav)[-1] if len(music_files(wav)) > n else None
            dur = frames_of(path) / 48000 if path else 0
            check("S1 play_media (HA proxy, flac 48 kHz): playing, the whole track written, then idle",
                  ok and ok2 and path is not None and wav_fmt(path) == (48000, 2) and abs(dur - 1.5) < 0.1
                  and states_since(ha, mark) == [PLAYING, IDLE], f"dur={dur:.3f} states={states_since(ha, mark)}")
            check("S1a fetched once, the conversion finished and its slot is free",
                  conv.fetches == 1 and conv.finished and proxy.slot_count(dev) == 0
                  and "music: track 1 ended: ended" in sat.text(), f"fetches={conv.fetches}")
            lvl = level_db(path, 0.3, 1.2)
            check("S1b level at volume 100 % is the source level (sine 0.5 + noise: about -9 dBFS)",
                  -11.0 < lvl < -7.0, f"{lvl:.1f} dBFS")

            # ---- S2 pause, resume, stop; S3 volume while playing
            mark, n = len(ha.media_states), len(music_files(wav))
            url = ha.play_media_like_ha(proxy, signal=fake_proxy.Signal((fake_proxy.Track(12.0),)))
            secrets_seen.append(token(url))
            conv = proxy.conversion_of(url)
            path = await new_file(wav, n)
            await ha.wait_media_state(PLAYING)
            await asyncio.sleep(0.6)
            ha.media_pause()
            ok = await ha.wait_media_state(PAUSED)
            await asyncio.sleep(0.3)
            grew = await grows(path, 1.0)
            check("S2 pause: paused, nothing written while paused", ok and grew == 0, f"grew={grew} frames")
            ha.media_resume()
            ok = await ha.wait_media_state(PLAYING)
            grew = await grows(path, 0.5)
            check("S2a play resumes the same stream (no new fetch)", ok and grew > 15000 and conv.fetches == 1,
                  f"grew={grew} fetches={conv.fetches}")
            t_before = frames_of(path) / 48000
            await asyncio.sleep(0.4)
            ha.volume_set(0.5)
            ok = await until_true(lambda: ha.media_state is not None and abs(ha.media_state.volume - 0.5) < 1e-6)
            t_set = frames_of(path) / 48000
            await asyncio.sleep(0.6)
            before = level_db(path, t_before, t_before + 0.3)
            after = level_db(path, t_set + 0.15, t_set + 0.45)
            check("S3 volume 50 % while playing: the shared volume lowers the music by 25 dB (0.5 dB per percent)",
                  ok and 22.0 < before - after < 28.0, f"{before:.1f} -> {after:.1f} dBFS")
            ha.volume_set(1.0)
            await asyncio.sleep(0.3)
            ha.media_stop()
            ok = await ha.wait_media_state(IDLE)
            await asyncio.sleep(0.3)
            grew = await grows(path, 0.5)
            check("S2b stop: idle, the file stops growing, the URL is not fetched again",
                  ok and grew == 0 and conv.fetches == 1 and states_since(ha, mark) == [PLAYING, PAUSED, PLAYING, IDLE]
                  and "music: stopping (stopped)" in sat.text(), f"states={states_since(ha, mark)}")

            # ---- S4 announce over music (MA flow music, an announcement through HA's proxy)
            n = len(music_files(wav))
            mark = len(ha.media_states)
            murl = flow.create_stream(fmt="flac", rate=48000, channels=2, endless=True)
            ha.play_media(murl)
            secrets_seen.append(token(murl))
            path = await new_file(wav, n)
            await ha.wait_media_state(PLAYING)
            await asyncio.sleep(1.0)
            t0 = frames_of(path) / 48000
            aurl = ha.play_media_like_ha(proxy, announce=True,
                                         signal=fake_proxy.Signal((fake_proxy.Track(1.0, 880.0),)))
            secrets_seen.append(token(aurl))
            ok = await until_true(lambda: "music: announcement" in sat.text())
            await until_true(lambda: any(f.startswith("media-announce-") for f in os.listdir(wav)), 6)
            await asyncio.sleep(2.6)  # hold 800 ms + release 1000 ms after the announcement
            t1 = frames_of(path) / 48000
            env = envelope(path, t0 - 0.5, t1)
            base = level_db(path, t0 - 0.5, t0)
            low = min(v for _, v in env)
            end = level_db(path, t1 - 0.3, t1)
            check("S4 announce: played on the reply path (media-announce wav), music ducked by about 20 dB and restored",
                  ok and any(f.startswith("media-announce-") for f in os.listdir(wav))
                  and 17.0 < base - low < 23.0 and abs(base - end) < 1.5, f"base={base:.1f} low={low:.1f} end={end:.1f}")
            check("S4a the entity stays playing (no idle), HA gets no announce-finished",
                  states_since(ha, mark) == [PLAYING] and ha.announce_finished.empty(),
                  f"states={states_since(ha, mark)}")
            log = sat.text()
            check("S4b duck begin and end logged at info (source and depth, no URL)",
                  "music: duck begin (Announcement, -20 dB)" in log
                  and "music: duck end (Announcement; released after 800 ms over 1000 ms)" in log,
                  "\n".join(l for l in log.splitlines() if "duck" in l)[-400:])

            # ---- S5 a voice run during music: wake word duck timing, reply, run end, release
            while not ha.stops.empty():
                ha.stops.get_nowait()
            ha.event(Ev.VOICE_ASSISTANT_RUN_START)
            ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_START)
            tw = frames_of(path) / 48000
            ha.event(Ev.VOICE_ASSISTANT_WAKE_WORD_END)
            ok = await sat.wait_log("wake word detected by HA")
            await asyncio.sleep(0.6)
            env = envelope(path, tw, tw + 0.6)
            base = level_db(path, tw - 0.5, tw)
            reached = next((t - tw for t, v in env if base - v >= 15.0), None)
            check("S5 wake word: the music is ducked 15 dB within 400 ms of the event in the written stream (budget 210 ms incl. pacat 100 ms)",
                  ok and reached is not None and reached + WIN <= 0.4,
                  f"reached after {None if reached is None else round((reached + WIN) * 1000)} ms")
            ha.event(Ev.VOICE_ASSISTANT_STT_START)
            ha.event(Ev.VOICE_ASSISTANT_STT_VAD_END)
            await asyncio.wait_for(ha.stops.get(), TIMEOUT)
            ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "what time is it"})
            ha.event(Ev.VOICE_ASSISTANT_INTENT_START)
            # ---- S6 continue_conversation keeps the duck between the two runs
            ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "m1", "speech": "It is noon. Anything else?",
                                                     "continue_conversation": "1"})
            ha.event(Ev.VOICE_ASSISTANT_TTS_START, {"text": "x"})
            ha.event(Ev.VOICE_ASSISTANT_TTS_END, {"url": "http://127.0.0.1:1/api/tts_proxy/x.wav"})
            ha.event(Ev.VOICE_ASSISTANT_RUN_END)
            await ha.stream_tts(20)
            conv_id, flags, _ = await ha.next_start()
            check("S6 continue_conversation: a follow-up run starts without the wake word",
                  conv_id == "m1" and not flags & USE_WAKE_WORD, f"flags={flags}")
            await asyncio.sleep(1.2)  # longer than the 800 ms hold
            tmid = frames_of(path) / 48000
            ducked_mid = base - level_db(path, tmid - 0.3, tmid)
            ha.event(Ev.VOICE_ASSISTANT_STT_END, {"text": "no"})
            ha.event(Ev.VOICE_ASSISTANT_INTENT_END, {"conversation_id": "m1", "speech": "", "continue_conversation": "0"})
            tend = frames_of(path) / 48000
            ha.event(Ev.VOICE_ASSISTANT_RUN_END)
            await ha.next_start()  # re-armed
            await asyncio.sleep(2.4)
            t2 = frames_of(path) / 48000
            env = envelope(path, tend, t2)
            back = next((t - tend for t, v in env if base - v < 1.0), None)
            check("S6a the duck holds through the reply and the follow-up run (no release in between)",
                  ducked_mid > 17.0, f"ducked {ducked_mid:.1f} dB 1.2 s after the reply")
            check("S5a after the last run ends: hold 800 ms then release over 1000 ms (back within 1 dB after about 1.8 s)",
                  back is not None and 1.5 < back < 2.2, f"back after {back} s")
            check("S5b the reply was written to its own wav (the reply path is not ducked)",
                  any(f.startswith("reply-") for f in os.listdir(wav)))

            # ---- S7 timer alarm over music, STOP stops the alarm first, then the music
            mark = len(ha.media_states)
            ta = frames_of(path) / 48000
            cli.send_voice_assistant_timer_event(TimerEv.VOICE_ASSISTANT_TIMER_FINISHED, "m-1", "", 5, 0, False)
            ok = await sat.wait_log("timer alarm: ringing")
            await asyncio.sleep(0.5)
            tb = frames_of(path) / 48000
            ducked = level_db(path, ta - 0.4, ta) - level_db(path, tb - 0.2, tb)
            ha.media_stop()
            ok2 = await sat.wait_log("timer alarm stopped (media player stop")
            await asyncio.sleep(0.4)
            check("S7 alarm over music: the chime ducks the music; the first STOP stops the alarm, the music plays on",
                  ok and ok2 and ducked > 17.0 and IDLE not in states_since(ha, mark) and (await grows(path, 0.3)) > 10000,
                  f"ducked={ducked:.1f} dB states={states_since(ha, mark)}")
            ha.media_stop()
            ok = await ha.wait_media_state(IDLE)
            s = flow.stream_of(murl)
            ok2 = await until_true(lambda: s.open == 0)
            check("S7a the second STOP stops the music; the MA stream connection is closed",
                  ok and ok2 and s.fetches == 1, f"open={s.open} fetches={s.fetches}")

            # ---- S11 MA flow: endless multi-track 44.1 kHz stream, then one that ends at close
            mark, n = len(ha.media_states), len(music_files(wav))
            murl = flow.create_stream(fmt="flac", rate=44100, channels=2, endless=True)
            ha.play_media(murl)
            secrets_seen.append(token(murl))
            path = await new_file(wav, n)
            await ha.wait_media_state(PLAYING)
            await asyncio.sleep(4.0)  # longer than the 3 s pass of three 1 s tracks
            secs = frames_of(path) / 44100
            check("S11 MA flow stream: 44.1 kHz stereo from the header, plays across track changes, stays playing",
                  path is not None and wav_fmt(path) == (44100, 2) and secs > 3.5
                  and states_since(ha, mark) == [PLAYING], f"{secs:.2f} s states={states_since(ha, mark)}")
            n = len(music_files(wav))
            murl = flow.create_stream(fmt="flac", rate=44100, channels=2, framing="close",
                                      tracks=(fake_proxy.Track(0.7, 330.0), fake_proxy.Track(0.7, 550.0)))
            ha.play_media(murl)
            secrets_seen.append(token(murl))
            path = await new_file(wav, n)
            ok = await until_true(lambda: ha.media_state.state == IDLE, 6)
            secs = frames_of(path) / 44100 if path else 0
            check("S11a a new play_media replaces the endless stream; a close-ended body plays to its end, then idle",
                  ok and abs(secs - 1.4) < 0.15 and states_since(ha, mark) == [PLAYING, IDLE]
                  and "ended: ended" in sat.text(), f"{secs:.2f} s states={states_since(ha, mark)}")

            # ---- S12 HA proxy eviction by a third URL: the track ends, idle, no retry. The proxy is
            # paced at real time (like ffmpeg reading a live source), so the conversion is still
            # running when the third URL is created (a fast one would be done and hold no slot).
            async with fake_proxy.HaProxy(realtime=1.0) as paced:
                mark = len(ha.media_states)
                url = ha.play_media_like_ha(paced, signal=fake_proxy.Signal((fake_proxy.Track(30.0),)))
                secrets_seen.append(token(url))
                conv = paced.conversion_of(url)
                await ha.wait_media_state(PLAYING)
                await asyncio.sleep(1.5)
                fmts = ha.media_formats()
                u2 = paced.url_for_play_media(dev, fmts, False, fake_proxy.Signal((fake_proxy.Track(1.0),)))
                u3 = paced.url_for_play_media(dev, fmts, False, fake_proxy.Signal((fake_proxy.Track(1.0),)))
                t = time.monotonic()
                ok = await until_true(lambda: ha.media_state.state == IDLE, 12)
                ended = time.monotonic() - t
                await asyncio.sleep(0.5)
                check("S12 HA proxy: a third URL evicts the playing conversion; the track ends, idle, never fetched again",
                      conv.killed and ok and conv.fetches == 1 and states_since(ha, mark) == [PLAYING, IDLE]
                      and paced.slot_count(dev) == 2 and re.search(r"track \d+ ended: error", sat.text()) is not None,
                      f"killed={conv.killed} idle after {ended:.1f} s fetches={conv.fetches} slots={paced.slot_count(dev)}")

                # ---- S13 ENQUEUE is refused and its URL opened and closed: the slots come back
                ha.enqueue_media(u2)
                ha.enqueue_media(u3)
                ok = await until_true(lambda: paced.slot_count(dev) == 0, 6)
                c2, c3 = paced.conversion_of(u2), paced.conversion_of(u3)
                await asyncio.sleep(0.5)
                check("S13 ENQUEUE refused: each URL opened once and closed, the proxy's slot count is back to 0, nothing played",
                      ok and c2.fetches == 1 and c3.fetches == 1 and ha.media_state.state == IDLE
                      and sat.text().count("opened and closed without playing") >= 2,
                      f"slots={paced.slot_count(dev)} fetches={c2.fetches},{c3.fetches}")

            # ---- S8 stall and early close end the track, idle, no retry
            async with fake_proxy.HaProxy(fault=fake_proxy.Fault(stall_after=120_000)) as stall, \
                    fake_proxy.HaProxy(fault=fake_proxy.Fault(close_after=120_000)) as early:
                mark = len(ha.media_states)
                url = ha.play_media_like_ha(stall, signal=fake_proxy.Signal((fake_proxy.Track(20.0),)))
                await ha.wait_media_state(PLAYING)
                t = time.monotonic()
                ok = await until_true(lambda: ha.media_state.state == IDLE, 10)
                c = stall.conversion_of(url)
                check("S8 a stalled stream ends after idle_timeout_s (2 s) once the buffers ran dry; idle; no retry",
                      ok and c.fetches == 1 and "ended: idle_timeout" in sat.text()
                      and states_since(ha, mark) == [PLAYING, IDLE], f"{time.monotonic() - t:.1f} s")
                mark = len(ha.media_states)
                url = ha.play_media_like_ha(early, signal=fake_proxy.Signal((fake_proxy.Track(20.0),)))
                await ha.wait_media_state(PLAYING)
                ok = await until_true(lambda: ha.media_state.state == IDLE, 10)
                c = early.conversion_of(url)
                check("S8a a connection closed early (no terminal chunk) ends the track with an error; idle; no retry",
                      ok and c.fetches == 1 and states_since(ha, mark) == [PLAYING, IDLE]
                      and re.search(r"track \d+ ended: error", sat.text()) is not None, "")

            # ---- S15 a TV-local (own) address or link-local target: refused, never connected to
            ip = own_address()
            refusals = sat.text().count("fetch target refused")
            ha.play_media("http://169.254.169.254/latest/meta-data/x.flac")
            ok = await until_true(lambda: sat.text().count("fetch target refused") > refusals, 6)
            check("S15 a link-local target is refused before connecting (state stays idle)",
                  ok and "link-local address" in sat.text() and ha.media_state.state == IDLE)
            if ip is None:
                print("SKIP  S15a no non-loopback address on this host")
            else:
                async with fake_proxy.HaProxy(host="0.0.0.0") as own:
                    u = own.create_url(dev).replace("0.0.0.0", ip)
                    refusals = sat.text().count("fetch target refused")
                    ha.play_media(u)
                    ok = await until_true(lambda: sat.text().count("fetch target refused") > refusals, 6)
                    await asyncio.sleep(0.3)
                    check("S15a this host's own address is refused (address of this device), no request reaches it",
                          ok and "address of this device" in sat.text() and not own.requests, f"{len(own.requests)} requests")

            # ---- S9 HA disconnect: plays on, a reconnect within the grace keeps it; else stops after 3 s
            n = len(music_files(wav))
            murl = flow.create_stream(fmt="flac", rate=48000, channels=2, endless=True)
            ha.play_media(murl)
            secrets_seen.append(token(murl))
            path = await new_file(wav, n)
            await ha.wait_media_state(PLAYING)
            await cli.disconnect()
            await asyncio.sleep(2.0)
            ha = await media_ha(port, psk)
            cli = ha.cli
            await asyncio.sleep(2.0)
            check("S9 HA disconnect for 2 s (grace 3 s): the music plays on, HA sees playing again",
                  (await grows(path, 0.5)) > 15000 and "Home Assistant is back" in sat.text()
                  and await ha.wait_media_state(PLAYING), "")
            await cli.disconnect()
            await asyncio.sleep(1.5)
            mid = await grows(path, 0.5)
            await asyncio.sleep(2.0)
            after = await grows(path, 0.5)
            ha = await media_ha(port, psk)
            cli = ha.cli
            check("S9a HA gone longer than disconnect_stop_s: plays on during the grace, then stops; idle on reconnect",
                  mid > 15000 and after == 0 and "stayed disconnected for 3 s" in sat.text()
                  and await ha.wait_media_state(IDLE), f"mid={mid} after={after}")

            # ---- Active Standby (D6): stops the music, refuses play_media (released), back on Active
            n = len(music_files(wav))
            murl = flow.create_stream(fmt="flac", rate=48000, channels=2, endless=True)
            ha.play_media(murl)
            secrets_seen.append(token(murl))
            await new_file(wav, n)
            await ha.wait_media_state(PLAYING)
            mark = len(ha.media_states)
            with open(os.path.join(tv, "power"), "w") as f:
                f.write("Active Standby\n")
            ok = await until_true(lambda: ha.media_state.state == IDLE, 6)
            u = ha.play_media_like_ha(proxy, signal=fake_proxy.Signal((fake_proxy.Track(1.0),)))
            c = proxy.conversion_of(u)
            ok2 = await until_true(lambda: c.fetches == 1 and proxy.slot_count(dev) == 0, 6)
            await asyncio.sleep(0.5)
            check("SA Active Standby: the music stops; play_media is refused there and its URL only released (slot free)",
                  ok and ok2 and states_since(ha, mark) == [IDLE] and "ended: standby" in sat.text()
                  and "refused: the TV is in Active Standby" in sat.text(), f"states={states_since(ha, mark)}")
            ok = await until_true(lambda: '"standby":true' in open(voice).read(), 3)
            check("SA1 run/voice.json: standby and last_end standby, no URL", ok
                  and '"last_end":"standby"' in open(voice).read() and "http" not in open(voice).read(),
                  open(voice).read())
            with open(os.path.join(tv, "power"), "w") as f:
                f.write("Active\n")
            await sat.wait_log("play_media is accepted again")
            url = ha.play_media_like_ha(proxy, signal=fake_proxy.Signal((fake_proxy.Track(0.5),)))
            ok = await ha.wait_media_state(PLAYING)
            ok2 = await until_true(lambda: ha.media_state.state == IDLE, 6)
            check("SA2 back in Active: play_media plays again", ok and ok2)

        log = sat.text()
        leaked = [s for s in secrets_seen if s in log]
        check("S16 log policy: no proxy or flow stream id in the log (URLs are redacted)",
              len(secrets_seen) >= 8 and not leaked, str(leaked))
        sat.proc.terminate()
        rc = await asyncio.wait_for(sat.proc.wait(), 5)
        check("S17 SIGTERM with music configured: clean exit", rc == 0, f"rc={rc}")
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()
        sat.log.close()


async def scenario_music_off_and_plaintext(binary: str, tmp: str) -> None:
    print("\n== satellite e2e: music off (S10), plaintext refusal (S14), loopback refused without the test aid (S15b) ==")
    write_mic(os.path.join(tmp, "mic-music.raw"), 20.0)
    async with fake_proxy.HaProxy() as proxy:
        # S10: [music] enabled = false: flags 5124, a media URL is ignored (never fetched).
        port = free_port()
        config = write_config(tmp, "e2e-music-off", port, "02:00:00:00:00:e8",
                              device="allow_plaintext = true", music="enabled = false")
        sat = Satellite(binary, config, os.path.join(tmp, "satellite-music-off.log"))
        await sat.start()
        try:
            ha = await media_ha(port, None)
            fmts = ha.media_formats()
            url = proxy.create_url(ha.device_name, signal=fake_proxy.Signal((fake_proxy.Track(1.0),)))
            ha.play_media(url)
            ha.media_pause()
            await asyncio.sleep(1.0)
            voice = os.path.join(tmp, "run-e2e-music-off", "voice.json")
            check("S10 music off: flags 5124, one ANNOUNCEMENT format, a media URL is never fetched, voice.json says enabled false",
                  int(ha.media_flags()) == MEDIA_FLAGS_MUSIC_OFF and len(fmts) == 1
                  and proxy.conversion_of(url).fetches == 0 and ha.media_state.state == IDLE
                  and '"music":{"enabled":false}' in open(voice).read(), str(fmts))
            await ha.cli.disconnect()
        finally:
            sat.proc.terminate()
            await sat.proc.wait()
            sat.log.close()
        # S14: music on, plaintext session, allow_plaintext = false: refused, nothing fetched.
        port = free_port()
        config = write_config(tmp, "e2e-music-plain", port, "02:00:00:00:00:e9",
                              device="allow_plaintext = true", music="enabled = true")
        sat = Satellite(binary, config, os.path.join(tmp, "satellite-music-plain.log"))
        await sat.start()
        try:
            ha = await media_ha(port, None)
            url = proxy.create_url(ha.device_name, signal=fake_proxy.Signal((fake_proxy.Track(1.0),)))
            ha.play_media(url)
            ha.enqueue_media(url)
            await asyncio.sleep(1.0)
            check("S14 plaintext session: play_media and ENQUEUE refused by the API layer, never fetched, idle",
                  proxy.conversion_of(url).fetches == 0 and ha.media_state.state == IDLE
                  and "media URL refused: the API session is not encrypted" in sat.text())
            await ha.cli.disconnect()
        finally:
            sat.proc.terminate()
            await sat.proc.wait()
            sat.log.close()
        # S15b: music.allow_plaintext = true and no --test-fetch-loopback: a 127.0.0.1 URL is refused unfetched.
        port = free_port()
        config = write_config(tmp, "e2e-music-lo", port, "02:00:00:00:00:ea",
                              device="allow_plaintext = true", music="enabled = true\nallow_plaintext = true")
        sat = Satellite(binary, config, os.path.join(tmp, "satellite-music-lo.log"), extra=())
        await sat.start()
        try:
            ha = await media_ha(port, None)
            url = proxy.create_url(ha.device_name, signal=fake_proxy.Signal((fake_proxy.Track(1.0),)))
            ha.play_media(url)
            ok = await sat.wait_log("fetch target refused: loopback address")
            await asyncio.sleep(0.3)
            check("S15b a loopback URL is refused with no fetch (the default guard, no test aid)",
                  ok and proxy.conversion_of(url).fetches == 0 and not proxy.requests
                  and ha.media_state.state == IDLE)
            await ha.cli.disconnect()
        finally:
            sat.proc.terminate()
            await sat.proc.wait()
            sat.log.close()


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, help="path to the webos-satellite binary")
    args = parser.parse_args()
    failed = False
    with tempfile.TemporaryDirectory(prefix="webos-sat-music-e2e-") as tmp:
        try:
            await scenario_music(args.binary, tmp)
            await scenario_music_off_and_plaintext(args.binary, tmp)
        except Exception as err:  # noqa: BLE001
            failed = True
            print(f"FAIL  {type(err).__name__}: {err}", flush=True)
            for name in ("satellite-music.log", "satellite-music-off.log", "satellite-music-plain.log",
                         "satellite-music-lo.log"):
                log = os.path.join(tmp, name)
                if os.path.exists(log):
                    print(f"---- {name} ----")
                    print(open(log).read())
    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print(f"\n{passed} checks passed, {'FAILED' if failed else 'all OK'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
