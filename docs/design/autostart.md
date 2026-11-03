# Autostart design: boot hook, supervisor and rollback

- **Status:** proposal, **not installed**. Nothing has been written to `/var/lib/webosbrew/init.d` or to any persistent path on c3-a. Installing the hook is the first persistent change of the project and needs the owner's explicit approval (roadmap Phase 3, needs-owner).
- **Date:** 2026-11-02. Device in scope: c3-a (LG C3, webOS TV 8.3.0, firmware 03.30.74, rooted with the Homebrew Channel). Other models and firmware are not covered.
- **Files:** [`packaging/webos/init.d/webos-satellite`](../../packaging/webos/init.d/webos-satellite) (the draft hook), [`autostart.sh`](../../packaging/webos/autostart.sh), [`supervise.sh`](../../packaging/webos/supervise.sh), [`start.sh`](../../packaging/webos/start.sh), [`stop.sh`](../../packaging/webos/stop.sh). Host test: [`packaging/webos/tests/run.sh`](../../packaging/webos/tests/run.sh).
- **Related:** [rooted-tv-improvements item 8](../discovery/c3-a/rooted-tv-improvements.md), [D0-D1 root method details](../discovery/c3-a/D0-D1-runtime.md#root-method-details), [P3 robustness](../discovery/c3-a/P3-robustness.md), [development: install on a TV](../development.md#install-on-a-tv-non-persistent).

## How the boot hook runs (verified-on-target, read-only)

`/var/lib/webosbrew/startup.sh` on c3-a (read 2026-11-02, unchanged since the D0-D1 inventory) runs once per boot, guarded by `/tmp/webosbrew_startup` and a `flock`. Its relevant steps, in order:

1. If `/var/luna/preferences/webosbrew_failsafe` exists (a previous boot did not finish its startup): open a root telnet, show a "Failsafe mode" toast and alert, delete the flag, and **skip everything else**, including the hooks. The next boot is normal again.
2. Otherwise: `touch` the failsafe flag and `sync`, close inherited descriptors, block update hosts, start dropbear, bind-mount over telemetry spools, elevate the Homebrew Channel service.
3. `run-parts /var/lib/webosbrew/init.d` (BusyBox `run-parts` runs each executable file in turn and **waits for each to exit**).
4. `sleep 10`, then delete the failsafe flag.

So the failsafe flag is set from step 2 until 10 s after the last hook returns. Failsafe mode is triggered only if the TV **restarts or loses power inside that window** (a kernel panic, a watchdog reset, or the owner unplugging it while a hook hangs). A hook that exits with an error does not trigger it by itself; a hook that never returns keeps the window open indefinitely, and then any later power cycle trips it. `init.d` is empty on c3-a today.

## Risks reviewed

| Risk | Mitigation in the draft |
| --- | --- |
| A hook that blocks keeps the failsafe window open (any later reboot trips failsafe) and delays the other hooks | The hook only tests three files and starts `autostart.sh` in the background (`setsid ... &`), then `exit 0`. Measured on the host: returns in well under 1 s with no output |
| A hook that writes to stdout/stderr or keeps them open can stall the caller if it reads a pipe | All three standard descriptors of the background job go to `/dev/null`; the hook prints nothing |
| The satellite crashes or misbehaves inside the failsafe window | `autostart.sh` waits 60 s before doing anything (the window closes 10 s after `run-parts` returns), so the daemon never runs inside it |
| The satellite triggers a kernel problem that resets the TV | The daemon opens only the capture PCM by ioctls already validated on c3-a ([P1 capture validation](../discovery/c3-a/P1-capture-validation.md)); it never reads the ASoC/DAPM sysfs or debugfs nodes that caused the D2 oops. Even so, a reset 60 s after boot is outside the failsafe window |
| A crash loop hammers the system | `supervise.sh` restarts with exponential backoff (1, 2, 4, ... s, at most 300 s) and gives up for good after 5 exits within 600 s, writing `DIR/crashloop`. Nothing starts again, at boot or by hand with `--supervise`, until the owner removes that file. Tested on the host (3 exits with 1 s and 2 s backoff, then the marker) |
| The daemon hangs instead of crashing | The internal watchdog exits with code 70/71 when the event loop or the capture thread makes no progress for 30 s, and with 72 above the 64 MiB resident ceiling; the supervisor then counts it as a crash. Tested on c3-a and on the host with `--test-hang` |
| A firmware update changes the audio stack or LG services | `autostart.sh` runs `webos-satellite --check-firmware` and does not start unless the firmware is in `safety.validated_firmware` (exit 0). An empty allow-list also prevents the start. A daemon started by hand on an unvalidated firmware keeps its microphone disabled |
| The owner wants it off now, without a reboot | Kill switch (`touch /tmp/webos-satellite.disable`): the PCM is released in about 30 to 70 ms on c3-a and HA shows the mute switch on. Full stop: `touch DIR/disable; sh DIR/bin/stop.sh` |
| The prefix disappears (developer mode lost, `/media/developer` wiped) | The hook checks the install marker and `autostart.sh` first and does nothing if they are missing |
| A second copy at boot | `start.sh` refuses when the daemon or a supervisor already runs; HBC's `startup.sh` itself runs once per boot |
| Logs fill the storage | Logs stay in `DIR/log` with size rotation (`rotate.sh`, about 2 × 256 KiB); the autostart log gets a few lines per boot. With a persistent prefix this is flash, so the log volume is kept at `info` |

Not covered by this design: an HBC-style service instead of `init.d` (item 8 prefers it, but the HBC `autostart` trigger chain is still untraced, D0-D1 open question 8), and starting at screen-off/standby transitions (D5, [EXP-004](../experiments/EXP-004-power-lifecycle.md)). What is known about each power state, including that `init.d` hooks run once per boot and not on resume and that a Suspend freezes a running process, is in [always-on](always-on.md#3-power-states).

## Proposed install (needs the owner's approval; not done)

Prerequisites: the owner approves (1) a persistent prefix, (2) the one hook file, and (3) one test reboot; LG hands-free stays off; the TV is idle; a working SSH session is open; the owner knows the rollback below.

1. Choose the persistent prefix. Proposal: `/media/developer/webos-satellite` (exec allowed, about 4 GB free; lost together with developer mode, which then disables autostart by itself). Alternative: a directory under `/var/lib`.
2. Install with the existing script (`install.sh --prefix /media/developer/webos-satellite --persistent --config ... --psk-file ...`). The config must keep `validated_firmware = ["03.30.74"]` and a silent or approved sink. Optionally set `safety.mute_file` inside the prefix so an HA mute survives reboots.
3. Run it supervised by hand first (`start.sh --supervise`), check `status.sh`, the kill switch and `stop.sh`.
4. Edit `PREFIX=` in the hook if another prefix was chosen, then copy it: `install -m 0755 packaging/webos/init.d/webos-satellite /var/lib/webosbrew/init.d/webos-satellite` (the name must match `run-parts` rules: letters, digits, `-`, `_`, no dot).
5. One test reboot (owner's action). After boot: `cat DIR/log/autostart.log` shows the firmware line and `starting` about 60 s after boot; `status.sh` shows the supervisor and daemon; `/var/luna/preferences/webosbrew_failsafe` is absent; no failsafe toast appeared.

## Rollback

| Goal | Action | Reboot needed |
| --- | --- | --- |
| Stop listening now | `touch /tmp/webos-satellite.disable` (resume: `rm` it) | No |
| Stop everything now | `touch DIR/disable; sh DIR/bin/stop.sh` | No |
| No autostart at the next boot | `touch DIR/disable` (the hook then does nothing) or `rm /var/lib/webosbrew/init.d/webos-satellite` | No (takes effect at the next boot) |
| Remove completely | `rm /var/lib/webosbrew/init.d/webos-satellite`, then `sh DIR/bin/uninstall.sh` | No |
| Failsafe mode appeared anyway | HBC opens a root telnet and skips the hooks on that boot; remove the hook file from SSH or telnet, then reboot normally | One, by the owner |

## Open questions

- Whether the HBC `autostart` service method is what runs `startup.sh` at every boot on c3-a, or only `jumpstart.sh` (D0-D1 open question 8). The design does not depend on it, since it only uses `init.d`.
- Behaviour after resume from standby or Quick Start+: `startup.sh` does not run again on resume (script guard `/tmp/webosbrew_startup`; reports from other TVs say the same), and a running process is frozen, not killed (observed-on-other-device); both are **unverified on c3-a**, as is the behaviour of the draft hook with Quick Start+ off (every power-on is a cold boot). D5 (see [EXP-004](../experiments/EXP-004-power-lifecycle.md)) and [always-on](always-on.md).
- A luna hub restart stops everything in its cgroup, a daemon started by the hook included (hypothesis; see [always-on](always-on.md#2-what-was-observed-on-c3-a-read-only-2026-11-03)).
- Multi-day memory and descriptor behaviour of the daemon under the supervisor (Phase 3 exit criterion).
