# EXP-024 — Assistant overlay window on webOS 8.3 (window type, second app, focus, cost, log leakage)

**Status:** TARGET_VERIFIED on c3-a for window type, registration, launch and relaunch, foreground app, self-close, cost and log leakage; **INCONCLUSIVE for remote key routing** (no key was pressed; `document.hasFocus()` became true); visual appearance not checked (no screen capture, by rule)
**Date:** 2026-11-07 (three short sessions, about 18:05 to 18:30 CET)
**Device alias / model / firmware / webOS / architecture:** c3-a, LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0, Homebrew Channel 0.6.3, aarch64 kernel with 32-bit userland; satellite 0.4.6 running (not touched; the same daemon pid before and after)
**Power and playback state:** `Active`; foreground app `com.webos.app.hdmi1` (the owner's PC on HDMI 1) throughout; load average about 12 (the TV's own software)
**Related research / requirements / issue:** [assistant overlay design](../design/assistant-overlay.md), [ADR 0014](../decisions/0014-assistant-overlay.md), [remote mic button research 3.3](../research/remote-mic-button.md#33-on-screen-feedback) (CleanYourTV's `topui` window)
**Risk:** installs, launches and removes **our own** probe apps only; read-only SAM queries (`getAppInfo`, `getForegroundAppInfo`, `running`), `/proc` reads, log search. No sound, no LG app or service changed, no screen capture, no key injected
**Approval needed / obtained:** the coordinator's brief for this feature (owner request of 2026-11-07): install and launch our own app on c3-a only, check the window from logs and app state; screen capture, sound, LG apps and services, debugfs and dapm not allowed

Raw output is in the gitignored `devices/private/c3-a/EXP-024/` and is not committed.

## Question and falsifiable hypothesis

Can a homebrew web app draw a small transparent panel over whatever plays on webOS 8.3, without becoming the foreground app, and how is it packaged, started, fed and closed?

1. **Window type.** H1: `"defaultWindowType":"topui"` works on webOS 8.3 (CleanYourTV uses it from webOS 25 on). H1b: `"overlay"` works (CleanYourTV's choice for SDK 5 to 9).
2. **Second app in one package.** H2: SAM registers a second app of the same IPK at install time (CleanYourTV needs a SAM restart on webOS 25).
3. **Foreground and focus.** H3: the app in front stays the foreground app while the overlay shows; the overlay page does not get focus.
4. **State delivery.** H4: `applicationManager/launch` with `params` starts the page with `webOSSystem.launchParams`; a later launch of the running app arrives as a `webOSRelaunch` event, also when it is sent while the page still loads.
5. **Close.** H5: `window.close()` from the page ends the app (no hidden window left).
6. **Cost.** H6: launch latency well under a second when warm; a few percent of a core while shown.
7. **Log leakage.** H7: launch parameters (cold and relaunch) are not retained in readable logs (WAM's OSE source logs the first launch's `launch_params_` at INFO).

## Prerequisites and safe baseline

- Power state `Active` checked first by each script (`tvpower/power/getPowerState`); a script aborts otherwise (an app launch must never wake a dark screen).
- Probe apps: ids `io.github.pando85.vsprobe*` (ours), built in the session scratchpad from the repository's `packaging/webos/overlay/` page plus a probe hook (`probe.js`, not shipped) that appends one line per page event (load, frame, focus at 0.5 s and 2 s, close) with a restricted character set to `/tmp/vsprobe/report.log` through the Homebrew Channel `exec`. Then the real overlay package built by `build_ipk.py overlay_package()`.
- Installed and removed with `packaging/webos/tv/appinstall.sh` (appinstalld `dev/install` and `dev/remove`, the Homebrew Channel path). Scripts under `/tmp`, run over `ssh -tt`.
- Baseline: foreground app, WebAppMgr processes (pid, RSS, CPU ticks), load average, free memory.

## Commands / steps (copyable and bounded)

1. Session 1: install one package with three apps (dummy main, `.top` with `topui`, `.ovl` with `overlay`); `getAppInfo` each; launch each with a `listening` frame plus a random marker `EXP024C…Z` in an extra parameter; relaunch `thinking` (marker `EXP024R…Z` in `heard`), `answer`, `done`; foreground app and processes after each step; wait 8 s; `running`; page report; marker search in `/tmp/var/log` (= `/var/log`), `/var/luna`, `/tmp/pmlogdaemon`, `/run/log`, `/media/developer/log` (plain and `.gz`), `journalctl -a`, `logread`, `dmesg` (the EXP-012 P9 set).
2. Session 1b: the same with two single-app packages (`topui`, `overlay`).
3. Session 2: the `overlay` probe again: a relaunch sent **immediately after the cold launch's reply**, a re-send after 3 s, an `error` frame and its auto-hide; a second cold run closed by a `hide` frame; then the **real** overlay package (`io.github.pando85.voicesatellite.overlay`): `getAppInfo`, launch, answer, done, wait for the self-close.
4. Session 3: the real package with the stepped animation (5 Hz, no CSS animation): CPU ticks of all `/usr/bin/WebAppMgr` processes over 6 s idle, 6 s listening, 6 s static answer; renderer RSS; `hide`.
5. Cleanup after each session: `appinstall.sh remove` of every probe and overlay app, `rm` of the scripts, packages and report.

## Expected evidence and failure interpretation

| Item | Supports | Refutes |
| --- | --- | --- |
| 1 | `getAppInfo` true and launch `returnValue: true` | "Unsupported Application Type" / `not exist`: that type cannot be used |
| 2 | the extra apps answer `getAppInfo` after install | they do not: ship the overlay as its own package |
| 3 | `getForegroundAppInfo` unchanged; `document.hasFocus()` false | the foreground changes (the overlay replaced the app); focus true: keys may go to the overlay (needs a key test) |
| 4 | the page logs `how=launch` then `how=relaunch` frames in order | missing frames: a re-send is needed |
| 5 | `running` no longer lists the app; no renderer process | a hidden window lingers (reported to hold the remote on older webOS) |
| 7 | markers found nowhere | a hit: texts must not travel in launch parameters |

## Observations and redacted evidence links

### Window type and packaging (`verified-on-target`, c3-a)

- **Two extra apps in one package are not registered.** Session 1: the dummy main app answered `getAppInfo` `returnValue: true`; both extra apps were on disk under `/media/developer/apps/usr/palm/applications/` but `getAppInfo` answered `{"errorCode":1,"returnValue":false,"errorText":"Invalid appId specified OR Unsupported Application Type: …"}` and `launch` `{"errorCode":-101,"returnValue":false,"errorText":"not exist"}`. H2 refuted (same as CleanYourTV's webOS 25 note). Not tried: a SAM restart (an LG service; not allowed, and it blinks the screen).
- **`topui` is refused.** As its own single-app package, the `topui` app gave the same `Unsupported Application Type` and `not exist`. H1 refuted for webOS 8.3.
- **`overlay` works** as its own package: registered at once (`getAppInfo` true with `transparent: true`, `class.hidden: true`), launch `returnValue: true`. H1b supported. The real package (session 2) reported `"defaultWindowType":"overlay"`, `"transparent":true`, `"visible":false`.

### Launch, relaunch, close (`verified-on-target`, c3-a)

- **Cold start:** session 1, the first web page started on the TV that day (a WebAppMgr network utility process was created with it): launch reply 922 ms, page script 2.0 s after the call. Sessions 2 and 3: launch reply 211 ms and about 0.4 s from the call to the page script (376 and 425 ms).
- **Relaunch:** reply 131 to 265 ms; the frame reached the page 49 to 95 ms after the call (`webOSRelaunch`, the page re-reads `webOSSystem.launchParams`).
- **Relaunch during the cold start (session 2):** a `thinking` frame sent right after the cold launch's reply was applied 67 ms after the page loaded (`how=relaunch`, in order). H4 supported for this one trial; the daemon's one re-send 2.5 s after a run's first frame is kept as a cheap safety net.
- **Close:** after `done` (5 s) or `error` (4 s) and after `hide` the page closed itself (`window.close()`); `running` no longer listed it and no renderer process was left. H5 supported.
- **Runtime facts from the page:** `webOSSystem` and `PalmSystem` present; `setInputRegion`, `setKeyMask`, `keepAlive`, `hide`, `activate`, `deactivate` are functions; `setInputRegion([{x:0,y:0,width:0,height:0}])` and `setKeyMask([])` accepted the array form; window 1920x1080; `navigator.language` `es-ES`; Chromium 94.

### Foreground and focus (`verified-on-target` for SAM; key routing **not tested**)

- `getForegroundAppInfo` stayed `com.webos.app.hdmi1` before, during (2 s after the launch, with an answer shown) and after, in every session.
- The page's `document.hasFocus()` was false at load and 0.5 s later, and **true at 2 s and after** (`visibilityState` visible). So the overlay window may hold keyboard focus inside the web runtime even though SAM keeps the app below in front. Whether a remote key then reaches the overlay instead of the app below is **not known** (no key was pressed or injected). The page closes itself on the first key it receives, so the worst case is one lost key press; this is the owner's check.

### Cost (`verified-on-target`, c3-a, load average about 12)

- Memory while shown: the overlay's renderer process 35 to 37 MB RSS, gone after the close. WebAppMgr's main process grew from 39 MB to 89 MB at the first launch and stayed at 69 to 82 MB afterwards; the network utility process (29 MB) created at the first launch stayed.
- CPU with the first page (CSS keyframe animations at 60 fps): WebAppMgr's main process about 36 % of one core while bars or dots moved, about 5 % with a static panel.
- CPU with the shipped page (bars and dots moved by script 5 times a second, no CSS animation), all WebAppMgr processes: idle 0, **listening 17 % of one core**, static answer 0.7 % (103 and 4 ticks in 6 s). A cold start costs more (not separated from the listening sample in session 1).

### Log leakage (`verified-on-target` for the places searched)

- The cold-launch marker (in the launch parameters) and the relaunch marker (in `heard`) of session 1b were **not found** in any of the places listed in step 1. H7 supported for those places. Limits as in EXP-012: memory-only buffers not searched; the parameters are visible in `/proc/<pid>/cmdline` of `luna-send` while it runs.

## Conclusion and confidence

- Use an **`overlay` window** in a **separate single-app package** (`<app id>.overlay`), installed through appinstalld by `ctl.sh install`; frames in the launch parameters, later frames by relaunch; the page closes itself after each run. High confidence for c3-a, firmware `03.30.74`.
- `topui` and a second app inside the main package do not work on this firmware without a SAM restart.
- Open: whether remote keys reach the overlay while it shows (focus became true in the web runtime); the look over video, HDMI, the home screen and native apps; behaviour in `Screen Off` and `Active Standby` (the daemon does not launch then, by design); c3-b and other firmware.

## Rollback and cleanup

Every probe app and the real overlay app were removed with `appinstall.sh remove` (`remove done` each); `/media/developer/apps/usr/palm/applications/` lists the same apps as before; no file left under `/tmp`; no renderer process; the satellite 0.4.6 daemon kept the same pid.

## Follow-ups / unresolved questions

1. Owner's visual and remote check after the release (list in the [design, section 10](../design/assistant-overlay.md#10-the-owners-visual-check)).
2. If keys do reach the overlay: look for a documented or observed way to give the focus back (the page exposes `webOSSystem.deactivate`, whose effect on an overlay window is not known; in OSE's WAM a background move closes an overlay window). Nothing is guessed here; it needs its own experiment.
3. c3-b and other firmware: repeat steps 2 and 3.
