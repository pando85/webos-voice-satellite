# EXP-026 — Magic Remote mic key: evdev grab of `LGE M-RCU - Builtin [0]` with re-injection into a sibling node

> **Part B needs the owner's authorization for each run**, given separately from Part A, with the owner at the TV. Part B grabs the Magic Remote's input device: while it runs, the remote works only through our test binary. Nothing in Part B may run on the strength of this document alone. Part A (read-only) ran once on 2026-11-07, owner-authorized for that run only.

**Status:** Part A **run** (2026-11-07, read-only, c3-a); Part B **PLANNED, not authorized, not run**. The test binary of Part B does not exist yet.
**Date:** written and Part A run 2026-11-07
**Device alias / model / firmware / webOS / architecture:** c3-a (LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, `Rockhopper release 8.3.0-1508`, aarch64 kernel `5.4.96-307`, 32-bit ARM userland). c3-b is not part of this experiment.
**Power and playback state:** Part A: `Active`, uptime about 14 h; LG hands-free off (unchanged); the satellite running with the mic button feature **on** (its passive reader held `event2`), left alone. Part B: `Active`, the satellite's mic button feature **off** (see prerequisites).
**Related research / requirements / issue:** [mic-button key interception](../research/mic-button-key-interception.md) (candidate C1a and section 8), [remote mic button research](../research/remote-mic-button.md), [EXP-022](EXP-022-remote-mic-button-observe.md), [EXP-023](EXP-023-remote-mic-button-handover.md), [EXP-025](EXP-025-mic-button-toast.md), [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md)
**Risk:** Part A **read-only** (sysfs, `/proc`, udev data, unit files, one filtered log `grep`; nothing written on the TV, scripts streamed over `ssh` stdin). Part B **invasive for the Magic Remote**: `EVIOCGRAB` of one input device and writes of input events into a sibling device, for at most 60 s per step; no configd, LG service, file or setting change.
**Approval needed / obtained:** Part A: obtained for one run (owner, through the coordinator, 2026-11-07). Part B: **needed per run, not obtained.**

## Question and falsifiable hypothesis

Can the satellite take the mic key (`EV_KEY` 428) away from LG's compositor at the Linux input layer, by grabbing the remote's event node and re-sending every other event into a sibling node the compositor already reads, so that a press shows **no LG toast and no LG assistant**, while the pointer, wheel and all other keys keep working?

- **H1 (Part A):** a sibling of `LGE M-RCU - Builtin [0]` exists whose capabilities contain all of `[0]`'s (so it can carry the pointer, the wheel and every key), and the compositor holds it open. Falsified by a missing sibling or a smaller bitmap.
- **H2 (Part A):** nothing but the compositor and our passive reader has `event2` open, so nothing else holds or could hold a grab, and `lginput2` and `voiceinput` do not read it. Falsified by another holder.
- **H3 (Part A, open):** how the compositor learns about input devices (udev and uevents, LG's input manager, a fixed list), which decides whether a uinput clone (C1b) could work.
- **H4 (Part B):** with `[0]` grabbed and re-sent into `[2]`, the remote behaves as before, and a mic press opens neither LG's assistant nor the toast with `com.webos.keyaction.voice` at LG's own `launch`. Falsified by any of them appearing, by a broken pointer or key, or by a perceptible delay.
- **H5 (Part B):** killing the grabbing process gives the remote back at once, with nothing left over.

## Prerequisites and safe baseline

- **Authorization:** Part A done. Part B: the owner's yes for that run, the owner at the TV with the Magic Remote, **the IR remote at hand**, and SSH open from the PC.
- **Access:** SSH as root (address only in the gitignored local config). Every TV command wrapped in `timeout`. Raw output only in the gitignored `devices/private/c3-a/EXP-026/`. `/proc/bus/input/devices` holds `Uniq` and `Phys` lines (identifiers): never copied into a document.
- **Never (both parts):** ASoC `dapm_widget` or debugfs (they caused a kernel oops on this TV); the configd `voiceframework` category; any configd write; a read of `/dev/hidraw*`; signals to any LG process; a grab or a write to any input node other than the ones named in Part B.
- **Part B only:**
  - The test binary `remote-proxy` built from this repository (to be written; static, armv7 musl like the satellite), reviewed and host-tested before the session: an **input-event proxy only**, no network, no luna, no file writes except its log on stdout.
  - The satellite's mic button feature **off** before the session (HA switch "Remote mic button" or the TV app, by the owner), so `com.webos.keyaction.voice` reads `launch` and the satellite holds no reader on `event2`. Check: `ctl.sh status --json` shows `mic_button_active` false and `lg_voice_key` `lg`. The owner turns it back on afterwards if wanted.
  - Copy the binary to `/tmp/exp026/` (Part B's only file write on the TV); remove it at the end.

## Commands / steps (copyable and bounded)

### Part A: read-only (run 2026-11-07)

Two scripts, streamed with `ssh root@<tv> 'sh -s' < script > devices/private/c3-a/EXP-026/a.txt` (and `a2.txt`), so nothing was written on the TV. Every command under `timeout 5`.

| Step | What was read |
| --- | --- |
| A0 | `date`, `uname -r`, `/etc/starfish-release` |
| A1 | `/proc/bus/input/devices`, `/proc/bus/input/handlers` |
| A2 | per `eventN`: `device/name`, `readlink -f device`, `device/id/*`, `device/properties`, `device/capabilities/*`, `dev`; `ls -l /dev/input /dev/uinput` |
| A3 | every process's `/proc/<pid>/fd` filtered for `/dev/input/`, `/dev/uinput`, `/dev/hidraw`; `lginput2`, `surface-manager`, `micomservice`, `tvservice` `maps` filtered for `hook`, `inject`, `preload`, and their `LD_PRELOAD`; `/var/lib/webosbrew/init.d` listing |
| A4 | `pidof systemd-udevd udevd`; `/run/udev/data/c13:64` to `c13:72` |
| A5 | `/proc/net/netlink` rows with protocol 15 (`NETLINK_KOBJECT_UEVENT`), owners mapped by pid; `surface-manager`'s socket inodes matched against all netlink rows |
| A6 | `surface-manager` `maps` (library names filtered), environment variable names and values for `QT_*`, `WEBOS_*` matching input or platform |
| A7 | `systemctl list-units` filtered; `systemctl cat surface-manager lginput2` |
| A8 | process start times (`/proc/<pid>/stat` field 22) against uptime; filtered `grep` of `/var/log/messages` |

### Part B: short guarded trial (needs per-run authorization; not run)

**Time limits and dead-man kill, all four always on:**

1. **Hard cap in the binary:** `alarm(N)` once at start (`N` = the step's limit, at most 60 s); `SIGALRM` keeps its default action (the process dies), no handler.
2. **Loop watchdog in the binary:** the forward loop polls with a 200 ms timeout and stamps a heartbeat each turn; a second thread checks it every 250 ms and calls `_exit(3)` if it is older than 1 s. Any exit closes the fds, so the kernel releases the grab.
3. **Session dead-man:** the binary exits when its stdin reaches end of file or on `SIGHUP`, so a dropped SSH session ends it. It runs in the foreground of `ssh -tt`.
4. **Outer kill:** `timeout -s KILL $((N+10))` around the binary on the TV.

Plus the human fallback: a second SSH shell with `kill -9 $(pidof remote-proxy)` typed in advance, and the IR remote (a separate device, `LGE RCU`, not grabbed).

**Grab hygiene in the binary:** grab only when `EVIOCGKEY` on `[0]` reports no key down (wait at most 2 s, else exit without grabbing); forward whole frames (up to `SYN_REPORT`) in one `write()`; target opened `O_WRONLY | O_NONBLOCK`; on exit, forward a release for any key still down before closing; on `SYN_DROPPED`, resync with `EVIOCGKEY`; refuse to run unless the target's sysfs capabilities contain all of `[0]`'s and both nodes are found **by name** (never by index).

**Logging:** counts per event type and code, the 428 press and release times, and the per-frame delay (time of our write minus the kernel time of the source frame) as min, median and max. No other content.

```sh
# on the PC
scp remote-proxy root@<tv>:/tmp/exp026/remote-proxy        # the only file written on the TV
ssh root@<tv> 'ctl.sh status --json'                         # mic_button_active false, lg_voice_key lg (path per install)
ssh root@<tv> 'timeout 10 luna-send -n 1 -w 5000 -f luna://com.webos.service.config/getConfigs "{\"configNames\":[\"com.webos.keyaction.voice\"]}"'   # expect "launch" (ssh -tt)
# each step below in the foreground of: ssh -tt root@<tv> 'timeout -s KILL <N+10> /tmp/exp026/remote-proxy <args>'
```

| Step | Args (limit) | Owner does | Pass | If not |
| --- | --- | --- | --- | --- |
| B0 | `--dry-run --source "LGE M-RCU - Builtin [0]"` (30 s): read-only open, no grab, no write | Moves the pointer, presses OK, Back, and one silent mic tap | Counts match; the mic tap opens LG's assistant as usual (no grab yet) | Stop: parsing or discovery is wrong |
| B1 | `--inject-test --target "LGE M-RCU - Builtin [2]"` (5 s): no grab, writes **one** frame `ABS_X`, `ABS_Y` = screen centre, `SYN_REPORT` | Watches the cursor | The cursor appears or jumps to the centre: the compositor reads injected events from `[2]` | Repeat once with `[1]`; if neither, stop (C1a fails here) |
| B2 | `--grab --source "...[0]" --target "...[2]"` (60 s) | Pointer, click, wheel scroll, Back to close a menu (not the app), Home, volume held (repeats), a number key, then one silent mic **tap** and one silent mic **hold** (2 s) | Everything as before; **no toast and no LG assistant** on both mic presses; the binary logs both 428 press and release pairs; frame delay median below 2 ms and max below 10 ms; filtered log of the window shows no `setInputEvent` and no `com.webos.app.toast` | Any failure: the cap or a kill ends it; note what failed. Back exiting the app: repeat B2 with `[1]` as the target (magic_mapper's webOS 10 quirk) |
| B3 | B2's args (20 s), then `kill -9` from the second shell after about 10 s | Uses the pointer and keys after the kill, then one silent mic tap | Remote works at once after the kill; the tap opens LG's assistant (nothing left over) | See rollback |
| B4 (optional, only if the owner wants C1b checked) | `--clone-test` (20 s): creates a uinput copy of `[0]` (same name, id, bits, ranges), no grab, writes nothing | Nothing | `ls -l /proc/$(pidof surface-manager)/fd` lists the new node within 2 s | The clone disappears with the process; C1b stays rejected |

Filtered log after B2 and B3 (counting lines only, cut, bounded):

```sh
ssh root@<tv> 'timeout 10 grep -a -e NL_BUTTON_CLICK -e setInputEvent -e com.webos.app.toast -e com.webos.app.voice /var/log/messages | cut -c1-200 | tail -20'
```

## Expected evidence and failure interpretation

| Signal | Supports | Against / interpretation |
| --- | --- | --- |
| A1/A2: `[1]` and `[2]` with the same `EV`, `KEY`, `REL`, `ABS` as `[0]` | H1 | A smaller bitmap: that sibling cannot carry everything; C1a needs another target or C1b |
| A3: `event2` held only by `surface-manager` and the satellite | H2 | Another holder (for example a hook app): find it before any grab |
| A4 to A8: compositor's discovery path | H3 | — (informative) |
| B1 cursor moves | Injection into a sibling reaches the compositor | No movement on `[2]` and `[1]`: C1a fails on this firmware |
| B2 as described | H4 | LG's assistant opens: a second trigger path (not through the key filter) on this firmware. Toast appears: the key reached the compositor (grab not effective). Lost pointer or keys: capability or frame handling bug. Back exits the app: use `[1]` |
| B3 remote back at once | H5 | A stuck key or dead remote after the kill: see rollback |

## Observations and redacted evidence links

### Part A results (2026-11-07, c3-a)

Raw output in the gitignored `devices/private/c3-a/EXP-026/a.txt` and `a2.txt`. Labels: **verified-on-target (c3-a)** for every row unless noted.

| Item | Result |
| --- | --- |
| Input nodes | `event0` "CHECK INPUT", `event1` "LGE RCU", `event2` to `event4` "LGE M-RCU - Builtin [0..2]", `event5` "LGE TONE+ - Builtin [3]", `event6` "LGE Simple Premium", `event7` "Bluetooth-audio-source", `event8` "IoT keypad". All are virtual (`/sys/devices/virtual/input/inputN`), so all are uinput devices. Same layout as c3-b in EXP-022 A |
| **Sibling capabilities** | `[0]`, `[1]`, `[2]`, TONE+ `[3]` and "Simple Premium" have **identical** capabilities: `EV=f` (SYN, KEY, REL, ABS), `REL=100` (`REL_WHEEL`), `ABS=3` (`ABS_X`, `ABS_Y`), no `MSC`, `properties=0`, and the same 39-word `KEY` bitmap (it includes 428 and LG's private codes 1194 to 1199, so LG's kernel has a key range beyond the standard 767). id: bus 3 (USB), vendor 0, product 0, version 4. **`[1]` and `[2]` can carry the pointer, the wheel and every key of `[0]`.** This matches the static reading of `lginput2` (research note section 2) and the magic_mapper dumps from webOS 8.2 |
| `ABS` ranges | Not read (needs `EVIOCGABS`, an ioctl on the node, left out of a read-only run); `lginput2` creates all three with the same ranges (static) |
| Handlers | `kbd`, `joydev` (`js0` to `js4` on the five LGE pointer devices) and `evdev`; nobody holds a `js` node |
| **Holders of `event2`** | Only `surface-manager` (which holds `event0` to `event8` and one `/dev/uinput`) and `webos-satellite` (our passive reader; the mic button feature was on). `lginput2` holds 5 `/dev/uinput` fds and no event node; `micomservice`, `btaudiosrc` and `iot-client` hold one `/dev/uinput` each; **`voiceinput` holds no input node.** No process held a `hidraw` node at that moment (the remote was idle; EXP-022 saw `hidraw0` appear only with the Bluetooth link) |
| Grabs | evdev exposes no grab state read-only. A grab needs an open fd on the node, and only the two holders above have one; our reader opens without a grab (code), and it receives the presses (0.5.0 works, owner-reported), so `surface-manager` does not grab `event2` either (inference) |
| Hooks | The Homebrew app `org.webosbrew.inputhook` is **installed**, but nothing is injected: no hook library in the `maps` of `lginput2`, `surface-manager` or `micomservice`, no `LD_PRELOAD` in `lginput2`'s environment, and `/var/lib/webosbrew/init.d` holds only `webos-satellite`. `surface-manager` runs with LG's own `LD_PRELOAD=:/usr/lib/libsystrim.so.3` |
| udev | `systemd-udevd` runs (`systemd-udev-trigger` is masked). The LGE pointer nodes are tagged `ID_INPUT=1`, `ID_INPUT_MOUSE=1`, `ID_INPUT_KEY=1`; "LGE RCU", "Bluetooth-audio-source" and "IoT keypad" also `ID_INPUT_KEYBOARD=1`; "CHECK INPUT" has no udev record |
| **How the compositor enumerates devices** | `surface-manager` loads LG's input manager **`/usr/lib/libim.so.1.0.0`** with `libqstarfishegl.so` and `libstarfishcompositorplugin.so` (`QT_QPA_PLATFORM=wayland`, `WEBOS_COMPOSITOR_PLUGIN=starfishcompositorplugin`); no libudev, libinput or Qt evdev plugin in its maps. **It has no netlink socket at all** (none of its socket inodes is in `/proc/net/netlink`), so it does not listen to uevents itself. The uevent listeners are `systemd`, `lginput2`, `voiceinput`, `bthidmanager`, `physical-device-manager`, `WebAppMgr` and two unmapped port ids |
| **Devices created after the compositor started** | `surface-manager` started 1 s after boot; `lginput2` at 11 s (udev initialised `event2` at 11.87 s and `event4` at 11.88 s); `iot-client` at 22 s (`event8` at 22.34 s). `surface-manager` holds all of them, so **it does pick up uinput devices created after it started**, at least during boot. How (a registration through `libim`, as `lginput2`'s `xpa_im_registerEvdev` suggests, or a scan) is unknown, and so is whether a device created hours later is picked up |
| Units | `lginput2.service`: `Restart=on-failure`, `OOMScoreAdjust=-1000`, `EnvironmentFile=-/var/systemd/system/env/lginput2.env` (the path the Mahta project uses for `LD_PRELOAD`; no `LD_PRELOAD` active). `surface-manager.service` is a oneshot starting `surface-manager-daemon.service`; systemd warned that the unit file changed on disk since it was loaded (LG's, not ours; recorded only) |
| Log | `/var/log/messages` was 369 bytes (just rotated): no device registration lines to read |

## Conclusion and confidence

- **Part A supports C1a (H1, H2): high confidence.** Two siblings (`[1]`, `[2]`) with exactly `[0]`'s capabilities exist and are read by the compositor; nothing but the compositor and our passive reader opens `event2`; `lginput2` writes through uinput and `voiceinput` reads no input node. No blocker for Part B was found.
- **H3, partly answered:** the compositor does not use udev or uevents itself; it uses LG's `libim` and does pick up devices created after its start (during boot). This weakens the community claim that new devices are ignored, without proving runtime hot-plug: C1b stays a fallback, testable in B4.
- **Not answered (needs Part B):** whether events written into `[2]` reach the compositor (B1), whether the grab alone stops LG's assistant and the toast with `launch` (B2), the delay, and the kill behaviour (B3).

## Rollback and cleanup

- **Part A:** nothing written on the TV, nothing to undo.
- **Part B:**
  - Normal end: the binary exits by its cap; the kernel releases the grab. Check that the pointer and a key work.
  - Hang: the watchdog thread, the `alarm`, the SSH dead-man and `timeout -s KILL` each end it; by hand `kill -9 $(pidof remote-proxy)`.
  - Stuck key in the compositor after the end (a key held during the kill): press and release that key on the remote once.
  - Remote still not working after the process is gone: wake it (press any key), then take the batteries out for 10 s; use the IR remote meanwhile. Last resort: a TV restart (owner approval). Nothing persists: a grab and the injected events live only with the process.
  - Cleanup: `rm -rf /tmp/exp026`; the owner turns the satellite's mic button feature back on if wanted; check `ctl.sh status --json`.

## Follow-ups / unresolved questions

- Write and host-test `remote-proxy` (research note section 3.5), then ask for Part B.
- Read the `ABS` ranges of `[0]` and `[2]` with `EVIOCGABS` at the start of Part B (the binary refuses to run if they differ).
- After Part B: an ADR proposal for C1a, with ADR 0013's override kept as the safety net.
- c3-b and other firmware: repeat Part A before any Part B there.
