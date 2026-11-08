# TV control from Home Assistant via the satellite

- **Status:** design, accepted direction ([ADR 0005](../decisions/0005-tv-control-entities.md)); **implemented in 0.2.0 (2026-11-04) and run on both TVs**: crate `webos-tv-control`, daemon adapter `crates/satellite/src/tv_control.rs`, generic `switch` and `text_sensor` entities in the ESPHome API crate, unit tests with synthetic fixtures, fake-HA and satellite end-to-end scenarios with a fake `luna-send` (see [section 15](#15-implementation-status-2026-11-04)). EXP-010 Part B ran the same day: c3-b (disruptive controls through an API client) and c3-a (entity list, mute, volume down, refresh and restart through the owner's HA); results in the [EXP-010 Part B results](../experiments/EXP-010-tv-control-setters.md#part-b-results-2026-11-04). Backlog item [#11](../project/backlog.md#11-tv-control-from-home-assistant-via-the-satellite). Setters were run on **c3-b only** (firmware `03.30.73`) on 2026-11-04 in [EXP-010](../experiments/EXP-010-tv-control-setters.md) Part A: screen, mute, volume set and step, input switch and app launch all passed and restored (section 5). On **c3-a** (`03.30.74`) only the entity list, mute, volume down and the refresh were verified (the TV is the owner's active display); screen, input, app and power-off are c3-b-only.
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
- No TV settings writes: no picture or sound mode, no sound output change, no Quick Start+, energy saving or auto-off changes. The daemon's existing rule stands ([always-on section 7](always-on.md#7-exposing-power-options-in-the-ui-design)). **Exception (owner 2026-11-05, [ADR 0008](../decisions/0008-user-initiated-lg-settings.md)):** the TV app, not the daemon and not HA, may change Always Ready and LG hands-free on an explicit press ([Voice and power screen](voice-and-power-screen.md)); HA gets two read-only sensors for them.
- ~~No TV `media_player` entity~~ Changed the same day by [ADR 0007](../decisions/0007-tv-media-player.md): a second, control-only media player "TV" is added (section 3.5). The satellite's own media player keeps meaning "reply volume" (owner 2026-11-04).
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
| TV | `media_player` | `tv_media_player` | control only: volume set and step, mute, turn on and off (section 3.5) | `[tv_control] media_player` and any of its controls | on (ADR 0007) | none |
| TV screen | `switch` | `tv_screen` | on = picture on, off = `Screen Off` | `[tv_control] screen` | on | none |
| Turn on TV | `button` | `tv_turn_on` | stateless | `[tv_control] power_on` | on | none |
| TV power (keep listening) | `switch` | `tv_power` | on = picture on, off = looks off but listening (section 4) | `[tv_control] power_off` | **off (opt-in)** | none |
| TV volume | `number` | `tv_volume` | 0 to `maxVolume` read from the TV (**60** on c3-b, read 2026-11-04; an earlier draft said 100), step 1, slider, **no unit** (LG volume steps, not percent); the slider ends at `maxVolume`, or at the optional `volume_limit` when that is lower | `[tv_control] volume` | on (EXP-010 Part A passed on c3-b; recommended, section 9) | none |
| TV volume up / TV volume down | `button` | `tv_volume_up`, `tv_volume_down` | one TV step each | `[tv_control] volume` | as `tv_volume` | none |
| TV mute | `switch` | `tv_mute` | on = muted | `[tv_control] mute` | on (EXP-010 Part A passed on c3-b) | none |
| TV input | `select` | `tv_input` | labels of the TV's external inputs, plus `Other` | `[tv_control] input` | on (EXP-010 Part A passed on c3-b) | none |
| TV app | `select` | `tv_app` | titles of user-visible launch points (section 8), plus `Other` | `[tv_control] apps` | on (EXP-010 Part A passed on c3-b) | none |
| Refresh TV inputs and apps | `button` | `tv_refresh_lists` | stateless | `input` or `apps` on | on | config |
| TV power state | `text_sensor` | `tv_power_state` | exists since 0.1.0 | power watch on | on | diagnostic |
| TV sound output | `text_sensor` | `tv_sound_output` | the TV's value, for example `external_arc`, `tv_speaker` | `volume` or `mute` on | on | diagnostic |
| TV play state | `text_sensor` | `tv_play_state` | the media service's `playState` value | `[tv_control] play_state` | off | diagnostic |
| Always Ready | `text_sensor` | `lg_always_ready` | `Off`, `On`, `On with wallpaper` or `Other` (an unknown word is only in the log); from `settingsservice/getSystemSettings` `general/lifeOnScreenMode`; read-only ([ADR 0008](../decisions/0008-user-initiated-lg-settings.md), [Voice and power screen 7](voice-and-power-screen.md#7-home-assistant-two-read-only-diagnostic-sensors)) | `[tv_control] lg_settings` | on | diagnostic |
| LG hands-free | `text_sensor` | `lg_hands_free` | `Off`, `On` or `Other`; from `voiceinput/setting/getActivation` (on when the top level or a provider reports `settingStatus: true`); read-only | `[tv_control] lg_settings` | on | diagnostic |
| Listening | `binary_sensor` | `listening` | on = capture PCM open | always (privacy) | on | none |
| Microphone muted | `binary_sensor` | `microphone_muted` | on = a gate keeps the PCM closed | always (privacy) | on | none |

`listening` and `microphone_muted` are defined by the parallel daemon change of 2026-11-04 (capture thread, `crates/satellite/src/capture.rs`); this design only requires their semantics (section 3.3). The existing "Mute microphone" switch, the reply-volume media player and the select "When the TV is turned off with the remote" (`remote_power_off`) are unchanged.

**Why separate entities** (owner 2026-11-04): a TV `media_player` would clash with the reply-volume media player, and separate entities each carry one truthful state. They stay; since [ADR 0007](../decisions/0007-tv-media-player.md) a second, control-only media player sits next to them for HA's built-in voice intents (section 3.5). `Other` in the two selects is the state when the foreground app is not in the list; choosing `Other` from HA is refused (the state is sent again).

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
- `microphone_muted` is on only while the PCM is closed **and** a gate keeps it closed (mute switch, kill switch, the strict firmware guard, a failed mic self-check, not paired).
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

The TV media player (section 3.5) follows the same table: `off` in `Screen Off` and `Active Standby`, `on` or `playing` in `Active` and `Screen Saver`, held during a transition.

Why refuse in `Active Standby`: the audio DSP input gain is near zero there ([D6 variant C](../discovery/c3-b/D6-poweroff-screenoff.md#variant-c-why-active-standby-is-silent)), nothing else is tested in that state, and the remote itself does not act there except the power key. After a resume from Suspend (the daemon's G2 detection) every getter is read again before any state is sent.

### 3.5 The TV media player (ADR 0007)

Owner decision 2026-11-04 ([ADR 0007](../decisions/0007-tv-media-player.md)), after [P4](../discovery/c3-b/P4-sound-session.md) showed that HA's Spanish volume and mute sentences only target `media_player` entities. Implemented on the PC 2026-11-04; **not yet run on a TV or against the owner's HA**.

**Entity.** A second ESPHome `media_player` of the satellite device: name "TV" (HA shows "<device> TV"), object id `tv_media_player`, key the FNV-1 hash of it, listed after the satellite's own player. The satellite's player (object id `media_player`, key 2232357057, the reply volume and the `ANNOUNCEMENT` format) is unchanged. The owner adds the Assist aliases in HA ("tele del despacho" on c3-a, "tele del hall" on c3-b) and exposes it to Assist.

**Features** (HA `MediaPlayerEntityFeature` bits, each only while its control is enabled):

| Feature | Bit | Enabled by | Action |
| --- | --- | --- | --- |
| `VOLUME_SET` | 4 | `volume` | `master/setVolume` with `round(share × maxVolume)`; coalesced, clamped by `volume_limit` (a set above the cap is clamped and logged), read back |
| `VOLUME_STEP` | 1024 | `volume` | `master/volumeUp` / `volumeDown`: one TV step, as the remote and the step buttons; up refused at the cap; 4 per second |
| `VOLUME_MUTE` | 8 | `mute` | `apiadapter/audio/setMute`, read back |
| `TURN_ON` | 128 | `power_on` | as the `tv_turn_on` button: `turnOnScreen` from `Screen Off`, `powerOn` from `Active Standby`, nothing when lit |
| `TURN_OFF` | 256 | `screen` or `power_off` | `power_off` enabled **and** the select on "Keep listening (screen off)": the power-off of section 4 with all of its preconditions (a refusal stays a refusal, no fallback); otherwise `turnOffScreen`. Nothing in `Screen Off` or `Active Standby` |

Never offered: `PLAY_MEDIA`, `MEDIA_ANNOUNCE`, `BROWSE_MEDIA`, `MEDIA_ENQUEUE`, `PLAY`, `PAUSE`, `STOP`, `SELECT_SOURCE`. It advertises **no audio format**, so HA 2026.10.4's assist satellite keeps taking the TTS and announcement format from the satellite's player (the first `ANNOUNCEMENT` format of any player of the device), and announcements keep going through `VoiceAssistantAnnounceRequest`. HA's `media_player.play_media` on it is refused by HA (feature missing); a media URL that reaches the device anyway is ignored with a log line (`tv_media_player: media or announcement refused`).

**Source (input) selection is not on the media player.** The ESPHome API (`api.proto` 2026.10.1) has no source list or source command, and HA 2026.10.4's ESPHome platform drops `SELECT_SOURCE` (`_UNSUPPORTED_FEATURES`). Input stays the `tv_input` select.

**State.** `playing` when lit (`Active`, `Screen Saver`) and the media service's `playState` is `playing`; `on` when lit otherwise (or the play state getter failed); `off` in `Screen Off` and `Active Standby`. Volume: the TV volume over its `maxVolume` (12 of 60 is 0.2), in `Active`, `Screen Saver` and `Screen Off`; NaN when not known (HA shows no level). Mute where the volume getter is live, else `false`. A transition or an unknown power state is held: the ESPHome media player has no "unknown" state (HA 2026.10.4 maps only idle, playing, paused, off and on), so HA shows unknown only until the first state. The play state is polled every 10 s while lit (section 7) whenever the media player is listed, and read once when the TV is lit again after `Screen Off`.

**Same safety as the other entities:** the startup capability check, the declaration check, rate limits, the read-back and the disable-on-failure of section 6, and the firmware policy of section 9. Every media player command is logged at `info` by its name only (`tv_media_player: volume set`); refusals name the control (`tv_volume: refused: ...`).

**Voice (expected, to verify on the owner's HA 2026.10.4):** "baja/sube el volumen de la tele del hall" (`HassSetVolumeRelative`: one TV step), "pon el volumen de la tele del hall a 20" (`HassSetVolume`: 20 % of `maxVolume`, 12 of 60), "silencia/quita el silencio de la tele del hall" (`HassMediaPlayerMute`/`Unmute`), "apaga/enciende la tele del hall" (`HassTurnOff`/`HassTurnOn`). An unnamed "baja el volumen" in the satellite's area matches both media players; HA then keeps only the `playing` ones (HA source), so it reaches the TV while it plays and fails otherwise. A percentage is a share of `maxVolume` as the TV reports it, not a TV volume value: on c3-b (`maxVolume` 60) "al 50" sets 30. How that compares with the number the TV shows on screen was not looked at.

## 4. Power-off: which state, and the way back

**Result on the TVs and the owner's HA 2026.10.4 (0.2.3, 2026-11-04, text through `conversation/process`, agent `conversation.home_assistant`, language `es`; the satellite device as context; each read back with `luna-send`).** Label: verified-on-target (c3-a `03.30.74`, c3-b `03.30.73`) with HA 2026.10.4. HA aliases: "tele del despacho" and "televisión del despacho" (c3-a), "tele del hall" and "televisión del hall" (c3-b) on the TV media player; the screen switch keeps only "pantalla de la tele del ..." and "pantalla del ...". Entity: features 1420 (VOLUME_SET, VOLUME_MUTE, TURN_ON, TURN_OFF, VOLUME_STEP), no play, announce or stop; state `on` (c3-a, `Active`, no foreground media), `playing` (c3-b `Screen Saver`: the screen saver reports `playState` `playing`), `off` (c3-b `Screen Off`, volume level hidden by HA), `unavailable` while the TV is suspended.

| Sentence | TV | HA intent and target | Result on the TV (luna) |
| --- | --- | --- | --- |
| "baja el volumen de la tele del despacho" | c3-a | relative volume, the TV player | 89 to 88 |
| "sube el volumen de la tele del despacho" | c3-a | same | 88 to 89 |
| "silencia la tele del despacho" | c3-a | mute, the TV player | `muteStatus true` |
| "quita el silencio de la tele del despacho" | c3-a | unmute, the TV player | `muteStatus false`, volume 89 |
| "baja el volumen" (unnamed, satellite context) | c3-a | `no_valid_targets` (the TV player is `on`, the satellite player `idle`; HA keeps only `playing` ones) | nothing changed |
| "baja el volumen de la tele del hall" / "sube ..." | c3-b | relative volume, the TV player | 12 to 11, 11 to 12 |
| "silencia la tele del hall" / "quita el silencio de la tele del hall" | c3-b | mute / unmute, the TV player | `muteStatus true`, then `false`, volume 12 kept |
| "pon el volumen de la tele del hall a 20" | c3-b | set volume 20 %, the TV player | 12 (= 20 % of 60, equal to the current level) |
| "pon el volumen de la tele del hall a 30" | c3-b | same | 18 (30 % of 60), then "a 20" back to 12 |
| "apaga la tele del hall" | c3-b | turn off, the TV player (answer "Se ha apagado el reproductor") | `Screen Saver` to `Screen Off` (a screen off: `power_off` is off by default); media player `off` |
| "enciende la tele del hall" | c3-b | turn on, the TV player | `Screen Off` to `Active`; media player `on`, volume level back |
| "baja el volumen" / "sube el volumen" (unnamed, satellite context) | c3-b | relative volume; the TV player was `playing` (screen saver), so it was the only match | 12 to 11, 11 to 12 |

Notes: the named commands reached the TV player with no ambiguity against the mute switch ("silenciar la tele del hall" alias) or the turn-on button, whose aliases stay. Power off and screen off were not run on c3-a (owner's rule). Unnamed volume commands work only while the TV player is `playing` (c3-b: during the screen saver; c3-a: not while it shows a PC input with no media service state), as the ADR predicted; naming the TV avoids it. HA's answer to "apaga la tele" is "Se ha apagado el reproductor" although only the screen went off. Not tested: sentences by voice (STT), the sound path, a real power-off.

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

**Never called:** `com.webos.service.apiadapter/system/getSystemInfo` (its reply carries the serial number), `tvpower/power/powerOff`, `power2/*`, the tvpower request registration methods, `micomservice/*`, any settings setter (`setSystemSettings`; the TV app's `ctl.sh set` for Always Ready and Hi LG is the one exception, [ADR 0008](../decisions/0008-user-initiated-lg-settings.md), and is not the daemon), `/dev/uinput`. c3-a (`03.30.74`): none of the getters or setters in this table has been called there except the power ones.

## 6. Capability detection at startup

Per control, after the power watch delivered its first state and before the ESPHome server lists entities (total budget 10 s; anything not answered in time counts as failed):

1. **Config:** the control's flag is on, and `luna-send` was found (on a PC every TV control is off, as the power watch is today).
2. **Getter:** one read-only call. Success means `returnValue: true` and the fields this design uses are present with the expected types. The answer seeds the state.
3. **Setter declared:** the method name is present in the TV's luna-service2 API declaration files (read-only file read, no call). A missing declaration means the control is read-only.
4. **Subscription:** for volume and foreground app, the getter with `{"subscribe":true}` is started as a supervised `luna-send -i` child; if its first reply lacks `subscribed: true`, the control polls (and, since the fix of 2026-11-04, keeps the child running to see whether it streams anyway, section 15) (section 7).
5. **Never a probe write.** Setters are not called at startup, not even with the current value.

Outcomes: a failed getter makes the entity report missing state; a missing setter makes it refuse commands; both are logged once at `warn` with the control name and the luna `errorCode`/`errorText` (no reply body). A setter that fails at run time (`returnValue: false`, a timeout, or a read-back that does not match) is logged once, reported as missing state and disabled until the next restart or list refresh. **No fallback to another, guessed method or parameter set.** The only fallback chain is the existing, verified one in the power path (`powerOn`, then the state-checked toggle key).

## 7. Polling, subscriptions and rate limits

| Data | Mechanism | Rate |
| --- | --- | --- |
| Power state | existing subscription | push |
| Volume, mute, sound output | `getVolume` subscription if supported, else poll | push, or every 10 s in `Active`, `Screen Saver`, `Screen Off`; none in `Active Standby` |
| Foreground app (drives `tv_app` and `tv_input`) | `getForegroundAppInfo` subscription if supported, else poll | push, or every 5 s in the same states |
| Play state (if the sensor or the TV media player is on) | poll | every 10 s in `Active` and `Screen Saver`, and once when lit again after `Screen Off` |
| Input list, launch points | read once | startup, after a resume, on `tv_refresh_lists` |
| After any setter | one read-back | 0.5 s after the reply |

At most three extra `luna-send -i` children (volume, foreground app, and the existing power watch makes four in all), each supervised with the power watch's backoff (1 s doubling to 60 s) and killed on shutdown. Their CPU cost is measured in EXP-010 Part B.

Setter limits: one call in flight per control; volume set commands from an HA slider are coalesced (latest value wins after 250 ms); volume up and down at most 4 per second; input and app at most one per 2 s; all TV-control setters together at most 30 per minute. A command above a limit is refused with one log line, and the entity's real state is sent again. The power path keeps its own limits (section 4.2).

`volume_limit` is **optional and unset by default** (changed 2026-11-04 after EXP-010 Part B: the first default, 40, was below c3-a's usual volume 89, and `maxVolume` differs per TV, 60 on c3-b). Unset, there is no cap below the TV's own `maxVolume`: the number's range is `0..maxVolume` and a set above it is clamped to it. With `volume_limit = N`: the number's range ends at `min(N, maxVolume)` (so the server also ignores a larger value from HA), a set above the ceiling is clamped to it, and `volume up` is refused at or above it. A TV that is **already above** the cap (set with the remote) is never forced down and can still be lowered from HA: `volume down` always works, and any value up to the ceiling can be set (the slider then shows the real value above its end, and a value between the ceiling and the current volume cannot be chosen). The entity shows the value read back from the TV, so a clamp is visible. The remote is not limited. Use it when one misheard number or a slider slip from a phone must not reach full volume.

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
volume = true         # tv_volume, tv_volume_up/down (EXP-010 Part A passed on c3-b)
# volume_limit = 40   # optional cap on the TV volume HA may set; unset = the TV's maxVolume
mute = true           # tv_mute (passed on c3-b)
input = true          # tv_input (passed on c3-b)
apps = true           # tv_app (passed on c3-b)
app_limit = 30        # maximum apps in the tv_app select
play_state = false    # tv_play_state sensor
media_player = true   # tv_media_player "TV" (ADR 0007, section 3.5)
lg_settings = true    # lg_always_ready and lg_hands_free read-only sensors (ADR 0008)
follow_firmware_guard = false  # owner decision 2026-11-04: TV control is not gated by the firmware guard
```

| Key | Default | Why |
| --- | --- | --- |
| `enabled` | true | Read-only state is harmless and makes the controls discoverable; the per-control flags gate every write |
| `screen`, `power_on` | true | Their calls are verified on c3-b (D5, D6, section 13), they keep the satellite running, and any remote key undoes them |
| `power_off` | **false** | Owner decision: opt-in, and only in the always-on mode. Costs: TV stays running dark (power draw not measured), LG's OLED care at power-off may not run, a brief panel flash, a second remote press takes about 5 s ([always-on 12.2](always-on.md#122-the-setting-to-expose-app-and-ha-ui)) |
| `volume`, `mute`, `input`, `apps` | **true** (owner, 2026-11-04, after EXP-010 Part A passed for all four on c3-b `03.30.73`) | The setters worked, read back and restored. The evidence is c3-b only; c3-a (`03.30.74`) is untested and each control is still checked per TV at startup and switched off when its getter, its declaration or a write fails (section 6). The optional `volume_limit` cap is available. Two caveats: volume and mute were tested on `external_arc` only, and input only on a disconnected HDMI input |
| `volume_limit` | unset (no cap below `maxVolume`) | Section 7. The first default (40) blocked real use on c3-a (volume 89) |
| `app_limit` | 30 | Keeps the select usable in HA and in voice |
| `play_state` | false | Extra polling for a sensor no v1 feature needs (the TV media player polls the play state itself) |
| `lg_settings` | **true** (owner, 2026-11-05, ADR 0008) | Two read-only diagnostic sensors, no setter. Costs two short luna reads a minute while the TV is lit (none in `Active Standby` or a transition) and one more pair when `ctl.sh set` creates the trigger `run/lg-settings-refresh`. A change seen by a read is logged once at `info` (`lg setting: always-ready off -> on (not changed by the satellite)`); a read that fails is logged once at `warn` and the sensor shows no state. Not gated by the firmware guard |
| `media_player` | **true** (owner, 2026-11-04, ADR 0007) | HA's built-in volume, mute and turn on/off intents only target media players. Costs one read-only poll every 10 s while lit |

Every control is also gated at run time by section 6. **Firmware guard (owner decision 2026-11-04): TV control is not gated by it.** On a firmware that is not in `safety.validated_firmware` (or whose version cannot be read) TV control keeps working and the daemon logs a warning at startup; since the same day the microphone is not hard-gated either ([firmware policy](firmware-policy-and-reports.md): it starts, warns and self-checks; the old microphone block exists only with `firmware_policy = "strict"`). The key `follow_firmware_guard` (default `false`) exists for an owner who wants the stricter behaviour: with `true` every setter is refused on an unvalidated firmware, while getters, sensors and states still report.

## 10. Safety

1. **User-initiated only.** A setter runs only in response to an HA command on an entity the owner turned on. The daemon never changes volume, input, app or screen by itself. The only automatic action stays the existing, opt-in wake-and-darken on a remote power-off.
2. **No silent settings writes.** v1 writes no TV setting. *(ADR 0008, 2026-11-05: the TV app may write Always Ready and LG hands-free on a user's press; the daemon and these entities never do.)* Volume and mute are the normal user controls (the TV keeps them as the remote would).
3. **Read back every write** (section 6); a mismatch is reported, never retried with other parameters.
4. **Never a write at startup**, not even a probe.
5. **Risk per action:**

| Action | Risk | Reversal |
| --- | --- | --- |
| Screen off | Low. The TV stays running: more power than standby (not measured), and if left dark for days LG's power-off OLED care does not run | `tv_screen` on, `tv_turn_on`, any remote key |
| Power-off (keep listening) | Medium. As screen off plus a 0.3 to 0.7 s panel flash (indirect, [D6](../discovery/c3-b/D6-poweroff-screenoff.md)); if the wake fails, LG standby or Suspend (section 4.2) | `tv_turn_on`, the remote; WoL from Suspend |
| Power-on | Low. Lights the panel in a room that may be dark | screen off or the remote |
| Volume set and step | Low to medium: loudness, optionally capped by `volume_limit` | set it back; the remote |
| Mute | Low | unmute; the remote |
| Input and app | Low: interrupts what is being watched; an app may start playing sound | select the previous one; the remote |

6. **The kill switch** blocks the power-off (as it blocks the darkener). Other TV controls do not use the microphone, so the kill switch and the mic mute do not block them.
7. **No new listening port**: everything goes over the existing ESPHome connection, which needs the paired key.

## 11. Voice

Voice works through HA, not through the satellite: the owner exposes the entities to Assist in HA (Settings, Voice assistants, Expose) and may add aliases ("TV", "television"). Expected behaviour, to be confirmed on the owner's HA 2026.10.4:

- The media player "TV" (`tv_media_player`, section 3.5, [ADR 0007](../decisions/0007-tv-media-player.md)) is the target for HA's built-in volume (set, relative), mute and unmute, and turn on and off intents (HA 2026.10.4 source; the owner's HA not yet checked). Give it the alias, not the satellite's own media player (the reply volume).
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
3. **On c3-b:** [EXP-010](../experiments/EXP-010-tv-control-setters.md): Part A (raw setters over SSH) ran on 2026-11-04 and passed; Part B ran on 2026-11-04 on c3-b (API client) and c3-a (owner's HA, non-disruptive subset).

## 13. Privacy

- The foreground app, the input and the play state describe viewing habits. They go only to the owner's HA, over the encrypted ESPHome connection. They are not logged at `info`: log lines name the control and the result ("tv_app: launch accepted, read back matches"); titles, app ids and input labels appear at `debug` only, and raw luna replies never (length only).
- No identifiers are read: no `getSystemInfo`, no serial, MAC or account data. Launch-point data beyond title and app id is dropped on parse.
- The listening indicator (section 3.3) is the privacy signal for a TV that looks off; no on-TV indicator in v1 (owner 2026-11-04).

## 14. Open questions

1. Still open after EXP-010 Part A (c3-b): volume and mute on the TV speaker output (A5, not authorized); whether an on-screen volume bar or mute icon appears; switching to a **connected** source; whether an app launch or volume change in `Screen Off` lights the panel (only the power state was read); behaviour with content playing; every setter on c3-a.
2. Whether an HA power-off with an app playing stops its sound (EXP-010 B10, not run).
3. Partly answered (EXP-010 Part B): HA 2026.10.4 shows `unknown` for a switch, number and select before their first state, and a daemon restart relists the same entities. Not seen on HA: a live change from a value to missing, and an option list that changed.
4. Which Assist intents act on buttons, numbers and selects on the owner's HA (section 11).
5. ~~Whether TV control should follow the firmware guard~~ Decided 2026-11-04 (owner): it does not; a warning is logged (section 9).
6. c3-a (`03.30.74`): mute, volume down and the getters (through HA) verified; screen, input, app launch and power-off untested there.
7. Findings of EXP-010 Part B, **fixed on the PC 2026-11-04, to re-check on the TVs** (section 15): the volume subscription without a flag (kept and confirmed by a pushed line, else polling), the `launch` declaration check (SAM file), `volume_limit` (optional, no cap by default), the refresh button's result line.

## 15. Implementation status (2026-11-04)

Written on the PC; then deployed as 0.2.0 and run on c3-a (permanent install) and c3-b (from `/tmp`), see the EXP-010 Part B results. Layout as in section 12.1: crate `crates/tv-control` (`webos-tv-control`: `luna` parsers, `lists`, `limits`, `setters`, `entities`, `controller`), adapter `crates/satellite/src/tv_control.rs` (startup check, `luna-send` calls, subscriptions, entity mapping) and a few hooks in `daemon.rs`; `darken.rs` gained `would_act` and `note_own_wake`; the ESPHome API crate gained generic `switch` and `text_sensor` entities, application-owned selects (no echo, `ClearSelect`, `SetSelectOptions`).

Deviations from the text above, and why:

- **Calls run on a pseudo-terminal**, not the pipe of `power::call`: EXP-010 Part A found that the audio methods print nothing over a pipe. The power-path calls of the existing daemon are unchanged.
- **Mute** uses `apiadapter/audio/setMute {"mute":bool}` (EXP-010), not `setMuted`. The `tv_sound_output` sensor comes from `getVolume`'s `soundOutput`; `apiadapter/audio/getSoundOutput` is not called.
- **A failed write disables a control** (until a restart or a list refresh) only for volume, mute, input and apps. Screen, power-on and power-off are never disabled: tvpowerd's refusals (`busy`, `Keyblock1.5second`, "substate must be active") are transient and known.
- **Declaration check:** the TV's `api-permissions.d` files are read for the services involved. The file format is not verified on a TV: when no file of a service can be read the result is `Unchecked` (allowed, with a warning; the read-back after every write is the guard), and only a readable file that lacks the method makes the control read-only.
- **Read-back timing:** 0.5 s after the setter's reply, then once more 2.5 s later (input and app changes took 1.5 to 2 s in EXP-010) before a mismatch disables the control; never a second write.
- **`tv_power` reads unknown** whenever any precondition of section 4.2 fails, including the darkener's 20 s cooldown after a power-off. The way back is `tv_turn_on`.
- **`launch` payload and app list:** launch points have no hidden marker on c3-b, so every entry with an id and a title is kept, minus the satellite's own app (`io.github.pando85.voicesatellite`), the input apps and duplicate ids, capped at `app_limit`. The parser also honours `visible: false` or `hidden: true` if a firmware sends them (not verified anywhere).
- **Select options change:** the controller queues new options; the daemon applies them (the server closes the HA connection, HA lists again) only when no conversation or reply is running. Whether HA lists entities on every reconnect is checked in the fake-HA scenario only, not against HA 2026.10.4.
- **Volume and foreground subscriptions** start only for getters that answered, poll when the first reply lacks `subscribed: true` (the subscription is then kept to see whether it streams, see the fixes below), and the play state is always polled (every 10 s in `Active` and `Screen Saver`).

Fixes after EXP-010 Part B (PC only, 2026-11-04, tests in `crates/tv-control` and the fake-HA scenario V; **not yet run on a TV**):

- **`volume_limit` is optional** (default unset, no cap below `maxVolume`), see section 7 for the semantics. `[tv_control] volume_limit` accepts 1 to 100 when set. The startup summary shows `volume_limit=tv-max` when unset. The HA number's range follows `maxVolume` (or the cap, when lower); because the ESPHome server ignores a number command outside the entity's range, with a cap HA cannot even send a value above it, and the controller's own clamp is a second line.
- **Launch declaration check:** the declaration files read now include any file whose name contains `com.webos.sam` (the application manager's methods, `com.webos.sam.api.json` on both TVs), and the entry match is case-insensitive and accepts `com.webos.applicationManager/launch`, `applicationManager/launch` (with or without `luna://`) and wildcards. The entry shape inside that file was **not read** (no raw output is committed), so the fixture is synthetic. A SAM file can only prove `Yes`: a miss there leaves `launch` at `Unchecked` (as before), never `No`, so a wrong guess about the format cannot turn the app select read-only. Re-check on a TV: the "cannot check that the TV declares launch" warning must be gone.
- **Refresh button:** one summary line when both list reads are in: `tv_refresh_lists: done: N inputs, M apps` (counts only, no titles), or a warning `tv_refresh_lists: failed to read (...)` with the lists kept.
- **Volume subscription without the flag:** the first reply lacking `subscribed: true` no longer ends the subscription. The child keeps running and the 10 s poll keeps running too, until a **second line** arrives on the subscription (a push: `an update was pushed on the volume subscription; it streams without the flag, polling stopped`). If instead a poll sees a different value and no line was pushed within 3 s (`PUSH_GRACE`), the subscription is dropped and the control polls only (warning `pushed nothing for a change`). A luna error as the first line still falls back to polling at once. This rests on EXP-010 A3 (c3-b: one pushed line per change on that subscription) and was verified on the PC only with a fake `luna-send` that does not send the flag. On a TV: change the volume with the remote and look for the "pushed" line within seconds, then no more 10 s volume polls (`tv_control` debug log); or the "pushed nothing" warning if the TV does not stream.

TV media player ([ADR 0007](../decisions/0007-tv-media-player.md), PC only, 2026-11-04; **not yet run on a TV or against the owner's HA**): `tv_media_player` in the controller (`Kind::MediaPlayer`, `StateValue::Media`, `Command::Media`, delegating to the same command paths), the ESPHome API crate's application-owned media players (`Config::media_players`, `AppEvent::MediaPlayerCommand`, `AppCommand::SetMediaPlayer`; commands routed by key, the satellite's player unchanged), `[tv_control] media_player`. Tests: controller unit tests (state mapping per power state, volume share round trip, commands, `volume_limit`, turn off mapping, refusals), the adapter's mapping and a fake `luna-send` run (`setVolume 30` for 0.5), `server_io` (two players routed by key), fake-HA scenario J (two players, HA's feature mapping, no audio format, commands, no echo, play media ignored) and satellite scenario V25 to V32. To verify on a TV and on HA: the entity and its state, each native Spanish intent of section 3.5, that announcements and TTS still play on the satellite, and the extra play-state poll's cost.

Answered by the TV run (EXP-010 Part B, 2026-11-04): the pty one-shot calls work on the real `luna-send` on both TVs; the declaration files are in `/usr/share/luna-service2/api-permissions.d` as JSON lists of `service/method` strings (audio and apiadapter found; the application manager's methods are in `com.webos.sam.api.json`, which the filter does not match, so `launch` stays `Unchecked`); the subscription children cost about 0 ticks each, the daemon about 1.5 % of one core in all on c3-a (0.1.1: 1.3 %); the volume getter reports no `subscribed` flag, so the daemon polls every 10 s; HA shows a missing state as `unknown`; the refresh button runs without a reconnect when nothing changed. Still open: the disruptive controls on c3-a, a live missing state and a changed option list against HA, B10 and B11.
