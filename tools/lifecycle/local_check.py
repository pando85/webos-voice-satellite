#!/usr/bin/env python3
"""Local end-to-end check of the microphone gate of the real satellite binary.

Starts webos-satellite on 127.0.0.1 with a file mic and the null sink and
checks, from HA's side (aioesphomeapi):

  L1  kill switch present at startup: mute switch on, no start request
  L2  kill switch removed: switch off, the wake word arms (start request)
  L3  kill switch while streaming: audio stops, switch on, mic released
  L4  HA unmute while the kill switch exists: refused, switch back on;
      removing the kill switch turns it off again
  L5  HA mute: remembered in the mute file
  L6  restart with the mute file: starts muted, no start request
  L7  HA unmute: mute file removed, the wake word arms again
  L8  log policy: no key in the log

Usage: tools/esphome-fake-ha/.venv/bin/python -I tools/lifecycle/local_check.py --binary PATH
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import os
import socket
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lifecycle_client import Dev  # noqa: E402

FAILS: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  -- {detail}" if detail else ""), flush=True)
    if not ok:
        FAILS.append(name)


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


async def until(pred, timeout: float = 5.0) -> bool:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        await asyncio.sleep(0.02)
    return pred()


async def drain(q: asyncio.Queue) -> list:
    items = []
    while not q.empty():
        items.append(q.get_nowait())
    return items


async def last_mute(dev: Dev, settle: float = 0.5):
    await asyncio.sleep(settle)
    states = await drain(dev.mute)
    return states[-1][1] if states else None


class Sat:
    def __init__(self, binary: str, config: str, log: str) -> None:
        self.binary, self.config, self.log_path = binary, config, log

    async def start(self) -> None:
        self.log = open(self.log_path, "a")
        starts = self.text().count("ESPHome API on") if os.path.exists(self.log_path) else 0
        self.proc = await asyncio.create_subprocess_exec(
            self.binary, "--config", self.config,
            stdin=asyncio.subprocess.DEVNULL, stdout=self.log, stderr=self.log)
        assert await until(lambda: self.text().count("ESPHome API on") > starts, 10), self.text()

    def text(self) -> str:
        with open(self.log_path) as f:
            return f.read()

    async def stop(self) -> None:
        self.proc.terminate()
        await asyncio.wait_for(self.proc.wait(), 5)
        self.log.close()


async def connect(port: int, psk: str) -> Dev:
    dev = Dev("127.0.0.1", port, psk)
    await dev.connect()
    dev.subscribe_voice()
    return dev


async def scenario(binary: str, tmp: str) -> None:
    print("== lifecycle: kill switch, mute release and mute persistence ==")
    psk = base64.b64encode(os.urandom(32)).decode()
    with open(os.path.join(tmp, "psk"), "w") as f:
        f.write(psk + "\n")
    os.chmod(os.path.join(tmp, "psk"), 0o600)
    with open(os.path.join(tmp, "mic.raw"), "wb") as f:
        f.write(bytes(16000 * 8 * 10))  # 10 s of S24_LE stereo zeros
    kill = os.path.join(tmp, "disable")
    muted = os.path.join(tmp, "muted")
    port = free_port()
    config = os.path.join(tmp, "sat.toml")
    with open(config, "w") as f:
        f.write(f"""
mode = "wake"
[device]
name = "lifecycle-check"
bind = "127.0.0.1"
port = {port}
noise_psk_file = "psk"
[mic]
source = "file"
path = "mic.raw"
loop = true
[sink]
type = "null"
[safety]
kill_switch = "{kill}"
mute_file = "{muted}"
""")
    sat = Sat(binary, config, os.path.join(tmp, "sat.log"))

    open(kill, "w").close()
    await sat.start()
    dev = await connect(port, psk)
    try:
        st = await last_mute(dev)
        no_start = dev.starts.empty()
        check("L1 kill switch at startup: switch on, no start request", st is True and no_start,
              f"muted={st} starts={dev.starts.qsize()}")
        os.remove(kill)
        try:
            _, flags = await asyncio.wait_for(dev.starts.get(), 3)
        except TimeoutError:
            flags = None
        st = await last_mute(dev, 0.2)
        check("L2 kill switch removed: switch off, wake word armed", st is False and flags == 2,
              f"muted={st} flags={flags}")

        ok = await until(lambda: dev.chunks > 10)
        open(kill, "w").close()
        released = await until(lambda: "PCM released (kill switch)" in sat.text(), 2)
        st = await last_mute(dev, 0.5)
        before = dev.chunks
        await asyncio.sleep(0.5)
        check("L3 kill switch while streaming: audio stops, switch on, mic released",
              ok and released and st is True and dev.chunks == before,
              f"muted={st} chunks {before}->{dev.chunks}")

        dev.cli.switch_command(dev.mute_key, False)
        await asyncio.sleep(0.5)
        states = [s for _, s in await drain(dev.mute)]
        check("L4 HA unmute refused while the kill switch exists", states[-1:] == [True]
              and "unmute refused: kill switch" in sat.text(), f"states={states}")
        os.remove(kill)
        ok = await until(lambda: sat.text().count("kill switch removed") == 2)
        check("L4b kill switch removed again: switch off", ok and await last_mute(dev) is False)
        await drain(dev.starts)

        dev.cli.switch_command(dev.mute_key, True)
        ok = await until(lambda: os.path.exists(muted))
        check("L5 HA mute remembered in the mute file", ok and await last_mute(dev) is True)
        await dev.cli.disconnect()
        await sat.stop()

        await sat.start()
        dev = await connect(port, psk)
        st = await last_mute(dev)
        check("L6 restart with the mute file: starts muted, no start request",
              st is True and dev.starts.empty() and "remembered in" in sat.text(), f"muted={st}")
        dev.cli.switch_command(dev.mute_key, False)
        try:
            _, flags = await asyncio.wait_for(dev.starts.get(), 3)
        except TimeoutError:
            flags = None
        check("L7 HA unmute: mute file removed, wake word armed",
              not os.path.exists(muted) and flags == 2, f"flags={flags}")
        await dev.cli.disconnect()
        await sat.stop()
        check("L8 log policy: no key in the log", psk not in sat.text())
    finally:
        if sat.proc.returncode is None:
            sat.proc.kill()
            await sat.proc.wait()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    a = ap.parse_args()
    with tempfile.TemporaryDirectory(prefix="webos-sat-lifecycle-") as tmp:
        try:
            asyncio.run(scenario(a.binary, tmp))
        except Exception as e:  # noqa: BLE001
            check("scenario completed", False, f"{type(e).__name__}: {e}")
        if FAILS:
            print("---- satellite log ----")
            with open(os.path.join(tmp, "sat.log")) as f:
                print(f.read())
    print(f"{'all checks passed' if not FAILS else f'{len(FAILS)} check(s) failed'}")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
