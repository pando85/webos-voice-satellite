# ADR 0013: Remote mic button: coexistence with LG's voice assistant

- **Status:** **Accepted (2026-11-07, owner) for the override-first path: decision 2(a) with decisions 1, 3, 4 and 5.** The owner: "we have to add options to switch that option in the TV while enabling this. We cannot have our feature enabled without disabling the LG one. Option must be in the UI and in esphome." So the satellite's mic button feature runs **only** while LG's voice key is taken over by the in-memory configd override; there is no "both assistants" mode. Decision 2(b) (the `voiceconductor` SIGSTOP) stays a **conditional fallback, not accepted and not implemented**: it is considered only if [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) C1 shows that the override does not work on this firmware, and then needs its own owner decision. **Implemented on the host (2026-11-07), not yet run on a TV:** the override's first on-TV use is the owner's first enable session, which is EXP-023 C1. Implementation notes: [design](../design/remote-mic-button.md) section 7.
- **History:** proposed earlier on 2026-11-07 with the owner's direction "a supported switch first, otherwise pause LG's voice service while the satellite runs".
- **Scope:** what happens to LG's assistant when the satellite uses the Magic Remote's mic button for push-to-talk (the "mic button feature").
- **Relaxes, if accepted:** the rule "avoid changing native LG voice services" ([AGENTS.md](../../AGENTS.md) device safety, [discovery rules](../discovery/README.md#safety-rules-learned)). It relaxes it for the two mechanisms named in decision 2 only.
- **Related:**
  - research: [remote mic button](../research/remote-mic-button.md), section 9 has the evidence for this ADR;
  - experiments: [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md) (key and stream, run) and [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) (hand-over, **not run**);
  - ADRs: [ADR 0004](0004-mvp-capture-device-and-lg-handsfree-off.md) (Hi LG off), [ADR 0006](0006-soft-firmware-policy-and-compatibility-reports.md) (firmware policy), [ADR 0008](0008-user-initiated-lg-settings.md) (the only LG settings the app writes), [ADR 0012](0012-config-switches-in-ha.md) (config switches).

## Context

- **The key** (verified-on-target, c3-a and c3-b, EXP-022):
  - The mic button reaches a passive evdev reader as code 428 (press and release) on `LGE M-RCU - Builtin [0]`.
  - Each press also opens LG's assistant (`com.webos.app.voice`), even with "Hi LG" off.
- **The audio** (verified-on-target, c3-a, EXP-022 B):
  - The remote's microphone is available through `voiceinput/startStreaming {"deviceType":"remote","subscribe":true}`: 16 kHz mono S16 on a Unix socket.
  - It does not stop at the release.
  - With LG's flow running too, one `2003 audio data is already capturing` refusal and an LG "audio processing is unstable" message were seen.
  - So both assistants on one press is not a usable product: two consumers of one stream, two UIs, and the audio goes to LG's cloud.
- **The routing** (static, read from the files shipped on c3-a `03.30.74`, 2026-11-07; runtime behaviour is a hypothesis until EXP-023):
  - The compositor's key filter (`/usr/lib/qml/KeyFilters/appLaunch.js`, package `starfish-systemui-keyfilters`) handles `Key_webOS_Voice`.
  - Only when the configd value `com.webos.keyaction.voice` has `"action":"launch"` does it call `voiceconductor/setInputEvent` (hold, down and up, `keyType thinQtv`). Otherwise it shows the toast "This function is not supported." and consumes the key.
  - The base layer sets `launch` (`/etc/configd/layers/base/com.webos.keyaction.json`). **LG's own hotel-mode, `tvoverlay` and `hd1080-mini` layers set `"ignore"`**, so this is LG's mechanism for products where the voice key must do nothing.
  - The key filter subscribes to `com.webos.keyaction.*`, so a change should apply live.
- **The override** (documented upstream, consistent with the shipped binary):
  - configd's `setConfigs` with `"volatile": true` keeps the value in an in-memory database, cleared by a reboot or `reconfigure` ([webosose/configd](https://github.com/webosose/configd) `Configd.cpp`, `Manager::updateFactoryDatabase`, commit `99873ce`).
  - **Without** `volatile` it writes the persistent `/var/preferences/configd_factory_db.json`. That file does not exist on c3-a today.
  - The OSE API page says `setConfigs` values are "kept until rebooting". The source shows that holds only for the volatile path.
- **The other leads** (static, research section 9):
  - `/tmp/voiceinput_disable_set_input_event` only stops the far-field "Hi LG" trigger event.
  - `getVoiceKey` returns LG's security voice keys.
  - `setInputEvent` is the trigger input itself.
  - No LG setting and no `com.webos.service.mrcu` method reroutes the key.
- **The fallback** (observed-on-other-device, webOS 25, CleanYourTV): pausing `voiceconductor` with SIGSTOP keeps LG's screen and cloud recognition away while `voiceinput`'s remote stream still works.

## Decision (proposed)

1. **Default off.** The mic button feature is a satellite setting, `[remote] mic_button = "off" | "ptt"`, default `off`. With it off, the satellite changes nothing of LG's and only observes (passive evdev reader, no grab). Turning it on is the user's explicit action. Where it is exposed (TV app, HA config switch as in ADR 0012) is decided at implementation.
2. **Order of mechanisms, while the daemon runs and the feature is on:**
   - **(a) Preferred, "supported switch":** an in-memory configd override.
     - Apply: `setConfigs {"configs":{"com.webos.keyaction.voice":{"action":"ignore"}},"volatile":true}`.
     - Read it back with `getConfigs`; a mismatch means "not applied".
     - Restore: `setConfigs {"configs":{"com.webos.keyaction.voice":null},"volatile":true}`, then read back that the value is LG's again.
     - Never call `setConfigs` without `"volatile":true`. Never write a file under `/etc/configd` or `/var/preferences`.
     - Used only on firmware where EXP-023 C1 passed.
   - **(b) Fallback, "pause":** `kill -STOP` of `voiceconductor` (found by `pidof` and checked through `/proc/<pid>/exe` = `/usr/sbin/voiceconductor`), and `kill -CONT` to restore.
     - Used only where (a) is unavailable (C1 failed, or the key or the key filter differs on that firmware) and EXP-023 C2 passed on that firmware.
     - Possible refinement after C2: pause only during our run (from the 428 press to the end of the run) instead of for the daemon's whole lifetime, if late queued events are not a problem.
   - **(c) Otherwise:** the feature cannot be turned on for that firmware. "Both assistants on one press" is not offered.
   - No other LG voice service change: no flag files, no `setInputEvent` or `getVoiceKey` calls, no grab, no hook, no `systemctl` stop or mask, no setting writes beyond ADR 0008.
3. **When.** The mechanism is applied when the daemon has started, the feature is on, and the self-check passes:
   - the key node is found by name;
   - (a) reads `launch` before it is changed, or (b) `voiceconductor` exists.

   It is restored when any of these happens: the feature is turned off; the daemon stops (`ctl.sh stop`, signal, clean exit); `ctl.sh uninstall` or purge; the app package is removed; or the supervisor sees the daemon die and will not restart it.
4. **Crash safety.** Both mechanisms outlive the process that applied them: a volatile configd value lives in `configd`, and a stopped process stays stopped. So restore must not depend on the daemon:
   - The daemon writes a state file (for example `run/lg_voice_override` = `configd` or `sigstop` plus the time) **before** applying, and removes it after a verified restore.
   - `ctl.sh restore-lg-voice` restores whatever the state file names, is idempotent (SIGCONT of a running process and removing an absent override are harmless), and is called by `ctl.sh stop`, `uninstall`, `purge` and the package removal script.
   - The supervisor (`start.sh`) calls it whenever the daemon exits, before any restart. The restarted daemon re-applies if the feature is still on.
   - At daemon start, if the state file exists from an earlier crash, the daemon first restores, then re-applies.
   - A reboot clears both mechanisms by itself (volatile override gone, `voiceconductor` started fresh). The HBC failsafe therefore also restores LG's voice.
   - `ctl.sh status --json` reports `lg_voice_key`: `lg`, `ours (configd)`, `ours (paused)` or `unknown`. The TV app shows it, with a "Give the mic button back to LG" action that runs the restore.
5. **Feedback and truthfulness.** The satellite's own listening indicator follows the real stream (F7). The remote stream is closed by the satellite at release (plus a short tail) and at a hard cap, because LG's stream does not end at the release (EXP-022 B).

## Consequences

- **Privacy benefit:** with the feature on, a mic press no longer starts LG's cloud recognition. The audio goes only to the user's Home Assistant, and only while the button is held (plus the tail).
- **LG features affected:**

  | LG feature | (a) configd override | (b) SIGSTOP of `voiceconductor` |
  | --- | --- | --- |
  | Magic Remote mic key → LG assistant | Off; toast "This function is not supported." on each press (static reading; to confirm in EXP-023) | Off; no toast expected |
  | TONE+ earbuds voice button (`Key_webOS_TonePlusVoice`) | Off too (same `voiceAction` check) | Off |
  | "Hi LG" far-field wake word | Not touched by this mechanism (separate path from `voiceinput`); stays off anyway by ADR 0004 | Off (its trigger goes through `voiceconductor`) |
  | Alexa key, Search key | Not touched (separate branches) | Off (they call `voiceconductor`) |
  | Apps using `recordVoice`, `getVoiceUiEvents`, `interactor/*`; Settings pages that query `voiceconductor` | Not touched | Calls queue until SIGCONT; pages may wait for luna timeouts (hypothesis) |

- **UX cost of (a):** an LG toast on every press, unless EXP-023 shows otherwise. The satellite's own feedback (earcon, later the overlay) should make the toast secondary. (b) has no toast but a wider blast radius.
- **Firmware-update risk:**
  - The key filter JS, the configd key name, the value LG honours, configd's `volatile` handling, and `voiceconductor`'s name can each change in an LG update (webOS upgrades are offered for the C3).
  - Mitigations: the self-check (decision 3) and read-backs; the per-firmware allow list (ADR 0006: the mechanism is enabled only on firmware where EXP-023 passed); the status field; and the fact that both mechanisms vanish on a reboot.
  - A firmware where the override is not honoured shows LG's assistant again. This is visible, not harmful.
- **Device safety:**
  - (a) changes no file the satellite owns. configd's own bookkeeping may write `/tmp/configd_*_before_setConfigs.json` and, per the upstream source, a timestamp list in `/var/preferences/configd_debug_db.json`. EXP-023 records whether that happens. If it does, the ADR must state it before acceptance.
  - (b) carries the known risks of pausing a service other components call.
- **Rules:** if accepted, the device-safety wording changes as proposed below. Agents still may not touch LG voice services in experiments without authorization per run.

## Proposed rule change (proposal only; AGENTS.md is not edited by this ADR)

In [AGENTS.md](../../AGENTS.md), "Device safety and privacy", replace

> - Maintain rollback paths and avoid changing native LG voice services during discovery.

with

> - Maintain rollback paths. Do not change native LG voice services, with one exception (ADR 0013, `docs/decisions/0013-remote-mic-button-coexistence.md`): while the satellite runs with the mic button feature enabled, it may set the in-memory (`"volatile": true`) configd override `com.webos.keyaction.voice` or, only where that is unavailable, pause `voiceconductor` with SIGSTOP. Both must be restored on feature off, stop, crash and uninstall, and must never be made persistent. Experiments that exercise either still need the owner's authorization per run.

In [discovery/README.md](../discovery/README.md#safety-rules-learned), the sentence "Do not change native LG voice services." gains: "(exception: ADR 0013, satellite runtime only)".

## Alternatives considered

- **Both assistants on one press (nothing changed):** rejected. There are two UIs, a contended single stream (`2003`, LG's "unstable" message), and audio to LG's cloud.
- **A different key** (a spare shortcut key, grabbed and re-emitted): avoids touching LG's voice, but a grab risks locking the owner out of the remote, and the owner asked for the mic button. Kept as a future option.
- **`/tmp/voiceinput_disable_set_input_event`:** rejected. It stops only the far-field "Hi LG" trigger event (static analysis), and its delete path sets a sound-engine mode that may differ from the one it had.
- **Persistent configd override** (`setConfigs` without `volatile`, or a layer file): rejected. It survives uninstall and a crashed restore, and it writes LG's persistent configuration.
- **Stopping or masking `voiceconductor` through systemd, or killing it:** rejected. `Restart=on-failure` restarts it, masking is persistent, and other services depend on it.
- **Hooks in `lginput2` or `surface-manager`:** rejected (process injection; reported broken on webOS 24/25).

## Revisit when

- EXP-023 has run: accept, amend (for example the toast, debug-DB writes, or (b) restricted to the run) or reject.
- A firmware update changes the key filter, the configd key or `voiceconductor`.
- LG offers a documented setting to assign the mic key to a third party.
