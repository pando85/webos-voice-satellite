# Backlog: improvements

This backlog holds ideas and options, not commitments, except where the owner priorities below say otherwise. None of it blocks the MVP defined in the [roadmap](roadmap.md). Statements about techniques are `hypothesis` or `documented` until an experiment on the target verifies them. Each item lists why, approach options, dependencies and the evidence needed before it can be accepted.

## Owner priorities (2026-11-02)

Decisions recorded from the owner on 2026-11-02:

- **Public project.** The project is public. Its audience is other owners of rooted LG webOS TVs as well as the owner's own two TVs. Consequences: keep the code and docs general (no hard-coded c3-a specifics, see [AGENTS.md](../../AGENTS.md)), keep a [compatibility table](../compatibility/README.md) scoped by model, firmware and power state, and write docs that other users can follow.
- **Interleaved, not after Phase 5.** Improvements are scheduled into the current phases instead of waiting until the end of Phase 5. The phase exit criteria in the [roadmap](roadmap.md) are unchanged; each landing below adds deliverables.
- **Wake word stays `alexa`** for now. No custom wake word training. This defers the custom model work that Phase 4 used to name.
- **Current focus (owner, 2026-11-03): always-on operation** (listening and playback when the remote powers the TV off or the screen is off, plus autostart). See [always-on](../design/always-on.md) and the [D5 plan](../discovery/c3-a/D5-plan.md). **The audio front-end (item 1) is PARKED** (owner, 2026-11-03).

Ranked priorities and where each lands:

| Rank | Item | Where it lands |
| --- | --- | --- |
| 1 | [#1 Professional audio front-end](#1-professional-audio-front-end) (AEC with a TV reference tap, beamforming, noise suppression, AGC) | **Evaluation in Phase 2**: already in scope through D4, [EXP-006](../experiments/EXP-006-aec-reference.md) and the AEC evaluation report. **Integration into the daemon in Phase 4 (now PARKED, 2026-11-03)**, alongside wake word and UX work, earlier if Phase 2 shows the SpeakerFeedback reference is usable. Any integration decision goes into an ADR |
| 2 | [#3 Install and configuration UX](#3-install-and-configuration-ux) (Homebrew Channel package, phone configuration through a QR code, pairing without typing the key) | **Phase 3** (with autostart): Homebrew Channel package and first-run configuration pulled forward. **Phase 5** (packaging): the `ipk` and its install flow. The QR and phone configuration needs a **security design first** (authentication, TLS, secrets handling) before any implementation |
| 3 | [#8 Share findings with the webOS homebrew community](#8-share-findings-with-the-webos-homebrew-community) | **After Phase 2**, because the project is public. Needs a redaction review first |

Items not in the ranked list stay in the backlog without a priority or a phase: #2, #4, #5, #6, #7 and #9, each marked "not prioritized (2026-11-02)" below. Later items: #10 (follow-up), #11 (design done 2026-11-04, implementation next), #12 (decision pending, owner) and #13 (owner priority 2026-11-04: make the mic work on any firmware; design done, implementation next).

## 1. Professional audio front-end

- **Status:** **PARKED** (owner decision, 2026-11-03). Defaults stay off (`[aec] enabled = false`). The status and best-known config are in [AEC front-end §12](../design/aec-frontend.md#12-tuning-results-and-parked-state) and [P2 AEC tuning](../experiments/P2-aec-tuning.md). Open items: TV delay lock and ERLE, a false-wake soak, a JBL talker A/B, a live-person test, STT on the tuned variants, and TV CPU. Earlier (owner priority 1, 2026-11-02): evaluate in Phase 2, integrate in Phase 4. Echo cancellation with the SoundEngineCapture reference is implemented in the daemon, **off by default**, and measured on real c3-a recordings on the PC only; the TV CPU benchmark and the content A/B are pending ([AEC front-end](../design/aec-frontend.md)).
- **Why:** commercial assistants combine echo cancellation, beamforming, noise suppression and gain control. The built-in mic path on c3-a is near-raw (DC removal plus a fixed +16 dB gain, no AGC or noise suppression; [D3](../discovery/c3-a/D3-mic-identification.md)), so all of that is left to the satellite or to Home Assistant. TV playback in the mic is the main expected quality problem.
- **Approach options:**
  - Acoustic echo cancellation using a TV reference tap: WebRTC AEC3 or speexdsp (cheaper CPU, weaker). P2 session 1 found `hw:LGSIC,1` SpeakerFeedback unusable as a linear reference and `hw:LGSIC,3` SoundEngineCapture clean and drift-free but with a delay that changes per open, so the AEC needs delay estimation at start ([P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md#other-lgsic-taps-mixercapture-soundenginecapture)).
  - Two-microphone beamforming with a spacing of about 6 to 8 cm (inferred, geometry unknown): delay-and-sum, MVDR or GSC. The D3 analysis showed that a naive channel average lowered SNR, so any gain must be measured.
  - Noise suppression: RNNoise, DeepFilterNet, WebRTC NS. Check effects on wake word and STT, since suppression can hurt both.
  - AGC, dereverberation and a tuned VAD.
  - Optional: study LG's own vendor front-end library (`libaec-fe`) with the [REA tool](../tools/rea.md) to learn how it maps the reference and mic, **not to reuse it**. Static analysis only; follow the REA rules in [AGENTS.md](../../AGENTS.md) and do not treat results as device validation.
- **Dependencies:** Phase 2 D4 results (SpeakerFeedback is not usable as a linear reference; SoundEngineCapture is the candidate), the test corpus, CPU and memory budget from Phase 3, a Rust or C library choice under [ADR 0002](../decisions/0002-rust-implementation.md).
- **Evidence needed:** a consented test corpus with TV content playing; metrics for false rejects, false accepts per hour and word error rate (WER) with and without each stage; CPU and latency per stage on the Cortex-A76 cores under the 32-bit userland.

## 2. On-TV wake word

- **Status:** not prioritized (2026-11-02).
- **Why:** lower latency, lower bandwidth and less Home Assistant load; stop continuous audio streaming to HA; support several and custom wake words. Privacy is **not** the motivation, since everything is local ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)).
- **Approach options:** microWakeWord or openWakeWord on the TV (ARM Cortex-A76 cores running 32-bit code; check TFLite or ONNX runtime builds for the soft-float target); a hybrid that runs a light on-TV detector and confirms server-side; wake word through the ESPHome voice-assistant start flow.
- **Dependencies:** Phase 4 custom model work, front-end quality (item 1), runtime build feasibility for the 32-bit softfp target, resource limits from Phase 3.
- **Evidence needed:** CPU and memory use on c3-a, detection rate and false accepts per hour compared with the server-side baseline on the same corpus, and behaviour when the TV is playing content.

## 3. Install and configuration UX

- **Status:** owner priority 2 (2026-11-02): Homebrew Channel package and first-run configuration in Phase 3 and Phase 5; QR and phone configuration needs a security design first. Proposal: [install and configuration UX](../design/install-and-config-ux.md). Pairing without typing the key (milestone 3b, daemon side) implemented and PC-tested on 2026-11-02; TV and owner-HA verification pending ([status](../design/install-and-config-ux.md#8-implementation-status-milestone-3b)).
- **Why:** a rooted-TV install via SSH is acceptable for the owner but not for others.
- **Approach options:**
  - Homebrew Channel package with a simple webOS app UI for basic settings (enable, mic gain, wake word, HA connection status).
  - Embedded web config server plus a QR code shown on the TV, so the user configures from a phone (HA connection, ESPHome encryption key, device name). Authentication and TLS for that server must be designed.
  - A pairing flow that avoids typing the encryption key.
- **Dependencies:** Phase 5 `ipk`, the ESPHome encryption model ([ADR 0001](../decisions/0001-esphome-native-api.md)), a security review of any listening configuration port.
- **Evidence needed:** the app can run on c3-a and read or write only its own configuration; the web server is not reachable without authentication; secrets are not displayed or logged.

## 4. TV integration

- **Status:** not prioritized (2026-11-02).
- **Why:** a voice assistant in a TV should cooperate with what the TV is doing.
- **Approach options:** duck or pause TV audio during interactions; on-screen listening indicators and toasts; screen-off listening (depends on D5 power-state results); continue-conversation; arbitration between several TVs hearing the same wake word.
- **Dependencies:** D5 and D4 results, a verified public interface for ducking and toasts (no private Luna endpoints assumed, see [AGENTS.md](../../AGENTS.md)), Home Assistant satellite arbitration features.
- **Evidence needed:** which interfaces are exposed on commercial webOS (webOS OSE docs are not proof), behaviour per app and input, power draw in screen-off mode.

## 5. Music via the TV's AirPlay 2 receiver and Music Assistant

- **Status:** not prioritized (2026-11-02).
- **Why:** music should use the receiver the TV already ships, separate from conversational TTS ([research 06](../research/06-media.md), [EXP-008](../experiments/EXP-008-airplay-media.md)).
- **Approach options:** Home Assistant to Music Assistant to the TV's AirPlay receiver; test coexistence with the satellite's playback path.
- **Dependencies:** Phase 2 playback behaviour, TV input and app state handling.
- **Evidence needed:** AirPlay 2 receiver presence and behaviour on c3-a, and how it interacts with TV audio sources and voice replies.

## 6. Observability

- **Status:** not prioritized (2026-11-02).
- **Why:** a long-running daemon on a TV needs health signals the owner can graph and alert on.
- **Approach options:** metrics for the owner's Prometheus and Grafana (connection state, capture overruns, pipeline latency, wake counts, restarts, memory); a health endpoint. Metrics must not contain audio, speech text or secrets.
- **Dependencies:** Phase 3 resource work; the configuration UX (item 3) if the endpoint shares a web server.
- **Evidence needed:** the overhead on c3-a is small; the endpoint is access restricted.

## 7. OTA updates and fleet configuration

- **Status:** not prioritized (2026-11-02).
- **Why:** manual copying does not scale to several TVs.
- **Approach options:** self-update of the satellite binary with signature verification and rollback; fleet configuration from the owner's GitOps repository or Home Assistant for multiple TVs.
- **Dependencies:** Phase 5 packaging, a signing scheme, the firmware update guard from Phase 3.
- **Evidence needed:** failed-update rollback test, and proof that an update cannot leave the TV in a failsafe boot loop.

## 8. Share findings with the webOS homebrew community

- **Status:** owner priority 3 (2026-11-02): after Phase 2, with a redaction review.
- **Why:** the discovery logs (capture parameters, exclusivity model, oops hazard) are useful beyond this project.
- **Approach options:** a redacted write-up, an upstream issue or wiki entry for the Homebrew projects, a compatibility table.
- **Dependencies:** a redaction review of the [discovery logs](../discovery/README.md); no identifiers, tokens or raw dumps.
- **Evidence needed:** owner approval of the redacted text and a second review for private data.

## 9. Rooted-TV hardening

- **Status:** not prioritized (2026-11-02).
- **Why:** a rooted TV with a permissive local audio server and incomplete update blocking is a security risk.
- **Approach options:** see the full list in [rooted-tv-improvements](../discovery/c3-a/rooted-tv-improvements.md): firmware update blocking gaps, SSH hardening, the anonymous PulseAudio socket, the probe denylist for ASoC sysfs files and others.
- **Dependencies:** owner approval for each change; a rollback plan per item; the PulseAudio item matters once the satellite captures audio.
- **Evidence needed:** per-item verification steps in the linked document.

## 10. Official Homebrew Channel listing

- **Status:** follow-up, not started (2026-11-03). Decided by the owner: the target is the official Homebrew Channel apps repository (`webosbrew/apps-repo`), later. Until then the app is installed by hand from the ipk. There is no self-hosted repository. Decisions: [packaging P4 and P14](../design/packaging.md#1-decisions).
- **Why:** owners of rooted TVs find the app in the Homebrew Channel store and get updates there, instead of copying an ipk by hand.
- **Steps when picked up:**
  1. The repository license is settled: GPL-3.0-or-later (owner, 2026-11-03; [packaging P15](../design/packaging.md#8-license-decision-and-dependency-audit)), so the listing's `pool` is `main`. Still to settle: the title and the icon. Before the release, generate the third-party notice file for the binaries (dependency audit flags in packaging section 8).
  2. Publish a release with the ipk and the generated repo manifest `<id>.manifest.json` ([packaging P13](../design/packaging.md#1-decisions)). The generated manifest next to the ipk is the starting point, not the finished file: check it against the HBC manifest schema, and check that `ipkUrl` and `ipkHash.sha256` match the published ipk.
  3. Write the AI-use disclosure. The apps-repo README says AI use must be disclosed in the PR, and it rejects apps made primarily by AI without meaningful human development, testing and review. **The disclosure wording is part of this step** and needs the owner's approval before the PR is opened. It must describe the human development, testing and review, including the TVs it was run on.
  4. Open a PR to `webosbrew/apps-repo` adding `packages/<name>.yml` (`title`, `iconUri`, `manifestUrl`, `category`, `pool`, `description`, optional `screenshots`), with the disclosure in the PR text.
  5. Confirm the install from the published entry on a TV before announcing it.
- **Release prerequisites (owner decision 2026-11-03: no hard firmware whitelist in the public release; [design](../design/packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist)):** implement and ship before listing: a published **verified-models list** (docs/compatibility plus a README "Verified TVs" section; only listed models are announced as supported); a sanitized compatibility report (`ctl.sh report` and a status-app button: model, webOS, firmware, arch, audio PCM summary, capture and playback check results; no serial, MAC, IP or hostname; allow-list built, with a test); a GitHub issue template "TV compatibility report"; a runtime "verified / not verified" status in the app and as an HA diagnostic entity instead of blocking; and a softer replacement for what the gate protects (silent LG firmware update changing the audio stack): log line, app banner and HA entity when the firmware changed since install, with cheap self-checks. **Update (owner, 2026-11-04):** these prerequisites are pulled forward and reshaped as [backlog #13](#13-soft-firmware-policy-and-one-click-compatibility-reports) (the mic starts on any firmware; the hard gate is replaced now, not at the release). The daemon's current gate (`safety.validated_firmware`) is unchanged until [that design](../design/firmware-policy-and-reports.md) step S1 lands.
- **Dependencies:** [EXP-009](../experiments/EXP-009-ipk-install-update.md) (install, update and uninstall from the real package on c3-b); the release prerequisites above; a compatibility record that names the models and firmware the listing is tested on; a third-party notice file.
- **Evidence needed:** the store install and update work on a TV from the published manifest; the disclosure is accepted in review; the listing does not claim models or firmware that were not tested.

## 11. TV control from Home Assistant via the satellite

- **Status:** **implemented in 0.2.0 and run on both TVs** (2026-11-04; [design section 15](../design/tv-control.md#15-implementation-status-2026-11-04)): on c3-a permanently (non-disruptive subset through the owner's HA) and on c3-b from `/tmp` (disruptive controls through an API client), [EXP-010 Part B](../experiments/EXP-010-tv-control-setters.md#part-b-results-2026-11-04). Fixed on the PC after Part B, to re-check on the TVs: the volume subscription (kept, confirmed by a push, else polls), the `launch` declaration check (SAM file), `volume_limit` (optional, unset by default), the refresh summary line. Open: B10 and B11, the disruptive controls on c3-a. Owner decisions of 2026-11-04 after Part A: volume, mute, input and apps are on by default, TV control is not gated by the firmware guard. Earlier: design done. Owner decisions in [ADR 0005](../decisions/0005-tv-control-entities.md): complement HA's `webostv`; v1 = power and screen, volume and mute, input, app launch (no toasts); separate entities, not a TV media player; power-off only into the keep-listening `Screen Off` state and only when that mode is enabled (opt-in); listening indicator as HA binary sensors only. Design: [TV control](../design/tv-control.md). Setter test procedure: [EXP-010](../experiments/EXP-010-tv-control-setters.md) on c3-b: Part A ran on 2026-11-04 and passed (screen, mute, volume set and step, input switch, app launch; A5 skipped); Part B ran the same day. Disruptive controls verified on c3-b (`03.30.73`) only; c3-a (`03.30.74`): mute, volume down and the entity list only. The text below is the original idea (2026-11-03), kept for context; its open questions on overlap and entity design are settled by ADR 0005.
- **Idea:** the satellite already runs as root on the TV and holds an ESPHome API connection to Home Assistant. It could expose TV controls as HA entities:
  - power and screen on and off (the daemon already calls `tvpower` and `turnOffScreen` for the remote power-off feature, [always-on section 13](../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b));
  - TV volume and mute;
  - input or source selection;
  - launching apps;
  - possibly notifications or toasts.

  Voice commands such as "turn off the TV" or "TV volume down" would then work through Assist.
- **Why:** no separate SSAP pairing or token is needed. HA's built-in webOS TV integration needs that pairing, and it does not work while the TV is in standby. The satellite reuses its existing power-state subscription ([always-on](../design/always-on.md), G1). These are reasons to test, not verified claims; the standby limitation of the built-in integration is to be confirmed on the owner's HA before it is cited.
- **Approach options:**
  - Entities in the ESPHome device, with the controls implemented by the daemon through public webOS interfaces only (no private Luna endpoints assumed, see [AGENTS.md](../../AGENTS.md)).
  - Entity design: one ESPHome `media_player` for the TV, or separate entities (switch, number, select, button). The satellite's existing media player already means "reply volume" ([always-on section 13](../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b)), so a TV media player would clash with it unless the existing one is renamed or the TV entities are separate.
  - Overlap with HA's webOS TV integration: complement it (standby control, voice, no pairing) or replace it. Decide before any build.
- **Open questions:**
  - Overlap with HA's `webostv` integration: complement or replace?
  - Which luna methods are stable across firmware versions? Verify each one on the target and record it per model and firmware in the [compatibility table](../compatibility/README.md); do not assume an interface from webOS OSE documentation.
  - Safety: every action is user-initiated, and nothing writes TV settings silently. This follows the existing rules in [AGENTS.md](../../AGENTS.md) and the [discovery safety rules](../discovery/README.md#safety-rules-learned). Any action that changes a TV setting must be explicit, reversible and listed with its risk.
  - Entity design: a TV `media_player` versus separate entities, given that the satellite's media player already means reply volume (see approach options).
- **Dependencies:** the power-state subscription and remote power-off feature ([always-on](../design/always-on.md)); the ESPHome entity work in [ADR 0001](../decisions/0001-esphome-native-api.md); a verified list of public interfaces per firmware; item 4 (TV integration) for any overlap with ducking or toasts.
- **Evidence needed:** for each control, a read-only check first, then a single user-initiated test on the target with the state restored afterwards; the luna methods and results recorded per model and firmware; the built-in HA integration's standby behaviour checked on the owner's HA before the comparison is written up.

## 12. Conversation agent for the Kiwi pipeline

- **Status:** **decision pending (owner)**, deferred to a future plan discussion (owner, 2026-11-04). No HA change made.
- **Why:** the satellite works end to end with HA's built-in agent, but the owner's Kiwi pipeline has no working conversation agent, so free-form requests fail. The choice also decides how voice reaches TV-control numbers and selects ([TV control section 11](../design/tv-control.md#11-voice)).
- **Facts (2026-11-04, owner's homelab, read by an agent):**
  - Kiwi's Ollama agent fails: the in-cluster Ollama has no models loaded and answers 404 for `qwen2.5:3b`.
  - The pipeline option `prefer_local_intents` exists on HA 2026.10.4 and is currently `false`.
  - The homelab "Kiais" OpenAI-compatible gateway has a fast tier with a TTFT p50 of about 1.5 s, measured on **only 2 requests**; it is cloud-routed. HA's core `openai_conversation` integration has no base URL setting, so using it needs a custom or HACS integration.
  - Separate homelab issue: the HA pod was recreated 8 times in one day, and each recreation disconnected the satellite. This is not a satellite fault; it is tracked on the homelab side.
- **Options:**
  - **A.** Reload `qwen2.5:3b` on the in-cluster Ollama (GTX 1060, 3 GB). Local; quality and latency on that GPU not measured.
  - **B.** Set `prefer_local_intents = true` on the pipeline, so HA's built-in intents (TV switches, lights) answer first and only other requests reach the agent.
  - **C.** B plus a fallback agent on the Kiais gateway's fast tier. Better language ability; requests leave the house (cloud-routed), and it needs a custom or HACS integration.
- **Dependencies:** owner decision; the homelab GitOps repository for any change; the HA pod stability issue.
- **Evidence needed before acceptance:** for the chosen option, end-to-end latency and success on the owner's own phrases through the satellite, and for C, what leaves the house and under which terms.

## 13. Soft firmware policy and one-click compatibility reports

- **Status:** design done (2026-11-04), implementation next, **owner priority**. Owner decision (2026-11-04): "make the mic work. Why do we stop it? It is better to start it, and if it doesn't work we have to have an easy way for users to report that it doesn't work for that firmware version". Design: [firmware policy and reports](../design/firmware-policy-and-reports.md); decision: [ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md). Nothing is implemented; the hard gate still runs.
- **Why:** the hard gate keeps the mic off for every TV outside a list of two firmwares and gives no way to learn whether it would have worked. No failure of the capture path on another firmware has been observed (c3-b `03.30.73` captured 81 minutes).
- **Scope:** policy `warn` by default with an opt-in `strict`; measured self-checks on the normal capture stream (no ASoC sysfs reads, no recording); entities `firmware_verified`, `tv_firmware`, `tv_model`, `compat_status`, `selfcheck`, `capture_problem`, `firmware_changed`, `report_status` and three buttons; an allow-listed report with a prefilled GitHub issue link through an HA button (needs the HA option "Allow the device to perform Home Assistant actions"), a QR code in the status app and `ctl.sh report`; the issue form [`compatibility-report.yml`](../../.github/ISSUE_TEMPLATE/compatibility-report.yml); the verified-models list (data file plus docs). Absorbs the release prerequisites of #10.
- **Steps:** S1 to S8 in the [design, section 9](../design/firmware-policy-and-reports.md#9-implementation-plan).
- **Open questions (owner):** the HA option trade-off, the repository and label names, whether a GitHub login is acceptable, the first verified rows, exact model versus series matching ([design section 11](../design/firmware-policy-and-reports.md#11-questions-for-the-owner)).
- **Evidence needed:** the thresholds measured on c3-a and c3-b, the prefill behaviour of the issue form, the report checked against the TV's real identifiers, the HA action path on the owner's HA ([design section 10](../design/firmware-policy-and-reports.md#10-what-to-verify-after-deployment)).
