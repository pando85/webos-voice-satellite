# Roadmap

**Status: revised 2026-11-01** after device discovery on c3-a (LG C3) and the owner decisions recorded in [ADR 0001-0004](../decisions/README.md). This roadmap replaces the earlier phase model (Phase 0 research, Phase 1 capture proof of concept, Phase 2 voice pipeline, Phase 3 playback and acoustic quality, Phase 4 productization), which was written before any device evidence existed and is **superseded**. The old text is not kept; use git history if needed.

Research tasks and implementation work are tracked in GitHub Issues. This roadmap is not a second issue tracker.

## Current focus (owner, 2026-11-03)

- **Always-on operation is the current focus:** listening and playback when the remote powers the TV off or the screen is off, plus autostart. Design: [always-on](../design/always-on.md). Procedure: [D5 plan](../discovery/c3-a/D5-plan.md). Autostart: [autostart design](../design/autostart.md).
- **Audio front-end (echo cancellation) is PARKED** (owner decision, 2026-11-03). Defaults stay off (`[aec] enabled = false`). The status and the best-known config are in [AEC front-end §12](../design/aec-frontend.md#12-tuning-results-and-parked-state) and [P2 AEC tuning](../experiments/P2-aec-tuning.md). Open items when it resumes: TV delay lock and ERLE with a playing stimulus, a false-wake soak over TV content, a JBL talker A/B, a live-person test, STT on the tuned variants, and TV CPU. P2 session 3 is on hold with partial data.

## Ground rules

- Evidence first: each phase exit criterion names the evidence that closes it. Claims are labelled `documented`, `observed-on-other-device`, `hypothesis` or `verified-on-target` as in [AGENTS.md](../../AGENTS.md).
- **Needs-owner** items are actions only the owner can authorize or perform (TV state changes, sound at audible levels, adding devices in the Home Assistant UI, changes in the homelab GitOps repository, reboots, persistence). Agents prepare the procedure and wait.
- Device safety rules in [AGENTS.md](../../AGENTS.md) and the [discovery safety rules](../discovery/README.md#safety-rules-learned) apply in every phase. Raw probe output and recordings are never committed.
- Fixed inputs from discovery and decisions: Rust ([ADR 0002](../decisions/0002-rust-implementation.md)), ESPHome native API ([ADR 0001](../decisions/0001-esphome-native-api.md)), server-side wake word for the MVP ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)), capture from `hw:LGSIC,0` with LG hands-free off ([ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md)).
- **Interleaved improvements (owner, 2026-11-02):** the project is public (other rooted-LG-TV owners as well as the owner's TVs) and improvements are interleaved into the phases below instead of waiting until after Phase 5. Priorities: the audio front-end (backlog #1, **parked 2026-11-03**), install and configuration UX (backlog #3), then sharing findings (backlog #8, after Phase 2). Wake word stays `alexa`, with no custom wake word training. See [owner priorities in the backlog](backlog.md#owner-priorities-2026-11-02).
- The ESPHome protocol and Rust ecosystem background is in [esphome-rust-satellite](../research/esphome-rust-satellite.md). Treat its claims as `documented` or `hypothesis` until a Phase 1 experiment verifies them on the target.

## Phase 0: Discovery (largely done)

- **Goal:** know enough about c3-a to choose a runtime, a capture path and an integration route.
- **Scope:** read-only and consented observations on c3-a: runtime and ABI (D0, D1), audio stack inventory (D2), microphone identification, capture characterization and an offline Home Assistant (Wyoming) check (D3).
- **Done, with evidence:**
  - D0/D1: [32-bit ARM EABI5 soft-float userland, glibc 2.31, systemd 244, aarch64 kernel](../discovery/c3-a/D0-D1-runtime.md#32-bit-vs-64-bit-userland); Homebrew Channel 0.6.3 startup hook.
  - D2: [audio stack and exclusivity model](../discovery/c3-a/D2-audio-stack.md), including the [DAPM sysfs oops incident](../discovery/c3-a/D2-audio-stack.md#incident-during-this-step-read-first).
  - D3: [built-in mic is `hw:LGSIC,0`](../discovery/c3-a/D3-mic-identification.md), near-raw 2-channel capture, Magic Remote mic is not an ALSA capture, STT word-perfect on TV-mic audio, wake word detected at +12 dB playback (single runs, no rates).
- **Remaining, folded into later phases:**
  - **D4**, behaviour while the TV plays sound (echo in the mic, whether `hw:LGSIC,1` SpeakerFeedback is a usable AEC reference): Phase 2.
  - **D5**, power states (screen off, standby, Always Ready; capture and network behaviour): Phase 3.
  - Wake word model name that the HA pipeline requests: checked in Phase 1.4 ([P1 HA integration](../discovery/c3-a/P1-ha-integration.md)). `alexa_v0.1` never fired; Kiwi now uses `alexa`.
- **Exit criteria:** met for D0 to D3. Residual unknowns are listed in the discovery logs and carried as Phase 2/3 work.
- **Needs-owner:** none open for Phase 0 itself.

## Phase 1: Silent build and integration (no sound played)

- **Goal:** a Rust satellite that appears in Home Assistant as an Assist satellite and carries TV-mic audio through wake word, STT and intent, with reply audio reaching PulseAudio without being audible.
- **Scope:** everything except audible output. The TV does not play sound in this phase except where an explicit, owner-approved step says so. LG hands-free stays off ([ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md)).
- **Sub-phases:**

| ID | Work item | Deliverable | Exit signal |
| --- | --- | --- | --- |
| 1.1 | Rust workspace, cross-compile, CI, TV smoke test from `/tmp` | Cargo workspace; reproducible cross-build for the 32-bit softfp glibc 2.31 target (candidate `armv7-unknown-linux-gnueabi`, **hypothesis**, to be confirmed against the shipped loader `/lib/ld-linux.so.3`); CI job building and running unit tests; a hello-world binary that prints its ABI and exits | The binary runs on c3-a from `/tmp` (lost at reboot), no writes elsewhere, no persistence. Binary inspected and checksummed before copy. Binaries are never committed |
| 1.2 | Capture module | `hw:LGSIC,0` opened with S24_LE, 2 ch, 16 kHz, period 256, buffer 8192 (parameters verified in [D3](../discovery/c3-a/D3-mic-identification.md#setup-and-method)); discard first 250 ms after open; select ch0; S24 to S16 with fixed gain (about +20 to +30 dB with saturation, to be tuned); high-pass about 100 to 150 Hz; mic-busy detection (open failure while LG hands-free holds the PCM) with a clear log line and bounded retry | 16 kHz mono S16 stream with no startup burst; deterministic unit tests on recorded synthetic fixtures; busy state reported, not hidden; capture device chosen by configuration and discovery, not hard-coded as a universal truth |
| 1.3 | ESPHome API server | Frame handling (plaintext and Noise encrypted), hello/connect, device info, entity list, Assist satellite voice-assistant messages; no on-TV audio dependence | A fake HA built with `aioesphomeapi` connects, completes the Noise handshake, reads device info and exchanges voice-assistant messages with the module in a test harness |
| 1.4 | Satellite on a PC with file mic input and reply-to-file, against the real HA | The satellite binary built for the host, reading a WAV as the "microphone" and writing the reply audio to a file | The owner adds the device in the HA UI; an Assist satellite entity exists; wake word, STT and intent complete from file audio; the wake model name requested by the pipeline is recorded |
| 1.5 | Satellite on the TV, live mic, no wake word on the TV | Same binary cross-built; pipeline started by a manual trigger (push-to-talk style, for example a command or button entity), live capture from 1.2; reply discarded or written to `/tmp`, no sound | A spoken phrase reaches HA and is transcribed and answered; CPU, memory and bandwidth sampled. TV stays silent; no LG hands-free |
| 1.6 | Playback module | TTS download, decode and resample to the sink format, write to PulseAudio (candidate route: system PulseAudio default sink, `card0 pcm6p`, S16_LE 2 ch 44.1 kHz, [hypothesis](../discovery/c3-a/D3-mic-identification.md#conclusions-from-step-5a)) | Verified with the TV muted or the sink volume at zero and with PulseAudio stream statistics (`pactl` stream and sink-input counters): the stream is consumed, underruns are counted, nothing audible |
| 1.7 | Ops basics | Config file format and loader, logs that never contain audio or secrets, start, stop and uninstall scripts that leave no residue | Uninstall returns the TV to the pre-install state (checked against a before/after file listing); logs reviewed for leaks |

- **Status (2026-11-02):**

| ID | Status | Evidence / what remains |
| --- | --- | --- |
| 1.1 | done | [P1 runtime smoke](../discovery/c3-a/P1-runtime-smoke.md) |
| 1.2 | done | [P1 capture validation](../discovery/c3-a/P1-capture-validation.md) |
| 1.3 | done | [ESPHome API crate](../development/esphome-api.md) |
| 1.4 | done | The permanent ESPHome entry `lg-c3-a` was added on 2026-11-02, with HA connecting directly to the TV (owner's network rule). Device, `assist_satellite` and the other entities were verified, along with a live push-to-talk run and a wake word → STT → built-in intent → TTS run streamed by HA and played silently. Earlier: relay and bridge verification, and the wake word id fix (`alexa_v0.1` → `alexa`). The entry shows `unavailable` while the satellite is not running, until autostart is approved (Phase 3). Announcements work since HA's internal URL was set to the LAN ingress (verified silently, with and without the chime). Open on the owner's side: a working conversation agent for Kiwi. See [P1 HA integration](../discovery/c3-a/P1-ha-integration.md#permanent-entry-direct-connection-2026-11-02-morning) |
| 1.5 | done except the live human | On c3-a from `/tmp`, silent: live-mic push-to-talk through HA's real integration (4 consecutive clean runs) and through the bridge; the full chain from TV-mic audio, with the reply in PulseAudio at gain 0; an 11-minute stability run; CPU, memory and bandwidth sampled. **Remaining:** a phrase spoken by a live person (Phase 2: no sound may be played and nobody spoke); autostart (Phase 3). See [P1 TV live](../discovery/c3-a/P1-tv-live.md) |
| 1.6 | done | [P1 playback silent validation](../discovery/c3-a/P1-playback-silent-validation.md); streamed TTS to `pacat` at gain 0 verified in [P1 TV live](../discovery/c3-a/P1-tv-live.md) |
| 1.7 | done | Config loader with strict validation and warnings; logging policy with a leak review (one real leak path fixed: debug `AppEvent` dumps); `packaging/webos/` install, start, stop, status and uninstall scripts. On c3-a under `/tmp/webos-satellite`: the before/after `/tmp` listing, sink-inputs, listening sockets and persistent paths are identical after uninstall. See [P1 ops](../discovery/c3-a/P1-ops.md). Autostart is not part of this step (Phase 3) |

- **Phase exit criteria:** an HA satellite entity exists for c3-a; wake word, STT and intent work end to end from **TV-mic** audio (not only PC audio); reply audio reaches PulseAudio silently; no persistent change on the TV.
  - Exit status (2026-11-02): **met.** The permanent HA satellite entity for c3-a exists, with a direct connection. Wake word, STT and intent work end to end from TV-mic audio (intent with the built-in agent; the owner's Kiwi agent is broken on HA's side). Reply audio reaches PulseAudio silently. No persistent change on the TV. Remaining for Phase 2: a phrase spoken by a live person, and audible replies.
- **Needs-owner:**
  - Authorize running a copied binary from `/tmp` on c3-a (1.1, 1.5), and each TV session.
  - Add the ESPHome device in the HA UI and supply the encryption key via a private channel (1.4). Done 2026-11-02: the owner's network rule lets HA reach the TV on TCP 6053, and the entry `lg-c3-a` was added through the config flow ([P1 HA integration](../discovery/c3-a/P1-ha-integration.md)).
  - Restore a working conversation agent for the Kiwi pipeline (Ollama model `qwen2.5:3b` missing; Kiwi Test's `conversation.chatgpt` does not exist).
  - Keep LG hands-free off; confirm TV state before each live test.
  - Approve the TV-muted playback test (1.6) and how mute is achieved.
  - ~~Check the pipeline's wake word model name~~: done in 1.4. Kiwi used `alexa_v0.1`, which never fires; it is now `alexa`. Kiwi Test still has `alexa_v0.1`.

## Phase 2: Sound tests

- **Goal:** audible replies and an evidence-based view of acoustic behaviour with the TV producing sound.
- **Scope:** audible TTS, volume and ducking, D4 and echo behaviour, AEC evaluation using `hw:LGSIC,1` SpeakerFeedback as a reference (session 1 found it unusable as a linear reference; `hw:LGSIC,3` SoundEngineCapture is the candidate), wake accuracy with real voices and rig stimuli, false triggers from TV content, barge-in.
- **Deliverables:**
  - Audible reply path, with a volume policy and measured relationship between sink volume and heard level.
  - D4 results: mic signal while the TV plays content; whether SpeakerFeedback carries a usable reference, its delay and coherence relative to the mic (see [EXP-006](../experiments/EXP-006-aec-reference.md)).
  - A test corpus and metrics definition (consented voices, rig stimuli) with measured wake detection rates and false triggers per hour with TV content, replacing the single-run results in [D3](../discovery/c3-a/D3-mic-identification.md).
  - Wake accuracy at the owner-approved reference level (+12 dB playback gain on the JBL Go 3 at system volume 82 %), plus real voices at normal distance.
  - Barge-in behaviour: what happens when the user speaks during a reply.
  - AEC evaluation report (baseline without AEC versus candidates); any AEC integration decision goes into an ADR.
- **Status (2026-11-02, sessions 1 and 2 of the unattended sound tests):**

| Deliverable | Status | Evidence / what remains |
| --- | --- | --- |
| Audible reply path and volume relationship | done for the TV speaker | `pacat` tones and noise plus HA Piper announcements through the satellite at master volumes 5 to 25, with mic and SpeakerFeedback levels per step (+14, +9, +4, +2.5 dB per 5-step); an HA end-to-end reply at volume 10; no stream-based ducking. `sink.latency_msec` now defaults to 200: replies start 85 to 332 ms after the TTS stream (n = 6) instead of 1.6 to 1.8 s (n = 4), no underruns ([session 2](../discovery/c3-a/P2-session2-wake-accuracy.md#0-reply-start-latency)). The TV levels its program audio (20 dB in, about 4 dB out at volume 10; AI Sound Pro is the likely cause). **Remaining:** a reply-loudness policy with a listener, outputs other than the TV speaker. See [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md) |
| D4: mic while the TV plays, the SpeakerFeedback reference | done for test signals; content only at volume 6 | SpeakerFeedback: linked with the mic, 1.4 ms ahead, no drift, carries HDMI and `pacat`, post-volume, but **unusable as a linear AEC reference** (coherence 0.5 to 0.6, 3 to 7 dB ceiling). SoundEngineCapture (`hw:LGSIC,3`) is clean (oracle-level ERLE, no drift) with a per-open delay: the candidate reference. Content at volume 6 reaches the mic only 3 to 5 dB above its floor |
| Corpus and metrics | defined | [P2 corpus and metrics](../experiments/P2-corpus.md); generators committed, output private |
| Wake accuracy, false triggers with content | partly done; **rig accuracy blocked** | Session 2: the JBL rig was unreachable, so no talker-in-the-room rate exists. Digital baseline (corpus straight into HA): wake 58/97 = 60 % [50, 69], strongly voice-dependent; near-misses 3/14; WER 15.6 % (11.9 % with a number-format fix); intent 23/40 given the wake word. Semi-synthetic (corpus at the rig level mixed into TV-mic recordings): see [session 2](../discovery/c3-a/P2-session2-wake-accuracy.md#13-semi-synthetic-conditions-no-talker-in-the-room). **False wakes, live through HA:** 0 in 1.14 h of content at volume 10 (≤ 2.6/h), 2 in 1.125 h at volume 15 (1.8/h [0.2, 6.4]). The TV speaker is not a usable talker stand-in. **Remaining:** rig runs A/B/C with `p2_runner.py` (ready), real voices |
| Barge-in | behaviour verified; no barge-in possible | The satellite does not stream the mic during a reply or announcement (7/7 trials); the wake word re-arms 70 to 80 ms after the reply ends. Semi-synthetic: a reference-level "alexa" over the reply would not be detected without AEC (0/41 at volume 10 and 15); at volume 15 the reply clips 0.23 % of the samples after +24 dB. Design options in [session 2](../discovery/c3-a/P2-session2-wake-accuracy.md#3-barge-in); not implemented |
| AEC evaluation and go/no-go | **PARKED (owner, 2026-11-03)**; offline tuning done, TV work open | Baseline numbers (4096-tap NLMS): SpeakerFeedback 3 to 4 dB, SoundEngineCapture 14 dB in 200 to 4000 Hz. Session 2 recorded mic + SoundEngineCapture sets (quiet, 10 min content at volume 10 and 15, replies) with a manifest; offline openWakeWord finds no detection in them. With stereo content the summed reference reaches coherence as low as 0.39 (a two-channel reference looks necessary, **hypothesis**). No corpus speech over content (rig blocked). The canceller exists, off by default ([AEC front-end](../design/aec-frontend.md)). On c3-a (2026-11-02, [P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md)): 11 % of one core for the stage on the armv7 build (13 % for the daemon in wake mode, 2.8 % with `bypass_when_silent`; aarch64 build four times cheaper), the reference PCM linked and released with the mic, 0 xruns in 10 minutes; delay lock and ERLE on the TV still need a stimulus, and the live A/B is open. Offline tuning (2026-11-03, [P2 AEC tuning](../experiments/P2-aec-tuning.md), [§12](../design/aec-frontend.md#12-tuning-results-and-parked-state)): the best-known config keeps a quiet talker better than the default; no go decision, defaults stay off |

- **Exit criteria:** audible reply works with documented volume limits; wake and STT metrics with and without TV content are recorded with sample counts; a go or no-go on AEC for the MVP, with the SpeakerFeedback reference classified as usable, unusable or unknown. Negative or inconclusive results are recorded explicitly.
- **Needs-owner:**
  - Explicit consent for each audible test and for any playback by the TV. Start low and raise in steps; never play TV content or replies unannounced.
  - Consent from anyone whose voice is recorded; recordings are short, local and not committed.
  - Test sessions at the agreed reference level with the JBL Go 3 and the room set up.
  - Decision on which TV content types to test (HDMI, apps).

## Phase 3: Lifecycle and robustness

- **Goal:** the satellite survives normal TV use and fails safely.
- **Scope:** autostart through the Homebrew Channel startup hook, watchdog, kill switch and rollback, power states (D5), reconnection, resource limits, a firmware update guard.
- **Deliverables:**
  - Autostart design reviewed against the failsafe risk of `run-parts` hooks (see [rooted-tv-improvements item 8](../discovery/c3-a/rooted-tv-improvements.md)), with a documented rollback that does not need a reboot where possible.
  - Watchdog and kill switch (a local file, a Home Assistant switch, and a documented manual stop) that stop capture and release the PCM.
  - D5 results: behaviour of capture, network and the satellite in screen-off, standby and Always Ready states ([EXP-004](../experiments/EXP-004-power-lifecycle.md), [always-on design](../design/always-on.md)), with power measurements if feasible, and an "always on" operating mode with a playback check per state. Power-related settings that turn out to be critical are exposed as HA entities and in the install/status app with their risks.
  - Reconnection behaviour after HA restarts, network loss and TV resume; backoff and state recovery.
  - Resource limits (memory, CPU, file descriptors) given about 750 MB available memory, nearly exhausted CMA and 3 of 4 cores online on c3-a ([constraints](../discovery/c3-a/rooted-tv-improvements.md#constraints-not-changes)).
  - Firmware update guard: detect a firmware change and disable or re-validate the satellite; review the update-blocking gaps ([item 1](../discovery/c3-a/rooted-tv-improvements.md)).
  - Homebrew Channel package and first-run configuration, delivered with autostart (backlog #3, interleaved). The QR and phone configuration is not part of this step until its security design exists.
- **Status (2026-11-02):** the parts that need no sound and no persistent change are done and measured on c3-a ([P3 robustness](../discovery/c3-a/P3-robustness.md)); everything persistent or power-related waits for the owner.

| Item | Status | Evidence / what remains |
| --- | --- | --- |
| Kill switch and mute release the PCM | done | Kill-switch file: PCM `closed` in 30 to 70 ms; HA mute switch: 3 to 18 ms after the mute reached the satellite; HA cannot unmute while the kill switch or firmware guard holds; mute remembered across restarts. Manual stop: `stop.sh`, or `touch DIR/disable` plus `stop.sh` |
| Watchdog | done | Event-loop and capture-thread hangs exit with 70/71 after 30 s (tested on c3-a with `--test-hang`); SIGSTOP/suspend is not counted; RSS ceiling exits with 72; an ALSA read timeout (5 s) closes and reopens a stalled device (code path, a real stall not provoked) |
| Supervisor and crash-loop guard | done (scripts, not installed) | `supervise.sh`: backoff 1 s doubling to 300 s, gives up after 5 exits in 600 s and writes `crashloop` (host test); restart after watchdog exits verified on c3-a |
| Autostart design | draft, **needs owner** | [Autostart design](../design/autostart.md): run-parts/failsafe review, draft hook `packaging/webos/init.d/webos-satellite` (returns at once, starts after 60 s, honours `disable`, `crashloop`, the kill switch and the firmware guard), rollback by removing one file. Not installed; needs approval and one test reboot |
| Homebrew Channel package (3a) | built, **PC only** | `make ipk`: one reproducible ipk with both builds, the scripts, a default config and a status app; state in `/var/lib/webos-satellite` survives updates; update applied on app open or at boot; opt-in boot hook; uninstall keeps or purges data. 42-check lifecycle test plus app smoke test on the host, also under BusyBox ([packaging design](../design/packaging.md)). **Remaining:** the on-TV run on c3-b ([EXP-009](../experiments/EXP-009-ipk-install-update.md)), then the owner's approval for c3-a |
| Firmware guard | done | `/var/run/nyx/os_info.json` `webos_manufacturing_version` vs `safety.validated_firmware`; mismatch keeps the mic off; `--check-firmware` exit codes; verified on c3-a with a matching and a non-matching list. Update blocking (item 1) still needs the owner's decision |
| Resource limits | done | nice 10, `RLIMIT_NOFILE` 256, RSS ceiling 64 MiB; measured 10 to 12 fds, 3 threads, 1.5 to 2 % of one core, RSS about 1 to 2 MB with one rise to 9.5 MB in the soak (then flat). PC investigation: musl `malloc` grows about 1.7 bytes per TTS reply byte with a flat Rust heap; fixed with `dlmalloc` on musl targets and a bounded TTS queue; TV re-soak: 55 streamed replies and 770 reconnects, RSS flat at 976 kB (see P3) |
| Reconnection | partly done | Bridge kill/restart, bridge freeze 60 s, satellite freeze 60/120 s, 20 rapid reconnects and a simulated 180 s partition all recover (re-arm within milliseconds of the new connection); soak with periodic reconnects. **Remaining:** HA's own integration (network block), HA restarts, TV Wi-Fi loss and resume (D5) |
| D5 power states | procedure ready (agent-runnable, scheduled by the orchestrator), **not run** | [D5 plan](../discovery/c3-a/D5-plan.md); [always-on design](../design/always-on.md) (states, recommended mode, gaps). Desk research and read-only inspection on 2026-11-03: Quick Start+ and Always Ready are both on on c3-a; screen off on a running TV is the candidate mode for listening and replies; Suspend freezes the process. Nothing was changed on the TV |
| Multi-day run | not done | Needs a persistent or long-lived install (owner). A 35-minute soak (13 bridge sessions, 65 reconnects, 0 failures) is in P3; the 59-minute re-soak with the `dlmalloc` build is done (RSS flat, see P3); the multi-day run is still open |

- **Exit criteria:** after a TV reboot the satellite returns without owner action; a crash loop cannot trigger LG failsafe mode; the kill switch releases the mic in a measured time; D5 states are documented for c3-a with firmware and state labels; the satellite stays inside the resource limits over a multi-day run.
- **Needs-owner:**
  - Approval for the first persistent change (startup hook). Reboots, power-state changes and TV settings changes for the D5 tests are allowed by the owner (2026-11-03) when a test needs them, with every setting restored; the orchestrator schedules the run.
  - Owner decision on the power options exposed in the UI after D5 ([always-on section 7](../design/always-on.md#7-exposing-power-options-in-the-ui-design)).
  - A decision on update blocking (DNS on the home network is the preferred option in the improvements document).

## Phase 4: Wake word and UX

- **Goal:** a wake word and interaction experience suited to daily use.
- **Scope:** custom wake word model in openWakeWord, a listening and mute indicator, timers, announcements. Wake word stays server-side ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)); on-TV detection is a backlog item.
- **Deliverables:**
  - **Deferred (owner, 2026-11-02: keep `alexa` for now; see [backlog](backlog.md#owner-priorities-2026-11-02)).** Custom wake word model trained and deployed through the owner's GitOps homelab repository. The change is **referenced** from here (link or commit id), not stored in this repository.
  - Listening and microphone-mute indicator that is truthful (tied to the real capture state) and documented, as required by the privacy gate F7.
  - Timers and announcements supported by the ESPHome Assist satellite features that Home Assistant exposes, with the TV-side behaviour verified.
  - **Parked (owner, 2026-11-03):** audio front-end integration in the daemon (AEC with a TV reference tap, after P2 session 1 the SoundEngineCapture tap rather than SpeakerFeedback, and other stages that Phase 2 measurements justify), earlier if Phase 2 shows the reference is usable (backlog #1, interleaved). The integration decision goes into an ADR.
- **Exit criteria:** **deferred (owner, 2026-11-02: keep `alexa` for now; see [backlog](backlog.md#owner-priorities-2026-11-02))**: custom wake word false accepts per hour and missed wakes measured with the Phase 2 corpus and compared with the stock `alexa` model. Remaining criteria: the indicator cannot show "not listening" while capture runs (tested); timers and announcements work audibly.
- **Needs-owner:**
  - Training data and recorded voices (consent), model approval, GitOps repository changes and deployment.
  - Choice of the indicator form (on-screen versus LED or none) and mute policy.

## Phase 5: Second TV and packaging

- **Goal:** the satellite is installable and works on the second C3 without code edits.
- **Scope:** validation on the second C3, runtime capability discovery (no hard-coded device names such as `LGSIC` or card indexes), an `ipk` for the Homebrew Channel, a compatibility record.
- **Deliverables:**
  - Second C3 discovery results filed per device and firmware (a new directory next to [c3-a](../discovery/README.md), plus an entry in [compatibility](../compatibility/README.md)).
  - Capability discovery module: finds the capture PCM, supported formats and the playback sink at runtime, with explicit failure messages when it cannot.
  - Reproducible `ipk` build and install and uninstall flow for the Homebrew Channel, with a package inspection checklist. Status (2026-11-03): built and checked on a PC (`make ipk`, `build_ipk.py check`, [packaging design](../design/packaging.md)); manual install only; TV run planned ([EXP-009](../experiments/EXP-009-ipk-install-update.md)); license decided: GPL-3.0-or-later (owner, 2026-11-03; [packaging P15](../design/packaging.md#8-license-decision-and-dependency-audit)); publication waits for the release prerequisites in backlog #10, including the verified-models list that replaces a hard firmware whitelist ([packaging section 9](../design/packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist)) (app id confirmed and the official Homebrew Channel repository chosen, 2026-11-03; see [backlog #10](backlog.md#10-official-homebrew-channel-listing)).
  - Install and configuration UX for other users: first-run configuration and pairing without typing the encryption key; the QR and phone configuration only after its security design is accepted (backlog #3, interleaved). Pairing status (2026-11-02): implemented, and verified on c3-a against the owner's HA with a test device (pairing, reconnects, reset with re-authentication, mic gate, no key in logs); the window timeout shows `connection_error` in HA instead of the setup-window message (fix proposed) and HA leaves its key behind when the entry is deleted ([P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md#3-pairing-on-the-tv-with-the-owners-ha)).
  - Compatibility record scoped by model, firmware and power state, using [DEVICE-TEMPLATE](../compatibility/DEVICE-TEMPLATE.md).
- **Exit criteria:** a clean install from the `ipk` on the second C3 reaches the Phase 1 exit state; the compatibility record lists what is verified, what is not, and which firmware it applies to; no claim exceeds the evidence.
- **Needs-owner:** access and consent for the second TV (state changes, rooting status, firmware pinning); approval to publish the `ipk`; release policy (the license is decided: GPL-3.0-or-later).

## After Phase 5

Remaining improvements are in the [backlog](backlog.md); the owner priorities of 2026-11-02 are interleaved into the phases above.
