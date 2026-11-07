# EXP-022 — Magic Remote mic button: passive key observation and the remote's voice stream

**Status:** Part A RUN on c3-b (2026-11-07, read-only). Part B (`startStreaming`) RUN on c3-a (2026-11-07, second attempt; the first attempt saw no mic press and is recorded below as inconclusive). Part A proper on c3-a: only its key capture was repeated inside Part B. Results below
**Date:** written 2026-11-07
**Device alias / model / firmware / webOS / architecture:** c3-a first (LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel, 32-bit ARM userland); c3-b (`OLED55C36LC`, `03.30.73`) optional and only Part A. The Magic Remote model is recorded during the run
**Power and playback state:** `Active`, no playback started by the agent, LG hands-free ("Hi LG") **off** (unchanged), installed satellite running unchanged
**Related research / requirements / issue:** [remote mic button research](../research/remote-mic-button.md), [D3 step 5a](../discovery/c3-a/D3-mic-identification.md#2-step-5a-magic-remote-mic-button-2026-11-01), [D2 service surface](../discovery/c3-a/D2-audio-stack.md#lg-audio-and-voice-service-surface-luna-service2), [D5 passive input trace](../discovery/c3-b/D5-results.md), [EXP-011](EXP-011-lg-settings-toggle.md) (`voiceinput` private calls over `ssh -tt`); requirement F7 (truthful listening indicator)
**Risk:** Part A **read-only** (passive reads of event nodes, named files, process fd lists, one read-only luna getter). Part B is a **short consented voice stream**: one luna subscription to LG's `voiceinput`; audio is counted in memory and never stored
**Approval needed / obtained:** Part A on c3-b: **obtained** from the owner on 2026-11-07 ("We can test it at c3-b now"), read-only, owner at the TV. Part A on c3-a: not obtained. Part B on c3-a: **obtained** from the owner on 2026-11-07 ("we can do it now at c3-a"), with the owner speaking "uno dos tres" while holding the button (consent for LG's assistant to hear it too). Part B on c3-b: not run

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

All of this is `verified-on-target` for **c3-b** (`OLED55C36LC`, webOS TV 8.3.0, firmware `03.30.73`, satellite 0.4.5 running, power state `Active`, foreground app a third-party player, LG hands-free off, 2026-11-07), unless marked otherwise. Raw files stay in the gitignored `devices/private/c3-b/EXP-022/`. Nothing was written on the TV (the readers streamed to the PC over ssh; no `/tmp/exp022*` file existed), no process was touched, `hidraw` was never read, no grab. Part B was not called.

**Deviations from the written plan.** The coordinator changed the choreography to Up once, 5 s pause, three silent mic taps, and the owner did not keep the timing. Readers: `cat` of `event0` to `event8` (`event9` to `event12` were retried but never opened: `/dev/input/event9` to `event31` exist as stale files with no device behind them). The log was read with an awk filter on the TV (`NL_BUTTON_CLICK`, `KEY_VOICE`, `button_type`, `voiceinput`, `voiceconductor`, `hidraw`), Bluetooth addresses redacted on the TV, 202 lines kept, none of them speech. Window: readers started **2026-11-07T14:07:51.3Z**, owner was told to start at **14:07:56.3Z**, readers ended **14:09:57.0Z** (planned end 14:09:56.3Z). The PC-TV clock offset was under 1 s (event timestamps and log lines agree to 1 ms).

**Inventory (before the presses).**

| Item | Result |
| --- | --- |
| Input nodes | `event0` "CHECK INPUT", `event1` "LGE RCU", `event2` to `event4` "LGE M-RCU - Builtin [0..2]", `event5` "LGE TONE+ - Builtin [3]", `event6` "LGE Simple Premium", `event7` "Bluetooth-audio-source", `event8` "IoT keypad". **No "Smart Remote RCU Input" and no "LGE Network Input"** (D5 used `event9` and `event10` for them) |
| Remote | **`LGE MR23`** (Magic Remote 2023, from `HID_NAME` and from the log), `/dev/hidraw0` created through `uhid`. The node appears **only while the remote is connected**: absent at 14:04, present at 14:05:10Z, removed at **14:08:45Z** (about 15 s after the last press; `lginput2` logged `STAGE_REMOVED`, `voiceinput` "can not find device info"), created again at about 14:09:40Z, just before the owner's next key |
| `hidraw0` holders | `voiceinput` (pid 3367) and `lginput2` (pid 3631), nobody else. **No `voiceinput_hidraw` process** exists on webOS 8.3 (`voiceinput` itself has a `CaptureHidRaw` component) |
| uinput / event holders | `lginput2`: 5 `/dev/uinput` fds. `surface-manager`: `event0` to `event8` plus one uinput fd |
| `voiceinput/getDevices` | `{"devices":[]}` (empty although the remote is connected). `setting/getActivation`: everything off (`thinQ.settingStatus` false) |
| `startStreaming` schema (`/usr/share/voiceinput/schemas/VoiceInputService.startStreaming.schema`, read only) | properties `id` (pattern `(remote\|sound\|mobile\|internal)\d+`), `deviceType` (pattern `(remote\|sound\|mobile\|internal)`), `key` (string), `socketType` (string), `subscribe` (only `true`); one of `id` or `deviceType` required. **There is no `keyType` property.** The files `startStreamingWithTrigger` (only `internal`), `stopStreaming` and `Tas.setStream` exist too |
| Log | `/var/log/messages` (same file as `/tmp/var/log/messages`; it was rotated during the session, 1.5 MB to 0.85 MB) has no earlier `NL_BUTTON_CLICK`; the lines appear only on presses (below) |
| Satellite | `ctl.sh status --json`: running (pid unchanged), HA connected, `mic_off` false; `pcm0c` `closed` before and after the window (the satellite was not holding the mic at that moment; not investigated here) |

**Capture (evdev).** Only `event2`, **`LGE M-RCU - Builtin [0]`**, produced any data: 1684 records, 638 `SYN`, 1020 `EV_ABS` codes 0 and 1 (pointer movement) and the `EV_KEY` events below. `event0`, `event1`, `event3` to `event8` were silent. Times are UTC, 2026-11-07.

| Time | Action | `event2` `EV_KEY` (code, value) | Log line (`lginput2`) |
| --- | --- | --- | --- |
| 14:07:53.408 / .659 | mic tap 1 (owner pressed before the "ready" stamp) | **428** 1, then 428 0 (held 0.25 s) | `MRCU {LGE MR23} [KEY_VOICE] button press` / `release`, and `NL_BUTTON_CLICK {"remote_type":"LGE MR23","button_type":"KEY_VOICE"}` |
| 14:08:02.909 | Back (owner closing LG's UI; not in the plan) | 412 1 and 0 at the same instant | `NL_NON_LIST_BUTTON_CLICK … KEY_PREVIOUS` |
| 14:08:04.496 / .692 | **Up** (the control) | **103** 1, then 103 0 (0.20 s); an extra code **1199** 1 and 0 at the same instant as the press (meaning unknown) | `NL_NON_LIST_BUTTON_CLICK … KEY_UP` |
| 14:08:10.599 / .809 | mic tap 2 | **428** 1, 0 (0.21 s) | as tap 1 |
| 14:08:18.159 / .420 | mic tap 3 | **428** 1, 0 (0.26 s) | as tap 1 |
| 14:08:29.920 / .170 | mic tap 4 | **428** 1, 0 (0.25 s) | as tap 1 |
| 14:09:42 to 14:09:46 | owner's navigation after the window (Down, Up, Left x3, OK) | 108, 103, 105 x3, 28, each 1 then 0 | `KEY_DOWN`, `KEY_UP`, `KEY_LEFT` x3, `KEY_OK` as `NL_NON_LIST_BUTTON_CLICK` |

So four mic presses reached the log and `event2` (one more than the plan, and spaced about 8 s and 12 s apart, not 5 s). The mic button is an ordinary press and release pair, code 428, no repeat events, and each press was a short tap (about 0.25 s). The Up key (and Back, and the navigation keys) take the same node.

**What LG did on each mic press (log, redacted).** The same sequence ran four times:
1. `lginput2` logs the press and the `KEY_VOICE` click; `voiceconductor` receives `setInputEvent` from `com.webos.starfish.keyfilters` (`key_down`, "start checking long press", a 500 ms timer; `detectShortPress: DetectHold`). It also asks `tvpowerd` to turn the screen on (refused harmlessly: the screen is already on).
2. About 0.25 s later, at the release, `short_press_detected`. LG's `performer` then builds the voice request with **`recognitionSource {"input":"voiceinput","key":"mrcu","type":"remote","keyType":"thinQtv"}`, `deviceId "remote"`, `voiceEngine "thinQtv"`, `recognitionType "intent"`, `activateType "view"`, `subscribe true`**, and `sam` launches `com.webos.app.voice` (launch mode `knowme`, `useTtsData` true).
3. `voiceconductor` reports `VOICEUI_FOCUSED com.webos.app.voice` 0.2 s after the launch for taps 1 and 2. For taps 3 and 4 the voice app was already focused and was only relaunched with the same parameters.
4. The voice UI went away at 14:08:03.254 (0.35 s after the Back key) and at **14:09:01.614** with **no key press**, about 31 s after the last tap. In both cases focus went back to the foreground player.

**Owner observation (reported by the coordinator, not measured):** "The assistant appeared when I clicked the mic button, didn't say anything, and then disappeared." This matches the log: LG's voice UI opens on a short mic tap **with "Hi LG" off**, and closes without a recognized phrase.

**What the filtered log does not show.** No `voiceinput` line mentions starting a capture, a stream or a socket during the presses (the only `Capture`, `TaskManager` and `CaptureHidRaw` lines are the remote's disconnect at 14:08:45Z and its reconnect at about 14:09:40Z). That fits "no speech, no stream", but the filter would not catch every line, so it is not proof that no audio was captured. No transcript or speech text appeared in the 202 kept lines.

## Part B on c3-a: the remote's voice stream (2026-11-07)

Target: c3-a (LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, remote `LGE MR23`, satellite 0.4.5 holding `pcm0c`, LG hands-free off). Raw files (key events, stream counters, filtered log) are in the gitignored `devices/private/c3-a/EXP-022/`. **No audio was stored, copied or transmitted:** the counter ran on the TV (a python script piped over ssh stdin, so no file was written on the TV), keeping only byte counts and levels in memory. No `EVIOCGRAB`, no hidraw read, no LG process touched (only my own `luna-send` children were stopped). The owner's phrase was "uno dos tres".

**Attempt 1 (readers 14:50:49.6Z to 14:52:54.9Z): inconclusive.** The only key event was code 1199 (press, release) at 14:50:51.7Z; no 428, no Up, no hidraw node before, during or after, 0 filtered log lines, 0 `startStreaming` attempts. The remote had not connected to this TV in that window. This says nothing about the feature.

**Attempt 2 (readers 14:54:33.0Z to 14:59:38.8Z, owner told at 14:54:38.0Z).** The stream script keys on the 428 press, then calls `luna://com.webos.service.voiceinput/startStreaming` every 0.3 to 0.4 s while the button is down (cap 14 per hold, 42 in all; **4 used**). `hidraw0` was absent in the 14:54:33Z and 14:59:39Z snapshots and in a check at 15:00:14Z, and **present in a read-only check at 14:55:51Z** (it exists only while the remote is connected; LG's `voiceinput` and `lginput2` held it on c3-b).

Key events (`event2`, `LGE M-RCU - Builtin [0]`, UTC): Up 103 at 14:54:50.220; mic holds **428** 14:54:53.440 to 14:54:56.600 (3.16 s), 14:55:02.162 to 14:55:05.361 (3.20 s), 14:55:10.021 to 14:55:13.841 (3.82 s); then volume up (115, with auto-repeat 14:55:36 to 14:55:41, owner raising the volume) and a lone 1199 at 14:58:36.7. Code 1199 also appeared 31 ms and 51 ms after our two successful subscriptions (14:54:53.500 and 14:55:04.269) and alone twice, so it is not tied to our call; its meaning is still unknown. `NL_BUTTON_CLICK … KEY_VOICE` (`LGE MR23`) was logged for each of the three holds (and `KEY_UP`, `KEY_VOLUMEUP`).

Every `startStreaming` response (socket path unredacted: it holds no identifier):

| Time | Hold, attempt | Parameters | Response |
| --- | --- | --- | --- |
| 14:54:53.469 (29 ms after the press) | 1, n=1 | `{"deviceType":"remote","subscribe":true}` | `{"subscribed":true,"returnValue":true,"socketPath":"/tmp/voiceinput_remote1.soc"}` |
| 14:55:03.570 | 2, n=1 | same | `{"errorCode":"2003","errorText":"audio data is already capturing","returnValue":false,"subscribed":true}` |
| 14:55:03.892 | 2, n=2 | `{"id":"remote1","deviceType":"remote","subscribe":true}` | `{"returnValue":false}` |
| 14:55:04.218 | 2, n=3 | `{"deviceType":"remote","keyType":"thinQtv","subscribe":true}` | `{"subscribed":true,"returnValue":true,"socketPath":"/tmp/voiceinput_remote1.soc"}` |

The `10000` refusal seen on webOS 25 was never returned: the call was only made while the button was already down (29 ms and later), and the first one succeeded. Hold 3 had no call of its own (the script was still counting the hold-2 stream); the hold-2 stream covered it.

Stream counts (the socket is a Unix stream socket; the numbers are for the format assumed to be 16 kHz mono S16LE):

| Stream | Connected | Ended | Bytes | Rate | Whole-stream level (assumed S16LE) |
| --- | --- | --- | --- | --- | --- |
| 1 (hold 1) | 14:54:53.472 | 14:55:03.481, **my 10 s cap** | 316 160 | 31 587 B/s | rms -34.0 dBFS, peak -4.9 dBFS |
| 2 (holds 2 and 3) | 14:55:04.222 | 14:55:14.223, **my 10 s cap** | 315 840 | 31 581 B/s | rms -32.7 dBFS, peak -12.5 dBFS |

The rate is 98.7 % of 32 000 B/s (16 kHz x 2 bytes), so the format guess fits. The per-0.5 s levels (about 16 400 bytes each) follow what the owner did:
- **Stream 1:** rms -49.7 dBFS in the first 0.5 s, then -32 to -27 dBFS at 1.0 to 3.1 s (the first "uno dos tres", while the button was held until 3.16 s), a floor of about -51.5 dBFS (peak about -39.5) from 4.2 s to 8.3 s, and speech again at 8.8 to 9.9 s (-45 to -30 dBFS; this is the second hold, which began at 14:55:02.2Z, 8.7 s after the stream started). Speech over the floor: about 24 dB.
- **Stream 2:** speech at 0 to 1.0 s (hold 2 was still held), a floor of about -51 dBFS from 2.1 s to 5.7 s, a short bump at 6.2 s, and speech at 7.3 to 9.3 s (-33 to -24 dBFS, which is hold 3 at 14:55:10.0 to 14:55:13.8Z).

**What happened on release:** nothing. The stream was **not** closed by the button release: it kept delivering 32 kB/s of live remote-microphone audio for at least 6.9 s after the release (stream 1) and 8.9 s (stream 2) until my cap, and it carried the next hold's speech. The remote's microphone therefore stays live for as long as the subscription is open; a satellite run must end it itself (a close, or a timer).

**LG's UI (owner observation, reported by the coordinator; time not recorded):** after the three holds LG's assistant said "El procesamiento de audio es inestable, inténtelo de nuevo" ("audio processing is unstable, try again"). On c3-b in Part A (no subscription by us, no speech), the owner saw only "the assistant appeared, said nothing, disappeared", with no such message. **The filtered log shows nothing on c3-a for the voice UI:** unlike c3-b, where each tap wrote `voiceconductor` lines and a `com.webos.app.voice` launch, here the filter (`voiceinput`, `voiceconductor`, `KEY_VOICE`, `hidraw` and others) kept only the five `lginput2` button lines (753 bytes). Why is unknown (a different log setting, or LG's voice flow did not start); so LG's UI opening is known only from the owner's report.

**Does the message line up with our subscription? Possibly, not shown.**
- For: error 2003 "audio data is already capturing" at 14:55:03.570 means a capture was active 86 ms after I stopped my stream 1 and 1.4 s after press 2. That is either `voiceinput` still tearing down my stream, or LG's own hold-triggered capture holding the single remote stream. Either fits a contention story, and LG's own session would then get an unstable stream. Our subscriptions overlapped LG's three holds almost completely (14:54:53.5 to 14:55:14.2Z).
- Against or unknown: the error cleared within 0.65 s, and our third attempt then succeeded, so two consumers were not permanently exclusive; no LG log lines to timestamp the message; no control hold on c3-a without our subscription; c3-a and c3-b differ in unit and in the spoken phrase (c3-b had none).
- **Test that would settle it:** on c3-a, the owner holds the mic button and says the same phrase **without** our stream script running. If LG shows the message anyway, it is LG's own reaction; if not, our subscription is the cause.

## Conclusion and confidence

Hypotheses, against the evidence (c3-b, `03.30.73`, one session, four presses; confidence high for A1 and A4, medium for A2, A3 and the voice UI facts):

| Hypothesis | Verdict |
| --- | --- |
| H-A1: 428 with 1 at the press and 0 at the release, on `LGE M-RCU - Builtin [n]` | **Confirmed** for node `[0]` (`event2`), four of four presses; 428 press and release are separate events, about 0.25 s apart for a tap. Up (103) appears on the same node. **Not tested:** a held press (the owner tapped only) |
| H-A2: `voiceinput_hidraw` holds the hidraw node | **Falsified as stated:** no such process. `hidraw0` (HID name `LGE MR23`) is held by `voiceinput` and `lginput2`. Whether our reading it would starve LG was not tested (we never read it) |
| H-A3: `deviceType` accepts `remote`, a `keyType` such as `thinQtv` exists | **Partly confirmed:** `deviceType` accepts `remote`; the schema has **no `keyType`** (it has `id`, `key`, `socketType`). `thinQtv` is, however, the `keyType` and `voiceEngine` in LG's **own** launch parameters for a mic tap on this TV. Whether `startStreaming` accepts it is untested (Part B) |
| H-A4: `NL_BUTTON_CLICK … KEY_VOICE` in the system log | **Confirmed** on webOS 8.3 (`{"remote_type":"LGE MR23","button_type":"KEY_VOICE"}`), within 1 ms of the event, one line per press. It also names the remote model |
| LG's UI opens on a mic tap with hands-free off | **Confirmed** (log: `com.webos.app.voice` launched and focused each time; owner saw it appear, say nothing and disappear) |
| The mic press is also visible on hidraw | **Not tested by design** (no read of hidraw). `lginput2` logs the press at the same millisecond as the evdev event while holding the hidraw node, which suggests it reads the HID report and writes uinput (inference) |

Consequences for the design (inferences, not tested): a passive trigger on `event2` code 428 (value 1 then 0), or a filtered `NL_BUTTON_CLICK … KEY_VOICE` log line, is a working read-only signal on this unit. **LG's voice UI opens at the same time**, so a satellite run started from the button would have LG's assistant on screen too, until a coexistence decision (research note 1.3) says otherwise. The remote's hidraw node comes and goes with the Bluetooth connection (it vanished 15 s after the last press), while the uinput nodes stay.

**Residual unknowns (Part A, c3-b):** a held press (hold-to-talk); the code 1199 sent with Up; the Magic Remote node on **c3-a** (not run there); `startStreaming` behaviour (Part B); whether `voiceinput` captured audio during the presses; why `getDevices` is empty; why the voice UI closed at 14:09:01Z.

**Part B conclusion (c3-a, `03.30.74`, one session of 3 holds, 2 successful subscriptions; confidence medium-high for the stream, low for the cause of LG's message):**

| Hypothesis | Verdict |
| --- | --- |
| H-B1: with the button held, `startStreaming` replies with a `socketPath` and the socket delivers about 32 kB/s (16 kHz mono S16LE) whose level rises while the owner speaks | **Confirmed** (both successes: 31.6 kB/s, speech 24 dB above a -51.5 dBFS floor). **The call worked with `{"deviceType":"remote","subscribe":true}`, no `keyType` and no `id`**, 29 ms after the press. The `10000` refusal before the hold was not seen (not tested before the press) |
| "The stream ends at the release" | **Falsified:** it stays open and live until the subscription ends (at least 6.9 s after release) |
| `keyType thinQtv` is needed | **Not needed**; it was accepted too (attempt 2.3, after a 2003 refusal), but the plain call also worked |
| No contention with LG's own voice flow | **Not shown:** error 2003 "audio data is already capturing" once, and the owner saw LG's "audio processing is unstable" message. See the section above; a control hold is needed |

Next steps (not authorized): a control hold without our stream; the remote's stream while the satellite runs a real push-to-talk run; closing the subscription at release or after a timer.

## Rollback and cleanup

- Part A changes nothing. On the c3-b run nothing was written on the TV, so no cleanup was needed (the readers ended by their own `timeout`). Generally: stop the readers (`kill $(cat /tmp/exp022-pids)`), then `rm -f /tmp/exp022*` on the TV. LG's voice screen closes with Back or by itself.
- Part B: killing that one `luna-send` (B4) ends the subscription; the counter exits by its `timeout`. Then `rm -f /tmp/exp022*`. No setting was changed, so there is nothing to restore. If LG's voice stays stuck (B5), a reboot restores it, and needs the owner's approval.
- Private raw files (on c3-b, kept in the gitignored `devices/private/c3-b/EXP-022/` until the owner has seen the results) are deleted after review; only the redacted table goes into this document and the compatibility record.

## Follow-ups / unresolved questions

- Fill in the results here and in [c3-a](../compatibility/c3-a.md) (one row per capability), and update the labels in the [research note](../research/remote-mic-button.md).
- A coexistence ADR (LG's assistant and ours on the same key) before any grab, hook or change to an LG process.
- c3-a Part A inventory is covered (same nodes and schema as c3-b); a control hold without our stream (see Part B); Part B on c3-b to compare `03.30.73`; a coexistence ADR before any use of the remote's stream next to LG's assistant.
