# Source and evidence ledger

Consulted: 2026-11-01. These are references and hypotheses, **not** claims that interfaces work on the target TVs.

| Source | Scope | Evidence level | Use |
| --- | --- | --- | --- |
| [webosbrew Homebrew Channel](https://github.com/webosbrew/webos-homebrew-channel) | Startup hooks, root exec, developer tooling | DOCUMENTED for project; per-TV confirmation needed | Runtime/deployment |
| [own-your-glass](https://github.com/JulioFerrero/own-your-glass) | ALSA capture and LG voice processes on author's LG TV | OBSERVED_OTHER_MODEL | Capture discovery, not ALSA index copy |
| [CleanYourTV voice guide](https://lasharor.github.io/cleanyourtv-WebOS/voice/) | Magic Remote `voiceinput`, local Moonshine and Assist text API | OBSERVED_OTHER_MODEL | Existing voice path, reference implementation |
| [CleanYourTV AGENTS](https://github.com/lasharor/cleanyourtv-WebOS/blob/dev/AGENTS.md) | TV-specific read-only diagnostics and agent rules | DOCUMENTED | Agent workflow precedent |
| [Home Assistant Assist pipelines](https://developers.home-assistant.io/docs/voice/pipelines/) | WebSocket pipeline, stages, streaming audio | DOCUMENTED | Assist transport |
| [Home Assistant Assist Satellite](https://developers.home-assistant.io/docs/core/entity/assist-satellite/) | Satellite entity, announcements and configuration | DOCUMENTED | Post-MVP API requirements |
| [OHF Voice Linux Voice Assistant](https://github.com/OHF-Voice/linux-voice-assistant) | Linux voice satellite implementation | DOCUMENTED | Behavior and test inspiration |
| [webOS Homebrew native toolchain](https://github.com/webosbrew/native-toolchain) | Native cross compilation | DOCUMENTED | Target portability |
| [REA](https://github.com/morluto/rea) | Optional proprietary binary/runtime reversing | DOCUMENTED | Only after bounded unknowns |
| [Music Assistant](https://music-assistant.io/) | Music controller / AirPlay ecosystem | DOCUMENTED | Separate media transport |
| [webOS OSE Audio](https://www.webosose.org/docs/reference/ls2-api/com-webos-service-audio/) | OSE audio APIs | DOCUMENTED **OSE only** | Hypothesis, never commercial-TV proof |

## Reference policy

For each new source record the source URL, date inspected, target versions covered, and what the source actually proves. Cite exact files, APIs or lines when possible. Link every `TARGET_VERIFIED` claim to an experiment and its redacted evidence.

Do not vendor REA, vendor binaries, copy proprietary LG artifacts, or reproduce entire external documents here. Respect upstream licenses.
