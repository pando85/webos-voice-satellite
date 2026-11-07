# Experiment protocol

All experiments start from a **read-only** baseline. The user must explicitly approve audio recording, changing settings, rebooting, service tracing, persistent installation, or invasive reversing.

## Execution rules

1. Use the numbered protocols below and [TEMPLATE.md](TEMPLATE.md) for dated results; record one device/firmware/state per result.
2. State falsifiable hypotheses, signals, failure interpretations and safe rollback before running commands.
3. Use the read-only [probe](../../tools/probe/README.md) for baseline observations.
4. Do not collect ambient PCM without consent. Prefer short controlled phrases and delete files afterward.
5. Redact identifiers and do not commit raw dumps, firmware, logs with private identifiers, audio or secrets.
6. Update research, compatibility and linked issue based on the actual outcome; keep `INCONCLUSIVE` outcomes visible.

| ID | Experiment | Gate |
| --- | --- | --- |
| [EXP-001](EXP-001-runtime-inventory.md) | Runtime and audio inventory | F0 |
| [EXP-002](EXP-002-microphone-mapping.md) | Built-in microphone mapping and PCM capture | F1 |
| [EXP-003](EXP-003-concurrency.md) | Concurrent LG services, DSP and TV playback | F2 |
| [EXP-004](EXP-004-power-lifecycle.md) | Apps, screen off, Always Ready and standby | F0/F6 |
| [EXP-005](EXP-005-native-playback.md) | TTS and native audio mixing | F4 |
| [EXP-006](EXP-006-aec-reference.md) | Echo reference and AEC feasibility | F5 |
| [EXP-007](EXP-007-assist-pipeline.md) | Wake word and Assist roundtrip | F3 |
| [EXP-008](EXP-008-airplay-media.md) | Music Assistant and AirPlay. **Not planned** (owner decision 2026-11-05, [ADR 0010](../decisions/0010-music-playback.md)) | F8 |
| [EXP-009](EXP-009-ipk-install-update.md) | App package install, update, autostart and removal | Phase 3 / 5 |
| [EXP-010](EXP-010-tv-control-setters.md) | TV control setters (screen, volume, mute, input, app, power) on c3-b; Part A run 2026-11-04 (passed, A5 skipped), Part B not run | Backlog #11 |
| [EXP-011](EXP-011-lg-settings-toggle.md) | Read and toggle Always Ready (`lifeOnScreenMode`) and LG hands-free (`setActivation`) through luna; c3-a run 2026-11-05 (passed), c3-b not reachable | TV-app settings screen |
| [EXP-012](EXP-012-phone-setup-probe.md) | Phone setup web server: on-target probe (firewall, inbound reachability, web engine, exec survival and polling cost, log leakage); Part A run on c3-a 2026-11-05 (passed), Part B (owner's phone) not run | Backlog #15 |
| [EXP-013](EXP-013-ha-fresh-setup.md) | Fresh "add to Home Assistant" on c3-b with 0.4.2 (delete the old entry, QR, phone page, pairing window, ESPHome add with host and port) and what Home Assistant restores (entity ids, area, aliases, Assist exposure); run 2026-11-05 (key cleared on delete, no key typed, everything restored by HA itself; owner's text feedback not given); restore tool `tools/ha-restore/` | Phone setup / install UX |
| [EXP-014](EXP-014-music-mix-and-ducking.md) | Music mix and ducking on c3-a (announcement and timer alarm over music, duck depth and timing, restore, pause, volume, STOP order); run 2026-11-05 (passed for the tested sources; wake-word source and listening not tested) | F8 / music M7, gate G3 |
| [EXP-016](EXP-016-music-real-ha-and-ma.md) | Real Home Assistant 2026.10.4 path for music on c3-a (flags, browse, proxy URL and format, `ENQUEUE`, two-conversion limit, `bypass_proxy`, pause, stop); run 2026-11-05 (HA passed, Music Assistant not installed: inconclusive) | F8 / music M2b and M6, gates G0b and G5 |
| [EXP-017](EXP-017-music-engine-resources.md) | Music engine resources on c3-a (CPU per thread, PulseAudio, RSS, underruns, mic dropouts, pause timeout; the music plan's "EXP-013", renumbered); run 2026-11-05 (passed; standby entry not tested) | F8 / music M6, gate G2 |
| [EXP-018](EXP-018-aec-live-c3-a.md) | Live echo-cancellation A/B on c3-a (OFF, AEC3 default, tuned "B", B + NS; JBL talker at the owner's level, TV content at volume 10 and 15, HA's real pipeline), delay lock, ERLE, CPU, music in the reference; run 2026-11-05 (directional gains over content, not significant; lock, CPU and music-in-reference verified) | Backlog #1 |
| EXP-019 (moved) | Small local models on the cluster GTX 1060 for Kivi (router, STT corrector, home-control agent) versus non-LLM fixes (Whisper prompt, built-in intents): now in the homelab repository, [docs/voice-assistant](https://github.com/pando85/homelab/blob/master/docs/voice-assistant/experiments/small-local-models.md); scripts `scripts/voice-assistant/small_models/` | Backlog #12 |
| EXP-020 (moved) | Conversation agent evaluation for "Kivi" (Kiais gateway metrics, dry-run harness, built-in intents): now in the homelab repository, [docs/voice-assistant](https://github.com/pando85/homelab/blob/master/docs/voice-assistant/experiments/conversation-agent-eval.md); harness `scripts/voice-assistant/agent_eval/` | Backlog #12 |
| [EXP-021](EXP-021-aec-live-off-vs-bns.md) | Live echo-cancellation A/B on c3-a, OFF against B + NS at TV volume 10 (counterbalanced ABBA, 40 items per arm, JBL talker at the owner's level, HA's real pipeline); run 2026-11-05 (rejected: 61 % against 59 %, paired p 1.0; no harm, 0 wake-word false wakes; STT inconclusive). Filed as EXP-021 because EXP-020 is taken by the moved Kivi evaluation; its private data is under `EXP-020` | Backlog #1 |
| [EXP-022](EXP-022-remote-mic-button-observe.md) | Magic Remote mic button: passive evdev trace of the press and release (no grab), LG's voice route (`voiceinput_hidraw`, hidraw holder), the `voiceinput/startStreaming` schema and log signal (Part A, read-only); the remote's voice stream through `voiceinput` counted in memory (Part B, consented); **planned, not run** (needs owner authorization) | Owner request: mic button push-to-talk |
| [P2-corpus](P2-corpus.md) | Phase 2 test corpus and metric definitions | Phase 2 |
| [P2-aec-offline-ab](P2-aec-offline-ab.md) | Offline A/B of echo cancellation, gain and NS for wake word and STT over TV content (semi-synthetic) | Phase 2 / F5 |
| [P2-aec-tuning](P2-aec-tuning.md) | Offline AEC3 tuning (suppressor, echo model, `linear_mix`, stereo render, NS/RNNoise/AGC) and wake threshold/trigger-level sweep, voice-split held-out (semi-synthetic) | Phase 2 / F5 (parked) |

**Reserved IDs:** EXP-012 is reserved by [ADR 0009](../decisions/0009-phone-setup-web-server.md) (phone setup probe). EXP-013 was taken on 2026-11-05 by the Home Assistant fresh-setup test above, so the music plan's resources experiment ("EXP-013" in its text) is filed as **EXP-017**; its EXP-014 (mix and ducking) and EXP-016 (real HA and Music Assistant) keep their numbers, EXP-015 (false wakes and barge-in with music) is not written or run yet; they were claimed by the [music playback plan](../design/music-playback.md#g-on-target-experiments) (resources, audible mix, wake and barge-in with music, real HA and Music Assistant path); each file is written when its milestone starts, and the IDs are renumbered there if they are taken by then.
