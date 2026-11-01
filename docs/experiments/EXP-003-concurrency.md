# EXP-003 — Concurrent capture and native voice DSP

**Status:** PLANNED · **Risk:** approved audio testing, otherwise read-only

**Question:** Does microphone capture work during native TV voice use, Netflix/HDMI playback, and background apps? Is any accessible signal vendor-processed?

**Prerequisites:** EXP-002. Compare controlled mic samples while TV is silent, playing a fixed test signal, and with native voice processing enabled/disabled via documented settings only. Measure channel/sample behavior, gain/noise and errors. Do not stop LG services.

**Acceptance:** coexistence, capture stability and signal differences documented by TV/firmware/source. DSP or AEC capabilities require measured evidence.
