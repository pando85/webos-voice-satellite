# Device: c3-b

**Model:** LG C3 55 inch (`OLED55C36LC`)
**webOS:** webOS TV 8.3.0 (release 8.3.0-1506)
**Firmware:** `03.30.73`
**Kernel and userland architecture:** 5.4.96-307 aarch64 kernel, 32-bit ARM EABI5 userland (glibc 2.31); both static builds run, the aarch64 daemon was run
**Source:** consented probe and a live run on 2026-11-03, rooted with the Homebrew Channel, Ethernet, sound out through ARC. Details: [D0 quick](../discovery/c3-b/D0-quick.md), [D5 results](../discovery/c3-b/D5-results.md). Scope: this unit and firmware only; c3-a's results are a different unit and firmware.

| Capability | Runtime/media state | Result | Experiment |
| --- | --- | --- | --- |
| Built-in mic capture (`hw:LGSIC,0`, S24_LE 2 ch 16 kHz) | Normal, LG hands-free off | PASS (aarch64 daemon, 81 min, no `MIC BUSY`) | EXP-002 |
| Capture, screen saver / screen off | `ScreenSaver`, `Screen Off` | PASS (5 min screen off) | EXP-004, D5 |
| Capture, remote power-off, Always Ready on | `Active Standby` | PASS (59 min 18 s, ended by a remote key; wake word chain ran 6 times) | EXP-004, D5 |
| Capture, remote power-off, Always Ready off | `Suspend` | FAIL by design: process frozen, network down 20 s after the key; resumes after WoL or remote | EXP-004, D5 |
| Reply playback (`pacat`, tone) | Active, screen saver, screen off | PASS (heard by the TV mic) | EXP-005, D5 |
| Reply playback | `Active Standby` (Always Ready on), ARC and TV speaker | **FAIL** (not audible, 5 checks) | EXP-005, D5 |
| Wake from standby by WoL | From `Suspend`, `wolwowlOnOff = true` | PASS (2 to 4 s); with the original `false`: INCONCLUSIVE; from `Active Standby`: INCONCLUSIVE | D5 |
| Assist pipeline (wake, STT, TTS stream) | Active Standby and Active | PASS (HA reached port 6053 directly) | EXP-007 |
| AirPlay audio | any | INCONCLUSIVE (not tested) | EXP-008 |
| Wake and darken after a remote power-off (watcher: `powerOn` with a reason, then `turnOffScreen`) | Always Ready on and off | PASS (7 of 7 runs, `Screen Off` 5.3 to 5.9 s after the key; panel dark and power draw not observed) | D6 |
| Reply playback after wake and darken; 20 min dwell in `Screen Off` | Always Ready off | PASS (beeps 28 of 28, an announcement audible, process and HA link stable, no suspend) | D6 |
| Wake for a reply from `Active Standby`, return to standby | Always Ready on | PASS (audible 2.75 to 3.77 s after the request) | D6 |
| App package (`.ipk`): install by appinstalld `dev/install`, HBC 0.7.2 | Active | PASS (built without ares-cli; modes and exec bits kept; `folderPath` under `/media/developer/apps`) | EXP-009 |
| App package: HBC `exec` as root from the app, daemon survives app close and HBC idle exit | Active | PASS (after fix 6410eff) | EXP-009 |
| App package: update over a running daemon, applied on app open | Active | PASS (config and identity kept, one daemon; after fixes d749183, 8959a39) | EXP-009 |
| App package: downgrade (older version over newer) | Active | PASS (accepted by appinstalld) | EXP-009 |
| Boot hook in `/var/lib/webosbrew/init.d` (opt-in), start after 60 s | Reboot (`micomPower`) | PASS (2 boots, no failsafe; update applied at boot; firmware guard honoured at boot) | EXP-009 |
| App removal, uninstall, purge | Active, 1 boot | PASS (inert hook after app removal; nothing left after purge) | EXP-009 |
| TV control getters as root through `luna-send`, no SSAP pairing: `getPowerState`, audio `master/getVolume` (`volume`, `muteStatus`, `maxVolume`, `soundOutput`), `apiadapter/audio/getSoundOutput`, `apiadapter/tv/getExternalInputList`, `applicationManager/getForegroundAppInfo`, `com.webos.media/getForegroundAppInfo` (`playState`), `applicationManager/listLaunchPoints` (about 35 KB) | TV running (exact power state not recorded here) | PASS (read-only, 2026-11-04) | [TV control design](../design/tv-control.md) |
| TV control setters: screen on (`turnOnScreen`), mute (`apiadapter/audio/setMute` and `audio/master/muteVolume`), volume set (`master/setVolume`), volume step (`volumeUp`, `volumeDown`), input switch (`apiadapter/tv/switchInput`, disconnected HDMI input), app launch (`applicationManager/launch`, a built-in silent app); volume, mute and launch also in `Screen Off` | `Active` and `Screen Off`, `soundOutput = external_arc`, nothing playing | PASS (2026-11-04, each read back and restored, latency 20 to 440 ms, no kernel warnings of note) | [EXP-010](../experiments/EXP-010-tv-control-setters.md) Part A |
| TV control setters: `turnOffScreen` from `Screen Saver` | `Screen Saver` | PASS (2026-11-04, twice: `Screen Off` accepted, `turnOnScreen` back to `Active`) | EXP-010 A1 |
| TV control 0.2.0 through an API client (daemon from `/tmp`, file microphone): entity list, `tv_screen` (also from `Screen Saver`), `tv_turn_on`, `tv_mute`, `tv_volume` and buttons, `volume_limit` clamp, `tv_input` (disconnected HDMI), `tv_app` (silent built-in), `Other` refused, external app change seen, daemon restart relists | `Screen Saver` and `Active`, `external_arc`, nothing playing | PASS (2026-11-04), every value restored | [EXP-010 Part B](../experiments/EXP-010-tv-control-setters.md#part-b-results-2026-11-04) |
| TV control 0.2.0 `tv_power` (power-off in "Keep listening"): refused with the select on "Turn off normally" and with the kill switch; off from `Active` twice, `Screen Off` at 9.96 s and 12.8 s (the second used the toggle-key fallback); `tv_power` on and `tv_turn_on` from `Screen Off`; TV entities missing during the transient `Active Standby` | `Active`, home app, Always Ready as the owner set it | PASS (the real microphone was not open: file microphone) | EXP-010 Part B |
| Permanent install of 0.2.0 (app package, HA entry with pairing, start at boot) on the household's TV, HBC 0.7.2 | `Screen Saver`; LG hands-free ("Hi LG") **on** | Install, pairing without a typed key (HA 2026.10.4, 11 s after the start), HA entry with 22 entities, states equal the luna getters, start at boot over one reboot (no failsafe, daemon about 100 s after boot, HA reconnected with the stored key, no re-authentication): PASS (2026-11-04). **Microphone capture: FAIL (busy)** while hands-free is on: `MIC BUSY`, `listening` off, the PCM held by another process; idle daemon 0.15 % CPU, 1.7 MB RSS | [P4 on c3-b](../discovery/c3-b/P4-permanent-install.md) |
| TV control: settled `Active Standby` (B11), power-off with content playing (B10), the TV speaker output (A5), a connected HDMI source, volume and mute with content | | NOT TESTED (B10, B11 and A5 not authorized or unsafe without remote access) | EXP-010 |

## Notes and redacted evidence

- The API key `networkinput/controls/power` and the IR remote's power key gave the same state sequences and the reason `remoteKey`. The IR key is a `KEY_POWER` event on `LGE RCU`; the API key appears on `Smart Remote RCU Input`.
- `suspend_stats/success` rises for each Suspend and does not rise in Always Ready's `Active Standby`.
- The firmware guard needed `validated_firmware = ["03.30.73"]`; the permanent install on 2026-11-04 lists `03.30.73` and `03.30.74` in this TV's own config (the package default stays c3-a's `03.30.74`). With LG hands-free on, capture is busy (see the P4 row); the capture rows above were measured with hands-free off. Since 2026-11-04 `03.30.73` is a row of the [verified list](verified-models.md) (series LG OLED C3) and the default policy no longer blocks the microphone on a firmware that is not listed (not yet run on this unit).
- The TV's sound output falls back to ARC after a power cycle even if set to `tv_speaker` earlier.
- One of three `reboot`s over SSH ended in `Active Standby` 53 s after boot (power-off reason `homekit` from `com.webos.service.qbd`); `networkinput/controls/power` brought it back to the home app (EXP-009).
- Other apps' boot hooks are present in `init.d` (four symlinks); the satellite's hook sorts after them.

## Firmware regressions and rollback

None seen. Every setting changed for the run (Always Ready, wake on LAN, sound output) was restored and read back.
