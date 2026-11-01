# ADR 0003: Server-side wake word for the MVP

- **Status:** Accepted (2026-11-01, owner)
- **Scope:** where wake word detection runs for the MVP.
- **Related:** [ADR 0001](0001-esphome-native-api.md), [ADR 0004](0004-mvp-capture-device-and-lg-handsfree-off.md), [research 04](../research/04-activation.md), [backlog](../project/backlog.md)

## Context

- Wake word location was an open question in [research 04](../research/04-activation.md) and the [architecture overview](../architecture/overview.md).
- The owner's HA already runs openWakeWord and Whisper over Wyoming. Offline results on TV-mic audio ([D3 offline check](../discovery/c3-a/D3-mic-identification.md#offline-ha-pipeline-check-wyoming-2026-11-01)):
  - Whisper large-v3-turbo transcribed the TV-mic captures word-perfectly in the best variants.
  - The openWakeWord `alexa` model detected the wake word in 4 of 4 variants at +12 dB playback gain, was borderline at +6 dB, and did not detect it at -30 dBFS playback. These are single runs with a TTS source, not human voices, so **no detection rate is established**.
- Privacy is not a driver: the audio stays on the home network and in the owner's own HA ([AGENTS.md](../../AGENTS.md) still requires TLS or Noise and restricted access).
- The TV has limited CPU and memory budget ([constraints](../discovery/c3-a/rooted-tv-improvements.md#constraints-not-changes)) and the on-TV wake word runtimes are not yet evaluated for the 32-bit softfp target.

## Decision

For the MVP the wake word runs **server-side in HA (openWakeWord)**. The satellite streams captured audio to HA through the ESPHome voice-assistant flow ([ADR 0001](0001-esphome-native-api.md)). During Phase 1 the pipeline is also triggered manually (push-to-talk style) to separate transport tests from wake word tests. On-TV wake word is a later improvement for latency and efficiency.

## Consequences

- The satellite streams audio to HA continuously while listening. Bandwidth and HA load are higher than with an on-TV detector, and the microphone-active indicator and mute control matter more (roadmap Phase 4, requirement F7).
- Wake accuracy depends on HA's openWakeWord model and on level and front-end quality. A software gain stage before the wake engine is worth keeping, since D3 showed detection is level-sensitive near the threshold.
- Custom wake words are trained and deployed through the owner's homelab GitOps repository, not in this repository.
- Which wake word name the HA pipeline requests is **not yet verified** (the server lists `alexa`; `alexa_v0.1` returned no detections). It must be recorded in roadmap 1.4.
- Network loss means no wake detection.

## Alternatives considered

- **On-TV wake word (microWakeWord or openWakeWord) in the MVP:** lower latency and bandwidth, but it needs a runtime on the 32-bit target, CPU measurements and a model pipeline before any end-to-end evidence exists. Deferred to the [backlog](../project/backlog.md).
- **LG native hands-free activation:** conflicts with exclusive use of the mic PCM ([ADR 0004](0004-mvp-capture-device-and-lg-handsfree-off.md)) and is not under our control.
- **Push-to-talk only:** useful for testing but not the product goal.

## Revisit when

- Phase 2 measurements show unacceptable false accepts or missed wakes, or latency that a local detector would fix.
- Streaming load or latency to HA is a practical problem.
- A custom wake word needs to run when HA is unreachable.
