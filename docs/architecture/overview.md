# Provisional architecture

**Status: proposal, not implementation contract.**

```text
LG TV (rooted)
  far-field microphones
        |
   [Audio source adapter] -- [optional vendor DSP / AEC]
        |
   [Activation adapter] <-- LG native? local wake? server wake?
        |
   [Assist pipeline client] <----authenticated WebSocket----> Home Assistant
        |                                               wake/STT/agent/TTS
   [TTS playback adapter] ---> native LG audio mixing ---> TV speakers/eARC

Independent music plane:
Home Assistant -> Music Assistant -> LG AirPlay/other receiver
```

Daemon boundaries: capture, optional preprocessing, wake/activation, transport, playback, lifecycle, diagnostics, privacy controls. Each backend is independently replaceable; no universal ALSA device indexes.

## Why music is separate

The TV already has media receivers, audio decoders, and source policies. Implementing AirPlay, a music library, or a mixer inside the daemon would duplicate functionality. Use AirPlay/Music Assistant as a **candidate media integration**; avoid using it for quick TTS while another TV app is playing.

## Unsettled decisions

- ALSA capture source vs LG vendor DSP processed stream.
- LG-native vs local vs Assist wake word.
- Native audio policy mixing vs direct playback; availability of echo reference.
- Native daemon runtime and toolchain.
- Assist WebSocket vs full ESPHome protocol.
- Screen-off and standby policy, power budget, privacy UI.

Decisions go into [decision records](../decisions/README.md) only after measurements.
