# webOS app package (ipk): design and status

- **Status:** **built and tested on a PC only (2026-11-03).** `make ipk` produces an installable, updatable package; nothing has been installed on a TV yet. The on-TV verification is [EXP-009](../experiments/EXP-009-ipk-install-update.md), planned for c3-b (the second LG C3) with the owner's authorization. c3-a is not a target for it.
- **Date:** 2026-11-03. External sources were read on 2026-11-03 at the commits listed under [Sources](#sources).
- **Scope:** milestone 3a of the [install and configuration UX](install-and-config-ux.md#41-phase-3-slice-with-autostart-owner-approval-gated-on-the-tv) and the "reproducible ipk" deliverable of [roadmap Phase 5](../project/roadmap.md#phase-5-second-tv-and-packaging). Manual install now; a Homebrew Channel (HBC) repository later.
- **Files:** [`packaging/webos/`](../../packaging/webos/): `ctl.sh` (entry point), `install.sh` and the ops scripts, `config.default.toml`, `init.d/webos-satellite` (hook template), `app/` (status app), `ipk/build_ipk.py` (builder and checker), `tv/appinstall.sh` (appinstalld helper), `tests/`.
- **Related:** [autostart design](autostart.md), [always-on](always-on.md), [development: install the app package](../development.md#install-the-app-package-on-a-rooted-tv-persistent).

Labels follow [AGENTS.md](../../AGENTS.md). Facts about HBC and other apps are `documented` (their README or schema) or `observed-on-other-device` (their source code); nothing in this document is `verified-on-target` yet.

## 1. Decisions

| # | Decision | Why | Source |
| --- | --- | --- | --- |
| P1 | **One ipk with both static builds** (armv7 and aarch64), `Architecture: all`, named `<id>_<version>_all.ipk`. The build is chosen when the payload is installed on the TV by the existing `install.sh` logic (`uname -m`, then the binary must answer `--version` with its own target triple, else armv7) | c3-a has a 32-bit userland on an aarch64 kernel, so the package architecture says nothing useful about which binary runs; one file is simpler to publish and to update. Size: about 3.5 MB compressed | ares-package names non-native packages `_all` (`lib/package.js` `setIpkFileName`); HBC's release script expects `<id>_<version>_all.ipk` (`tools/gen-manifest.js`); [development: choosing the build](../development.md#choosing-the-build-armv7-or-aarch64) |
| P2 | **A web app (`type: web`), no Luna service.** The app controls the satellite through HBC's root `exec` with fixed argument lists for the package's `payload/ctl.sh` | The daemon is a static musl binary without `libLS2`, so it cannot be a Luna service; `elevate-service` exists to rewrite a Luna service's `Exec=` line and role file, which we do not have. PicCap's web UI calls `exec` the same way | HBC README (`exec`), `services/elevate-service.ts`; PicCap `frontend/js/servicecalls.js` |
| P3 | **Layout as ares-package writes it**, built by a standard-library Python script instead of ares-cli: `ar` members `debian-binary` (`2.0`), `control.tar.gz` (one `control` file with the same fields), `data.tar.gz` with `usr/palm/applications/<id>/` and `usr/palm/packages/<id>/packageinfo.json` | No npm toolchain in the build; byte-for-byte reproducible output (sorted entries, fixed mtimes from `SOURCE_DATE_EPOCH` or the commit time, uid/gid 0, gzip without name or time) | ares-cli `lib/package.js` (`createControlFile`, `fillPackageDir`, `makeTgz`, `makeIpk`); HBC reads `Package` from `control.tar.gz` (`services/service.ts` `packageInfo`) |
| P4 | **App id `io.github.pando85.voicesatellite`** (Makefile `APP_ID`) | LG rules: lowercase letters, digits, `-`, `.`; not `com.palm`, `com.webos`, `com.lge`, `com.palmdts`; reverse domain; permanent once published. The project's host gives the reverse domain; `org.webosbrew.*` is used by apps of the webosbrew organisation; no `-` to avoid LG's caution for JS services. **Owner decision before publishing**, because an id change is a different app | [LG appinfo.json reference](https://webostv.developer.lge.com/develop/references/appinfo-json); ares-cli's id check `^[a-z0-9.+-]*$` |
| P5 | **Version = the workspace version** in `Cargo.toml` (three numbers, no leading zeros). Update detection on the TV compares the payload's `SHA256SUMS` with the installed copy, not the version | LG's version format; HBC offers an update when the repository version differs. Comparing sums also catches two builds with the same version during development | LG appinfo.json reference; HBC README ("Updating") |
| P6 | **Persistent state outside the app: `/var/lib/webos-satellite`** (config, key, pairing state, mute state, logs, and the running copy of the daemon) | appinstalld replaces the app directory on every install, so anything kept there is lost on update (PicCap keeps its `config.json` in its service directory). Other apps keep state elsewhere: lginputhook in `/home/root/.config/lginputhook/`, HBC in `/var/lib/webosbrew/`. On c3-a `/var` is ext4, read-write, `nosuid`, not `noexec`, about 810 MB free (verified-on-target, [D0-D1](../discovery/c3-a/D0-D1-runtime.md#mount-layout)). Not `/media/developer`: HBC 0.7 supports other install locations | hyperion-webos `src/settings.h`; lginputhook `service/service.js`; HBC `startup.sh`, CHANGELOG 0.7.0 |
| P7 | **The daemon runs from a copy** in `/var/lib/webos-satellite/bin`, made by `ctl.sh install` from the app's payload, never from the app directory | The exec bit of files in an installed ipk does not matter (the copy is `chmod`ed; scripts run with `sh`), which removes open question 2 of the UX design for us. An update of the app does not touch the running binary, and `/proc/<pid>/exe` keeps naming the installed path, so the scripts always find the running daemon and never start a second one | [install UX §2.1](install-and-config-ux.md#21-homebrew-channel-packages-repository-elevation-and-autostart) consequences |
| P8 | **No reliance on install-time scripts.** The payload is installed (and updates applied) by: the app's "Set up" button; opening the app when an update is pending; at boot by `autostart.sh` when "start at boot" is on; or by hand (`ctl.sh install`) | No webOS or HBC source documents maintainer scripts for app ipks; HBC re-elevates itself after a self-update from its own service code, which suggests none run (hypothesis) | HBC `services/service.ts` (`install`, self-update comment) |
| P9 | **Boot hook is opt-in** and written by `ctl.sh autostart on` as a small real file `/var/lib/webosbrew/init.d/webos-satellite` (template with the install directory filled in), deleted by `autostart off`, `uninstall` and `purge`. It keeps the [autostart design](autostart.md) rules: returns at once, prints nothing, starts `autostart.sh` detached, which waits 60 s and honours `disable`, `crashloop`, the kill switch and the firmware allow-list | A copy pointing at the state directory does not depend on where the app is installed and does not dangle when the app is removed. lginputhook writes a file the same way; PicCap writes a symlink into its service directory. File names follow `run-parts` rules (letters, digits, `_`, `-`) | lginputhook `service/service.js` (`autostart`); hyperion-webos `src/service.c`; RootMyTV `files/stage3.sh` (`init.d/README.txt` text) |
| P10 | **Removal is explicit.** "Remove service (keep settings)" (`ctl.sh uninstall`) deletes the hook, stops and deletes the daemon, keeps `etc/` and `log/`; "Remove service and all data" (`ctl.sh purge`) deletes the whole directory. An app removed without either: the hook and `autostart.sh` see that the recorded app directory is gone and start nothing; the data stays until `purge` or `rm -rf /var/lib/webos-satellite` | Removing an app through HBC or the launcher calls appinstalld's `dev/remove`, which runs nothing of ours (P8) | HBC `services/service.ts` (`uninstall` → `dev/remove`) |
| P11 | **One operation at a time:** install, update, start, stop, uninstall and the boot start take a lock (`/tmp/webos-satellite.lock`, a directory with the owner's pid, taken over when the owner is dead); files in `bin/` are replaced by rename so a running script keeps its old copy; the daemon starts in a new session (`setsid`) so it does not depend on the HBC `exec` that started it | An update while the boot start waits, or two presses of Start, must not produce two daemons | host tests below |
| P12 | **The app sends only whitelisted commands** (`status --json`, `install`, `start`, `stop`, `autostart on/off`, `mic off/on`, `pair`, `reset-pairing`, `clear-crashloop`, `uninstall`, `purge`) and builds the path from its own location. Luna calls use the web runtime's `PalmServiceBridge` directly, which is what LG's `webOSTV.js` does inside `webOS.service.request`, so no third-party library ships. The TV's address comes from the documented connection manager `getStatus` | No typed or received text reaches a root shell ([install UX §3.1](install-and-config-ux.md#31-option-a-a-webos-app-ui-on-the-tv-remote-control)) | `webOSTV.js` 1.2.4 as bundled by PicCap; [LG connection manager API](https://webostv.developer.lge.com/develop/references/connection-manager) |
| P13 | **An HBC package manifest is generated** next to the ipk (`<id>.manifest.json`: id, version, type, title, `iconUri` as a `data:` URI, `rootRequired: true`, relative `ipkUrl`, `ipkHash.sha256`). Not published | Ready for a self-hosted repository (HBC `addRepository`) or an `apps-repo` entry later; both need the owner's decision ([install UX open question 7](install-and-config-ux.md#6-open-questions)) | webosbrew/docs `HomebrewPackageManifest.schema.json`; lginputhook `buildmanifest.js` |

## 2. Package contents

```
<id>_<version>_all.ipk  (ar: debian-binary, control.tar.gz, data.tar.gz)
usr/palm/applications/<id>/
  appinfo.json, index.html, app.js, style.css, icon.png (80x80), largeIcon.png (130x130)
  payload/
    ctl.sh install.sh lib.sh start.sh stop.sh status.sh supervise.sh autostart.sh rotate.sh uninstall.sh
    webos-satellite-armv7 webos-satellite-aarch64     (static musl builds)
    config.default.toml  webos-satellite.hook  VERSION  SHA256SUMS
usr/palm/packages/<id>/packageinfo.json
```

The icons are drawn by the build script; no image or other third-party binary is committed. `build_ipk.py check` (run by `make ipk`) verifies the `ar` member order, `debian-binary`, the control fields, the file name, appinfo.json (required fields, id and version equal to control, icon files and sizes), packageinfo.json, safe paths, root ownership and modes, the payload file list, every `SHA256SUMS` line, the ELF class and machine of both builds, the `@PREFIX@` placeholder in the hook template and the absence of an inline key in the default config.

## 3. On the TV

```
/media/developer/apps/usr/palm/applications/<id>/   the app (replaced on every install)
/var/lib/webos-satellite/                           the install (survives updates and reinstalls)
  bin/       webos-satellite (copy of the chosen build), the scripts, webos-satellite.hook
  etc/       config.toml (0600), noise_psk (0600, only with a fixed key),
             pairing/noise_psk (0600, set by Home Assistant), muted
  log/       webos-satellite.log (+ .1), autostart.log
  run/       pidfiles
  VERSION, package.sums, app-dir, .webos-satellite-install
/var/lib/webosbrew/init.d/webos-satellite           the boot hook, only after "start at boot: on"
/tmp/webos-satellite.disable                        the kill switch ("Microphone off"), cleared at reboot
```

The app directory above is where `dev/install` puts apps on HBC 0.6.x (HBC's own README and install script use that path); on HBC 0.7 it can differ, which is why the scripts take it from their own location and record it in `app-dir`.

**First install** (`ctl.sh install`, the app's "Set up"): writes `etc/config.toml` from `config.default.toml` with a random device name (`webos-satellite-<6 hex>`) and a random `mac_seed`, so every TV is a separate device in Home Assistant and keeps its identity on later updates. Pairing is on and mDNS off: the owner adds the device in HA by host and port 6053 within the 10-minute window, and HA 2026.9 or newer sets the key ([pairing](install-and-config-ux.md#5-pairing-mode-design-option-c-detail)). The firmware allow-list is c3-a's (`03.30.74`); on any other firmware the daemon runs with the microphone off and the app says so. `--config` and `--psk-file` set an owner's own config or fixed key instead.

**Update:** installing a newer ipk replaces only the app directory; the daemon keeps running the old copy. The update is applied the next time the app is opened (automatically), at the next boot (when start at boot is on), or by `ctl.sh install`: stop (supervisor first, then the daemon), install the new build and scripts, keep `etc/` and `log/`, refresh the hook if present, start again if it was running. If the new build does not run, the old install stays and is restarted.

## 4. The status app

Shows: whether the HBC service is root, installed and package versions (and a pending update), the running build, running or stopped and supervised, start at boot, the microphone kill switch, the firmware guard result, the pairing state (fixed key, paired, not paired, unpaired by HA), the device name, the TV's address and port, and the last log lines. Buttons appear only when they apply: Set up, Apply update, Start, Stop, Start at boot on/off, Microphone off/on, Open pairing window, Reset pairing, Clear crash-loop marker, Remove service (keep settings), Remove service and all data. Destructive actions need a second press within 5 s. Arrow keys move between buttons.

**Not built yet (stubbed with a "not available yet" note):** the pairing window state and countdown (the daemon has no status output for it; [install UX 4.1](install-and-config-ux.md#41-phase-3-slice-with-autostart-owner-approval-gated-on-the-tv)), power options ([always-on §7](always-on.md#7-exposing-power-options-in-the-ui-design), waits for D5), and settings such as microphone gain (planned as HA config entities). Translations and screen-reader review are not done.

## 5. Verified on the host (2026-11-03)

| Test | What it covers |
| --- | --- |
| `packaging/webos/tests/ipk.sh` (42 checks) | Builds ipks with the host build standing in for both TV builds; structure check; byte-identical rebuild; manifest fields and hash; invalid id and version refused. Unpacks them as appinstalld lays them out and drives `ctl.sh`: status JSON before install; first install (generated config with random name and MAC seed, mode 0600, valid, no hook, not started); `--config` and `--psk-file`; one supervised daemon after start and after a second start; no key in the status output; kill switch; hook (0755, prefix filled in, returns at once without output, launches `autostart.sh` detached); update while running (new pid, old one gone, one daemon and one supervisor, new binary, config and key unchanged, hook refreshed); update at boot by `autostart.sh`; two concurrent starts give one daemon; a stale lock is taken over; app removed: neither `autostart.sh` nor the hook starts anything; uninstall keeps config, key and logs; reinstall keeps the identity; purge removes everything and refuses a directory that is not an install; two fresh installs get different identities; refused without root |
| `packaging/webos/tests/run.sh` | The existing ops-script checks (supervisor, crash loop, autostart gating, build selection), unchanged and passing |
| `packaging/webos/tests/app_smoke.js` | The app with a fake DOM and `PalmServiceBridge`: rendering per state, Set up runs install then start, a pending update is applied once on open, confirm-twice for purge, the not-root screen, only whitelisted commands and only the HBC exec and connection manager services are called |
| BusyBox | `ipk.sh` and `run.sh` also pass with BusyBox 1.37 applets first on `PATH` (the TV's `sh` is BusyBox ash; c3-a's applet set may differ) |
| Real builds | The `make ipk` package unpacked on the PC with `qemu-user` binfmt: `ctl.sh install` chose the aarch64 build for `uname -m` aarch64 and armv7 for armv7l, the generated config passed `--check-config`, status and purge worked |

Also: ShellCheck clean, `sh -n` of all scripts, `node --check` of the app.

## 6. Not verified; needs a TV

All of it, on c3-b first ([EXP-009](../experiments/EXP-009-ipk-install-update.md)): appinstalld accepts the package built without ares-cli; the app launches and HBC `exec` runs as root from it; a daemon started through `exec` survives closing the app and HBC's idle exit; the update path with a running daemon; the boot hook with a reboot (no failsafe); update at boot; uninstall, purge and app removal leave nothing behind; downgrade by installing an older ipk (accepted by appinstalld or not). HBC 0.6.3 (c3-a) and 0.7.x differ; each needs its own record.

## 7. Open questions

1. App id and title before the first publication (P4); repository license (needed for `apps-repo`).
2. Does appinstalld accept a downgrade (older version over newer)? If not, rollback is "remove the app (data kept), install the older ipk, `ctl.sh install`".
3. Does a process started through HBC `exec` stay alive when HBC's service is killed (HBC self-update kills app services with SIGKILL on webOS 4+, per its source comment) or when the luna hub restarts ([always-on](always-on.md#2-what-was-observed-on-c3-a-read-only-2026-11-03))?
4. Should the first install add the firmware of the TV it runs on to the allow-list after an explicit "I understand" ([install UX open question 5](install-and-config-ux.md#6-open-questions))? Today the owner edits `validated_firmware` by hand after validating capture on that model.
5. Publication path: self-hosted repository file or `apps-repo` (AI-use disclosure rule).

## Sources

Read 2026-11-03:

- [webos-homebrew-channel](https://github.com/webosbrew/webos-homebrew-channel) at tag v0.7.3 (release 2026-01-11): README (install with `luna-send-pub ... appInstallService/dev/install`, `exec`, `spawn`, init.d hooks), `tools/install.sh`, `services/service.ts` (`install`, `uninstall` via `dev/remove`, `getConfiguration` `homebrewBaseDir`, `exec`, self-update), `services/startup.sh`, `services/elevate-service.ts`, `tools/gen-manifest.js`, CHANGELOG.
- [TBSniller/piccap](https://github.com/TBSniller/piccap) at a243d0e: `frontend/js/servicecalls.js`, `servicefiles/`, `package.json`, `frontend/webOSTVjs-1.2.4/webOSTV-dev.js`; [webosbrew/hyperion-webos](https://github.com/webosbrew/hyperion-webos) at 00c932f: `src/service.c` (autostart symlink), `src/settings.h` (config path).
- [Simon34545/lginputhook](https://github.com/Simon34545/lginputhook) at a607900: `service/service.js` (init.d file, config under `/home/root/.config`), `buildmanifest.js`, `manifest.json`.
- [webos-tools/cli](https://github.com/webos-tools/cli) (ares-cli) at ed7a699: `lib/package.js`, `files/schema/ApplicationDescription.schema`.
- [webosbrew/docs](https://github.com/webosbrew/docs) at 555a3a0: `schemas/HomebrewPackageManifest.schema.json`.
- [RootMyTV](https://github.com/RootMyTV/RootMyTV.github.io) `files/stage3.sh` (init.d naming and exit-status rules).
- LG: [appinfo.json reference](https://webostv.developer.lge.com/develop/references/appinfo-json), [connection manager](https://webostv.developer.lge.com/develop/references/connection-manager).
