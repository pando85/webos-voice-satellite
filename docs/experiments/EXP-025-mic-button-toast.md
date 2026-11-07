# EXP-025 — Magic Remote mic button: where LG's "not supported" toast comes from, and whether any scoped switch avoids it

> **Every on-TV step below needs the owner's authorization for that run.** TV access is otherwise install-only. Part A is read-only. Part B sets ADR 0013's own in-memory override for about one minute and restores it at once. Nothing here may run on the strength of this document alone.

**Status:** PLANNED, **not run**
**Date:** written 2026-11-07
**Device alias / model / firmware / webOS / architecture:** c3-a (LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel, 32-bit ARM userland), Magic Remote `LGE MR23` class (c3-a's remote model is not recorded). c3-b is not part of this experiment.
**Power and playback state:** `Active`; LG hands-free ("Hi LG") **off** (unchanged); the satellite running with the mic button feature **off** for Part B (`mic_button_active` false, `lg_voice_key` `lg`)
**Related research / requirements / issue:** [remote mic button research section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07), [EXP-023](EXP-023-remote-mic-button-handover.md) (the owner's 0.5.0 session that reported the toast), [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md), [ADR 0014](../decisions/0014-assistant-overlay.md)
**Risk:** Part A **read-only** (one `getConfigs` of named keys, file listings, hashes, `cat` and `grep` of named compositor files, a filtered log `grep`). Part B **configuration change**: exactly ADR 0013's volatile override of `com.webos.keyaction.voice` (`{"action":"ignore"}`), about one minute, restored with `null` and read back as `launch`. No other configd key or value is set.
**Approval needed / obtained:** Part A and Part B each need the owner's yes. **Not obtained.**

## Question and falsifiable hypothesis

Desk research ([section 10](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07)) found, from the files shipped on c3-a, that the toast comes from the compositor's key filter and that **no value** of `com.webos.keyaction.voice` avoids it. This experiment checks that reading against the running TV and settles the open hypotheses. It does **not** try to remove the toast: no in-scope candidate exists.

- **H1 (the filter that runs is the one read):** the active key-filter chain (`com.webos.surfacemanager.keyFilters`) includes `appLaunch.js` `handleHotkeys`, and the files on the TV have the same SHA-256 as the copies analysed. Falsified by a different file, a different hash or a handler order in which another filter decides the voice key first.
- **H2 (the toast is the key filter's):** with ADR 0013's override in place, one press produces a `createToast` from the key filter (`com.webos.starfish.keyfilters` or `surface-manager` as the caller) and no `voiceconductor` `setInputEvent`. Falsified by a toast logged from another caller (for example `voiceconductor` or `lginput2`).
- **H3 (the text):** the Spanish catalogue translates the key filter's source string "This function is not supported." (context `appLaunch`) as "Función no disponible". Falsified if that string has another translation and "Función no disponible" belongs to "This function is not available." or to another component.
- **H4 (open, for section 10.5):** the key-filter loader accepts only files in `/usr/lib/qml/KeyFilters/` (so a filter of our own cannot be loaded at all), and `blockKeyFilterNonCardTypeApps` / `allowedKeyMap` come from a source a third-party app cannot set. Either result is informative; neither leads to a trial in this experiment.

## Prerequisites and safe baseline

- **Authorization:** Part A (read-only) and Part B (override, one press) each need the owner's yes. Part B needs the owner at the TV with the Magic Remote; the IR remote at hand.
- **Access:** SSH as root (address only in the gitignored local config). `luna-send` needs a pty on this firmware: copy each script to `/tmp/exp025*.sh` and run it with `ssh -tt` ([EXP-011](EXP-011-lg-settings-toggle.md)). Wrap every TV command in `timeout`. `L` below is `timeout 10 luna-send -n 1 -w 5000 -f`.
- **Raw output:** only in the gitignored `devices/private/c3-a/EXP-025/`. Copied LG files (`.qm`, `.qml`, `.js`) stay there and are never committed. Hashes of the files analysed earlier are in `devices/private/c3-a/EXP-023/keyfilters-sha256.txt`.
- **Never:**
  - `setConfigs` without `"volatile":true`; any configd key other than `com.webos.keyaction.voice`; any value other than `{"action":"ignore"}` and `null`;
  - `reconfigure`; any write under `/etc/configd`, `/var/preferences` or `/usr`;
  - reading the `voiceframework` settings category (it holds LG's security voice keys); `getVoiceKey`;
  - any call to `com.webos.notification` (not `disable`, not `closeToast`, not `createToast`);
  - a grab (`EVIOCGRAB`), a hidraw read, signalling or pausing any LG process, debugfs or ASoC `dapm_widget` (they caused a kernel oops on this TV);
  - copying the system log: it is read only through the `grep` patterns given.
- **Baseline (read-only, record it, Part A and Part B):**
  - `ctl.sh status --json`: note `mic_button`, `mic_button_active`, `lg_voice_key` (Part B requires `false` and `lg`);
  - `$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.keyaction.voice"]}'`, expected `{"action":"launch","id":"com.webos.app.voice","params":{"activateType":"mrcu"}}`;
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

### Part B: toast attribution under ADR 0013's override (about 5 minutes, owner at the TV, one silent press)

The satellite's mic button feature must be **off** (baseline), so the satellite holds no override and no state file. The script restores on any exit.

```sh
# /tmp/exp025b.sh, run with: ssh -tt <tv> 'sh /tmp/exp025b.sh'
L="timeout 10 luna-send -n 1 -w 5000 -f"
LOG=/var/log/messages
restore() {
  $L luna://com.webos.service.config/setConfigs '{"configs":{"com.webos.keyaction.voice":null},"volatile":true}'
  $L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.keyaction.voice"]}'   # must read "launch"
}
trap restore EXIT INT TERM
ls -l /var/preferences/ | grep configd; ls /tmp | grep -c '^configd_'
N=$(wc -l < "$LOG"); date -u +%H:%M:%S
# B1 set exactly ADR 0013's value, in memory only, and read it back
$L luna://com.webos.service.config/setConfigs '{"configs":{"com.webos.keyaction.voice":{"action":"ignore"}},"volatile":true}'
$L luna://com.webos.service.config/getConfigs '{"configNames":["com.webos.keyaction.voice"]}'      # expect {"action":"ignore"}
echo "OWNER: one short silent tap of the mic button now"; sleep 15
# B2 restore at once (also done by the trap), then the filtered log of the window only
restore; trap - EXIT INT TERM
M=$(wc -l < "$LOG"); [ "$M" -ge "$N" ] || N=0   # the log rotated: search the whole (new) file
tail -n +$((N+1)) "$LOG" | grep -a -i -e createToast -e 'Caller:' -e 'starfish.keyfilters' -e NL_BUTTON_CLICK -e setInputEvent -e 'com.webos.app.voice' | cut -c1-300 | head -40
ls -l /var/preferences/ | grep configd; ls /tmp | grep -c '^configd_'
```

- **During B1 the owner notes:** the toast's exact text, where it appears on screen (corner or centre, relative to where our overlay panel sits, bottom centre), and roughly how long it stays.
- **B3 (owner):** one more silent tap after the script ended: LG's voice screen must open (the restore works). Close it with Back.

## Expected evidence and failure interpretation

| Signal | Supports | Against / interpretation |
| --- | --- | --- |
| A1 lists `appLaunch.js` `handleHotkeys`, after `preProcess` handlers and with no other handler before it that switches on the voice key | H1 | Another file or handler decides the voice key first: re-read that file (read-only) before any conclusion; section 10 then needs a revision |
| A2 hashes equal to `EXP-023/keyfilters-sha256.txt` | H1: section 10's static reading is the running code | Different hash: the TV changed (firmware update?); copy the new file privately and redo section 10.1 |
| A3 loader builds the path inside `/usr/lib/qml/KeyFilters/` | H4: no filter of ours can be loaded through `com.webos.surfacemanager.keyFilters`; section 10.5's first path is closed | An absolute or free path is accepted: the path stays **possible but not recommended** (code inside the compositor; new ADR needed). Not tried here |
| A4 shows the source of `blockKeyFilterNonCardTypeApps` / `allowedKeyMap` (for example a configd key or an appinfo property) | Settles 10.5's app-scoped bypass | A source a third-party app can set: still limited to our app being in front (the overlay is not, EXP-024); record only |
| A5: context `appLaunch`, "This function is not supported." → "Función no disponible" | H3; the English UI text is "This function is not supported." | Another translation: the toast may be "This function is not available." or another component's; Part B's log caller decides |
| A6 method list without per-source suppression | 10.4: no scoped notification switch on this build | A per-source or per-app method exists: record it; using it would need a new ADR (it changes LG's notification service), and it must not hide other system toasts |
| B1 reads back `ignore`; the press shows the toast; the log shows `createToast` with the key filter as caller and `NL_BUTTON_CLICK`, no `setInputEvent` | H2 | Caller is `voiceconductor`, `lginput2` or another: the toast is not the key filter's; section 10 is wrong on the component and needs revision. LG's screen opens instead: the override did not reach the filter this time (compare EXP-023) |
| B2 reads back `launch`; B3 opens LG's screen | Restore works (also a datum for EXP-023's residual unknown 7) | Not `launch`, or no LG screen: follow the rollback below |
| B `configd_*` listings before and after | Records configd's bookkeeping (`/tmp/configd_*_before_setConfigs.json` count, `configd_debug_db.json` appearing or not) for ADR 0013 | `configd_factory_db.json` appears: stop, follow EXP-023's C1 rollback row for a persistent entry, report |

## Observations and redacted evidence links

Not run.

## Conclusion and confidence

Not run. Prior (static, c3-a files): high confidence for H2 and for "no value of the key avoids the toast"; medium for H3 (translation not read); H4 unknown.

## Rollback and cleanup

- **Part A:** nothing to roll back. `rm -f /tmp/exp025*`.
- **Part B:**
  - The script's `trap` restores on any exit. By hand: `setConfigs {"configs":{"com.webos.keyaction.voice":null},"volatile":true}`, then `getConfigs` must read `launch`.
  - If it does not read `launch`, or LG's screen does not open in B3: a TV restart clears every volatile override (owner approval). `reconfigure` is not used.
  - `setConfigs` writes `/tmp/configd_<time>_before_setConfigs.json`: delete the ones created after the baseline. If `/var/preferences/configd_debug_db.json` appeared and was absent at the baseline, record it; deleting it needs the owner's approval (EXP-023 rollback).
  - The satellite was not touched (feature off): check that `ctl.sh status --json` still shows `lg_voice_key` `lg` and the same pid.
- Raw files stay in `devices/private/c3-a/EXP-025/` until the owner has seen the results; nothing raw is committed.

## Follow-ups / unresolved questions

- If H1 to H3 hold: the toast is accepted as the cost of ADR 0013 (a), unless the owner wants one of the paths of [section 10.5](../research/remote-mic-button.md#10-removing-the-mic-button-toast-2026-11-07), each of which needs an ADR 0013 amendment (`voiceconductor` pause) or a new ADR (key-filter chain, another configd key, a notification change).
- UX mitigation from Part B's owner notes: if the toast overlaps our panel, move or delay the overlay's first frame (design change, no LG change). The TV app's note could then say that the toast appears, not that it "may".
- c3-b (`03.30.73`) and any firmware update: repeat A1 and A2 before trusting section 10 there.
