# P4: Permanent install of the app package on c3-a (the owner's daily TV)

- **Device:** c3-a (LG C3 42", webOS TV 8.3.0, firmware `03.30.74`, on the allow-list). aarch64 kernel with 32-bit userland, Homebrew Channel **0.6.3**, Always Ready ON and Quick Start+ ON, LG hands-free off. The TV is the owner's PC display and was in use during the whole session.
- **Date:** 2026-11-03
- **Package:** `io.github.pando85.voicesatellite` **0.1.0** (`make ipk`, sha256 `5d66fe00...`), installed with `make tv-install-ipk` (appinstalld `dev/install`).
- **Approval:** the owner asked for a permanent install with start at boot on. No reboot, no power-state change, no setting change, no sound, no TTS, no announcement and no pipeline run was made; the TV volume was not touched (rules of this session).
- **Related:** [packaging design](../../design/packaging.md), [EXP-009](../../experiments/EXP-009-ipk-install-update.md) (the same procedure on c3-b), [P1 HA integration](P1-ha-integration.md) (the permanent HA entry), [always-on section 13](../../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b).
- **Privacy:** raw output is in the gitignored `devices/private/c3-a/`. No address, hostname, MAC or key appears here.

Labels: **verified-on-target** (c3-a, this date) unless marked.

## What was done

1. `make ipk` at 0.1.0, then `make tv-install-ipk` (copied to `/tmp`, `dev/install`, copy deleted). Baseline before: no app, no `/var/lib/webos-satellite`, `init.d` empty, no processes of ours.
2. State was set up with `ctl.sh install --config FILE --psk-file FILE`, so the install used the identity of the **existing** HA entry and not the random one a first install generates: the device name, the MAC seed and the Noise key of the `lg-c3-a` entry (from the earlier P1 runs), key file mode 0600, `[pairing] enabled = false` (an explicit key wins anyway). Everything else is the package default: wake mode, mic `LGSIC` / `WoV PDM Mic`, sink `pacat` with reply volume 40 % (-30 dB), `remote_off = "normal"`, log level `info`, `validated_firmware = ["03.30.74"]`. The staged files in the TV's `/tmp` were deleted right after.
3. `ctl.sh start`, then `ctl.sh autostart on` (hook `/var/lib/webosbrew/init.d/webos-satellite`, mode 0755). The app itself was not opened (it would have been drawn over the owner's screen); `ctl.sh` is what its buttons run.

## Results

| Check | Result | Label |
| --- | --- | --- |
| Install | `installed`; `ctl.sh install` with `--arch auto` (`uname -m` = aarch64) chose the **aarch64** build; `--check-config` printed `encryption=noise`, no warning. Modes: directories 0700, config and key 0600 | verified-on-target |
| **aarch64 daemon on c3-a (first run)** | `webos-satellite 0.1.0 (aarch64-unknown-linux-musl)` runs on c3-a: ALSA capture through its own ioctls, Noise, HA, power watch. The armv7 build was the one verified on this unit before; it is still in the package as the fallback | verified-on-target |
| Processes | 1 supervisor (`supervise.sh`) and 1 daemon, plus the daemon's `luna-send` subscription child; no `pacat`, no rotator needed | verified-on-target |
| Mic PCM | `hw:LGSIC,0` capture `RUNNING`, owned by the daemon; the other capture PCMs `closed` | verified-on-target |
| Firmware guard | "firmware 03.30.74 (webOS 8.3.0, build 20240226022140): validated" | verified-on-target |
| HA reconnect | The existing `lg-c3-a` ESPHome entry reconnected **6 s after the start with no change in HA**: same device and entity ids (no `_2` duplicates), no new entry, no re-authentication. HA 2026.10.4 hello, wake-word stage streaming | verified-on-target |
| Entities | `assist_satellite` `idle`; `media_player` `idle`, volume 0.4; mute switch `off`; the two pipeline selects and the finished-speaking select; the two wake word selects (`unavailable`, expected); new in 0.1.0: select "When the TV is turned off with the remote" = "Turn off normally" and text sensor "TV power state" = `Active` | verified-on-target |
| Pipeline select | Both pipeline selects still `preferred` (Kiwi). Nothing was written to HA | verified-on-target |
| Boot hook | `ctl.sh status`: start at boot on; the hook is the template with `PREFIX=/var/lib/webos-satellite`. On HBC 0.6.3 `startup.sh` runs `run-parts /var/lib/webosbrew/init.d` and resets the failsafe flag afterwards (read on the TV). **No boot was made**; the first boot with the hook is the owner's next natural restart | verified-on-target (hook written); boot **not verified on c3-a** (verified on c3-b with HBC 0.7.2, EXP-009) |
| Kernel | `dmesg` after the start: only the `aud_pdm` open lines of our own capture and the Wi-Fi driver's periodic P2P warnings; no oops, no ASoC or sound warning; `tainted` stays `4097` (as after the last reboot) | verified-on-target |

## Resources

Measured from `/proc/<pid>` with `CLK_TCK` 100 while the daemon streamed the mic to HA's wake word stage and nothing else happened (no reply, no announcement). The TV's load average was about 12 before and after (an unrelated baseline of the TV's own software).

| Measure | Result |
| --- | --- |
| Interval | 331 s after the start (two samples: 55 s and 331 s) |
| CPU | 437 ticks of user plus system time in 331 s: **1.3 % of one core** (1.2 % over the first 55 s); the TV has 4 cores |
| RSS | **1280 kB** at every sample, VmHWM 1280 kB, 4 threads, 13 descriptors |
| Mic PCM | `RUNNING` at every sample |
| Log | 19 lines, all `INFO` apart from the supervisor line: firmware guard, HA connected, streaming to the wake word stage; later only the TV's own power-state notices (the TV itself went through `Request Screen Saver` then `Screen On` twice, 3 minutes apart, with the PC attached; the daemon only logs them (cause not investigated)) |

Longer-term numbers (hours, days) are not part of this record; the supervisor, the rotator and the memory ceiling (64 MiB) bound a leak, and the P3 soak covered about 1 hour ([P3 robustness](P3-robustness.md)).

## First day of real use (2026-11-03 to 2026-11-04)

Counts only (no transcripts). Source: the satellite log on c3-a, HA's log and the Kiwi pipeline's debug runs. Label: verified-on-target for the TV log, observed on the HA side for the rest.

| Class | Runs | Cause |
| --- | --- | --- |
| Wake word detected, speech-to-text returned text, then `intent-failed` | 14 | The conversation agent: HA logged 14 matching "Unexpected error during intent recognition" errors, all `model not found (404)` from the Ollama integration (the server answered; the model was not on it) |
| Of those, speech-to-text took about 16 s (no end of speech found) | 3 | Hypothesis: HA's speech-to-text ran to its time limit; not explained (cause of the missing end of speech unknown) |
| Wake stage run ended by HA with no error | 11 | HA going away (see below) |
| Run in flight when HA disconnected | 11 | Same |
| Empty speech-to-text, or a successful reply | 0 | |

The 12 "HA disconnected" lines coincide with HA going down: the cluster recreated the HA pod 8 times in the window (pod start times from the cluster metrics, each within seconds of a disconnect or the reconnect that followed), and the other drops are the same pattern (hypothesis: more restarts of the old pod). The TV's power state is not the cause: they happen in Active and Active Standby alike. The satellite reconnected each time, 11 s to 11 min after the drop. Every `intent_end=-` is genuine: the pipeline sent an error instead of an intent end. In the next release the latency line also carries `error_at=<ms>` for that case. `ctl.sh status` also showed `reply volume: default %` and could never show a stored reply volume or remote power-off setting (`sed s///gp` printed nothing for an already clean line); both are fixed.

## Rollback, as left

- Stop: `ctl.sh stop`. Microphone off without stopping: `ctl.sh mic off`. No start at boot: `ctl.sh autostart off` (or `touch /var/lib/webos-satellite/disable`). Remove everything: `ctl.sh purge`, then `make tv-remove-ipk`. Details: [development: rollback](../../development.md#install-the-app-package-on-a-rooted-tv-persistent).
- TV `/tmp` holds none of our files; nothing outside `/var/lib/webos-satellite`, `/var/lib/webosbrew/init.d/webos-satellite` and the app directory was written.

## Residual unknowns

- Start at boot on c3-a with HBC 0.6.3: pending the owner's next restart. Check afterwards: `log/autostart.log` (60 s delay, "validated; starting"), `ctl.sh status`, no failsafe toast.
- The app UI on c3-a (HBC 0.6.3) was not opened.
- Replies, announcements, the "Keep listening (screen off)" mode and resume after a kernel suspend were not exercised on this unit in this session (no sound, no power-state change allowed). They are verified on c3-b only ([always-on section 13](../../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b)).
