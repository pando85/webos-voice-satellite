# EXP-022 — Magic Remote mic button: passive key observation and the remote's voice stream

**Status:** PLANNED (not run; needs the owner's authorization, because TV access is currently limited to installs)
**Date:** written 2026-11-07
**Device alias / model / firmware / webOS / architecture:** c3-a first (LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel, 32-bit ARM userland); c3-b (`OLED55C36LC`, `03.30.73`) optional and only Part A. The Magic Remote model is recorded during the run
**Power and playback state:** `Active`, no playback started by the agent, LG hands-free ("Hi LG") **off** (unchanged), installed satellite running unchanged
**Related research / requirements / issue:** [remote mic button research](../research/remote-mic-button.md), [D3 step 5a](../discovery/c3-a/D3-mic-identification.md#2-step-5a-magic-remote-mic-button-2026-11-01), [D2 service surface](../discovery/c3-a/D2-audio-stack.md#lg-audio-and-voice-service-surface-luna-service2), [D5 passive input trace](../discovery/c3-b/D5-results.md), [EXP-011](EXP-011-lg-settings-toggle.md) (`voiceinput` private calls over `ssh -tt`); requirement F7 (truthful listening indicator)
**Risk:** Part A **read-only** (passive reads of event nodes, named files, process fd lists, one read-only luna getter). Part B is a **short consented voice stream**: one luna subscription to LG's `voiceinput`; audio is counted in memory and never stored
**Approval needed / obtained:** needed for Part A (a TV session); separately needed for Part B (a session plus consent for one spoken neutral phrase that LG's assistant may also send to LG). Not obtained

## Question and falsifiable hypothesis

1. **A (key):** on webOS 8.3, does the Magic Remote's mic button appear on an evdev node a root process can read without grabbing? Which node, which code, and are press and release separate events?
   - H-A1: an `EV_KEY` event with code **428** (`0x01ac`, LG's `KEY_VOICE`) with value 1 at the press and 0 at the release, on a node named `LGE M-RCU - Builtin [n]` (seen on webOS 10.3 by LG Input Mapper and CleanYourTV) or `Smart Remote RCU Input`.
   - Falsified by no 428 event on any node during the three presses, while a control key (Up, 103) on the same remote does appear.
2. **A (LG's voice route):** does a `voiceinput_hidraw` process exist on webOS 8.3, and who holds the `/dev/hidraw*` node of the remote?
   - H-A2: `voiceinput_hidraw` (or `voiceinput`) holds it, so LG's voice route reads the remote directly and bypasses evdev (as on a webOS 10.3.1 TV, own-your-glass F37).
3. **A (stream parameters):** what does the shipped schema of `voiceinput/startStreaming` allow (`deviceType`, `keyType`, `subscribe`)?
   - H-A3: `deviceType` accepts `remote` and a `keyType` value such as `thinQtv` exists (the values CleanYourTV uses on webOS 25).
4. **A (log signal):** does a press write an `NL_BUTTON_CLICK … "button_type":"KEY_VOICE"` line to the system log on webOS 8.3?
   - H-A4: yes, as on webOS 10.3.1.
5. **B (stream):** with the button held, does `voiceinput/startStreaming` with remote parameters reply with a `socketPath`, and does the socket deliver about 32 000 bytes per second (16 kHz mono S16LE) whose level rises while the owner speaks?
   - H-B1: yes; before the button is held the call is refused with `errorCode` `10000` (as on webOS 25).
   - Falsified by a refusal while held, or by no data or another byte rate on the socket.

## Prerequisites and safe baseline

- **Authorization:** the owner authorizes this session (Part A) and, separately, Part B. The owner is at the TV with the Magic Remote. The IR remote is at hand too, although nothing here grabs input.
- SSH as root (address only in the gitignored local config). `luna-send` needs a pty on this firmware: run scripts with `ssh -tt`, copying each script to `/tmp` first ([EXP-011](EXP-011-lg-settings-toggle.md)). Wrap every TV command in `timeout`.
- Raw output goes only to the gitignored `devices/private/<tv>/EXP-022/`. Delete the TV-side `/tmp/exp022*` files at the end.
- **Never:**
  - `EVIOCGRAB`, a write to any `/dev/input/event*`, `/dev/uinput` or `/dev/hidraw*`, a read of `/dev/hidraw*` (it carries the remote's audio);
  - reading `dapm_widget` or anything under `/sys/kernel/debug` ([D2 incident](../discovery/c3-a/D2-audio-stack.md#incident-during-this-step-read-first));
  - walking `/sys` or `/` with `find`;
  - copying or committing the system log;
  - stopping, signalling or restarting any LG process;
  - any setting change.
- **Identifiers:** `/proc/bus/input/devices` (`U: Uniq=`, `P: Phys=`) and `/sys/class/hidraw/*/device/uevent` (`HID_UNIQ`, `HID_PHYS`) contain Bluetooth addresses. Keep raw copies private. Only node names, handlers and key bitmaps go into the results.
- **Privacy (Part A):** pressing the mic button starts LG's assistant, which may send the audio to LG's cloud. The owner **does not speak** during the presses, and nobody else in the room speaks during them. A webOS 10.3.1 TV wrote voice transcripts to its system log (own-your-glass F5), so the log is read only through a `grep` for `NL_BUTTON_CLICK`, and nothing else from it is kept.
- Baseline to record (private file and the results table): power state, foreground app, `voiceinput/setting/getActivation` (expect everything off), the satellite's `ctl.sh status --json` (mic open, HA connected), and the status of `/proc/asound/card1/pcm0c/sub0/status` (a named procfs file).

## Commands / steps (copyable and bounded)

`L` is `luna-send -n 1 -w 5000 -f` (inside `ssh -tt`).

### Part A0: read-only inventory (no presses)

```sh
# Input devices: keep raw private (Uniq/Phys are identifiers); note N:, H: and B: KEY= lines only.
timeout 5 cat /proc/bus/input/devices > /tmp/exp022-devices.txt
# hidraw nodes and their HID names (named files only; HID_NAME only, never HID_UNIQ/HID_PHYS)
ls -l /dev/hidraw* 2>/dev/null
for u in /sys/class/hidraw/hidraw*/device/uevent; do echo "$u: $(grep '^HID_NAME=' "$u")"; done
# LG input and voice processes, and which of them hold input, uinput or hidraw nodes
for n in lginput2 surface-manager voiceinput voiceinput_hidraw voiceconductor; do echo "$n: $(pidof $n)"; done
for p in $(pidof lginput2 surface-manager voiceinput voiceinput_hidraw voiceconductor); do
  echo "== $p $(cat /proc/$p/comm)"; ls -l /proc/$p/fd 2>/dev/null | grep -E 'hidraw|/dev/input|uinput'
done
# Any other holder of a hidraw node (pid and comm only)
for d in /proc/[0-9]*; do ls -l $d/fd 2>/dev/null | grep -q hidraw && echo "${d#/proc/} $(cat $d/comm 2>/dev/null)"; done
# voiceinput: device list (read-only getter, called before in D2) and the startStreaming schema (named files)
$L luna://com.webos.service.voiceinput/getDevices '{}'
ls /usr/share/voiceinput/schemas/ | grep -i stream
#   then cat each file listed by the line above, e.g. the one whose name contains "startStreaming"
# System log: existence and size only (do not read it here)
ls -l /tmp/var/log/messages /var/log/messages 2>/dev/null
```

### Part A1: passive trace during presses

1. From `/tmp/exp022-devices.txt`, pick the `eventN` handlers of every node named `LGE M-RCU - Builtin [0]`, `[1]`, `[2]`, `Smart Remote RCU Input`, `LGE RCU`, `LGE Simple Premium`, `LGE TONE+` and `IoT keypad`. Node numbers are not universal; never assume them.
2. Start the readers. Use raw `cat`, not `od`, which buffered and recorded nothing in D5. Save their pids:

```sh
for n in <the eventN list>; do timeout 240 cat /dev/input/$n > /tmp/exp022-$n.bin & echo $! >> /tmp/exp022-pids; done
LOG=<the messages path that exists, or skip>
timeout 240 tail -n0 -F "$LOG" | grep NL_BUTTON_CLICK > /tmp/exp022-click.txt & echo $! >> /tmp/exp022-pids
date +%s   # T0, also stamp each action below on the PC
```

3. Owner actions, each announced and stamped, about 15 s apart:
   - (a) the **Up** arrow once (control: proves the trace sees this remote);
   - (b) the **mic** button, a short tap (under 0.3 s), **silent**;
   - (c) and (d) the **mic** button **held about 2 s**, **silent**, twice;
   - (e) **Back** to close LG's voice screen if it is still open.

   Note what appears on screen after each press (LG's voice UI: yes or no, earcon heard: yes or no).
4. After the last action plus 20 s, stop the readers (`kill $(cat /tmp/exp022-pids)`; every reader also ends by its own `timeout` at the latest). Copy the `.bin` files and `exp022-click.txt` to the private folder. Read the satellite status and the PCM status file again.
5. On the PC, decode the 16-byte records (32-bit userland: `u32 sec, u32 usec, u16 type, u16 code, s32 value`):

```python
import struct, sys
for f in sys.argv[1:]:
    b = open(f, "rb").read()
    for i in range(0, len(b) - 15, 16):
        s, us, t, c, v = struct.unpack("<IIHHi", b[i:i + 16])
        if t == 1:  # EV_KEY
            print(f, f"{s + us / 1e6:.3f}", c, v)
```

### Part B: the remote's voice stream (separate authorization and consent; only after Part A)

- **Consent:** the owner speaks one neutral phrase ("one two three four five") while holding the button; nobody else speaks. LG's assistant will very likely also hear and process it (it is not paused).
- **Parameters:** only from the schema read in A0. Start with `{"deviceType":"remote","keyType":"thinQtv","subscribe":true}` if the schema allows these values. At most two variants, each a value the schema names. Never guess beyond the schema.

```sh
# B1: the subscription; replies go to a file (a pipe keeps luna-send printing on this firmware)
timeout 25 luna-send -i luna://com.webos.service.voiceinput/startStreaming '<params>' | cat > /tmp/exp022-b-reply.txt &
# B2: within 8 s the owner holds the mic button and says the phrase, holding about 4 s, then releases
# B3: counter: reads the socket named in the reply, prints bytes and level per 0.5 s, never writes audio
timeout 20 python3 - <<'EOF'
import json, math, re, socket, struct, time
path, t0 = None, time.time()
while path is None and time.time() - t0 < 10:
    for line in open("/tmp/exp022-b-reply.txt"):
        m = re.search(r'"socketPath"\s*:\s*"([^"]+)"', line)
        if m: path = m.group(1)
    time.sleep(0.2)
print("socketPath", path)
if path:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    for _ in range(20):
        try: s.connect(path); break
        except OSError: time.sleep(0.3)
    s.settimeout(1.0); end = time.time() + 8; win = b""; t = time.time()
    while time.time() < end:
        try: d = s.recv(4096)
        except socket.timeout: d = b""
        if d == b"" and time.time() - t > 1.5: break
        win += d
        if time.time() - t >= 0.5:
            n = len(win) // 2
            rms = math.sqrt(sum(x * x for x in struct.unpack("<%dh" % n, win[:2 * n])) / n) if n else 0
            print("bytes", len(win), "dBFS", round(20 * math.log10(rms / 32768), 1) if rms else None)
            win, t = b"", time.time()
EOF
# B4: end: stop only this luna-send (the daemon may run other luna-send children), which ends the subscription;
#     keep only the reply lines (no audio exists anywhere). Its timeout ends it at the latest.
for p in $(pidof luna-send); do grep -q startStreaming /proc/$p/cmdline 2>/dev/null && kill $p; done
$L luna://com.webos.service.voiceinput/getDevices '{}'
```

- B5: the owner presses the mic button once more, **silent**, to check that LG's own voice still opens normally. Close it with Back.

## Expected evidence and failure interpretation

| Signal | Supports | Against / interpretation |
| --- | --- | --- |
| Up (103) seen on node X; 428 with values 1 then 0 on X, with the release about 2 s after the press in (c) and (d) | H-A1. The node and code for a passive trigger and for hold-to-talk | 103 seen but no 428 anywhere: the mic button does not reach evdev on webOS 8.3. Use the log line or a luna event instead (REA, [research section 7](../research/remote-mic-button.md#7-what-rea-analysis-would-add-not-done)). Nothing seen at all: the trace failed (wrong nodes, buffering), so the result is INCONCLUSIVE, not "no event" |
| 428 press only, no release, or value 2 repeats | A1 partly | Hold-to-talk needs another release signal (LG's stream ending, or a timer) |
| `voiceinput_hidraw` holds `/dev/hidraw*` (HID name of the Magic Remote) | H-A2: LG's voice route bypasses evdev, so a grab would not stop it | No `voiceinput_hidraw`: note which process holds it (`voiceinput` itself, `lginput2`, or none) |
| Schema lists `remote` and a `keyType` enum | H-A3; Part B can use schema values | No schema file: Part B uses CleanYourTV's values once, and a refusal other than `10000` ends Part B |
| `NL_BUTTON_CLICK` with `KEY_VOICE` and a remote type | H-A4: a second read-only signal (and the remote model) | No line, or no log file: the log is not a usable signal on webOS 8.3 |
| Satellite: mic still open, HA connected, `pcm0c` still held by the satellite during and after the presses | The remote route does not touch `hw:LGSIC,0` (agrees with D3 5a) | `MIC BUSY` or a PCM change: LG's remote voice takes the far-field mic on this state; record it and stop |
| B: `returnValue:false` with `10000` before the hold, then a reply with `socketPath`; about 16 000 bytes per 0.5 s; a level rise of 20 dB or more during the phrase; the stream ends at the release | H-B1: the remote's mic is usable for a satellite run | Refused while held: wrong parameters for webOS 8.3 (only schema values; stop after two). Data at another rate: another format; note the rate, do not guess. A socket but no data: INCONCLUSIVE |
| B5: LG's voice opens normally | No side effect | LG's voice does not respond: wait 60 s, press Back, try once more; if still stuck, report and ask the owner (a reboot restores it; only with approval) |

## Observations and redacted evidence links

Not run.

## Conclusion and confidence

Not run.

## Rollback and cleanup

- Part A changes nothing. Stop the readers (`kill $(cat /tmp/exp022-pids)`), then `rm -f /tmp/exp022*` on the TV. LG's voice screen closes with Back or by itself.
- Part B: killing that one `luna-send` (B4) ends the subscription; the counter exits by its `timeout`. Then `rm -f /tmp/exp022*`. No setting was changed, so there is nothing to restore. If LG's voice stays stuck (B5), a reboot restores it, and needs the owner's approval.
- Private raw files are deleted after the analysis; only the redacted table goes into this document and the compatibility record.

## Follow-ups / unresolved questions

- Fill in the results here and in [c3-a](../compatibility/c3-a.md) (one row per capability), and update the labels in the [research note](../research/remote-mic-button.md).
- A coexistence ADR (LG's assistant and ours on the same key) before any grab, hook or change to an LG process.
- c3-b Part A, once c3-a is done, to compare `03.30.73`.
