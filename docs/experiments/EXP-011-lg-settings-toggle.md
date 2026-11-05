# EXP-011 — Reading and toggling Always Ready and LG hands-free ("Hi LG") through luna

**Status:** TARGET_VERIFIED (c3-a, writes and read-backs); c3-b read-only part NOT RUN (TV unreachable)
**Date:** 2026-11-05 (06:06 to 06:12 UTC)
**Device alias / model / firmware / webOS / architecture:** c3-a, LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel; satellite 0.2.3 running (HA 2026.10.4, entry `lg-c3-a`)
**Power and playback state:** `Active`, owner's PC input, no playback, `aod/getStatus` `none`
**Related research / requirements / issue:** a TV-app screen that shows and toggles "Always Ready" and "Hi LG"; [P4 sound session](../discovery/c3-b/P4-sound-session.md) (the documented, never-run `setActivation`), [D6](../discovery/c3-b/D6-poweroff-screenoff.md) (earlier `lifeOnScreenMode` writes on c3-b), [always-on design](../design/always-on.md)
**Risk:** configuration change (two settings, both restored and read back)
**Approval needed / obtained:** owner authorized the write test on c3-a (the coordinator's brief, 2026-11-05); c3-b read-only

## Question and falsifiable hypothesis

1. Can a root process on the TV read and change `general/lifeOnScreenMode` (Always Ready) through `settingsservice`, and which values may it write?
2. Can it read and change LG hands-free through `voiceinput/setting/{getActivation,setActivation}`, and does `setActivation` need a provider parameter?
3. What does switching "Hi LG" on do to the satellite (capture PCM, `MIC BUSY`, HA **Listening** and **Microphone muted**) and how long does the satellite take to recover after switching it off?

Hypotheses: (1) yes, with the same call that D6 used on c3-b; (2) yes, with `{"settingStatus": true|false}`; (3) `voiceinput` opens the capture PCM, the satellite reports `MIC BUSY`, HA shows Listening off and Microphone muted on, and it recovers after the switch-off (the daemon retries a busy PCM). Hypothesis (3) was **not confirmed** on c3-a (see results).

## Prerequisites and safe baseline

- SSH as root to the TV (address in the gitignored local config, never in this document). `luna-send` needs a pty on this firmware: run scripts with `ssh -tt`, copying the script to `/tmp` first (a script piped into `sh -s` over a pty never gets EOF).
- Raw output goes only to the gitignored `devices/private/c3-a/EXP-011/`.
- Never read ASoC `dapm_widget` sysfs nodes or `/sys/kernel/debug` (the c3-a oops hazard). Named `/proc/asound/card*/pcm*c/sub0/status` files and `/proc/<pid>/fd` links only. Never call `apiadapter/system/getSystemInfo`.
- No power, screen, input, app, volume, sound or reboot action. Do not hold "Hi LG" on for more than about 3 minutes.
- Originals recorded before any write (private file `A-read.txt` and this table): `lifeOnScreenMode = alwaysReady`; `getActivation`: top-level, `thinQ`, `amazon` and `dangbei` all `settingStatus false`, `activationState false`; `aod/getStatus = none`; power `Active`.
- HA verification: short-lived token from the login flow in a 0600 file, read-only REST `GET /api/states/...`, the token's own refresh token deleted at the end.

## Commands / steps (copyable and bounded)

`L` is `luna-send -n 1 -w 5000 -f` (run inside `ssh -tt`).

Part A (read-only):

```sh
$L luna://com.webos.settingsservice/getSystemSettings    '{"category":"general","keys":["lifeOnScreenMode"]}'
$L luna://com.webos.settingsservice/getSystemSettingDesc '{"category":"general","keys":["lifeOnScreenMode"]}'
$L luna://com.webos.service.alwaysready/aod/getStatus    '{}'
$L luna://com.webos.service.voiceinput/setting/getActivation '{}'
cat /usr/share/voiceinput/schemas/VoiceInputService.Setting.setActivation.schema   # parameter schemas, named file
cat /usr/share/voiceinput/schemas/VoiceInputService.Setting.getActivation.schema
```

Part B1 (Always Ready, c3-a only):

```sh
$L luna://com.webos.settingsservice/setSystemSettings '{"category":"general","settings":{"lifeOnScreenMode":"off"}}'
# read back: getSystemSettings (above), aod/getStatus, tvpower/power/getPowerState
sleep 10
$L luna://com.webos.settingsservice/setSystemSettings '{"category":"general","settings":{"lifeOnScreenMode":"alwaysReady"}}'
# read back as above
```

Part B2 (Hi LG, c3-a only):

```sh
$L luna://com.webos.service.voiceinput/setting/setActivation '{"settingStatus":true}'
# every few seconds: getActivation; /proc/asound/card*/pcm*c/sub0/status (non-closed ones);
#   which /proc/<pid>/fd links point to /dev/snd/pcmC*c (holder pid and comm)
# PC side, every 3 s: GET /api/states/binary_sensor.<entry>_listening and ..._microphone_muted
$L luna://com.webos.service.voiceinput/setting/setActivation '{"settingStatus":false}'
# read back getActivation; satellite: ctl.sh status; grep -i busy in the daemon log
```

## Expected evidence and failure interpretation

- `returnValue true` and a read-back equal to the written value: the setter works. `returnValue false`, an `errorText`, or a read-back that differs (the TV reverts it): negative.
- A terms or consent dialog, or a call that waits for a screen prompt: stop, restore, record as negative (nobody may accept terms for the owner).
- Hi LG on: `voiceinput` as a holder of a capture PCM, satellite `MIC BUSY`, HA Listening `off` and Microphone muted `on` would confirm hypothesis 3. No holder change and Listening staying `on` means LG's service did not take the microphone while the satellite held it.

## Observations and redacted evidence links

Labels: **verified-on-target** (c3-a, firmware `03.30.74`, 2026-11-05) unless marked.

### Part A

| Item | c3-a | c3-b |
| --- | --- | --- |
| `getSystemSettings` `general/lifeOnScreenMode` | `alwaysReady` (`returnValue true`) | **not read**: the TV was unreachable (`No route to host` on port 22; its HA entities were `unavailable`, so it is in suspend). Not woken, no WoL. Last records (2026-11-04): `off`, see [D0](../discovery/c3-b/D0-quick.md) and [D5](../discovery/c3-b/D5-results.md) |
| `getSystemSettingDesc` for that key | `vtype ArrayExt`, `dbtype G`, five values (below) | not read |
| `alwaysready/aod/getStatus` | `status none` | not read (last record `none`) |
| `voiceinput/setting/getActivation` | `settingStatus false`, `activationState false` at the top level and for `thinQ`, `amazon` and `dangbei` | not read (last records 2026-11-04: off in the P4 sound session, on again at the end of D6) |

Allowed values of `lifeOnScreenMode` (`getSystemSettingDesc`, `values.arrayExt`, verified-on-target, c3-a):

| Value | `visible` | `active` | Meaning (from the settings UI strings in [always-on](../design/always-on.md), not re-verified here) |
| --- | --- | --- | --- |
| `none` | false | false | hidden, disabled |
| `off` | true | true | Always Ready off |
| `lifeOnScreen` | true | true | a value this TV lists; not seen in earlier notes; meaning unknown (hypothesis: the older "Life on Screen" ambient mode); **not written** |
| `alwaysReady` | true | true | Always Ready, the dark variant used by c3-a |
| `allEnabled` | true | true | Always Ready with wallpaper |

`getActivation` reply shape (c3-a): top level `settingStatus`, `activationState`, `systemStatus`, `supportPlatform`, `supportLanguage`; per provider `thinQ`, `amazon`, `dangbei` the same booleans; `thinQ` and the top level also carry `currentTriggerWord` (`hiLG`) and `availableTriggerWord` (`["hiLG"]`) once it was switched. `getActivation` accepts only `{"subscribe": true}` as a parameter (schema file).

`setActivation` parameter schema (named file `/usr/share/voiceinput/schemas/VoiceInputService.Setting.setActivation.schema`): one property, `settingStatus` (boolean), **required**. **No provider parameter exists** in the schema. The reply names the provider that was changed: `{"serviceName":"thinQ","returnValue":true,"settingStatus":true}`. The `voiceinput` binary's internal log format also has `serviceName`, `setActivationCaller`, `triggerWord`, `locale`, but those are internal; I sent only `settingStatus`.

### Part B1, Always Ready on c3-a

| Time (UTC) | Call | Reply | Read-back |
| --- | --- | --- | --- |
| 06:07:26 | `setSystemSettings {"category":"general","settings":{"lifeOnScreenMode":"off"}}` | `returnValue true` | `lifeOnScreenMode off` immediately; `aod/getStatus none`; power `Active` |
| 06:07:36 (10 s later) | `setSystemSettings {"category":"general","settings":{"lifeOnScreenMode":"alwaysReady"}}` | `returnValue true` | `alwaysReady`; `aod/getStatus none`; power `Active` |

No dialog or error came back through luna. `aod/getStatus` does **not** reflect the setting (it stayed `none`; it is a display status, not the setting). I could not see the panel, so an on-screen prompt cannot be excluded from this record, but the TV was `Active` and the owner's session was not interrupted as far as the daemon and HA showed.

### Part B2, Hi LG on c3-a

| Time (UTC) | Event |
| --- | --- |
| 06:07:55 | Before: `hw:LGSIC,0` (`card1/pcm0c`) `RUNNING`, held only by `webos-satellite`; `getActivation` all false; HA Listening `on`, Microphone muted `off`; no `busy` line in the daemon log |
| 06:08:11 | `setActivation {"settingStatus":true}` -> `{"serviceName":"thinQ","returnValue":true,"settingStatus":true}`; no dialog |
| 06:08:11 to 06:11:24 | Read-back: top-level `settingStatus true` and `activationState true`; `thinQ` `settingStatus true`, `activationState true`; `amazon` and `dangbei` stayed false. Ten samples about 18 s apart (my sampler was slower than planned): `card1/pcm0c` `RUNNING` with the same owner pid, **one holder only (the satellite)**, no other capture PCM open, `voiceinput` held no capture PCM. HA (3 s poll, 06:07:55 to 06:11:29): Listening `on`, Microphone muted `off` in every sample |
| 06:11:24 | `setActivation {"settingStatus":false}` -> `{"serviceName":"thinQ","returnValue":true,"settingStatus":false}` |
| 06:11:30 | Read-back: all `settingStatus` and `activationState` false again; PCM still held by the satellite |

Results:

- **`setActivation {"settingStatus":true|false}` works as root over luna** (no provider parameter, no terms dialog came back, reply `returnValue true`), and the read-back follows immediately. It switches the `thinQ` provider (the reply says so); the other providers were not touched.
- **The satellite did not see `MIC BUSY`:** the daemon log has no `busy` line before, during or after; the PCM stayed `RUNNING` with the same owner; HA Listening stayed `on` and Microphone muted `off`. Hypothesis 3 is **rejected for this order** (the satellite already holds the PCM when "Hi LG" is switched on). `voiceinput` reported `activationState true` while it held no capture PCM (it may use another path or retry; unknown).
- **Recovery time: not applicable** (nothing was lost); the daemon was not restarted.
- The earlier c3-b observation (LG's service holds the PCM and the satellite reports `MIC BUSY` with hands-free on) is a different start order: there LG's service already held the PCM (`RUNNING`) before the satellite opened it ([P4 sound session](../discovery/c3-b/P4-sound-session.md), [D6](../discovery/c3-b/D6-poweroff-screenoff.md)). Not reproduced here.
- **I held "Hi LG" on for 193 s, about 13 s over the 3 minute cap**, because the sampler was slow and the safety trap on the TV did not fire when the PC-side `ssh` timeout killed the client; I switched it off by hand when I saw the overrun. Lesson: run the watch loop on the PC side with short remote calls, not a long script with a client timeout.

### Final state (c3-a), read back at 06:12 UTC

| Item | Original | Final |
| --- | --- | --- |
| `general/lifeOnScreenMode` | `alwaysReady` | `alwaysReady` |
| `alwaysready/aod/getStatus` | `none` | `none` |
| `getActivation` (top level, `thinQ`, `amazon`, `dangbei`) | all `false` / `false` | all `false` / `false` |
| Satellite | running pid 7161, `hw:LGSIC,0` `RUNNING` | same pid, same PCM owner, `ctl.sh status` running, HA Listening `on`, Microphone muted `off`, capture problem `off`, power state `Active` |
| Our files on the TV | none | removed (`/tmp` clean of `e11*` scripts) |
| HA | | the refresh token this run created was deleted by its own id (a refresh answers `invalid_grant`, the access token answers 401); no other token touched, no HA configuration changed |

## Conclusion and confidence

- `getSystemSettings`/`setSystemSettings` on `general/lifeOnScreenMode`: **works for read and write on c3-a** (verified-on-target, `off` and `alwaysReady` written and read back). Writable values per the TV's own description: `off`, `lifeOnScreen`, `alwaysReady`, `allEnabled` (`none` is hidden and inactive). `lifeOnScreen` and `allEnabled` were **not written** here (`allEnabled` is planned in D5 on c3-b only).
- `setActivation {"settingStatus":bool}` and `getActivation {}`: **work for read and write on c3-a**; one parameter, no provider selector, it addresses `thinQ`; Amazon and the third provider are not switchable by this call (not tested whether they have their own switch).
- With the satellite already holding the microphone, switching "Hi LG" on for 193 s caused no capture loss and no `MIC BUSY`. A TV-app toggle can therefore be shown and used safely in this order; whether the satellite loses the microphone when "Hi LG" is on at start (c3-b) or after a restart, is the earlier finding, and the app should warn about it.
- Confidence: high for the call syntax and read-backs (single TV, firmware `03.30.74`, one run each); low for the effects of "Hi LG" (one 193 s observation).

## Rollback and cleanup

Both settings restored and read back (table above). To restore by hand: the two `setSystemSettings` and `setActivation` calls above with the original values (`alwaysReady`; `settingStatus false`), or LG's own menus (Settings, General, Always Ready; Settings, General, AI Service, Voice recognition settings, "Hi LG"). If the satellite ever loses the PCM: `ctl.sh stop`, `ctl.sh start` (not needed here).

## Follow-ups / unresolved questions

- c3-b read-only part (`getSystemSettings`, `getSystemSettingDesc`, `getActivation` shape with its own Always Ready `off`): **not run**, TV suspended; repeat when it is on, still read-only.
- Is a change made in LG's own menu reflected by the getters, and the other way round (does the TV's menu show the value we set)? **Not tested** (needs a person at the TV).
- Is any on-screen prompt shown by the TV after `setActivation true` (for example a voice-recognition consent screen)? Luna returned success, but the panel was not observed. **Not tested** (needs a person).
- Does "Hi LG" really listen while the satellite holds the PCM (does saying "Hi LG" start LG's assistant, and does it cause `MIC BUSY` on restart)? **Not tested** (it needs speech and consent).
- Effect of `lifeOnScreen` and `allEnabled` (not written), and of writing `off`/`alwaysReady` while the TV is in `Screen Saver` or after the remote's power key.
- Whether Amazon or the third provider can be switched on (no parameter in the schema; no documented method).
