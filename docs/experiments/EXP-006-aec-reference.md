# EXP-006 — Echo reference and AEC feasibility

**Status:** PLANNED · **Risk:** controlled audio capture/output with approval

**Question:** Can the TV provide a sample-aligned reference of what it actually emits, and can we use existing LG DSP output?

**Prerequisites:** EXP-002/003 and EXP-005. Investigate audio routing and loopbacks read-only; do not call every ALSA capture device a microphone or loopback. Compare output path including HDMI/eARC, buffering and latency; only then evaluate external AEC.

**Acceptance:** documented signal provenance and timing/quality measurements. An AEC implementation is NOT planned without a reference and a baseline comparison.
