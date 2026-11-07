# On-screen assistant overlay

- **Status:** built on the host (daemon, overlay app, packaging, TV app row, HA switch; tests below). The window approach is checked on c3-a with our own probe and overlay packages ([EXP-024](../experiments/EXP-024-assistant-overlay-window.md)); the daemon side has **not run on a TV**, and the look is the owner's check (section 10). Decision record: [ADR 0014](../decisions/0014-assistant-overlay.md) (accepted). **Off by default** in the first release, until the owner's check (section 10). Revised after the independent review of `9fca695`.
- **Date:** 2026-11-07. TV facts are `verified-on-target` for **c3-a** (LG C3 42", webOS TV 8.3.0, firmware `03.30.74`) unless labelled otherwise.
- **Owner request (2026-11-07):** "Go with the full version": a small transparent panel over whatever is playing that shows listening, thinking and the answer, like the voice bar on phones.

## 1. What it shows

One panel, 1120 x about 130 px, at the bottom centre of the screen; the rest of the window is transparent.

| Stage | When (daemon) | Panel | Hides |
| --- | --- | --- | --- |
| `listening` | a wake word run: `WAKE_WORD_END`; a run without the wake word (mic button, `continue_conversation` follow-up, the conversation after an announcement): its start | moving bars, **Te escucho…** | 30 s safety limit |
| `thinking` | the audio ended (`STT_VAD_END`), and again at `STT_END` with the text | dots, **Pensando…**, what was heard in «» | 45 s safety limit |
| `answer` | the reply starts to play (TTS stream or URL); the reply text is the `speech` of `INTENT_END` | the reply text (up to three lines), the heard text above | 180 s safety limit |
| `error` | an `ERROR` of a shown run before a reply text exists; a disconnect while listening or thinking | **No te he entendido** (`stt-no-text-recognized`), **Sin conexión con Home Assistant**, **Algo ha fallado** | 4 s |
| `done` | `RUN_END` and the reply's playback over (finished, failed or stopped) | the last texts stay; **Hecho** when there is none | 5 s (1.5 s with nothing to read) |
| `hide` | the microphone was muted | nothing | at once |

- Texts are Spanish (the household language) unless the TV's own language is English (`navigator.language`, `es-ES` on c3-a); the TV app has no translation, so there is no other language.
- The idle wake word stage (the satellite streams all the time) and its errors never show. A reply text without a spoken reply (no TTS in the pipeline, or a TTS error after the intent) shows with `done`.
- The listening bars are **not** a level meter: no audio level reaches the page (a level would need a frame per step through SAM); they move in a fixed pattern 5 times a second.
- Announcements alone show nothing; the conversation HA starts after one does.

## 2. Rules

- **Never take the remote:** the page sets an empty input region and an empty key mask at load (`webOSSystem.setInputRegion`, `setKeyMask`, array form, JSON text as a fallback); a key that still reaches the window closes it at once, so the next press goes to the app below (at worst one press is lost). SAM keeps the app below as the foreground app (EXP-024).
- **Never block the picture:** one small panel, transparent elsewhere, `pointer-events: none`.
- **Never play sound:** no audio element, no sound file; the page has no network, storage or log calls (checked by `tests/overlay.js`).
- **Never wake a dark screen:** a screen gate is open only for the settled power state `Active` with no transition in progress (`processing`, for example `Request Power Off` while the state still says `Active`), and closed while the state is not known. The sender checks it right before **every** launch, the re-send included, so "apaga la tele" (the answer and the re-send land during the power-off) launches nothing. In `Screen Off` (Keep listening), `Active Standby` or during suspend nothing is launched.
- **Never log speech:** the heard and reply texts exist in the daemon only to be shown; log lines name the stage and the run number (`Frame`'s `Debug` prints lengths). The daemon's and the voice layer's debug lines for the transcript, the reply, an announcement's text and HA's error message print lengths only. A failed launch is logged with SAM's `errorCode` and `errorText` only.
- **Close after use:** the page closes its window when a run is over; no hidden window stays (a hidden overlay window is reported to keep the remote on older webOS, CleanYourTV).

## 3. The window (EXP-024)

| Question | Answer on c3-a | Label |
| --- | --- | --- |
| `topui` window | refused: "Unsupported Application Type" | verified-on-target |
| `overlay` window | starts over HDMI 1; the foreground app stays `com.webos.app.hdmi1` | verified-on-target |
| A second app inside the main package | on disk but not registered by SAM (needs a SAM restart) | verified-on-target |
| The overlay app as its own package | registered at install | verified-on-target |
| First frame / later frames | `webOSSystem.launchParams` at load; `webOSRelaunch` for a launch of the running app, also when sent during the cold start | verified-on-target |
| Latency | cold start about 0.4 s (2 s for the first web page after boot); relaunch 50 to 100 ms to the page | verified-on-target |
| Close | `window.close()` ends the app; no renderer left | verified-on-target |
| Remote keys while it shows | `document.hasFocus()` becomes true after about 2 s; whether keys reach it is **not tested** | open (owner's check) |
| Launch parameters in logs | not found in `/var/log`, `/var/luna`, `/tmp/pmlogdaemon`, the journal, `logread`, `dmesg` | verified-on-target for those places |

## 4. Architecture

```text
HA voice events ──> daemon on_event ──> Overlay (overlay.rs, pure) ──> Frame
                    playback end ─────┘        │ tick (stale run: done after 90 s)
                                               ▼
   power updates ──> screen gate (watch<bool>: Active, no processing; closed while unknown)
                                               │ read right before every launch
                                               ▼
                    Sender task (one luna-send at a time on a pty, latest frame wins,
                                 an open frame again 2.5 s after a run's first)
                                               ▼
        luna-send -n 1 -w 1500 luna://com.webos.applicationManager/launch
            {"id":"<app id>.overlay","params":{"overlay":1,"inst":I,"seq":N,"stage":"…","heard":"…","answer":"…","error":…}}
                                               ▼
                    SAM ──> overlay app (cold: launchParams; running: webOSRelaunch)
                             model.js parse/newer/view ──> overlay.js draws, hides, window.close()
```

- **Frame schema 1:** `overlay` 1; `inst` the daemon's start time in ms, fixed per start (a restarted daemon counts `seq` from 1 again; the page takes a frame of another `inst` as a new run, so a restart's `hide` closes a stale panel at once); `seq` grows by one per shown run (the page drops a frame of an older run of the same start, with the wrap at 2^32; an identical frame sent again changes nothing and does not restart the hold time); `stage` one of six words; `heard` and `answer` cleaned (control characters and runs of white space folded, at most 280 characters, cut at a word with "…"); `error` `no_speech`, `disconnected` or `failed` for `error`, else `null`. The page refuses anything else (strict parse, texts up to 400 characters).
- **Daemon:** `Overlay` gets every `AppEvent` before the daemon's own handling, the end of a TTS playback (`on_done` for `Kind::Tts`, `StopPlayback`) and the 1 s tick (a run with no news for 90 s is closed, but never while its reply still plays); `publish_overlay` hands a changed frame to the `Sender`. The sender uses the power watch's `luna-send`, on a pseudo-terminal (SAM may print nothing over a pipe), with a 1.5 s wait; no reply is not an error. Without the power watch, the app directory or the overlay app the overlay is unavailable: the log says so once, HA's switch has no state (unknown) and `status --json` has `overlay_available: false`.
- **App id:** the main app's directory name (the `app-dir` file `ctl.sh install` writes) plus `.overlay`, the same rule `ctl.sh` uses; `[overlay] app_id` overrides it.
- **App:** `packaging/webos/overlay/` (`index.html`, `model.js`, `overlay.js`, `overlay.css`), ES5 like the TV app. `index.html` starts a 4 s fallback timer that closes the window unless `overlay.js` has started (a script that fails to load must not leave a window over the picture).

## 5. Packaging and lifecycle

- `build_ipk.py` builds `<app id>.overlay` as its own single-app ipk (`overlay_package()`) and puts it into the main package as `payload/overlay.ipk`, listed in `SHA256SUMS`; `check` refuses a package without it or with a wrong window type.
- `ctl.sh install` (the app runs it when it opens with an update pending; `autostart.sh` does at boot; by hand) installs the overlay app through appinstalld (`luna-send-pub -w 20000 -i luna://com.webos.appInstallService/dev/install`) **after** the daemon is started again, so it never keeps the voice service down, when the payload's checksum differs from `overlay.sums` in the state directory or the overlay app's directory is gone. The ipk copy and the reply fifo live in a fresh `mktemp -d /tmp/webos-satellite-overlay.XXXXXX` directory (never `TMPDIR` from the environment: the path goes into appinstalld's parameters), removed afterwards. Without `luna-send-pub`, on an appinstalld error or after 20 s the result is a note; the satellite runs without the overlay.
- `ctl.sh uninstall` and `purge` remove it (`dev/remove`). Removing the main app with the Homebrew Channel alone leaves it behind (as it leaves the state directory).
- `status --json` reports `overlay_enabled` (config), `overlay_app` (installed) and `overlay_available` (installed and `luna-send` there); the text status says the same, and the TV app's row says "(unavailable)".
- The app is started per run and closes itself after it: nothing stays resident between runs.

## 6. Settings

| Where | What |
| --- | --- |
| Config | `[overlay] enabled = false` (**default off** until the owner's check), `app_id` (unset: derived from the app directory as `ctl.sh` does; set: checked against LG's id rules, LG prefixes refused) |
| SSH | `ctl.sh config overlay on\|off` (the ADR 0012 transaction: check, cooldown, busy, restart, rollback, history line) |
| TV app | Home, Sound, **On-screen assistant** (fourth row, two presses) |
| Home Assistant | config switch **On-screen assistant** (`overlay_enabled`, `mdi:message-text-outline`) |

## 7. Cost (EXP-024, c3-a, TV load average about 12)

- Memory: the overlay renderer about 36 MB while shown, freed at the close. The TV's WebAppMgr main process grew by about 30 to 50 MB at the first web page after boot and keeps it.
- CPU, all WebAppMgr processes: listening 17 % of one core (bars stepped 5 times a second), static answer 0.7 %; a cold start per run on top. The first version with CSS animations at 60 fps cost about twice as much, so the page has no CSS animation.
- Daemon: one `luna-send` per frame (about 4 to 6 per run plus at most one re-send), none between runs.

## 8. Privacy

- The texts pass through `luna-send`'s command line (visible to local processes for the time of the call, at most the 1.5 s wait), SAM and the web runtime. A direct LS2 call from the daemon would avoid the command line; it is the future fix (ADR 0014), not built. EXP-024 searched the TV's readable logs for markers in cold-launch and relaunch parameters and found none; memory-only buffers were not searched.
- The daemon logs no text; the page logs nothing; nothing is stored.

## 9. Tests

- `crates/satellite/src/overlay.rs`: the machine (wake word run to the end of the reply, runs without the wake word and follow-ups, errors only for shown runs, a reply text without speech, disconnect, mute, the stale-run close, disabled), the frame JSON and its `Debug`, text cleaning.
- `overlay.rs` also: the screen gate (Active only, no `processing`, unknown is closed), SAM replies read without their body, which stages are sent again, the app id derived like `ctl.sh`, a long reply not closed by the stale limit.
- `daemon.rs` `overlay_frames_reach_the_app_only_while_the_screen_is_on`: a fake `luna-send` (through the pty) receives the launches in order with the texts and the instance id; nothing before the power state is known; "apaga la tele": with `Request Power Off` in progress neither the re-send nor the reply nor the run's end launch anything; nothing on `Screen Off`.
- `config.rs` (`[overlay]` defaults, summary, `app_id` validation), `config_switch.rs` (`overlay_enabled` id, arguments, entity, initial state).
- `packaging/webos/tests/overlay.js`: model (parse with the instance id, order across a restart, identical frames, texts, hold times, language) and the page in a fake runtime (launch and relaunch frames, input region with the string fallback, stepped bars and dots, auto-hide and close, a frame during the fade, a re-sent frame does not extend the hold, a restart's hide closes a stale panel, a key closes, safety limit, scaling), the `index.html` fallback timer, and source rules (ES5, no sound, network, storage or log, no CSS animation).
- `tests/app_matrix.js` (the Sound row, unavailable), `tests/tv_settings.sh` (`config overlay`, default off, status fields), `tests/ipk.sh` (the overlay ipk installed through a fake appinstalld on install, update after the daemon is running again, no-op install, re-install when removed, the `mktemp -d` copy under `/tmp` also with `TMPDIR` set, the 20 s wait, missing `luna-send-pub`, removal on uninstall).

## 10. The owner's visual check

After the release on c3-a (and later c3-b), with the TV on and something playing. The overlay is **off by default**: first turn it on (app Home, Sound, **On-screen assistant**, two presses; or the HA switch **On-screen assistant**).

1. Say the wake word over a video (HDMI from the PC, then an app such as YouTube, then the home screen): within about half a second a dark panel appears at the bottom centre with moving bars and **Te escucho…**. The rest of the picture stays visible and the video keeps playing.
2. Stop talking: **Pensando…** with your words in «».
3. When the reply speaks: its text in the panel. About 5 s after the reply ends the panel slides away. No extra sound at any point.
4. **Remote while the panel shows:** press an arrow, OK and Back meant for the app below. The app should react. If instead the panel vanishes and the key did nothing, the overlay had the remote: please report it (one press is lost each time). Volume keys should work as usual.
5. Mumble something: **No te he entendido** for about 4 s.
6. A follow-up question (when HA asks one) and a mic-button request (if that is on) show the panel too.
7. Turn the TV off with the remote while "Keep listening" is on, then talk: the screen must stay dark (no panel).
8. App Home, Sound, **On-screen assistant**: two presses turn it off (the voice service restarts); the next request shows nothing. Same with the HA switch **On-screen assistant**. If item 4 passed, tell the coordinator, so a later release can turn it on by default.
9. The panel must never stay on screen. If it ever does, hold Back for a few seconds and report it.

## 11. Open questions

- Whether remote keys reach the overlay while it shows (section 10 item 4); if so, a follow-up experiment for a documented way to give the focus back.
- The look over native apps (Netflix, YouTube), over the home screen and on HDMI with HDR content; webosbrew's appinfo guide says an overlay app "is automatically closed when opening home screen" (`documented` by webosbrew, not checked here).
- c3-b and other firmware: the window type and the package registration.
- A real input level for the bars: it would need a frame per step (a `luna-send` each), which costs more than it shows.
