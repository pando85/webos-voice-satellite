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
| [EXP-008](EXP-008-airplay-media.md) | Music Assistant and AirPlay | F8 |
| [EXP-009](EXP-009-ipk-install-update.md) | App package install, update, autostart and removal | Phase 3 / 5 |
| [EXP-010](EXP-010-tv-control-setters.md) | TV control setters (screen, volume, mute, input, app, power) on c3-b; Part A run 2026-11-04 (passed, A5 skipped), Part B not run | Backlog #11 |
| [P2-corpus](P2-corpus.md) | Phase 2 test corpus and metric definitions | Phase 2 |
| [P2-aec-offline-ab](P2-aec-offline-ab.md) | Offline A/B of echo cancellation, gain and NS for wake word and STT over TV content (semi-synthetic) | Phase 2 / F5 |
| [P2-aec-tuning](P2-aec-tuning.md) | Offline AEC3 tuning (suppressor, echo model, `linear_mix`, stereo render, NS/RNNoise/AGC) and wake threshold/trigger-level sweep, voice-split held-out (semi-synthetic) | Phase 2 / F5 (parked) |
