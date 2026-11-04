# TV control from Home Assistant via the satellite

- **Status:** design, accepted direction ([ADR 0005](../decisions/0005-tv-control-entities.md)); **nothing implemented**. Backlog item [#11](../project/backlog.md#11-tv-control-from-home-assistant-via-the-satellite). Setters were run on **c3-b only** (firmware `03.30.73`) on 2026-11-04 in [EXP-010](../experiments/EXP-010-tv-control-setters.md) Part A: screen, mute, volume set and step, input switch and app launch all passed and restored (section 5). Everything there is **c3-b-only**: c3-a (`03.30.74`) is untested. Part B (through HA) is not run.
- **Date:** 2026-11-04. Owner decisions of 2026-11-04 are recorded in ADR 0005.
- **Devices in scope for TV facts:** c3-b (LG C3, webOS TV 8.3.0, firmware `03.30.73`) for the getters and the power path; c3-a (firmware `03.30.74`) is **untested** for every method in this document except the power-state subscription and the wake-and-darken path that run there in 0.1.0. A finding on one unit or firmware is not confirmation on another.
- **Related:** [always-on design](always-on.md) (sections 10 to 13), [D5 results](../discovery/c3-b/D5-results.md), [D6 results](../discovery/c3-b/D6-poweroff-screenoff.md), [ESPHome API crate](../development/esphome-api.md), [install and configuration UX](install-and-config-ux.md), [compatibility](../compatibility/README.md).

Labels follow [AGENTS.md](../../AGENTS.md): `documented`, `observed-on-other-device`, `hypothesis`, `verified-on-target` (always with the unit and firmware). "Declared" means the method name appears in the TV's own luna-service2 files; it says nothing about parameters or behaviour.

## 1. Scope and non-goals

**In scope (v1, owner 2026-11-04):**

1. Screen off and on, and a power-off that keeps the satellite running (section 4).
2. TV volume (set, step) and TV mute.
3. Input (source) selection.
4. App launch.
5. Read-only sensors that explain the state: TV power state (exists), sound output, optionally the media play state.
6. The listening indicator as HA entities only: the truthful `listening` and `microphone_muted` binary sensors tied to the real capture state (being added to the daemon in parallel, 2026-11-04; section 3.3).

**Non-goals (v1):**

- No toasts, notifications or on-screen indicators (owner 2026-11-04). No controllable status LED exists (research 2026-11-04).
- No real power-off into LG standby or Suspend from HA (section 4). No Wake-on-LAN from the satellite: a TV in Suspend cannot run it. Waking from Suspend stays HA's job (its `wake_on_lan` integration), outside this project.
- No TV settings writes: no picture or sound mode, no sound output change, no Always Ready, Quick Start+, energy saving or auto-off changes. The daemon's existing rule stands ([always-on section 7](always-on.md#7-exposing-power-options-in-the-ui-design)).
- No TV `media_player` entity: the satellite's media player keeps meaning "reply volume" (owner 2026-11-04).
- No channel control, no playback control (play/pause), no remote-key injection beyond the existing power key path.
- No replacement of HA's `webostv` integration. The satellite **complements** it (section 2).
- Audible timer and announcement verification: skipped for now (owner 2026-11-04).

## 2. Relationship to HA's `webostv` integration

**Decision: complement it** (owner 2026-11-04). The owner's HA has no `webostv` entry today.

What the satellite adds, and the evidence for each:

| Property | Satellite | Label |
| --- | --- | --- |
| No SSAP pairing or client key | The daemon runs as root and calls luna services directly with `luna-send`; the getters in section 5 answered as root with no pairing | verified-on-target (c3-b, `03.30.73`, 2026-11-04) |
| Works while the TV looks off | The daemon keeps running in `Screen Off` and `Active Standby` ([D5](../discovery/c3-b/D5-results.md), [D6](../discovery/c3-b/D6-poweroff-screenoff.md)) | verified-on-target (c3-b) |
| HA's integration and standby | HA documents that turning the TV on needs a Wake-on-LAN automation ([webostv docs](https://www.home-assistant.io/integrations/webostv/)); users report the TV drops the connection in standby ([always-on section 10.2](always-on.md#102-candidates-ranked)) | documented / observed-on-other-device. **Not checked on the owner's HA** (no entry) |
| Voice | The entities can be exposed to Assist like any HA entity (section 11) | design |

If an owner runs both, both control the same TV. Neither knows about the other; the TV's own state is the truth, and the satellite re-reads it (section 7), so it does not drift.

## 3. Entities

All entities belong to the satellite's existing ESPHome device. Object ids are stable (the entity key is a hash of the object id). Names are what HA shows after the device name; the owner can add Assist aliases in HA.

### 3.1 Entity table

| Entity (name) | Type | Object id | Range, options, unit | Listed when | Default | Category |
| --- | --- | --- | --- | --- | --- | --- |
| TV screen | `switch` | `tv_screen` | on = picture on, off = `Screen Off` | `[tv_control] screen` | on | none |
| Turn on TV | `button` | `tv_turn_on` | stateless | `[tv_control] power_on` | on | none |
| TV power (keep listening) | `switch` | `tv_power` | on = picture on, off = looks off but listening (section 4) | `[tv_control] power_off` | **off (opt-in)** | none |
| TV volume | `number` | `tv_volume` | 0 to `maxVolume` read from the TV (**60** on c3-b, read 2026-11-04; an earlier draft said 100), step 1, slider, **no unit** (LG volume steps, not percent); commands clamped to `volume_limit` | `[tv_control] volume` | on (EXP-010 Part A passed on c3-b; recommended, section 9) | none |
| TV volume up / TV volume down | `button` | `tv_volume_up`, `tv_volume_down` | one TV step each | `[tv_control] volume` | as `tv_volume` | none |
| TV mute | `switch` | `tv_mute` | on = muted | `[tv_control] mute` | on (EXP-010 Part A passed on c3-b) | none |
| TV input | `select` | `tv_input` | labels of the TV's external inputs, plus `Other` | `[tv_control] input` | on (EXP-010 Part A passed on c3-b) | none |
| TV app | `select` | `tv_app` | titles of user-visible launch points (section 8), plus `Other` | `[tv_control] apps` | on (EXP-010 Part A passed on c3-b) | none |
| Refresh TV inputs and apps | `button` | `tv_refresh_lists` | stateless | `input` or `apps` on | on | config |
| TV power state | `text_sensor` | `tv_power_state` | exists since 0.1.0 | power watch on | on | diagnostic |
| TV sound output | `text_sensor` | `tv_sound_output` | the TV's value, for example `external_arc`, `tv_speaker` | `volume` or `mute` on | on | diagnostic |
| TV play state | `text_sensor` | `tv_play_state` | the media service's `playState` value | `[tv_control] play_state` | off | diagnostic |
| Listening | `binary_sensor` | `listening` | on = capture PCM open | always (privacy) | on | none |
| Microphone muted | `binary_sensor` | `microphone_muted` | on = a gate keeps the PCM closed | always (privacy) | on | none |

`listening` and `microphone_muted` are defined by the parallel daemon change of 2026-11-04 (capture thread, `crates/satellite/src/capture.rs`); this design only requires their semantics (section 3.3). The existing "Mute microphone" switch, the reply-volume media player and the select "When the TV is turned off with the remote" (`remote_power_off`) are unchanged.

**Why separate entities** (owner 2026-11-04): a TV `media_player` would clash with the reply-volume media player, and separate entities each carry one truthful state. `Other` in the two selects is the state when the foreground app is not in the list; choosing `Other` from HA is refused (the state is sent again).

### 3.2 Availability rules

The entity **list** is fixed by the configuration (it changes only on a restart, or on a list refresh, section 8). ESPHome has no per-entity "unavailable" for a connected device; a state can only be "missing" (HA shows `unknown`). So:

1. A control whose capability check failed (section 6), or whose state cannot be read, reports **missing state** and refuses commands with one log line. It is never shown with a guessed value.
2. A control that does not apply in the current power state (table in 3.4) reports missing state and refuses commands.
3. When the daemon is frozen in Suspend or not running, HA marks the whole device unavailable once its connection times out (ESPHome keepalive; the exact delay on the owner's HA is not measured).
4. The power-off switch `tv_power` is not listed at all unless `power_off = true`, and reports missing state and refuses both directions while the runtime conditions of section 4.2 do not hold. The `tv_turn_on` button is the way back in every state where the daemon runs.

How HA 2026.10.4 renders `missing_state` for a `switch`, `number` and `select` (unknown versus unavailable) is to be confirmed in the fake-HA test (section 12.2); the binary sensor and number cases show `unknown` per the API crate's tests.

### 3.3 Listening indicator (owner decision 5)

HA entities only, no on-TV indicator. Required semantics (implemented by the parallel change):

- `listening` is on from the moment the capture PCM is open until it is closed again, published from the capture thread in order with the audio, so no audio can reach HA before it reads on. It is derived from the PCM, not from configuration or HA's mute switch.
- `microphone_muted` is on only while the PCM is closed **and** a gate keeps it closed (mute switch, kill switch, firmware guard, not paired).
- Both are listed always, not optional. They matter most in the states this design adds: a TV that looks off (`Screen Off`, `Active Standby`) with an open microphone ([always-on 10.4 item 5](always-on.md#104-recommendation)).

### 3.4 Behaviour per power state

States are the settled `state` (and `processing` while in transition) of the existing power-state subscription ([always-on section 13](always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b)).

| Entity | `Active`, `Screen Saver` | `Screen Off` | `Active Standby` | Transition (`processing` set) | `Suspend` |
| --- | --- | --- | --- | --- | --- |
| `tv_screen` | on; off command: `turnOffScreen` | off; on command: `turnOnScreen` | missing; refused | last settled state kept; commands refused (tvpowerd answers "busy", D6) | daemon frozen |
| `tv_turn_on` | no-op, logged | `turnOnScreen` | `powerOn {"reason":"remoteKey"}` after the 1.5 s key block | refused | daemon frozen |
| `tv_power` (if listed and enabled) | on; off command: section 4 | off; on command: `turnOnScreen` | off; on command: `powerOn` as above | kept; refused | daemon frozen |
| `tv_volume`, buttons, `tv_mute` | read and set | read and set (verified-on-target c3-b: volume and mute accepted and read back in `Screen Off`, EXP-010 A8) | missing; refused | kept; refused | daemon frozen |
| `tv_input`, `tv_app` | read and set | read; set allowed. Launch in `Screen Off` kept the power state `Screen Off` (c3-b A8); whether the panel lights was **not looked at** | missing; refused | kept; refused | daemon frozen |
| sensors | live | live | `tv_play_state` missing | live | daemon frozen |

Why refuse in `Active Standby`: the audio DSP input gain is near zero there ([D6 variant C](../discovery/c3-b/D6-poweroff-screenoff.md#variant-c-why-active-standby-is-silent)), nothing else is tested in that state, and the remote itself does not act there except the power key. After a resume from Suspend (the daemon's G2 detection) every getter is read again before any state is sent.

## 4. Power-off: which state, and the way back

### 4.1 The owner's rule and the chosen mapping

Owner, 2026-11-04: *support power off only if the satellite can keep running in that state (the always-on mode), and only if it is enabled.*

**Chosen mapping: an HA power-off is the remote's power-off in the "Keep listening (screen off)" mode.** The daemon sends the same key as the remote (`luna://com.webos.service.networkinput/controls/power`, the method the TV's own SSAP gateway maps `system/turnOff` to) and the existing wake-and-darken state machine (variant A, `darken.rs`) brings the TV to `Screen Off`. The end state is `Screen Off` on a running TV: the satellite keeps listening and replies stay audible.

Evidence:

| Claim | Label | Source |
| --- | --- | --- |
| The API key `controls/power` gives `Request Power Off` with reason `remoteKey`, the same sequence as the IR remote | verified-on-target (c3-b, `03.30.73`) | [D5 results](../discovery/c3-b/D5-results.md), [c3-b notes](../compatibility/c3-b.md#notes-and-redacted-evidence) |
| With daemon 0.1.0 and the select on "Keep listening (screen off)", 4 power-offs by the API key (from `Screen Saver` and `Active`, Always Ready off) ended in `Screen Off` 5.2 to 5.3 s after the key in 3 runs, 8.1 s in one | verified-on-target (c3-b) | [always-on section 13](always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b) |
| In the daemon-made `Screen Off` the satellite listens and 12 of 12 quiet beeps were audible at the TV mic; a 20 min dwell kept the process and HA link | verified-on-target (c3-b) | always-on section 13, [D6](../discovery/c3-b/D6-poweroff-screenoff.md) |
| `turnOnScreen` from `Screen Off` answered `Active` at once | verified-on-target (c3-b) | [D5 results](../discovery/c3-b/D5-results.md) |
| `powerOn {"reason":"remoteKey"}` from settled `Active Standby` (after the 1.5 s key block) reaches `Active` | verified-on-target (c3-b) | D6 variants A and B, always-on section 13 |
| An HA-initiated power-off goes through LG's own power-off sequence, so a playing app is handled as with the remote | **hypothesis** (D6 ran on the home app; content playback was never on) | EXP-010 Part B |

**Why not a plain `turnOffScreen` for power-off:** `Screen Off` keeps the running app's sound playing (LG: "Screen is turned off and only sound is played", [Energy Saving help](https://www.lg.com/au/support/product-help/CT20088015-1437132262154), documented for the menu entry). That is the right behaviour for the **screen** switch and the wrong one for "turn off the TV". Using the key path also keeps one behaviour for the remote and for HA.

**Why not a real power-off:** with Always Ready off the TV suspends and the daemon is frozen (network down about 10 to 20 s after the key); with Always Ready on it stays in `Active Standby`, where replies are not audible ([D5](../discovery/c3-b/D5-results.md), [always-on 11](always-on.md#11-result-on-c3-b-2026-11-03)). Neither meets the owner's rule.

### 4.2 Conditions for the power-off (all must hold, checked before the key is sent)

1. `[tv_control] power_off = true` (config, default false). Otherwise `tv_power` is not listed.
2. The select "When the TV is turned off with the remote" is on "Keep listening (screen off)" (the always-on mode is enabled). Otherwise `tv_power` reports missing state and refuses.
3. The power watch runs and the settled state is `Active` or `Screen Saver`. The key is a toggle: sent from `Screen Off` it is a "second press" and wakes the TV lit ([D6 S1](../discovery/c3-b/D6-poweroff-screenoff.md#s-second-press-and-the-power-key-in-screen-off)), so it is never sent there; an off command in `Screen Off` or `Active Standby` is a no-op.
4. The darkener would act: kill switch absent, not in its cooldown (20 s), hourly budget left (10 per hour). The HA power-off counts against the same budget. If the darkener would refuse, the daemon refuses the power-off instead of sending a key that ends in LG standby or Suspend.

**Residual risk (labelled):** if the wake fails after the key was sent (for example `powerOn` refused twice and the toggle-key fallback also fails), the TV ends where LG puts it: `Active Standby` with Always Ready on, `Suspend` with it off. The margin before `Request Suspend` was about 2 s in one D6 run (A4) and held in the 4 section 13 runs; on other units or firmware it is unknown. The power-state sensor then shows the real state, and the satellite logs it. This is the same risk the remote key already carries in this mode.

### 4.3 The way back

| From | Action | Label |
| --- | --- | --- |
| `Screen Off` | `turnOnScreen` (`tv_turn_on`, `tv_power` on, or `tv_screen` on) | verified-on-target (c3-b, D5) |
| `Active Standby` | `powerOn {"reason":"remoteKey"}` 1.5 s or more after the settle, one retry after `Keyblock1.5second`; the toggle key only as the existing state-checked fallback | verified-on-target (c3-b, D6, section 13) |
| `Suspend` | not reachable by the satellite (frozen). HA's own Wake-on-LAN, or the remote | WoL wake verified on c3-b with `wolwowlOnOff = true` ([D5](../discovery/c3-b/D5-results.md)) |
| Any state | the remote's power key | verified-on-target (c3-b) |

A power-on from HA is not a power-off, so the darkener does not act on it. The daemon tells the darkener that the wake is its own, so the darkener's "screen dark by us" bookkeeping stays right.

## 5. Interface per control

All calls are `luna-send -n 1 -w 3000 <uri> <payload>` as root (the existing `power::call`). Getters were read on c3-b on 2026-11-04 (read-only check by an agent, raw output not committed); **the setters were run on c3-b on 2026-11-04 (EXP-010 Part A)**; the rows say which. The gateway interface files map methods but carry no parameters; the parameters below come from the apiadapter's own JavaScript on the TV (read-only), or, for `setVolume`, from what aiowebostv sends over SSAP (`observed-on-other-device`), and every one was confirmed by the call. luna-send over a plain SSH pipe printed nothing for the audio methods: a pseudo-terminal is needed (the daemon already runs `luna-send` on a pty for the subscription).

| Control | Read | Label (read) | Write | Label (write) |
| --- | --- | --- | --- | --- |
| Power state | `com.webos.service.tvpower/power/getPowerState {"subscribe":true}` (existing child) | verified-on-target (c3-b, c3-a) | n/a | |
| Screen | power state (`Screen Off` versus `Active`) | as above | `tvpower/power/turnOffScreen {}`, `tvpower/power/turnOnScreen {}` | verified-on-target (c3-b): off from `Active` and at the settled `Active` after a wake (refused at the `Screen On` substate once); on from `Screen Off`. Off from `Screen Saver`: verified-on-target (c3-b, EXP-010 A1, twice, 80 ms, then `turnOnScreen` to `Active`) |
| Power-off | power state | as above | `com.webos.service.networkinput/controls/power {}` then variant A | verified-on-target (c3-b, daemon 0.1.0) |
| Power-on | power state | as above | `tvpower/power/powerOn {"reason":"remoteKey"}` or `turnOnScreen` | verified-on-target (c3-b) |
| Volume, mute, sound output | `com.webos.service.audio/master/getVolume {}` → `volumeStatus{volume, muteStatus, maxVolume, soundOutput}`; also `{"subscribe":true}` pushes one reply per change | verified-on-target (c3-b, 2026-11-04) | **Set:** `master/setVolume {"volume":N}`, reply `{returnValue, volume, soundOutput:""}`, 30 ms. **Step:** `master/volumeUp {}` and `master/volumeDown {}`, one step each, 20 to 30 ms. **Mute:** `apiadapter/audio/setMute {"mute":bool}` or `audio/master/muteVolume {"mute":bool}`, reply `{returnValue, muteStatus, soundOutput}`, 30 ms. All read back correctly and restored, also in `Screen Off` | verified-on-target (c3-b `03.30.73`, 2026-11-04, `soundOutput = external_arc`, volume 3 to 4 and back, nothing playing). **c3-b-only.** TV speaker output (A5) not run; whether ARC volume reaches the soundbar is still a hypothesis |
| Sound output (detail) | `com.webos.service.apiadapter/audio/getSoundOutput {}` | verified-on-target (c3-b, 2026-11-04) | none in v1 | |
| Inputs | `com.webos.service.apiadapter/tv/getExternalInputList {}` → `devices[{id, label, connected, appId, subList}]` | verified-on-target (c3-b, 2026-11-04) | `apiadapter/tv/switchInput {"inputId":"<devices[].id>"}` → `{returnValue:true}`, 440 ms; unknown id → `returnValue:false, errorCode:-1000, errorText:"no such input"` | verified-on-target (c3-b, 2026-11-04) for a **disconnected** HDMI input (foreground app became that input's `appId`; restored by launching the home app). A connected source was not switched to (it could make sound). **c3-b-only** |
| Foreground app | `com.webos.applicationManager/getForegroundAppInfo {}` → `{appId, ...}` | verified-on-target (c3-b, 2026-11-04) | n/a | |
| Apps | `com.webos.applicationManager/listLaunchPoints {}` (about 35 KB on c3-b, 43 entries) | verified-on-target (c3-b, 2026-11-04) | `com.webos.applicationManager/launch {"id": <appId>}` → `{returnValue:true}`, 130 ms; unknown id → `returnValue:false, errorCode:-101, errorText:"not exist"`. Also `apiadapter/system_launcher/launch {"id":...}` (adds a `sessionId`) | verified-on-target (c3-b, 2026-11-04) for the satellite's own app ([EXP-009](../experiments/EXP-009-ipk-install-update.md)) **and** for a built-in silent app (foreground matched, restored). **c3-b-only** |
| Play state | `com.webos.media/getForegroundAppInfo {}` → `playState` | verified-on-target (c3-b, 2026-11-04) | n/a | |

**Never called:** `com.webos.service.apiadapter/system/getSystemInfo` (its reply carries the serial number), `tvpower/power/powerOff`, `power2/*`, the tvpower request registration methods, `micomservice/*`, any settings setter (`setSystemSettings`), `/dev/uinput`. c3-a (`03.30.74`): none of the getters or setters in this table has been called there except the power ones.

## 6. Capability detection at startup

Per control, after the power watch delivered its first state and before the ESPHome server lists entities (total budget 10 s; anything not answered in time counts as failed):

1. **Config:** the control's flag is on, and `luna-send` was found (on a PC every TV control is off, as the power watch is today).
2. **Getter:** one read-only call. Success means `returnValue: true` and the fields this design uses are present with the expected types. The answer seeds the state.
3. **Setter declared:** the method name is present in the TV's luna-service2 API declaration files (read-only file read, no call). A missing declaration means the control is read-only.
4. **Subscription:** for volume and foreground app, the getter with `{"subscribe":true}` is started as a supervised `luna-send -i` child; if its first reply lacks `subscribed: true`, the control falls back to polling (section 7).
5. **Never a probe write.** Setters are not called at startup, not even with the current value.

Outcomes: a failed getter makes the entity report missing state; a missing setter makes it refuse commands; both are logged once at `warn` with the control name and the luna `errorCode`/`errorText` (no reply body). A setter that fails at run time (`returnValue: false`, a timeout, or a read-back that does not match) is logged once, reported as missing state and disabled until the next restart or list refresh. **No fallback to another, guessed method or parameter set.** The only fallback chain is the existing, verified one in the power path (`powerOn`, then the state-checked toggle key).

## 7. Polling, subscriptions and rate limits

| Data | Mechanism | Rate |
| --- | --- | --- |
| Power state | existing subscription | push |
| Volume, mute, sound output | `getVolume` subscription if supported, else poll | push, or every 10 s in `Active`, `Screen Saver`, `Screen Off`; none in `Active Standby` |
| Foreground app (drives `tv_app` and `tv_input`) | `getForegroundAppInfo` subscription if supported, else poll | push, or every 5 s in the same states |
| Play state (if enabled) | poll | every 10 s in `Active` and `Screen Saver` |
| Input list, launch points | read once | startup, after a resume, on `tv_refresh_lists` |
| After any setter | one read-back | 0.5 s after the reply |

At most three extra `luna-send -i` children (volume, foreground app, and the existing power watch makes four in all), each supervised with the power watch's backoff (1 s doubling to 60 s) and killed on shutdown. Their CPU cost is measured in EXP-010 Part B.

Setter limits: one call in flight per control; volume set commands from an HA slider are coalesced (latest value wins after 250 ms); volume up and down at most 4 per second; input and app at most one per 2 s; all TV-control setters together at most 30 per minute. A command above a limit is refused with one log line, and the entity's real state is sent again. The power path keeps its own limits (section 4.2).

`volume_limit` (default 40) clamps a volume set and stops `volume up` at the limit; the entity shows the value read back from the TV, so a clamp is visible. The remote is not limited. Reason: one misheard number or a slider slip from a phone should not set the TV to full volume; c3-a and c3-b were used at TV volume 9 to 12 in the tests.

## 8. Input and app lists

**Inputs (`tv_input`):** all entries of `getExternalInputList` in the TV's order, connected or not, by `label`; duplicate labels get the input `id` appended. The state is the input whose `appId` equals the foreground app, else `Other`. Selecting an input calls the input setter with that entry's `id` (parameter name from EXP-010). `subList` is ignored in v1.

**Apps (`tv_app`):** from `listLaunchPoints`:

1. Keep only fields `title` and the app id; drop the rest of the reply at once (icons, paths and other data are never stored or logged).
2. Drop entries the TV launcher does not show (on c3-b `03.30.73` the launch-point entries have no `visible` or `hidden` field; the 24 keys seen are `mediumLargeIcon`, `bgColor`, `installTime`, `systemApp`, `appDescription`, `launchPointId`, `bgImages`, `lptype`, `relaunch`, `favicon`, `icon`, `id`, `bgImage`, `largeIcon`, `removable`, `iconColor`, `tileSize`, `userData`, `params`, `unmovable`, `extraLargeIcon`, `imageForRecents`, `miniicon`, `title`; so keep all, and note that `org.webosbrew.*` helper apps, an app id listed twice and settings-style apps appear: the owner's filter is the app list limit and the exclude rules below), the satellite's own app, and the input apps (the `appId`s from `getExternalInputList`, matched exactly, not by name patterns).
3. Keep the launcher order, de-duplicate titles (append a counter), and cap at `app_limit` (default 30) options.
4. The state is the title of the foreground app if listed, else `Other`.

**Size:** the reply is about 35 KB on c3-b; it is parsed once with a 1 MB cap (a larger reply is refused and logged), and only a few KB are kept.

**Refresh:** select options are sent in the entity list, which HA reads when it connects. A change of options (an app installed or removed) is applied by `tv_refresh_lists`, at startup and after a resume: the daemon re-reads both lists and, only if they changed and no voice pipeline is running, closes the HA connection so HA reconnects and lists the entities again (that HA lists entities on every connection is to be confirmed in the fake-HA test). No periodic refresh in v1.

## 9. Configuration

New section in `config/example.toml` (design; not added until the implementation):

```toml
[tv_control]
enabled = true        # master switch; off by itself without luna-send (PC)
screen = true         # tv_screen switch
power_on = true       # tv_turn_on button
power_off = false     # tv_power switch: opt-in, needs the "Keep listening" mode
volume = false        # tv_volume, tv_volume_up/down (EXP-010 Part A passed on c3-b: may become true)
volume_limit = 40     # highest TV volume HA may set
mute = false          # tv_mute (passed on c3-b: may become true)
input = false         # tv_input (passed on c3-b: may become true)
apps = false          # tv_app (passed on c3-b: may become true)
app_limit = 30        # maximum apps in the tv_app select
play_state = false    # tv_play_state sensor
```

| Key | Default | Why |
| --- | --- | --- |
| `enabled` | true | Read-only state is harmless and makes the controls discoverable; the per-control flags gate every write |
| `screen`, `power_on` | true | Their calls are verified on c3-b (D5, D6, section 13), they keep the satellite running, and any remote key undoes them |
| `power_off` | **false** | Owner decision: opt-in, and only in the always-on mode. Costs: TV stays running dark (power draw not measured), LG's OLED care at power-off may not run, a brief panel flash, a second remote press takes about 5 s ([always-on 12.2](always-on.md#122-the-setting-to-expose-app-and-ha-ui)) |
| `volume`, `mute`, `input`, `apps` | false in the code until the owner flips it; **EXP-010 Part A passed for all four on c3-b `03.30.73` (2026-11-04), so the recommended default is true** | The setters worked, read back and restored. The evidence is c3-b only; c3-a (`03.30.74`) is untested and a failed setter is detected at run time (section 6) and disabled. The `volume_limit` clamp stays. Two caveats: volume and mute were tested on `external_arc` only, and input only on a disconnected HDMI input |
| `volume_limit` | 40 | Section 7 |
| `app_limit` | 30 | Keeps the select usable in HA and in voice |
| `play_state` | false | Extra polling for a sensor no v1 feature needs |

Every control is also gated at run time by section 6. The firmware guard (`safety.validated_firmware`) blocks the microphone only; it does not gate TV control, because the getters run on any firmware and a failed method is detected (section 6). Whether TV control should also follow the guard on an unverified firmware is an open question (section 14).

## 10. Safety

1. **User-initiated only.** A setter runs only in response to an HA command on an entity the owner enabled. The daemon never changes volume, input, app or screen by itself. The only automatic action stays the existing, opt-in wake-and-darken on a remote power-off.
2. **No silent settings writes.** v1 writes no TV setting. Volume and mute are the normal user controls (the TV keeps them as the remote would).
3. **Read back every write** (section 6); a mismatch is reported, never retried with other parameters.
4. **Never a write at startup**, not even a probe.
5. **Risk per action:**

| Action | Risk | Reversal |
| --- | --- | --- |
| Screen off | Low. The TV stays running: more power than standby (not measured), and if left dark for days LG's power-off OLED care does not run | `tv_screen` on, `tv_turn_on`, any remote key |
| Power-off (keep listening) | Medium. As screen off plus a 0.3 to 0.7 s panel flash (indirect, [D6](../discovery/c3-b/D6-poweroff-screenoff.md)); if the wake fails, LG standby or Suspend (section 4.2) | `tv_turn_on`, the remote; WoL from Suspend |
| Power-on | Low. Lights the panel in a room that may be dark | screen off or the remote |
| Volume set and step | Low to medium: loudness, capped by `volume_limit` | set it back; the remote |
| Mute | Low | unmute; the remote |
| Input and app | Low: interrupts what is being watched; an app may start playing sound | select the previous one; the remote |

6. **The kill switch** blocks the power-off (as it blocks the darkener). Other TV controls do not use the microphone, so the kill switch and the mic mute do not block them.
7. **No new listening port**: everything goes over the existing ESPHome connection, which needs the paired key.

## 11. Voice

Voice works through HA, not through the satellite: the owner exposes the entities to Assist in HA (Settings, Voice assistants, Expose) and may add aliases ("TV", "television"). Expected behaviour, to be confirmed on the owner's HA 2026.10.4:

- `switch` entities (`tv_screen`, `tv_power`, `tv_mute`) work with HA's built-in turn on and turn off intents (documented for switches in HA's intents).
- `button` entities (`tv_turn_on`, volume up and down): HA's turn-on intent presses buttons in recent versions (**hypothesis**, not re-read for 2026.10.4).
- `number` and `select` (`tv_volume`, `tv_input`, `tv_app`): no built-in intent is known to set them (**hypothesis**). Options: HA scripts exposed to Assist ("set the TV volume to N"), custom sentences, or an LLM conversation agent that can call entity actions. The Kiwi pipeline's agent is a separate pending decision ([backlog #12](../project/backlog.md#12-conversation-agent-for-the-kiwi-pipeline)).
- Which entities HA exposes to Assist by default when they first appear depends on HA's "expose new entities" setting (not checked on the owner's HA). The owner decides what is exposed.

No HA change is made by this design.

## 12. Module separation and test plan

### 12.1 Structure

Following [AGENTS.md](../../AGENTS.md) (keep capture, playback, power lifecycle and media control separable):

- **New crate `crates/tv-control` (`webos-tv-control`)**, sans-IO: luna reply parsers, the capability model, entity definitions, the per-power-state availability rules, the command-to-call mapping, list filtering and the rate limits. Input: power-state updates, luna replies, HA commands. Output: luna calls to make, entity states to send, log events. No tokio, no process spawning, no knowledge of capture, voice or audio. JSON with `serde_json` (pure Rust, MIT or Apache-2.0; added to the dependency audit in [packaging section 8](packaging.md#8-license-decision-and-dependency-audit)).
- **Daemon adapter** (`crates/satellite/src/tv_control.rs`): runs the calls with the existing `power::call` and supervised `luna-send -i` children, routes `tv_*` entity events, and asks `darken.rs` whether a power-off is allowed (section 4.2).
- **ESPHome API crate:** uses the `number`, `button` and `binary_sensor` types being added on 2026-11-04. Still needed: a generic `switch` (with missing state; today only the mute switch exists), generic `text_sensor`s beyond the power state, and select states with missing state.

### 12.2 Tests

1. **Unit tests (host, `cargo test -p webos-tv-control`)** with **synthetic** luna JSON fixtures written by hand from the field names seen on c3-b (no raw replies committed; labels such as `HDMI 1`, titles such as `App A`): `getVolume` (muted, ARC, missing fields, `returnValue: false` with `errorCode`), input list (connected and not, duplicate labels, empty), launch points (filter, hidden, own app, input apps, de-duplication, cap, a generated 35 KB and an over-1 MB reply), foreground app (input app, listed app, unknown app), malformed JSON. Controller: availability per power state (table 3.4), the power-off conditions of 4.2 one at a time, the volume clamp, coalescing and limits, read-back mismatch disables the control, `Other` refused, and a log-capture test that no app title, app id, input label or reply body appears at `info`.
2. **PC fake-HA test** (`tools/esphome-fake-ha/`): the real daemon on 127.0.0.1 with a fake `luna-send` script that serves the fixtures, scripts power-state sequences (`Active`, `Screen Off`, `Active Standby`, transitions) and records every setter call to a file. An aioesphomeapi client checks the entity list (types, object ids, options, categories), missing states, each command and the recorded luna calls, refused commands, the power-off with the select on and off, the list refresh and re-listing on reconnect, and the log policy. Also one run against a local HA 2026.10.4 to see how a missing state shows for a switch, number and select.
3. **On c3-b:** [EXP-010](../experiments/EXP-010-tv-control-setters.md): Part A (raw setters over SSH) ran on 2026-11-04 and passed; Part B (through HA) needs the implementation and a separate go. Nothing on c3-a until the owner agrees.

## 13. Privacy

- The foreground app, the input and the play state describe viewing habits. They go only to the owner's HA, over the encrypted ESPHome connection. They are not logged at `info`: log lines name the control and the result ("tv_app: launch accepted, read back matches"); titles, app ids and input labels appear at `debug` only, and raw luna replies never (length only).
- No identifiers are read: no `getSystemInfo`, no serial, MAC or account data. Launch-point data beyond title and app id is dropped on parse.
- The listening indicator (section 3.3) is the privacy signal for a TV that looks off; no on-TV indicator in v1 (owner 2026-11-04).

## 14. Open questions

1. Still open after EXP-010 Part A (c3-b): volume and mute on the TV speaker output (A5, not authorized); whether an on-screen volume bar or mute icon appears; switching to a **connected** source; whether an app launch or volume change in `Screen Off` lights the panel (only the power state was read); behaviour with content playing; every setter on c3-a.
2. Whether an HA power-off with an app playing stops its sound (EXP-010 Part B).
3. How HA 2026.10.4 renders a missing state for switch, number and select, and whether it lists entities on every reconnect (fake-HA test).
4. Which Assist intents act on buttons, numbers and selects on the owner's HA (section 11).
5. Whether TV control should follow the firmware guard on firmware not in `validated_firmware` (owner).
6. c3-a (`03.30.74`): all getters and setters untested.
