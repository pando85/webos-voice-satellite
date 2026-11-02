# EXP-007 — Wake-word and Home Assistant Assist pipeline

**Status:** IN PROGRESS: on c3-a the satellite runs through HA's ESPHome integration (Phase 1). [P2 session 2](../discovery/c3-a/P2-session2-wake-accuracy.md) measured false wakes with TV content (0/1.14 h at volume 10, 2/1.125 h at volume 15), digital and semi-synthetic wake/STT/intent baselines, and that no wake word can be detected during a reply. Talker-in-the-room rates are open (the rig was unavailable) · **Risk:** limited local audio stream, authenticated network

**Question:** Can TV mic PCM activate Assist, execute a command and play the answer reliably? How do we deal with multiple TV satellites?

**Prerequisites:** EXP-002/003 and known-good HA pipeline; avoid embedding long-lived tokens in commands or committed files. Start with `assist_pipeline/run` streaming and server-side wake word; compare TV-native event only if demonstrated. Test cancellation, reconnection, timeouts, false positives and simultaneous wake on two devices.

**Acceptance:** end-to-end scripted evidence, timings, authentication method and known failures, scoped to model/firmware.
