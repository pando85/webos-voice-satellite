# On-screen assistant overlay

- **Status:** built on the host (daemon, overlay app, packaging, TV app row, HA switch; tests below). The window approach is checked on c3-a with our own probe and overlay packages ([EXP-024](../experiments/EXP-024-assistant-overlay-window.md)); the daemon side has **not run on a TV**, and the look is the owner's check (section 10). Decision record: [ADR 0014](../decisions/0014-assistant-overlay.md) (proposed).
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
- **Never wake a dark screen:** the daemon sends no frame unless the TV's power state is `Active` (or not known yet). In `Screen Off` (Keep listening), `Active Standby` or during suspend nothing is launched.
- **Never log speech:** the heard and reply texts exist in the daemon only to be shown; log lines name the stage and the run number (`Frame`'s `Debug` prints lengths).
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
                    power state Active? ──> Sender task (one luna-send at a time, latest frame wins,
                                               │  the latest frame again 2.5 s after a run's first)
                                               ▼
        luna-send -n 1 -w 3000 luna://com.webos.applicationManager/launch
            {"id":"<app id>.overlay","params":{"overlay":1,"seq":N,"stage":"…","heard":"…","answer":"…","error":…}}
                                               ▼
                    SAM ──> overlay app (cold: launchParams; running: webOSRelaunch)
                             model.js parse/newer/view ──> overlay.js draws, hides, window.close()
```

- **Frame schema 1:** `overlay` 1; `seq` grows by one per shown run (the page drops a frame of an older run, with the wrap at 2^32); `stage` one of six words; `heard` and `answer` cleaned (control characters and runs of white space folded, at most 280 characters, cut at a word with "…"); `error` `no_speech`, `disconnected` or `failed` for `error`, else `null`. The page refuses anything else (strict parse, texts up to 400 characters).
- **Daemon:** `Overlay` gets every `AppEvent` before the daemon's own handling, the end of a TTS playback (`on_done` for `Kind::Tts`, `StopPlayback`) and the 1 s tick; `publish_overlay` hands a changed frame to the `Sender` when the power state allows. The sender uses the same `luna-send` (`power.luna_send`) as the power watch; without it (a PC) the overlay is off and says so once in the log.
- **App:** `packaging/webos/overlay/` (`index.html`, `model.js`, `overlay.js`, `overlay.css`), ES5 like the TV app.

## 5. Packaging and lifecycle

- `build_ipk.py` builds `<app id>.overlay` as its own single-app ipk (`overlay_package()`) and puts it into the main package as `payload/overlay.ipk`, listed in `SHA256SUMS`; `check` refuses a package without it or with a wrong window type.
- `ctl.sh install` (the app runs it when it opens with an update pending; `autostart.sh` does at boot; by hand) installs the overlay app through appinstalld (`luna-send-pub -i luna://com.webos.appInstallService/dev/install`, a copy of the ipk in `/tmp`) when the payload's checksum differs from `overlay.sums` in the state directory or the overlay app's directory is gone. Without `luna-send-pub` or on an appinstalld error the result is a note; the satellite runs without the overlay.
- `ctl.sh uninstall` and `purge` remove it (`dev/remove`). Removing the main app with the Homebrew Channel alone leaves it behind (as it leaves the state directory).
- `status --json` reports `overlay_enabled` (config) and `overlay_app` (installed); the text status says the same.
- The app is started per run and closes itself after it: nothing stays resident between runs.

## 6. Settings

| Where | What |
| --- | --- |
| Config | `[overlay] enabled = true` (default), `app_id` (the package's overlay app; checked against LG's id rules, LG prefixes refused) |
| SSH | `ctl.sh config overlay on\|off` (the ADR 0012 transaction: check, cooldown, busy, restart, rollback, history line) |
| TV app | Home, Sound, **On-screen assistant** (fourth row, two presses) |
| Home Assistant | config switch **On-screen assistant** (`overlay_enabled`, `mdi:message-text-outline`) |

## 7. Cost (EXP-024, c3-a, TV load average about 12)

- Memory: the overlay renderer about 36 MB while shown, freed at the close. The TV's WebAppMgr main process grew by about 30 to 50 MB at the first web page after boot and keeps it.
- CPU, all WebAppMgr processes: listening 17 % of one core (bars stepped 5 times a second), static answer 0.7 %; a cold start per run on top. The first version with CSS animations at 60 fps cost about twice as much, so the page has no CSS animation.
- Daemon: one `luna-send` per frame (about 4 to 6 per run plus one re-send), none between runs.

## 8. Privacy

- The texts pass through `luna-send`'s command line (visible to local processes for the time of the call), SAM and the web runtime. EXP-024 searched the TV's readable logs for markers in cold-launch and relaunch parameters and found none; memory-only buffers were not searched.
- The daemon logs no text; the page logs nothing; nothing is stored.

## 9. Tests

- `crates/satellite/src/overlay.rs`: the machine (wake word run to the end of the reply, runs without the wake word and follow-ups, errors only for shown runs, a reply text without speech, disconnect, mute, the stale-run close, disabled), the frame JSON and its `Debug`, text cleaning.
- `daemon.rs` `overlay_frames_reach_the_app_through_luna_send`: a fake `luna-send` receives the launches in order with the texts; nothing of a run while the screen is off.
- `config.rs` (`[overlay]` defaults, summary, `app_id` validation), `config_switch.rs` (`overlay_enabled` id, arguments, entity, initial state).
- `packaging/webos/tests/overlay.js`: model (parse, order, texts, hold times, language) and the page in a fake runtime (launch and relaunch frames, input region with the string fallback, stepped bars and dots, auto-hide and close, a frame during the fade, a key closes, safety limit, scaling), and source rules (ES5, no sound, network, storage or log, no CSS animation).
- `tests/app_matrix.js` (the Sound row), `tests/tv_settings.sh` (`config overlay`, status fields), `tests/ipk.sh` (the overlay ipk installed through a fake appinstalld on install, update, no-op install, re-install when removed, missing `luna-send-pub`, removal on uninstall).

## 10. The owner's visual check

After the release on c3-a (and later c3-b), with the TV on and something playing:

1. Say the wake word over a video (HDMI from the PC, then an app such as YouTube, then the home screen): within about half a second a dark panel appears at the bottom centre with moving bars and **Te escucho…**. The rest of the picture stays visible and the video keeps playing.
2. Stop talking: **Pensando…** with your words in «».
3. When the reply speaks: its text in the panel. About 5 s after the reply ends the panel slides away. No extra sound at any point.
4. **Remote while the panel shows:** press an arrow, OK and Back meant for the app below. The app should react. If instead the panel vanishes and the key did nothing, the overlay had the remote: please report it (one press is lost each time). Volume keys should work as usual.
5. Mumble something: **No te he entendido** for about 4 s.
6. A follow-up question (when HA asks one) and a mic-button request (if that is on) show the panel too.
7. Turn the TV off with the remote while "Keep listening" is on, then talk: the screen must stay dark (no panel).
8. App Home, Sound, **On-screen assistant**: two presses turn it off (the voice service restarts); the next request shows nothing. Same with the HA switch **On-screen assistant**. Turn it back on.
9. The panel must never stay on screen. If it ever does, hold Back for a few seconds and report it.

## 11. Open questions

- Whether remote keys reach the overlay while it shows (section 10 item 4); if so, a follow-up experiment for a documented way to give the focus back.
- The look over native apps (Netflix, YouTube), over the home screen and on HDMI with HDR content; webosbrew's appinfo guide says an overlay app "is automatically closed when opening home screen" (`documented` by webosbrew, not checked here).
- c3-b and other firmware: the window type and the package registration.
- A real input level for the bars: it would need a frame per step (a `luna-send` each), which costs more than it shows.
