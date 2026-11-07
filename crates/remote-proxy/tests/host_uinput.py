#!/usr/bin/env python3
"""End-to-end host check of remote-proxy with two throwaway uinput devices.

Needs write access to /dev/uinput and read access to /dev/input/event* (on a
desktop: the seat ACL and the `input` group). Not run by `cargo test`. Usage:

    python3 -I crates/remote-proxy/tests/host_uinput.py target/debug/remote-proxy

Only harmless codes are used: BTN_TRIGGER_HAPPY1..3 (0x2c0..0x2c2, joystick
buttons a desktop ignores) and 428 (the mic key, which must never reach the
target). No pointer, no wheel, no keyboard keys, so the desktop is not driven.
"""
import fcntl
import glob
import os
import queue
import select
import signal
import struct
import subprocess
import sys
import threading
import time

UI_SET_EVBIT = 0x40045564
UI_SET_KEYBIT = 0x40045565
UI_SET_ABSBIT = 0x40045567
UI_DEV_CREATE = 0x5501
UI_DEV_DESTROY = 0x5502
EV_SYN, EV_KEY, EV_ABS = 0, 1, 3
MIC = 428
B1, B2, B3 = 0x2C0, 0x2C1, 0x2C2
EVENT = struct.Struct("llHHi")  # native struct input_event (64-bit host: 24 bytes)

failures = []


def check(cond, what):
    print(("ok   " if cond else "FAIL ") + what)
    if not cond:
        failures.append(what)


def make_dev(name, keys, absmax=(1920, 1080)):
    """A key device with ABS_X/ABS_Y (0..absmax), like the remote's pointer; with
    only joystick buttons and absolute axes a desktop treats it as a joystick."""
    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    for k in keys:
        fcntl.ioctl(fd, UI_SET_KEYBIT, k)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_ABS)
    fcntl.ioctl(fd, UI_SET_ABSBIT, 0)
    fcntl.ioctl(fd, UI_SET_ABSBIT, 1)
    amax = [0] * 64
    amax[0], amax[1] = absmax
    dev = (name.encode().ljust(80, b"\0") + struct.pack("HHHHi", 3, 0, 0, 4, 0)
           + struct.pack("64i", *amax) + b"\0" * (3 * 64 * 4))
    os.write(fd, dev)
    fcntl.ioctl(fd, UI_DEV_CREATE)
    return fd


def node_of(name, timeout=3.0):
    end = time.time() + timeout
    while time.time() < end:
        for p in glob.glob("/sys/class/input/event*/device/name"):
            try:
                if open(p).read().strip() == name:
                    return "/dev/input/" + p.split("/")[4]
            except OSError:
                pass
        time.sleep(0.05)
    raise SystemExit(f"no node for {name}")


def emit(fd, code, value):
    os.write(fd, EVENT.pack(0, 0, EV_KEY, code, value) + EVENT.pack(0, 0, EV_SYN, 0, 0))
    time.sleep(0.05)


def drain_abs(fd, wait=0.3):
    out = []
    end = time.time() + wait
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            data = os.read(fd, EVENT.size * 64)
            for i in range(0, len(data), EVENT.size):
                _, _, t, c, v = EVENT.unpack(data[i : i + EVENT.size])
                if t == EV_ABS:
                    out.append((c, v))
    return out


def drain(fd, wait=0.3):
    out = []
    end = time.time() + wait
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            data = os.read(fd, EVENT.size * 64)
            for i in range(0, len(data), EVENT.size):
                _, _, t, c, v = EVENT.unpack(data[i : i + EVENT.size])
                if t == EV_KEY:
                    out.append((c, v))
    return out


class Proc:
    """The binary with its output collected by a thread (no select on a buffered pipe)."""

    def __init__(self, binary, args, env=None):
        self.p = subprocess.Popen([binary] + args, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True, bufsize=1)
        self.lines = queue.Queue()
        self.all = []
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            self.lines.put(line.rstrip())

    def wait_line(self, needle, timeout=5.0):
        end = time.time() + timeout
        while time.time() < end:
            try:
                line = self.lines.get(timeout=0.1)
            except queue.Empty:
                continue
            self.all.append(line)
            if needle in line:
                return True
        return False

    def finish(self, timeout=8.0, close_stdin=True):
        if close_stdin:
            self.p.stdin.close()
        try:
            self.p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.p.kill()
            self.p.wait()
        time.sleep(0.1)
        while not self.lines.empty():
            self.all.append(self.lines.get())
        if not close_stdin:
            self.p.stdin.close()
        return self.p.returncode, "\n".join(self.all)


def main():
    binary = os.path.abspath(sys.argv[1])
    keys = [MIC, B1, B2, B3]
    src = make_dev("rp-test-src", keys)
    tgt = make_dev("rp-test-tgt", keys)
    odd = make_dev("rp-test-odd", [B1])
    wide = make_dev("rp-test-wide", keys, absmax=(3840, 2160))
    try:
        src_node, tgt_node = node_of("rp-test-src"), node_of("rp-test-tgt")
        time.sleep(0.3)
        tgt_r = os.open(tgt_node, os.O_RDONLY | os.O_NONBLOCK)
        src_r = os.open(src_node, os.O_RDONLY | os.O_NONBLOCK)

        # 1. grab: everything but 428 passes; the grab hides the source; stdin EOF ends it
        #    and releases the keys still down in the target.
        p = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-tgt", "--secs", "10"])
        check(p.wait_line("grab:"), "grab started")
        drain(src_r, 0.1)
        drain(tgt_r, 0.1)
        emit(src, B1, 1)
        emit(src, MIC, 1)
        emit(src, MIC, 0)
        emit(src, B2, 1)
        emit(src, B3, 1)
        emit(src, B3, 0)
        got = drain(tgt_r)
        check(got == [(B1, 1), (B2, 1), (B3, 1), (B3, 0)], f"target got everything but 428: {got}")
        check(drain(src_r, 0.1) == [], "a second reader of the grabbed source sees nothing")
        os.write(src, EVENT.pack(0, 0, EV_ABS, 0, 100) + EVENT.pack(0, 0, EV_ABS, 1, 200) + EVENT.pack(0, 0, EV_SYN, 0, 0))
        got = drain_abs(tgt_r)
        check(got == [(0, 100), (1, 200)], f"pointer (ABS) frames pass through the grab: {got}")
        rc, out = p.finish()
        rel = drain(tgt_r)
        check(rc == 0 and "end: stdin end of file" in out, f"stdin EOF ends it (rc {rc})")
        check(sorted(rel) == [(B1, 0), (B2, 0)], f"keys still down released in the target on exit: {rel}")
        check("mic 428 press" in out and "mic 428 release" in out, "mic press and release logged")
        check("count KEY 428: 2" in out, "428 counted, not forwarded")
        emit(src, B1, 0)  # the source's own held keys, released after the run
        emit(src, B2, 0)
        check(drain(src_r) == [(B1, 0), (B2, 0)], "grab released after exit (source readable again)")
        drain(tgt_r, 0.1)

        # 2. the cap ends the run by itself
        p = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-tgt", "--secs", "2"])
        t0 = time.time()
        rc, out = p.finish(6, close_stdin=False)
        dt = time.time() - t0
        check(rc == 0 and "end: cap reached" in out and 1.8 < dt < 3.5, f"2 s cap ends the run ({dt:.1f} s, rc {rc})")

        # 3. SIGTERM and SIGHUP end it through the normal release path
        for sig, name in ((signal.SIGTERM, "15"), (signal.SIGHUP, "1")):
            p = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-tgt", "--secs", "10"])
            p.wait_line("grab:")
            emit(src, B1, 1)
            p.p.send_signal(sig)
            rc, out = p.finish(close_stdin=False)
            rel = drain(tgt_r)
            check(rc == 0 and f"end: signal {name}" in out and (B1, 0) in rel,
                  f"signal {name} ends it and releases held keys ({rel})")
            emit(src, B1, 0)
            drain(src_r, 0.1)

        # 3b. watchdog (debug build only): the loop stalls 3 s on a mic event; the
        #     watchdog must release the held key in the target and exit (code 3)
        #     within about 1.25 s, which also releases the grab.
        env = dict(os.environ, REMOTE_PROXY_TEST_STALL_MS="3000")
        p = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-tgt", "--secs", "10"], env)
        p.wait_line("grab:")
        emit(src, B2, 1)
        t0 = time.time()
        emit(src, MIC, 1)
        rc, out = p.finish(5, close_stdin=False)
        dt = time.time() - t0
        rel = drain(tgt_r)
        check(rc == 3 and "watchdog: loop stalled" in out and dt < 2.0 and (B2, 0) in rel,
              f"watchdog ends a stalled loop in {dt:.2f} s (rc {rc}) and releases held keys ({rel})")
        emit(src, MIC, 0)
        emit(src, B2, 0)
        check((B2, 0) in drain(src_r), "grab released after the watchdog exit")

        # 4. a q line on stdin ends it
        p = Proc(binary, ["dry-run", "--source", "rp-test-src", "--secs", "10"])
        p.wait_line("dry-run:")
        p.p.stdin.write("q\n")
        p.p.stdin.flush()
        rc, out = p.finish(close_stdin=False)
        check(rc == 0 and "end: q on stdin" in out, "q on stdin ends a dry run")

        # 4b. clone probe: a uinput copy of the source appears and is destroyed again
        p = Proc(binary, ["clone-probe", "--source", "rp-test-src", "--secs", "2"])
        rc, out = p.finish(5, close_stdin=False)
        check(rc == 0 and "clone node: event" in out and "end: cap reached" in out, "clone probe creates a copy")
        time.sleep(0.3)
        names = [open(n).read().strip() for n in glob.glob("/sys/class/input/event*/device/name")]
        check(names.count("rp-test-src") == 1, "the clone is gone after the probe")

        # 4c. inject-test writes one pointer move (screen centre) into the target
        rc, out = Proc(binary, ["inject-test", "--target", "rp-test-tgt"]).finish(5, close_stdin=False)
        got = drain_abs(tgt_r)
        check(rc == 0 and got == [(0, 960), (1, 540)], f"inject-test moves the target to the centre: {got}")

        # 5. refusals
        rc, out = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-wide", "--secs", "3"]).finish(close_stdin=False)
        check(rc == 2 and "absolute ranges differ" in out, "different abs ranges refused")
        # 5b. refusals: different capabilities, same node, cap over 60 s
        rc, out = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-odd", "--secs", "3"]).finish(close_stdin=False)
        check(rc == 2 and "capabilities differ" in out, "different capabilities refused")
        rc, out = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-src"]).finish()
        check(rc == 2, "same source and target refused")
        rc, out = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-tgt", "--secs", "61"]).finish()
        check(rc == 2, "cap over 60 s refused")
        rc, out = Proc(binary, ["grab", "--source", "no-such-device", "--target", "rp-test-tgt"]).finish()
        check(rc == 2 and "no input node" in out, "missing device refused")

        # 6. no grab while a key is down on the source
        emit(src, B2, 1)
        drain(src_r, 0.1)
        rc, out = Proc(binary, ["grab", "--source", "rp-test-src", "--target", "rp-test-tgt", "--secs", "5"]).finish(close_stdin=False)
        check(rc == 2 and "not grabbing" in out, "refuses to grab while a key is down")
        emit(src, B2, 0)
        os.close(tgt_r)
        os.close(src_r)
    finally:
        for fd in (src, tgt, odd, wide):
            try:
                fcntl.ioctl(fd, UI_DEV_DESTROY)
            except OSError:
                pass
            os.close(fd)
    print(f"{len(failures)} failure(s)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
