# ADR 0008: User-initiated changes of two LG settings from the TV app

- **Status:** Accepted (2026-11-05, owner). **Replaces, for the two settings named below only,** the rule "the satellite never writes a TV setting" (stated in [always-on section 13](../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b), [TV control section 1 and 10](../design/tv-control.md#10-safety), [ADR 0005](0005-tv-control-entities.md) by reference). The rest of those rules stands.
- **Scope:** exactly two LG settings, changed only by an explicit press in the TV app: **Always Ready** (settingsservice `general/lifeOnScreenMode`) and **LG hands-free "Hi LG"** (`voiceinput/setting/getActivation` and `setActivation`). The satellite's own "keep listening after the power button" setting is not an LG setting and is not covered by this limit. Design: [Voice and power screen](../design/voice-and-power-screen.md). Backlog: [#14](../project/backlog.md#14-voice-and-power-screen-in-the-tv-app).
- **Related:** [ADR 0004](0004-mvp-capture-device-and-lg-handsfree-off.md), [ADR 0005](0005-tv-control-entities.md), [always-on design](../design/always-on.md), [install and configuration UX](../design/install-and-config-ux.md) (open question 12), [AGENTS.md](../../AGENTS.md) (device safety). The luna write details are `pending EXP-011` (an experiment on c3-b that lists the settings' values and the setter behaviour; its record is added separately).

## Context

- The satellite can only listen, and reply audibly, in some combinations of three settings ([always-on sections 1, 11 to 13](../design/always-on.md#1-short-answer), [c3-b record](../compatibility/c3-b.md); `verified-on-target` on c3-b, firmware `03.30.73`):
  - **Always Ready off, remote power key, satellite on "Turn off normally":** the TV suspends 7 to 15 s after the key and the satellite is frozen.
  - **Always Ready on, "Turn off normally":** the TV rests in `Active Standby`; the satellite listens but its replies are silent (LG cuts the DSP input gain).
  - **"Keep listening (screen off)" with either Always Ready value:** the daemon wakes the TV and darkens it to `Screen Off` about 5 to 6 s after the key; listening and replies work.
  - **LG hands-free ("Hi LG") on:** LG's voice service holds the capture PCM in every state and the satellite reports `MIC BUSY` ([P4 on c3-b](../discovery/c3-b/P4-permanent-install.md), [ADR 0004](0004-mvp-capture-device-and-lg-handsfree-off.md)).
- Today the owner and testers must change Always Ready and "Hi LG" through the TV's menus (Settings, General, AI Service, Voice recognition, "Hi LG"; the Always Ready page) or over SSH. The household found "Hi LG" switched off without knowing why ([P4 sound session 2](../discovery/c3-b/P4-sound-session.md#state-found-and-the-hi-lg-change)), so a screen that shows these states and what they mean is needed regardless of who may change them.
- The existing rule ([always-on section 7](../design/always-on.md#7-exposing-power-options-in-the-ui-design), [TV control 10](../design/tv-control.md#10-safety)) already allowed a write "with an explicit user action in the app", but the shipped docs and the daemon say flatly that no TV setting is written, and [install and configuration UX](../design/install-and-config-ux.md) left open whether the satellite may write them at all (open question 12).
- Both setters were used by agents on c3-b with the owner's authorization and read back ([D5](../discovery/c3-b/D5-results.md), [D6](../discovery/c3-b/D6-poweroff-screenoff.md): `general/lifeOnScreenMode` and `setActivation`); they are private luna methods, not a documented API. c3-a is a different firmware and is untested for the writes. The luna call shapes to ship are `pending EXP-011`.

## Decision

1. **The TV app may change two LG settings, on an explicit user press only:** Always Ready (`off` or `on`) and LG hands-free "Hi LG" (`off` or `on`), on the "Voice and power" screen. The third control on that screen, "Keep listening after the power button", is the satellite's own existing select and writes the satellite's own state, not an LG setting.
2. **Scope is exactly these two settings.** No other LG setting is written by the app or the daemon (not Quick Start+, wake on LAN, sound output, energy saving, auto-off, picture or sound modes). Adding a setting needs a new ADR.
3. **The daemon never changes either setting.** Not at startup, not on a power event, not to "fix" a state it finds, not from Home Assistant. A diagnostic read is allowed; a write is not. The write path is `ctl.sh set ...`, which only the app (or a person over SSH) runs.
4. **Home Assistant gets two read-only diagnostic sensors** ("Always Ready" and "LG hands-free"); writes stay in the app. No HA switch for either.
5. **Every write is read back** and the app shows the value the TV reports. A mismatch or a failure is shown as such, never retried with other parameters and never counted as success.
6. **The old value is logged** (before, requested, read back, time, source) in a persistent file so the change can be undone by hand or by the app.
7. **A confirm step lists the consequence** before each LG setting changes (the same "press again" pattern as the app's other risky buttons).
8. **No write when the read fails or the value is not one the satellite knows.** An unknown current value (Always Ready has more values than the two used here) is shown as unknown with its raw word, and the app offers no change from it, only the TV's menu path.

## Consequences

- Owners no longer need SSH or menu hunting to make the satellite work, and the app can say, in one sentence, what the power button will do and whether the satellite can hear.
- The satellite becomes a program that can change LG's voice assistant and standby behaviour. That is an intended, visible, reversible action by the user; it is not an automatic one. A bug in the app's command handling would be a bug that writes an LG setting, so the argument list is fixed (six strings, no typed text), like the app's other commands.
- Turning Always Ready on raises standby power (not measured). Turning "Hi LG" on takes the microphone from the satellite. Both are warned in the screen text.
- Private luna methods can change in a firmware update. A write that stops working is shown as failed, the other controls keep working, and the screen falls back to the TV's menu path in its text.
- The "never writes a TV setting" wording in the docs is amended to point here; the code comment in `darken.rs` ("the machine never changes a TV setting") stays true (the darkener itself writes nothing).
- Verification: a write on c3-a needs the owner's authorization per run; c3-b stays read-only unless the owner says otherwise ([plan](../design/voice-and-power-screen.md#8-test-plan-and-implementation-steps)).

## Alternatives considered

- **Display only, with the TV's menu path:** safest, no new private-API writes, and the matrix sentence is the same. Rejected by the owner: the point of the screen is to remove the menu hunt, and the household had already lost "Hi LG" without noticing. Kept as the fallback for an unknown value or a failed write.
- **Home Assistant switches for the two settings:** one place for automations, but anyone with HA access, an automation or a voice command could change LG's voice assistant or standby behaviour at a distance, which contradicts "only on a press in the app". Rejected; HA gets read-only sensors.
- **Daemon manages the settings automatically** (for example turns "Hi LG" off when `MIC BUSY` appears): rejected as a silent write; the daemon only reports `MIC BUSY` and the app's sentence says what to do.
- **A broader "TV settings" screen:** out of scope; each further setting would need its own evidence and ADR.

## Revisit when

- EXP-011 shows a setter does not apply, applies differently per provider or value, or that LG reverts the value by itself.
- c3-a or another model or firmware behaves differently.
- The owner wants a third setting, or wants Home Assistant to be able to write.
- A firmware update changes or removes either method.
