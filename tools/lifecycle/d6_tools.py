#!/usr/bin/env python3
"""D6 helpers (PC side, standard library only).

  d6_tools.py tone OUT.raw [--ms 150] [--amp 0.2]
      Write a short 1 kHz tone, s16le, 44.1 kHz, stereo, with 10 ms fades. Played on the TV with
      `pacat --playback --format=s16le --rate=44100 --channels=2`. Amplitude 0.2 of full scale is
      the level of the D5 tone checks.

  d6_tools.py beeps MIC.raw BEEPS.log --mic-start-ms MS [--event NAME=MS ...] [--threshold-dbfs -60]
      MIC.raw: capture of hw:LGSIC,0 (S24_LE in 32-bit words, 2 channels, 16 kHz, channel 0 used).
      BEEPS.log: lines "EPOCH_MS beep N" written by d6-beeper.sh when each pacat was started.
      --mic-start-ms: TV epoch ms taken right before arecord was started (the first sample is up
      to about 100 ms later, which is the accuracy of the result).
      Prints, per beep, the 1 kHz band peak in dBFS over the following 500 ms and whether it is
      audible, then the first audible beep and its delay after each --event.

  d6_tools.py selftest
"""
import argparse
import array
import math
import struct
import sys

MIC_RATE = 16000


def tone(path, ms, amp):
    rate = 44100
    n = rate * ms // 1000
    fade = rate // 100
    out = array.array("h")
    for i in range(n):
        g = min(1.0, i / fade, (n - 1 - i) / fade)
        v = int(32767 * amp * g * math.sin(2 * math.pi * 1000 * i / rate))
        out.extend((v, v))
    with open(path, "wb") as f:
        f.write(out.tobytes())


def band_dbfs(samples, freq=1000.0, rate=MIC_RATE):
    """Goertzel amplitude of `freq` in `samples` (floats scaled to +-1), in dBFS."""
    w = 2 * math.pi * freq / rate
    c = 2 * math.cos(w)
    s1 = s2 = 0.0
    for x in samples:
        s0 = x + c * s1 - s2
        s2 = s1
        s1 = s0
    p = s1 * s1 + s2 * s2 - c * s1 * s2
    amp = math.sqrt(max(p, 1e-30)) * 2 / max(len(samples), 1)
    return 20 * math.log10(amp) if amp > 0 else -200.0


def load_mic(path):
    raw = open(path, "rb").read()
    n = len(raw) // 8
    a = array.array("i")
    a.frombytes(raw[: n * 8])
    # S24_LE is carried in 32-bit words with the value in the low 24 bits (sign-extended by ALSA).
    return [a[2 * i] / 2**23 for i in range(n)]


def analyse(mic, beeps, mic_start_ms, events, threshold):
    win = int(0.05 * MIC_RATE)
    rows = []
    for ms, label in beeps:
        t0 = (ms - mic_start_ms) / 1000.0
        i0 = int(t0 * MIC_RATE)
        if i0 < 0 or i0 + 1 > len(mic):
            rows.append((label, ms, t0, None, False))
            continue
        peak = -200.0
        for k in range(10):  # 500 ms in 50 ms steps
            seg = mic[i0 + k * win : i0 + (k + 1) * win]
            if len(seg) == win:
                peak = max(peak, band_dbfs(seg))
        rows.append((label, ms, t0, peak, peak > threshold))
    return rows


def beeps_cmd(args):
    mic = load_mic(args.mic)
    beeps = []
    for line in open(args.log):
        p = line.split()
        if len(p) >= 3 and p[1] == "beep":
            beeps.append((int(p[0]), p[2]))
    events = {}
    for e in args.event or []:
        k, v = e.split("=")
        events[k] = int(v)
    rows = analyse(mic, beeps, args.mic_start_ms, events, args.threshold_dbfs)
    print("beep  t_rel_s  band_peak_dBFS  audible")
    first = None
    for label, ms, t0, peak, aud in rows:
        print("%4s  %7.2f  %14s  %s" % (label, t0, "n/a" if peak is None else "%.1f" % peak, "yes" if aud else "no"))
        if aud and first is None:
            first = ms
    if first is None:
        print("no audible beep")
    else:
        print("first audible beep started at %d" % first)
        for k, v in events.items():
            print("  %s -> first audible beep: %+d ms" % (k, first - v))


def selftest():
    import os
    import tempfile

    d = tempfile.mkdtemp()
    p = os.path.join(d, "t.raw")
    tone(p, 150, 0.2)
    assert os.path.getsize(p) == 44100 * 150 // 1000 * 4
    # synthetic mic: 4 s of low noise, a 1 kHz burst at 2.0 s
    n = 4 * MIC_RATE
    a = array.array("i", [0] * (2 * n))
    for i in range(n):
        v = int(2**23 * 0.0005 * math.sin(i * 1.7))
        a[2 * i] = v
    for i in range(int(2.0 * MIC_RATE), int(2.15 * MIC_RATE)):
        a[2 * i] += int(2**23 * 0.03 * math.sin(2 * math.pi * 1000 * i / MIC_RATE))
    m = os.path.join(d, "m.raw")
    open(m, "wb").write(a.tobytes())
    rows = analyse(load_mic(m), [(1000, "1"), (3000, "2")], 0, {}, -60.0)
    # beep 1 starts at 1.0 s, its 500 ms window ends at 1.5 s: silent; beep 2 at 3.0 s: silent;
    assert not rows[0][4] and not rows[1][4], rows
    rows = analyse(load_mic(m), [(1800, "x")], 0, {}, -60.0)
    assert rows[0][4] and rows[0][3] > -40, rows
    print("selftest ok")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("tone")
    t.add_argument("out")
    t.add_argument("--ms", type=int, default=150)
    t.add_argument("--amp", type=float, default=0.2)
    b = sub.add_parser("beeps")
    b.add_argument("mic")
    b.add_argument("log")
    b.add_argument("--mic-start-ms", type=int, required=True)
    b.add_argument("--event", action="append")
    b.add_argument("--threshold-dbfs", type=float, default=-60.0)
    sub.add_parser("selftest")
    args = ap.parse_args()
    if args.cmd == "tone":
        tone(args.out, args.ms, args.amp)
    elif args.cmd == "beeps":
        beeps_cmd(args)
    else:
        selftest()


if __name__ == "__main__":
    sys.exit(main())
