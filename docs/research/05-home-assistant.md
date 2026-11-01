# Home Assistant protocol

> **Superseded hypothesis.** The "prototype Assist WebSocket first" hypothesis below is superseded by [ADR 0001](../decisions/0001-esphome-native-api.md) (ESPHome native API) and the research in [esphome-rust-satellite.md](esphome-rust-satellite.md). The Assist pipeline details below remain as reference only.

## Core questions

How should the TV authenticate, register a satellite, send streaming PCM, handle audio events, TTS and follow-up conversations? How to reconcile Assist pipeline WebSocket with ESPHome API and Assist Satellite entity capabilities?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

Home Assistant documents assist_pipeline/run stages and binary audio streaming. Assist Satellite entities add feature contracts such as announcements; not automatically provided by raw pipeline clients.

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Prototype the documented Assist WebSocket pipeline. Document auth, failure recovery, timeouts, cancellation, barge-in and multi-satellite competition. Decide ESPHome compatibility later based on feature needs.

## Planned experiments

- [EXP-007](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
