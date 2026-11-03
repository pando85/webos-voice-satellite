# EXP-009 — App package install, update, autostart and removal on a TV

**Status:** TARGET_VERIFIED on c3-b (with three packaging bugs found and fixed during the run)
**Date:** 2026-11-03, 12:50 to 13:45 local time
**Device alias / model / firmware / webOS / architecture:** c3-b, LG `OLED55C36LC`, firmware `03.30.73`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland, Homebrew Channel **0.7.2**, Ethernet, sound out through ARC. **Not c3-a.**
**Power and playback state:** `Screen Saver` or `Active` on the home app, no stream playing, before every reboot; LG hands-free ("Hi LG") on, as the household left it (the microphone was never opened: firmware guard and the not-paired gate).
**Related research / requirements / issue:** [packaging design](../design/packaging.md), [install UX §7 items 1-3, 7, 10](../design/install-and-config-ux.md#7-what-must-be-verified-on-a-tv-before-acceptance), [autostart design](../design/autostart.md).
**Risk:** persistent installation (an app, `/var/lib/webos-satellite`, later one init.d hook) and two reboots.
**Approval needed / obtained:** obtained 2026-11-03 (owner, through the orchestrator) after the D5 run: install, reboots and settings changes on c3-b, every setting restored. The "owner" steps were done by the agent: the app was opened and closed with `applicationManager/launch` and `closeByAppId`, and its buttons were exercised by sending the same `ctl.sh` commands through the same HBC `exec` service the app uses. Microphone capture follows the firmware guard: on a firmware outside `validated_firmware` the microphone stays off, which is fine for this experiment.

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

Raw output and screenshots: `devices/private/c3-b/exp009/` (gitignored). Packages: `0.0.1` (commit 1d45515), `0.0.2` (ca97685, after fix 1), `0.0.3` and `0.0.4` (8959a39, all fixes), and a `0.0.2` rebuilt from 8959a39 for the downgrade; versions set with `make ipk VERSION=...`, same daemon build (`--version` 0.0.1).

| Step | Result | Observation |
| --- | --- | --- |
| 0 Baseline | PASS | HBC 0.7.2; `/var` ext4 rw, `nosuid`, not `noexec`, 525 MB free; no `/var/lib/webos-satellite`; no process of ours; `luna-send-pub`, `setsid` (util-linux), `timeout`, `od`, `flock`, `mkfifo`, `cmp`, `pgrep`, `sha256sum`, `mktemp`, `hexdump` present; no failsafe flag. `init.d` already holds four hooks of other apps (symlinks: amazoff, aerial screensaver, HyperHDR, PicCap); ours sorts last |
| 1 Install | PASS | `dev/install` answered and the app was registered (`getAppInfo`: `folderPath` `/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite`, also on HBC 0.7.2). All payload sums matched, **file modes and exec bits kept**, `usr/palm/packages/<id>/packageinfo.json` present. appinstalld rewrote `appinfo.json` (new mtime, same content). The copy in `/tmp` is deleted by the make target |
| 2 App and root | PASS | The app opened from `launch`; a screen capture showed "Not set up yet", the TV's address from the connection manager and the firmware line, i.e. HBC `exec` ran `ctl.sh status --json` as root. Two cosmetic issues (doubled word "firmware", log panel cut at 720p) fixed in ca97685 |
| 3 Set up | PASS after fix | `install` through HBC `exec`: aarch64 build chosen (`uname -m` aarch64, triple check), config 0600 with a random name, pairing state directory, firmware guard line, pairing window opened. **`start` through `exec` never answered**, and every started process held the HBC service's Luna sockets; two `start.sh` subshells stayed behind holding the `exec` output sockets, and the rotator watched one of them (**bug 1**, fixed in 6410eff). Re-checked on the TV: `start` answered in 1.6 s, supervisor and rotator hold only 0 to 2 and their script (plus an inotify descriptor every dynamic process gets at exec on this TV), the daemon only its own |
| 4 Survival | PASS | After the fix: app closed (`closeByAppId`), 3 min 11 s later the same daemon pid ran; HBC's service had exited and come back (old node pid gone) and answered `getConfiguration` with `root: true` |
| 5 Update while running | PASS after fixes | First attempt (0.0.1 to 0.0.2): the running daemon was untouched by the app install and "update pending" was reported; opening the app applied the update (config hash unchanged), but the new daemon was stopped again 1 s later and stayed stopped. Cause: two installs, one from the app instance left in the background since step 2 and one from the relaunched app; the second stopped the first one's daemon and then died in a `set -e` race in `stop_daemon` (**bug 2**, fixed in d749183; same-payload install made a no-op in 8959a39, **change 3**). Second attempt (0.0.2 to 0.0.3, app closed first): app install did not touch pid 15944; app opened, update applied within 4 s, new pid, one supervisor, one daemon, config hash unchanged |
| 6 Start at boot | PASS | `autostart on` through `exec`: hook 0755 with `PREFIX=/var/lib/webos-satellite`. 0.0.4 installed without opening the app, reboot 1: SSH back about 37 s after boot, no failsafe flag, `startup.sh` ran, dropbear up; `autostart.log`: "package update pending (0.0.3 -> 0.0.4); applying", "update applied" (config kept, hook refreshed), then "firmware check failed ... not starting" (03.30.73 is not in the allow-list: the guard works at boot). With `03.30.73` added to this test install's allow-list, reboot 2: "validated; starting", supervised daemon about 100 s after boot, microphone closed by the not-paired gate, pairing window open. No failsafe |
| 7 Rollback | PASS | `mic off`: the daemon logged the kill switch 12 ms later; `mic on`; `stop` (hit the race of bug 2, handled: "daemon not running" after the supervisor stop); **downgrade 0.0.4 to 0.0.2 accepted by appinstalld** (`getAppInfo` 0.0.2), `ctl.sh install` applied it with the config hash unchanged |
| 8 Removal | PASS | (after step 9, with the app reinstalled) `uninstall`: hook gone, daemon and `bin/` gone, `etc/` (same config hash) and `log/` kept; `purge`: `/var/lib/webos-satellite` gone; `dev/remove`: app and package directories gone, `getAppInfo` "Invalid appId". The final reboot of the procedure was not repeated: reboot 3 (step 9) had already shown the inert hook, and after `purge` no file of ours is left that a boot could run |
| 9 App removed without uninstall | PASS | With start at boot on, `dev/remove` left the hook and `/var/lib/webos-satellite` in place, as designed. Reboot 3: no failsafe, dropbear up, `autostart.log` unchanged, no process of ours: the hook saw that the app was gone |

**Unplanned:** after reboot 3 the TV put itself into `Active Standby` 53 s after boot (power-off reason `homekit`, sent by `com.webos.service.qbd` after `waitAllDevicesResumeDoneTimeout`), with no remote press in the log. After reboots 1 and 2 it came up `Active`. Woken with `networkinput/controls/power` to the home app. Not caused by our package as far as the log shows (no process of ours ran in that boot); recorded as an observation about `reboot` on this TV.

## Conclusion and confidence

The package works on c3-b with HBC 0.7.2: appinstalld accepts an ipk not built by ares-cli, the app reaches root through HBC `exec`, updates keep config and identity with one daemon, start at boot works through the hook without failsafe and applies a pending update, removal is complete, and downgrades are accepted. Confidence: high for this unit, firmware and HBC version (each path exercised once or twice; the update path four times). Three defects only showed on the TV (HBC's `exec` descriptors, BusyBox timing of the supervisor's TERM, a second app instance); each now has a host test.

## Rollback and cleanup

At any point: `touch /tmp/webos-satellite.disable` (microphone off), `CTL stop`, `rm -f /var/lib/webosbrew/init.d/webos-satellite` (no autostart at the next boot), `CTL purge`, `make tv-remove-ipk`. If failsafe mode appears, HBC skipped the hooks for that boot: delete the hook over SSH or telnet and reboot normally ([autostart rollback](../design/autostart.md#rollback)).

## Follow-ups / unresolved questions

- Repeat steps 1-8 on HBC 0.6.3 (c3-a's version) only with the owner's separate approval; c3-a is the owner's daily display.
- Install through HBC itself from a repository manifest (HBC's `install` method; publication path).
- Whether a daemon started through `exec` survives an HBC **self-update** (HBC kills its services with SIGKILL during it) or a luna hub restart: not provoked.
- Why `reboot` sometimes ends in `Active Standby` (`homekit`, qbd) on c3-b.
