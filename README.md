# webOS Voice Satellite

An experimental, local-first Home Assistant voice satellite for **rooted LG webOS TVs**, using their built-in far-field microphones and speakers when accessible.

> **Status (2026-11-01): discovery done for the microphone; Phase 1 (silent build and integration) in progress.** On one LG C3 (c3-a, webOS TV 8.3.0, firmware 03.30.74), the built-in mic was identified as ALSA `hw:LGSIC,0` and its capture was characterized, and Whisper STT and the openWakeWord `alexa` model were checked offline on TV-mic audio. Not yet verified: any satellite-to-Home-Assistant roundtrip, audible replies, behaviour while the TV plays sound, and power states. **This is not a working satellite.** Decisions so far: ESPHome native API, Rust, server-side wake word, capture from `hw:LGSIC,0` with LG hands-free off ([ADRs](docs/decisions/README.md)).

**Plan and evidence:** [roadmap](docs/project/roadmap.md) · [backlog](docs/project/backlog.md) · [decisions](docs/decisions/README.md) · [discovery logs](docs/discovery/README.md)

## Goals

- Hands-free voice commands through the TV's built-in microphones, without the Magic Remote.
- Home Assistant Assist for wake-word detection, speech recognition, conversation, and text-to-speech (TTS).
- Responses using the TV's native audio stack without disrupting normal TV playback when supported.
- Operation with the screen off if technically feasible and power-efficient.
- Support multiple LG models/firmware versions through capability discovery, not hard-coded ALSA card numbers.
- Optional music integration through existing receivers (AirPlay/other supported players), not a bespoke music engine.

## Boundaries

- Rooted devices owned or authorized for testing only. No rooting or privilege-escalation implementation.
- No cloud requirement or continuous recording by default.
- Do not disable LG voice services, patch firmware, modify system partitions, or install persistent boot hooks during discovery.
- No release binaries or claims of device compatibility until experiments pass. Development binaries are run only with owner authorization and are never committed.

## Where to start

1. Read [AGENTS.md](AGENTS.md) and [the roadmap](docs/project/roadmap.md).
2. Read the [discovery logs](docs/discovery/README.md) and the [decision records](docs/decisions/README.md), then the [research index](docs/research/README.md) and [source ledger](docs/research/sources.md).
3. For each TV, run the read-only [probe](tools/probe/README.md) and keep the results **private** until redacted.
4. Follow [experiment protocol](docs/experiments/README.md); file device-specific findings in [compatibility](docs/compatibility/README.md).
5. Implement in the order of the [roadmap](docs/project/roadmap.md); each phase closes against the [feasibility gates](docs/project/requirements.md) and their evidence.

**Development:** [build, cross-compile and deploy](docs/development.md) · **Architecture:** [overview](docs/architecture/overview.md) (provisional, partly superseded by the ADRs) · **Backlog:** [post-Phase-5 ideas](docs/project/backlog.md) · **Agent skills:** [.agents/skills](.agents/skills) · **CI:** [documentation checks](.github/workflows/validate.yml), [Rust](.github/workflows/rust.yml)

This is an independent community project, not endorsed by LG or Home Assistant.
