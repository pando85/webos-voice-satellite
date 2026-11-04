# webOS app package (ipk): design and status

- **Status:** **verified on c3-b (2026-11-03, HBC 0.7.2, firmware 03.30.73)** in [EXP-009](../experiments/EXP-009-ipk-install-update.md): install, set up, update, downgrade, start at boot, uninstall, purge and app removal; three bugs found there are fixed (section 6). Installed on c3-a (HBC 0.6.3, firmware 03.30.74) on 2026-11-03 with the owner's approval: [P4](../discovery/c3-a/P4-permanent-install.md) (install, set up, start, start-at-boot hook written, HA entry reconnected; the first boot with the hook is pending).
- **Date:** 2026-11-03. External sources were read on 2026-11-03 at the commits listed under [Sources](#sources).
- **Scope:** milestone 3a of the [install and configuration UX](install-and-config-ux.md#41-phase-3-slice-with-autostart-owner-approval-gated-on-the-tv) and the "reproducible ipk" deliverable of [roadmap Phase 5](../project/roadmap.md#phase-5-second-tv-and-packaging). Manual install only for now. The target is the official Homebrew Channel apps repository, decided 2026-11-03 ([P14](#1-decisions)); there is no self-hosted repository.
- **Files:** [`packaging/webos/`](../../packaging/webos/): `ctl.sh` (entry point), `install.sh` and the ops scripts, `config.default.toml`, `init.d/webos-satellite` (hook template), `app/` (status app), `ipk/build_ipk.py` (builder and checker), `tv/appinstall.sh` (appinstalld helper), `tests/`.
- **Related:** [autostart design](autostart.md), [always-on](always-on.md), [development: install the app package](../development.md#install-the-app-package-on-a-rooted-tv-persistent).

Labels follow [AGENTS.md](../../AGENTS.md). Facts about HBC and other apps are `documented` (their README or schema) or `observed-on-other-device` (their source code); section 6 holds what is `verified-on-target` on c3-b.

## 1. Decisions

| # | Decision | Why | Source |
| --- | --- | --- | --- |
| P1 | **One ipk with both static builds** (armv7 and aarch64), `Architecture: all`, named `<id>_<version>_all.ipk`. The build is chosen when the payload is installed on the TV by the existing `install.sh` logic (`uname -m`, then the binary must answer `--version` with its own target triple, else armv7) | c3-a has a 32-bit userland on an aarch64 kernel, so the package architecture says nothing useful about which binary runs; one file is simpler to publish and to update. Size: about 3.5 MB compressed | ares-package names non-native packages `_all` (`lib/package.js` `setIpkFileName`); HBC's release script expects `<id>_<version>_all.ipk` (`tools/gen-manifest.js`); [development: choosing the build](../development.md#choosing-the-build-armv7-or-aarch64) |
| P2 | **A web app (`type: web`), no Luna service.** The app controls the satellite through HBC's root `exec` with fixed argument lists for the package's `payload/ctl.sh` | The daemon is a static musl binary without `libLS2`, so it cannot be a Luna service; `elevate-service` exists to rewrite a Luna service's `Exec=` line and role file, which we do not have. PicCap's web UI calls `exec` the same way | HBC README (`exec`), `services/elevate-service.ts`; PicCap `frontend/js/servicecalls.js` |
| P3 | **Layout as ares-package writes it**, built by a standard-library Python script instead of ares-cli: `ar` members `debian-binary` (`2.0`), `control.tar.gz` (one `control` file with the same fields), `data.tar.gz` with `usr/palm/applications/<id>/` and `usr/palm/packages/<id>/packageinfo.json` | No npm toolchain in the build; byte-for-byte reproducible output (sorted entries, fixed mtimes from `SOURCE_DATE_EPOCH` or the commit time, uid/gid 0, gzip without name or time) | ares-cli `lib/package.js` (`createControlFile`, `fillPackageDir`, `makeTgz`, `makeIpk`); HBC reads `Package` from `control.tar.gz` (`services/service.ts` `packageInfo`) |
| P4 | **App id `io.github.pando85.voicesatellite`** (Makefile `APP_ID`) | LG rules: lowercase letters, digits, `-`, `.`; not `com.palm`, `com.webos`, `com.lge`, `com.palmdts`; reverse domain; permanent once published. The project's host gives the reverse domain; `org.webosbrew.*` is used by apps of the webosbrew organisation; no `-` to avoid LG's caution for JS services. **Decided by the owner (2026-11-03).** An id change later makes it a different app | [LG appinfo.json reference](https://webostv.developer.lge.com/develop/references/appinfo-json); ares-cli's id check `^[a-z0-9.+-]*$` |
| P5 | **Version = the workspace version** in `Cargo.toml` (three numbers, no leading zeros). Update detection on the TV compares the payload's `SHA256SUMS` with the installed copy, not the version | LG's version format; HBC offers an update when the repository version differs. Comparing sums also catches two builds with the same version during development | LG appinfo.json reference; HBC README ("Updating") |
| P6 | **Persistent state outside the app: `/var/lib/webos-satellite`** (config, key, pairing state, mute state, logs, and the running copy of the daemon) | appinstalld replaces the app directory on every install, so anything kept there is lost on update (PicCap keeps its `config.json` in its service directory). Other apps keep state elsewhere: lginputhook in `/home/root/.config/lginputhook/`, HBC in `/var/lib/webosbrew/`. On c3-a `/var` is ext4, read-write, `nosuid`, not `noexec`, about 810 MB free (verified-on-target, [D0-D1](../discovery/c3-a/D0-D1-runtime.md#mount-layout)). Not `/media/developer`: HBC 0.7 supports other install locations | hyperion-webos `src/settings.h`; lginputhook `service/service.js`; HBC `startup.sh`, CHANGELOG 0.7.0 |
| P7 | **The daemon runs from a copy** in `/var/lib/webos-satellite/bin`, made by `ctl.sh install` from the app's payload, never from the app directory | The exec bit of files in an installed ipk does not matter (the copy is `chmod`ed; scripts run with `sh`), which removes open question 2 of the UX design for us. An update of the app does not touch the running binary, and `/proc/<pid>/exe` keeps naming the installed path, so the scripts always find the running daemon and never start a second one | [install UX §2.1](install-and-config-ux.md#21-homebrew-channel-packages-repository-elevation-and-autostart) consequences |
| P8 | **No reliance on install-time scripts.** The payload is installed (and updates applied) by: the app's "Set up" button; opening the app when an update is pending; at boot by `autostart.sh` when "start at boot" is on; or by hand (`ctl.sh install`) | No webOS or HBC source documents maintainer scripts for app ipks; HBC re-elevates itself after a self-update from its own service code, which suggests none run (hypothesis) | HBC `services/service.ts` (`install`, self-update comment) |
| P9 | **Boot hook is opt-in** and written by `ctl.sh autostart on` as a small real file `/var/lib/webosbrew/init.d/webos-satellite` (template with the install directory filled in), deleted by `autostart off`, `uninstall` and `purge`. It keeps the [autostart design](autostart.md) rules: returns at once, prints nothing, starts `autostart.sh` detached, which waits 60 s and honours `disable`, `crashloop`, the kill switch and the firmware allow-list | A copy pointing at the state directory does not depend on where the app is installed and does not dangle when the app is removed. lginputhook writes a file the same way; PicCap writes a symlink into its service directory. File names follow `run-parts` rules (letters, digits, `_`, `-`) | lginputhook `service/service.js` (`autostart`); hyperion-webos `src/service.c`; RootMyTV `files/stage3.sh` (`init.d/README.txt` text) |
| P10 | **Removal is explicit.** "Remove service (keep settings)" (`ctl.sh uninstall`) deletes the hook, stops and deletes the daemon, keeps `etc/` and `log/`; "Remove service and all data" (`ctl.sh purge`) deletes the whole directory. An app removed without either: the hook and `autostart.sh` see that the recorded app directory is gone and start nothing; the data stays until `purge` or `rm -rf /var/lib/webos-satellite` | Removing an app through HBC or the launcher calls appinstalld's `dev/remove`, which runs nothing of ours (P8) | HBC `services/service.ts` (`uninstall` → `dev/remove`) |
| P11 | **One operation at a time:** install, update, start, stop, uninstall and the boot start take a lock (`/tmp/webos-satellite.lock`, a directory with the owner's pid, taken over when the owner is dead); files in `bin/` are replaced by rename so a running script keeps its old copy; the daemon starts in a new session (`setsid`) so it does not depend on the HBC `exec` that started it | An update while the boot start waits, or two presses of Start, must not produce two daemons | host tests below |
| P12 | **The app sends only whitelisted commands** (`status --json`, `install`, `start`, `stop`, `autostart on/off`, `mic off/on`, `pair`, `reset-pairing`, `clear-crashloop`, `uninstall`, `purge`) and builds the path from its own location. Luna calls use the web runtime's `PalmServiceBridge` directly, which is what LG's `webOSTV.js` does inside `webOS.service.request`, so no third-party library ships. The TV's address comes from the documented connection manager `getStatus` | No typed or received text reaches a root shell ([install UX §3.1](install-and-config-ux.md#31-option-a-a-webos-app-ui-on-the-tv-remote-control)) | `webOSTV.js` 1.2.4 as bundled by PicCap; [LG connection manager API](https://webostv.developer.lge.com/develop/references/connection-manager) |
| P13 | **An HBC package manifest is generated** next to the ipk (`<id>.manifest.json`: id, version, type, title, `iconUri` as a `data:` URI, `rootRequired: true`, relative `ipkUrl`, `ipkHash.sha256`). Not published | The starting point for the `apps-repo` entry under P14. Not published yet | webosbrew/docs `HomebrewPackageManifest.schema.json`; lginputhook `buildmanifest.js` |
| P14 | **Distribution: manual install only for now.** The target is the official Homebrew Channel apps repository (`webosbrew/apps-repo`), later. There is no self-hosted repository. The listing needs the AI-use disclosure in the PR, and the generated manifest (P13) next to the ipk is the starting point. Follow-up: [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing) | Owner decision, 2026-11-03 (closes [install UX open question 7](install-and-config-ux.md#6-open-questions)). apps-repo rules (AI disclosure, human development and review) from the [apps-repo README](https://github.com/webosbrew/apps-repo) |
| P15 | **License: GPL-3.0-or-later.** `LICENSE` holds the verbatim GPLv3 text. `license = "GPL-3.0-or-later"` is set once in `[workspace.package]` and inherited by every crate. The ipk control file carries `License: GPL-3.0-or-later` (the opkg convention; `check` verifies it) and the app directory ships `LICENSE`, so the text travels with the binaries. `appinfo.json` has no license field in LG's format and the HBC manifest schema has none either (its properties are id, version, type, title, appDescription, iconUri, sourceUrl, rootRequired, ipkUrl, ipkHash), so nothing is invented there: the store derives "open source" from the `pool: main` entry in `apps-repo` and `sourceUrl`. Dependency audit and flags: [section 8](#8-license-decision-and-dependency-audit) | Owner decision, 2026-11-03 | GPLv3 text from gnu.org; webosbrew/docs `HomebrewPackageManifest.schema.json`; apps-repo README (`pool`) |
| P16 | **No hard firmware whitelist in the public release; a verified-models list plus a runtime "verified / not verified" status instead.** Documented now, implemented at the Homebrew Channel release. **Today's gate stays unchanged until then.** Design: [section 9](#9-verified-models-list-instead-of-a-firmware-whitelist) | Owner decision, 2026-11-03 | [Section 9](#9-verified-models-list-instead-of-a-firmware-whitelist) |

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

**Not built yet (stubbed with a "not available yet" note):** the pairing window state and countdown (the daemon has no status output for it; [install UX 4.1](install-and-config-ux.md#41-phase-3-slice-with-autostart-owner-approval-gated-on-the-tv)), power options other than the remote power-off setting ([always-on §7](always-on.md#7-exposing-power-options-in-the-ui-design)), and settings such as microphone gain (planned as HA config entities). Since 0.1.0 the app shows the reply volume and the remote power-off setting (both set in Home Assistant and stored by the daemon in `etc/reply_volume` and `etc/remote_off`), with a one-line risk note when "keep listening" is on. Translations and screen-reader review are not done.

## 5. Verified on the host (2026-11-03)

| Test | What it covers |
| --- | --- |
| `packaging/webos/tests/ipk.sh` (42 checks) | Builds ipks with the host build standing in for both TV builds; structure check; byte-identical rebuild; manifest fields and hash; invalid id and version refused. Unpacks them as appinstalld lays them out and drives `ctl.sh`: status JSON before install; first install (generated config with random name and MAC seed, mode 0600, valid, no hook, not started); `--config` and `--psk-file`; one supervised daemon after start and after a second start; no key in the status output; kill switch; hook (0755, prefix filled in, returns at once without output, launches `autostart.sh` detached); update while running (new pid, old one gone, one daemon and one supervisor, new binary, config and key unchanged, hook refreshed); update at boot by `autostart.sh`; two concurrent starts give one daemon; a stale lock is taken over; app removed: neither `autostart.sh` nor the hook starts anything; uninstall keeps config, key and logs; reinstall keeps the identity; purge removes everything and refuses a directory that is not an install; two fresh installs get different identities; refused without root |
| `packaging/webos/tests/run.sh` | The existing ops-script checks (supervisor, crash loop, autostart gating, build selection), unchanged and passing |
| `packaging/webos/tests/app_smoke.js` | The app with a fake DOM and `PalmServiceBridge`: rendering per state, Set up runs install then start, a pending update is applied once on open, confirm-twice for purge, the not-root screen, only whitelisted commands and only the HBC exec and connection manager services are called |
| BusyBox | `ipk.sh` and `run.sh` also pass with BusyBox 1.37 applets first on `PATH` (the TV's `sh` is BusyBox ash; c3-a's applet set may differ) |
| Real builds | The `make ipk` package unpacked on the PC with `qemu-user` binfmt: `ctl.sh install` chose the aarch64 build for `uname -m` aarch64 and armv7 for armv7l, the generated config passed `--check-config`, status and purge worked |

Also: ShellCheck clean, `sh -n` of all scripts, `node --check` of the app.

## 6. Verified on a TV (c3-b, 2026-11-03)

[EXP-009](../experiments/EXP-009-ipk-install-update.md), `verified-on-target` for c3-b, firmware 03.30.73, HBC 0.7.2: appinstalld accepts the package built without ares-cli and keeps file modes; the app launches and HBC `exec` runs `ctl.sh` as root from it; a daemon started through `exec` survives closing the app and HBC's idle exit; an app install does not touch the running daemon and opening the app applies the update (config and identity kept, one daemon); the boot hook runs after other apps' hooks, starts the daemon about 100 s after boot, applies a pending update first and honours the firmware guard, with no failsafe over two boots; appinstalld accepts a downgrade; after the app is removed the hook starts nothing; uninstall and purge leave nothing behind.

Bugs found only on the TV, all fixed with host tests:

1. **Inherited descriptors (6410eff).** HBC's `exec` hands its children the service's Luna bus sockets and the sockets it reads the output from. The daemon kept the Luna sockets, and `setsid` called through a shell function in the background left `start.sh` subshells holding the output sockets, so `exec` never answered and the rotator watched the wrong pid. `start.sh` now closes inherited descriptors (as HBC's `startup.sh` does) and runs `setsid` as a plain command.
2. **Stop race under `set -e` (d749183).** The supervisor forwards its TERM to the daemon, which can be gone before `stop_daemon`'s own `kill`; the failing `kill` aborted `ctl.sh install` half way with the daemon stopped. Every kill there now tolerates a missing process.
3. **Two update triggers (8959a39).** An app instance left in the background and the relaunched app both applied one update; the second stopped the daemon the first had started. `ctl.sh install` now does nothing when the payload is already installed (`--force` reinstalls).

On HBC 0.6.3 (c3-a) install, `ctl.sh` set up with the existing HA identity, start and the hook were verified ([P4](../discovery/c3-a/P4-permanent-install.md)); the app UI and a boot there are not. Still not verified: installing through HBC's own `install` from a manifest, survival across an HBC self-update or a luna hub restart.

## 7. Open questions

1. App title before the first publication (P4 settles the app id). The repository license is decided: GPL-3.0-or-later (P15).
2. ~~Does appinstalld accept a downgrade?~~ **Yes on c3-b** (EXP-009, 0.0.4 to 0.0.2). Other firmware unverified; the fallback stays "remove the app (data kept), install the older ipk, `ctl.sh install`".
3. A process started through HBC `exec` survives the app closing and HBC's idle exit (c3-b, EXP-009). Still open: does it stay alive when HBC's service is killed (HBC self-update kills app services with SIGKILL on webOS 4+, per its source comment) or when the luna hub restarts ([always-on](always-on.md#2-what-was-observed-on-c3-a-read-only-2026-11-03))?
4. Superseded by P16 for the public release: no allow-list override at install; the status is "verified / not verified" and the gate becomes a warning ([section 9](#9-verified-models-list-instead-of-a-firmware-whitelist)). Until then the owner edits `validated_firmware` by hand after validating capture on that model.
5. Publication path: decided 2026-11-03, the official `apps-repo` (P14). Still open: the AI-use disclosure wording, which is part of the [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing) step.

## 8. License decision and dependency audit

**Decision (owner, 2026-11-03): GPL-3.0-or-later** for this repository (P15). What was done: `LICENSE` (verbatim, from <https://www.gnu.org/licenses/gpl-3.0.txt>, sha256 `3972dc97...6986`), `license` in `[workspace.package]` inherited by all five crates, `License:` in the ipk control file, `LICENSE` in the app directory, and a `check` rule for both. The vendored ESPHome `.proto` files stay MIT under their own notice (`crates/esphome-api/proto/LICENSE-ESPHOME-MIT`); MIT is GPLv3-compatible.

### 8.1 Rust dependencies (audited 2026-11-03, `cargo metadata --format-version 1 --locked`, 180 external packages, plus `cargo deny check licenses` with a scratch config outside the repo; the whole lock file, all targets)

| License expression (as published) | Packages | GPL-3.0-or-later compatible | Note |
| --- | --- | --- | --- |
| `MIT OR Apache-2.0` and spelling variants (`Apache-2.0 OR MIT`, `MIT/Apache-2.0`, `Apache-2.0/MIT`, `Apache-2.0 / MIT`) | 117 | yes | Apache-2.0 is compatible with GPLv3 (not with GPLv2-only; irrelevant here) |
| `MIT` | 17 | yes | |
| `BSD-3-Clause` | 10 (sonora family, nnnoiseless, curve25519-dalek, subtle) | yes | sonora also has the WebRTC patent grant ([aec-candidates](../research/aec-candidates.md)) |
| `MPL-2.0` | 10 (the `symphonia` family) | yes | MPL-2.0 section 3.3 allows combining with GPL code; its file-level copyleft applies to the unmodified crates only. Keep the crate sources available (crates.io) and name them in the release notes |
| `Apache-2.0` | 6 (miette, prost family) | yes | |
| `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | 5 (linux-raw-sys, rustix, wasi, wasip2, wit-bindgen) | yes | |
| `Unlicense OR MIT`, `Zlib OR Apache-2.0 OR MIT`, `Zlib`, `ISC`, `MIT OR BSD-3-Clause`, `Apache-2.0 OR ISC OR MIT`, `BlueOak-1.0.0 OR MIT OR Apache-2.0`, `MIT OR Apache-2.0 OR BSD-1-Clause`, `(MIT OR Apache-2.0) AND Unicode-3.0` | 12 | yes | choose the MIT or Apache option where there is a choice |
| `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | 1 (`r-efi`) | yes | choose MIT or Apache-2.0; the LGPL option is not needed (and is a UEFI crate, not in the TV build) |
| `Apache-2.0 AND ISC` | 1 (`ring` 0.17.14) | yes, **check on release** | Both terms apply. Its `LICENSE` files list ISC, Apache-2.0 (BoringSSL parts) and once_cell (MIT/Apache-2.0); none of them is the OpenSSL advertising-clause license that GPLv3 rejects. Pulled in by `rustls` through `ureq` (`webos-playback`), so it is in the TV binary. Confirm the file-by-file notices when the release notice file is generated |
| `CDLA-Permissive-2.0` | 1 (`webpki-roots` 1.0.11, the Mozilla CA bundle as data) | yes (permissive, attribution) | **Not on the FSF list, and `cargo deny` rejects it unless allowed explicitly.** It is a permissive data license with no copyleft; judged compatible, but unclear to a strict reviewer. In the TV binary through `ureq`. Flag for the owner: either accept it, or build `ureq` against the TV's own CA store instead |

No GPL-2.0-only, proprietary, missing or unknown licenses. Nothing is incompatible. Flags for the owner:

1. `webpki-roots` (`CDLA-Permissive-2.0`): unusual license, judged fine.
2. `ring`: confirm the notices at release time.
3. **Binary distribution needs a third-party notice file.** MIT, BSD, ISC and Apache-2.0 require that the copyright and license texts of the dependencies are reproduced with the binary. The ipk does not ship one yet. Before the release, generate one (for example with `cargo about`, which is not installed here and was not added) and include it next to `LICENSE`. Release prerequisite in [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing).
4. The proto files' MIT notice must also stay in the source tree (it does).

### 8.2 Python tools and the status app

The Python tools are development and test aids, **not shipped in the ipk and not part of the TV runtime**; `packaging/webos/ipk/build_ipk.py` is standard library only. Third-party modules they import, with the licenses read from the installed metadata (the local `.venv`) or PyPI on 2026-11-03: `aioesphomeapi` (MIT, pinned in `tools/esphome-fake-ha/requirements.txt`), `aiohttp` 3.14.5 (Apache-2.0 AND MIT, same file), `numpy` (BSD-3-Clause and permissive parts), `scipy` (BSD-3-Clause), `matplotlib` (matplotlib license, BSD-style), `soundfile` (BSD-3-Clause), `wyoming` (MIT), `onnxruntime` (MIT), `faster-whisper` (MIT), `pyopen-wakeword` (Apache-2.0), `openwakeword` (Apache-2.0 code), `piper-tts` 1.8.0 (**GPL-3.0-or-later**, fine: the same license, and it is only run on the PC to synthesize test speech). Flag: upstream publishes the pre-trained openWakeWord models under a more restrictive (non-commercial) license than its code (recalled from its README, not re-checked in this audit), which this repository neither ships nor depends on at runtime (no model files are tracked); keep it that way, and check the model card before anyone bundles a model. The test-content clips and voices have their own licenses, recorded in the audio-rig manifest ([tools/audio-rig/README.md](../../tools/audio-rig/README.md)); none is committed.

The status app (`packaging/webos/app/`) is plain HTML, CSS and JavaScript with **no JS dependencies** (no `package.json`, no bundled library). `packaging/webos/tests/app_smoke.js` uses Node's standard library only.

### 8.3 CI

There is no license check in CI. The `cargo deny` run above used a scratch config outside the repository and found exactly one expression outside the common permissive set (`CDLA-Permissive-2.0`). `cargo-deny` is installed locally but no `deny.toml` was added, because the owner did not ask for new tooling; adding one, with the allow-list from the table above, is a small follow-up if wanted.

## 9. Verified-models list instead of a firmware whitelist

**Update (owner, 2026-11-04): this section is refined and partly superseded by [firmware policy and reports](firmware-policy-and-reports.md) ([ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md)).** The hard gate is **not** kept until the release: the mic is to start on any firmware, with a visible status, measured self-checks, a firmware-changed marker, an easy compatibility report and an opt-in strict mode. In 9.4 the open point on closing the mic after a failed self-check is answered (a measured failure closes it with bounded retries; a warning does not), the assumed entity "attributes" do not exist in the ESPHome API (separate entities instead), and the confirmation before the first capture is not required. **None of this is implemented yet**; the text below describes the state until step S1 of that design lands.

**Decision (owner, 2026-11-03), P16.** The public release will not hard-whitelist firmware. This section is the design; **nothing here is implemented, and the daemon behaves as before: it refuses to open the microphone unless the TV firmware is in `safety.validated_firmware`** ([config](../../config/example.toml), `crates/satellite/src/firmware.rs`, roadmap Phase 3 "Firmware guard", c3-a evidence). The gate stays until the Homebrew Channel release ([backlog #10](../project/backlog.md#10-official-homebrew-channel-listing)).

### 9.1 What the gate protects today

LG can update the firmware silently, and with it the audio stack (ALSA card names and indexes, the capture ioctls, the DAPM and sysfs behaviour behind the [oops hazard](../discovery/c3-a/rooted-tv-improvements.md)). Capture is validated only on c3-a firmware 03.30.74 (and c3-b is being validated). The gate turns "the mic path was never tested on this firmware" into "the mic stays off until the owner has looked". A bare "not verified" label would drop that safety net, so the release replaces it with a softer form, it does not just remove it.

### 9.2 Published list

- A **Verified TVs** table: model (exact), webOS version, firmware, architecture, what passed (capture, playback, wake roundtrip), the date, the experiment or report, and the power states covered. Kept in [docs/compatibility/](../compatibility/README.md) (one row per model and firmware, the per-device record as today) and linked from the README. Only rows in this table are announced as supported; this follows the existing rule that no model is marked supported without device evidence.
- Everything else is "not verified": it may work, and the user is asked to report.

### 9.3 Compatibility report (to be implemented)

`ctl.sh report` and a button in the status app produce one sanitized block of text:

- model name, webOS version, firmware (`webos_manufacturing_version`), kernel and userland architecture;
- the ALSA PCM summary (card names, capture and playback device lists, the chosen capture device and its format);
- results of the built-in checks: capture opened and delivered non-silent frames, playback played the test tone, HA connection and pairing state, build version;
- the satellite version.

It must **not** contain a serial number, MAC address, IP address, hostname, TV name, pairing key, Wi-Fi name or HA address. Implementation rule: build the report from an allow-list of fields, never by filtering a raw dump, and cover it with a test that seeds fake identifiers and asserts none appears. The report is shown to the user before they copy it; nothing is sent anywhere by the app.

A GitHub issue template, **TV compatibility report**, takes this block (model, firmware and the pass/fail lines as fields, the privacy reminder at the top). A maintainer adds the verified row after reading it; a report alone is `observed-on-other-device` until a maintainer or a second report confirms it.

### 9.4 Runtime behaviour at the release (replaces the block)

| Situation | Today | At the release |
| --- | --- | --- |
| Firmware is on the verified list | mic opens | mic opens; status "verified" |
| Firmware not on the list | mic stays off | mic opens (the user installed it on purpose); status "not verified" in the app, a log line at start, and an HA diagnostic entity (for example a text or binary sensor `verification`: `verified` / `not verified`, with model and firmware as attributes) |
| Firmware changed since install | mic stays off (firmware not in the list) | **warning, not a block**: the daemon stores the firmware seen at install (and the last firmware it ran on) in its state directory; when `os_info.json` differs it logs a `WARN firmware changed: <old> -> <new>`, shows a banner in the status app, and the HA entity reports `firmware changed` until the user acknowledges it (a button entity, or `ctl.sh ack-firmware`). It also reruns the self-checks that cost nothing (capture opens and delivers frames, playback tone) and puts the result in the same entity |
| Firmware cannot be read | mic stays off | status "unknown"; the mic opens; log and entity say so |
| Strict mode wanted | (default) | keep `safety.validated_firmware` as an **optional** list: if it is non-empty it works as today (mic off on another firmware). It becomes empty by default; owners who want the hard gate set it. Decide at the release whether the default list is empty or holds the verified firmware versions |

Open points for that release: whether a failed self-check after a firmware change should close the mic (recommended: yes, with the reason shown, because it is a measured failure rather than an unknown); whether the first capture on an unverified firmware should wait for one explicit confirmation in the app (this reopens [install UX question 5](install-and-config-ux.md#6-open-questions), which this decision answers as "no blocking override, a visible status"); how the DAPM oops hazard on a different audio stack is surfaced, since a warning cannot prevent it (the daemon never touches the ASoC sysfs files, so the hazard comes from probes and tooling, not from capture).

### 9.5 Release prerequisites

Recorded in [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing): the verified-models table with at least the tested TVs, `ctl.sh report` and the app button, the issue template, the status and the HA entity, the firmware-changed warning, and the README "Verified TVs" section. Until all of them exist the current gate is the protection.

## Sources

Read 2026-11-03:

- [webos-homebrew-channel](https://github.com/webosbrew/webos-homebrew-channel) at tag v0.7.3 (release 2026-01-11): README (install with `luna-send-pub ... appInstallService/dev/install`, `exec`, `spawn`, init.d hooks), `tools/install.sh`, `services/service.ts` (`install`, `uninstall` via `dev/remove`, `getConfiguration` `homebrewBaseDir`, `exec`, self-update), `services/startup.sh`, `services/elevate-service.ts`, `tools/gen-manifest.js`, CHANGELOG.
- [TBSniller/piccap](https://github.com/TBSniller/piccap) at a243d0e: `frontend/js/servicecalls.js`, `servicefiles/`, `package.json`, `frontend/webOSTVjs-1.2.4/webOSTV-dev.js`; [webosbrew/hyperion-webos](https://github.com/webosbrew/hyperion-webos) at 00c932f: `src/service.c` (autostart symlink), `src/settings.h` (config path).
- [Simon34545/lginputhook](https://github.com/Simon34545/lginputhook) at a607900: `service/service.js` (init.d file, config under `/home/root/.config`), `buildmanifest.js`, `manifest.json`.
- [webos-tools/cli](https://github.com/webos-tools/cli) (ares-cli) at ed7a699: `lib/package.js`, `files/schema/ApplicationDescription.schema`.
- [webosbrew/docs](https://github.com/webosbrew/docs) at 555a3a0: `schemas/HomebrewPackageManifest.schema.json`.
- [RootMyTV](https://github.com/RootMyTV/RootMyTV.github.io) `files/stage3.sh` (init.d naming and exit-status rules).
- LG: [appinfo.json reference](https://webostv.developer.lge.com/develop/references/appinfo-json), [connection manager](https://webostv.developer.lge.com/develop/references/connection-manager).
