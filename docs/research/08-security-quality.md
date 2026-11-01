# Privacy, security and acoustic evaluation

## Core questions

How can users see whether mic is active, mute physically/logically, consent to capture, authenticate HA connection, and test false activation, accuracy and power?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

Root + ambient microphone creates a high-trust component. Reliable acoustics and measured power are product features, not later polish.

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Default to no capture until enabled and never persist speech by default. Build reproducible synthetic phrase/noise corpus, consented spoken tests, clipping and loud-TV scenarios. Define privacy and threat model prior to release.

## Planned experiments

- [EXP-002](../experiments/README.md)
- [EXP-003](../experiments/README.md)
- [EXP-004](../experiments/README.md)
- [EXP-007](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
