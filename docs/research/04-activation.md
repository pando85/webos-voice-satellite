# Wake word and voice activity

## Core questions

Use LG native activation events, an on-TV model, or Home Assistant server-side wake word? How to avoid false triggers and wake-word collisions between TVs?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

Assist pipeline supports start_stage wake_word or stt. LG's built-in hotword presence does not establish a reusable event API.

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Start MVP with server-side wake word if PCM is available, measure bandwidth/CPU and privacy; investigate on-TV and native LG detectors as interchangeable providers. VAD is not equivalent to wake word.

## Planned experiments

- [EXP-007](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
