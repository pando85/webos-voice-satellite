#!/usr/bin/env python3
"""Lifecycle checks against a running satellite, from HA's side.

Uses HA's own ESPHome client library (aioesphomeapi, the venv of
tools/esphome-fake-ha) and plays only the parts of HA's ESPHome integration
needed here; it never runs a pipeline and discards any mic audio it gets.

Subcommands:
  mute-cycle N     toggle the "Mute microphone" switch N times (hold S s per
                   state); prints the epoch-ms time of each command and of the
                   device's switch-state confirmation
  watch SECS       print every mute switch state the device reports (for the
                   kill-switch -> HA report latency)
  reconnect N      connect, subscribe to voice and states, wait for the wake
                   word start request, disconnect; N times back to back

Usage:
  tools/esphome-fake-ha/.venv/bin/python -I tools/lifecycle/lifecycle_client.py \
      --device HOST:6053 --psk-file FILE mute-cycle 10 [--hold 3]

Prints no audio, key or transcript. Output is a JSON object per line.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
import time

from aioesphomeapi import APIClient, SwitchInfo, SwitchState


def now_ms() -> int:
    return int(time.time() * 1000)


def out(**kv) -> None:
    print(json.dumps(kv), flush=True)


class Dev:
    def __init__(self, host: str, port: int, psk: str) -> None:
        self.host, self.port, self.psk = host, port, psk
        self.mute: asyncio.Queue = asyncio.Queue()
        self.starts: asyncio.Queue = asyncio.Queue()
        self.chunks = 0

    async def connect(self, timeout: float = 10.0) -> None:
        self.cli = APIClient(self.host, self.port, "", client_info="webos-lifecycle-check", noise_psk=self.psk)
        await asyncio.wait_for(self.cli.connect(login=True), timeout)
        _, entities, _ = await self.cli.device_info_and_list_entities()
        keys = [e.key for e in entities if isinstance(e, SwitchInfo) and e.object_id == "mute_microphone"]
        if not keys:
            raise RuntimeError("no mute_microphone switch")
        self.mute_key = keys[0]

        def on_state(s) -> None:
            if isinstance(s, SwitchState) and s.key == self.mute_key:
                self.mute.put_nowait((now_ms(), s.state))

        self.cli.subscribe_states(on_state)

    def subscribe_voice(self) -> None:
        async def handle_start(conversation_id, flags, audio_settings, wake_word_phrase):
            self.starts.put_nowait((now_ms(), flags))
            return 0

        async def handle_stop(abort: bool) -> None:
            pass

        async def handle_audio(data: bytes, data2: bytes | None) -> None:
            self.chunks += 1  # discarded

        self.cli.subscribe_voice_assistant(
            handle_start=handle_start, handle_stop=handle_stop, handle_audio=handle_audio
        )

    async def next_mute(self, want: bool, timeout: float = 5.0) -> int | None:
        end = time.monotonic() + timeout
        while (left := end - time.monotonic()) > 0:
            try:
                t, state = await asyncio.wait_for(self.mute.get(), left)
            except TimeoutError:
                return None
            if state == want:
                return t
        return None


async def mute_cycle(dev: Dev, n: int, hold: float) -> int:
    await dev.connect()
    dev.subscribe_voice()
    t, state = await asyncio.wait_for(dev.mute.get(), 5)
    out(event="initial", muted=state)
    if state:
        dev.cli.switch_command(dev.mute_key, False)
        await dev.next_mute(False)
    await asyncio.sleep(hold)
    bad = 0
    for i in range(1, n + 1):
        for want in (True, False):
            sent = now_ms()
            dev.cli.switch_command(dev.mute_key, want)
            ack = await dev.next_mute(want)
            out(event="mute" if want else "unmute", cycle=i, sent_ms=sent, ack_ms=ack,
                ack_after_ms=None if ack is None else ack - sent)
            bad += ack is None
            await asyncio.sleep(hold)
    await dev.cli.disconnect()
    return 1 if bad else 0


async def watch(dev: Dev, secs: float) -> int:
    await dev.connect()
    dev.subscribe_voice()
    end = time.monotonic() + secs
    while (left := end - time.monotonic()) > 0:
        try:
            t, state = await asyncio.wait_for(dev.mute.get(), left)
        except TimeoutError:
            break
        out(event="mute_state", t_ms=t, muted=state)
    await dev.cli.disconnect()
    return 0


async def reconnect(dev: Dev, n: int) -> int:
    bad = 0
    for i in range(1, n + 1):
        t0 = now_ms()
        try:
            await dev.connect()
            dev.subscribe_voice()
            t_start, flags = await asyncio.wait_for(dev.starts.get(), 10)
            out(event="reconnect", i=i, connect_to_start_ms=t_start - t0, flags=flags)
        except Exception as e:  # noqa: BLE001 - report and count every failure
            out(event="reconnect", i=i, error=type(e).__name__)
            bad += 1
        finally:
            try:
                await dev.cli.disconnect()
            except Exception:  # noqa: BLE001
                pass
            while not dev.starts.empty():
                dev.starts.get_nowait()
    out(event="summary", attempts=n, failures=bad)
    return 1 if bad else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--device", required=True, help="satellite HOST:PORT")
    ap.add_argument("--psk-file", required=True)
    sub = ap.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("mute-cycle")
    m.add_argument("n", type=int)
    m.add_argument("--hold", type=float, default=3.0)
    w = sub.add_parser("watch")
    w.add_argument("secs", type=float)
    r = sub.add_parser("reconnect")
    r.add_argument("n", type=int)
    a = ap.parse_args()
    host, _, port = a.device.rpartition(":")
    with open(a.psk_file) as f:
        psk = f.read().strip()
    dev = Dev(host, int(port), psk)
    if a.cmd == "mute-cycle":
        return asyncio.run(mute_cycle(dev, a.n, a.hold))
    if a.cmd == "watch":
        return asyncio.run(watch(dev, a.secs))
    return asyncio.run(reconnect(dev, a.n))


if __name__ == "__main__":
    sys.exit(main())
