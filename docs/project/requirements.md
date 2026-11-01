# Requirements and feasibility gates

All requirements are targets until verified; see [experiments](../experiments/README.md).

| ID | Requirement | Evidence gate |
| --- | --- | --- |
| F0 | Persistent daemon while TV is fully running | Process survives launcher/input/app transitions and reconnects |
| F1 | Built-in far-field microphone PCM | Repeatable voice capture with no Magic Remote |
| F2 | Non-disruptive concurrent operation | Capture does not break native TV playback or services |
| F3 | Home Assistant pipeline | Wake/STT/conversation/TTS roundtrip with bounded failure handling |
| F4 | Audible native TTS | Short replies through TV audio without unwanted input switching |
| F5 | Quality in real conditions | Repeatable speech recognition with TV playback and noise |
| F6 | Screen-off/Always Ready mode | Daemon, audio, network and energy behavior measured separately |
| F7 | Secure, consented always-listening | Mute, privacy, authentication and retention model documented |
| F8 | Music playback | Existing AirPlay/other receiver behavior validated separately |

**Go/no-go for MVP:** F1 + F2 + F3; F4 to complete end-to-end voice response. F6 is a crucial product goal but not a prerequisite to prove initial feasibility.

**Non-goals for MVP:** rooting televisions; replacing LG firmware/audio policy; writing AirPlay or a music player; LLM, STT, or TTS inference on-TV; universal compatibility without measurements.

**Quality measurements:** capture format/channels, clipping, peak/RMS, latency, wake false accepts/hour, missed wakes, command success rate, TV-playback volume, power draw by state, resume time, and recovery after network loss. Define a reproducible test corpus with consent before setting acceptance thresholds.
