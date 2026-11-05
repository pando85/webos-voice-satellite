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
- Optional music playback through the satellite's Home Assistant media player (owner decision 2026-11-05, [ADR 0010](docs/decisions/0010-music-playback.md); design only, not built). AirPlay is not used.

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

## Verified TVs

The microphone starts on every TV and firmware; the daemon, Home Assistant and the TV app only say how well known the combination is. Matching is by **series and firmware** ([how it works](docs/compatibility/verified-models.md)). Nothing here has run with the soft policy on a TV yet (implemented 2026-11-04, host tests only).

| Series | Tested firmware | Units |
| --- | --- | --- |
| LG OLED C3 (2023, webOS 8.x) | `03.30.74`, `03.30.73` | c3-a (`OLED42C34LA`), c3-b (`OLED55C36LC`) |

- **Same series, other firmware or size:** "series verified, firmware not yet reported".
- **Anything else is "not verified"**: that means nobody has reported it yet, not that it does not work. The microphone still starts; the daemon checks that audio flows and has a sane level, and closes the microphone with a visible `capture problem` status if it measurably does not.
- **Report your TV, working or not:** open the TV app, press "Report compatibility" and scan the QR code (or run `ctl.sh report` over SSH). It opens a prefilled GitHub issue; you review it and press Submit. The report holds only the TV model, webOS and firmware, architecture, audio device ids, check results and levels, power states and TV control results; never a serial number, MAC or IP address, device or Home Assistant name, key, audio or transcript. The satellite sends nothing by itself.
- Want the old behaviour (microphone off on unlisted firmware)? Set `[safety] firmware_policy = "strict"` and `validated_firmware`.

## License

GPL-3.0-or-later. The full text is in [LICENSE](LICENSE). Third-party dependencies and their licenses are audited in the [packaging design](docs/design/packaging.md#8-license-decision-and-dependency-audit). The vendored ESPHome `.proto` files keep their MIT notice.

This is an independent community project, not endorsed by LG or Home Assistant.
