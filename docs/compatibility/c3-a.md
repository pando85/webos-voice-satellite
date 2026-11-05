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
| Voice and power screen in the app, power-off cells, "Keep listening" with a real power key | any | NOT TESTED (owner's active PC display) | P4 0.3.0 |
| TV control: screen off and on, input switch, app launch, power-off in "Keep listening", `Active Standby` states | any | NOT TESTED (the TV is the owner's active PC display); passed on c3-b | EXP-010 |

## Notes and redacted evidence

- The identity of the HA entry (device name, MAC seed, Noise key) is kept in the gitignored `devices/private/c3-a/`; the install used it so the entry did not change.
- The TV's load average was about 12 before and after the install (the TV's own software).
- `tainted` is `4097`, unchanged by the install.

## Firmware regressions and rollback

None seen. Rollback: `ctl.sh stop`, `ctl.sh autostart off`, `ctl.sh purge`, `make tv-remove-ipk` ([development](../development.md#install-the-app-package-on-a-rooted-tv-persistent)).
