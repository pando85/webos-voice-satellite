# EXP-009 — App package install, update, autostart and removal on a TV

**Status:** PLANNED
**Date:** 2026-11-03 (procedure written; not run)
**Device alias / model / firmware / webOS / architecture:** c3-b (second LG C3, aarch64 kernel). Firmware, webOS and HBC version are read in step 0. **Not c3-a.**
**Power and playback state:** screen on, idle, no other satellite install running (the D5 `/tmp` install must be gone).
**Related research / requirements / issue:** [packaging design](../design/packaging.md), [install UX §7 items 1-3, 7, 10](../design/install-and-config-ux.md#7-what-must-be-verified-on-a-tv-before-acceptance), [autostart design](../design/autostart.md).
**Risk:** persistent installation (an app, `/var/lib/webos-satellite`, later one init.d hook) and two reboots.
**Approval needed / obtained:** owner approved installing the package on c3-b once the D5 run there is finished (2026-11-03); each reboot is announced first. Microphone capture follows the firmware guard: on a firmware outside `validated_firmware` the microphone stays off, which is fine for this experiment.

## Question and falsifiable hypothesis

Does the ipk from `make ipk` install with appinstalld, run its app, install and start the satellite through HBC `exec`, update in place without losing config or key and without two daemons, start at boot through the opt-in hook without tripping HBC failsafe, and remove cleanly?

Falsified by any of: appinstalld rejects the package; HBC `exec` from the app is not root; the daemon dies when the app or HBC's service exits; an update leaves two daemons, loses `etc/`, or starts nothing; the hook trips failsafe or starts nothing; anything of ours remains after purge and app removal.

## Prerequisites and safe baseline

- `make ipk` on the PC (records the ipk sha256); `make test-packaging` green.
- SSH as root to c3-b; `TV=root@<c3-b>` in a gitignored `local.mk` or on the command line. Raw output goes to `devices/private/c3-b/` only.
- LG hands-free mic setting as on c3-b's D5 baseline; no audible playback is needed.
- The app path below assumes HBC installs to `/media/developer/apps`; step 1 confirms it (`APP`).

## Commands / steps (copyable and bounded)

All `ssh` calls run under `timeout`. `CTL` means `sh $APP/payload/ctl.sh` with `APP=/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite`.

0. **Read-only baseline:** `cat /var/run/nyx/os_info.json | grep -e webos_manufacturing_version -e webos_release`, `uname -m`, `id -u`, `grep '"version"' /media/developer/apps/usr/palm/applications/org.webosbrew.hbchannel/appinfo.json`, `ls -la /var/lib/webosbrew/init.d`, `ls -d /var/lib/webos-satellite` (expect absent), `grep ' /var ' /proc/mounts`, `df /var`, `ps | grep -c '[w]ebos-satellite'` (expect 0), `command -v luna-send-pub setsid timeout od flock`, `ls /var/luna/preferences/webosbrew_failsafe` (expect absent).
1. **Install the package:** `make tv-install-ipk TV=root@<c3-b>` (copies to `/tmp`, `dev/install`, waits for `installed`, deletes the copy, prints `ctl.sh status`). Then `ls -la $APP $APP/payload` (modes; whether the exec bit survived is recorded, not required) and `sha256sum $APP/payload/*` against `SHA256SUMS`.
2. **App and root:** owner opens "Voice Satellite" from the launcher. Expected: "Not set up yet", the TV's address, firmware line. If it says HBC is not root, stop and record.
3. **Set up from the app:** owner presses Set up. Expected: status "Running", `CTL status` shows the chosen build (aarch64 on c3-b), one pid; `/var/lib/webos-satellite/etc/config.toml` mode 0600; `log/webos-satellite.log` shows the firmware guard line and the pairing window. Optionally pair a **new test entry** in HA by host and port (never the permanent c3-a entry), then remove it.
4. **Survival:** owner closes the app; wait 3 minutes (HBC's service has an idle timeout); `CTL status` still running with the same pid.
5. **Update with a running daemon:** `make tv-install-ipk TV=... VERSION=0.0.2` (same binaries, new version and sums). Expected right after: the old pid still runs, `CTL status` reports "update pending". Record `sha256sum` of `etc/config.toml` and of any key file before. Owner opens the app: it applies the update. Expected: new pid, `VERSION` 0.0.2, exactly one `webos-satellite` and one `supervise.sh` process, same hashes.
6. **Start at boot:** owner presses "Start at boot: turn on". Check `/var/lib/webosbrew/init.d/webos-satellite` (mode 0755, `PREFIX=/var/lib/webos-satellite`). Install `VERSION=0.0.3` the same way **without** opening the app. Owner-announced reboot. After boot: no failsafe toast, `webosbrew_failsafe` absent, `log/autostart.log` shows the 60 s delay, "package update pending (0.0.2 -> 0.0.3); applying", "update applied", the firmware line and "starting"; `CTL status` running 0.0.3.
7. **Rollback paths:** `CTL mic off` (log shows KILL SWITCH, PCM released), `CTL mic on`; `CTL stop`; try a downgrade: `make tv-install-ipk VERSION=0.0.1`, record whether appinstalld accepts it, then `CTL install` and check `etc/` unchanged.
8. **Removal:** app "Remove service (keep settings)": hook gone, no process, `bin/` gone, `etc/` and `log/` kept. Then "Remove service and all data": `/var/lib/webos-satellite` gone. Then `make tv-remove-ipk TV=...`: app directory gone. Final: `ls /var/lib/webosbrew/init.d`, `ls -d /var/lib/webos-satellite`, `ps`, `ls /tmp | grep -i satellite` all clean. One more owner-announced reboot: no failsafe, nothing of ours runs.
9. **App removed without uninstall** (optional, after reinstalling and turning start at boot on): `make tv-remove-ipk`, reboot; expected: the hook starts nothing (no new `autostart.log` line, no process); then `rm /var/lib/webosbrew/init.d/webos-satellite; rm -rf /var/lib/webos-satellite`.

## Expected evidence and failure interpretation

| Step | Pass | Failure means |
| --- | --- | --- |
| 1 | `installed`, files and sums match | appinstalld wants something ares-package writes that `build_ipk.py` does not (compare with an `ares-package` build of the same tree) |
| 2-3 | root status, daemon running | HBC not elevated, or `exec` blocked for this app; check `luna://org.webosbrew.hbchannel.service/getConfiguration` `root` |
| 4 | same pid | children of HBC `exec` are killed with its service: start the daemon through the hook or `spawn` instead (design change) |
| 5 | one daemon, hashes equal | locking or stop logic wrong on BusyBox; keep logs |
| 6 | no failsafe, update applied at boot | hook contract broken (failsafe), or `startup.sh` did not run hooks this boot (check `/tmp/webosbrew_startup`) |
| 8-9 | nothing left | removal paths incomplete |

## Observations and redacted evidence links

None yet.

## Conclusion and confidence

Not run.

## Rollback and cleanup

At any point: `touch /tmp/webos-satellite.disable` (microphone off), `CTL stop`, `rm -f /var/lib/webosbrew/init.d/webos-satellite` (no autostart at the next boot), `CTL purge`, `make tv-remove-ipk`. If failsafe mode appears, HBC skipped the hooks for that boot: delete the hook over SSH or telnet and reboot normally ([autostart rollback](../design/autostart.md#rollback)).

## Follow-ups / unresolved questions

- Repeat steps 1-8 on HBC 0.6.3 (c3-a's version) only with the owner's separate approval; c3-a is the owner's daily display.
- Install through HBC itself from a self-hosted repository file (needs a hosted manifest; publication decision).
