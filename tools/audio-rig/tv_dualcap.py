#!/usr/bin/env python3
"""Linked capture of the TV mic and a reference PCM, streamed to stdout.

Runs ON THE TV (Python 3.8, stdlib only) through ctypes over the TV's own
libasound. It opens the mic PCM and, optionally, a reference PCM (for example
LGSIC SpeakerFeedback), tries `snd_pcm_link` so both start on the same
trigger, and streams both to stdout in a framed format that `tv_dualcap_demux.py`
splits on the PC. Nothing is written on the TV; copy it to /tmp, run it, delete it.

Frames: b"TC" + stream id (1 byte: 0 mic, 1 ref, 2 JSON meta) + uint32 LE
length + payload. Meta records: "open" (params, link result, trigger
timestamps), "ts" (about once per second per stream: frames read so far, the
`snd_pcm_htimestamp` avail and timestamp, for clock-drift estimates), "xrun"
and "end".

Device safety (c3-a): only ALSA PCM calls on the named devices. It never
touches sysfs, debugfs or mixer controls. Run it only while LG hands-free is
off and nothing else holds the PCMs.

Example (from the PC):
  ssh root@tv 'python3 /tmp/tv_dualcap.py --seconds 30' > cap.tc
  python3 tools/audio-rig/tv_dualcap_demux.py cap.tc --out-dir DIR
"""
import argparse
import ctypes
import json
import struct
import sys
import time

FORMATS = {"S16_LE": (2, 2), "S24_LE": (6, 4), "S32_LE": (10, 4)}  # alsa enum, bytes/sample
STREAM_CAPTURE = 1
NONBLOCK = 1
ACCESS_RW_INTERLEAVED = 3
EAGAIN = 11
EPIPE = 32
ESTRPIPE = 86


class Timespec(ctypes.Structure):
    _fields_ = [("tv_sec", ctypes.c_long), ("tv_nsec", ctypes.c_long)]


def load_alsa():
    lib = ctypes.CDLL("libasound.so.2")
    lib.snd_strerror.restype = ctypes.c_char_p
    lib.snd_pcm_readi.restype = ctypes.c_long
    lib.snd_pcm_readi.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong]
    for name in ("snd_pcm_open",):
        getattr(lib, name).argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_char_p, ctypes.c_int, ctypes.c_int]
    return lib


class Pcm:
    def __init__(self, lib, name, fmt, channels, rate, period, buffer):
        self.lib, self.name = lib, name
        self.fmt, self.channels, self.rate = fmt, channels, rate
        self.bps = FORMATS[fmt][1]
        self.frame_bytes = self.bps * channels
        self.handle = ctypes.c_void_p()
        self.frames = 0
        self.xruns = 0
        rc = lib.snd_pcm_open(ctypes.byref(self.handle), name.encode(), STREAM_CAPTURE, NONBLOCK)
        self.check(rc, "open")
        hw = ctypes.c_void_p()
        self.check(lib.snd_pcm_hw_params_malloc(ctypes.byref(hw)), "hw_params_malloc")
        try:
            h = self.handle
            self.check(lib.snd_pcm_hw_params_any(h, hw), "hw_params_any")
            self.check(lib.snd_pcm_hw_params_set_access(h, hw, ACCESS_RW_INTERLEAVED), "set_access")
            self.check(lib.snd_pcm_hw_params_set_format(h, hw, FORMATS[fmt][0]), "set_format " + fmt)
            self.check(lib.snd_pcm_hw_params_set_channels(h, hw, ctypes.c_uint(channels)), "set_channels")
            self.check(lib.snd_pcm_hw_params_set_rate(h, hw, ctypes.c_uint(rate), 0), "set_rate")
            self.check(lib.snd_pcm_hw_params_set_period_size(h, hw, ctypes.c_ulong(period), 0), "set_period_size")
            self.check(lib.snd_pcm_hw_params_set_buffer_size(h, hw, ctypes.c_ulong(buffer)), "set_buffer_size")
            self.check(lib.snd_pcm_hw_params(h, hw), "hw_params")
        finally:
            lib.snd_pcm_hw_params_free(hw)
        sw = ctypes.c_void_p()
        self.check(lib.snd_pcm_sw_params_malloc(ctypes.byref(sw)), "sw_params_malloc")
        try:
            self.check(lib.snd_pcm_sw_params_current(self.handle, sw), "sw_params_current")
            # timestamps for snd_pcm_htimestamp: ENABLE (1), CLOCK_MONOTONIC (1)
            self.check(lib.snd_pcm_sw_params_set_tstamp_mode(self.handle, sw, 1), "set_tstamp_mode")
            self.tstamp_type_rc = lib.snd_pcm_sw_params_set_tstamp_type(self.handle, sw, 1)
            self.check(lib.snd_pcm_sw_params(self.handle, sw), "sw_params")
        finally:
            lib.snd_pcm_sw_params_free(sw)
        self.check(lib.snd_pcm_prepare(self.handle), "prepare")
        self.chunk = buffer
        self.buf = ctypes.create_string_buffer(self.chunk * self.frame_bytes)

    def check(self, rc, what):
        if rc < 0:
            raise RuntimeError("%s %s: %s" % (self.name, what, self.lib.snd_strerror(rc).decode()))

    def read(self):
        n = self.lib.snd_pcm_readi(self.handle, self.buf, self.chunk)
        if n == -EAGAIN:
            return b""
        if n in (-EPIPE, -ESTRPIPE):
            self.xruns += 1
            self.lib.snd_pcm_prepare(self.handle)
            self.lib.snd_pcm_start(self.handle)
            return None
        if n < 0:
            self.check(n, "readi")
        self.frames += n
        return self.buf.raw[: n * self.frame_bytes]

    def htimestamp(self):
        avail = ctypes.c_ulong()
        ts = Timespec()
        rc = self.lib.snd_pcm_htimestamp(self.handle, ctypes.byref(avail), ctypes.byref(ts))
        return rc, avail.value, ts.tv_sec + ts.tv_nsec * 1e-9

    def trigger_tstamp(self):
        st = ctypes.c_void_p()
        self.lib.snd_pcm_status_malloc(ctypes.byref(st))
        try:
            self.lib.snd_pcm_status(self.handle, st)
            ts = Timespec()
            self.lib.snd_pcm_status_get_trigger_htstamp(st, ctypes.byref(ts))
            return ts.tv_sec + ts.tv_nsec * 1e-9
        finally:
            self.lib.snd_pcm_status_free(st)

    def close(self):
        if self.handle:
            self.lib.snd_pcm_close(self.handle)
            self.handle = ctypes.c_void_p()


def emit(out, sid, payload):
    out.write(b"TC" + struct.pack("<BI", sid, len(payload)) + payload)


def meta(out, **kw):
    emit(out, 2, json.dumps(kw).encode())


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--seconds", type=float, required=True, help="capture length (mic clock), max 600")
    ap.add_argument("--mic", default="hw:LGSIC,0")
    ap.add_argument("--mic-format", default="S24_LE", choices=sorted(FORMATS))
    ap.add_argument("--mic-channels", type=int, default=2)
    ap.add_argument("--mic-rate", type=int, default=16000)
    ap.add_argument("--ref", default="hw:LGSIC,1", help="reference PCM; 'none' for mic only")
    ap.add_argument("--ref-format", default="S32_LE", choices=sorted(FORMATS))
    ap.add_argument("--ref-channels", type=int, default=8)
    ap.add_argument("--ref-rate", type=int, default=48000)
    ap.add_argument("--period", type=int, default=256)
    ap.add_argument("--buffer", type=int, default=8192)
    ap.add_argument("--no-link", action="store_true", help="start the PCMs separately")
    ap.add_argument("--no-mic", action="store_true", help="reference only")
    a = ap.parse_args()
    if not 0 < a.seconds <= 600:
        ap.error("--seconds must be in (0, 600]")
    out = sys.stdout.buffer
    lib = load_alsa()
    pcms = []
    if not a.no_mic:
        pcms.append((0, Pcm(lib, a.mic, a.mic_format, a.mic_channels, a.mic_rate, a.period, a.buffer)))
    if a.ref != "none":
        pcms.append((1, Pcm(lib, a.ref, a.ref_format, a.ref_channels, a.ref_rate, a.period, a.buffer)))
    if not pcms:
        ap.error("nothing to capture")
    link_rc = None
    try:
        if len(pcms) == 2 and not a.no_link:
            link_rc = lib.snd_pcm_link(pcms[0][1].handle, pcms[1][1].handle)
        t0 = time.monotonic()
        if link_rc == 0:
            pcms[0][1].check(lib.snd_pcm_start(pcms[0][1].handle), "start (linked)")
        else:
            for _, p in pcms:
                p.check(lib.snd_pcm_start(p.handle), "start")
        meta(out, kind="open", t_start_monotonic=t0, link_rc=link_rc,
             link_error=(lib.snd_strerror(link_rc).decode() if link_rc not in (None, 0) else None),
             pcms=[dict(sid=sid, name=p.name, format=p.fmt, channels=p.channels, rate=p.rate,
                        period=a.period, buffer=a.buffer, trigger_tstamp=p.trigger_tstamp(),
                        tstamp_type_rc=p.tstamp_type_rc)
                   for sid, p in pcms])
        _, lead = pcms[0]
        target = int(a.seconds * lead.rate)
        next_ts = 0.0
        while lead.frames < target:
            got = False
            for sid, p in pcms:
                data = p.read()
                if data is None:
                    meta(out, kind="xrun", sid=sid, frames=p.frames, t=time.monotonic())
                    continue
                if data:
                    emit(out, sid, data)
                    got = True
            now = time.monotonic()
            if now >= next_ts:
                for sid, p in pcms:
                    rc, avail, ts = p.htimestamp()
                    meta(out, kind="ts", sid=sid, frames=p.frames, avail=avail, htstamp=ts, rc=rc, t=now)
                next_ts = now + 1.0
            if not got:
                time.sleep(0.005)
        meta(out, kind="end", t=time.monotonic(),
             pcms=[dict(sid=sid, frames=p.frames, xruns=p.xruns) for sid, p in pcms])
        out.flush()
    finally:
        if link_rc == 0:
            lib.snd_pcm_unlink(pcms[0][1].handle)
        for _, p in pcms:
            p.close()


if __name__ == "__main__":
    main()
