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
| [CleanYourTV `helper/voice.js`, `helper/notify.js`, `helper/keynames.js`](https://github.com/lasharor/cleanyourtv-WebOS/tree/dev/helper) (branch `dev`, commit `e524ac8`, read 2026-11-07) and [PR 184](https://github.com/lasharor/cleanyourtv-WebOS/pull/184) | `voiceinput/startStreaming` remote stream (`socketPath`, 16 kHz S16 mono, `errorCode` 10000 until the mic button is held), `voiceconductor` paused with SIGSTOP, mic key 428 `KEY_VOICE` via an `lginput2` hook, `topui` overlay app; OLED65G26LA and 65UR91006LB, webOS 25 | OBSERVED_OTHER_MODEL | [Remote mic button](remote-mic-button.md) |
| [LG Input Mapper](https://github.com/fivefold3/lginputmapper) (commit `fca03ae`, read 2026-11-07) | `EVIOCGRAB` of `LGE M-RCU - Builtin [0]` with re-emit into `[2]`, `--dry-run`, key table (428 `AI / Voice`); C5, webOS 10.3, MR25GA | OBSERVED_OTHER_MODEL | [Remote mic button](remote-mic-button.md) |
| [own-your-glass `docs/FINDINGS.md`](https://github.com/JulioFerrero/own-your-glass/blob/main/docs/FINDINGS.md) F5, F37 (commit `f6fd6f2`, read 2026-11-07) | Magic Remote mic as Bluetooth HID on `/dev/hidraw0` read by `voiceinput_hidraw`; `NL_BUTTON_CLICK` `KEY_VOICE` in the system log; plaintext voice transcripts in the log; webOS 10.3.1 | OBSERVED_OTHER_MODEL | [Remote mic button](remote-mic-button.md) |
| [LG MR22GN manual](https://lg.com/content/dam/channel/wcms/mx/download/mr22_manual.pdf), [MR23GA user information](https://www.manualslib.com/manual/3399539/Lg-Mr23ga.html) | "Speak while pressing and holding the button"; 2.400 to 2.4835 GHz, 40 channels | DOCUMENTED (remote models, not our units) | [Remote mic button](remote-mic-button.md) |
| [HA core `esphome/assist_satellite.py`](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/assist_satellite.py) at 2026.10.4, aioesphomeapi 46.6.0 `client.py` | Start stage STT without `USE_WAKE_WORD`; `VoiceAssistantAudio{end}` stops, `VoiceAssistantRequest{start:false}` aborts | DOCUMENTED | [Remote mic button](remote-mic-button.md) |
| [webOS OSE Audio](https://www.webosose.org/docs/reference/ls2-api/com-webos-service-audio/) | OSE audio APIs | DOCUMENTED **OSE only** | Hypothesis, never commercial-TV proof |

## Reference policy

For each new source record the source URL, date inspected, target versions covered, and what the source actually proves. Cite exact files, APIs or lines when possible. Link every `TARGET_VERIFIED` claim to an experiment and its redacted evidence.

Do not vendor REA, vendor binaries, copy proprietary LG artifacts, or reproduce entire external documents here. Respect upstream licenses.
