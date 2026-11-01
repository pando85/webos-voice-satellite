# webOS Voice Satellite

An experimental, local-first Home Assistant voice satellite for **rooted LG webOS TVs**, using their built-in far-field microphones and speakers when accessible.

> **Status: feasibility research.** Internal microphone PCM access, simultaneous capture/playback, and power-state behavior are **not yet verified on the target TVs**. This is not a working satellite.

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
- No production daemon, release binaries, or claims of device compatibility until experiments pass.

## Where to start

1. Read [AGENTS.md](AGENTS.md) and [the roadmap](docs/project/roadmap.md).
2. Read [research index](docs/research/README.md) and [source ledger](docs/research/sources.md).
3. For each TV, run the read-only [probe](tools/probe/README.md) and keep the results **private** until redacted.
4. Follow [experiment protocol](docs/experiments/README.md); file device-specific findings in [compatibility](docs/compatibility/README.md).
5. Implement only after the [feasibility gates](docs/project/requirements.md) have supporting evidence.

**Architecture:** [overview](docs/architecture/overview.md) · **Agent skills:** [.agents/skills](.agents/skills) · **CI:** [documentation checks](.github/workflows/validate.yml)

This is an independent community project, not endorsed by LG or Home Assistant.
