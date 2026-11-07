# EXP-026 — Magic Remote mic key: evdev grab of `LGE M-RCU - Builtin [0]` with re-injection into a sibling node

> **Part B needs the owner's authorization for each run**, given separately from Part A, with the owner at the TV. Part B grabs the Magic Remote's input device: while it runs, the remote works only through our test binary. Nothing in Part B may run on the strength of this document alone. Part A (read-only) ran once on 2026-11-07, owner-authorized for that run only.

**Status:** Part A **run** (2026-11-07, read-only, c3-a); Part B **PLANNED, not authorized, not run**. The test binary `remote-proxy` is built and host-tested (2026-11-07, PC only; never copied to a TV).
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
  - The test binary `remote-proxy` (crate `crates/remote-proxy`, section "The test binary" below): an **input-event proxy only**, no network, no luna, no file writes; its log goes to stdout.
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

#### The test binary

- **Source:** `crates/remote-proxy` (its own crate; `unsafe` only in `src/sys.rs`, the evdev and uinput ioctls, `poll`, `signalfd`, `alarm`, `_exit`). Modes: `dry-run`, `inject-test`, `grab`, `clone-probe`, and (added during the session, 2026-11-07) `trace` (read only: every event's time, type, code and value) and `inject-key` (one press and release of an arrow key, Up 103, Left 105, Right 106 or Down 108 only, into a target; no grab). Usage: run it without arguments.
- **Build (PC):** `cargo zigbuild -j4 --release -p webos-remote-proxy --target armv7-unknown-linux-musleabi`, as the satellite's armv7 build (`.cargo/config.toml` flags). Output `target/armv7-unknown-linux-musleabi/release/remote-proxy`, never committed.
- **Build checked for the session (2026-11-07, rustc 1.99.1):** static ELF 32-bit ARM EABI5, stripped. First copy (B0, B0all, B1): 416236 bytes, SHA-256 `a239c8dafcab8c7db484ac2323b0d46950a236ff5d1a50c315947f60029a45e8`. **Current copy on c3-a** (with `trace` and `inject-key`): **421116 bytes**, SHA-256 **`57f39126655e12263f1ebf467d781ee1ab3a8b74a85cc4b8342ffffe795e9527`**. Rebuilds give the same hash. Rebuild and compare before the session; a different hash means a different binary (code or toolchain change): re-run the host tests below first.
- **What it checks before touching anything:** both nodes found by **exact name** in sysfs (exactly one match each, never an `eventN`); the source has key 428; source and target capabilities identical (`EVIOCGBIT` for the event types, keys, `REL`, `ABS`, `MSC`); `ABS` ranges identical (`EVIOCGABS`: min, max, fuzz, flat, resolution). Any difference: it logs both and exits with code 2 without grabbing. It logs the capability lists and ranges of both nodes.
- **Exit codes:** 0 normal end (cap, `q`, signal, stdin end of file), 2 refused or a write error, 3 watchdog, 4 panic, 5 source node gone, 137 the outer kill.

#### Dead-man: all four layers always on

1. **Hard cap in the binary:** `--secs` (1 to 60; defaults dry-run 30, inject-test 5, grab 60, clone-probe 20) is a deadline the loop honours (the normal release path runs), and `alarm(secs + 2)` is set at start with `SIGALRM` at its default action, so the process dies even if the loop never runs again.
2. **Loop watchdog in the binary:** the loop polls with a 200 ms timeout and stamps a heartbeat each turn; a watchdog thread checks every 250 ms and, when the heartbeat is older than 1 s, writes releases for every key still down in the target and calls `_exit(3)`. The exit closes the fds: the kernel releases the grab.
3. **Session dead-man:** the binary exits on SIGHUP, SIGINT or SIGTERM (handled through `signalfd`, so the normal release path runs), on end of file on stdin, and on a line containing `q` (the owner's or operator's "stop" key). It ignores SIGTTIN and SIGTTOU (a background read must never **stop** the process, which would also stop the watchdog), and it **refuses to start** when stdin is a terminal whose foreground group it is not in (for example under coreutils `timeout` without `--foreground`, which would keep a hangup from reaching it). Run it in the foreground of `ssh -tt`.
4. **Outer kill on the TV:** not `timeout` (see 3); the command line starts a sleeper that `kill -9`s the binary's pid `N + 10` s later if, and only if, that pid is still `remote-proxy`:
   `P=$$; (sleep <N+10>; [ "$(cat /proc/$P/comm 2>/dev/null)" = remote-proxy ] && kill -9 $P) & exec /tmp/exp026/remote-proxy ...`
   (`exec` keeps the shell's pid and foreground group; when the session ends, the sleeper gets the hangup too.)

Plus the human fallback: a second SSH shell with the kill typed in advance, and the IR remote (a separate device, `LGE RCU`, never grabbed).

**Grab hygiene (in the binary):** grab only when `EVIOCGKEY` on the source reports no key down (waits at most 2 s, else exits without grabbing); forward whole frames (up to `SYN_REPORT`) in one `write()`, every event except `EV_KEY` 428 (press, release and repeat), unchanged; frames that held only the mic key are not sent; target opened `O_WRONLY | O_NONBLOCK`; on `SYN_DROPPED`, discard up to the next report and release the keys that `EVIOCGKEY` shows up; on every exit (cap, `q`, signal, end of file, watchdog, panic), release in the target every key it holds down, then ungrab.

**Logging (stdout only):** the two nodes' capability lists and `ABS` ranges, the dead-man settings, each 428 press and release with its time since start and the hold length, and at the end the counts per event type and code, the reason for the end, and the frame delay (our write time minus the source frame's kernel time) as frame count, median and maximum in microseconds. Nothing else; no file on the TV.

#### Host tests (PC, 2026-11-07)

- `cargo test -p webos-remote-proxy`: **20 passed** (the filter drops only `EV_KEY` 428 and passes 427, 429 and `REL` code 428; frames; `SYN_DROPPED`; held keys released on exit, once; resync; the cap is 1 to 60 s and the `alarm` comes 2 s after it; the watchdog fires on a stall and not while the loop beats; ioctl numbers equal to the kernel headers' values; c3-a's key bitmap (Part A) has 428, 412, 103, 28 and LG's codes 1194 to 1199; name lookup refuses missing and duplicate names; capability and range comparison). `cargo clippy -D warnings` clean for the host and for the armv7 target.
- `python3 -I crates/remote-proxy/tests/host_uinput.py target/debug/remote-proxy`: end to end against two throwaway uinput devices with the same keys and `ABS_X`/`ABS_Y` (desktop-harmless joystick buttons `BTN_TRIGGER_HAPPY1..3`, and 428). It needs `/dev/uinput` write access, which this PC grants the user through the seat ACL (no root). **23 of 23 passed:** grab starts; the target gets every key and pointer frame except 428; a second reader of the grabbed source sees nothing; stdin end of file, the 2 s cap, SIGTERM, SIGHUP and `q` each end the run, and the keys still down in the target are released; the **watchdog** (a debug-build-only stall hook, `REMOTE_PROXY_TEST_STALL_MS`, compiled out of the release binary) ends a stalled loop in 1.37 s with code 3 and releases the held key; the grab is gone after every exit; `clone-probe` creates a copy and it disappears; `inject-test` writes the centre of the range; refusals for different capabilities, different `ABS` ranges, the same node twice, a cap above 60 s, a missing name, and a key held at the start.
- Wrappers under a pseudo-terminal: the outer-kill command line above killed the binary after 2 s of a 6 s cap (exit 137); a hangup of the terminal ended it.
- **Not tested on the host:** the armv7 binary itself end to end (under `qemu-arm` the kernel answers a 64-bit process, so the record size and sysfs widths differ from the TV's 32-bit compat path; the satellite's reader already uses the same 16-byte records on the TV); the panic hook (unit-level only: it calls the same release-and-`_exit` path as the watchdog).

#### Session commands (owner present; Part B authorized for that run)

On the PC, from the repository (raw output only to the gitignored `devices/private/c3-a/EXP-026/`). `<tv>` is the address from the gitignored local config.

```sh
# 0. build and verify, then copy (the only file Part B writes on the TV)
cargo zigbuild -j4 --release -p webos-remote-proxy --target armv7-unknown-linux-musleabi
sha256sum target/armv7-unknown-linux-musleabi/release/remote-proxy   # must equal the hash above
ssh root@<tv> 'mkdir -p /tmp/exp026'
scp target/armv7-unknown-linux-musleabi/release/remote-proxy root@<tv>:/tmp/exp026/remote-proxy
ssh root@<tv> 'chmod 755 /tmp/exp026/remote-proxy && sha256sum /tmp/exp026/remote-proxy'   # same hash

# 1. baseline (the owner has turned the mic button feature off)
ssh root@<tv> '<install>/ctl.sh status --json'      # mic_button_active false, lg_voice_key lg
ssh -tt root@<tv> 'timeout 10 luna-send -n 1 -w 5000 -f luna://com.webos.service.config/getConfigs "{\"configNames\":[\"com.webos.keyaction.voice\"]}"'   # "launch"

# 2. second shell, kept open for the whole session (the human kill)
ssh root@<tv>                                          # then, only if needed:  kill -9 $(pidof remote-proxy)
```

Every step runs in the foreground of its own `ssh -tt` (the whole remote command in single quotes, so nothing is expanded on the PC), with the outer kill 10 s after the step's cap:

```sh
# B0  dry run, 30 s (outer kill at 40 s)
ssh -tt root@<tv> 'P=$$; (sleep 40; [ "$(cat /proc/$P/comm 2>/dev/null)" = remote-proxy ] && kill -9 $P) & exec /tmp/exp026/remote-proxy dry-run --source "LGE M-RCU - Builtin [0]" --secs 30' | tee devices/private/c3-a/EXP-026/b0.txt
# B1  one pointer move into [2], 5 s (outer kill at 15 s)
ssh -tt root@<tv> 'P=$$; (sleep 15; [ "$(cat /proc/$P/comm 2>/dev/null)" = remote-proxy ] && kill -9 $P) & exec /tmp/exp026/remote-proxy inject-test --target "LGE M-RCU - Builtin [2]" --secs 5' | tee devices/private/c3-a/EXP-026/b1.txt
# B2  grab [0], re-send to [2], 60 s (outer kill at 70 s)
ssh -tt root@<tv> 'P=$$; (sleep 70; [ "$(cat /proc/$P/comm 2>/dev/null)" = remote-proxy ] && kill -9 $P) & exec /tmp/exp026/remote-proxy grab --source "LGE M-RCU - Builtin [0]" --target "LGE M-RCU - Builtin [2]" --secs 60' | tee devices/private/c3-a/EXP-026/b2.txt
# B3  kill test, 20 s (outer kill at 30 s); in the second shell after about 10 s: kill -9 $(pidof remote-proxy)
ssh -tt root@<tv> 'P=$$; (sleep 30; [ "$(cat /proc/$P/comm 2>/dev/null)" = remote-proxy ] && kill -9 $P) & exec /tmp/exp026/remote-proxy grab --source "LGE M-RCU - Builtin [0]" --target "LGE M-RCU - Builtin [2]" --secs 20' | tee devices/private/c3-a/EXP-026/b3.txt
# B4  optional clone probe, 20 s (outer kill at 30 s)
ssh -tt root@<tv> 'P=$$; (sleep 30; [ "$(cat /proc/$P/comm 2>/dev/null)" = remote-proxy ] && kill -9 $P) & exec /tmp/exp026/remote-proxy clone-probe --source "LGE M-RCU - Builtin [0]" --secs 20' | tee devices/private/c3-a/EXP-026/b4.txt
```

For `[1]` instead of `[2]` (B1 or B2 fallback), change only the target name.

To stop any step early: type `q` and Enter in its terminal, or close the terminal (hangup), or use the second shell.

| Step | Command | Owner does | Pass | If not |
| --- | --- | --- | --- | --- |
| B0 | `dry-run`, 30 s (no grab, no write) | Moves the pointer, presses OK, Back, and one silent mic tap | Capability lines show `[0]` with 428; counts match the actions; one 428 press and release; LG's assistant opens on the tap as usual (no grab yet); read delay logged | Stop: discovery or parsing is wrong |
| B1 | `inject-test`, 5 s (no grab, one frame) | Watches the screen | The cursor appears or jumps to the centre: the compositor reads injected events from `[2]` | Repeat once with `[1]` as the target; if neither, stop (C1a fails here) |
| B2 | `grab`, 60 s | Pointer, click, wheel scroll, Back to close a menu (not the app), Home, volume held (repeats), a number key, then one silent mic **tap** and one silent mic **hold** (about 2 s) | Everything as before; **no toast and no LG assistant** on both mic presses; both 428 pairs logged; frame delay median below 2000 us and max below 10000 us; end reason `cap reached` (or `q`); the filtered log of the window shows no `setInputEvent` and no `com.webos.app.toast` | Any failure: `q`, the cap or a kill ends it; note what failed. Back exiting the app: repeat B2 with `[1]` (magic_mapper's webOS 10 quirk) |
| B3 | `grab`, 20 s, then `kill -9 $(pidof remote-proxy)` in the second shell after about 10 s | Uses the pointer and keys before and after the kill, then one silent mic tap | Exit 137; the remote works at once after the kill; the tap opens LG's assistant (nothing left over) | See rollback |
| B4 (optional) | `clone-probe`, 20 s (no grab, no events) | Nothing | The log says `opened by surface-manager after <ms> ms` (C1b possible) or `NOT opened` (C1b rejected on this firmware); the clone disappears at the end | The clone disappears with the process |

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

### Part B progress (2026-11-07, c3-a, owner-run; satellite mic-button feature off, `com.webos.keyaction.voice` = `launch`)

Raw output in the gitignored `devices/private/c3-a/EXP-026/` (`b0.txt`, `b0all.txt`, `b1.txt`, `diag1.txt`). The owner runs each step with the local wrapper `run.sh` (gitignored).

| Step | Result | Label |
| --- | --- | --- |
| B0 (dry run on `[0]`, 30 s) | `[0]` delivered 126 frames (pointer `ABS` 214, wheel 1, OK as `BTN_LEFT` 272, LG code 1198 ×2; read delay median 95 µs, max 1.9 ms) but **no 428 and no Back**; LG's own log had no `KEY_VOICE` either (only `KEY_OK`), and LG's assistant did not open: the mic press never reached `lginput2` in that window (cause unknown; the remote may have been waking). Not a failure of the reader | verified-on-target |
| B0all (dry runs on `[0]`, `[1]`, `[2]` at once, 30 s) | **Everything on `[0]` only**: 428 press and release (held 2859 ms), Back 412, Home **773** (LG's code; logged by `lginput2` as `KEY_HOME`), volume-down 114 with repeats, OK 272, wheel, LG codes 1198 and 1199; `[1]` and `[2]` silent. LG's log: `KEY_VOICE`, and `voiceagent` became visible. `lginput2` writes only to `[0]`; the siblings carry nothing | verified-on-target |
| B1 (one `ABS` frame, centre, into `[2]`, then `[1]`) | The write succeeded (48 bytes: `ABS_X` 960, `ABS_Y` 540, `SYN`; ranges 0 to 1920 and 0 to 1080), but the owner saw **no pointer jump**, also with the pointer made visible first. Whether the events reach the sibling's readers, and whether the compositor takes keys from a sibling, is the next check (`b1key`) | verified-on-target (write) ; open (effect) |

**New static facts (c3-a `libim.so.1.0.0`, copied read-only, SHA-256 `7340c837…2b52`):** LG's input manager, loaded by `surface-manager` **and** `lginput2`, keeps a list of accepted device names (`LGE M-RCU - Builtin`, `LGE RCU`, `LGE Network Input`, `LGE Simple Premium`, `LGE M-RCU - For GAME`, and LG's test devices `LGE Tellurium Input Generator Keyboard/Mouse/Touch`), opens `/dev/input/event%d` itself, rescans when asked over a POSIX message queue (`IM_RequestToCheckInput`, which `lginput2` calls after creating its devices: `xpa_im_registerEvdev`), draws the cursor itself (`IM_CURSOR_*`), and has per-device filters (`_IM_INPUT_READ_GetConnectedDeviceIDs`, `_IM_INPUT_READ_CheckKeySkipped`, `_IM_INPUT_SEND_IsSendEventByDeactivateDeviceType`, `_gCurrentDeviceIndex`). Hypothesis: it takes pointer (and maybe key) events only from the device of the connected remote, which would explain B1. static (c3-a).

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

- ~~Write and host-test `remote-proxy`~~ done (2026-11-07, PC only); Part B waits for the owner's authorization.
- The `ABS` ranges of `[0]` and `[2]` are read with `EVIOCGABS` at the start of every Part B step (the binary refuses to run if they differ).
- After Part B: an ADR proposal for C1a, with ADR 0013's override kept as the safety net.
- c3-b and other firmware: repeat Part A before any Part B there.
