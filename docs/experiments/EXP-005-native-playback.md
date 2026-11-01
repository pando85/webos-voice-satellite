# EXP-005 — Native audio playback and mix policy

**Status:** PLANNED · **Risk:** audio output, no system changes

**Question:** Can a brief TTS sound play through the TV's active output while streaming/HDMI content continues, without changing inputs?

**Prerequisites:** document the specific commercially available Luna/audio APIs and their permissions; do not assume OSE API parity. Test with a harmless short locally generated tone/speech, with user approval. Compare TV speakers, HDMI/eARC, normal media app and paused playback. Measure focus, ducking and restoration.

**Acceptance:** verified working route with permissions/format/source/mix behavior per TV, or scoped failure.
