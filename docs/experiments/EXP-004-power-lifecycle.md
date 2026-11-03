# EXP-004 — Background and power state transitions

**Status:** PLANNED (procedure for c3-a ready, not run) · **Risk:** controlled settings/power change requires approval

**Question:** Which runtime/audio capabilities survive app switching, screen off, Always Ready, Quick Start+, and standby?

**Prerequisites:** EXP-001 and a reversible test process; agree on recovery method. Observe PID persistence, CPU/network, ability to stream PCM and power draw in each **distinct** mode. Test cold boot and recovery separately. Avoid invasive wake/standby hacks.

**Acceptance:** state transition matrix per model and measured power/recovery; unavailable results marked INCONCLUSIVE rather than assumed.

**c3-a procedure (2026-11-02):** [D5 plan](../discovery/c3-a/D5-plan.md) gives the owner-run steps (screen off, standby with and without Always Ready, power-on recovery), the read-only observation loops, expected signals, failure interpretation and rollback. The satellite side it relies on (capture stall reopen, watchdog suspend detection, reconnection) is in [P3 robustness](../discovery/c3-a/P3-robustness.md). No power state has been changed by an agent. Revised 2026-11-03: the plan is agent-runnable (the owner allowed power-state changes, settings changes and reboots when a test needs them, with every setting restored) and adds a way-back gate, playback checks, a cold-boot step and an all-day screen-off soak; the design it feeds is [always-on](../design/always-on.md).
