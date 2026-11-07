# Research index

The question is **not** “can a rooted TV run code?” but “can a specific rooted LG TV act as a reliable, private, full-duplex voice satellite under real playback and power conditions?”

Read in dependency order:

1. [Platform and lifecycle](01-platform-lifecycle.md)
2. [Audio capture and DSP](02-audio-capture.md)
3. [Playback and echo cancellation](03-playback-aec.md)
4. [Wake word and VAD](04-activation.md)
5. [Home Assistant protocol](05-home-assistant.md)
6. [Music and media](06-media.md)
7. [Deployment and compatibility](07-deployment.md)
8. [Privacy, security, and performance](08-security-quality.md)

Topic notes (features researched after the feasibility phase):

- [Magic Remote mic button](remote-mic-button.md): the press, the remote's microphone, LG's assistant and its toast
- [Mic-button key interception](mic-button-key-interception.md): portable ways to take the mic key with no LG toast and no LG assistant (evdev grab and re-emit, compositor and service options), ranked
- [Home Assistant side audio processing](ha-side-audio-processing.md): where HA processes satellite audio (enhancer, VAD, wake word, STT), why ESPHome `audio_settings` are ignored, options against false wakes and noise transcribed as words, "normalise, then detect", and a staged plan with an offline replay experiment

Use the [source ledger](sources.md) to distinguish vendor-documented facts from observations on **other** devices. Never represent the presence of webOS Open Source Edition APIs as a confirmed LG commercial TV interface.

**Status convention:** `DOCUMENTED` (contract only), `OBSERVED_OTHER_MODEL`, `HYPOTHESIS`, `TARGET_VERIFIED`, `REJECTED`, `INCONCLUSIVE`. A `TARGET_VERIFIED` claim must link an experiment and identify firmware/model/state.
