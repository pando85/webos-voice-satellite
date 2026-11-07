# ADR 0015: A silent mic-key filter in LG's compositor, through an in-memory override of the key-filter list

- **Status:** **Proposed (draft, 2026-11-07). NOT accepted.** No owner decision. Untested: [EXP-025](../experiments/EXP-025-mic-button-toast.md) Part B, which would try it once on c3-a, is **not authorized**. Nothing here is built, and nothing here may run on a TV on the strength of this draft.
- **Scope:** removing LG's toast ("Esta función no es compatible.", which the owner reported as "Función no disponible") that appears on every mic-button press while the satellite owns the key ([ADR 0013](0013-remote-mic-button-coexistence.md)).
- **Would relax, if accepted:** the AGENTS.md device-safety exception, which today covers only the volatile override of `com.webos.keyaction.voice`. This ADR would add a second configd key, and code of ours loaded by LG's compositor.
- **Related:** [research section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07) (10.7 has the evidence and the risks), [EXP-025](../experiments/EXP-025-mic-button-toast.md), [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md), [ADR 0013](0013-remote-mic-button-coexistence.md), [ADR 0014](0014-assistant-overlay.md).

## Context

- The toast is sent by the compositor's key filter (`appLaunch.js` `handleHotkeys`) for **any** `com.webos.keyaction.voice` action other than `launch`; no value of that key avoids it (static, c3-a files; confirmed on the running TV by hash in EXP-025 A2). On the owner's presses with 0.5.0, LG's toast window opened 12 to 58 ms after each press and stayed 5 s (`verified-on-target`, filtered log, EXP-025 A7).
- The compositor's chain of key handlers is itself a configd value, `com.webos.surfacemanager.keyFilters`: 18 `{handler, file}` entries with absolute paths; `handleHotkeys` is 13th (`verified-on-target`, EXP-025 A1). The voice key has no entry of its own.
- The loader includes any absolute path (`Qt.include("file://" + file)`, no directory check; `verified-on-target`, A3). The compositor subscribes to the list and rebuilds the chain on every change (static).
- configd keeps any key set with `"volatile":true` in memory only, and `null` removes it (upstream source, `documented`; the same mechanism runs live for `com.webos.keyaction.voice`, `verified-on-target`).
- The notification service has no per-source suppression (`verified-on-target`, A6); its global `disable` is rejected.
- `surface-manager` runs as root; the developer app directories (ours included) are world-writable (`verified-on-target`).

## Decision (proposed)

Only while the mic button feature runs (ADR 0013), and only on firmware where EXP-025 Part B passed:

1. **Keep ADR 0013's override** of `com.webos.keyaction.voice` (`ignore`). It stays the fallback: if anything below fails, a press shows today's toast, never LG's assistant.
2. **Write the handler file** at acquire: a fresh `mktemp -d` directory under `/tmp` (root, mode 0700), file mode 0600, content fixed in the satellite (three lines: `init()` empty; `handleSatelliteVoiceKey(key, pressed, autoRepeat)` returns `KeyPolicy.Accepted` for `WebOS.Key_webOS_Voice`, otherwise `KeyPolicy.NextPolicy`). Never in the install directory (world-writable on this TV).
3. **Apply:** read `com.webos.surfacemanager.keyFilters`; require exactly one entry with handler `handleHotkeys`, and no entry of ours; insert ours directly before it; `setConfigs` with `"volatile":true`; read back and compare. Any mismatch: restore (4) and keep only the ADR 0013 behaviour.
4. **Restore** (on the same paths as ADR 0013 decision 3, before ADR 0013's own restore, under the same lock and state file): `setConfigs {"configs":{"com.webos.surfacemanager.keyFilters":null},"volatile":true}`, read back and check that our entry is gone; then remove the directory. Never write a saved list back: `null` always returns to LG's current value. A TV restart clears both the override and `/tmp`.
5. **Report** the state like `lg_voice_key` (for example `silent filter on`, `off`, `unknown` with the reason) in `ctl.sh status`, the TV app and HA.
6. **Off by default** as a separate option under `[remote]`, so the owner can keep ADR 0013's behaviour (with the toast) and turn this on only if wanted.

## Consequences

- **Gain:** no LG toast on mic presses; the panel of ADR 0014 is the only thing on screen.
- **Risks** (research 10.7):
  - our JavaScript runs inside LG's compositor, as root; a broken handler should be skipped (OSE `WebOSKeyFilter`), but the TV's build is not verified;
  - every apply and restore rebuilds the whole chain (timers, luna subscriptions, `init()` of every filter, Quick Settings and input picker pre-launch); a key pressed in that moment may be lost; a failure to create one filter object would leave the handlers after it out until the next change (static, LG's own QML would have to fail);
  - while active, the override masks any change LG makes to the list (a mode or layer switch) (hypothesis);
  - firmware updates can rename handlers or move the voice branch; the read, insert and compare steps refuse anything unexpected.
- **Rules:** if accepted, the AGENTS.md exception would name both keys, the volatile-only rule, the file location rule, and restore on feature off, stop, crash and uninstall. Experiments still need authorization per run.

## Alternatives considered

- **Accept the toast (ADR 0013 as is):** no new risk. The default if this draft is not accepted.
- **Remove `handleHotkeys` from the list:** silences the mic key but breaks every hotkey that handler serves. Rejected.
- **Another value of `com.webos.keyaction.voice`:** none avoids the toast (static). Rejected.
- **Notification `disable` or store-mode silence:** global. Rejected.
- **Pause `voiceconductor` with `launch` kept (ADR 0013 decision 2(b)):** no toast, but queued key events reach LG's assistant at resume, and every `voiceconductor` client waits. Not proposed.
- **The fullscreen app's `_WEBOS_ACCESS_POLICY_KEYACTION_ACTIVATE_XINPUT`, `blockKeyFilterNonCardTypeApps`, `registerAllowedKey`:** act only for the app in front; the overlay is not in front. Rejected.
- **A key grab or a hook in `lginput2`:** excluded by ADR 0013.

## Revisit when

- The owner decides on EXP-025 Part B, and after it: accept, amend or reject.
- A firmware update changes the key-filter list, the loader or the voice branch.
