# Experiment protocol

All experiments start from a **read-only** baseline. The user must explicitly approve audio recording, changing settings, rebooting, service tracing, persistent installation, or invasive reversing.

## Execution rules

1. Create an experiment record from [TEMPLATE.md](TEMPLATE.md); include bounded question, exact device and firmware, and risk class.
2. Collect a system inventory first (see [probe](../../tools/probe/README.md)).
3. State the expected evidence and what would *disprove* the hypothesis before running commands.
4. Do not collect ambient PCM without explicit consent. Use short controlled phrases and delete files.
5. Record observations separately for each TV, input source, power state and firmware version.
6. Redact raw outputs; never commit any full diagnostic dump, recording, credential or proprietary binary.
7. Update the associated research document and compatibility matrix, leaving conclusions `INCONCLUSIVE` where appropriate.

## Initial experiment catalogue

| ID | Experiment | Phase | Gate |
| --- | --- | --- | --- |
| EXP-001 | Runtime, ALSA and service inventory | 0 | F0 |
| EXP-002 | Built-in microphone mapping and PCM capture | 1 | F1 |
| EXP-003 | Concurrent LG services, DSP and TV playback | 1 | F2 |
| EXP-004 | Lifecycle: apps, screen off, Always Ready, standby | 0/3 | F0/F6 |
| EXP-005 | TTS/native mixing with Netflix, HDMI, eARC | 3 | F4 |
| EXP-006 | Echo reference and AEC feasibility | 3 | F5 |
| EXP-007 | Wake word and Assist roundtrip | 2 | F3 |
| EXP-008 | AirPlay/Music Assistant playback | 3 | F8 |

Create separate dated results per model/firmware, not one global 'pass'.
