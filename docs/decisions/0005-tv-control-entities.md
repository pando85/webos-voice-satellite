# ADR 0005: TV control from Home Assistant through separate satellite entities

- **Status:** Accepted (2026-11-04, owner). **Decision 3 amended by [ADR 0007](0007-tv-media-player.md)** (same day, owner): a control-only TV `media_player` is added next to the separate entities, so HA's built-in volume, mute and turn on/off intents reach the TV.
- **Scope:** whether and how the satellite exposes TV controls (power, screen, volume, mute, input, apps) to Home Assistant. Design: [TV control](../design/tv-control.md). Not implemented; setters untested ([EXP-010](../experiments/EXP-010-tv-control-setters.md)).
- **Related:** [ADR 0001](0001-esphome-native-api.md), [backlog #11](../project/backlog.md#11-tv-control-from-home-assistant-via-the-satellite), [always-on design](../design/always-on.md)

## Context

- The satellite runs as root on the TV and already calls luna services with `luna-send`: the power-state subscription, `tvpower/power/powerOn {"reason":"remoteKey"}`, `networkinput/controls/power` and `tvpower/power/turnOffScreen` ([always-on section 13](../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b)).
- On c3-b (firmware `03.30.73`, 2026-11-04) the getters for volume and mute, sound output, external inputs, foreground app, media play state and launch points answered as root with no SSAP pairing (`verified-on-target` for that unit only). The setters are declared but untested; `turnOnScreen` and the power path are verified on c3-b ([D5](../discovery/c3-b/D5-results.md), [D6](../discovery/c3-b/D6-poweroff-screenoff.md)). c3-a (`03.30.74`) is untested.
- A real power-off either suspends the TV (the daemon is frozen) or leaves it in `Active Standby` (replies not audible). In `Screen Off` the satellite listens and replies are audible (verified on c3-b).
- HA's `webostv` integration needs SSAP pairing and documents Wake-on-LAN for turning on. The owner's HA has no `webostv` entry.
- The satellite's ESPHome `media_player` already means "reply volume".

## Decision

1. **Complement HA's `webostv` integration, do not replace it.** The satellite adds no-pairing, standby-aware and voice-friendly control; owners may run both.
2. **v1 controls:** power and screen, TV volume and mute, input selection, app launch. **No toasts or notifications in v1.**
3. **Separate entities** (`switch`, `number`, `select`, `button`, sensors), not a TV `media_player`. The existing media player stays the reply volume. *(Amended 2026-11-04 by [ADR 0007](0007-tv-media-player.md): the separate entities and the reply-volume player stay, and a second, control-only media player "TV" is added; HA's Spanish volume and mute intents only target media players, [P4](../discovery/c3-b/P4-sound-session.md).)*
4. **Power-off only into a state where the satellite keeps running, only when enabled.** An HA power-off sends the remote's power key and relies on the always-on mode "Keep listening (screen off)" (wake and darken) to end in `Screen Off`. It is opt-in (`[tv_control] power_off`, default off), not listed when off, and refused unless the always-on mode is selected and the darkener would act. No real Suspend power-off from HA.
5. **Listening indicator as HA entities only:** truthful `listening` and `microphone_muted` binary sensors tied to the real capture PCM state. No on-TV indicator.

Also decided by the owner on 2026-11-04: the audible timer and announcement check is skipped for now; setters are tested on c3-b only when the owner says so; the conversation agent for the Kiwi pipeline is deferred ([backlog #12](../project/backlog.md#12-conversation-agent-for-the-kiwi-pipeline)).

## Consequences

- New entities on the satellite device; a new sans-IO crate for TV control, separate from capture, voice and playback; the ESPHome API crate needs generic `switch` and `text_sensor` entities.
- Volume, mute, input and app stay off by default until EXP-010 passes on c3-b; a control whose setter fails there stays off. (Update 2026-11-04: Part A passed for all four on c3-b `03.30.73`; the owner then made them **on by default**, and decided that TV control is **not gated by the firmware guard** (a warning is logged on an unvalidated firmware; see the [design section 9](../design/tv-control.md#9-configuration)).)
- Every control is gated by a startup capability check; a failed method gives an entity with a missing state, never a guessed value.
- An HA "turn off the TV" leaves the TV running with a dark screen: more power than standby (not measured), LG's power-off OLED care may not run, a brief panel flash, and a 5 s second press. These costs are the always-on mode's, shown next to the setting.
- Waking a TV from Suspend stays outside the satellite (HA's Wake-on-LAN or the remote).
- Voice for numbers and selects needs HA scripts, custom sentences or an LLM agent; switches work with HA's built-in intents.

## Alternatives considered

- **Replace `webostv`:** would duplicate a maintained integration and its media features; the satellite's advantage is only standby and pairing.
- **One TV `media_player`:** clashes with the reply-volume media player, and its single state mixes power, input and volume. (Revisited in [ADR 0007](0007-tv-media-player.md): added as a second, control-only player with its own key, in addition to the separate entities.)
- **Real power-off (key without the always-on mode, or `tvpower/power/powerOff`):** the satellite is frozen in Suspend or cannot reply in `Active Standby`.
- **Power-off as `turnOffScreen`:** keeps the running app's sound playing; kept as the separate screen switch.
- **Toasts and an on-TV listening indicator:** deferred by the owner; no controllable status LED exists.

## Revisit when

- EXP-010 shows a setter does not work or behaves differently per sound output or power state.
- c3-a or another model or firmware gives different results.
- The owner wants toasts, an on-TV indicator, or a real standby power-off.
- HA's `webostv` integration gains standby control without pairing, or the owner adds it and the two conflict.
