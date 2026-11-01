# Platform and lifecycle

## Core questions

Can native code run while another TV application owns the screen? What is the smallest set of services needed? What survives screen-off, Always Ready, Quick Start+ and standby?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

Homebrew Channel documents boot-time root startup hooks; ordinary TV web applications can suspend in the background. LG Always Ready advertises vendor voice features with the screen off, not arbitrary daemon execution.

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Do not assume native service persistence in deep standby; compare screen-off vs low-power standby, CPU availability, network access, ALSA capture, power watts and wake latency.

## Planned experiments

- [EXP-001](../experiments/README.md)
- [EXP-004](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
