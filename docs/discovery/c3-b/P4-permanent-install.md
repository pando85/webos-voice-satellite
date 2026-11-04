# P4: Permanent install of the app package on c3-b (the household's hall TV)

- **Device:** c3-b (LG C3 55", webOS TV 8.3.0, firmware `03.30.73`, aarch64 kernel with 32-bit userland, Homebrew Channel **0.7.2**, Ethernet, sound out through ARC). The TV is in a household room. **LG hands-free ("Hi LG") is ON**, as the household left it; it was not touched.
- **Date:** 2026-11-04 (UTC times below).
- **Package:** `io.github.pando85.voicesatellite` **0.2.0** (current `main`, commit `3d08d44`, `make ipk` check passed, sha256 `1bdf4035...`), installed with `make tv-install-ipk` (appinstalld `dev/install`, copy deleted).
- **Approval:** the owner (2026-11-04): a permanent install of the satellite on c3-b with start at boot on, a permanent Home Assistant entry for it, the same updates as c3-a from now on, and one reboot to confirm the autostart (reboots on c3-b were pre-authorized). Rules kept: **no sound** (no TTS, announcement, beep or `start_conversation`), no ASoC or debugfs reads, no `apiadapter/system/getSystemInfo`, no LG setting changed, the remote power-off select left at "Turn off normally", nothing on c3-a changed except HA-side entity exposure (below).
- **Related:** [packaging design](../../design/packaging.md), [EXP-009](../../experiments/EXP-009-ipk-install-update.md) (the same procedure with test builds), [c3-a P4](../c3-a/P4-permanent-install.md), [pairing verification (P3)](../c3-a/P3-tv-verify-aec-pairing.md#3-pairing-on-the-tv-with-the-owners-ha), [TV control design](../../design/tv-control.md), [EXP-010](../../experiments/EXP-010-tv-control-setters.md).
- **Privacy:** no address, hostname, MAC, key or HA entry id appears here. No audio was recorded.

Labels: **verified-on-target** (c3-b, this date, firmware `03.30.73`) unless marked.

## What was done

1. Baseline: no app of ours, no `/var/lib/webos-satellite`, four other apps' hooks in `init.d`, no process of ours. Power state `Screen Saver`, foreground app the home app, no audio playing (recorded before the reboot test).
2. `make ipk` then `make tv-install-ipk` (the app was not opened).
3. `ctl.sh install --config FILE` with the package default config (see `packaging/webos/config.default.toml`) plus three owner-chosen values: device name `lg-c3-b`, friendly name "LG C3 (c3-b) Voice Satellite", a random `mac_seed` generated for this TV, and `validated_firmware = ["03.30.73", "03.30.74"]` (the capture path was validated on this firmware in [D5](D5-results.md); this is the first time the guard lists it). **No key in the config:** `[pairing] enabled = true` as in the default. The staged config in the TV's `/tmp` was deleted right after.
4. `ctl.sh start`, then the HA config flow was started by host and port 6053 straight away (inside the 10-minute pairing window), then `ctl.sh autostart on`.

## Results

| Check | Result | Label |
| --- | --- | --- |
| Install | `installed`; `--arch auto` chose the aarch64 build; `--check-config` printed `encryption=pairing(not paired)` and no warning; the firmware guard line "03.30.73 ... validated" | verified-on-target |
| **Pairing through the documented flow, no key typed** | HA's ESPHome config flow (host and port only) returned `create_entry` "LG C3 (c3-b) Voice Satellite" 11 s after the daemon started. Log: keyless plaintext probe, keyless zero-PSK Noise session, `key set ... (mode 0600)`, `PAIRED by "Home Assistant 2026.10.4"`; HA reconnected with the new key 5 s later. `etc/pairing` mode 0700, `noise_psk` mode 0600. HA reached the TV directly (no network rule problem on this unit). No fixed-key fallback was needed | verified-on-target |
| **Microphone: BUSY** | `WARN capture: MIC BUSY: another process holds the capture PCM (is LG hands-free voice on?)` once at the pairing and once at every start; `listening` stays `off` in HA, `microphone muted` `off`. `/proc/asound/card1/pcm0c` and `pcm2c` are `RUNNING`, owned by another process, not ours (`voiceinput`, LG's voice input service, was running; the owner pid's command name was `TaskManager` when read before the reboot). The daemon streams nothing and keeps retrying with its backoff (no further log line). The satellite is therefore **not usable for voice on this unit while "Hi LG" is on**; the household setting was not changed (owner rule). This is the expected EBUSY of the mic design ([P1 capture validation](../c3-a/P1-capture-validation.md)); the "Listening" sensor tells the truth | verified-on-target |
| HA entry | Loaded, 22 entities (list below), no duplicates; both assistant selects `preferred` (= the Kiwi pipeline, the same as c3-a; nothing was written to the pipelines); mute switch `off`; `assist_satellite` `idle`. The satellite's conversation agent still fails in Kiwi (backlog #12, owner decision pending), so even with a free microphone a command would end in an error | verified-on-target |
| States against the luna getters | Volume 3 of max 60, mute off, sound output `external_arc`, power state `Screen Saver`, foreground app the home app (`Other`): the HA states equal `master/getVolume`, `getForegroundAppInfo` and `getPowerState` read over SSH | verified-on-target |
| Boot hook | `ctl.sh autostart on`: `/var/lib/webosbrew/init.d/webos-satellite` mode 0755, sorted after the four hooks of other apps | verified-on-target |
| **Reboot test (one, `reboot` over SSH)** | SSH back at 36 s uptime; no failsafe flag; the daemon started at uptime about 100 s (`autostart.log`: "firmware 03.30.73 ... validated; starting", `start.sh exited with status 0`); HA reconnected 10 s after the start with the stored key (no re-authentication, no new entities); the TV control entities came back with the same states; the 5 threads and RSS as before the reboot. The TV came back in `Active` (not `Active Standby`) with HA's input select showing `HDMI 1` (not `Other` as before), so no wake key was needed; that is not the state it was in before (`Screen Saver` on the home app): it was left as the TV chose | verified-on-target |
| Resources | Idle with the microphone busy (no capture, no reply): 35 ticks of user plus system time in 240 s (`CLK_TCK` 100) = **0.15 % of one core** (this includes the 10 s volume poll); RSS 1.7 MB (VmHWM 1.8 MB), 5 threads, 13 descriptors. c3-a streaming the live microphone costs about 1.5 %: the capture, not the daemon, is the cost | verified-on-target |
| Kernel | `dmesg`: no oops, BUG, panic or call trace; `tainted` stays `4097`; the only warnings are the TV's own HDMI 2.1 receiver and Wi-Fi P2P lines (hypothesis: an HDMI source trying to link up; not investigated) | verified-on-target |

### Home Assistant side (done with a short-lived token that was deleted afterwards)

- **Entities of the device (22):** assistant satellite, media player (reply volume), mute-microphone switch, two pipeline selects, finished-speaking select, two wake word selects, "When the TV is turned off with the remote" select, "Listening" and "Microphone muted" binary sensors, TV screen switch, TV mute switch, Turn on TV button, TV volume up and down buttons, TV volume number, TV input select, TV app select, "Refresh TV inputs and apps" button, and the diagnostic "TV power state" and "TV sound output". `tv_power` is not listed (`power_off` is off by default).
- **Exposed to Assist (`conversation`), both c3-a and c3-b, with Spanish aliases** (the pipeline language is `es`): the screen switch ("tele del despacho" and "televisión del despacho" for c3-a, "tele del hall" and "televisión del hall" for c3-b, plus "pantalla de la tele ..." and "pantalla del ..."), the turn-on button ("encender la tele ..."), the volume number ("volumen de la tele ...", "volumen de la televisión ...", "volumen del ..."), the volume up and down buttons ("subir volumen de la tele ...", "bajar volumen de la tele ..."), the mute switch ("silencio de la tele ...", "silenciar la tele ...", "mute de la tele ...", "sonido de la tele ..."), the input select ("entrada de la tele ...", "fuente de la tele ...") and the app select ("aplicación de la tele ...", "app de la tele ..."). Diagnostic entities, the microphone switch, the pipeline selects and the sensors are not exposed. The two satellite media players were already exposed by default and were left as they were.
- **Areas:** the c3-a device was put in the area "Office A" (alias "oficina de Alex"), the owner's office. HA has no area for the hall (areas: Bedroom, Living Room, Office A, Office T, Terrace), so **c3-b has no area**; none was created.
- **Alias check with the built-in conversation agent (read-only questions, no action taken):** "¿está la tele del hall encendida?" answered "Sí" and "¿está la tele del despacho encendida?" answered "No, su estado es off" (c3-a was in `Active Standby`, its screen switch `off`), "¿cuál es el estado de la tele del hall?" answered with the device. Commands were not issued from Assist. "¿está silenciada ...?" is not a sentence the Spanish intents know; that is HA's grammar, not the alias.
- **Token:** the refresh token created for the session was deleted over the websocket (the only one with the current flag); no other token, user or session was touched.

## Left changed

On the TV: the app, `/var/lib/webos-satellite` (config, pairing key, log), the boot hook, a running supervised daemon. In HA: the c3-b entry, device and entities, the exposure and aliases of the TV control entities of both devices, and the c3-a device area. Nothing else on c3-a.

Rollback: `ctl.sh stop`; `ctl.sh autostart off`; `ctl.sh purge` and `make tv-remove-ipk` to remove everything; in HA delete the c3-b entry (HA keeps its generated key in its key storage, see P3).

## Residual unknowns

- Whether capture works on c3-b with "Hi LG" on: **no, it is busy**. Turning "Hi LG" off is the household's decision; nothing was changed. After it is off, the capture validated in [D5](D5-results.md) applies (a 81 min run with no `MIC BUSY`), with this build.
- IR remote presses for a real suspend/resume and the remote power-off path on the installed daemon: deferred by the owner (2026-11-04), listed in the roadmap.
- The Kiwi conversation agent (backlog #12) and the sound session are still open; nothing audible was run here.
- The app UI on c3-b with this build was not opened.
