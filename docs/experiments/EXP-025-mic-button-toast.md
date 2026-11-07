# EXP-025 — Magic Remote mic button: where LG's "not supported" toast comes from, and whether a scoped switch avoids it

> **Every on-TV step below needs the owner's authorization for that run.** TV access is otherwise install-only. Part A is read-only (run once, 2026-11-07). Part B (rewritten after Part A) sets an in-memory override of the compositor's key-filter list for at most 180 s and restores it at once. Nothing here may run on the strength of this document alone.

**Status:** **Part A run 2026-11-07 (about 20:15 to 20:30 CET), read-only, owner-authorized for this one run** ([results](#part-a-results-2026-11-07)). It produced one candidate (an in-memory override of the key-filter list, [research 10.7](../research/remote-mic-button.md#107-part-a-on-c3-a-the-key-filter-list-is-a-candidate-2026-11-07), draft [ADR 0015](../decisions/0015-silent-mic-key-filter.md), proposed). **Part B: not authorized, not run**; rewritten after Part A as a trial of that candidate ([Part B](#part-b-trial-of-the-key-filter-candidate-not-authorized)).
**Date:** written 2026-11-07; Part A run 2026-11-07; Part B rewritten 2026-11-07
**Device alias / model / firmware / webOS / architecture:** c3-a (LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel, 32-bit ARM userland), Magic Remote `LGE MR23` class (c3-a's remote model is not recorded). c3-b is not part of this experiment.
**Power and playback state:** `Active`; LG hands-free ("Hi LG") **off** (unchanged); Part A found the satellite with the mic button feature **on** (as the owner left it) and changed nothing; the rewritten Part B wants it **on** (`lg_voice_key` `ours (configd)`, so a failed handler falls back to today's toast, not to LG's assistant)
**Related research / requirements / issue:** [remote mic button research section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07), [EXP-023](EXP-023-remote-mic-button-handover.md) (the owner's 0.5.0 session that reported the toast), [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md), [ADR 0014](../decisions/0014-assistant-overlay.md)
**Risk:** Part A **read-only** (`getConfigs` of named keys, file listings, hashes, `cat` and `grep` of named compositor files and catalogues, a filtered log `grep`). Part B (rewritten, not authorized) **configuration change**: an in-memory override of `com.webos.surfacemanager.keyFilters` that adds one handler of ours, a few minutes, restored with `null` and read back; it runs our JavaScript inside LG's compositor (see the risks there).
**Approval needed / obtained:** Part A: **obtained** (owner, through the coordinator, 2026-11-07: "Part A only, read-only, no button press, this one run"). Part B: needed, **not obtained**.

## Question and falsifiable hypothesis

Desk research ([section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07)) found, from the files shipped on c3-a, that the toast comes from the compositor's key filter and that **no value** of `com.webos.keyaction.voice` avoids it. Part A checks that reading against the running TV and settles the open hypotheses; it does not try to remove the toast. (Part A falsified H4, which produced the one candidate that the rewritten Part B would try.)

- **H1 (the filter that runs is the one read):** the active key-filter chain (`com.webos.surfacemanager.keyFilters`) includes `appLaunch.js` `handleHotkeys`, and the files on the TV have the same SHA-256 as the copies analysed. Falsified by a different file, a different hash or a handler order in which another filter decides the voice key first.
- **H2 (the toast is the key filter's):** with ADR 0013's override in place, one press produces a `createToast` from the key filter (`com.webos.starfish.keyfilters` or `surface-manager` as the caller) and no `voiceconductor` `setInputEvent`. Falsified by a toast logged from another caller (for example `voiceconductor` or `lginput2`).
- **H3 (the text):** the Spanish catalogue translates the key filter's source string "This function is not supported." (context `appLaunch`) as "Función no disponible". Falsified if that string has another translation and "Función no disponible" belongs to "This function is not available." or to another component.
- **H4 (open, for section 10.5):** the key-filter loader accepts only files in `/usr/lib/qml/KeyFilters/` (so a filter of our own cannot be loaded at all), and `blockKeyFilterNonCardTypeApps` / `allowedKeyMap` come from a source a third-party app cannot set. Either result is informative; neither leads to a trial in this experiment.

## Prerequisites and safe baseline

- **Authorization:** Part A (read-only) and Part B each need the owner's yes. Part B needs the owner at the TV with the Magic Remote; the IR remote at hand.
- **Access:** SSH as root (address only in the gitignored local config). `luna-send` needs a pty on this firmware: run it directly under `ssh -tt` (Part A did so; nothing copied to the TV) ([EXP-011](EXP-011-lg-settings-toggle.md)). Wrap every TV command in `timeout`. `L` below is `timeout 10 luna-send -n 1 -w 5000 -f`.
- **Raw output:** only in the gitignored `devices/private/c3-a/EXP-025/`. Copied LG files (`.qm`, `.qml`, `.js`) stay there and are never committed. Hashes of the files analysed earlier are in `devices/private/c3-a/EXP-023/keyfilters-sha256.txt`.
- **Never:**
  - in Part A: any `setConfigs`; in Part B: `setConfigs` without `"volatile":true`, any configd key other than `com.webos.surfacemanager.keyFilters`, or any value other than LG's list plus our one entry and `null`;
  - `reconfigure`; any write under `/etc/configd`, `/var/preferences` or `/usr`;
  - reading the `voiceframework` settings category (it holds LG's security voice keys); `getVoiceKey`;
  - any call to `com.webos.notification` (not `disable`, not `closeToast`, not `createToast`);
  - a grab (`EVIOCGRAB`), a hidraw read, signalling or pausing any LG process, debugfs or ASoC `dapm_widget` (they caused a kernel oops on this TV);
  - copying the system log: it is read only through the `grep` patterns given.
- **Baseline (read-only, record it, Part A and Part B):**
  - `ctl.sh status --json`: note `mic_button`, `mic_button_active`, `lg_voice_key` (Part B wants the feature on: `true`, `ours (configd)`);
  - `$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.keyaction.voice"]}'`: `launch` with the feature off, `{"action":"ignore"}` with it on;
  - `ls -l /var/preferences/ | grep configd` and `ls /tmp | grep -c '^configd_'`.

## Commands / steps (copyable and bounded)

### Part A: read-only inspection (about 15 to 20 minutes, no key press needed)

```sh
# A1 the active key-filter chain (one named config key; no category read)
$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.surfacemanager.keyFilters"]}'

# A2 the key-filter files: list and hash (compare on the PC with EXP-023/keyfilters-sha256.txt)
ls -l /usr/lib/qml/KeyFilters/
sha256sum /usr/lib/qml/KeyFilters/*.js /usr/lib/qml/WebOSCompositor/controllers/StarfishKeyFilter.qml

# A3 how the loader resolves a filter "file" (copy both files to the private directory)
ls -l /usr/lib/qml/WebOSCompositor/controllers/base/
cat /usr/lib/qml/WebOSCompositor/controllers/base/StarfishKeyFilterLoader.qml
grep -n -e 'import' -e 'Qt.include' -e 'filename' /usr/lib/qml/WebOSCompositor/controllers/base/StarfishKeyFilterLoader.qml
#   then cat the shell script it names (KeyFilterShell.js or similar), found by that grep, never guessed

# A4 where the app-scoped bypass list and access.keyAction come from (lines only, cut, bounded)
timeout 20 grep -rn -e blockKeyFilterNonCardTypeApps -e allowedKeyMap -e 'keyAction' /usr/lib/qml/WebOSCompositor 2>/dev/null | cut -c1-300 | head -60

# A5 the translation catalogue of the key filter
timeout 30 find /usr/share /usr/lib/qml -xdev -name '*.qm' 2>/dev/null | grep -i -e keyfilter -e starfish -e surface -e applaunch | head -40
#   then ls -l of the Spanish candidates (names with "es"), and cat each one (binary) over ssh into
#   devices/private/c3-a/EXP-025/; on the PC, read the Qt .qm tags (context, source text, translation)
#   for context "appLaunch" and the sources "This function is not supported." and
#   "This function is not available."

# A6 the notification service as declared on the TV (files only; the service is not called)
ls /usr/share/luna-service2/services.d/ /usr/share/luna-service2/api-permissions.d/ | grep -i notif
#   then cat each file listed (method list of com.webos.notification on this build)
```

Part A changes nothing. If any path above does not exist, record that and do not search more widely than the directories named.

### Part B: trial of the key-filter candidate (NOT authorized)

> Rewritten after Part A. The original Part B (one press under ADR 0013's override with a log `grep` for the toast's caller) is mostly answered by the log of the owner's own session (A7); the remaining question, the exact toast text, needs only the owner's eyes. This Part B tries the candidate instead. **It needs the owner's explicit authorization for the run**, the owner at the TV with the Magic Remote, the IR remote and the phone at hand, and an agent session kept open for the restore. It changes a configd key that ADR 0013 does not cover, and runs our code inside LG's compositor; it is an experiment only, never product behaviour, until [ADR 0015](../decisions/0015-silent-mic-key-filter.md) is decided.

**State before:** the satellite running with the mic button feature **on** (so `com.webos.keyaction.voice` = `ignore` is the fallback: if our handler does not load, the press shows today's toast, never LG's assistant). Time on the TV: about 15 minutes.

1. **Save LG's list** (PC side): `$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.surfacemanager.keyFilters"]}'` into `devices/private/c3-a/EXP-025/b-base.json`; it must equal A1 (18 entries).
2. **Place the handler** (a write to `/tmp`, tmpfs):
   ```sh
   D=$(mktemp -d /tmp/vs-keyfilter.XXXXXX); chmod 700 "$D"; ls -ld "$D"   # root, drwx------
   cat > "$D/voicekey.js" <<'JS'
   // webos-voice-satellite EXP-025: consume only the Magic Remote mic key, silently.
   function init() {}
   function handleSatelliteVoiceKey(key, pressed, autoRepeat) {
       return key === WebOS.Key_webOS_Voice ? KeyPolicy.Accepted : KeyPolicy.NextPolicy;
   }
   JS
   chmod 600 "$D/voicekey.js"; sha256sum "$D/voicekey.js"
   ```
3. **Dead-man restore** (started before the change; it restores after 180 s whatever happens to the session):
   `( sleep 180; $L luna://com.webos.service.config/setConfigs '{"configs":{"com.webos.surfacemanager.keyFilters":null},"volatile":true}' ) >/dev/null 2>&1 &`
4. **Apply** (one call, `volatile`): the list from step 1 with **one** entry `{"handler":"handleSatelliteVoiceKey","file":"<D>/voicekey.js"}` inserted directly before `handleHotkeys` (built on the PC with a JSON tool, never typed by hand), sent as `setConfigs {"configs":{"com.webos.surfacemanager.keyFilters":[…19 entries…]},"volatile":true}`. Read back with `getConfigs`: 19 entries, ours 13th.
5. **Owner, within the 180 s:**
   - one silent tap of the mic button: expected **no toast**, no LG screen; our overlay and the HA run as usual;
   - Home, Back, OK, the arrows, volume up and down, Settings (gear), the input key: each behaves normally;
   - optionally one more mic tap held about 2 s.
6. **Restore at once** (do not wait for step 3): `setConfigs {"configs":{"com.webos.surfacemanager.keyFilters":null},"volatile":true}`; `getConfigs` must equal `b-base.json` exactly (compare on the PC); then kill the dead-man job.
7. **Owner:** one silent mic tap: the toast is back (the ADR 0013 fallback); Home and Settings normal.
8. **Evidence, read-only:** `ls /tmp | grep -c '^configd_'`, `ls -l /var/preferences/ | grep configd` (no `configd_factory_db.json`); the filtered log of the window: `grep -a -e NL_BUTTON_CLICK -e '"com.webos.app.toast"' -e 'com.webos.app.voice' -e 'voicesatellite.overlay'` on `/var/log/messages` (and the newest rotated file) from the step 4 time on, plus `grep -a -i -e 'keyfilter' -e 'voicekey.js'` for the loader's console lines if they are logged.
9. **Clean up:** `rm -r "$D"`; `ctl.sh status --json` unchanged (feature on, `lg_voice_key` `ours (configd)`).

## Expected evidence and failure interpretation

| Signal | Supports | Against / interpretation |
| --- | --- | --- |
| A1 lists `appLaunch.js` `handleHotkeys`, after `preProcess` handlers and with no other handler before it that switches on the voice key | H1 | Another file or handler decides the voice key first: re-read that file (read-only) before any conclusion; section 10 then needs a revision |
| A2 hashes equal to `EXP-023/keyfilters-sha256.txt` | H1: section 10's static reading is the running code | Different hash: the TV changed (firmware update?); copy the new file privately and redo section 10.1 |
| A3 loader builds the path inside `/usr/lib/qml/KeyFilters/` | H4: no filter of ours can be loaded through `com.webos.surfacemanager.keyFilters`; section 10.5's first path is closed | An absolute or free path is accepted: the path stays **possible but not recommended** (code inside the compositor; new ADR needed). Not tried here |
| A4 shows the source of `blockKeyFilterNonCardTypeApps` / `allowedKeyMap` (for example a configd key or an appinfo property) | Settles 10.5's app-scoped bypass | A source a third-party app can set: still limited to our app being in front (the overlay is not, EXP-024); record only |
| A5: context `appLaunch`, "This function is not supported." → "Función no disponible" | H3; the English UI text is "This function is not supported." | Another translation: the toast may be "This function is not available." or another component's; Part B's log caller decides |
| A6 method list without per-source suppression | 10.4: no scoped notification switch on this build | A per-source or per-app method exists: record it; using it would need a new ADR (it changes LG's notification service), and it must not hide other system toasts |
| Part B step 4: 19 entries read back, ours 13th | The key takes a volatile override | Refused or not read back: configd treats this key differently; stop, restore, record |
| Step 5: no `com.webos.app.toast` window after the mic press, no LG voice app, our overlay shows | The candidate works: only the mic key is consumed, silently | Toast still shown: the compositor did not reload, or our file did not load (look for the loader's lines); restore. LG's screen: impossible with `ignore` in place unless the chain broke; restore at once |
| Step 5: Home, Back, OK, arrows, volume, Settings, input normal | The rest of the chain was rebuilt intact | Any key dead or wrong: restore at once (step 6, or the dead-man job, or a TV restart); the candidate is rejected unless the cause is understood |
| Step 6: read-back equal to `b-base.json`; step 7 toast back | Restore is as simple as for the voice key | Different list: restore again; if it persists, a TV restart clears it (owner approval) |
| Step 8: no `configd_factory_db.json`; `configd_*` bookkeeping counted | Nothing persistent | Factory DB appears: follow EXP-023's C1 rollback row, report |

## Observations and redacted evidence links

### Part A results (2026-11-07)

Run read-only over `ssh` from the PC (no file written on the TV: no script copied, `luna-send` run directly under `ssh -tt`; the Spanish catalogues streamed with `tar cf -` to the PC). Raw output in the gitignored `devices/private/c3-a/EXP-025/`. The satellite was left as the owner had it: mic button feature **on** (`com.webos.keyaction.voice` read `{"action":"ignore"}`), overlay installed. Labels: `verified-on-target` (c3-a, `03.30.74`) for what was read; the meaning of the code is static reading.

| Step | Result | Hypothesis |
| --- | --- | --- |
| A0 baseline | `com.webos.keyaction.voice` = `{"action":"ignore"}` (the satellite's override, feature on) | — |
| A1 key-filter chain | `com.webos.surfacemanager.keyFilters` is a list of 18 `{handler, file}` entries with **absolute paths** under `/usr/lib/qt5/qml/KeyFilters/` (`/usr/lib/qml` is the same directory): `preProcess.js handleKeyBlockByApp`, `systemUi.js handleScreenSaver`, 8 `appLaunch.js` handlers (hotel mode, accessibility, AV reset, FreeSync info, factory reset, panel/UTT USB download, external vendor overlays, low-speed data broadcast), `systemUi.js handleGamepadKeys`, `handleSystemKeys`, then **`appLaunch.js handleHotkeys`** (the voice-key branch, 13th), `handleCPHotkeys`, `handleFactoryKeys`, `handleTvhotkeyAppLaunch`, `handleLocalKeys`, `systemUi.js handleMagicNumber`. No entry before `handleHotkeys` handles `Key_webOS_Voice` (static, the files below). **There is no per-key entry**: the voice key is one case among dozens inside `handleHotkeys` | H1 supported |
| A2 hashes | All 5 `KeyFilters/*.js` and `StarfishKeyFilter.qml` have the same SHA-256 as the copies analysed in section 10 | H1 supported |
| A3 loader | `StarfishKeyFilterLoader.qml` creates one object per `file` and calls `Shell.load(filename)`; `StarfishKeyFilterShell.js` `load()` prefixes `file://` and runs `Qt.include(Qt.resolvedUrl(file))`, then `init()` in a `try`. **No directory check**: any absolute path is included | H4 (loader restricted) **falsified** |
| A4 bypass and access sources | `blockKeyFilterNonCardTypeApps` is the configd key `com.webos.surfacemanager.blockKeyFilterNonCardTypeApps` (`StarfishViewStateController.qml`); `allowedKeyMap` is filled by a compositor event `registerAllowedKey` from an app and cleared only for `com.webos.app.remotecontrolguide`; `access.keyAction.activateXinput` is the **fullscreen app's** window property `_WEBOS_ACCESS_POLICY_KEYACTION_ACTIVATE_XINPUT` (`StarfishAccessControlPolicy.qml`), so only the app in front can silence the key, for itself | settles 10.5 rows; none narrow for us |
| A5 Spanish text | The key filter's catalogue (`/usr/share/qml/locales/starfish-systemui-keyfilters/…_es_ES.qm` and `…_es.qm`, context `appLaunch`): "This function is not supported." → **"Esta función no es compatible."**; "This function is not available." → "Esta función no está disponible". **"Función no disponible" occurs in none of the 48 Spanish catalogues** under `/usr/share/qml/locales` (27 components). The nearest are "La función no está disponible." (`com.webos.app.voice`, `com.webos.app.voiceagent`: "Function is unavailable.") and "Esta función no está disponible." (several) | H3 **falsified as worded**; see below |
| A6 notification service | The TV's `com.webos.notification` ACG declares 22 methods (create/close toast, alert, input alert, PIN prompt, signage alert, notification; `enable`, `disable`, the `get*Notification` subscriptions, `removeNotification`, `removeAllNotification`); **no per-source or per-app suppression method**. `com.webos.service.independanotification` is unrelated (video-call service) | 10.4 confirmed on target |
| A7 filtered log of the owner's session (rotated `/var/log/messages.*.gz`, `grep` for the button, window and voice-app lines only) | 19:10:09 UTC, a `KEY_VOICE` press **before** the feature was on: LG's `com.webos.app.voiceagent`, then `com.webos.app.voice` launched by `com.webos.service.voice.performer.plugin` (LG's flow). 19:10:45, 19:11:01, 19:11:14 UTC, three presses **with** the feature on: each `NL_BUTTON_CLICK KEY_VOICE` is followed **12 to 58 ms** later by `LSM NL_VSC {"app_id":"com.webos.app.toast","visible":true,"window_type":"_WEBOS_WINDOW_TYPE_TOAST"}`, hidden again **5.0 s** later; **no** `com.webos.app.voice` or `voiceagent` window; our overlay (`io.github.pando85.voicesatellite.overlay`, window type `overlay`) visible 0.6 to 2.0 s after the press. No line names the toast's caller or text (the notification service logs nothing at this level) | H2 supported by timing (the toast is drawn by LG's toast window within 60 ms of the press, with no LG voice app involved), caller not logged |

**The toast text.** The owner quoted "Función no disponible". The key filter's string on this TV is "Esta función no es compatible."; no catalogue holds "Función no disponible". Two readings:
- (a) the owner paraphrased the key filter's toast (**hypothesis, preferred**): the toast follows each press within 12 to 58 ms, LG's voice apps never appear, and the key filter is the only component known to send a toast on that path;
- (b) another component sent a toast with a "no está disponible" text (**hypothesis**, weaker): it would have to do so within 60 ms of the press without any LG voice window opening.
- The owner can settle it by reading the toast once more, with no agent access to the TV. English source string under (a): "This function is not supported." (`verified-on-target`, catalogue).

**The main question: can a narrow in-memory configd change stop the toast?**
- **Volatile override of `com.webos.surfacemanager.keyFilters`:** configd stores any key with `"volatile":true` in its in-memory database (upstream `Configd::setConfigs`: the whole `configs` object goes to `onSetConfigs(…, true)`; `documented`), the same mechanism already used live for `com.webos.keyaction.voice` (`verified-on-target`, 0.5.0). The compositor subscribes to the key and rebuilds the chain on every change (`StarfishKeyFilter.qml`, static). Untested for this key: `hypothesis` until Part B.
- **Removing or changing only the voice-key entry:** not possible; there is no such entry (A1). Removing `handleHotkeys` would send the voice key to the compositor's fallback (`defaultKeyEventHandler`: `NotAccepted`, so the key goes to the focused app, no toast, no LG; static) but would also drop every hotkey that handler serves (Settings, Guide, inputs, app keys…). **Rejected.** LG's existing handlers that accept the voice key silently are not narrow (10.5).
- **Adding one handler of ours:** because the loader includes any absolute path (A3), a list equal to LG's with one extra entry before `handleHotkeys`, pointing to a 3-line file of ours that returns `KeyPolicy.Accepted` for `Key_webOS_Voice` and `KeyPolicy.NextPolicy` for every other key, would consume only the mic key: **no toast, no LG** (`hypothesis`, static basis). The satellite still sees the press through its passive evdev reader (that path is below the compositor). This is the candidate of [research 10.7](../research/remote-mic-button.md#107-part-a-on-c3-a-the-key-filter-list-is-a-candidate-2026-11-07) and draft [ADR 0015](../decisions/0015-silent-mic-key-filter.md).
- **Restore:** `setConfigs {"configs":{"com.webos.surfacemanager.keyFilters":null},"volatile":true}` removes the in-memory entry (upstream `Manager::updateFactoryDatabase`: `null` → `remove`), so configd serves LG's base list again and the compositor rebuilds it; then read back and compare with the list saved before. As simple as for the voice key (`documented` + static), untested.
- **Placement of the file:** `surface-manager` runs as root (uid 0). The app directories under `/media/developer/apps/usr/palm/applications/` are **world-writable (mode 0777)** on this TV, as are their parents, so a filter file there could be replaced by any local process and would run as root inside the compositor. The file must live in a root-owned, private directory (for example a fresh `mktemp -d` under `/tmp`, mode 0700, which a reboot clears together with the override). Side note for the project: the satellite's own install directory is world-writable too (`verified-on-target`); not investigated here.

## Conclusion and confidence

- **Part A:** H1 supported (the filter that runs is the one analysed). H2 supported by timing and by the absence of LG's voice apps; the caller is not logged. H3 falsified as worded: the key filter's Spanish text is "Esta función no es compatible."; the owner's "Función no disponible" is most likely a paraphrase (to confirm by looking at the toast). H4 falsified: the loader accepts any absolute path, which turns the key-filter list into a working, if invasive, candidate. Confidence: high for the file facts, medium for the runtime behaviour of the candidate (untested).
- **No value of `com.webos.keyaction.voice` avoids the toast** (unchanged).
- **One candidate:** an in-memory override of `com.webos.surfacemanager.keyFilters` that adds a silent voice-key handler of ours before `handleHotkeys`. It needs Part B, and a decision on draft [ADR 0015](../decisions/0015-silent-mic-key-filter.md) before any product use.

## Rollback and cleanup

- **Part A:** nothing to roll back. `rm -f /tmp/exp025*`.
- **Part B (if ever run):**
  - `setConfigs {"configs":{"com.webos.surfacemanager.keyFilters":null},"volatile":true}`, read back equal to the saved list. The dead-man job does the same after 180 s.
  - If keys misbehave and no shell is reachable: a TV restart clears every volatile override and `/tmp` (owner, with the IR remote or the power button).
  - `rm -r` the `mktemp` directory; delete `/tmp/configd_*_before_setConfigs.json` files created after the baseline; record `configd_debug_db.json` as in EXP-023.
- Raw files stay in `devices/private/c3-a/EXP-025/` until the owner has seen the results; nothing raw is committed.

## Follow-ups / unresolved questions

- The owner reads the toast's exact text once more (no agent access needed): "Esta función no es compatible." confirms the key filter.
- Decide draft [ADR 0015](../decisions/0015-silent-mic-key-filter.md) (proposed): run Part B, or accept the toast as the cost of ADR 0013 (a).
- UX mitigation if the toast stays (owner's observation of where it sits): if it overlaps our panel, move or delay the overlay's first frame (design change, no LG change). The TV app's note could then say that the toast appears, not that it "may".
- c3-b (`03.30.73`) and any firmware update: repeat A1 and A2 before trusting section 10 there.
