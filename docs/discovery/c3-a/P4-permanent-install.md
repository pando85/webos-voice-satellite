# P4: Permanent install of the app package on c3-a (the owner's daily TV)

- **Device:** c3-a (LG C3 42", webOS TV 8.3.0, firmware `03.30.74`, on the allow-list). aarch64 kernel with 32-bit userland, Homebrew Channel **0.6.3**, Always Ready ON and Quick Start+ ON, LG hands-free off. The TV is the owner's PC display and was in use during the whole session.
- **Date:** 2026-11-03
- **Package:** `io.github.pando85.voicesatellite` **0.1.0** at first install (updated to 0.1.1, 0.2.0 and 0.2.2 below) (`make ipk`, sha256 `5d66fe00...`), installed with `make tv-install-ipk` (appinstalld `dev/install`).
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

## Update to 0.1.1 (2026-11-04)

- **Approval:** the owner (2026-11-04): deploy newer versions to c3-a to test them, and verify through Home Assistant everything that makes no sound. Same session rules as above: no reboot, no power-state or setting change, no sound, no reply-producing pipeline run, no ASoC or debugfs reads.
- **Package:** `0.1.1` (commit `ac8aca6`, "version 0.1.1", which bumps only `Cargo.toml` and `Cargo.lock`; contains d3fd410 and 82f4914), `make ipk` (check passed), sha256 `5d440b8b...`, built from the pushed commit. A bump was not strictly needed: update detection compares `SHA256SUMS`, not versions ([packaging P5](../../design/packaging.md)); it was made so the running build is told apart from 0.1.0.
- **Method (the same as P4 above, HBC 0.6.3, app not opened):** `make tv-install-ipk` (appinstalld `dev/install`, copy deleted); `ctl.sh status` then said "an update is pending" with the old daemon still running; `ctl.sh install` applied it (stop supervisor and daemon, re-install the aarch64 build chosen by `--arch auto`, "keeping etc/config.toml", boot hook refreshed, started again under a new supervisor). Time without a daemon: about 2 s.

| Check | Result | Label |
| --- | --- | --- |
| Version | `ctl.sh status`: package, installed and running build all `0.1.1 (aarch64-unknown-linux-musl)`; `update_pending` false | verified-on-target |
| State kept | `etc/config.toml` and `etc/noise_psk` sha256 prefixes equal before and after, modes 0600; device name and port unchanged; `etc/muted` absent before and after | verified-on-target |
| Start at boot | still `on` (hook present, refreshed by the install). Boot itself still not tested on c3-a | verified-on-target (hook); boot unverified |
| Reply volume line | `reply volume: 40 % (config default; none stored yet)`; 0.1.0 printed `default %`. It showed as soon as the new ipk was installed (before `ctl.sh install`), because `ctl.sh` runs from the app directory | verified-on-target |
| Startup log | limits, "reply volume 40 %", version line, firmware guard validated, listening on the API port, power watch subscribed, state `Active`, microphone open | verified-on-target |
| Processes | 1 supervisor, 1 rotator, 1 daemon (+ the power-watch `luna-send` child); nothing of ours left in `/tmp` | verified-on-target |
| RSS | daemon 1308 kB (1280 kB at 0.1.0), 4 threads; mic PCM `RUNNING` and owned by the daemon | verified-on-target |
| Kernel | no oops or ASoC warning in the `dmesg` tail (only CPU hotplug lines, the TV's own) | verified-on-target |
| HA reconnect | `HA connected` 3.7 s after the start (HA 2026.10.4), same entry `lg-c3-a`, no re-auth; "requesting pipeline (wake_word=true)", then "streaming mic audio to HA (wake word stage)" | verified-on-target |
| Entities in HA | Existing 10 entities unchanged, no `_2` duplicates of the device; **new: binary sensors "Listening" and "Microphone muted"**. Pipeline selects still `preferred`; nothing in HA was changed except the mute switch test below | verified-on-target |
| Listening in wake-word streaming | "Listening" `on`, "Microphone muted" `off`, mute switch `off` | verified-on-target |
| Mute ON via HA (`switch.turn_on`) | HA polled every 50 ms through the REST API: switch `on`, Listening `off`, Microphone muted `on` all seen within the first 50 ms poll after the call returned (the call took 0.01 s). Log: "microphone MUTED by Home Assistant", "stopped streaming mic audio", "capture: microphone closed, PCM released (muted)" within 1 ms. States held for 5 s | verified-on-target |
| Mute OFF (`switch.turn_off`) | switch `off` and Microphone muted `off` within 50 ms; **Listening `on` after 0.13 s**, i.e. after the capture PCM was reopened (log: unmuted, pipeline requested, streaming resumed, "capture: microphone open" 36 ms later). So "Listening" follows the real PCM, not the switch | verified-on-target |
| Mute ends as it began | switch `off`, Listening `on`, Microphone muted `off`; `etc/muted` still absent; wake-word streaming resumed (log lines above) | verified-on-target |
| `tools/dogfood_report.py` (`--days 1 --no-ha`) | Parses the 0.1.1 log: daemon version 0.1.1, RSS and threads, "HA mutes 1" and microphone-muted time, 2 starts (1 boot, 1 manual), power-state times. It reports 8 attempts of the day all `intent_error` (the conversation agent problem above, not 0.1.1). **Defect found:** it prints "supervisor not running" although one is (it searches the `ctl.sh status` text for "supervisor running", which the text no longer contains; the JSON has `supervised: true`). Not fixed here | verified-on-target (tool run); defect open |

The unmute made the daemon log a run record with every latency `-` ("run 1 latency ... stt_end=- ..."): the wake-stage restart after the mute counts as a run with no events; harmless, but it will count as a wake-stage cycle in the report. HA access for the test: a short-lived token from the login flow in a 0600 file, deleted afterwards, and the refresh token created for it revoked (only that one).

Left changed on the TV: only the new version (app directory, `/var/lib/webos-satellite/bin`, `VERSION`, `package.sums`, refreshed hook with the same content, rotated log). Rollback: install the 0.1.0 ipk the same way (appinstalld accepts a downgrade, EXP-009) and run `ctl.sh install`.

## Update to 0.2.0 (2026-11-04, TV control)

- **Approval:** the owner (2026-11-04): all newer versions are deployed to c3-a to test them, and everything that makes no sound is verified through HA. Rules kept: no reboot, no TV reboot, no input switch, no screen off, no power-state change, no app launch (the TV is the owner's active PC display), **no sound** (no TTS, announcement, start_conversation or beep). Allowed and done: volume down one step then restored exactly, mute on then off, the refresh button.
- **Package:** `0.2.0` (commit `10d298b`, "version 0.2.0", bumps `Cargo.toml` and `Cargo.lock`), `make ipk` (check passed, sha256 `d7a3f3f4...`), `make tv-install-ipk`, then `ctl.sh install` as for 0.1.1 (supervisor and daemon stopped, aarch64 build, "keeping etc/config.toml", hook refreshed, started again; about 2 s without a daemon). The config has no `[tv_control]` section, so the package defaults apply: screen, power_on, volume, mute, input, apps on; `power_off` off; `volume_limit` 40.

| Check | Result | Label |
| --- | --- | --- |
| Version and state kept | `ctl.sh status`: package, installed and running `0.2.0 (aarch64-unknown-linux-musl)`; `etc/config.toml` and `etc/noise_psk` sha256 prefixes equal before and after, modes 0600; start at boot still on; firmware validated | verified-on-target |
| Startup log | "tv_control: ready (power state Active; volume=ok mute=ok input=ok apps=ok)"; three children subscribed (power state, volume, foreground app); warnings: "cannot check that the TV declares launch (no declaration file readable)" and "the volume does not take a subscription; polling every 10 s" (both explained in [EXP-010 findings](../../experiments/EXP-010-tv-control-setters.md#part-b-results-2026-11-04)) | verified-on-target |
| HA | The entry `lg-c3-a` reconnected 3.7 s after the start (no re-auth); 22 entities of the device in HA's registry, no duplicates; the 10 new TV entities as in EXP-010 B1; `tv_power` not listed; states equal the luna getters (volume 89, mute off, `tv_speaker`, input `HDMI 1`, app `Other`, `Active`) | verified-on-target |
| Controls | mute on and off, volume down one step, refresh: pass; `volume up` refused because the TV (89) is above `volume_limit` 40; volume restored with a direct `volumeUp` call to 89 (see EXP-010 finding 1) | verified-on-target |
| Daemon restart (`ctl.sh stop`, `start`) | HA: TV entities `unavailable` about 5 s, back 2 s after the start, same entities, same states, no new registry entries | verified-on-target |
| Resources | Over 616 s in `Active`: daemon 923 ticks of user plus system time = **1.50 % of one core**, 1.63 % including the one-shot `luna-send` children of the 10 s volume poll; RSS 1408 kB at the start and 1672 kB at 10 min (VmHWM 1740 kB), 5 threads; the two subscription `luna-send` children about 0 ticks, 2.0 MB each. 0.1.1 measured 1.3 % and 1308 kB (a different 331 s window): about 0.2 points more CPU and 0.3 to 0.4 MB more RSS | verified-on-target |
| Kernel | `dmesg` grep for oops, BUG, panic and call trace: nothing; no ASoC or debugfs path read | verified-on-target |

HA access: a short-lived token from the login flow in a 0600 file, shredded afterwards; the refresh token created for it was deleted over the websocket (it was the only one with the current flag), no other token touched; no HA configuration change.

Left changed on the TV: only the new version (app directory, `/var/lib/webos-satellite/bin`, `VERSION`, `package.sums`, hook with the same content, rotated log). Volume, mute, input, screen and power state are as before. Rollback: install the 0.1.1 ipk the same way and run `ctl.sh install`.

## Update to 0.2.2 (2026-11-04, firmware policy, self-check, timers)

- **Approval:** the owner (2026-11-04): every new version is deployed to c3-a and verified silently through HA and the logs. Rules kept: no sound, no TV reboot, no screen off, no input switch, no app launch, no power change, no ASoC or debugfs read, no `apiadapter/system/getSystemInfo`. Allowed and done: restart of the daemon (by the update), volume up one step then restored exactly, mute on then off, the refresh button.
- **Package:** `0.2.2` (commit `afd6b02`, "version 0.2.2"; the commits after it are documentation only), `make ipk` (check passed, sha256 `b9d19df5...`), `make tv-install-ipk`, `ctl.sh install` as for 0.2.0 (supervisor and daemon stopped, aarch64 build, "keeping etc/config.toml", hook refreshed, started again; about 2 s without a daemon).

| Check | Result | Label |
| --- | --- | --- |
| Version and state kept | `ctl.sh status`: package, installed and running `0.2.2 (aarch64-unknown-linux-musl)`; `etc/config.toml` and `etc/noise_psk` sha256 prefixes equal before and after, modes 0600; `etc/firmware_seen` written at the first start; start at boot still on; firmware "verified (in safety.validated_firmware)"; "compatibility: verified; microphone self-check: passed (peak -48 dBFS)" | verified-on-target |
| Startup log | `voice_features=31`; `tv_control: ready (power state Active; volume=ok mute=ok input=ok apps=ok)`; **the launch-declaration warning of 0.2.0 is gone** (only the two 0.2.0 lines remain in the older log); "the volume reply has no subscribed flag; keeping the subscription and polling every 10 s until an update is pushed", and after the first volume change "an update was pushed on the volume subscription; it streams without the flag, polling stopped" (the `34d39ff` behaviour, as on c3-b); microphone open, HA connected 3.7 s after the start with the stored key, "requesting pipeline (wake_word=true)", `self-check: passed`; no WARN or ERROR after the start | verified-on-target |
| Entities in HA | The 22 entities of 0.2.0 unchanged and the new ones present, no duplicates: Firmware verified `on`, Capture problem `off`, Firmware changed `off`, Compatibility `verified`, Microphone self-check `passed (peak -48 dBFS)`, TV firmware `03.30.74`, TV model, Compatibility report ("ready: TV app > Report compatibility, or ctl.sh report"), buttons Re-run microphone check and Acknowledge firmware change, and the timer entities **Timers** (`none`), **Timer alarm** (`off`) and **Stop timer alarm** (button). No timer was set | verified-on-target |
| Self-check (C1 to C7 pass, C8 skipped) | 10 s window at the first open: RMS **-67.5 dBFS**, peak **-47.9 dBFS**, clipped 0, xruns 0, 205 chunks, 0 dropped. c3-b: RMS -67.5, peak -39.6 ([c3-b sound session 2](../c3-b/P4-sound-session.md#self-check-with-the-real-microphone-and-the-report)). The RMS is equal; the peak differs by what happened in each room during the 10 s. Same as c3-a's quiet-room reference (-67.6) | verified-on-target |
| Report privacy | `ctl.sh report`, `--full`, `--json` and the files `report.url`, `report.json`, `compat.json`, `report.short.txt`, `report.full.txt` compared on the PC (identifiers held only in a pipe or a temporary file shredded at once) with 12 identifiers read on the TV (MAC of every interface, every IPv4 and IPv6 address, the hostname, the device ids of `nyx-cmd`): **0 matches**, and no MAC or IPv4 pattern in any of the text. The report has model, webOS, firmware, kernel, PCM and level numbers only | verified-on-target |
| Refresh button | One summary line, `tv_refresh_lists: done: 4 inputs, 27 apps` (c3-b: 4 and 30) | verified-on-target |
| Volume | HA number now has **max 100** (it follows the TV's maxVolume; 0.2.0 refused to go above `volume_limit` 40 at 89). `number.set_value` 90 from 89: HA 90 after 3 s, log "setVolume accepted", "read back matches"; set 89: "read back matches", HA 89. **Restored exactly (89)** | verified-on-target |
| Mute | on then off through the HA switch: "setMute accepted", "read back matches" each time; ends **off** | verified-on-target |
| Resources | Over 300 s in `Active` (including the checks above): daemon 441 ticks = **1.47 % of one core** (0.2.0: 1.50 %); RSS 1744 kB, VmHWM 1800 kB, 6 threads (0.2.0: 1.4 to 1.7 MB, 5 threads); the three subscription `luna-send` children 0 ticks, about 2.0 MB each | verified-on-target |
| Kernel | `dmesg` tail: no oops, BUG, panic or call trace | verified-on-target |
| `tools/dogfood_report.py` (window from the restart, with the HA part) | Daemon 0.2.2, up 98.9 % of the covered 2 min, 1 start (manual), 0 crash restarts, 0 watchdog trips, 0 panics, 0 capture stalls or errors; HA 1 connect, 0 disconnects; microphone open 2 min, 0 mutes (the 2 min window began before the mute test), 0 busy events. Pipeline: 0 wakes, 0 attempts. HA debug runs: 1 in the window, 0 wake detected, 0 errors. **Defect found:** it prints "firmware validated: False" because it still reads the old status text ("validated"); 0.2.2 prints "verified". Not fixed here | verified-on-target (tool run); defect open |

**HA alias fix:** the TV mute switch of c3-a had the aliases "silencio de la tele del despacho", "silenciar la tele del despacho", "mute de la tele del despacho" and "sonido de la tele del despacho". The last one made "apaga el sonido de la tele del despacho" turn mute **off** (un-mute), as found on c3-b ([c3-b sound session, finding 3](../c3-b/P4-sound-session.md#2-findings)). It was **removed**; the other three were kept. No other HA entity, area or setting was changed.

HA access: a short-lived token from the login flow in a 0600 file, shredded afterwards; the refresh token created for it was deleted over the websocket (a refresh then returns `invalid_grant`), no other token touched.

Left changed: on the TV only the new version (app directory, `/var/lib/webos-satellite/bin`, `VERSION`, `package.sums`, new `etc/firmware_seen`, `run/` report files, hook with the same content, rotated log); in HA only the alias removal. Volume (89), mute (off), input, screen and power state are as before. Rollback: install the 0.2.0 ipk the same way and run `ctl.sh install`.

## Home Assistant side, 2026-11-04 (no change on the TV)

The owner asked for the TV control entities to be exposed to Assist: the screen switch, mute switch, turn-on button, volume number and up and down buttons, input select and app select of `lg-c3-a` are exposed to `conversation` with Spanish aliases ("tele del despacho", "televisión del despacho", "volumen de la tele del despacho" and similar), and the device was put in the HA area "Office A". Details and the c3-b equivalent: [c3-b P4](../c3-b/P4-permanent-install.md#home-assistant-side-done-with-a-short-lived-token-that-was-deleted-afterwards). No setter was called on c3-a.

## Rollback, as left

- Stop: `ctl.sh stop`. Microphone off without stopping: `ctl.sh mic off`. No start at boot: `ctl.sh autostart off` (or `touch /var/lib/webos-satellite/disable`). Remove everything: `ctl.sh purge`, then `make tv-remove-ipk`. Details: [development: rollback](../../development.md#install-the-app-package-on-a-rooted-tv-persistent).
- TV `/tmp` holds none of our files; nothing outside `/var/lib/webos-satellite`, `/var/lib/webosbrew/init.d/webos-satellite` and the app directory was written.

## Residual unknowns

- Start at boot on c3-a with HBC 0.6.3: pending the owner's next restart. Check afterwards: `log/autostart.log` (60 s delay, "validated; starting"), `ctl.sh status`, no failsafe toast.
- The app UI on c3-a (HBC 0.6.3) was not opened.
- Replies, announcements, the "Keep listening (screen off)" mode and resume after a kernel suspend were not exercised on this unit in this session (no sound, no power-state change allowed). They are verified on c3-b only ([always-on section 13](../../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b)).
