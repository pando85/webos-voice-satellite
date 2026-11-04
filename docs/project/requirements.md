# Requirements and feasibility gates

All requirements are targets until verified; see [experiments](../experiments/README.md).

**Status as of 2026-11-01 (c3-a, LG C3, webOS TV 8.3.0, firmware 03.30.74).** The Status column below was added after device discovery. Labels follow [AGENTS.md](../../AGENTS.md) (`verified-on-target`, `observed-on-other-device`, `hypothesis`, `documented`); "pending" means no evidence yet. The plan that closes each gate is in the [roadmap](roadmap.md), and decisions are in the [ADR index](../decisions/README.md).

| ID | Requirement | Evidence gate | Status |
| --- | --- | --- | --- |
| F0 | Persistent daemon while TV is fully running | Process survives launcher/input/app transitions and reconnects | pending. Not tested. Runtime facts only: [32-bit ARM soft-float, glibc 2.31, Homebrew startup hook](../discovery/c3-a/D0-D1-runtime.md). Roadmap 1.1 and Phase 3 |
| F1 | Built-in far-field microphone PCM | Repeatable voice capture with no Magic Remote | **verified-on-target for basic capture** (`hw:LGSIC,0`, 2 ch, S24_LE, 16 kHz, with LG hands-free off and the TV silent; one room, one position, no human voice yet). See [D3](../discovery/c3-a/D3-mic-identification.md). Not yet verified in other power states or with TV playback |
| F2 | Non-disruptive concurrent operation | Capture does not break native TV playback or services | **Concurrency with LG hands-free = exclusive**: `voiceinput` holds the mic PCM and the SpeakerFeedback PCM while hands-free is on, and releases them when it is off ([D3](../discovery/c3-a/D3-mic-identification.md#setup-and-method), [D2](../discovery/c3-a/D2-audio-stack.md#who-holds-which-sound-device-snapshot)). MVP mitigation: keep hands-free off ([ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md)). Capture while TV playback and other services are active: pending (Phase 2) |
| F3 | Home Assistant pipeline | Wake/STT/conversation/TTS roundtrip with bounded failure handling | **offline evidence only**: Whisper STT word-perfect on TV-mic audio; openWakeWord `alexa` detected in 4/4 variants at +12 dB playback, borderline at +6 dB, not detected at -30 dBFS (single runs, TTS source, no rates). See [D3 offline check](../discovery/c3-a/D3-mic-identification.md#offline-ha-pipeline-check-wyoming-2026-11-01). No satellite-to-HA roundtrip yet (roadmap Phase 1) |
| F4 | Audible native TTS | Short replies through TV audio without unwanted input switching | pending. Candidate route: system PulseAudio default sink (`card0 pcm6p`), [hypothesis](../discovery/c3-a/D3-mic-identification.md#2-step-5a-magic-remote-mic-button-2026-11-01). Roadmap 1.6 (silent) and Phase 2 (audible) |
| F5 | Quality in real conditions | Repeatable speech recognition with TV playback and noise | pending. Mic is near-raw with no AEC/NS; echo behaviour and the SpeakerFeedback reference are untested (D4). Roadmap Phase 2 |
| F6 | Screen-off/Always Ready mode | Daemon, audio, network and energy behavior measured separately | pending (D5 not done). Roadmap Phase 3 |
| F7 | Secure, consented always-listening | Mute, privacy, authentication and retention model documented | pending. Safety and privacy rules exist ([discovery safety rules](../discovery/README.md#safety-rules-learned)); mute switch and a truthful listening indicator (HA entities "Listening" and "Microphone muted", driven by the real PCM state, [development](../development.md#listening-indicator)) built and PC-tested 2026-11-04, **not yet verified on a TV or in the owner's HA**; the rest of the model is open. Roadmap Phases 1.7 and 4 |
| F8 | Music playback | Existing AirPlay/other receiver behavior validated separately | pending. Not tested ([EXP-008](../experiments/EXP-008-airplay-media.md)); backlog item |

**Go/no-go for MVP:** F1 + F2 + F3; F4 to complete end-to-end voice response. F6 is a crucial product goal but not a prerequisite to prove initial feasibility.

**Status note (2026-11-01):** F2 resolves for the MVP as *not concurrent with LG hands-free*: the MVP requires hands-free off ([ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md)). F1 and F2 have target evidence; F3 has offline evidence only; F4 and the others remain open.

**Non-goals for MVP:** rooting televisions; replacing LG firmware/audio policy; writing AirPlay or a music player; LLM, STT, or TTS inference on-TV; universal compatibility without measurements.

**Quality measurements:** capture format/channels, clipping, peak/RMS, latency, wake false accepts/hour, missed wakes, command success rate, TV-playback volume, power draw by state, resume time, and recovery after network loss. Define a reproducible test corpus with consent before setting acceptance thresholds.

## Superseded text

- The persistent native daemon in F0 remains a **provisional** design choice (not proven in standby). The implementation language and the integration route, which were unspecified here, are now fixed by [ADR 0002](../decisions/0002-rust-implementation.md) and [ADR 0001](../decisions/0001-esphome-native-api.md).
- The earlier implicit open question of wake word location is **superseded** by [ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md) for the MVP.
- The phase model that used to be referenced by these gates is **superseded** by the [revised roadmap](roadmap.md).
