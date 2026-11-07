# EXP-023 — Magic Remote mic button: handing the key from LG's assistant to the satellite

**Status:** **C1 through the satellite: owner-reported PASS with one negative finding** (c3-a, satellite 0.5.0, 2026-11-07 evening; `verified-on-target (owner-reported, no logs captured)`; see [Observations](#observations-and-redacted-evidence-links)). Negative finding: every mic press shows LG's toast "Función no disponible" while the override is in place (follow-up: [research section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07), [EXP-025](EXP-025-mic-button-toast.md)). C0 not run; C2 not needed (C1 did not fail) and not run. The hand-typed C1 steps were not run. Earlier status, kept: PLANNED, **not run** (needs owner authorization per step). The read-only inspection and static analysis that motivate it ran on 2026-11-07 and are recorded in [research section 9](../research/remote-mic-button.md#9-hand-over-candidates-configd-and-the-key-filter-static-2026-11-07). **Update 2026-11-07:** [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md) is accepted for the configd override and the satellite now applies it itself (design note section 7). **C1 is therefore run as the owner's first enable session of the mic button** ([C1 through the satellite](#c1-through-the-satellite-the-owners-first-enable-session) below), not with hand-typed `luna-send` calls; C0 stays optional, C2 only if C1 fails.
**Date:** written 2026-11-07
**Device alias / model / firmware / webOS / architecture:** c3-a (LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel, 32-bit ARM userland), Magic Remote `LGE MR23`. c3-b is not part of this experiment.
**Power and playback state:** `Active`, no playback started by the agent, LG hands-free ("Hi LG") **off** (unchanged), installed satellite running unchanged
**Related research / requirements / issue:** [remote mic button research](../research/remote-mic-button.md), [EXP-022](EXP-022-remote-mic-button-observe.md) (key trace and remote stream), proposed [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md); requirement F7 (truthful listening indicator)
**Risk:** **configuration change** (C1: an in-memory configd override; C2: pausing an LG process). Both are meant to revert on a reboot, but C1 also writes small configd bookkeeping files (see C1 rollback). Optional consented speech in C0b and C1b.
**Approval needed / obtained:** needed separately for C0, C1 and C2. **Not obtained.** C2 runs only if C1 fails and the owner approves it after seeing C1's result.

## Question and falsifiable hypothesis

The question: can the satellite take the Magic Remote's mic button for push-to-talk without LG's assistant starting on the same press? It should do this cheaply, reversibly, and through LG's own mechanism where one exists.

1. **C0 (control, no change).** Does LG show "El procesamiento de audio es inestable" when the mic is held and the phrase spoken **without** our `startStreaming` subscription? (The open question from EXP-022 B.)
   - H-C0: no. The message in EXP-022 B came from our subscription competing with LG's capture.
2. **C1 (configd, preferred).** With the in-memory override `com.webos.keyaction.voice = {"action":"ignore"}` set through `setConfigs` with `"volatile":true`:
   - H-C1a: the compositor's key filter receives the change through its existing subscription, so it applies without a restart.
   - H-C1b: a mic press then does **not** reach `voiceconductor` (no `setInputEvent`, no `com.webos.app.voice` launch). The key filter shows the toast "This function is not supported." instead (static reading of `appLaunch.js`).
   - H-C1c: evdev code 428 on `LGE M-RCU - Builtin [0]` and the `NL_BUTTON_CLICK … KEY_VOICE` log line still appear.
   - H-C1d: our `voiceinput/startStreaming` works with no `2003` refusal.
   - H-C1e: no file `/var/preferences/configd_factory_db.json` appears (the override is not persistent).
   - H-C1f: setting the key back to `null` (volatile) restores LG's assistant at once.
   - Falsified by: LG's UI still opening (H-C1b), a factory DB file appearing (H-C1e), or LG's assistant not returning after the restore (H-C1f).
3. **C2 (fallback, only if C1 fails).** With `voiceconductor` paused by SIGSTOP:
   - H-C2: a press shows no LG UI and our stream works.
   - After SIGCONT, LG's assistant works again, and the queued key events do not open a late LG screen.

## Why these candidates and not others

From [research section 9](../research/remote-mic-button.md#9-hand-over-candidates-configuration-and-static-analysis-2026-11-07):

| Candidate | Verdict | Reason |
| --- | --- | --- |
| configd `com.webos.keyaction.voice` | **C1, best candidate** | It is LG's own per-key action config. The key filter only forwards the mic key to `voiceconductor` when the action is `launch`, and LG ships `"ignore"` for hotel mode and some product layers. `volatile` overrides stay in memory |
| SIGSTOP `voiceconductor` | **C2, fallback** | Works on webOS 25 (CleanYourTV). It stops every `voiceconductor` feature, and queued luna calls are delivered at SIGCONT |
| `/tmp/voiceinput_disable_set_input_event` | **Excluded** | Static analysis: it switches `voiceinput`'s far-field sound engine to `RUNNING_MODE_NOT_SEND_EVENT` (the "Hi LG" trigger), not the remote key. Deleting it calls `setRunningMode(0)`, which need not be the mode the engine had before, so the delete could restart LG's far-field processing next to our capture of `hw:LGSIC,0` |
| `voiceconductor/getVoiceKey` | **Excluded** | It returns LG's security voice keys (`staticVoiceKey`/`dynamicVoiceKey`), not a key binding. It is a credential, so it is not called |
| `voiceconductor/setInputEvent`, `voiceinputadapter/setInputEvent` | Not a switch | They are the trigger inputs that start LG's flow (setters) |
| LG settings (`voiceframework` category) | Not a switch | No key that routes or disables the mic key |
| `EVIOCGRAB`, `lginput2` hooks | Excluded | Lock-out risk and process injection ([research section 5](../research/remote-mic-button.md#5-risks)) |

## Prerequisites and safe baseline

- **Authorization:**
  - C0, C1 and C2 each need the owner's yes.
  - The owner is at the TV with the Magic Remote, and the IR remote is at hand.
  - Speech (C0b, C1b) needs consent. In C0b LG's assistant is active and may send the phrase to LG's cloud.
- **Access:**
  - SSH as root (address only in the gitignored local config).
  - `luna-send` needs a pty on this firmware: copy each script to `/tmp` and run it with `ssh -tt` ([EXP-011](EXP-011-lg-settings-toggle.md)).
  - Wrap every TV command in `timeout`. `L` below is `luna-send -n 1 -w 5000 -f`.
- **Raw output:** only in the gitignored `devices/private/c3-a/EXP-023/`. The system log is read only through a `grep` for `NL_BUTTON_CLICK`, `KEY_VOICE`, `voiceconductor`, `com.webos.app.voice`, `keyfilter` and `configd`. Never copy the log itself.
- **Never:**
  - `setConfigs` **without** `"volatile":true` (that writes the persistent factory DB);
  - `reconfigure`, unless as a rollback the owner approves;
  - any write under `/etc/configd` or `/var/preferences`, except what `setConfigs` itself writes (C1 rollback);
  - the flag file, `getVoiceKey`, a grab, a hidraw read, debugfs or `dapm_widget`;
  - signalling any LG process other than `voiceconductor` in C2;
  - reading the `voiceframework` settings category (it holds LG's voice keys).
- **Baseline (read-only, record it):**
  - `$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.keyaction.voice"]}'`, expected `{"action":"launch","id":"com.webos.app.voice","params":{"activateType":"mrcu"}}`;
  - `ls -l /var/preferences/ | grep configd` (on 2026-11-07 only `configd_db.json`, no `configd_factory_db.json` or `configd_debug_db.json`);
  - `ls /tmp | grep '^configd_'`;
  - `pidof voiceconductor voiceinput lginput2 surface-manager`;
  - `ctl.sh status --json` (HA connected, mic open);
  - `voiceinput/setting/getActivation` (expect everything off).
- **Readers (as EXP-022 A):**
  - a passive `cat` of the `LGE M-RCU - Builtin [0]` node, found by name in `/proc/bus/input/devices` and never assumed;
  - the filtered log;
  - the satellite's status.
  - Readers stream to the PC over ssh, so no file is written on the TV.

## Commands / steps (copyable and bounded)

### C0: control hold (no change)

- C0a: the owner taps the mic button once, **silent**, and notes what is on screen. Close it with Back.
- C0b (consent): the owner holds the mic button about 3 s and says "uno dos tres", **with no stream script running**. Note whether LG shows "El procesamiento de audio es inestable". Close it with Back.

### C1: in-memory configd override of the voice key action

```sh
# C1.1 set (volatile = memory only)
$L luna://com.webos.service.config/setConfigs '{"configs":{"com.webos.keyaction.voice":{"action":"ignore"}},"volatile":true}'
# C1.2 read back and check persistence
$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.keyaction.voice"]}'   # expect {"action":"ignore"}
ls -l /var/preferences/ | grep configd; ls /tmp | grep '^configd_'
```

- **C1.3:** the owner taps the mic button once, silent. Expected: no LG voice screen. The toast "This function is not supported." (or its Spanish text) may appear; note it. The readers show 428 press and release, plus `NL_BUTTON_CLICK`, and no `voiceconductor` `setInputEvent` line.
- **C1.4 (consent):** the EXP-022 B stream script (it counts bytes and levels in memory and never stores audio) with one hold of about 3 s and "uno dos tres". Its only change: it ends its subscription at the 428 release plus 0.5 s instead of after a 10 s cap. This is the hold-to-talk end the daemon will use, since the stream does not end at the release (EXP-022 B). Expected:
  - `socketPath` and about 31.6 kB/s;
  - no `2003`;
  - no LG message.
- **C1.5:** the owner presses Up, Back, Home, volume and the wheel once each. All must behave normally, because only the voice key action changed.

```sh
# C1.6 restore (remove the volatile override); configd then reports the base value again
$L luna://com.webos.service.config/setConfigs '{"configs":{"com.webos.keyaction.voice":null},"volatile":true}'
$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.keyaction.voice"]}'   # expect the baseline "launch" value
```

- **C1.7:** the owner taps the mic button, silent. LG's voice screen must open again. Close it with Back.

### C1 through the satellite (the owner's first enable session)

The satellite (0.4.7 or later) does C1.1, C1.2 and C1.6 itself: at its start with `[remote] mic_button = "ptt"` it reads the key (`launch` expected), writes `run/lg_voice_override`, sets the volatile override, reads it back and checks that `/var/preferences/configd_factory_db.json` was not created; any failure gives the key back and keeps the feature off with the reason in the log, `ctl.sh status` and HA. Turning the feature off, `ctl.sh stop`, `uninstall`, `purge` and the supervisor (after any daemon exit) give the key back. Prerequisites: the version deployed with `mic_button = "off"` (a separate install), the owner at the TV, LG hands-free off, consent for the C1.4 phrase. Steps:

1. **Baseline (read-only):** `ctl.sh status --json` shows `mic_button` `off`, `lg_voice_key` `lg`; `ls -l /var/preferences/` (record whether `configd_factory_db.json` and `configd_debug_db.json` exist) and `ls /tmp/configd_*_before_setConfigs.json 2>/dev/null | wc -l`. The owner taps the mic button once, silent: LG's voice screen opens (close it with Back).
2. **Enable** from the TV app (Home, Sound, "Remote mic button → Home Assistant", OK twice). Record the result line, then: daemon log `remote mic button: LG's voice key taken over (configd com.webos.keyaction.voice = ignore, in memory only ...)`; `status --json` `mic_button_active` true, `lg_voice_key` `ours (configd)`; HA: switch "Remote mic button" on, sensor "LG voice key" "Satellite (LG ignores the key)". Expected failure signal: `remote mic button: NOT running: ...` (record the reason; the app shows "Off (LG kept it)").
3. **Persistence check:** `configd_factory_db.json` still absent (the daemon also checks); record whether `configd_debug_db.json` appeared and how many new `/tmp/configd_*_before_setConfigs.json` files exist (C1 rollback).
4. **C1.3:** one silent tap: no LG voice screen; note the toast text if any. The satellite starts a run (tap-to-talk on the TV microphone; HA's end of speech ends it).
5. **C1.4 (consent):** hold about 3 s with "uno dos tres": HA shows a run with the transcript; the log shows `remote mic: stream open`, the release, `subscription closed`; no `2003`, no LG message.
6. **C1.5:** Up, Back, Home, volume and the wheel behave normally.
7. **Disable** from the app ("Give the mic button back to LG?", OK twice): log `LG's voice key given back`, `status --json` `lg_voice_key` `lg`, `run/lg_voice_override` gone. **C1.7:** one silent tap: LG's voice screen opens again.
8. **Optional, owner's choice:** with the feature on, `kill -KILL` of the daemon: the supervisor log says `LG's voice key given back`, the restarted daemon takes it again (`taken over`). Then disable as in step 7.

Rollback at any point: turn the feature off, or `ctl.sh stop` then `ctl.sh restore-lg-voice`; a TV restart also clears the in-memory override.

### C2: pause `voiceconductor` (only if C1 fails, separate approval)

```sh
VC=$(pidof voiceconductor); echo "$VC"; grep State /proc/$VC/status
kill -STOP "$VC"; grep State /proc/$VC/status          # expect "T (stopped)"
# owner: one silent tap, then the C1.4 stream script with one consented hold; at most 2 minutes stopped
kill -CONT "$VC"; grep State /proc/$VC/status          # expect "S (sleeping)" or "R"
```

- After SIGCONT, watch for 20 s whether a late LG voice screen opens (queued `setInputEvent` calls). Then the owner taps the mic button, silent: LG's screen must open normally.
- Also note whether anything else misbehaved while it was stopped: Settings, AI Service pages, the home screen.

## Expected evidence and failure interpretation

| Signal | Supports | Against / interpretation |
| --- | --- | --- |
| C0b: no "inestable" message without our stream | H-C0: the message came from contention with our subscription | Message anyway: it is LG's own reaction to the phrase or the network; C1.4 then decides whether it matters |
| C1.2: `ignore` read back; no `configd_factory_db.json` | The override applied in memory | Factory DB file appears: the shipped `volatile` handling differs from upstream. Run C1.6, then `setConfigs` with the same key `null` **without** `volatile` (removes it from the factory DB), check the file has no `keyaction` entry, stop C1 and report |
| C1.3: no LG screen, 428 and `NL_BUTTON_CLICK` present | H-C1a to c: the key filter took the change live, and the press is still observable | LG screen opens: the key filter did not see the change (subscription not notified) or another path triggers LG. Restore (C1.6), stop; INCONCLUSIVE for C1. Retrying after a compositor restart is **not** proposed |
| C1.3 toast text | Records the UX cost of C1 | No toast: better than predicted, note it |
| C1.4: stream at about 31.6 kB/s, no `2003`, no LG message | H-C1d: the satellite owns the remote's stream | `2003` with LG not started: another holder; record and stop |
| C1.5: other keys normal | Only the voice key changed | Any other key changed: restore at once; C1 rejected |
| C1.7: LG screen opens after the restore | H-C1f: runtime restore works | No LG screen: the restored value was not delivered to the key filter; LG's mic key stays off until a reboot (rollback below) |
| C2: no LG screen while stopped, normal after SIGCONT, no late screen | H-C2 | Late LG screen after SIGCONT: queued events; the daemon would have to restore only between presses. Settings or other UI hangs: C2 is unsuitable as a product mechanism |

## Observations and redacted evidence links

### Owner's hands-on session on c3-a with 0.5.0 (2026-11-07, evening)

- **Who and what:** the owner turned the mic button feature on with satellite **0.5.0** on c3-a (firmware `03.30.74`) and used it (C1 through the satellite). No agent was connected to the TV; **no logs, status output or file listings were captured**. Label for everything in this subsection: `verified-on-target (owner-reported, no logs captured)`.
- **The owner's words:** "the new feature work very well but when I click the button it shows in the TV 'Función no disponible'… The rest was tested and works like a charm in c3-a".
- **What this supports:**
  - the feature turns on and works on c3-a with 0.5.0 (hold to talk reaches Home Assistant, as reported in general terms);
  - **H-C1b, the toast part: confirmed.** Each mic press shows an LG toast. Its text on this TV (UI language Spanish) is "Función no disponible".
  - **H-C1a (live delivery), by inference:** the key filter shows that toast only in the branch where `com.webos.keyaction.voice` is not `launch` (`appLaunch.js`, static, research section 9.1). So the toast suggests that the in-memory override reached the key filter without a restart. This is an inference from the static code plus the owner's observation, not a logged fact.
- **Negative finding:** while LG's key is overridden to `{"action":"ignore"}`, **every mic-button press makes the TV show the LG toast "Función no disponible"**. The English UI string is probably the key filter's source string "This function is not supported." (`hypothesis`; the Spanish catalogue was not read, see [research section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07)). Whether it can be avoided is the subject of [EXP-025](EXP-025-mic-button-toast.md).
- **Residual unknowns (not reported in detail; each stays unconfirmed until a run with logs):**
  1. Step 1 baseline: `status --json` before enabling, the `configd_*` file listing, LG's screen on a tap before enabling.
  2. Step 2: the exact daemon log line `LG's voice key taken over`, the `status --json` fields (`mic_button_active`, `lg_voice_key` `ours (configd)`), the HA switch and the "LG voice key" sensor text.
  3. Step 3 (persistence): that `configd_factory_db.json` stayed absent, whether `configd_debug_db.json` appeared, and how many `/tmp/configd_*_before_setConfigs.json` files were created. The daemon checks the factory database itself and would have turned the feature off loudly; that it did not is consistent with "works very well", but no file listing was seen.
  4. C1.3 in detail: that **no LG voice screen** opened on a tap (not stated explicitly; "works very well" is read as yes), that 428 and `NL_BUTTON_CLICK` still appeared, and that no `voiceconductor` `setInputEvent` line was logged.
  5. C1.4 in detail: the log lines `remote mic: stream open`, the release, `subscription closed`; the absence of `2003` refusals and of LG's "El procesamiento de audio es inestable" message; the stream rate.
  6. C1.5: which other keys (Up, Back, Home, volume, wheel) were tried.
  7. Step 7 / C1.7: that turning the feature off gave the key back (`lg_voice_key` `lg`, the state file gone) and that LG's voice screen opened again on a tap.
  8. Step 8 (optional SIGKILL and the supervisor's restore): not known whether it was done.
  9. Timings (time to take over, press to stream, toast timing relative to press and release) were not reported.

## Conclusion and confidence

**Owner-reported (2026-11-07, 0.5.0, c3-a):** C1 through the satellite works as the hand-over mechanism, with the predicted cost: LG's "not supported" toast ("Función no disponible") on every press. Confidence: medium (one owner session, no logs; the residual unknowns above). The toast is the open UX issue; [research section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07) found no way to avoid it within ADR 0013's scope.

Before the run (kept): Static confidence for C1: medium. The key-filter code path is read directly from the shipped files. The configd `volatile` semantics come from the upstream source (`webosose/configd`, commit `99873ce`), and the shipped `configd` has the same symbols (`splitVolatileConfigs`, `getVolatileInstance`). That the subscription is notified on this firmware is unverified.

## Rollback and cleanup

- **C1:**
  - `setConfigs {"configs":{"com.webos.keyaction.voice":null},"volatile":true}` (C1.6).
  - If LG's key still does not open its screen, a reboot clears every volatile override (owner approval). `reconfigure` would also clear it, but it re-runs all of configd's layer selection, so it is used only with approval.
  - If a factory DB entry was written, follow the table row above.
  - `setConfigs` also writes `/tmp/configd_<time>_before_setConfigs.json` (delete the ones created after the baseline). It may create `/var/preferences/configd_debug_db.json` with only timestamps (upstream `writeDebugDatabase`). If that file did not exist at the baseline, delete it after the run, with the owner's approval, and record both facts.
- **C2:** `kill -CONT <pid>`. If LG's voice is stuck, a reboot (approval). Never `systemctl restart`.
- **All:** `rm -f /tmp/exp023*`. Raw files stay in `devices/private/c3-a/EXP-023/` until the owner has seen the results.

## Follow-ups / unresolved questions

- C1 on c3-b (`03.30.73`) and after any firmware update.
- Whether "Hi LG", the TONE+ voice button (it reads the same `voiceAction` in `appLaunch.js`) and the Alexa key behave as predicted under C1. Hi LG stays off by [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md).
- Whether the "not supported" toast can be avoided by any supported means: answered for the static side in [research section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07) (not within ADR 0013's value; read-only checks in [EXP-025](EXP-025-mic-button-toast.md)).
- A run of C1 with logs captured, to close the residual unknowns above.
- Accept or amend [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md) with the results.
