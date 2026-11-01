# Backlog: post-Phase-5 improvements

**Unprioritized; to be reprioritized by the owner.** These items are ideas and options, not commitments. None of them blocks the MVP defined in the [roadmap](roadmap.md). Statements about techniques are `hypothesis` or `documented` until an experiment on the target verifies them. Each item lists why, approach options, dependencies and the evidence needed before it can be accepted.

## 1. Professional audio front-end

- **Why:** commercial assistants combine echo cancellation, beamforming, noise suppression and gain control. The built-in mic path on c3-a is near-raw (DC removal plus a fixed +16 dB gain, no AGC or noise suppression; [D3](../discovery/c3-a/D3-mic-identification.md)), so all of that is left to the satellite or to Home Assistant. TV playback in the mic is the main expected quality problem.
- **Approach options:**
  - Acoustic echo cancellation using the `hw:LGSIC,1` SpeakerFeedback stream as the reference: WebRTC AEC3 or speexdsp (cheaper CPU, weaker). Needs delay estimation and clock-drift handling between the two PCMs.
  - Two-microphone beamforming with a spacing of about 6 to 8 cm (inferred, geometry unknown): delay-and-sum, MVDR or GSC. The D3 analysis showed that a naive channel average lowered SNR, so any gain must be measured.
  - Noise suppression: RNNoise, DeepFilterNet, WebRTC NS. Check effects on wake word and STT, since suppression can hurt both.
  - AGC, dereverberation and a tuned VAD.
  - Optional: study LG's own vendor front-end library (`libaec-fe`) with the [REA tool](../tools/rea.md) to learn how it maps the reference and mic, **not to reuse it**. Static analysis only; follow the REA rules in [AGENTS.md](../../AGENTS.md) and do not treat results as device validation.
- **Dependencies:** Phase 2 D4 results (is SpeakerFeedback a usable reference), the test corpus, CPU and memory budget from Phase 3, a Rust or C library choice under [ADR 0002](../decisions/0002-rust-implementation.md).
- **Evidence needed:** a consented test corpus with TV content playing; metrics for false rejects, false accepts per hour and word error rate (WER) with and without each stage; CPU and latency per stage on the Cortex-A76 cores under the 32-bit userland.

## 2. On-TV wake word

- **Why:** lower latency, lower bandwidth and less Home Assistant load; stop continuous audio streaming to HA; support several and custom wake words. Privacy is **not** the motivation, since everything is local ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)).
- **Approach options:** microWakeWord or openWakeWord on the TV (ARM Cortex-A76 cores running 32-bit code; check TFLite or ONNX runtime builds for the soft-float target); a hybrid that runs a light on-TV detector and confirms server-side; wake word through the ESPHome voice-assistant start flow.
- **Dependencies:** Phase 4 custom model work, front-end quality (item 1), runtime build feasibility for the 32-bit softfp target, resource limits from Phase 3.
- **Evidence needed:** CPU and memory use on c3-a, detection rate and false accepts per hour compared with the server-side baseline on the same corpus, and behaviour when the TV is playing content.

## 3. Install and configuration UX

- **Why:** a rooted-TV install via SSH is acceptable for the owner but not for others.
- **Approach options:**
  - Homebrew Channel package with a simple webOS app UI for basic settings (enable, mic gain, wake word, HA connection status).
  - Embedded web config server plus a QR code shown on the TV, so the user configures from a phone (HA connection, ESPHome encryption key, device name). Authentication and TLS for that server must be designed.
  - A pairing flow that avoids typing the encryption key.
- **Dependencies:** Phase 5 `ipk`, the ESPHome encryption model ([ADR 0001](../decisions/0001-esphome-native-api.md)), a security review of any listening configuration port.
- **Evidence needed:** the app can run on c3-a and read or write only its own configuration; the web server is not reachable without authentication; secrets are not displayed or logged.

## 4. TV integration

- **Why:** a voice assistant in a TV should cooperate with what the TV is doing.
- **Approach options:** duck or pause TV audio during interactions; on-screen listening indicators and toasts; screen-off listening (depends on D5 power-state results); continue-conversation; arbitration between several TVs hearing the same wake word.
- **Dependencies:** D5 and D4 results, a verified public interface for ducking and toasts (no private Luna endpoints assumed, see [AGENTS.md](../../AGENTS.md)), Home Assistant satellite arbitration features.
- **Evidence needed:** which interfaces are exposed on commercial webOS (webOS OSE docs are not proof), behaviour per app and input, power draw in screen-off mode.

## 5. Music via the TV's AirPlay 2 receiver and Music Assistant

- **Why:** music should use the receiver the TV already ships, separate from conversational TTS ([research 06](../research/06-media.md), [EXP-008](../experiments/EXP-008-airplay-media.md)).
- **Approach options:** Home Assistant to Music Assistant to the TV's AirPlay receiver; test coexistence with the satellite's playback path.
- **Dependencies:** Phase 2 playback behaviour, TV input and app state handling.
- **Evidence needed:** AirPlay 2 receiver presence and behaviour on c3-a, and how it interacts with TV audio sources and voice replies.

## 6. Observability

- **Why:** a long-running daemon on a TV needs health signals the owner can graph and alert on.
- **Approach options:** metrics for the owner's Prometheus and Grafana (connection state, capture overruns, pipeline latency, wake counts, restarts, memory); a health endpoint. Metrics must not contain audio, speech text or secrets.
- **Dependencies:** Phase 3 resource work; the configuration UX (item 3) if the endpoint shares a web server.
- **Evidence needed:** the overhead on c3-a is small; the endpoint is access restricted.

## 7. OTA updates and fleet configuration

- **Why:** manual copying does not scale to several TVs.
- **Approach options:** self-update of the satellite binary with signature verification and rollback; fleet configuration from the owner's GitOps repository or Home Assistant for multiple TVs.
- **Dependencies:** Phase 5 packaging, a signing scheme, the firmware update guard from Phase 3.
- **Evidence needed:** failed-update rollback test, and proof that an update cannot leave the TV in a failsafe boot loop.

## 8. Share findings with the webOS homebrew community

- **Why:** the discovery logs (capture parameters, exclusivity model, oops hazard) are useful beyond this project.
- **Approach options:** a redacted write-up, an upstream issue or wiki entry for the Homebrew projects, a compatibility table.
- **Dependencies:** a redaction review of the [discovery logs](../discovery/README.md); no identifiers, tokens or raw dumps.
- **Evidence needed:** owner approval of the redacted text and a second review for private data.

## 9. Rooted-TV hardening

- **Why:** a rooted TV with a permissive local audio server and incomplete update blocking is a security risk.
- **Approach options:** see the full list in [rooted-tv-improvements](../discovery/c3-a/rooted-tv-improvements.md): firmware update blocking gaps, SSH hardening, the anonymous PulseAudio socket, the probe denylist for ASoC sysfs files and others.
- **Dependencies:** owner approval for each change; a rollback plan per item; the PulseAudio item matters once the satellite captures audio.
- **Evidence needed:** per-item verification steps in the linked document.
