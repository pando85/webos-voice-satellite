# Voice and power screen in the TV app

- **Status:** design, accepted by the owner on 2026-11-05 ([ADR 0008](../decisions/0008-user-initiated-lg-settings.md)); **nothing here is implemented**. The luna write details are **pending EXP-011** (a read-first experiment on c3-b that lists the settings' values and the setters' behaviour; its record is added separately and is not linked from here yet).
- **Date:** 2026-11-05. TV facts are `verified-on-target` for **c3-b** (LG C3 55", webOS TV 8.3.0, firmware `03.30.73`) unless a label says otherwise; c3-a (`03.30.74`) is a different unit and mostly untested for this ([c3-b record](../compatibility/c3-b.md), [c3-a record](../compatibility/c3-a.md)).
- **Related:** [always-on design](always-on.md) (sections 1, 11 to 13), [TV control](tv-control.md), [install and configuration UX](install-and-config-ux.md), [packaging](packaging.md), [ADR 0005](../decisions/0005-tv-control-entities.md), [ADR 0008](../decisions/0008-user-initiated-lg-settings.md), backlog [#14](../project/backlog.md#14-voice-and-power-screen-in-the-tv-app).

## 1. Scope

A "Voice and power" screen in the existing status app (`packaging/webos/app/`) with three toggles and one live sentence that says what the remote's power button will do and whether the satellite can hear you:

| # | Toggle | Kind | Stored in | Written by |
| --- | --- | --- | --- | --- |
| 1 | **Always Ready** | LG setting `general/lifeOnScreenMode`, values `off` and `alwaysReady`; more values exist (`allEnabled` is seen in the TV's UI code; EXP-011 lists them) | the TV (settingsservice) | the app, on a press, through `ctl.sh set always-ready` |
| 2 | **LG hands-free "Hi LG"** | LG setting through `voiceinput/setting/getActivation` and `setActivation {"settingStatus":...}` | the TV (voiceinput) | the app, on a press, through `ctl.sh set hi-lg` |
| 3 | **Keep listening after the power button** | the satellite's own HA select "When the TV is turned off with the remote" (`remote_power_off`): "Turn off normally" or "Keep listening (screen off)" | `etc/remote_off` (word `normal` or `keep_listening`) | the app through `ctl.sh set remote-off`, or HA's select |

Rules (ADR 0008): the two LG settings change **only on an explicit press in the app** with a confirm step; every write is read back; the daemon never changes them; the old value is logged; Home Assistant gets two **read-only** diagnostic sensors. The app computes the sentence; `ctl.sh` returns facts and never sentences, so there is one implementation of the matrix (section 3), tested on the host.

Non-goals: any other LG setting; HA switches for the LG settings; the daemon changing an LG setting for any reason; an "OLED care window" and power measurements (still open in [always-on 12.4](always-on.md#124-open)).

## 2. Evidence and labels

Labels as in [AGENTS.md](../../AGENTS.md). Everything below the table is design.

| Claim | Label | Source |
| --- | --- | --- |
| Always Ready off, power key, "Turn off normally": `Active Standby` at +3 to +4 s, `Request Suspend` +7 to +15 s, network down 10 to 20 s after the key, process frozen (same pid after wake), no listening | verified-on-target (c3-b `03.30.73`, API power key and two real IR key presses) | [always-on 11](always-on.md#11-result-on-c3-b-2026-11-03), [D5](../discovery/c3-b/D5-results.md) |
| Always Ready on (`alwaysReady` and `allEnabled` alike): stays in `Active Standby` (59 min 18 s), process, mic and HA link alive, six wake-word runs, **replies not audible** (5 checks; LG cuts the DSP input gain at the power-off) | verified-on-target (c3-b) | always-on 11 and 12, [D6](../discovery/c3-b/D6-poweroff-screenoff.md) |
| TV-control entities missing while the TV is in `Active Standby` | by design (availability rules, [TV control 3.2 and 3.4](tv-control.md#32-availability-rules)); seen in the transient state, **settled `Active Standby` not tested** (B11) | [EXP-010 Part B](../experiments/EXP-010-tv-control-setters.md) |
| "Keep listening" (wake and darken), Always Ready off and on: `Screen Off` 5.2 to 5.9 s after the key, listening, replies audible 12 of 12; 20 min dwell, 28 of 28 beeps | verified-on-target (c3-b, daemon 0.1.0: 4 runs; D6 watcher: 7 of 7) | always-on 12 and 13 |
| Second press from a daemon-made `Screen Off`: TV lit (`Active` 4.8 s after the key) | verified-on-target (c3-b, 3 runs) | always-on 13 |
| Keep listening on **c3-a** | **not tested** | |
| Wake and darken with the **real IR remote** | **not tested** (the IR key gave the same state sequences in D5 without the daemon acting) | always-on 11 |
| Panel flash 0.28 to 0.67 s by the panel controller's screen-mute getter | **not observed visually** | always-on 12.1 |
| Power draw of `Screen Off` and of Always Ready | **not measured** | always-on 12.2 |
| LG's OLED care at power-off may not complete when the TV is woken within seconds | **hypothesis**, risk text only | always-on 12.2 |
| Hi LG on: LG's voice service holds `hw:LGSIC,0`, the satellite reports `MIC BUSY`, `listening` off, in the states tried | verified-on-target (c3-b, `Screen Saver`, one boot) | [P4 permanent install](../discovery/c3-b/P4-permanent-install.md), [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md) |
| `general/lifeOnScreenMode` and `setActivation {"settingStatus":bool}` were written and read back by agents | verified-on-target (c3-b, D5, D6, P4 session 2) | their records |
| The exact setter payloads to ship, the values list, per-provider behaviour of Hi LG, whether LG reverts a value, whether `subscribe` works for these | **pending EXP-011** | |

## 3. Outcome matrix (the single source for the app sentence)

### 3.1 Inputs

- **AR**: Always Ready, from `lifeOnScreenMode`: `off` (raw `off` or `none`), `on` (raw `alwaysReady` or `allEnabled`; behaviour was the same on c3-b), `unknown` (any other raw word), `error` (not readable).
- **HL**: Hi LG, from `getActivation`: `off`, `on`, `unknown`, `error`. How the per-provider values collapse to one is **pending EXP-011** (working rule: `on` when any provider reports `settingStatus true`).
- **RO**: `etc/remote_off`: `normal` or `keep_listening`.
- **MIC**: from the daemon's status file `run/voice.json` and the kill-switch file: `listening`, `busy`, `muted` (HA's mute switch or a gate), `kill` (kill switch file present), `closed` (other), `unknown`; plus `running` (daemon alive).

### 3.2 Power cell: what the remote's power button does

The kill switch makes the daemon do nothing on a power-off (it also blocks the HA power-off), so with the kill switch **on**, RO is treated as `normal`. A muted microphone does not stop wake and darken.

| Cell | AR | RO | After the power button | Listens | Replies audible | TV controls in HA | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **P1** | off | normal | `Active Standby`, then Suspend 7 to 15 s after the key; satellite frozen | no | no | unavailable | c3-b verified |
| **P2** | on | normal | stays in `Active Standby` | yes (mic permitting) | **no** (LG cuts the DSP gain) | unavailable (by design; settled state B11 not tested) | c3-b verified (audio), design (entities) |
| **P3** | off | keep | the daemon wakes the TV and darkens it: `Screen Off` about 5 to 6 s after the key | yes | yes | available | c3-b verified; c3-a not tested; IR not tested |
| **P4** | on | keep | as P3 | yes | yes | available | c3-b verified; c3-a not tested; IR not tested |
| **PX** | unknown or error | any | cannot say | | | | |

A second press from a `Screen Off` that the daemon made lights the TV (about 5 s). Timer, CEC and HomeKit power-offs are left alone by the daemon and follow LG's own behaviour (P1 or P2 by AR).

### 3.3 Listening line (what the microphone is doing now)

Evaluated top to bottom; the first match wins.

| Order | Condition | Cell |
| --- | --- | --- |
| 1 | daemon not running or not installed | **L-stopped** |
| 2 | MIC = `kill` | **L-kill** |
| 3 | MIC = `muted` | **L-muted** |
| 4 | MIC = `busy`, or (HL = `on` and MIC is not `listening`) | **L-hilg** when HL = `on`, else **L-busy** |
| 5 | MIC = `listening` | **L-ok** |
| 6 | anything else | **L-unknown** |

The observed MIC state wins over the setting: if HL is `on` but MIC is `listening`, the screen shows L-ok and adds the line **N-hilg-on** (a reminder, because the household found it switching back on).

### 3.4 Sentence

`sentence = headline(P) + " " + detail(P)`, then a new line with the listening line. The headline is the large text, the detail the normal text. A cell with PX shows only X-unknown and no claim about the power button.

## 4. User-facing strings (English, exact)

Short, plain, readable from the sofa at 22 px or larger. Strings are the contract: the app's matrix test compares them.

### 4.1 Power button

| Cell | Headline | Detail |
| --- | --- | --- |
| P1 | `Power button: the TV goes to standby.` | `About 10 seconds later the TV suspends and the satellite stops. It cannot hear you until the TV is turned on.` |
| P2 | `Power button: the TV rests in standby.` | `The satellite keeps listening, but its spoken replies are silent in standby. Its TV controls in Home Assistant are unavailable until the TV is turned on.` |
| P3, P4 | `Power button: the screen goes dark and the TV keeps listening.` | `The TV stays on with the screen dark, hears you and answers out loud. Press the power button again to light the screen.` |
| PX | `Power button: not known.` | `The TV's settings could not be read. Press Refresh.` |

With the kill switch on, the power line is the P1 or P2 text for the current AR, followed by `Keep listening does nothing while the microphone is off with the kill switch.` when RO = `keep_listening`.

### 4.2 Listening line

| Cell | Text |
| --- | --- |
| L-ok | `Listening now: yes.` |
| L-hilg | `Listening now: no. LG's assistant (Hi LG) has the microphone. Turn Hi LG off to use the satellite.` |
| L-busy | `Listening now: no. Another program has the microphone.` |
| L-muted | `Listening now: no. The microphone is muted in Home Assistant.` |
| L-kill | `Listening now: no. The microphone is off (kill switch).` |
| L-stopped | `Listening now: no. The satellite is not running.` |
| L-unknown | `Listening now: not known.` |
| N-hilg-on (extra line) | `Hi LG is on. If the satellite cannot hear you, turn it off.` |
| X-unknown (extra) | `Always Ready or Hi LG could not be read, so the sentence above may be wrong.` |

### 4.3 Toggle rows

Row label and the state word shown at the right:

| Row | Label | State words |
| --- | --- | --- |
| 1 | `Always Ready (LG setting)` | `On`, `Off`, `On, with wallpaper` (raw `allEnabled`), `Not known (<raw>)`, `Cannot read` |
| 2 | `Hi LG hands-free (LG setting)` | `On`, `Off`, `Not known`, `Cannot read` |
| 3 | `Keep listening after the power button` | `On`, `Off` |

Row sub-line (always visible, one line): row 1 `Keeps the TV in standby for LG's own features. Uses more standby power.`; row 2 `LG's own voice assistant. When it is on, the satellite cannot use the microphone.`; row 3 `A satellite setting. The TV stays on with the screen dark after the power button.`

### 4.4 Warnings and confirm texts

Shown in the warning area while a row is focused, and as the confirm text after the first OK.

| Change | Text (the confirm prompt ends with `Press OK again to confirm.`) |
| --- | --- |
| Always Ready off to on | `Turn on Always Ready? The TV then uses more power in standby (not measured). With "Keep listening" off, the satellite hears you after the power button but its replies are silent.` |
| Always Ready on to off | `Turn off Always Ready? Without "Keep listening", the TV suspends after the power button and the satellite stops.` |
| Hi LG off to on | `Turn on Hi LG? LG's assistant takes the microphone, so the satellite cannot hear you.` |
| Hi LG on to off | `Turn off Hi LG? The TV stops answering "Hi LG". The satellite can use the microphone.` |
| Keep listening off to on | `Turn on Keep listening? After the power button the TV stays on with the screen dark. It uses more power than standby (not measured), LG's OLED care at power-off may not run, and the screen may flash briefly (not observed).` |
| Keep listening on to off | no confirm (one press): `Done. The power button turns the TV off the normal way.` |

Result lines (one line under the rows, the app's existing `message` area):

| Result | Text |
| --- | --- |
| applied | `Always Ready is now On. Changed from Off at <time>.` (likewise for Hi LG, Keep listening) |
| already | `Already On. Nothing changed.` |
| failed | `Could not change Always Ready: <reason>. It is still <value>. You can also change it on the TV: Settings, General, ...` |
| mismatch | `The TV reports <x>, not <y>. The change did not stick, or LG changed it back. It is <x> now.` |
| cannot read | `Could not read Always Ready: <reason>.` |

`<reason>` is the luna `errorText` (a TV message such as `not exist`) or a fixed word from `ctl.sh` (`timeout`, `no luna-send`, `not root`, `unknown value`). Menu paths for the failed and unknown cases: Always Ready: `Settings, General, Always Ready` (exact path **pending EXP-011**, to be read from the TV's own settings page); Hi LG: `Settings, General, AI Service, Voice recognition settings, Hi LG` (the household's way, [P4](../discovery/c3-b/P4-sound-session.md#state-found-and-the-hi-lg-change)).

## 5. UI layout in the existing app

Existing app (`packaging/webos/app/`, ES5, one `main`, a hidden `report-view` second screen, whitelisted `ctl.sh` argument strings, arrows and OK, Back on 27, 8 and 461, 10 s status refresh paused while the report is open): the new screen is a third `<main id="voice-view" hidden>` opened by a new Actions button **Voice and power** (`id="b-voice"`, shown when installed; handled like `report --json` by a special case in `onButton`). The "Not available yet" panel text loses "power options (screen off, Always Ready)".

```
Voice and power                                   (h1)
+--------------------------------------------------------------+
| Power button: the screen goes dark and the TV keeps listening.|  headline, aria-live
| The TV stays on with the screen dark, hears you and answers   |  detail
| out loud. Press the power button again to light the screen.   |
| Listening now: yes.                                           |
+--------------------------------------------------------------+
| [ Always Ready (LG setting)                  Off ]            |  row 1  (focus)
|   Keeps the TV in standby for LG's own features. ...          |
| [ Hi LG hands-free (LG setting)              Off ]            |  row 2
| [ Keep listening after the power button       On ]            |  row 3
+--------------------------------------------------------------+
| warning area: text for the focused row, or the confirm prompt |
| message line: result of the last change                       |
| [ Refresh ]  [ Back ]                                         |
```

**Focus order and keys.** Rows are `<button role="switch" aria-checked>`; the order is row 1, row 2, row 3, Refresh, Back, in that visual order. Up and Left move to the previous, Down and Right to the next (wrapping), as the main screen does; OK presses; Back (27, 8, 461) leaves the screen. First focus on open: row 1. Focus never moves by itself after a refresh (the focused element keeps its place, unlike `focusFirst`).

**Press behaviour.** OK on a row whose change needs a confirm (everything except Keep listening to off) shows the confirm text in the warning area and arms the row for 5 seconds (state word `Press OK again`, class `armed`, the existing pattern); a second OK within 5 s sends the command; any other key, focus move or the timeout disarms. While a command runs all rows are disabled and the state word is `Changing...`.

**Row states** (state word and class):

| State | Word | Class | Press does |
| --- | --- | --- | --- |
| reading (first load) | `Checking...` | muted | nothing |
| off / on | `Off` / `On` | none, `warn` for Hi LG on and Keep on | arm, then change |
| armed | `Press OK again` | `armed` | send the command |
| pending | `Changing...` | muted | nothing |
| failed (command error, state unchanged) | the old value plus the result line | `bad` on the message line | arm again |
| mismatch (read-back differs from the request) | the value the TV reports, result line `mismatch` | `warn` | arm again; the screen keeps showing what the TV reports |
| unknown (raw word the app does not change) | `Not known (<raw>)` | `warn` | nothing; the warning area shows the menu path (decision 8 of ADR 0008) |
| unavailable (cannot read: no luna-send, timeout, not root) | `Cannot read` | `bad` | nothing |

**Refresh.** On open: one `tv-settings --json` (rows show `Checking...`). While the screen is open: every 10 seconds when nothing is pending or armed (the main screen's refresh is paused, as for the report). After a successful `set`: the command's own read-back is shown at once, plus one more `tv-settings --json` 3 seconds later to catch an LG-side revert (a revert shows as a mismatch line: `LG changed Always Ready back to Off.`). The Refresh button forces a read. Leaving the screen resumes the main refresh. A row whose read returns `error` keeps showing its last good value with `Cannot read` until a read succeeds.

**Sentence refresh.** The sentence is recomputed on every read (matrix in section 3, one function `outcome(state)` in `app.js`, exported to the host test the way `app_smoke.js` already loads the app).

**Main screen.** The "Remote power key" fact row stays; it links in its text to the new screen: `Keep listening: on. Open Voice and power to change it.`

## 6. `ctl.sh` interface

New commands, fixed arguments only, whitelisted in the app's `ALLOWED` list (six `set` strings and one read string; no typed text reaches a command):

```
tv-settings [--json]          read Always Ready, Hi LG, remote_off and the mic state
set always-ready on|off       change the LG setting (read back, logged)
set hi-lg on|off              change the LG setting (read back, logged)
set remote-off normal|keep    change the satellite's own setting
```

`ALLOWED` additions: `tv-settings --json`, `set always-ready on`, `set always-ready off`, `set hi-lg on`, `set hi-lg off`, `set remote-off normal`, `set remote-off keep`.

All need root (as the other commands; `WEBOS_SAT_ALLOW_NONROOT=1` for tests) and the install lock for writes. A new environment variable `WEBOS_SAT_LUNA_SEND` (default `luna-send`) lets the host tests substitute a fake; luna calls run on a pseudo-terminal as the daemon's calls do (how `ctl.sh` provides a pseudo-terminal on the TV, for example with `script`, is **to confirm**: the audio getters printed nothing over a plain pipe, [TV control 5](tv-control.md#5-interface-per-control); whether settingsservice and voiceinput need it is **pending EXP-011**), `-n 1 -w 3000` (one reply, 3 s).

### 6.1 `tv-settings --json` schema (version 1)

```json
{
  "schema": 1,
  "luna": true,
  "daemon_running": true,
  "always_ready": {"state": "off", "raw": "off", "variant": null, "error": ""},
  "hi_lg": {"state": "off", "raw": "", "error": ""},
  "remote_off": {"state": "keep_listening", "active": true},
  "mic": {"state": "listening", "kill_switch": false},
  "power_state": "Active",
  "last_change": {"setting": "always-ready", "from": "off", "to": "on", "at": "2026-11-05T20:10:03Z"}
}
```

- `luna`: `luna-send` exists and the caller is root; false makes both LG states `error` with `error` `no luna-send` or `not root`.
- `always_ready.state`: `off` | `on` | `unknown` | `error`; `raw` is the TV's word (`off`, `none`, `alwaysReady`, `allEnabled`, anything else); `variant`: `"dark"` for `alwaysReady`, `"wallpaper"` for `allEnabled`, else null. The getter is `settingsservice/getSystemSettings` for `general/lifeOnScreenMode` (read-only; call shape as used in D0, exact parameters **pending EXP-011** to write down).
- `hi_lg.state`: `off` | `on` | `unknown` | `error`; `raw` is a short list of the providers' `settingStatus` values (no trigger words, no names); the collapse rule is **pending EXP-011**.
- `remote_off.state`: the stored word; `active` is the daemon's report (false when the power watch is off, so keep listening cannot act), null when the daemon is not running.
- `mic.state`: `listening` | `busy` | `muted` | `closed` | `unknown`, from the daemon's `run/voice.json` (section 6.4); `kill_switch` from the kill-switch file (`/tmp/webos-satellite.disable`, lost at reboot).
- `power_state`: the daemon's last power state from `run/voice.json`, or null (the app shows it only in the log line; the matrix does not use it).
- `last_change`: the last line of the history file (section 6.3), or null.
- A field that cannot be read is `error` with a one-line `error`, never a guess. The reply holds no serial number, address, name, key or setting other than these (the same allow-list idea as the compatibility report).
- Exit 0 whenever the JSON was printed, even with an `error` field; 1 only when ctl.sh itself cannot run (not installed, bad lock); 2 for usage.

### 6.2 `set ... ` result

`set` prints one JSON line (and a short human line on stderr for SSH use):

```json
{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"on","result":"applied","error":""}
```

`result`: `applied` (read back equals the request), `already` (before equals the request, **no write is made**), `mismatch` (written, the read-back differs), `failed` (the setter returned `returnValue:false` or timed out; `error` holds the luna `errorCode` and `errorText`, no reply body), `refused` (see errors). Exit 0 for `applied` and `already`, 1 otherwise. Steps for an LG setting: take the lock; read (`before`, a failed read is `refused`); append a `request` line to the history; write; wait 0.5 s; read back; append a `result` line; print. **One write, one read-back, no retry, no second method or parameter set** (as [TV control 6](tv-control.md#6-capability-detection-at-startup)). The payload is **pending EXP-011** (Always Ready: `settingsservice/setSystemSettings` with category `general` and the key `lifeOnScreenMode`; Hi LG: `voiceinput/setting/setActivation {"settingStatus":bool}`; the provider handling is open). The values written: Always Ready `on` writes `alwaysReady` (the dark variant used by c3-a and the D5 and D6 runs, never `allEnabled`), `off` writes `off`. Turning Always Ready off from `allEnabled` is allowed and logged as such.

`set remote-off` (no luna): validates, then follows section 6.5, prints `result` `applied` when the file holds the word and, with the daemon running, the daemon consumed the trigger within 3 s (`"daemon":"applied"`), `"daemon":"pending"` when the trigger was not consumed in 3 s, `"daemon":"not running"` when stopped (the next start reads the file).

### 6.3 Old-value log

`$P/etc/tv-settings.log` (`/var/lib/webos-satellite/etc/tv-settings.log`): append-only text, one line per event, UTC time, no identifiers:

```
2026-11-05T20:10:03Z always-ready request from=off to=on source=ctl
2026-11-05T20:10:04Z always-ready result from=off to=on read=on outcome=applied
```

It lives in `etc/` (kept by `uninstall`, removed by `purge`, not rotated with the daemon log), is capped by ctl.sh itself at 200 lines (oldest dropped when appending), mode 0600. `source` is always `ctl` (the app and an SSH session run the same script). `last_change` and the sub-line "Before: Off" of a row read it. A restore is `ctl.sh set <setting> <from>` of the last `applied` line; the app does not need a separate restore button in the first version (the row's confirm text names the old value).

### 6.4 Status file from the daemon

The daemon writes `run/voice.json` (atomically, only on change, at most once a second, mode 0644) with `{"mic":"listening|busy|muted|closed","remote_off":"normal|keep_listening","remote_off_active":true,"power_state":"Active"}`, from state it already has (the capture-live and gate state, the darkener's mode, the power watch). It carries no identifiers. `ctl.sh` reads it; a file older than 60 s or missing gives `unknown`/null. This is the only new daemon-to-script channel; it follows `run/compat.json`.

### 6.5 How the running daemon picks up a `remote_off` change without a restart

**How it works today** (`crates/satellite/src/daemon.rs`, `darken.rs`, `config.rs`): the HA select arrives as `on_select`, which calls `darkener.set_mode(mode)` (leaving keep listening stops a wake or darken in progress) and writes the word with `volume::write_atomic` to `power.remote_off_file` (default `etc/remote_off`). The file is read **only at startup** (`darken::load_mode`); `ctl.sh status` already reads the same file. So a change made by `ctl.sh` would be ignored until a restart.

**Proposed mechanism: a reload trigger file plus a shared apply function.**

1. `ctl.sh set remote-off normal|keep` writes the word (`normal` or `keep_listening`) to the configured `remote_off_file` with a temporary file and `mv` (the same atomic write as the daemon; the path comes from `cfg_value remote_off_file` resolved against `etc/`, default `etc/remote_off`), then creates the trigger `run/remote-off-reload` (empty file), the same pattern as `selfcheck-run` and `firmware-ack`.
2. The daemon's existing once-a-second trigger poll (`poll_compat_triggers`, directory `report.dir`) also consumes `remote-off-reload` with the existing `consume_trigger`, then calls `darken::load_mode(&remote_off_file)`. `Ok(Some(mode))` different from the current mode goes through a new `apply_remote_off(mode, source)` that holds the code now inside `on_select` (set the mode, the `warn` or `info` log line with `source = "app"`, **no file write** because the file already holds the value) and sends `AppCommand::SetSelect` with the new option so HA's select follows. `Ok(None)` or `Err` logs one `warn` and keeps the current mode. HA's own select still goes through `on_select`, which now calls `apply_remote_off(mode, "Home Assistant")` and then writes the file.
3. With the daemon stopped, nothing to do: the file is read at the next start. `ctl.sh` reports `daemon: not running`.
4. Two writers at once (HA select and the app): the file holds the last writer's word; the daemon's reload reads the file, so after at most one second the daemon, the file, the HA select and the app agree.
5. `power.watch = false` (no power watch): the daemon ignores keep listening at start today; the reload applies the same rule and `remote_off.active` in `run/voice.json` is false, so the app says so (`Keep listening needs the power watch, which is off in the config.`).

Why not the alternatives: polling the file's modification time works but adds a stat per second for no benefit over the existing trigger pattern; a signal (SIGHUP) would reload the whole config, which the daemon does not support; a new socket or an HA round trip is heavier and breaks when HA is down.

### 6.6 Error cases

| Case | Behaviour |
| --- | --- |
| Not root | `refused`, `error` `not root` (app: "The Homebrew Channel is not running as root", already shown on the main screen) |
| Not installed | exit 1 `not installed` (the screen is hidden when not installed) |
| `luna-send` missing | LG fields `error` `no luna-send`; `set` for an LG setting `refused`; `remote-off` still works |
| Luna call timeout (3 s) | the field is `error` `timeout`; a timed-out write gets one read to say what the TV holds, `result` `failed` or `mismatch` |
| `returnValue:false` | `failed` with `errorCode` and `errorText` only |
| Read-back differs | `mismatch`; the log has `read=<value>`; the app shows what the TV reports |
| Current value unknown or unreadable | `refused` (`unknown value`): no write from a state the app cannot name |
| Requested value equals the current one | `already`, no write |
| Lock held by an install or another `set` | waits up to 60 s as the other commands do, then exit 1 |
| TV in a transient power state | the write is attempted; a failure is shown as `failed`. EXP-011 says whether a state matters (**pending**) |
| History file not writable | the write is **not made** (the log line is written first; an unlogged change is refused) |

## 7. Home Assistant: two read-only diagnostic sensors

| Entity | Type | Object id | Category | Values | Listed when |
| --- | --- | --- | --- | --- | --- |
| Always Ready | `text_sensor` | `lg_always_ready` | diagnostic | `Off`, `On`, `On with wallpaper`, `Other` (raw word in the log, not in HA); missing state when the read failed | `[tv_control] lg_settings` (default **true**, read-only) and the startup getter answered |
| LG hands-free | `text_sensor` | `lg_hands_free` | diagnostic | `Off`, `On`, `Other`; missing state when the read failed | as above |

Text sensors rather than binary sensors because Always Ready has more than two values and an `Other` must not read as "off". No `switch`, `select` or command for either: writes stay in the app (ADR 0008 decision 4). Names are fixed by the owner ("Always Ready", "LG hands-free"); the ESPHome object ids are stable keys.

**Cadence (cheap).** Reuse the TV-control machinery (`crates/tv-control` controller and `crates/satellite/src/tv_control.rs`, calls as `luna-send -n 1 -w 3000` on a pseudo-terminal): the controller gets two new read-only `Purpose`s with a parser per reply and two sensor `EntityDef`s, no setter and nothing in the setter limits. Reads:

- once at startup in the capability check (a getter that fails leaves the sensor out of the list, like the other controls) and again after a resume;
- every **60 s** while the power state is `Active`, `Screen Saver` or `Screen Off`; none in `Active Standby` or a transition (the other TV getters are not asked there either);
- immediately when `ctl.sh set` finishes (it creates `run/lg-settings-refresh`, consumed by the same one-second trigger poll as in 6.5), so HA shows the new value within a second;
- a subscription (`subscribe: true`) only if EXP-011 shows it works for both settings; otherwise polling stays.

Cost: two short `luna-send` calls a minute, far below the 5 to 10 s polls the controller already runs. A change seen by the poll that the app did not make is logged once at `info` (`lg setting: always-ready off -> on (not changed by the satellite)`), which would have explained the unexplained "Hi LG" switch-off of 2026-11-04. A value that cannot be read is logged once at `warn`. The sensors report the value; they never write, correct or "fix" it. Not gated by the firmware guard (TV control rule); no identifiers in any state or log line.

## 8. Test plan and implementation steps

### 8.1 Tests

1. **Host, shell (packaging/webos/tests, `run.sh`):** a fake `luna-send` (shell script, `WEBOS_SAT_LUNA_SEND`) with a state directory: `getSystemSettings` and `getActivation` answer from files, the setters change them, switches make it time out, return `returnValue:false`, or ignore the write (mismatch); synthetic JSON written by hand, no raw TV output committed. Checks: `tv-settings --json` schema and every error case of 6.6; `set` applied, already (no write), failed, mismatch, refused (unknown raw, unlogged); the history file lines, cap at 200 and mode 0600; no write when the log cannot be written; `set remote-off` writes the file atomically and creates the trigger; `uninstall` keeps and `purge` removes the log; the argument whitelist.
2. **Host, app logic (`app_smoke.js` and a new `app_matrix.js`):** the `outcome(state)` function against every cell of the 3.2 and 3.3 tables, all combinations of AR, HL, RO and MIC (the table is generated from the doc's strings: the test fails when a string differs), the kill-switch override, the observed-MIC-wins rule, `unknown`/`error` inputs; focus order, arm and confirm timing (second OK sends, other key disarms), disabled rows while pending, mismatch and failed rows, the 3 s delayed re-read, and that only whitelisted strings reach `exec`.
3. **Host, Rust:** `apply_remote_off` and the trigger reload in the satellite crate (a file written, the trigger consumed, the mode changed, HA select state sent, an invalid file ignored); `run/voice.json` written on change and only on change; the two sensors and their parsers in `webos-tv-control` with synthetic replies (`off`, `alwaysReady`, `allEnabled`, an unknown word, `returnValue:false`, malformed); no value other than the listed ones in HA; the fake-HA scenario lists both entities and shows no command path for them.
4. **On target, read-only first (any step):** `tv-settings --json` and the HA sensors against the real getters, compared with `luna-send` by hand (the same discipline as the report check).
5. **On target, writes:** **c3-a: the owner authorized the writes (2026-11-05); each run still records the originals first and restores them.** **c3-b: read-only unless the owner says otherwise.** Procedure for c3-a: record the three current values; for each LG setting one `set` to the other value, read back, then `set` back to the recorded value, confirm in HA and in the TV's own menu; the keep-listening reload from the app with the daemon running and HA open (select follows within 2 s); the whole matrix by hand for the cells reachable without a power-off, then, with a person present, the power-off cells P1 to P4 (a power key press changes power state; needs a quiet window and the usual way-back check). No sound is played without authorization (deploy and verify policy). Record results in a new experiment file and compatibility rows scoped to model, firmware and power state, as separate evidence.

### 8.2 Implementation steps (each sized for one agent, in order unless noted)

| Step | Work | Needs |
| --- | --- | --- |
| **V1** | Wait for the EXP-011 record; write the exact luna call shapes, values list, Hi LG provider handling and menu paths into section 6 (replace every "pending EXP-011"), amend ADR 0008 and the compatibility note. Docs only | EXP-011 |
| **V2** | `ctl.sh`: `tv-settings`, `set always-ready`, `set hi-lg`, `set remote-off`, the history log, `WEBOS_SAT_LUNA_SEND`, usage text; shell host tests with the fake `luna-send` (test plan 1). Shell only | V1 for the payloads (the remote-off part can go first) |
| **V3** | Daemon: `apply_remote_off` factored out of `on_select`, the `remote-off-reload` trigger, `run/voice.json` writer; Rust host tests (test plan 3, first part). No LG write code | none (parallel with V2) |
| **V4** | App: the `voice-view` screen, `outcome(state)` matrix and strings, focus, arm and confirm, refresh, whitelist additions, main-screen text; `app_smoke.js` and `app_matrix.js` (test plan 2) | V2 interface (schema) and the section 4 strings; can start from the schema |
| **V5** | `webos-tv-control` and `tv_control.rs`: the two read-only sensors, the 60 s poll, the refresh trigger, the log lines; Rust and fake-HA tests (test plan 3, second part); update the entity table in [TV control 3.1](tv-control.md#31-entity-table) | V1 for the getter shape |
| **V6** | Package: build the ipk, run the packaging tests (`packaging/webos/tests/run.sh`, `ipk.sh`), docs: [development](../development.md), [packaging](packaging.md), [install UX](install-and-config-ux.md) screens section, README status; version bump | V2 to V5 |
| **V7** | Deploy to c3-a (silent), read-only checks, then the authorized writes and matrix cells (test plan 5); record the experiment and compatibility rows | V6; a person present for the power-off cells |
| **V8** (optional, later) | A "restore the value before the first change" row action, an OLED-care counter display ([always-on 12.2](always-on.md#122-the-setting-to-expose-app-and-ha-ui)), the TV speaker run | owner, evidence |

## 9. Open questions

1. EXP-011: the full values list of `lifeOnScreenMode`, the setter payloads, whether `allEnabled` should ever be written, how Hi LG's per-provider state collapses and which `setActivation` parameters pick the provider, whether LG reverts values by itself, whether the getters work in `Active Standby` and whether `subscribe` works. (Section 6 and 7 hold the working shapes marked pending.)
2. Whether luna calls for these services need a pseudo-terminal from `ctl.sh` (the daemon runs them on one).
3. c3-a: is Keep listening, with the daemon already installed there, observed once with a person present (needed for the P3 and P4 labels on c3-a)?
4. The real IR remote and wake and darken; the panel flash by eye; the power draw of `Screen Off` and Always Ready (a plug meter): these keep the cost lines of 4.4 as "not measured" and "not observed".
5. Should the app warn when Hi LG is on and MIC is `listening` (a state not seen)? Currently it shows N-hilg-on.
6. Whether the sentence should also appear on the main screen's headline area (the owner can decide after seeing the screen).
