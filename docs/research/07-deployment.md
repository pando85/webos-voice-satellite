# Deployment and portability

## Core questions

What is each TV's userland architecture (not only kernel), libc, runtime dependencies, install persistence and rollback? What permissions are needed for capture/playback?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

Homebrew Channel provides startup hooks and root exec APIs; CleanYourTV describes a 32-bit LG userland alongside a 64-bit recognition sidecar for one TV.

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Inventory both TVs. Do not choose Rust target or base image without uname + ELF interpreter + libc evidence. Future packages should offer upgrades/rollback and firmware-specific capability detection.

## Planned experiments

- [EXP-001](../experiments/README.md)
- [EXP-004](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
