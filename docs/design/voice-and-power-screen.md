# Voice and power screen in the TV app

- **Status:** design, accepted by the owner on 2026-11-05 ([ADR 0008](../decisions/0008-user-initiated-lg-settings.md)); the luna call shapes below are `verified-on-target` on **c3-a** by [EXP-011](../experiments/EXP-011-lg-settings-toggle.md) (2026-11-05, firmware `03.30.74`; c3-b was not reachable, so its read-only part is not run). Implementation: V2 (`ctl.sh`, host-tested with a fake `luna-send`), V3 (daemon), V4 (the app, commit `85aeb35`) and V5 (the two HA sensors, commit `7b44281`) are written and shipped in 0.3.0 (V6); V7 (deploy and checks on c3-a) is recorded in [P4](../discovery/c3-a/P4-permanent-install.md#update-to-030-2026-11-05-voice-and-power-screen). **The strings in section 4 are copied from `packaging/webos/app/app.js` (exported as `window.voicePower`), which is the source of truth; `tests/app_matrix.js` checks them.**
- **Date:** 2026-11-05. TV facts are `verified-on-target` for **c3-b** (LG C3 55", webOS TV 8.3.0, firmware `03.30.73`) unless a label says otherwise; c3-a (`03.30.74`) is a different unit and mostly untested for this ([c3-b record](../compatibility/c3-b.md), [c3-a record](../compatibility/c3-a.md)).
- **Related:** [always-on design](always-on.md) (sections 1, 11 to 13), [TV control](tv-control.md), [install and configuration UX](install-and-config-ux.md), [packaging](packaging.md), [ADR 0005](../decisions/0005-tv-control-entities.md), [ADR 0008](../decisions/0008-user-initiated-lg-settings.md), backlog [#14](../project/backlog.md#14-voice-and-power-screen-in-the-tv-app).

## 1. Scope

A "Voice and power" screen in the existing status app (`packaging/webos/app/`) with three toggles and one live sentence that says what the remote's power button will do and whether the satellite can hear you:

| # | Toggle | Kind | Stored in | Written by |
| --- | --- | --- | --- | --- |
| 1 | **Always Ready** | LG setting `general/lifeOnScreenMode`; the TV lists five values (`none`, `off`, `lifeOnScreen`, `alwaysReady`, `allEnabled`); the app reads all and writes only `off` and `alwaysReady`; the toggle is enabled only while the value is one of those two, otherwise the row is disabled and the TV's menu is named | the TV (settingsservice) | the app, on a press, through `ctl.sh set always-ready` |
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
| Hi LG on **before the satellite opened the PCM**: LG's voice service holds `hw:LGSIC,0`, the satellite reports `MIC BUSY`, `listening` off, in the states tried | verified-on-target (c3-b, `Screen Saver`, one boot) | [P4 permanent install](../discovery/c3-b/P4-permanent-install.md), [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md) |
| Hi LG switched **on while the satellite already held the PCM**: no `MIC BUSY`, the PCM stayed with the satellite, HA Listening on, for 193 s. So the rule seen is **one microphone, the first opener wins**; that LG's service opens it first after a reboot is a **hypothesis** from the c3-b observation | verified-on-target (c3-a, `03.30.74`, one run; order effect only) | [EXP-011 part B2](../experiments/EXP-011-lg-settings-toggle.md) |
| Whether LG's own "Hi LG" wake word works while the satellite holds the PCM; whether switching Hi LG off makes `voiceinput` release a PCM it already holds (the daemon retries a busy PCM without limit) | **not tested** | EXP-011 follow-ups |
| `getSystemSettings` and `setSystemSettings` on `general/lifeOnScreenMode` (`off`, `alwaysReady` written and read back at once, no dialog through luna); `getSystemSettingDesc` lists the five values; `alwaysready/aod/getStatus` stays `none` and does **not** reflect the setting (do not use it) | verified-on-target (c3-a, `03.30.74`) | EXP-011 parts A and B1 |
| `voiceinput/setting/getActivation {}` (top level plus `thinQ`, `amazon`, `dangbei`, each with `settingStatus` and `activationState`) and `setActivation {"settingStatus":bool}` (the schema has only that property; the reply names `serviceName` `thinQ`: only thinQ changes) | verified-on-target (c3-a, `03.30.74`) | EXP-011 parts A and B2 |
| `general/lifeOnScreenMode` and `setActivation {"settingStatus":bool}` were written and read back by agents | verified-on-target (c3-b, D5, D6, P4 session 2) | their records |
| A change made in LG's own menu shows in the getters (and the other way round); an on-screen prompt after `setActivation true`; whether LG reverts a value by itself; whether `subscribe` works for these; the getters in `Active Standby`; c3-b | **not tested** | EXP-011 follow-ups |

## 3. Outcome matrix (the single source for the app sentence)

### 3.1 Inputs

- **AR**: Always Ready, from the raw `lifeOnScreenMode` word in `tv-settings` (the app reads `always_ready.raw`; the JSON `state` is informational): **off** = raw `off` or `none`, **on** = raw `alwaysReady` or `allEnabled` (the power button behaved the same with both on c3-b, `verified-on-target`), **not known** = any other raw word (for example `lifeOnScreen`) or unreadable (PX, and the note X-unknown). The **toggle** is a separate question: it is enabled only while the raw word is `off` or `alwaysReady`. `allEnabled` shows `On, with wallpaper` (disabled), any other word `Not known (<raw>)` (disabled), both naming LG's menu.
- **HL**: Hi LG, from `getActivation`: `on` when **any** provider (`thinQ`, `amazon`, `dangbei`) reports `settingStatus true`, `off` when all report false, `error` when not readable. The app reads `hi_lg.thinq`, `amazon` and `dangbei`: the **toggle** shows and changes thinQ only (`thinq true` is Hi LG on); when `amazon` or `dangbei` is true the screen adds the line N-other-handsfree. HL for the notes below is `on` only when thinQ is on.
- **RO**: `etc/remote_off`: `normal` or `keep_listening`.
- **MIC**: from the daemon's status file `run/voice.json` and the kill-switch file: `listening`, `busy`, `muted` (HA's mute switch or a gate), `closed` (other), `unknown`; the kill switch comes from `mic.kill_switch`; plus `running` (daemon alive).

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
| 4 | MIC = `busy` | **L-hilg** when HL = `on`, else **L-busy** |
| 5 | MIC = `listening` | **L-ok** |
| 6 | MIC = `closed` | **L-closed** |
| 7 | anything else | **L-unknown** |

The observed MIC state wins over the setting. With HL `on` and MIC `listening` the screen shows L-ok and adds **N-hilg-listening**; with HL `on` and MIC `closed` or unknown it adds **N-hilg-on** instead. The first state is real, not hypothetical: EXP-011 held Hi LG on for 193 s with the satellite listening on c3-a ("one microphone, first opener wins"). What happens after a restart or a reboot (LG's service may open the PCM first, as on c3-b) is a hypothesis, which is why the line warns instead of reassuring. With HL `on` and MIC `busy` the satellite cannot hear (L-hilg). Whether turning Hi LG off releases a PCM that LG's service already holds is **not tested**; the text does not promise it, the daemon keeps retrying a busy PCM.

### 3.4 Sentence

`sentence = headline(P) + " " + detail(P)`, then a new line with the listening line. The headline is the large text, the detail the normal text. A cell with PX shows the PX headline and detail and the note X-unknown, and no claim about the power button.

## 4. User-facing strings (English, exact)

Short, plain, readable from the sofa at 22 px or larger. Strings are the contract: the app's matrix test compares them.

### 4.1 Power button

| Cell | Headline | Detail |
| --- | --- | --- |
| P1 | `Power button: the TV goes to deep standby.` | `The satellite cannot hear until the TV is turned on.` |
| P2 | `Power button: the TV goes to standby.` | `The satellite still hears you, but you will not hear its replies.` |
| P3, P4 | `Power button: the screen goes dark but the TV stays on.` | `The satellite hears you and replies. Press power again to turn the picture on.` |
| PX | `Power button: not known.` | `The TV's settings could not be read. Press Refresh.` |

(Facts behind the short texts: P1 is `Active Standby` and then suspend 7 to 15 s after the key; P2 stays in `Active Standby`, replies silent, and the TV controls in Home Assistant are unavailable; P3 and P4 are the daemon's wake and darken. See 3.2.)

With the kill switch on, or with `remote_off.active` false (no power watch), "Keep listening" does nothing, so the cell is the P1 or P2 one for the current AR and the detail is followed by `Keep listening does nothing while the microphone is off with the kill switch.` (kill switch) or `Keep listening needs the power watch, which is off in the config.` (no power watch), when RO = `keep_listening`.

### 4.2 Listening line

| Cell | Text |
| --- | --- |
| L-ok | `Listening now: yes.` |
| L-hilg | `Listening now: no. The satellite cannot hear: LG's voice assistant has the microphone. Turn Hi LG off to use the satellite.` |
| L-busy | `Listening now: no. Another program has the microphone.` |
| L-muted | `Listening now: no. The microphone is muted in Home Assistant.` |
| L-kill | `Listening now: no. The microphone is off (kill switch).` |
| L-stopped | `Listening now: no. The satellite is not running.` |
| L-closed | `Listening now: no. The microphone is closed.` |
| L-unknown | `Listening now: not known.` |
| N-hilg-listening (extra line; HL = `on` and MIC = `listening`) | `Hi LG is on but this satellite has the microphone now; after a restart LG may take it. Turn Hi LG off to be sure.` |
| N-hilg-on (extra line; HL = `on` and MIC = `closed` or unknown) | `Hi LG is on. If the satellite cannot hear you, turn it off.` |
| N-other-handsfree (extra line; `hi_lg.amazon` or `hi_lg.dangbei` true) | `Alexa/other hands-free is on: turn it off in LG's menu.` |
| X-unknown (extra) | `Always Ready or Hi LG could not be read, so the sentence above may be wrong.` |

### 4.3 Toggle rows

Row label and the state word shown at the right:

| Row | Label | State words |
| --- | --- | --- |
| 1 | `Always Ready` | `On` (raw `alwaysReady`), `Off`, `On, with wallpaper` (raw `allEnabled`, disabled), `Not known (<raw>)` (any other word, disabled), `Not known`, `Cannot read`; the disabled rows say `Always Ready is set to <what>, which this app does not change. Change it in LG's menu (Settings, General, Always Ready).` |
| 2 | `Hi LG` | `On`, `Off` (both for thinQ), `Not known`, `Cannot read` (disabled, with the menu path) |
| 3 | `Keep listening` | `On`, `Off`, `Cannot read` |

Row sub-line (always visible, one line): row 1 `Keeps the TV in standby for LG's own features. Uses more standby power.`; row 2 `LG's own voice assistant. When it is on, the satellite may not be able to use the microphone.`; row 3 `A satellite setting. The TV stays on with the screen dark after the power button.`

### 4.4 Warnings and confirm texts

Shown in the warning area while a row is focused, and as the confirm text after the first OK.

| Change | Text (the confirm prompt ends with `Press OK again to confirm.`) |
| --- | --- |
| Always Ready off to on | `Turn on Always Ready? The TV then uses more power in standby (not measured). With "Keep listening" off, the satellite hears you after the power button but its replies are silent.` |
| Always Ready on to off | `Turn off Always Ready? Without "Keep listening", the TV suspends after the power button and the satellite stops.` |
| Hi LG off to on | `Turn on Hi LG? LG's assistant may take the microphone, so the satellite may not hear you. It did not while the satellite already had the microphone, but it can after a restart.` |
| Hi LG on to off | `Turn off Hi LG? The TV stops answering "Hi LG". The satellite can use the microphone.` |
| Keep listening off to on | `Turn on Keep listening? After the power button the TV stays on with the screen dark. It uses more power than standby (not measured), LG's OLED care at power-off may not run, and the screen may flash briefly (not observed).` |
| Keep listening on to off | no confirm (one press); the focused row shows `Keep listening is on. Press OK to turn it off. The power button then turns the TV off the normal way.`; the result line is `Done. The power button turns the TV off the normal way.` |

Result lines (one line under the rows, the app's existing `message` area):

| Result | Text |
| --- | --- |
| applied | `Always Ready is now On. Changed from Off.` (likewise for Hi LG and Keep listening; no time is shown). With the daemon not applying yet it adds `The satellite has not applied it yet.` (`daemon` `pending`) or `The satellite is not running; it applies this when it starts.` (`not running`) |
| already | `Already On. Nothing changed.` |
| failed, refused | `Could not change Always Ready: <reason>. It is still <value>. You can also change it on the TV: <menu path>.` (refused has no `It is still` part) |
| mismatch | `The TV reports <x>, not <y>. The change did not stick, or LG changed it back. It is <x> now.` |
| cannot read | `Could not read the TV settings: <reason>.` and the rows show `Cannot read` |

`<reason>` is the luna `errorText` (a TV message such as `not exist`) or a fixed word from `ctl.sh` (`timeout`, `no luna-send`, `not root`, `unknown value`). Menu paths for the failed and unknown cases: Always Ready: `Settings, General, Always Ready` (the path is from the TV's UI strings in [always-on](always-on.md); EXP-011 was luna only and did **not** look at the menu, so it is not verified on screen); Hi LG: `Settings, General, AI Service, Voice recognition settings, Hi LG` (the household's way, [P4](../discovery/c3-b/P4-sound-session.md#state-found-and-the-hi-lg-change)).

## 5. UI layout in the existing app

Existing app (`packaging/webos/app/`, ES5, one `main`, a hidden `report-view` second screen, whitelisted `ctl.sh` argument strings, arrows and OK, Back on 27, 8 and 461, 10 s status refresh paused while the report is open): the new screen is a third `<main id="voice-view" hidden>` opened by a new Actions button **Voice and power** (`id="b-voice"`, shown when installed; handled like `report --json` by a special case in `onButton`). The "Not available yet" panel text loses "power options (screen off, Always Ready)".

```
Voice and power                                   (h1)
+--------------------------------------------------------------+
| Power button: the screen goes dark but the TV stays on.       |  headline, aria-live
| The satellite hears you and replies. Press power again to     |  detail
| turn the picture on.                                          |
| Listening now: yes.                                           |
+--------------------------------------------------------------+
| [ Always Ready                               Off ]            |  row 1  (focus)
|   Keeps the TV in standby for LG's own features. ...          |
| [ Hi LG                                      Off ]            |  row 2
| [ Keep listening                              On ]            |  row 3
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
| not changeable (Always Ready raw word other than `off` and `alwaysReady`) | `On, with wallpaper` for `allEnabled`, else `Not known (<raw>)` | `warn` | nothing (the row is disabled); the warning area names LG's menu (decision 8 of ADR 0008) |
| unavailable (the read failed: no luna-send, timeout, not root, no reply) | `Cannot read` on every row | `bad` | nothing; the sentence is PX with the note X-unknown |

**Refresh.** On open: one `tv-settings --json` (rows show `Checking...`). While the screen is open: every 10 seconds when nothing is pending or armed (the main screen's refresh is paused, as for the report). After a successful `set`: the command's own read-back is shown at once, plus one more `tv-settings --json` 3 seconds later to catch an LG-side revert (a revert shows as the mismatch line of 4.4). The Refresh button forces a read. Leaving the screen resumes the main refresh. **Simplification as built:** a failed read does not keep the last good values; every row shows `Cannot read` and the sentence is the not-known one until a read succeeds.

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

All need root (as the other commands; `WEBOS_SAT_ALLOW_NONROOT=1` for tests) and the install lock for writes. A new environment variable `WEBOS_SAT_LUNA_SEND` (default `luna-send`) lets the host tests substitute a fake; luna calls are `luna-send -n 1 -w 3000 <uri> <payload>` (one reply, 3 s) and are made like this. The daemon runs its calls on a pseudo-terminal because the audio getters printed nothing over a plain pipe ([TV control 5](tv-control.md#5-interface-per-control), EXP-010). **Whether settingsservice and voiceinput need a terminal is not known:** EXP-011 ran every call under `ssh -tt`, so it cannot tell, and the app's `exec` has no terminal. TV facts: `script` is not in the TV's BusyBox applet list as far as recorded, but python3 3.8 is on c3-a ([D0-D1](../discovery/c3-a/D0-D1-runtime.md)). So `ctl.sh` does not assume: the **first read** of a run goes over a plain pipe (`timeout 6`, stdin from `/dev/null`); if nothing that looks like JSON comes back, it repeats that read on a pseudo-terminal made by `python3` (`os.openpty` plus `subprocess`, 6 s cap, output limited to 64 KiB; **not** the `pty` module, which c3-a's python3 3.8 does not have) and uses that mode for the rest of the run. A write is never repeated in another mode: reads always come first, so the mode is known before the write. With neither mode working the field is `error` `timeout`. **Checked on c3-a (0.3.0, 2026-11-05):** a plain pipe was enough, over plain SSH and through the app's `exec` (about 0.2 s over SSH, 0.8 s through `exec`); the `pty` fallback was not needed there. **To check on another TV:** run `ctl.sh tv-settings --json` through the app's `exec` (no tty) and see that both LG fields answer; whether the plain mode was enough is not printed, test it once by hand with `luna-send -n 1 luna://com.webos.settingsservice/getSystemSettings '{"category":"general","keys":["lifeOnScreenMode"]}' </dev/null | cat` over a plain SSH session; python3 on c3-b is not checked.

### 6.1 `tv-settings --json` schema (version 1)

```json
{
  "schema": 1,
  "luna": true,
  "daemon_running": true,
  "always_ready": {"state": "off", "raw": "off", "error": ""},
  "hi_lg": {"state": "off", "thinq": false, "amazon": false, "dangbei": false, "error": ""},
  "remote_off": {"state": "keep_listening", "active": true},
  "mic": {"state": "listening", "kill_switch": false},
  "power_state": "Active",
  "last_change": {"setting": "always-ready", "from": "off", "to": "on", "at": "2026-11-05T20:10:03Z"}
}
```

- `luna`: `luna-send` exists and the caller is root; false makes both LG states `error` with `error` `no luna-send` or `not root`.
- **What the app parses (V4, `app.js`):** `daemon_running`, `always_ready.raw` (and `error`; `state` is read only to tell `error` and as a fallback when `raw` is empty), `hi_lg.thinq`, `amazon`, `dangbei`, `state` and `error`, `remote_off.state` and `active`, `mic.state` and `kill_switch`. Everything else is informational. The app derives its Always Ready cell from `raw`, so `allEnabled` counts as on and `none` as off there.
- `always_ready.state`: `off` (raw `off`) | `on` (raw `alwaysReady`) | `unknown` (any other raw word: `none`, `lifeOnScreen`, `allEnabled`, anything) | `error`; `raw` is the TV's word, limited to letters and digits and 24 characters, empty on error. The getter is `luna://com.webos.settingsservice/getSystemSettings {"category":"general","keys":["lifeOnScreenMode"]}` (read-only; `verified-on-target` on c3-a, reply `settings.lifeOnScreenMode`). `alwaysready/aod/getStatus` is **not** used (it stays `none` whatever the setting is).
- `hi_lg.state`: `on` when any provider's `settingStatus` is true, `off` when none is, `error` when unreadable. `thinq`, `amazon`, `dangbei`: that provider's `settingStatus` (`true`, `false`, or `null` when the reply has no such object). The getter is `luna://com.webos.service.voiceinput/setting/getActivation {}` (the schema accepts only `{}` or `{"subscribe":true}`); if the reply has no `thinQ` object, the top-level `settingStatus` stands in for `thinq`. No trigger word, locale or other field is copied.
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

`result`: `applied` (read back equals the request), `already` (before equals the request, **no write is made**), `mismatch` (written, the read-back differs), `failed` (the setter returned `returnValue:false`, timed out, or the read-back could not be read; `error` holds the luna `errorText` (or `error <errorCode>`), `timeout`, or `read-back: <reason>`; never a reply body; `after` is what the read-back reported, possibly equal to the request when the setter's reply was lost), `refused` (see errors; the write was not made). Exit 0 for `applied` and `already`, 1 otherwise. Steps for an LG setting: take the lock; read (`before`, a failed read is `refused`); append a `request` line to the history; write; wait 0.5 s; read back; append a `result` line; print. **One write, one read-back, no retry, no second method or parameter set** (as [TV control 6](tv-control.md#6-capability-detection-at-startup)). The payloads (`verified-on-target`, c3-a, EXP-011): Always Ready `luna://com.webos.settingsservice/setSystemSettings {"category":"general","settings":{"lifeOnScreenMode":"alwaysReady"}}` for `on` and `"off"` for `off`, reply `{"returnValue":true,"method":"setSystemSettings"}`, read-back immediate; Hi LG `luna://com.webos.service.voiceinput/setting/setActivation {"settingStatus":true|false}`, reply `{"serviceName":"thinQ","returnValue":true,"settingStatus":...}`. `setActivation` has **no provider parameter**, so it changes thinQ only; `amazon` and `dangbei` are not switchable by the app (the row says so when one is on). For Hi LG `before`, `requested` and `after` are the **thinQ** value. `ctl.sh` never writes `none`, `lifeOnScreen` or `allEnabled`, and **refuses** (`unknown value`) a change of Always Ready while the TV holds any value other than `off` or `alwaysReady` (decision 8 of ADR 0008). After an applied change it creates `run/lg-settings-refresh` (for the HA sensors, section 7). The history file also gets one `remote-off result` line per `set remote-off`.

`set remote-off` (no luna): validates, then follows section 6.5 (the file is written first, then the trigger `run/remote-off-reload` is created, also when the daemon is not running; a leftover trigger is harmless, the next start reads the file and a reload finds the same word), prints `result` `applied` when the file holds the word and, with the daemon running, the daemon consumed the trigger within 3 s (`"daemon":"applied"`), `"daemon":"pending"` when the trigger was not consumed in 3 s, `"daemon":"not running"` when stopped (the next start reads the file).

### 6.3 Old-value log

`$P/etc/tv-settings.log` (`/var/lib/webos-satellite/etc/tv-settings.log`): append-only text, one line per event, UTC time, no identifiers:

```
2026-11-05T20:10:03Z always-ready request from=off to=on source=ctl
2026-11-05T20:10:04Z always-ready result from=off to=on read=on outcome=applied
```

It lives in `etc/` (kept by `uninstall`, removed by `purge`, not rotated with the daemon log), is capped by ctl.sh itself at 200 lines (oldest dropped when appending), mode 0600. `source` is always `ctl` (the app and an SSH session run the same script). `last_change` and the sub-line "Before: Off" of a row read it. A restore is `ctl.sh set <setting> <from>` of the last `applied` line; the app does not need a separate restore button in the first version (the row's confirm text names the old value).

### 6.4 Status file from the daemon

The daemon writes `run/voice.json` (atomically with a temporary file and `rename`, mode 0600 like `compat.json`; `ctl.sh` runs as root) with one JSON line:

```json
{"schema":1,"mic":"listening","remote_off":"keep_listening","remote_off_active":true,"power_state":"Active"}
```

- `mic`: `listening` (the capture PCM is open and delivering), `muted` (closed because a gate holds it: HA's mute switch, the kill switch, the firmware guard or an unpaired satellite), `busy` (wanted but another process holds the PCM, for example LG hands-free), `closed` (any other reason). `ctl.sh` still reads the kill-switch file itself for `mic.kill_switch`.
- `remote_off`: the mode the daemon holds now (`normal` or `keep_listening`).
- `remote_off_active`: the power watch runs, so keep listening can act (false with `power.watch = false` or no `luna-send`); it does not depend on the mode.
- `power_state`: the last settled power state word from the subscription (`Active`, `Screen Off`, `Active Standby`, ...), or `null` before the first update.

It is checked once a second from the daemon's existing trigger poll (never per audio frame) and rewritten only when the content changed, plus a keep-alive rewrite every 30 s so that a file older than 60 s can be read as stale (`unknown`/null, for example after a crash); a clean shutdown deletes it. It carries no identifiers. It lives in the report directory (`report.dir`, `run/` in the package) beside `compat.json`, which it follows as the daemon-to-script channel. Implemented in `crates/satellite/src/voice_status.rs` (step V3).

### 6.5 How the running daemon picks up a `remote_off` change without a restart

**How it works today** (`crates/satellite/src/daemon.rs`, `darken.rs`, `config.rs`): the HA select arrives as `on_select`, which calls `darkener.set_mode(mode)` (leaving keep listening stops a wake or darken in progress) and writes the word with `volume::write_atomic` to `power.remote_off_file` (default `etc/remote_off`). The file is read **only at startup** (`darken::load_mode`); `ctl.sh status` already reads the same file. So a change made by `ctl.sh` would be ignored until a restart.

**Proposed mechanism: a reload trigger file plus a shared apply function.**

1. `ctl.sh set remote-off normal|keep` writes the word (`normal` or `keep_listening`) to the configured `remote_off_file` with a temporary file and `mv` (the same atomic write as the daemon; the path comes from `cfg_value remote_off_file` resolved against `etc/`, default `etc/remote_off`), then creates the trigger `run/remote-off-reload` (empty file), the same pattern as `selfcheck-run` and `firmware-ack`.
2. The daemon's existing once-a-second trigger poll (`poll_compat_triggers`, directory `report.dir`) also consumes `remote-off-reload` with the existing `consume_trigger`, then calls `darken::load_mode(&remote_off_file)`. `Ok(Some(mode))` different from the current mode goes through `apply_remote_off(mode, source)` (implemented, step V3) that holds the code now inside `on_select` (set the mode, the `warn` or `info` log line with the source `ctl.sh`, **no file write** because the file already holds the value) and sends `AppCommand::SetSelect` with the new option so HA's select follows. `Ok(None)` (file missing) or `Err` logs one `warn` and keeps the current mode; the same word as now is ignored. Without the power watch the select entity does not exist, so no `SetSelect` is sent. HA's own select still goes through `on_select`, which now calls `apply_remote_off(mode, "Home Assistant")` and then writes the file.
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
| TV in a transient power state | the write is attempted; a failure is shown as `failed`. EXP-011 ran in `Active` only; other states are **not tested** |
| History file not writable | the write is **not made** (the log line is written first; an unlogged change is refused) |

## 7. Home Assistant: two read-only diagnostic sensors

| Entity | Type | Object id | Category | Values | Listed when |
| --- | --- | --- | --- | --- | --- |
| Always Ready | `text_sensor` | `lg_always_ready` | diagnostic | `Off`, `On`, `On with wallpaper`, `Other` (raw word in the log, not in HA); missing state when the read failed | `[tv_control] lg_settings` (default **true**, read-only); listed whenever it is on, whether or not the startup getter answered (a failed getter shows no state, the entity is not left out) |
| LG hands-free | `text_sensor` | `lg_hands_free` | diagnostic | `Off`, `On`, `Other`; missing state when the read failed | as above |

**Hands-free collapse rule (V5):** the reply has a top-level `settingStatus` and one object per provider (`thinQ`, `amazon`, `dangbei`) with its own `settingStatus`. The sensor shows `On` if the top level or any of the three provider values is `true`, `Off` if every value present is `false`, `Other` if any is not a boolean (and none is `true`). No `settingStatus` at all is a read failure (no state).

Text sensors rather than binary sensors because Always Ready has more than two values and an `Other` must not read as "off". No `switch`, `select` or command for either: writes stay in the app (ADR 0008 decision 4). Names are fixed by the owner ("Always Ready", "LG hands-free"); the ESPHome object ids are stable keys.

**Cadence (cheap).** Reuse the TV-control machinery (`crates/tv-control` controller and `crates/satellite/src/tv_control.rs`, calls as `luna-send -n 1 -w 3000` on a pseudo-terminal): the controller gets two new read-only `Purpose`s with a parser per reply and two sensor `EntityDef`s, no setter and nothing in the setter limits. Reads:

- once at startup in the capability check (the sensors are listed either way; a getter that fails gives no state and one `warn`, and **the polls continue** after a failed startup read, so the state appears when a later read answers) and again after a resume;
- every **60 s** while the power state is `Active`, `Screen Saver` or `Screen Off`; none in `Active Standby` or a transition (the other TV getters are not asked there either);
- immediately when `ctl.sh set` finishes (it creates the trigger `run/lg-settings-refresh`, consumed by the same one-second trigger poll as in 6.5), so HA shows the new value within a second;
- no subscription: `subscribe` was **not tested** in EXP-011 (`getActivation` accepts `{"subscribe":true}` by its schema; `getSystemSettings` was not tried with it), so polling stays.

Cost: two short `luna-send` calls a minute, far below the 5 to 10 s polls the controller already runs. A change seen by the poll that the app did not make is logged once at `info` (for example `lg setting: always-ready off -> other (lifeOnScreen) (not changed by the satellite)`; the raw word is shown only for `Other`), which would have explained the unexplained "Hi LG" switch-off of 2026-11-04. A value that cannot be read is logged once at `warn`. The sensors report the value; they never write, correct or "fix" it. Not gated by the firmware guard (TV control rule); no identifiers in any state or log line.

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
| **V1** | **Done 2026-11-05.** The exact luna call shapes, values list and Hi LG provider handling from EXP-011 are in sections 2, 3, 4 and 6; ADR 0008 amended. Docs only | EXP-011 |
| **V2** | **Done (commit `5ccc861`).** `ctl.sh`: `tv-settings`, `set always-ready`, `set hi-lg`, `set remote-off`, the history log, `WEBOS_SAT_LUNA_SEND`, usage text; shell host tests with the fake `luna-send` (test plan 1). Shell only | V1 for the payloads (the remote-off part can go first) |
| **V3** | **Done (commit `f018c91`).** Daemon: `apply_remote_off` factored out of `on_select`, the `remote-off-reload` trigger, `run/voice.json` writer; Rust host tests (test plan 3, first part). No LG write code | none (parallel with V2) |
| **V4** | **Done (commit `85aeb35`, strings aligned in `86bfbf7`).** App: the `voice-view` screen, `outcome(state)` matrix and strings, focus, arm and confirm, refresh, whitelist additions, main-screen text; `app_smoke.js` and `app_matrix.js` (test plan 2) | V2 interface (schema) and the section 4 strings; can start from the schema |
| **V5** | **Done (commit `7b44281`; trigger `run/lg-settings-refresh`).** `webos-tv-control` and `tv_control.rs`: the two read-only sensors, the 60 s poll, the refresh trigger, the log lines; Rust and fake-HA tests (test plan 3, second part); update the entity table in [TV control 3.1](tv-control.md#31-entity-table) | V1 for the getter shape |
| **V6** | **Done in 0.3.0 (commit "version 0.3.0").** Package: build the ipk, run the packaging tests (`packaging/webos/tests/run.sh`, `ipk.sh`), docs: [development](../development.md), [packaging](packaging.md), [install UX](install-and-config-ux.md) screens section, README status; version bump | V2 to V5 |
| **V7** | **Deployed to c3-a 2026-11-05 (read-only checks and the authorized writes; see [P4, update to 0.3.0](../discovery/c3-a/P4-permanent-install.md#update-to-030-2026-11-05-voice-and-power-screen)); the on-screen check of the screen and the power-off cells are still open.** Deploy to c3-a (silent), read-only checks, then the authorized writes and matrix cells (test plan 5); record the experiment and compatibility rows | V6; a person present for the power-off cells |
| **V8** (optional, later) | A "restore the value before the first change" row action, an OLED-care counter display ([always-on 12.2](always-on.md#122-the-setting-to-expose-app-and-ha-ui)), the TV speaker run | owner, evidence |

## 9. Open questions

1. After EXP-011 (answered: values, payloads, no provider parameter, one microphone first opener wins): still open are whether LG reverts a value by itself, a menu change showing in the getters and the reverse, an on-screen prompt after `setActivation true`, the getters in `Active Standby`, `subscribe`, whether Amazon or Dangbei hands-free can be switched at all, whether Hi LG's own wake word works while the satellite holds the microphone, whether Hi LG off releases a PCM LG already holds, and the c3-b read-only part.
2. Whether luna calls for these services need a pseudo-terminal from `ctl.sh` (the daemon runs them on one): `ctl.sh` tries a plain pipe first and falls back to python3's pty (section 6); check once on the TV through the app's `exec`.
3. c3-a: is Keep listening, with the daemon already installed there, observed once with a person present (needed for the P3 and P4 labels on c3-a)?
4. The real IR remote and wake and darken; the panel flash by eye; the power draw of `Screen Off` and Always Ready (a plug meter): these keep the cost lines of 4.4 as "not measured" and "not observed".
5. N-hilg-on now says what was seen (EXP-011, c3-a): Hi LG on with the satellite listening. Whether it should stay a permanent line or appear only once, is for the owner after seeing the screen.
6. Whether the sentence should also appear on the main screen's headline area (the owner can decide after seeing the screen).
