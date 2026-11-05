# Device: c3-a

**Model:** LG C3 42 inch
**webOS:** webOS TV 8.3.0
**Firmware:** `03.30.74` (on the daemon's allow-list)
**Kernel and userland architecture:** aarch64 kernel, 32-bit ARM EABI5 userland (glibc 2.31); both static builds run (the aarch64 daemon since 2026-11-03)
**Source:** consented probes and live runs on 2026-11-01 to 2026-11-04, rooted with the Homebrew Channel **0.6.3**, Always Ready on, Quick Start+ on, LG hands-free off. Details: [D0-D1](../discovery/c3-a/D0-D1-runtime.md), [D3](../discovery/c3-a/D3-mic-identification.md), [P1 HA integration](../discovery/c3-a/P1-ha-integration.md), [P4 permanent install](../discovery/c3-a/P4-permanent-install.md). Scope: this unit and firmware only; c3-b is a different unit and firmware.

This file starts with the install record. Earlier c3-a results (capture, playback, wake word, AEC, soak) are in the discovery notes above and are not repeated per row.

| Capability | Runtime/media state | Result | Experiment |
| --- | --- | --- | --- |
| App package 0.1.0 install with appinstalld `dev/install` (HBC 0.6.3) | Active | PASS (app registered under `/media/developer/apps`) | P4 |
| `ctl.sh install` with an existing identity (name, MAC seed, key) | Active | PASS (the permanent HA entry reconnected 6 s after the start, no entry change) | P4 |
| aarch64 daemon: capture `hw:LGSIC,0`, Noise to HA, power watch | Active, wake mode, LG hands-free off | PASS (running; resources in P4); no kernel warning | P4 |
| Boot hook written (`ctl.sh autostart on`), HBC 0.6.3 runs `init.d` with `run-parts` | Active | PASS (hook written, `startup.sh` read); **a boot was not made** | P4 |
| Start at boot after a real restart | Reboot | NOT TESTED (owner's next natural restart; passed on c3-b with HBC 0.7.2, EXP-009) | P4 |
| App UI through HBC `exec` | Active | NOT TESTED (not opened on the owner's screen) | P4 |
| Reply playback, announcements, "Keep listening (screen off)", resume after suspend with 0.1.0 | any | NOT TESTED on this unit (no sound, no power-state change allowed in the session); c3-b only | P4 |
| TV control 0.2.0: entity list and states in the owner's HA (10 entities, `tv_power` not listed), match with the luna getters | Active | PASS (2026-11-04; volume 89, mute off, `tv_speaker`, input `HDMI 1`, app `Other`, `Active`) | [EXP-010 Part B](../experiments/EXP-010-tv-control-setters.md#part-b-results-2026-11-04) |
| TV control 0.2.0: mute on and off, volume down one step (restored), `volume up` refusal above `volume_limit`, refresh button, re-listing after a daemon restart | Active | PASS (the TV's volume 89 is above the default `volume_limit` 40, so HA can only lower it) | EXP-010 Part B |
| TV control 0.2.0: resources over 10 min with three luna children | Active | 1.50 % of one core (1.63 % with the poll's child processes), RSS 1.4 to 1.7 MB; no kernel warning | EXP-010 Part B, [P4](../discovery/c3-a/P4-permanent-install.md#update-to-020-2026-11-04-tv-control) |
| Always Ready setting through luna (EXP-011): `settingsservice/getSystemSettings` and `setSystemSettings` on `general/lifeOnScreenMode`; allowed values from `getSystemSettingDesc` | Active | PASS (2026-11-05; `alwaysReady` -> `off` -> `alwaysReady`, each read back at once, `returnValue true`, `aod/getStatus` stays `none`); values `none` (hidden), `off`, `lifeOnScreen`, `alwaysReady`, `allEnabled`; a change in LG's own menu not tested | [EXP-011](../experiments/EXP-011-lg-settings-toggle.md) |
| LG hands-free through luna (EXP-011): `voiceinput/setting/getActivation {}` and `setActivation {"settingStatus":bool}` (no provider parameter; changes `thinQ`) | Active, satellite holding the PCM | PASS (2026-11-05; on for 193 s, read back, off again); no `MIC BUSY`, PCM owner unchanged, HA Listening stayed on; started with the satellite already holding the microphone, the reverse order and an on-screen prompt not tested | [EXP-011](../experiments/EXP-011-lg-settings-toggle.md) |
| `ctl.sh tv-settings --json` and `set always-ready|hi-lg|remote-off` with 0.3.0, plain SSH and through HBC `exec` (no tty) | Active, wake mode, satellite holding the PCM | PASS (2026-11-05; both LG calls answer over a **plain pipe**, no pty needed; 0.22 s over SSH, 0.78 s through `exec` (3.5 s first call); every write `applied` and restored: Always Ready on -> off -> on, Hi LG off -> on -> off, remote off normal -> keep -> normal; HA Listening stayed on) | [P4 0.3.0](../discovery/c3-a/P4-permanent-install.md#update-to-030-2026-11-05-voice-and-power-screen) |
| HA diagnostic sensors "Always Ready" and "LG hands-free" with 0.3.0 | Active | PASS (`On` and `Off`, follow a change in about 1 s through the refresh trigger; 38 entities, existing ones unchanged) | P4 0.3.0 |
| Reload of the "when the TV is turned off with the remote" mode through the daemon's trigger, `run/voice.json` | Active | PASS (HA select follows within the call, about 1 s; voice.json follows); no power key pressed | P4 0.3.0 |
| python3 on the TV: `pty` module | n/a | **absent** (python 3.8.13); `os.openpty` works; the `ctl.sh` fallback now uses it (`4d61619`) | P4 0.3.0 |
| Resources 0.3.0 (342 s, `Active`) | Active | 1.62 % of one core, RSS 1.7 MB (0.2.3: 1.54 %); no kernel warning | P4 0.3.0 |
| TV app 0.4.0 redesign in package 0.3.1 (no phone setup server): `status --json` `ha` and `pairing`, `tv-settings --json`, `setup start` refusal, through HBC `exec` | Screen Saver | PASS (2026-11-05; `ha.state` connected, `pairing.open` false; tv-settings values unchanged; `setup start` returns `the setup server is missing`, no process or listener left); **the app was not opened** | [P4 0.3.1](../discovery/c3-a/P4-permanent-install.md#update-to-031-2026-11-05-tv-app-040-redesign-no-phone-setup-server) |
| Reply-volume reload (`ctl.sh set reply-volume`) with 0.3.1 | Screen Saver | PASS (40 to 45 to 40; HA satellite media player followed in about 0.6 s each time; `applied`; no sound) | P4 0.3.1 |
| Resources 0.3.1 (317 s and a quiet 180 s window, `Screen Saver`) | Screen Saver | 2.2 % of one core (0.3.0 in `Active`: 1.62 %), RSS 1.7 to 1.8 MB; cause of the difference not established; no kernel warning | P4 0.3.1 |
| Package 0.4.0 (release candidate with the phone setup server): update over the running daemon, state kept | Active | PASS (2026-11-05; config and key hashes equal, HA reconnected 3.5 s after the start, 38 entities unchanged, 0 WARN or ERROR, no kernel warning, `tainted` 4097 unchanged) | P4 0.4.0 |
| Resources 0.4.0 (311.6 s, `Active`) | Active | **1.59 % of one core**, RSS 1.8 MB (0.3.0 `Active`: 1.62 %; 0.3.1 `Screen Saver`: 2.2 % with the same daemon code, so the difference was the power state, not the code); no kernel warning | P4 0.4.0 |
| Phone setup server started through HBC `exec` (`ctl.sh setup start`, EXP-012 Part B TV side) | Active | PASS (2026-11-05; 3.2 s cold, 1.5 s warm; `setup.pid` is the server's own pid; environment is only `PATH`, `WEBOS_SAT_SETUP_DIR`, `WEBOS_SAT_PREFIX`; one `0.0.0.0` IPv4 listener; `/tmp` is `1777` root; dir 0700, files 0600; no token in `/proc/*/cmdline`, `/proc/*/environ`, the logs or `ps`); a phone and the app NOT TESTED | [EXP-012](../experiments/EXP-012-phone-setup-probe.md), P4 0.4.0 |
| `setup status --json` as the heartbeat through `exec`, every 2 s | Active | PASS (110 polls in two sessions, 0 failures, 50 / 60 / 90 ms, the heartbeat mtime moves on every call, no session ended early); the server idles at 0.1 to 0.2 % of a core and 424 to 448 kB RSS; the HBC node process 0.64 % | P4 0.4.0 |
| `setup stop` through `exec` | Active | PASS (0.30 s; listener closed, directory removed, no process, `/tmp` clean) | P4 0.4.0 |
| Setup server HTTP from a same-LAN PC: token and cookie, `410` on reuse, `403` without cookie, CSRF, `Host` and `Origin` refusals, `/api/state` fields | Active | PASS (2026-11-05; see the P4 0.4.0 table); no action was posted | P4 0.4.0 |
| State refresh only while the page polls (SR-03) | Active | PASS (a refresh about every 5 s while polled; none during 30 s of silence; resumes on a poll) | P4 0.4.0 |
| Phone scan, the confirmation overlay with the real remote, any action through the setup page, a phone on Wi-Fi reaching the TV | Active | **PENDING OWNER** (steps in EXP-012 Part B) | [EXP-012](../experiments/EXP-012-phone-setup-probe.md) |
| Phone setup probe (EXP-012 Part A): host firewall | Active | **none** (2026-11-05; `iptables` filter, nat and mangle all policy ACCEPT with no rules; the kernel has no ip6 tables, no `nft`); nothing changed | [EXP-012](../experiments/EXP-012-phone-setup-probe.md) |
| Inbound TCP to a short-lived listener on the TV (`0.0.0.0` and `::`) | Active, TV on Wi-Fi | PASS from one wired PC on the same LAN /24 (2026-11-05; random ports in 49152-65535 x3, 38211, 8765 over IPv4; two high ports over global IPv6; 6053 control; a closed port was refused); a phone, another VLAN or guest network and other power states NOT TESTED | EXP-012 |
| Network facts: interfaces and route | Active | Wi-Fi `wlan0` is the only interface with IPv4 and the default route; `eth0` has no carrier, `p2p0` has no IPv4; ephemeral range 32768-60999; LG services already listen on `0.0.0.0` (22, 6053 and others) | EXP-012 |
| Tools for a fallback server: `ss`, `nft`, `tcpdump`, busybox `httpd`; python3 `http.server`, `socketserver`, `pty`, `resource` | n/a | **absent** (python3 3.8.13 imports `socket`, `select`, `selectors`, `json`, `ssl`, `threading`, `asyncio`, `subprocess`); present: `netstat`, `nc`, `ip`, `timeout`, `setsid`, `perl`, `curl`, `wget` | EXP-012 |
| App web engine (static evidence only, no app opened) | n/a | `hypothesis`: `libcbe.so` holds `94.0.4606.128` beside the app-shell code (and `Chrome/87.0.3945.79` for LG's Crow browser); ES2015+ and `fetch` expected; `navigator.userAgent`, key codes 13 and 461, timers while hidden NOT TESTED | EXP-012 |
| A detached `setsid` process started by HBC `exec` (`luna://org.webosbrew.hbchannel.service/exec`) | Active | PASS (2026-11-05; survives the call's return and the HBC service's idle exit, which came 20 to 30 s after the call; alive at 120 s, `ppid` 1, cgroup `ls-hubd.service`); app closed, HBC restart, `Screen Off` and standby NOT TESTED | EXP-012 |
| Polling through HBC `exec` once per 1 s and per 2 s (45 to 60 s runs) | Active | 0 failures in about 430 calls over three runs; per call `true` 40 ms, `ctl.sh --help` 80 ms (about 77 ms CPU over node, `ls-hubd` and `luna-send`: 7.7 % of one core at 1 Hz, 3.8 % at 0.5 Hz), **`ctl.sh status --json` 950 ms and about 0.57 core-seconds (28 % of a core at 0.5 Hz)**; satellite CPU unchanged (1.6 to 2.0 %), RSS 1.7 MB, no restart | EXP-012 |
| Exec `command` string or stdout kept in logs | Active | **not found** (2026-11-05; marker searched in 77 files under `/var/log`, `/var/luna`, `/tmp/pmlogdaemon`, plus `journalctl`, `logread`, `dmesg`; the HBC service start is logged, no exec call); memory-only buffers not searched | EXP-012 |
| Voice and power screen and the redesigned Home in the app, power-off cells, "Keep listening" with a real power key | any | NOT TESTED (owner's active PC display) | P4 0.3.0, P4 0.3.1 |
| TV control: screen off and on, input switch, app launch, power-off in "Keep listening", `Active Standby` states | any | NOT TESTED (the TV is the owner's active PC display); passed on c3-b | EXP-010 |

## Notes and redacted evidence

- The identity of the HA entry (device name, MAC seed, Noise key) is kept in the gitignored `devices/private/c3-a/`; the install used it so the entry did not change.
- The TV's load average was about 12 before and after the install (the TV's own software).
- `tainted` is `4097`, unchanged by the install.

## Firmware regressions and rollback

None seen. Rollback: `ctl.sh stop`, `ctl.sh autostart off`, `ctl.sh purge`, `make tv-remove-ipk` ([development](../development.md#install-the-app-package-on-a-rooted-tv-persistent)).
