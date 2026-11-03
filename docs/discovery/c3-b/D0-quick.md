# c3-b (LG C3, second unit): D0 quick discovery, compared with c3-a

- **Device:** c3-b, an LG C3 (55 inch, model `OLED55C36LC`), webOS TV 8.3.0, firmware `03.30.73`. c3-a is a 42 inch `OLED42C34LA` on firmware `03.30.74` ([D0-D1](../c3-a/D0-D1-runtime.md), [D2](../c3-a/D2-audio-stack.md), [D3](../c3-a/D3-mic-identification.md)). Findings for one unit are not confirmation for the other.
- **Date:** 2026-11-03. Rooted with the Homebrew Channel (HBC), in the owner's hall, in daily use.
- **Method:** read-only shell inspection over SSH (`/proc`, `/sys/power`, `/proc/asound`, getters through `luna-send` under `ssh -tt`), then the Phase 1 daemon from `/tmp` ([D5 results](D5-results.md)). Raw output is in the gitignored `devices/private/c3-b/`. No address, MAC, serial or hostname is recorded here.
- **Why this unit:** the owner's key question ("when I press power off on the remote, does the satellite keep listening and can it play replies?") needs a TV whose power behaviour can be tested without taking away the owner's PC display (c3-a).
- **Labels:** `verified-on-target` (seen on c3-b, with the date above), `hypothesis`, `inferred`.

## Verdict

c3-b matches c3-a where it matters for the satellite: the microphone PCM and its parameters, the PulseAudio sinks, the 32-bit userland on an aarch64 kernel. Firmware and image differ by one build. Three settings differ in ways that change the power tests: **Always Ready is off** (`lifeOnScreenMode = off`), **wake on LAN is off** (`wolwowlOnOff = false`) and the sound goes **out through ARC to an external device**, not the TV speaker. The run continued.

## Facts and differences

| Item | c3-b (this run) | c3-a | Label |
| --- | --- | --- | --- |
| Model, panel | `OLED55C36LC`, 55 inch | `OLED42C34LA`, 42 inch | verified-on-target |
| webOS, build, firmware | webOS TV 8.3.0, release `8.3.0-1506`, firmware `03.30.73`, build 2024-02-14 | release `8.3.0-1508`, firmware `03.30.74`, build 2024-02-26 | verified-on-target |
| Image name | `lib32-starfish-global-secured` | `lib32-starfish-global-flash` | verified-on-target |
| Kernel | `5.4.96-307` aarch64 PREEMPT, 3 of 4 cores online (`0-2`), Cortex-A76 | same | verified-on-target |
| Userland of `/bin/sh` | 32-bit ARM EABI5, soft-float flags `0x05000200`, loader `/lib/ld-linux.so.3` (glibc 2.31 family; `ld-2.31.so`); no `/lib64` | same | verified-on-target |
| Root method | HBC with `dropbear` from its service directory; `webosbrew_block_updates`, `webosbrew_sshd_enabled` and `webosbrew_telnet_disabled` flags present; `/tmp/webosbrew_startup` present | same | verified-on-target |
| `dropbear` at boot | Yes: the master process (parent pid 1) started about 24 s after boot | same | verified-on-target |
| Other autostart hooks | `/var/lib/webosbrew/init.d` holds four symlinks to other homebrew apps' hooks (a screen saver, a patcher, an ambient-light loader, a picture-capture tool). `init.d` is empty on c3-a. HyperHDR and PicCap were not running at the probe (not checked for the others) | empty | verified-on-target |
| Memory | 2 GB, about 408 MB available at the probe (c3-a: about 750 MB) | | verified-on-target |
| `/tmp` | tmpfs, about 1 GB, 997 MB free | same | verified-on-target |
| ALSA cards | `lg115x` (index 0) and `LGSIC` (index 1), the same PCM list: playback 0 to 9, capture 10 to 14 on `lg115x`; `WoV PDM Mic`, `SpeakerFeedback`, `MixerCapture`, `SoundEngineCapture`, `BluetoothCapture` on `LGSIC` | same | verified-on-target |
| Microphone PCM | `hw:LGSIC,0` "WoV PDM Mic": the satellite opened it and the kernel reported `RW_INTERLEAVED`, `S24_LE`, 2 channels, 16000 Hz, period 256, buffer 8192 (read from `/proc/asound/LGSIC/pcm0c/sub0/hw_params` while the daemon held it). `pcm0c/info` shows `subdevices_count: 1` and does not list capabilities, so the parameters come from an open, as on c3-a | same parameters | verified-on-target |
| PulseAudio | System mode; `pcm_output` and `ptts` are ALSA sinks, the others null sinks (the same list as c3-a); all `SUSPENDED` when idle; `pacat` to the default sink works | same | verified-on-target |
| LG hands-free / voice | **Off**: `voiceinput/setting/getActivation` reports `settingStatus: false` and `activationState: false` for ThinQ, Amazon and a third provider (`dangbei`). `option/turnOnByVoice = off`, `option/wakeUpword = LGTV`. Nothing was changed | off | verified-on-target |
| Capture PCM holder at start | none | none | verified-on-target |
| Sound output | **`external_arc`** (an ARC/eARC device through HDMI-CEC), `externalDeviceControl: true`, volume **12**, `maxVolume` 60, `volumeLimiter: familyCare`, not muted | `tv_speaker`, volume 89 at one reading, later 6 | verified-on-target |
| Network | **Ethernet** (`eth0` up, `wlan0` down); the PC is on the same L2 segment | Wi-Fi (last power-on reason `wakeOnWiFi`) | verified-on-target |
| Power state at start | `ScreenSaver` (screen saver on the home app, `Active` after a screen-on) | `Active` | verified-on-target |
| `getPowerOnReason` at start | `remoteKey` | `wakeOnWiFi` | verified-on-target |
| Kernel suspends since boot (`/sys/power/suspend_stats/success`) | **9** after 9 h of uptime (no fail): this TV suspends in daily use | 0 | verified-on-target |
| Power services | `tvpowerd`, `alwaysready` and `sleepd` active | same | verified-on-target |
| `com.webos.service.config` `alwaysready.supportedLowPowerMode` | not present (`missingConfigs`); c3-a returned `all` | `all` | verified-on-target |

## Settings read at the start (restored at the end of the run)

Read with `settingsservice/getSystemSettings`; the originals are kept in the private folder.

| Setting | c3-b | c3-a (2026-11-03) |
| --- | --- | --- |
| `option/quickStartMode` | `on` | `on` |
| `general/lifeOnScreenMode` (Always Ready) | **`off`** | `alwaysReady` |
| `network/wolwowlOnOff` | **`false`** | `true` |
| `option/turnOnByVoice` | `off` | `off` |
| `option/screenOff`, `screenOffTime` | `off`, `5` | `off`, `5` |
| `option/standByLight` | `on` | `on` |
| `option/lifeOnScreenUsingMotionSensor`, `OnTimer`, `EnergySaving`, `Notification`, `motionSensorSensitivityForAOD` | `false`, `[]`, `auto`, `true`, `medium` | same |
| `general/screenSaverEnabled` | `on` | `on` |
| `time/noSignalAutoOff`, `autoOff15Min` | `on`, `on` | `on`, `on` |
| `time/noOperationAutoOff` | `off` | `off` |
| `time/autoOff2HourOnTimer` | **`on`** | `off` |
| `time/automaticStandBy` | **`4`** | `0` |
| `picture/energySaving` | `auto` | `off` |
| `alwaysready/aod/getStatus` | `none` | `none` |
| Foreground app | `com.webos.app.home` | HDMI 1 |

## What differs and why it matters

1. **Always Ready is off on c3-b.** The test of the owner's goal needs it on, so the run sets `lifeOnScreenMode = alwaysReady` (the dark variant c3-a uses) for the Always Ready steps and restores `off` at the end. The result for "Always Ready off" is the c3-b baseline.
2. **Wake on LAN is off on c3-b**, which is the first gate of any standby test. The run set `wolwowlOnOff = true` and proved a WoL wake before any other standby step. Whether a wake works with the original `false` was not tested.
3. **The sound goes through ARC**, so the SpeakerFeedback tap (`hw:LGSIC,1`) stayed silent for an audible `pacat` tone while the microphone heard it: on c3-b SpeakerFeedback does not show playback that goes to ARC, and the microphone is the measure used for "a tone was produced". On c3-a SpeakerFeedback carried the stream.
4. **c3-b suspends daily** (9 kernel suspends in 9 h), unlike the freshly rebooted c3-a, so the Suspend path is exercised on every power-off by the owner.
5. **Other homebrew hooks** start from `init.d` on c3-b. That matters for the permanent autostart (ordering and the failsafe flag), not for this run.

## Decision

The microphone PCM and the PulseAudio sinks match c3-a, so the capture and playback paths are the same code. The firmware differs (`03.30.73`), so the daemon's firmware guard has to be told (`validated_firmware = ["03.30.73"]` in the test config). **That setting is valid only for this test**; adding `03.30.73` to the validated list is a decision for the owner after the live tests ([D5 results](D5-results.md)).
