# Security note: file permissions of what runs as root

- **Status:** fix implemented in the packaging and tested on the host (2026-11-07). **Not yet on a TV:** the next install of a version with this change (the owner's usual deploy, install only) is the first run on c3-a; the checks in [section 8](#8-what-needs-a-tv-check) are open.
- **Date:** 2026-11-07. PC only, no TV access. Device facts are copies of earlier read-only runs (raw output in the gitignored `devices/private/c3-a/`, not committed).
- **Trigger:** EXP-025 Part A found the developer app directories on c3-a world-writable, ours included ([EXP-025](../experiments/EXP-025-mic-button-toast.md#part-a-results-2026-11-07), side note under "Placement of the file").
- **Scope:** the webOS app package in [`packaging/webos/`](../../packaging/webos/) (`lib.sh`, `ctl.sh`, `install.sh`, `start.sh`, `supervise.sh`, `autostart.sh`, the boot hook, the TV app) and the daemon's call of `ctl.sh` (`crates/satellite/src/config_switch.rs`). The phone setup server's own directory checks are in the [setup server review](setup-server-review.md) (SR-02) and are not repeated here.
- **Related:** [packaging design](../design/packaging.md#3-on-the-tv), [SECURITY.md](../../SECURITY.md#app-package), [c3-a compatibility record](../compatibility/c3-a.md).

## 1. Summary

Root runs the satellite's code from two places:

1. **The install directory** `/var/lib/webos-satellite` (ours): the daemon, the supervisor, the scripts, the config. It is made by `install.sh` with mode 0700 directories and 0700/0600 files, under `/var/lib` and `/var`, which are root's 0755 on c3-a. It was not world-writable.
2. **The app directory** `/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite` (the TV's installer lays it out): `payload/ctl.sh` and `payload/lib.sh` ran as root on every TV app action and status refresh (every 10 s while the app is open, through the Homebrew Channel's root `exec`), and the boot path ran `payload/ctl.sh install`, which runs `payload/install.sh` and copies the payload's binaries and scripts into the install directory. On c3-a this directory and every directory above it up to `/media/developer` are mode 0777.

**Severity: High** (local privilege escalation to root) for the app directory path: any local process that can write under `/media/developer/apps` could replace `payload/ctl.sh` (or rename the `payload` directory away and put its own in place) and get its code run as root the next time the app is opened, or at the next boot with start at boot on and no user action at all (the update-at-boot path). **Marginal risk on c3-a is lower:** the same tree holds the Homebrew Channel's own root service with world-writable files (below), so a process that can write there has root through the Homebrew Channel anyway. The fix is still worth it: it removes our package as a path, and other TVs or Homebrew Channel versions may not share that weakness (c3-b's app directory was 0755 on 2026-11-03).

Which local non-root processes can actually reach `/media/developer/apps` (web apps in the web app manager, native apps in their jails) is **not known** (`hypothesis`: at least a compromised developer-mode app could).

## 2. Evidence

| Claim | Label | Source |
| --- | --- | --- |
| `/media/developer`, `/media/developer/apps`, `/media/developer/apps/usr/palm/applications` and our app directory are `drwxrwxrwx root root`; `/tmp` is `drwxrwxrwt` | `verified-on-target (read-only, EXP-025 copy)`, c3-a, firmware 03.30.74, 2026-11-07 | EXP-025 Part A, item a8 (raw file kept private) |
| The Homebrew Channel's service files (`service.js`, `startup.sh`, `elevate-service`, `bin/`) are mode 0777, owner uid 1001 | `verified-on-target (read-only)`, c3-a, 2026-11-01 | [D0-D1](../discovery/c3-a/D0-D1-runtime.md) raw listing (private) |
| `/var/lib` and `/var` are `drwxr-xr-x root root`; `/var/lib/webosbrew/init.d` is `drwxr-xr-x root root` | `verified-on-target (read-only)`, c3-a, 2026-11-01 | D0-D1 raw listing (private) |
| The install directory's files: `bin/*` `-rwx------ root`, logs and marker `-rw------- root`; `app-dir` `-rw-r--r-- root` | `verified-on-target (read-only)`, c3-a P1 and c3-b EXP-009 | private listings |
| On c3-b (HBC 0.7.2, 2026-11-03) the app directory was `drwxr-xr-x root root`, `payload/` the same, payload files `root` 0755 (scripts, binaries) and 0644 (data): the ipk's own tar modes | `verified-on-target (read-only)`, c3-b | [EXP-009](../experiments/EXP-009-ipk-install-update.md) step 1b (private) |
| The modes of the payload **files** on c3-a, and whether the installer resets the app directory's mode on an update | **unknown** (not recorded) | open, [section 8](#8-what-needs-a-tv-check) |
| The ipk's tar entries are owned by uid 0 with mode 0755 for directories and executables and 0644 for data; `build_ipk.py check` refuses group/other-writable entries | `documented` (repository code) | `packaging/webos/ipk/build_ipk.py` |

Why the app directory came out 0777 on c3-a but 0755 on c3-b is not known (different Homebrew Channel and firmware versions; `hypothesis`: the installer creates the top directory itself on c3-a).

## 3. What runs as root, and what it reads

| Who (as root) | Runs or sources | Reads or writes | From where |
| --- | --- | --- | --- |
| TV app, through `luna://org.webosbrew.hbchannel.service/exec` | `ctl.sh` with a fixed argument list (and `lib.sh` it sources) | everything below that `ctl.sh` does | **before:** always `payload/ctl.sh` (app directory). **Now:** `/var/lib/webos-satellite/bin/ctl.sh` once the install and its `app-dir` record exist; `payload/ctl.sh` only before the first install |
| `ctl.sh install` (app "Set up" and "Apply update", update at boot, SSH) | `payload/install.sh`, `payload/lib.sh`; each payload binary with `--version` before it is installed; copies binaries, scripts, the hook template and `config.default.toml` into `bin/` and `etc/` | `payload/SHA256SUMS`, `VERSION`; hands `payload/overlay.ipk` to appinstalld (root) | app directory |
| `ctl.sh start`, `restart`, `config ...` (also from Home Assistant's switches, through the daemon), `report`, `setup start`, `status` | `bin/start.sh` → `bin/supervise.sh`, `bin/rotate.sh`; `bin/webos-satellite` (`--version`, `--check-firmware`, `--check-config`, `--report`); `bin/webos-satellite-setup` | `etc/config.toml`, `etc/*`, `run/*` (`voice.json`, `compat.json`, `report.*`, `lg_voice_override`, the lock `run/lg_voice.lock`, `config-change.stamp`), `log/*`; `app-dir` names the payload | install directory |
| Supervisor `bin/supervise.sh` | `bin/webos-satellite` in a loop; `bin/ctl.sh restore-lg-voice` after every daemon exit | `run/*.pid`, `run/lg_voice_override`, appends `log/webos-satellite.log` | install directory |
| Daemon `bin/webos-satellite` | `bin/ctl.sh config ...` (ADR 0012 switches) with an empty environment; `luna-send`, `pacat`, `python3` from `PATH` | writes the report directory (default `run/`: `voice.json`, `report.*`, `compat.json`, `config_switch.out`), `etc/` (pairing key, reply volume, remote-off), takes `run/lg_voice.lock` | install directory |
| Boot hook `/var/lib/webosbrew/init.d/webos-satellite` (run by the Homebrew Channel's `startup.sh`) | `bin/autostart.sh` → `bin/lib.sh`; with a pending update, `ctl.sh install` (before: `payload/ctl.sh install`; now: `bin/ctl.sh install`) → `bin/start.sh` | `app-dir`, `disable`, `crashloop`, `log/autostart.log` | install directory, payload for an update |
| `ctl.sh mic off`, the lock, `setup ...` | | create or write `/tmp/webos-satellite.disable`, `/tmp/webos-satellite.lock/`, `/tmp/webos-satellite-setup/` (`stop`, `heartbeat`, `answer.json`, `state.json`) | `/tmp` (world-writable, sticky) |

The overlay app (`<id>.overlay`) and the TV app itself run in the web app manager, not as root. Their files are in the same world-writable tree: a replaced `app.js` could make the TV app send any command to the Homebrew Channel's `exec` the next time the owner opens it. That boundary belongs to the Homebrew Channel (its `exec` runs what it is given as root) and is not closed by this change (residual risk R3).

## 4. Which paths must not be writable by other users

- **Never writable by group or others, owner root:** everything root runs or sources (`bin/*`, `payload/*` before an install or update), the config (`etc/config.toml`), the install directory and its `bin/`, `etc/`, `log/`, and every directory above the install directory (unless sticky).
- **Private (0700, root):** `run/`, which holds the lock root opens for writing and the files the daemon writes and `ctl.sh` reads. A planted link there would make root write through it.
- **Owned by root, mode cannot be relied on:** the app directory and its files. The installer lays them out; on c3-a the directories above are 0777 and are not ours to change. What matters is that a non-root process cannot create a file owned by root: an owner check catches a replaced file even when the mode cannot be trusted.
- **In `/tmp`:** never write through a link or into a directory another user made: the lock directory, the kill switch, the setup session directory.

## 5. What the installer controls and what persists

- The install directory `/var/lib/webos-satellite` is ours: the installer never touches it, and its modes persist across app updates. `install.sh` sets them on every install (0700 directories, 0700 executables, 0600 config and records).
- The app directory is replaced by the installer on every install or update of the app, with the modes the installer chooses (0777 for the top directory on c3-a; unknown for the files). Anything we change there lasts only until the next update. So the fix: (a) removes group/other write from the app's own directory and files at every `ctl.sh install` and at every start (`start.sh`), (b) refuses to run anything from it when a file is not owned by root, and (c) runs the app's commands from the install directory once it exists. Whether the installer resets 0755 back to 0777 on an update is open ([section 8](#8-what-needs-a-tv-check)).
- The directories above the app directory (`/media/developer/apps/...`, 0777) belong to the TV and the Homebrew Channel. They are not changed (AGENTS.md: no changes to the platform). Their state is the reason the app directory can never be fully trusted.

## 6. The fix

All checks share one rule set, `trust_check` in [`lib.sh`](../../packaging/webos/lib.sh) (one `stat` for all paths; links are never followed). Trusted owner: root (the caller's own uid only in host tests, which do not run as root).

| Part | Before | Now |
| --- | --- | --- |
| TV app ([`app.js`](../../packaging/webos/app/app.js)) | `sh '<app>/payload/ctl.sh' ARGS` | `c=/var/lib/webos-satellite/bin/ctl.sh; [ -f "$c" ] && [ -f /var/lib/webos-satellite/app-dir ] \|\| c='<app>/payload/ctl.sh'; sh "$c" ARGS`: the installed copy for everything once installed, updates included (it reads the payload, checks it, then runs its `install.sh`) |
| `ctl.sh install` | ran `payload/install.sh` as found | `payload_trust_check` first: every file in the app directory and `payload/` must be a regular file owned by root (refused otherwise, nothing run); group/other write is removed from them and from the two directories (0777 becomes 0755), and named |
| Update at boot (`autostart.sh`) | `sh <app>/payload/ctl.sh install` | `bin/ctl.sh install` (checked, see above); on refusal the installed version starts |
| `ctl.sh start`, `restart`, `config`, `report`, `setup start`; `start.sh`; `autostart.sh` | ran `bin/*` as found | `install_trust_check` first: the directories above the install directory (root's, not writable by others unless sticky), the install directory, `bin/`, `etc/`, `log/` (group/other write removed, or refused when not root's), every file in `bin/` and `etc/config.toml` (refused when not root's or writable by group or others: the content may have changed), `app-dir` and the marker, and `run/` (made 0700; when it was not, links and files of other users in it are deleted). Refusals name the file. `status` runs nothing from an install that fails and reports `install_problem` |
| `start.sh` | | also removes group/other write from the app's files at every start (the installer lays them out again on each update); a problem there is a warning only |
| Supervisor | ran the daemon and `ctl.sh restore-lg-voice` as found | the same check before every daemon start and every restore; on failure it logs the reason and `NOT STARTING the daemon` and ends (no crash-loop marker) |
| Boot hook | ran `bin/autostart.sh` | runs it only when the install directory, `bin/`, `autostart.sh` and `lib.sh` are root's and not writable by group or others (`stat`, no output) |
| Daemon (`config_switch.rs`) | ran `bin/ctl.sh config ...` | checks `bin/`, `bin/ctl.sh` and `bin/lib.sh` the same way first; a failure is logged and HA gets the stored state again |
| `install.sh` | | refuses an install directory, or a directory above it, that is not root's or is writable by others; removes group/other write from a kept config |
| `ctl.sh` | the caller's umask | `umask 077` for everything it creates |
| `run/lg_voice.lock` (`ctl.sh restore-lg-voice`) | opened in `run/` as found | `run/` made private (0700) first |
| `/tmp/webos-satellite.lock` | `mkdir` with the caller's umask; a link or another user's directory held the lock | `mkdir -m 700`; a link or another user's directory at that name is removed, not honoured |
| `/tmp/webos-satellite.disable` (`ctl.sh mic off`) | `: >file` follows a planted link (truncates its target as root) | a link is removed; the file is created with `noclobber` (`O_EXCL`) |
| `/tmp/webos-satellite-setup/` | `ctl.sh setup status` and `stop` wrote `heartbeat` and `stop` into whatever directory was there | used only when it is a real directory owned by root (the shell's `-O`, `stat` as a fallback); otherwise reported as no session and removed by the next `setup stop` or `setup start` |

Modes stay stricter than 0755/0644 where they were: binaries and scripts in `bin/` are 0700 and the config 0600 (it can hold the key). Only root reads them.

## 7. Residual risks

- **R1, time of check to time of use:** between the owner check of a payload file and its use, a process that can write the app directory's parent could swap it again. Small window, at an install the owner starts; not closed (it would need a signed package).
- **R2, root-owned files moved around:** a non-root process cannot create a root-owned file, but it can rename existing ones in a 0777 directory, for example keep an older version's payload and put it back (a downgrade). Not closed.
- **R3, the TV app's own files:** `app.js` and `index.html` are in the same tree and run with the Homebrew Channel's root `exec` at hand. Their group/other write is removed at install and start, but the directory above is 0777, so the app directory can be replaced as a whole. The Homebrew Channel's own service is world-writable on c3-a too; both are platform issues.
- **R4, the kill switch is a denial of service:** any user can create `/tmp/webos-satellite.disable` and turn the microphone off. Accepted: it fails safe for privacy.
- **R5, the first update to a version with this change** runs through the older installed `ctl.sh` (the app now calls the installed copy), which has no payload check; from then on the check applies.

## 8. What needs a TV check

Not run; for the owner's next install of a version with this change (install only), then read-only:

1. After `ctl.sh install` from the app: `stat -c '%a %U %n'` of the app directory, `payload/` and its files: expected 755 (directories, scripts, binaries) and 644 (data), root, and the install log line "removed group/other write from the app's files" naming the app directory (was `drwxrwxrwx`).
2. **Does an app update reset the modes?** Install the next ipk through appinstalld, then the same `stat` before opening the app: 0777 again (the installer resets) or 0755 (it keeps them). Either way `start.sh` fixes them at the next start; the result decides whether this note's "at every start" is needed or only belt and braces.
3. The TV app still opens, `status` and the buttons work through the installed `ctl.sh` (HBC `exec` command text above), and the update path through `bin/ctl.sh install` applies the next version.
4. `ctl.sh status` prints no `NOT TRUSTED` line and the daemon starts (no false refusal from the real TV's modes: `/var/lib` 0755 root was read on 2026-11-01).
5. `[ -O /tmp ]` in BusyBox `sh` on the TV returns 0 as root (else the setup status poll falls back to one `stat` per call).

Rollback: reinstall the previous ipk (`make tv-remove-ipk` and the older package, [development](../development.md#install-the-app-package-on-a-rooted-tv-persistent)); the changed modes (0755 app directory) are harmless and the installer replaces them on the next install.

## 9. Tests (host)

- `packaging/webos/tests/run.sh`: a world-writable binary (refused by `start.sh`, `autostart.sh` and `ctl.sh start`, the file named), a group-writable config, files of another user (`WEBOS_SAT_TEST_TRUST_UID`, honoured only when not root), `run/` 0755 with a planted link (made 0700, the link deleted, logged, started), the supervisor refusing a restart once the binary is world-writable, the boot hook launching nothing when `autostart.sh` is world-writable.
- `packaging/webos/tests/setup.sh`: a link planted at the kill switch's name, a link at the lock's name, another user's setup session directory with a look-alike process and a link at `heartbeat` (nothing written, target untouched, process not signalled).
- `packaging/webos/tests/tv_settings.sh`: `run/` that is a file refuses `ctl.sh config` before anything runs.
- `packaging/webos/tests/ipk.sh`: an app directory 0777 and a world-writable payload script fixed at install and named; app files of another user refused before anything runs; the app directory fixed again at start.
- `crates/satellite` (`config_switch`): a `ctl.sh` that others can write is not run; a group-writable `bin/` and a linked `lib.sh` are refused.
- The app tests (`app_smoke.js`, `app_matrix.js`) check the new command text.
