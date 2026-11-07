# ADR 0014: The on-screen assistant overlay as an `overlay`-window app in its own package, fed by launch parameters

- **Status:** **Accepted** (2026-11-07, owner: "Go with the full version": a small transparent panel over whatever plays that shows listening, thinking and the answer). **Off by default** in the first release, until the owner's check on c3-a (whether remote keys reach the overlay is not verified). Implemented on the host (unit, integration and app tests); the window was checked on c3-a with our own probe and overlay packages ([EXP-024](../experiments/EXP-024-assistant-overlay-window.md)); the daemon side has **not run on a TV**. Revised after the independent review of `9fca695` (screen gate, restart instance id, logging, install order, temporary files, default off).
- **Scope:** how the satellite shows its voice runs on screen. Design: [assistant overlay](../design/assistant-overlay.md).
- **Related:** [ADR 0009](0009-phone-setup-web-server.md) (no HTTP in the daemon), [ADR 0012](0012-config-switches-in-ha.md) (config switches), [ADR 0013](0013-remote-mic-button-coexistence.md) (mic button runs), [remote mic button research 3.3](../research/remote-mic-button.md#33-on-screen-feedback), [AGENTS.md](../../AGENTS.md) (privacy: never log speech).

## Context

- The TV app is a full-screen app opened by the owner; during a voice run nothing shows on screen. The owner wants a phone-like voice bar over any app, HDMI input or the home screen.
- Prior art: CleanYourTV draws voice states in a separate web app with a `topui` window (webOS 25) or an `overlay` window (SDK 5 to 9), started with `applicationManager/launch` and fed through the launch parameters, with an empty input region and `window.close()` after use ([research 3.3](../research/remote-mic-button.md#33-on-screen-feedback); `observed-on-other-device`).
- EXP-024 on c3-a (`verified-on-target`, firmware `03.30.74`):
  - `topui` is refused ("Unsupported Application Type"); `overlay` starts, and the app in front stays the foreground app;
  - a second app inside the main package is installed on disk but not registered by SAM (it would need a SAM restart, an LG service); as its own package it is registered at once;
  - launch parameters reach the page, relaunches arrive as `webOSRelaunch` in 50 to 100 ms, a cold start takes about 0.4 s (2 s for the first web page after boot);
  - markers in cold-launch and relaunch parameters were not found in the TV's readable logs;
  - the page's `document.hasFocus()` becomes true after about 2 s; whether remote keys reach it is not known.
- The daemon must not parse HTTP (ADR 0009), and polling `ctl.sh` through the Homebrew Channel costs about 80 ms of CPU per call (EXP-012), too much for live states.

## Decision

1. **A second web app, `<app id>.overlay`**, with `"defaultWindowType":"overlay"`, `"transparent":true`, no splash, no spinner, `"visible":false`, `"class":{"hidden":true}`. Its page draws one panel at the bottom centre, sets an empty input region and an empty key mask at load, closes itself on the first key it receives, plays no sound, makes no network or storage call and logs nothing.
2. **Shipped inside the main package as `payload/overlay.ipk`** (built by `build_ipk.py`, covered by `SHA256SUMS` and the package check) and **installed by `ctl.sh install` through appinstalld** (`dev/install`, the Homebrew Channel path) when its checksum differs from the one installed last or its directory is gone; removed by `ctl.sh uninstall` and `purge` (`dev/remove`). A failure is a note; the satellite runs without it.
3. **The daemon feeds it through SAM**: a pure state machine (`overlay.rs`) turns the voice events into frames (`listening`, `thinking`, `answer`, `error`, `done`, `hide`, an instance id fixed per daemon start, a run sequence number, the heard text and the reply text, at most 280 characters each); one task sends the latest frame with `luna-send -n 1 -w 1500 luna://com.webos.applicationManager/launch {"id":…,"params":frame}` on a pseudo-terminal, one call at a time, overtaken frames skipped, and an open frame (`listening`, `thinking`, `answer`) once more 2.5 s after a run's first frame (never a `done`, `error` or `hide`). No reply within the wait is not an error; a failure is logged with SAM's `errorCode` and `errorText` only. The page hides itself 5 s after `done` (4 s after `error`) and closes the window; a frame sent again does not restart its hold time; a frame of another instance (a restarted daemon) is a new run; a fallback timer in `index.html` closes the window when the scripts do not start.
4. **Which runs:** wake word runs from the wake word, runs without the wake word (mic button, follow-up turns, the conversation after an announcement) from their start. The idle wake word stage and its errors never show.
5. **Only while the TV shows a picture**: a screen gate (a `watch` the power updates set) is open only for the settled state `Active` with no `processing` (power-off or suspend in progress), and closed while the state is not known. The sender reads it right before **every** launch, the re-send included, so a frame queued behind a slow call or sent during "apaga la tele" never launches the app on a dark screen. The overlay needs the power watch.
6. **Setting:** `[overlay] enabled`, **off by default** until the owner's check (then a later release may turn it on), changed with `ctl.sh config overlay on|off`, the TV app's Sound row "On-screen assistant" and the HA config switch `overlay_enabled` "On-screen assistant", the ADR 0012 pattern (guards, restart, rollback).
7. **Privacy:** the texts exist in the daemon only to be shown; no log line holds them (frame `Debug` prints lengths; the daemon's debug lines for the transcript, the reply, the announcement text and HA's error message now print lengths only, as does the voice layer). They pass through `luna-send`'s command line (for up to the 1.5 s wait) and SAM to the page. **Future fix:** a direct LS2 (luna-service2) call from the daemon would keep the texts off any command line; it needs the TV's `libluna-service2` from a static binary and is not built.

## Consequences

- With the setting on, every run shows on screen. Cost on c3-a (EXP-024): a renderer of about 36 MB while the panel shows, freed at the close; about 17 % of one core while listening (bars stepped 5 times a second; CSS animation at 60 fps cost about twice that), under 1 % with a static answer, plus a cold start per run.
- One more app id on the TV, hidden from the launcher; one more install step in `ctl.sh install`, after the daemon is started again (a luna subscription to appinstalld, at most 20 s, in a fresh `mktemp -d` directory under `/tmp`). HA's switch has no state (unknown) and `status --json` says `overlay_available: false` when `luna-send` or the overlay app is missing.
- Removing the main app with the Homebrew Channel without `ctl.sh uninstall` leaves the overlay app behind (as it already leaves the state directory).
- The transcript and the reply are visible, for the time of a call (at most the 1.5 s wait), in `luna-send`'s command line to any local process that lists processes; EXP-024 found them in no readable log, but memory-only buffers were not searched. Residual, documented; the LS2 transport above is the fix.
- **Unknown until the owner's check:** whether remote keys reach the overlay while it shows (then one press is lost, because the page closes on it), and how it looks over video, HDMI and native apps.

## Alternatives considered

- **`topui` window:** refused by SAM on webOS 8.3 (EXP-024).
- **The overlay as a second app inside the main package:** not registered without a SAM restart (EXP-024); restarting SAM blinks the screen and touches an LG service.
- **WebSocket or HTTP on loopback in the daemon:** against ADR 0009 (no HTTP parser next to the microphone and the key); a second server binary for this is more code and more exposure than SAM's launch parameters.
- **The page polls `ctl.sh` through the Homebrew Channel:** about 80 ms of CPU per call (EXP-012) and up to a second of latency; the overlay would need a call every few hundred milliseconds.
- **The page reads a state file with `XMLHttpRequest`:** web apps run in a jail; reading outside the app directory is not established, and inside it the file would sit on the flash.
- **A resident hidden window (no close between runs):** faster first frame, but CleanYourTV reports that a hidden overlay window keeps the remote on older webOS; a cold start is about 0.4 s on c3-a.
- **LG's toast (`com.webos.notification/createToast`):** documented for webOS OSE only, text-only, not verified on these TVs; no listening state.
- **A native surface:** no documented API for a homebrew process; far more invasive.

## Revisit when

- The owner's check shows that remote keys go to the overlay, or the panel misbehaves over some content.
- A firmware update changes SAM's handling of window types or of extra apps in a package.
- Another TV (c3-b, other firmware) refuses the `overlay` type.
