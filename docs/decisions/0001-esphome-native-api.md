# ADR 0001: Integrate with Home Assistant through the ESPHome native API

- **Status:** Accepted (2026-11-01, owner)
- **Scope:** transport and integration route between the TV satellite and Home Assistant (HA).
- **Related:** [ADR 0002](0002-rust-implementation.md), [ADR 0003](0003-server-side-wake-word-for-mvp.md), [esphome-rust-satellite research](../research/esphome-rust-satellite.md), [research 05](../research/05-home-assistant.md)

## Context

- The earlier proposal left open "Assist WebSocket versus full ESPHome protocol" ([architecture overview](../architecture/overview.md), [research 05](../research/05-home-assistant.md)). Research 05 suggested prototyping the Assist WebSocket first and deciding on ESPHome later. This ADR **supersedes that suggestion**.
- The owner runs Home Assistant with Wyoming services (openWakeWord, Whisper) in a homelab. An offline check showed STT works on TV-mic audio and the `alexa` wake word fires at higher playback levels ([D3 offline check](../discovery/c3-a/D3-mic-identification.md#offline-ha-pipeline-check-wyoming-2026-11-01)). The pipeline itself has **not** been run end to end with the TV.
- The TV runs a 32-bit soft-float userland with limited memory ([D0-D1](../discovery/c3-a/D0-D1-runtime.md)).
- The ESPHome integration in HA provides a native Assist satellite entity, device discovery, optional Noise encryption and handling of voice-assistant messages. Protocol details and the open implementation questions are in the [research document](../research/esphome-rust-satellite.md).

## Decision

The satellite implements the **ESPHome native API server**. HA connects to the TV (not the other way round) and exposes a native Assist satellite entity. STT, conversation and TTS stay in HA, as already assumed for the MVP. Encryption uses the ESPHome Noise mechanism with a per-device key held in private configuration.

## Consequences

- No custom HA integration is required: the owner adds the device in the HA UI.
- The TV must accept an inbound connection on a local port. Network exposure and key handling become part of the security model ([research 08](../research/08-security-quality.md)).
- The satellite must implement framing, the Noise handshake, device info, entity listing and the voice-assistant message flow. It can be tested against `aioesphomeapi` acting as a fake HA (roadmap 1.3).
- Behaviour that is defined by HA's ESPHome integration, such as wake word handling, announcements and timers, is constrained by what that integration exposes.
- A protocol or HA-version change can break the satellite; the supported HA and protocol versions need to be recorded in the compatibility documents.

## Alternatives considered

- **Assist pipeline WebSocket client:** authenticates with an HA token held on the TV, and the satellite would not appear as a native Assist satellite entity. Not chosen by the owner for the MVP.
- **Wyoming satellite protocol:** a mature satellite route, but it requires an additional integration path and the owner chose ESPHome. Not evaluated on the TV.
- **Custom HA integration or custom component:** extra code in the HA instance with an ongoing maintenance burden.
- **LG-native voice services:** not used; hands-free ties up the mic ([ADR 0004](0004-mvp-capture-device-and-lg-handsfree-off.md)).

## Revisit when

- A required feature (barge-in, continue-conversation, multi-satellite arbitration, announcements) is not available through the ESPHome integration.
- The Noise or protocol implementation cost on the TV proves larger than expected in roadmap items 1.3 to 1.5.
- HA changes the ESPHome voice-assistant protocol incompatibly.
