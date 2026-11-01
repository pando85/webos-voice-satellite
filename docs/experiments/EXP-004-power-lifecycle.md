# EXP-004 — Background and power state transitions

**Status:** PLANNED · **Risk:** controlled settings/power change requires approval

**Question:** Which runtime/audio capabilities survive app switching, screen off, Always Ready, Quick Start+, and standby?

**Prerequisites:** EXP-001 and a reversible test process; agree on recovery method. Observe PID persistence, CPU/network, ability to stream PCM and power draw in each **distinct** mode. Test cold boot and recovery separately. Avoid invasive wake/standby hacks.

**Acceptance:** state transition matrix per model and measured power/recovery; unavailable results marked INCONCLUSIVE rather than assumed.
