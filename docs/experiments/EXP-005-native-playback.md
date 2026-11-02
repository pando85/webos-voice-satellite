# EXP-005 — Native audio playback and mix policy

**Status:** IN PROGRESS: audible reply path verified on c3-a, TV speaker output (P2 session 1, 2026-11-02): `pacat` and HA TTS through the satellite at master volumes 5 to 25 with measured mic and SpeakerFeedback levels, an HA end-to-end reply at volume 10, no stream-based ducking of HDMI. Open: other outputs (eARC, BT), paused or app playback, a reply-loudness policy with a listener. See [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md#1-audible-reply-path-and-volume-curve-exp-005) · **Risk:** audio output, no system changes

**Question:** Can a brief TTS sound play through the TV's active output while streaming/HDMI content continues, without changing inputs?

**Prerequisites:** document the specific commercially available Luna/audio APIs and their permissions; do not assume OSE API parity. Test with a harmless short locally generated tone/speech, with user approval. Compare TV speakers, HDMI/eARC, normal media app and paused playback. Measure focus, ducking and restoration.

**Acceptance:** verified working route with permissions/format/source/mix behavior per TV, or scoped failure.
