# Device: c3-a

**Model:** LG C3 42 inch
**webOS:** webOS TV 8.3.0
**Firmware:** `03.30.74` (on the daemon's allow-list)
**Kernel and userland architecture:** aarch64 kernel, 32-bit ARM EABI5 userland (glibc 2.31); both static builds run (the aarch64 daemon since 2026-11-03)
**Source:** consented probes and live runs on 2026-11-01 to 2026-11-03, rooted with the Homebrew Channel **0.6.3**, Always Ready on, Quick Start+ on, LG hands-free off. Details: [D0-D1](../discovery/c3-a/D0-D1-runtime.md), [D3](../discovery/c3-a/D3-mic-identification.md), [P1 HA integration](../discovery/c3-a/P1-ha-integration.md), [P4 permanent install](../discovery/c3-a/P4-permanent-install.md). Scope: this unit and firmware only; c3-b is a different unit and firmware.

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
| TV control getters and setters (volume, mute, input, apps) | any | NOT TESTED on this unit; getters passed on c3-b (`03.30.73`) only | [TV control design](../design/tv-control.md), [EXP-010](../experiments/EXP-010-tv-control-setters.md) |

## Notes and redacted evidence

- The identity of the HA entry (device name, MAC seed, Noise key) is kept in the gitignored `devices/private/c3-a/`; the install used it so the entry did not change.
- The TV's load average was about 12 before and after the install (the TV's own software).
- `tainted` is `4097`, unchanged by the install.

## Firmware regressions and rollback

None seen. Rollback: `ctl.sh stop`, `ctl.sh autostart off`, `ctl.sh purge`, `make tv-remove-ipk` ([development](../development.md#install-the-app-package-on-a-rooted-tv-persistent)).
