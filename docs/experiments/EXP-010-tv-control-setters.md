# EXP-010 — TV control setters on c3-b (screen, volume, mute, input, app, power)

**Status:** Part A RUN on c3-b, 2026-11-04 (owner's go the same day, "can be run now"); step A5 skipped (not authorized). **Part B RUN 2026-11-04** with daemon 0.2.0: on **c3-b** the disruptive controls (screen, input, app, power-off in the "Keep listening" mode) through a PC-side aioesphomeapi client (no HA entry), on **c3-a** the non-disruptive subset through the owner's HA 2026.10.4 on the permanent install. B10 and B11 not run; see [Part B results](#part-b-results-2026-11-04).
**Date:** written and Part A run 2026-11-04.
**Device alias / model / firmware / webOS / architecture:** c3-b, LG C3 55 inch, firmware `03.30.73`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland. **Not c3-a** (c3-a is in daily use; a separate go is needed after c3-b passes).
**Power and playback state:** `Active` or `Screen Saver` on the home app, nothing playing, at the start of every step; `Screen Off` and `Active Standby` only where a step says so.
**Related research / requirements / issue:** [TV control design](../design/tv-control.md), [ADR 0005](../decisions/0005-tv-control-entities.md), [backlog #11](../project/backlog.md#11-tv-control-from-home-assistant-via-the-satellite), [D5](../discovery/c3-b/D5-results.md), [D6](../discovery/c3-b/D6-poweroff-screenoff.md).
**Risk:** user-level TV state changes (screen, volume, mute, input, foreground app, power key in the keep-listening mode). No TV settings menu writes, no firmware, no persistence, no reboot. Sound: only the TV's own volume changes by one or two steps on the home screen (no content playing), plus at most one short announcement in Part B.
**Approval needed / obtained:** the owner's go for Part A on c3-b was given on 2026-11-04 with these limits: no audio, A5 not authorized, every changed value restored right after, no reboot, no install. Part B: the owner's go of 2026-11-04 for c3-b (settings and power-state changes, everything restored, no sound, no HA change) and for c3-a (deploy 0.2.0 and verify through HA, no sound, and on c3-a only volume plus or minus one step restored exactly, mute on then off, the refresh button; no input, screen, power or app changes because it is the owner's active PC display).

## Question and falsifiable hypothesis

For each v1 control, does the declared setter, called as root through `luna-send` with the parameters the TV's own SSAP gateway uses, change the state as a getter reads it back, and does restoring the original value work?

Falsified per control by: the setter is refused (`returnValue: false`), accepted with no state change, changes something else, cannot be restored, or makes the TV misbehave. A failed control stays off by default in the design ([design section 9](../design/tv-control.md#9-configuration)); it is not retried with guessed parameters.

## Prerequisites and safe baseline

- The owner's go and a time when nobody watches c3-b. The [D6 gate](../discovery/c3-b/D6-poweroff-screenoff.md#procedure) applies before each step: power state `Active` or `Screen Saver`, foreground app home or screen saver, no sink inputs (`pactl list short sink-inputs` empty), 60 s of no remote input bytes on the remote event nodes (passive `cat`, no grab). If anything suggests use, wait 10 min and repeat. If a person walks in, stop, restore, report.
- SSH as root to c3-b; the address in a gitignored local file, never in this document. Raw output only to the gitignored `devices/private/c3-b/EXP-010/`.
- **Read nothing under `/sys/kernel/debug` or ASoC `dapm_widget` paths** (the c3-a oops hazard). No `find /`. Named paths only.
- Baseline `dmesg | tail -n 20` saved (private), to compare after each step.
- Way back known to work (verified on c3-b): `turnOnScreen`, `tvpower/power/powerOn {"reason":"remoteKey"}`, `networkinput/controls/power`, the remote, WoL from the PC (`wolwowlOnOff` as recorded; set it only if the owner agrees, and restore it).
- Part B needs the implementation (a build with `[tv_control]`) and a temporary HA entry as in [D6 step 3](../discovery/c3-b/D6-poweroff-screenoff.md#procedure), started muted (kill switch present) until the entry is checked.

## Commands / steps (copyable and bounded)

Shorthand on the TV: `L='luna-send -n 1 -w 3000'`. Every getter's reply goes to the private folder; this document gets only the result columns. **Never call** `apiadapter/system/getSystemInfo` (serial), `tvpower/power/powerOff`, `power2/*`, request registrations, `micomservice/*`, or any `setSystemSettings`.

### Step 0. Originals (read-only)

```sh
$L luna://com.webos.service.tvpower/power/getPowerState '{}'
$L luna://com.webos.service.audio/master/getVolume '{}'
$L luna://com.webos.service.apiadapter/audio/getSoundOutput '{}'
$L luna://com.webos.applicationManager/getForegroundAppInfo '{}'
$L luna://com.webos.service.apiadapter/tv/getExternalInputList '{}'
$L luna://com.webos.media/getForegroundAppInfo '{}'
```

Record: power state, `volume`, `muteStatus`, `maxVolume`, `soundOutput`, foreground `appId`, the input list (`id`, `connected`, `appId`; labels stay private). This is the restore target for every step.

### Step 1. Setter parameters (read-only file reads)

Read the TV's own SSAP gateway interface files (named directory, the same place that maps `system/turnOff` to `networkinput/controls/power`, [always-on 10.1](../design/always-on.md#101-new-facts-from-c3-a-read-only)) for the entries `audio/setVolume`, `audio/setMute`, `audio/volumeUp`, `audio/volumeDown`, `tv/switchInput`, `system.launcher/launch`. Record for each: the luna method it maps to and the parameter names. Also read the luna-service2 API declaration for each method (`/usr/share/luna-service2/api-permissions.d/`, file names filtered by service). **If a mapping or parameter is not found, that control is not tested in Part A** and is recorded as INCONCLUSIVE; no guessing.

Optionally, for subscription support (read-only): `$L luna://com.webos.service.audio/master/getVolume '{"subscribe":true}'` and the same for `applicationManager/getForegroundAppInfo`; record whether the first reply has `subscribed: true`.

### Part A: raw setters, one control at a time

Each step: read → set → read back after 0.5 s → restore → read back → `dmesg | tail` compared with the baseline. Wait 30 s between steps. `<...>` marks values from step 0 or step 1.

| Step | Set | Verify | Restore | Expected signal |
| --- | --- | --- | --- | --- |
| A1 screen from `Screen Saver` | wait for `Screen Saver` (or skip if it does not come within 10 min), then `tvpower/power/turnOffScreen '{}'` | `getPowerState` = `Screen Off` | `turnOnScreen '{}'`, state `Active` | The same as from `Active` in D5. Records whether `Screen Saver` is accepted |
| A2 mute | mute setter with "muted" from step 1 | `getVolume` `muteStatus = true` | mute setter "unmuted"; `muteStatus` = original | No sound change audible (nothing plays); on-screen mute icon noted if the owner looks |
| A3 volume set | `setVolume` to `<volume> + 1` with step 1 parameters | `volume = <volume> + 1` | `setVolume <volume>`; read back | One step; record whether an on-screen volume bar appears and whether `soundOutput = external_arc` changes the meaning (soundbar volume) |
| A4 volume step | `volumeUp`, read, `volumeDown` | `+1` then original | as shown | The step size of one call |
| A5 volume on the TV speaker (optional, owner's choice: it writes the sound output setting) | set `soundOutput` to `tv_speaker`, repeat A2 to A4 | as above | `soundOutput` back to `<soundOutput>`, read back | Behaviour on the internal speaker. Skip unless the owner asks; the sound output is a setting |
| A6 input | input setter with the `id` of another input (prefer a connected one; a disconnected one shows a "no signal" screen) | `getForegroundAppInfo` `appId` = that input's `appId`; picture changes | `applicationManager/launch {"id":"<original appId>"}`; read back | Input switch works; record the reply |
| A7 app | `applicationManager/launch {"id":"<a built-in app with no autoplay, chosen from the launch points and recorded privately>"}` | foreground `appId` matches | launch `<original appId>`; read back | Launch works for an app other than the satellite's own |
| A8 actions in `Screen Off` | `turnOffScreen`; then A3 (volume +1 and back), then A7 (launch and back) | state after each: still `Screen Off`, or `Active`? | `turnOnScreen`; originals | Whether volume and app actions light the panel. Both outcomes are results |
| A9 refusals in transitions (optional) | none: observe only that the design refuses; nothing to call | | | Covered by the PC tests; listed for completeness |

### Part B: through Home Assistant (after the implementation exists)

Daemon with `[tv_control]` all on except `power_off`, then a second run with `power_off = true`. Same gate and originals.

| Step | Action from HA | Verify |
| --- | --- | --- |
| B1 | Entity list | Types, object ids, options and categories as in [design 3.1](../design/tv-control.md#31-entity-table); no app title or input label in the daemon log at `info` |
| B2 | `tv_screen` off and on; `tv_turn_on` from `Screen Off` | States in HA follow `getPowerState` |
| B3 | `tv_volume` +1 and back; up and down buttons; a set above `volume_limit` | Read-back value in HA; the clamp is visible |
| B4 | `tv_mute` on and off | `muteStatus` |
| B5 | `tv_input`, `tv_app`, and `Other` | Foreground app; `Other` refused |
| B6 | `power_off = true`, select "Turn off normally": `tv_power` | Shows unknown; commands refused; no key sent (luna log) |
| B7 | Select "Keep listening (screen off)": `tv_power` off from `Active` | `Request Power Off` (reason `remoteKey`), `Screen Off` about 5 to 6 s later; `listening` stays on |
| B8 | `tv_power` on from `Screen Off`; then B7 again and `tv_turn_on` | `Active`; the darkener does not darken the HA wake |
| B9 | Kill switch present, `tv_power` off | Refused, no key |
| B10 | `tv_power` off with an app playing sound (only if the owner agrees to content playing at low volume) | Whether the sound stops after the power-off (design open question 2) |
| B11 | `Active Standby` (remote power-off with the select on "Turn off normally" and Always Ready as the owner set it): entities | Volume, mute, input and app show unknown; `tv_turn_on` brings the TV to `Active` |
| B12 | CPU of the daemon and the `luna-send` children over 10 min in `Active` | `top` samples, compared with 0.1.0 |

## Expected evidence and failure interpretation

| Outcome | Meaning | Design consequence |
| --- | --- | --- |
| Accepted and read back matches, restore works | Control works on c3-b `03.30.73` | Default may become true; compatibility row PASS |
| `returnValue: false` with `errorText` | Method or parameters wrong for this firmware, or a permission issue | Control stays false; record the error code; no retry with other parameters |
| Accepted, no state change | Silent no-op | Control stays false |
| State changes, restore fails | Unsafe | Stop; restore with the remote or the owner; control stays false and is noted in the design |
| Panel lights in `Screen Off` after volume or app (A8) | Not a failure | The design documents it next to the entity |
| Volume step on ARC changes the soundbar, not the TV | Expected (hypothesis) | Document per sound output |
| A kernel oops in `dmesg`, the TV unresponsive 60 s | Stop all steps | Owner decides on a reboot; nothing else runs |

## Observations and redacted evidence links

Run on **c3-b** (LG C3 55 inch, firmware `03.30.73`, webOS TV 8.3.0), **2026-11-04**, by an agent over SSH as root with `luna-send -n 1 -w 3000` (a pseudo-terminal is needed: through a plain pipe the audio getters print nothing, as in D5). Raw output is in the gitignored `devices/private/c3-b/EXP-010/`; nothing from it is committed. Labels below are `verified-on-target` for c3-b `03.30.73` unless said otherwise. Latency is the time of the `luna-send` call as seen on the TV (10 ms clock resolution), without the SSH round trip.

**Baseline (step 0).** Power state `Screen Saver`, foreground app the home app, `volume` 3, `muteStatus` false, `maxVolume` **60**, `soundOutput` `external_arc`, no sink inputs. `com.webos.media/getForegroundAppInfo` showed only the screen saver app as "playing" (the screen saver video; no content, no sound). Inputs: 4 HDMI entries (ids `HDMI_1` to `HDMI_4`, app ids `com.webos.app.hdmiN`), two connected. The earlier design text "maxVolume 100 on c3-b" was wrong; the TV reports 60 (volume limiter `familyCare`, `volumeLimitable` false). `dmesg` baseline saved privately.

**Step 1 (read-only file reads, no guessing).** The SSAP gateway interface files (`audio`, `tv`, `system.launcher` under the secondscreen gateway's `interfaces` directory) map: `audio/setVolume`, `volumeUp`, `volumeDown` to `com.webos.service.audio/master/setVolume`, `volumeUp`, `volumeDown`; `audio/setMute` to `com.webos.service.apiadapter/audio/setMute`; `tv/switchInput` to `apiadapter/tv/switchInput`; `system.launcher/launch` to `apiadapter/system_launcher/launch`. The interface files carry no parameters. Parameters come from two sources:

| Control | Parameter | Source |
| --- | --- | --- |
| mute | `{"mute": <bool>}` for `apiadapter/audio/setMute`, which forwards `mute` to `com.webos.service.audio/master/muteVolume` | the apiadapter's own JavaScript on the TV (read-only), then confirmed by the calls |
| switchInput | `{"inputId": "<id from getExternalInputList>"}`; it looks the id up with `eim/getInputStatus` and launches that input's app through `applicationManager/launch` | apiadapter JavaScript on the TV |
| launch | `{"id": "<appId>"}` | the apiadapter JavaScript, and the earlier EXP-009 use |
| setVolume | `{"volume": <int>}` | **not found on the TV** (the audio service is a binary); taken from what aiowebostv sends over SSAP (`observed-on-other-device`) and confirmed by the calls |
| volumeUp, volumeDown | `{}` | declared in `com.webos.service.audio.api.json`; no parameters needed, confirmed by the calls |

Subscriptions (first reply): `audio/master/getVolume {"subscribe":true}` is a real subscription (pushes seen in A3); `getForegroundAppInfo {"subscribe":true}` answered `subscribed: true`; `getPowerState` as before. `com.webos.media/getForegroundAppInfo` answered `subscribed: false` without the flag (not tried with it).

**Part A results**

| Step | Method and payload | Reply | Latency | State changed as read back | Restore and read back | Result |
| --- | --- | --- | --- | --- | --- | --- |
| A1 screen from `Screen Saver` | `tvpower/power/turnOffScreen {}`, then `turnOnScreen {}` | `turnOffScreen`: `{returnValue:true, state:"Screen Off"}`; `turnOnScreen`: `{returnValue:true, state:"Active"}`. Run twice, both from `Screen Saver` (the first run lost the `turnOffScreen` reply to a script timing bug, but the state after it was `Screen Off`; the second run captured everything) | `turnOffScreen` 80 ms, `turnOnScreen` 100 to 140 ms | `getPowerState` `Screen Off` at 0.5 s and still at 2.5 s | `Active` on the home app, volume 3, not muted. The TV came back to `Screen Saver` by itself about 10 min after the first restore (the screen saver is not forced back; the state was `Active`, not `Screen Saver`, between runs) | PASS: `Screen Saver` is accepted like `Active` |
| A2 mute | `apiadapter/audio/setMute {"mute":true}`; also `audio/master/muteVolume {"mute":true}` | `{returnValue:true, muteStatus:true, soundOutput:"external_arc"}` (both methods the same shape) | 30 ms | `muteStatus` true at 0.5 s; volume unchanged | `{"mute":false}` by both methods; `muteStatus` false | PASS (both methods) |
| A3 volume set | `audio/master/setVolume {"volume":4}` (3 to 4), then `{"volume":3}` | `{returnValue:true, volume:N, soundOutput:""}` (the reply's `soundOutput` is empty; the getter has the value) | 30 ms | `volume` 4 at 0.5 s, then 3. A `getVolume` subscription received the initial value and one push per change (3 lines for 2 changes) | 3, read back | PASS |
| A4 volume step | `audio/master/volumeUp {}` then `volumeDown {}` | `{returnValue:true, volume:N, soundOutput:""}` | 20 to 30 ms | one call = **1 step** (3 to 4 to 3) | 3, read back | PASS |
| A5 volume on the TV speaker | not run | | | | | skipped (not authorized) |
| A6 input | `apiadapter/tv/switchInput {"inputId":"HDMI_1"}` (a **disconnected** input, chosen so that no source can play sound) | `{returnValue:true}` | 440 ms | foreground app `com.webos.app.hdmi1` at 1.5 s; power state stayed `Active` | `applicationManager/launch {"id":"com.webos.app.home"}`, foreground home, volume and mute unchanged. Error case: an unknown id gives `{returnValue:false, errorCode:-1000, errorText:"no such input"}` | PASS (disconnected input only; a connected source was not switched to because it could play sound) |
| A7 app | `applicationManager/launch {"id":"com.webos.app.sportsteamsettings"}` (a built-in settings-style app, silent); also `apiadapter/system_launcher/launch {"id":...}` | `{returnValue:true}`; the apiadapter form adds a `sessionId` | 130 ms (direct), 180 ms (apiadapter) | foreground app matched at 2 s (both forms) | launch home, read back. Error case: an unknown id gives `{returnValue:false, errorCode:-101, errorText:"not exist"}` | PASS for an app other than the satellite's own |
| A8 actions in `Screen Off` | `turnOffScreen` (reply `{returnValue:true, state:"Screen Off"}`, 70 ms), then volume 3 to 4 to 3, mute on and off, launch of the settings app and back | all `returnValue:true` | as above | state stayed **`Screen Off`** after every action, including the app launch (the foreground app changed; **the panel was not looked at**, only the power state was read) | `turnOnScreen` answered `Active` at once (80 ms); home app, volume 3, not muted | PASS: none of the actions changed the power state. Whether a launch lights the panel is **not verified visually** |
| A9 | not called | | | | | n/a (PC tests) |

**Kernel log.** No oops, BUG, panic or call trace after any step. The tail showed only the driver's usual warning lines about the video capture path and a CPU core going offline and online. No ASoC or debugfs path was read.

**Not covered:** whether an on-screen volume bar or mute icon appeared (nobody looked); volume and mute on the **TV speaker** (A5); a **connected** HDMI source (to avoid sound); whether an app launch lights the dark panel; behaviour with content playing; every step in `Active Standby` (the design refuses there anyway); Part B.

## Part B results (2026-11-04)

**Method.** Version `0.2.0` (commit `10d298b`, aarch64 build, sha256 `4f27a6e8...`).

- **c3-a** (firmware `03.30.74`): the permanent install was updated to 0.2.0 ([P4](../discovery/c3-a/P4-permanent-install.md#update-to-020-2026-11-04-tv-control)); HA 2026.10.4 through its REST and websocket APIs with a short-lived token (deleted afterwards; the run's own refresh token deleted, no other token touched; no HA configuration changed). Luna getters over SSH on a pseudo-terminal were read-only.
- **c3-b** (firmware `03.30.73`): the same binary run from `/tmp/exp010` with a temporary config (file microphone of silence, `null` sink, its own device name and port, `power_off = true`, debug log, no HA entry), driven by a PC-side `aioesphomeapi` 46.6.0 client that plays HA (list entities, subscribe states, send commands, print every state change with its time). No voice pipeline was subscribed, so no audio was sent or recorded; **the microphone was a file, so "keeps listening" here means the process and API stayed up, not that the real microphone was open** (D6 covers the real one). The TV's real state was read after each step with `luna-send`. Everything was removed from `/tmp` afterwards; no process or file of ours is left.
- Times are from the command to the state change as the client saw it (includes the 0.5 s read-back for set commands). No sound was made on either TV; no TTS, announcement or beep.

**Per control and TV** (`verified-on-target`, 0.2.0, 2026-11-04; `-` means not run):

| Control (step) | c3-b `03.30.73` (PC client) | c3-a `03.30.74` (owner's HA) |
| --- | --- | --- |
| B1 entity list | PASS: 17 entities (types, categories and options as in design 3.1; `tv_power` listed with `power_off = true`; `tv_volume` 0 to 60; `tv_input`: 4 HDMI labels plus `Other`; `tv_app`: 30 apps (the `app_limit`) plus `Other`, the satellite's own app absent). Identical list and keys after a daemon restart | PASS: 10 new entities beside the 12 older ones (22 in HA's registry, no `_2` duplicates); `tv_power` **not listed** (default); `tv_volume` 0 to 100; `tv_app`: 27 apps plus `Other` (Homebrew Channel listed, own app absent); categories: refresh button `config`, power state and sound output `diagnostic`, the rest none. HA derives entity ids from the entity names (for example the button is `..._turn_on_tv`), not from the object ids |
| B1 states against luna | PASS (volume 3, not muted, `external_arc`, input and app `Other` on the home app) | PASS: volume 89, mute off, `tv_speaker`, input `HDMI 1` (foreground app `com.webos.app.hdmi1`), app `Other`, power state `Active`; each equal to the getters read over SSH |
| B2 `tv_screen` off and on, `tv_turn_on` | PASS: off from `Screen Saver` 0.08 s (`Screen Off`), on 0.10 s (`Active`); `tv_turn_on` from `Screen Off` 0.04 s; volume set (3 to 4 to 3) in `Screen Off` accepted and read back | - (display in use) |
| B3 `tv_volume`, up and down buttons, clamp | PASS: slider 3 to 4 to 3 (1.1 to 1.3 s to the new state, read back matches), buttons plus or minus one step (0.6 to 0.8 s). With `volume_limit = 4`: a set to 10 gave 4, `volume up` at 4 refused ("at the volume limit"), back to 3 | PASS (down and refusal): `volume down` 89 to 88 (log "volumeDown accepted", read back matches, HA saw 88 at its first poll, 1 s). **`volume up` at 88 was refused** ("at the volume limit"): the TV is at 89, above the default `volume_limit` of 40, so neither the slider (it would clamp to 40) nor `up` can return to it. Restored with a direct `volumeUp` call (the TV's own value 89); HA followed by the 10 s poll. See finding 1 |
| B4 `tv_mute` | PASS: on and off, read back matches (0.56 s and 0.69 s) | PASS: on then off, log "setMute accepted", "read back matches" 0.7 s later; HA showed it 0.5 to 1 s after the call; ended off |
| B5 `tv_input`, `tv_app`, `Other` | PASS: `HDMI 1` (a disconnected input) made the foreground app `com.webos.app.hdmi1` (select at 0.18 s); a built-in silent settings app by title: foreground matched at 0.28 s; `Other` refused (state sent again). The TV was returned to the home app with a direct `launch` call: the entities have no way back to the home app. A launch made outside the satellite (over SSH) reached HA within about 2 s (foreground subscription push) | - (display in use); the states of both selects already matched the TV |
| Refresh button | PASS: lists read, no reconnect when unchanged (finding 3) | PASS: pressed once, no disconnect, entities unchanged |
| B6 `power_off = true`, select "Turn off normally" | PASS: `tv_power` shows unknown; the command was refused ("... is not on Keep listening"); power state stayed `Active`, no key logged | not applicable (`power_off` default false, not listed) |
| B7 "Keep listening", `tv_power` off from `Active` | PASS twice: `tv_power` on 0.1 s after the select; off: `Request Power Off` 0.04 s, `Active Standby` at 3.3 s, `powerOn` try 1 refused (`Keyblock1.5second`), try 2 accepted, `Active` and `Screen Off` at **9.96 s** (run 1) and **12.8 s** (run 2: `powerOn` accepted but the state was still standby 5 s later, so the state-checked toggle-key fallback was sent; `Screen Off` 9.1 s after the wake). The process and API link stayed up | - |
| B8 `tv_power` on, `tv_turn_on` | PASS: `tv_power` on from `Screen Off` gave `Active` in 0.13 s; `tv_turn_on` 0.04 s; the darkener did not darken the TV again in the 20 s watched afterwards | - |
| B9 kill switch present, `tv_power` off | PASS: refused ("the kill switch is present"), power state stayed `Active`, no key; `listening` off and `microphone_muted` on while it existed, back after removal | - |
| B10 power-off with content playing | NOT RUN (no content playing authorized) | - |
| B11 `Active Standby` entities | PARTIAL: not run as a settled state (a plain power-off would end this unit in `Suspend` or `Active Standby` depending on Always Ready, needing the remote or WoL). During the transient `Active Standby` of the B7 power-off (3.3 to 9.7 s) the screen, volume, mute, input and app entities reported **missing state** and recovered at `Active`; `tv_power` went off, then missing during the 20 s cooldown, then off again in `Screen Off` | - |
| B12 resources (10 min, `Active`) | not measured (file microphone; RSS 1.73 MB) | CPU 1.50 % of one core for the daemon, 1.63 % with its one-shot `luna-send` children (616 s window, `CLK_TCK` 100); RSS 1.41 to 1.67 MB, VmHWM 1.74 MB; the two subscription children about 0 ticks and 2 MB each. 0.1.1 was 1.3 % and 1.31 MB (different window). No kernel oops |
| Daemon restart | list identical, states back | PASS: `ctl.sh stop` then `start`; HA marked the TV entities `unavailable` for about 5 s, reconnected 2 s after the start, same entities and states, nothing new in the registry |

**HA and missing states.** On the owner's HA 2026.10.4 a `switch`, `number` and `select` without a state show `unknown`: seen at the first listing (before the first state) for the mute switch, the volume number and the input and app selects. A **live** change from a value to missing on HA was not seen (it needs `Active Standby` or a failed control, not allowed on c3-a); the PC client saw the daemon send it correctly. Whether HA lists entities again after the option list changes was not tested against HA (no app changes were made); a restart of the daemon relists correctly.

**Findings (bugs and surprises, none fixed in this run):**

1. **Volume above the limit on c3-a.** c3-a's TV speaker is at volume 89 (the PC display), far above the default `volume_limit = 40` (written for 9 to 12 on c3-a and c3-b in earlier tests). With the default the HA volume controls can only lower it. Not a defect of the code; the default does not fit this unit. The owner decides whether to raise `volume_limit` on c3-a; the clamp itself works (c3-b with limit 4).
2. **The volume getter does not report a subscription on either TV.** `getVolume {"subscribe":true}` answers with the volume and no `subscribed` field (checked on both). By design 6 item 4 the control falls back to polling every 10 s ("the volume does not take a subscription; polling every 10 s"). EXP-010 A3 on c3-b saw pushes on that subscription, so push is probably available; the daemon polls, which misses a change that is undone within 10 s and shows an external volume change up to 10 s late. A fix would accept the volume reply as a subscription (small, not made).
3. **Declaration check for `launch` is always `Unchecked`.** The warning "cannot check that the TV declares launch (no declaration file readable)" appears at every start on both TVs. The directory and format are as assumed (`/usr/share/luna-service2/api-permissions.d`, JSON lists of `service/method` strings): the audio and apiadapter files were read and their methods found. The application manager's methods are declared in `com.webos.sam.api.json` (also in others), but the daemon only reads file names that contain `com.webos.applicationManager`. Harmless (the read-back guards the launch) and a one-line filter fix.
4. **No completion line for the refresh button.** An unchanged refresh logs only "reading the input and app lists"; whether the read succeeded is not visible at `info`.
5. **A power-off took 12.8 s once** (toggle-key fallback after `powerOn` was accepted but the state stayed in standby for 5 s); the previous D6 and section 13 runs were 5.3 to 8.1 s. The end state was correct both times. The darkener's margin before `Request Suspend` on this unit and firmware at that moment was not measured.
6. `tv_input` and `tv_app` flip together: launching an app makes `tv_input` `Other` and `tv_app` the app, as designed.

**Restored and left behind.** c3-a: volume 89, mute off, input `HDMI 1`, screen on, `Active`; the only change is the 0.2.0 install (P4). c3-b: volume 3, mute off, home app, power state `Active` after the runs and back to `Screen Saver` by itself within about 4 min, as in Part A; the temporary daemon, its config, key and logs removed from `/tmp`, no process left, no sink input. dmesg showed no oops, BUG or call trace on either TV (matched by grep only; no ASoC or debugfs path read). Raw outputs are in the gitignored `devices/private/*/EXP-010/`.

## Rollback and cleanup

- After each step: originals from step 0 restored and read back (volume, mute, sound output, foreground app, screen).
- Ladder if the TV does not answer: `turnOnScreen`, `powerOn {"reason":"remoteKey"}`, `networkinput/controls/power` (only in `Active Standby`), WoL from the PC, then the owner with the remote.
- Part B: stop the satellite, remove it from `/tmp`, delete the temporary HA entry and pipeline, revoke only the run's own tokens, select states restored.
- End state: `Active` on the home app, every original read back, nothing of the run left on the TV.

## Follow-ups / unresolved questions

- c3-a (`03.30.74`): the disruptive steps (screen, input, app, power-off) are still untested there (the TV is the owner's active display); mute, volume down and the refresh passed.
- Results go to the [c3-b compatibility record](../compatibility/c3-b.md), the [design](../design/tv-control.md) and backlog #11.
