# Provisional architecture

**Status: proposal, not implementation contract. Partly superseded on 2026-11-01** by [ADR 0001](../decisions/0001-esphome-native-api.md) (ESPHome native API instead of Assist WebSocket), [ADR 0002](../decisions/0002-rust-implementation.md) (Rust), [ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md) (server-side wake word for the MVP) and [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md) (capture from `hw:LGSIC,0`, LG hands-free off). The diagram below is the original sketch: read "authenticated WebSocket" as the ESPHome native API and "LG native?" as not used. The owner also considers this document not fully trustworthy; the [roadmap](../project/roadmap.md) and ADRs take precedence.

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

> **Superseded for music (2026-11-05):** the owner chose native playback in the satellite and rejected AirPlay ([ADR 0010](../decisions/0010-music-playback.md)). The music plane in the diagram above is therefore not the plan; the text below is the earlier reasoning, kept for history.

The TV already has media receivers, audio decoders, and source policies. Implementing AirPlay, a music library, or a mixer inside the daemon would duplicate functionality. Use AirPlay/Music Assistant as a **candidate media integration**; avoid using it for quick TTS while another TV app is playing.

## Unsettled decisions

- ALSA capture source vs LG vendor DSP processed stream.
- LG-native vs local vs Assist wake word.
- Native audio policy mixing vs direct playback; availability of echo reference.
- Native daemon runtime and toolchain.
- Assist WebSocket vs full ESPHome protocol.
- Screen-off and standby policy, power budget, privacy UI.

Decisions go into [decision records](../decisions/README.md) only after measurements.
