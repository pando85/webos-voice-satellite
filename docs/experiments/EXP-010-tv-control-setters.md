# EXP-010 — TV control setters on c3-b (screen, volume, mute, input, app, power)

**Status:** Part A RUN on c3-b, 2026-11-04 (owner's go the same day, "can be run at hall-tv now"); step A5 skipped (not authorized); Part B NOT RUN (the implementation does not exist on a TV yet).
**Date:** written and Part A run 2026-11-04.
**Device alias / model / firmware / webOS / architecture:** c3-b, LG C3 55 inch, firmware `03.30.73`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland. **Not c3-a** (c3-a is in daily use; a separate go is needed after c3-b passes).
**Power and playback state:** `Active` or `Screen Saver` on the home app, nothing playing, at the start of every step; `Screen Off` and `Active Standby` only where a step says so.
**Related research / requirements / issue:** [TV control design](../design/tv-control.md), [ADR 0005](../decisions/0005-tv-control-entities.md), [backlog #11](../project/backlog.md#11-tv-control-from-home-assistant-via-the-satellite), [D5](../discovery/c3-b/D5-results.md), [D6](../discovery/c3-b/D6-poweroff-screenoff.md).
**Risk:** user-level TV state changes (screen, volume, mute, input, foreground app, power key in the keep-listening mode). No TV settings menu writes, no firmware, no persistence, no reboot. Sound: only the TV's own volume changes by one or two steps on the home screen (no content playing), plus at most one short announcement in Part B.
**Approval needed / obtained:** the owner's go for Part A on c3-b was given on 2026-11-04 with these limits: no audio, A5 not authorized, every changed value restored right after, no reboot, no install. Part B needs a separate go once the implementation exists.

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

## Rollback and cleanup

- After each step: originals from step 0 restored and read back (volume, mute, sound output, foreground app, screen).
- Ladder if the TV does not answer: `turnOnScreen`, `powerOn {"reason":"remoteKey"}`, `networkinput/controls/power` (only in `Active Standby`), WoL from the PC, then the owner with the remote.
- Part B: stop the satellite, remove it from `/tmp`, delete the temporary HA entry and pipeline, revoke only the run's own tokens, select states restored.
- End state: `Active` on the home app, every original read back, nothing of the run left on the TV.

## Follow-ups / unresolved questions

- c3-a (`03.30.74`): the same procedure, only after c3-b passes and on a separate go.
- Results go to the [c3-b compatibility record](../compatibility/c3-b.md), the [design](../design/tv-control.md) and backlog #11.
