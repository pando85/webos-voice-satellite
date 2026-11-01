# Roadmap

## Phase 0 — Documentary research and safe discovery (current)
- Trace official and community evidence to bounded research questions.
- Build read-only inventory on both rooted TVs.
- Classify all findings by model, firmware, and runtime state.

## Phase 1 — Capture proof of concept
- Map ALSA sources and find actual built-in microphone PCM.
- Test concurrent native voice services and TV playback.
- Record formats, noise, DSP effects, and failure modes.

## Phase 2 — Voice pipeline proof of concept
- Stream audio to Home Assistant Assist over an authenticated connection.
- Test server-side wake word and TTS; compare with optional LG-native activation.
- Measure latency, reconnection, and multilingual behavior.

## Phase 3 — Playback and acoustic quality
- Find native playback/mixing and ducking behavior across apps/HDMI/eARC.
- Investigate AEC reference availability and whether vendor DSP output is usable.
- Evaluate TV playing content and screen-off behavior.

## Phase 4 — Productization (only after validated gates)
- Choose runtime toolchain, deployment/rollback, UI, security controls, and support matrix.
- Add automated tests using recorded **consented synthetic** fixtures and limited device smoke tests.

Research tasks tracked by GitHub Issues; this roadmap is not a second issue tracker.
