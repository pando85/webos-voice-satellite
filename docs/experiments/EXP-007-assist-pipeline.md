# EXP-007 — Wake-word and Home Assistant Assist pipeline

**Status:** PLANNED · **Risk:** limited local audio stream, authenticated network

**Question:** Can TV mic PCM activate Assist, execute a command and play the answer reliably? How do we deal with multiple TV satellites?

**Prerequisites:** EXP-002/003 and known-good HA pipeline; avoid embedding long-lived tokens in commands or committed files. Start with `assist_pipeline/run` streaming and server-side wake word; compare TV-native event only if demonstrated. Test cancellation, reconnection, timeouts, false positives and simultaneous wake on two devices.

**Acceptance:** end-to-end scripted evidence, timings, authentication method and known failures, scoped to model/firmware.
