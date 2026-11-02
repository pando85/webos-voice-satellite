# Roadmap

**Status: revised 2026-11-01** after device discovery on c3-a (LG C3) and the owner decisions recorded in [ADR 0001-0004](../decisions/README.md). This roadmap replaces the earlier phase model (Phase 0 research, Phase 1 capture proof of concept, Phase 2 voice pipeline, Phase 3 playback and acoustic quality, Phase 4 productization), which was written before any device evidence existed and is **superseded**. The old text is not kept; use git history if needed.

Research tasks and implementation work are tracked in GitHub Issues. This roadmap is not a second issue tracker.

## Ground rules

- Evidence first: each phase exit criterion names the evidence that closes it. Claims are labelled `documented`, `observed-on-other-device`, `hypothesis` or `verified-on-target` as in [AGENTS.md](../../AGENTS.md).
- **Needs-owner** items are actions only the owner can authorize or perform (TV state changes, sound at audible levels, adding devices in the Home Assistant UI, changes in the homelab GitOps repository, reboots, persistence). Agents prepare the procedure and wait.
- Device safety rules in [AGENTS.md](../../AGENTS.md) and the [discovery safety rules](../discovery/README.md#safety-rules-learned) apply in every phase. Raw probe output and recordings are never committed.
- Fixed inputs from discovery and decisions: Rust ([ADR 0002](../decisions/0002-rust-implementation.md)), ESPHome native API ([ADR 0001](../decisions/0001-esphome-native-api.md)), server-side wake word for the MVP ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)), capture from `hw:LGSIC,0` with LG hands-free off ([ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md)).
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
| 1.4 | partially done | The satellite binary works with TV-mic audio against the owner's real HA pipeline: wake word, STT and a streamed TTS reply. The wake word id is recorded and Kiwi was fixed (`alexa_v0.1` → `alexa`). These runs went through a bridge, because **HA cannot open connections into the LAN** where the TV and the PC are. **Remaining (needs owner):** a router rule (or network move) so HA reaches TCP 6053 on the TV, then the config flow and the `assist_satellite` entity; and a working conversation agent for Kiwi (the Ollama model is missing). See [P1 HA integration](../discovery/c3-a/P1-ha-integration.md) |
| 1.5 | partially done | The same binary ran on c3-a from `/tmp`, silent: the live mic streamed to HA's pipeline via push-to-talk; the full wake → STT → reply chain ran on the TV from TV-mic audio, with the reply in PulseAudio at gain 0; a 10+ minute stability run; CPU, memory and bandwidth sampled. **Remaining:** HA's own connection to the TV (same network block as 1.4), and "a spoken phrase from a live human" (Phase 2, since no sound may be played and nobody spoke). See [P1 TV live](../discovery/c3-a/P1-tv-live.md) |
| 1.6 | done | [P1 playback silent validation](../discovery/c3-a/P1-playback-silent-validation.md); streamed TTS to `pacat` at gain 0 verified in [P1 TV live](../discovery/c3-a/P1-tv-live.md) |
| 1.7 | partially done | Config file format and loader, and logs without audio or secrets, exist in `webos-satellite`. Start, stop and uninstall scripts are not written yet |

- **Phase exit criteria:** an HA satellite entity exists for c3-a; wake word, STT and intent work end to end from **TV-mic** audio (not only PC audio); reply audio reaches PulseAudio silently; no persistent change on the TV.
- **Needs-owner:**
  - Authorize running a copied binary from `/tmp` on c3-a (1.1, 1.5), and each TV session.
  - Add the ESPHome device in the HA UI and supply the encryption key via a private channel (1.4). **First, allow HA to reach the TV on TCP 6053**: HA's cluster network currently gets a TCP reset for every host on the LAN ([P1 HA integration](../discovery/c3-a/P1-ha-integration.md)).
  - Restore a working conversation agent for the Kiwi pipeline (Ollama model `qwen2.5:3b` missing; Kiwi Test's `conversation.chatgpt` does not exist).
  - Keep LG hands-free off; confirm TV state before each live test.
  - Approve the TV-muted playback test (1.6) and how mute is achieved.
  - ~~Check the pipeline's wake word model name~~: done in 1.4. Kiwi used `alexa_v0.1`, which never fires; it is now `alexa`. Kiwi Test still has `alexa_v0.1`.

## Phase 2: Sound tests

- **Goal:** audible replies and an evidence-based view of acoustic behaviour with the TV producing sound.
- **Scope:** audible TTS, volume and ducking, D4 and echo behaviour, AEC evaluation using `hw:LGSIC,1` SpeakerFeedback as a reference, wake accuracy with real voices and rig stimuli, false triggers from TV content, barge-in.
- **Deliverables:**
  - Audible reply path, with a volume policy and measured relationship between sink volume and heard level.
  - D4 results: mic signal while the TV plays content; whether SpeakerFeedback carries a usable reference, its delay and coherence relative to the mic (see [EXP-006](../experiments/EXP-006-aec-reference.md)).
  - A test corpus and metrics definition (consented voices, rig stimuli) with measured wake detection rates and false triggers per hour with TV content, replacing the single-run results in [D3](../discovery/c3-a/D3-mic-identification.md).
  - Wake accuracy at the owner-approved reference level (+12 dB playback gain on the JBL Go 3 at system volume 82 %), plus real voices at normal distance.
  - Barge-in behaviour: what happens when the user speaks during a reply.
  - AEC evaluation report (baseline without AEC versus candidates); any AEC integration decision goes into an ADR.
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
  - D5 results: behaviour of capture, network and the satellite in screen-off, standby and Always Ready states ([EXP-004](../experiments/EXP-004-power-lifecycle.md)), with power measurements if feasible.
  - Reconnection behaviour after HA restarts, network loss and TV resume; backoff and state recovery.
  - Resource limits (memory, CPU, file descriptors) given about 750 MB available memory, nearly exhausted CMA and 3 of 4 cores online on c3-a ([constraints](../discovery/c3-a/rooted-tv-improvements.md#constraints-not-changes)).
  - Firmware update guard: detect a firmware change and disable or re-validate the satellite; review the update-blocking gaps ([item 1](../discovery/c3-a/rooted-tv-improvements.md)).
- **Exit criteria:** after a TV reboot the satellite returns without owner action; a crash loop cannot trigger LG failsafe mode; the kill switch releases the mic in a measured time; D5 states are documented for c3-a with firmware and state labels; the satellite stays inside the resource limits over a multi-day run.
- **Needs-owner:**
  - Approval for the first persistent change (startup hook), and for any reboot.
  - Approval of screen-off and standby test procedures.
  - A decision on update blocking (DNS on the home network is the preferred option in the improvements document).

## Phase 4: Wake word and UX

- **Goal:** a wake word and interaction experience suited to daily use.
- **Scope:** custom wake word model in openWakeWord, a listening and mute indicator, timers, announcements. Wake word stays server-side ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)); on-TV detection is a backlog item.
- **Deliverables:**
  - Custom wake word model trained and deployed through the owner's GitOps homelab repository. The change is **referenced** from here (link or commit id), not stored in this repository.
  - Listening and microphone-mute indicator that is truthful (tied to the real capture state) and documented, as required by the privacy gate F7.
  - Timers and announcements supported by the ESPHome Assist satellite features that Home Assistant exposes, with the TV-side behaviour verified.
- **Exit criteria:** custom wake word false accepts per hour and missed wakes measured with the Phase 2 corpus and compared with the stock `alexa` model; the indicator cannot show "not listening" while capture runs (tested); timers and announcements work audibly.
- **Needs-owner:**
  - Training data and recorded voices (consent), model approval, GitOps repository changes and deployment.
  - Choice of the indicator form (on-screen versus LED or none) and mute policy.

## Phase 5: Second TV and packaging

- **Goal:** the satellite is installable and works on the second C3 without code edits.
- **Scope:** validation on the second C3, runtime capability discovery (no hard-coded device names such as `LGSIC` or card indexes), an `ipk` for the Homebrew Channel, a compatibility record.
- **Deliverables:**
  - Second C3 discovery results filed per device and firmware (a new directory next to [c3-a](../discovery/README.md), plus an entry in [compatibility](../compatibility/README.md)).
  - Capability discovery module: finds the capture PCM, supported formats and the playback sink at runtime, with explicit failure messages when it cannot.
  - Reproducible `ipk` build and install and uninstall flow for the Homebrew Channel, with a package inspection checklist.
  - Compatibility record scoped by model, firmware and power state, using [DEVICE-TEMPLATE](../compatibility/DEVICE-TEMPLATE.md).
- **Exit criteria:** a clean install from the `ipk` on the second C3 reaches the Phase 1 exit state; the compatibility record lists what is verified, what is not, and which firmware it applies to; no claim exceeds the evidence.
- **Needs-owner:** access and consent for the second TV (state changes, rooting status, firmware pinning); approval to publish the `ipk`; decision on the repository license and release policy.

## After Phase 5

Unprioritized improvements, to be reprioritized by the owner, are in the [backlog](backlog.md).
