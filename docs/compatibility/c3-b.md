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

## Notes and redacted evidence

- The API key `networkinput/controls/power` and the IR remote's power key gave the same state sequences and the reason `remoteKey`. The IR key is a `KEY_POWER` event on `LGE RCU`; the API key appears on `Smart Remote RCU Input`.
- `suspend_stats/success` rises for each Suspend and does not rise in Always Ready's `Active Standby`.
- The firmware guard needed `validated_firmware = ["03.30.73"]`; whether to validate it is an owner decision.
- The TV's sound output falls back to ARC after a power cycle even if set to `tv_speaker` earlier.

## Firmware regressions and rollback

None seen. Every setting changed for the run (Always Ready, wake on LAN, sound output) was restored and read back.
