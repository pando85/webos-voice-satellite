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
| [EXP-013](EXP-013-ha-fresh-setup.md) | Fresh "add to Home Assistant" on c3-b with 0.4.1 (delete the old entry, QR, phone page, pairing window, ESPHome add with host and port) and what Home Assistant restores (entity ids, area, aliases, Assist exposure); prepared 2026-11-05, not run; restore tool `tools/ha-restore/` | Phone setup / install UX |
| [P2-corpus](P2-corpus.md) | Phase 2 test corpus and metric definitions | Phase 2 |
| [P2-aec-offline-ab](P2-aec-offline-ab.md) | Offline A/B of echo cancellation, gain and NS for wake word and STT over TV content (semi-synthetic) | Phase 2 / F5 |
| [P2-aec-tuning](P2-aec-tuning.md) | Offline AEC3 tuning (suppressor, echo model, `linear_mix`, stereo render, NS/RNNoise/AGC) and wake threshold/trigger-level sweep, voice-split held-out (semi-synthetic) | Phase 2 / F5 (parked) |

**Reserved IDs:** EXP-012 is reserved by [ADR 0009](../decisions/0009-phone-setup-web-server.md) (phone setup probe). EXP-013 was taken on 2026-11-05 by the Home Assistant fresh-setup test above, so the music plan's four experiments (EXP-013 to EXP-016 in its text) move to the next free numbers when written; they were claimed by the [music playback plan](../design/music-playback.md#g-on-target-experiments) (resources, audible mix, wake and barge-in with music, real HA and Music Assistant path); each file is written when its milestone starts, and the IDs are renumbered there if they are taken by then.
