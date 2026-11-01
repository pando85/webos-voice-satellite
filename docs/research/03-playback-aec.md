# Playback, mixing and acoustic echo cancellation

## Core questions

How do we play TTS through the native LG audio policy while Netflix/HDMI remains active? Can we obtain time-aligned playback reference for AEC, including eARC, soundbars and AirPlay?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

webOS OSE describes audio services and focus policies, but API availability/permissions on commercial LG webOS TV are NOT verified.

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Test native Luna audio routes first; distinguish PCM speaker output, virtual sinks, HDMI/eARC output and a genuine loopback reference. Compare vendor-cleaned mic signal with independent AEC only after measurement.

## Planned experiments

- [EXP-005](../experiments/README.md)
- [EXP-006](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
