# P2 offline A/B: echo cancellation, gain staging and NS for wake word and STT over TV content

**Status:** INCONCLUSIVE for a talker at the rig reference level (no variant restores wake detection over content); positive for a talker 10 dB louder and for STT (the linear AEC3 output). PC only, semi-synthetic.
**Date:** 2026-11-02
**Device alias / model / firmware / webOS / architecture:** recordings from c3-a (LG C3, firmware 03.30.74, webOS TV 8.3.0, TV speaker, `aiSoundPlus`); processing on the PC (x86_64), armv7 build cross-checked under `qemu-arm`
**Power and playback state:** as recorded in [P2 session 2](../discovery/c3-a/P2-session2-wake-accuracy.md#4-aec-recording-set-and-offline-baseline): "Screen Saver" over HDMI 1, content set `p2-v1` at TV volume 10 and 15, and a quiet room
**Related research / requirements / issue:** [AEC front-end design](../design/aec-frontend.md) (section 9 step 3, the offline half), [AEC candidates](../research/aec-candidates.md), [P2 corpus and metrics](P2-corpus.md), [backlog item 1](../project/backlog.md#1-professional-audio-front-end)
**Risk:** none (PC only; no TV, no playback, no Home Assistant)
**Approval needed / obtained:** none needed; the recordings were made with the owner's authorization in session 2

Labels follow [AGENTS.md](../../AGENTS.md). Every rate below is **observed-on-other-device, semi-synthetic**: clean Piper speech added digitally to real TV-mic recordings, processed on the PC by the daemon's own front-end code. No talker was in the room, so room acoustics, distance and the rig loudspeaker are absent. The canceller's behaviour on the recordings is **verified-on-host, device recordings**. Nothing here ran on a TV.

## Question and falsifiable hypothesis

Does echo cancellation (with gain staging and optional noise suppression) restore wake word detection and STT for a talker while the TV plays content, and which `[aec]` settings should the live A/B on the TV start from?

- H1: with content at volume 10, the default canceller at least doubles the wake rate of no AEC (11 %) and does not lower the quiet-room rate (43 %). **Rejected** at the reference level (12 %), **supported** only for the linear output (21 %) and for a talker 10 dB louder.
- H2: more gain after the canceller (+30, +34 dB) raises detection. **Rejected** (within one or two items everywhere).
- H3: NS helps or is neutral. **Supported** in the quiet room, neutral with content.
- H4: a two-channel reference improves stereo content. **Supported** for ERLE on stereo content, **not** for wake rates at the reference level; it lowers ERLE on near-mono test signals.

## Method

1. **Semi-synthetic near-end** (`tools/audio-rig/aec_eval/semisyn_ab.py`). All 97 positives of corpus `p2-v1` (every voice) were added to the **raw** mic of the session 2 AEC recordings exactly as session 2 mixed them for Home Assistant: the same bed offset per item (checked to 1 ms against session 2's rows), 2 s lead, 3 s tail, the item at -39 dB re full scale (the rig reference level from D3, about -61 dBFS active RMS at the mic), added to both mic channels before any processing. The `SoundEngineCapture` reference was left untouched. Each mix ran through `aec_offline` (the daemon's file-pair reader, 256-frame reads, `Frontend` with the canceller) starting 10 s before the item, so the canceller had converged as during continuous playback; the output was cut to the item's segment with the canceller's latency removed. Beds: `aec/quiet` (150 s, TV silent), `aec/content-v10` and `aec/content-v15` (the 10-minute sampler). A second set used the speech 10 dB louder (-29 dB), as session 2 did for the quiet room.
2. **Variants** (flags of `aec_offline`; daemon `[aec]` keys in brackets):
   - `noaec`: no canceller, +24 dB (what shipped; identical to session 2's Python emulation, see calibration);
   - `aec`: sonora AEC3, default (`enabled = true`), +24 dB after it; `aec+30`, `aec+34`: more gain after it (the chain has no limiter or AGC, so the gain saturates);
   - `ns`: AEC3 plus sonora's NS, moderate (`ns = true`);
   - `render2`: a two-channel reference (`render_channels = 2`, new, see below);
   - `linear`: AEC3's linear filter output without its residual-echo suppressor (`output = "linear"`, new); `nearend`: AEC3's full output with its near-end suppressor tuning all the time (`output = "nearend"`, new); each also with NS and +30 dB;
   - diagnostics: `oracle-aec` and `oracle-render2` run the canceller on the bed **without** the speech and add the high-passed speech to its output afterwards, so the talker never passes the canceller (what the canceller's echo removal would allow if it left the talker untouched).
3. **Wake scoring offline** (`oww_wyoming.py`): HA runs Wyoming openWakeWord 2.1.0. Its source (tag v2.1.0) uses `pyopen-wakeword` 1.x, which resolves to 1.1.0 (TFLite models and a bundled `libtensorflowlite_c`, not the `openwakeword` package's ONNX models that `oww_offline.py` uses). The scorer repeats the server's handler loop: features and model reset per audio stream (with the 8 s zero autofill), 1024-sample chunks, a detection when the probability is **strictly above** 0.5, trigger level 1, 2 s refractory time, timestamp = audio time before the triggering chunk; model `alexa` from the wheel. Detections are scored with the P2-corpus window (wake start -0.2 s to wake end +1.5 s).
4. **False wakes**: (a) `content_eval.py` runs each variant over the whole content-only recordings (10 min each at volume 10 and 15, 2.5 min quiet) as one long wake run, re-armed 2 s after a detection; (b) the 28 negatives and 14 near-misses mixed into all three beds at both levels.
5. **STT** (`stt_eval.py`): faster-whisper `large-v3-turbo`, language `es`, beam size 1, no VAD, on the P2-corpus command segment (command start -0.5 s to item end +0.3 s) of all 70 `pos_cmd` items per condition and variant, independent of the wake result; WER with the P2-corpus normalization (v1, and v1.1 for number formats).
6. **Statistics**: Wilson 95 % intervals for rates; paired comparisons on the same items with an exact two-sided McNemar test (uncorrected for the many comparisons, so treat p between 0.01 and 0.05 as weak); exact Poisson intervals (rule of three for zero) for false wakes per hour.

### Code added for the variants (default behaviour unchanged)

- `[aec] render_channels = 1 | 2` (default 1). The brief asked for `reference_channels = 1|2`, but `reference_channels` already means the captured channel count, so the new key is separate. With 2, the first two reference channels go to AEC3 as a stereo render: sonora supports multichannel render (`StreamConfig` render channels; AEC3 detects stereo content itself and switches to its multichannel configuration, **documented**, sonora-aec3 `multi_channel_content_detector.rs`, `config_selector.rs`). The delay estimator, the bypass and the metrics keep using the downmix. Tests: two different echo paths (stereo render beats the downmix by more than 6 dB and reaches more than 15 dB), dual mono (both modes equal within 1 dB), bypass transparency, validation. If the device delivers fewer channels the daemon falls back to the downmix.
- `[aec] output = "full" | "linear" | "nearend"` (default `"full"`, experimental, mono render only). sonora's `AudioProcessing` exposes neither AEC3's linear output nor its suppressor tuning, so these drive sonora-aec3's `BlockProcessor` directly (`crates/dsp/src/aec/direct.rs`), as sonora's own `EchoCanceller3` does at 16 kHz. A unit test checks that this direct path with AEC3's default configuration is **sample-for-sample identical** to `AudioProcessing`, so the variants differ only by what they change. The linear output has 64 samples of latency (128 for the full output); NS, if on, runs after it through an `AudioProcessing` without echo canceller. Synthetic double talk (a noise talker 10 dB below the echo): the linear output keeps the talker at 0.0 dB, the full output at -6.4 dB.
- `aec_offline` gains `--render-channels` and `--output`. The armv7 build under `qemu-arm` gives the same ERLE as the host build (to 0.01 dB) for both. Static daemon binaries: armv7 3,066,596 bytes (+4,800 against the 3,061,796 of the first AEC build), aarch64 3,158,856 bytes (+5,328). On the host the stereo render costs about 15 % more per frame than the downmix (51 against 45 µs); the TV cost of the new variants was **not** measured (the default costs 11 % of a core on armv7, [P3](../discovery/c3-a/P3-tv-verify-aec-pairing.md)).

## Calibration against Home Assistant

The offline scorer was run on the same inputs session 2 streamed into HA (the digital items, and the semi-synthetic mixes through session 2's Python front-end emulation and through `aec_offline --no-aec`, which gave identical results).

| Condition | Offline | HA (session 2) | Items agreeing | Cohen's kappa |
| --- | --- | --- | --- | --- |
| Digital baseline | 58/97 | 58/97 | 97/97 | 1.00 |
| Quiet room | 42/97 | 42/97 | 97/97 | 1.00 |
| Content, volume 10 | 11/97 | 11/97 | 97/97 | 1.00 |
| Content, volume 15 | 1/97 | 0/97 | 96/97 | 0.00 (one item) |

- **Agreement: 387 of 388 items, the same items detected.** The one difference is a volume-15 item detected 1.0 s after its wake word ended (score 0.72) that HA did not report. Detection times in the item lead-in (content false wakes) also matched HA's to within 52 ms in 3 of 4 cases. The offline numbers below are therefore a close stand-in for HA's server on these inputs, not only relative values. (`oww_offline.py`, the python `openwakeword` 0.6.0 ONNX scorer, gave 37, 12 and 1 of 97 on the same sets in session 2.)

## Observations

### Wake detection, talker at the rig reference level (-39 dB, about -61 dBFS at the mic)

All 97 positives; "without MLS" excludes the two `mls` voices, which are near 0 even digitally (71 items).

| Variant | Quiet room | Content volume 10 | Content volume 15 | Volume 10 without MLS |
| --- | --- | --- | --- | --- |
| `noaec` (shipped) | 42/97 = 43 % [34, 53] | 11/97 = 11 % [6, 19] | 1/97 = 1 % [0, 6] | 11/71 = 15 % [9, 26] |
| `noaec+30` | 43/97 = 44 % [35, 54] | 12/97 = 12 % [7, 20] | 1/97 = 1 % [0, 6] | 11/71 |
| `aec` | 42/97 = 43 % [34, 53] | 12/97 = 12 % [7, 20] | 4/97 = 4 % [2, 10] | 12/71 = 17 % [10, 27] |
| `aec+30` | 43/97 = 44 % [35, 54] | 13/97 = 13 % [8, 22] | 4/97 = 4 % [2, 10] | 13/71 |
| `aec+34` | 43/97 = 44 % [35, 54] | 12/97 = 12 % [7, 20] | 4/97 = 4 % [2, 10] | 12/71 |
| `ns` | 49/97 = 51 % [41, 60] | 14/97 = 14 % [9, 23] | 4/97 = 4 % [2, 10] | 14/71 = 20 % [12, 30] |
| `render2` | 42/97 = 43 % [34, 53] | 10/97 = 10 % [6, 18] | 5/97 = 5 % [2, 12] | 10/71 |
| `render2+ns` | 49/97 = 51 % [41, 60] | 15/97 = 15 % [10, 24] | 7/97 = 7 % [4, 14] | 15/71 |
| `linear` | 42/97 = 43 % [34, 53] | **20/97 = 21 % [14, 30]** | 4/97 = 4 % [2, 10] | 20/71 = 28 % [19, 40] |
| `linear+30` | 43/97 = 44 % [35, 54] | **23/97 = 24 % [16, 33]** | 5/97 = 5 % [2, 12] | 23/71 = 32 % [23, 44] |
| `linear+ns` | **52/97 = 54 % [44, 63]** | 19/97 = 20 % [13, 29] | 6/97 = 6 % [3, 13] | 19/71 = 27 % [18, 38] |
| `linear+ns+30` | 51/97 = 53 % [43, 62] | 21/97 = 22 % [15, 31] | 7/97 = 7 % [4, 14] | 21/71 = 30 % [20, 41] |
| `nearend` | 42/97 = 43 % [34, 53] | 16/97 = 16 % [10, 25] | 3/97 = 3 % [1, 9] | 16/71 |
| `nearend+ns` | 51/97 = 53 % [43, 62] | 15/97 = 15 % [10, 24] | 3/97 = 3 % [1, 9] | 15/71 |
| *`oracle-aec` (diagnostic)* | *43/97* | *33/97 = 34 % [25, 44]* | *25/97 = 26 % [18, 35]* | *33/71 = 46 %* |
| *`oracle-render2` (diagnostic)* | *43/97* | *34/97 = 35 % [26, 45]* | *31/97 = 32 % [24, 42]* | *34/71 = 48 %* |

Paired against `noaec` on the same items (gained / lost, McNemar p): quiet `ns` 12/5 (p 0.14), `linear+ns` 15/5 (p 0.04); volume 10 `aec` 6/5 (p 1.0), `linear` 10/1 (p 0.01), `linear+30` 12/0 (p < 0.001), `nearend` 9/4 (p 0.27); volume 15 every real variant p ≥ 0.03 (at most 7 of 97 items).

### Wake detection, talker 10 dB louder (-29 dB, about -51 dBFS at the mic)

| Variant | Quiet room | Content volume 10 | Content volume 15 | Volume 15 without MLS |
| --- | --- | --- | --- | --- |
| `noaec` | 55/97 = 57 % [47, 66] | 33/97 = 34 % [25, 44] | 11/97 = 11 % [6, 19] | 11/71 = 15 % [9, 26] |
| `aec` | 55/97 = 57 % [47, 66] | 40/97 = 41 % [32, 51] | 15/97 = 15 % [10, 24] | 14/71 = 20 % [12, 30] |
| `ns` | 59/97 = 61 % [51, 70] | 43/97 = 44 % [35, 54] | 13/97 = 13 % [8, 22] | 13/71 |
| `render2` | 55/97 = 57 % [47, 66] | 44/97 = 45 % [36, 55] | 19/97 = 20 % [13, 29] | 19/71 = 27 % [18, 38] |
| `render2+ns` | 59/97 = 61 % [51, 70] | 41/97 = 42 % [33, 52] | 17/97 = 18 % [11, 26] | 17/71 |
| `linear` | 55/97 = 57 % [47, 66] | 45/97 = 46 % [37, 56] | **37/97 = 38 % [29, 48]** | 37/71 = 52 % [41, 63] |
| `linear+ns` | 59/97 = 61 % [51, 70] | **48/97 = 49 % [40, 59]** | 32/97 = 33 % [24, 43] | 32/71 = 45 % [34, 57] |
| `nearend` | 55/97 = 57 % [47, 66] | 44/97 = 45 % [36, 55] | 14/97 = 14 % [9, 23] | 14/71 |
| `nearend+ns` | 59/97 = 61 % [51, 70] | 44/97 = 45 % [36, 55] | 14/97 = 14 % [9, 23] | 14/71 |
| *`oracle-aec`* | *54/97* | *52/97 = 54 % [44, 63]* | *45/97 = 46 % [37, 56]* | *45/71 = 63 %* |

Paired: volume 15 `linear` against `noaec` 27/1 (p < 0.001), against `aec` 28/6 (p < 0.001); `linear+ns` against `aec` 24/7 (p 0.003); volume 10 `linear+ns` against `noaec` 19/4 (p 0.003), against `aec` 11/3 (p 0.06).

### Per voice (selected variants)

| Condition / variant | lessac | ryan | davefx | mls_10246 | mls_9972 | sharvard F | sharvard M | ald | claude |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| quiet, `noaec` | 3/3 | 3/3 | 6/13 | 0/13 | 0/13 | 8/13 | 5/13 | 11/13 | 6/13 |
| quiet, `linear+ns` | 3/3 | 3/3 | 8/13 | 0/13 | 0/13 | 9/13 | 9/13 | 11/13 | 9/13 |
| volume 10, `noaec` | 0/3 | 1/3 | 2/13 | 0/13 | 0/13 | 3/13 | 3/13 | 1/13 | 1/13 |
| volume 10, `aec` | 0/3 | 0/3 | 0/13 | 0/13 | 0/13 | 5/13 | 3/13 | 3/13 | 1/13 |
| volume 10, `linear` | 2/3 | 1/3 | 2/13 | 0/13 | 0/13 | 6/13 | 4/13 | 3/13 | 2/13 |
| volume 15 +10 dB, `noaec` | 1/3 | 1/3 | 1/13 | 0/13 | 0/13 | 3/13 | 3/13 | 1/13 | 1/13 |
| volume 15 +10 dB, `aec` | 0/3 | 0/3 | 1/13 | 1/13 | 0/13 | 5/13 | 3/13 | 3/13 | 2/13 |
| volume 15 +10 dB, `linear` | 2/3 | 2/3 | 5/13 | 0/13 | 0/13 | 6/13 | 7/13 | 8/13 | 7/13 |

- The MLS voices stay at 0 to 1 of 26 in every condition and variant; they lower every pooled rate by the same amount and do not change any ranking.
- The gains of `linear` are spread over the voices (all six non-MLS voices gain at volume 15 +10 dB).

### Why the default canceller does not help a quiet talker

- **The oracle separates the two effects.** With the speech kept out of the canceller, the same echo removal would allow 34 % at volume 10 and 26 % at volume 15 (reference level). With the speech through it, the default output reaches 12 % and 4 %. So the loss is not residual echo but the **talker being removed along with the echo**: AEC3's nonlinear residual-echo suppressor attenuates the near-end when the echo is 8 to 16 dB louder than the talker (session 2's SNRs at the mic).
- The item segments confirm it: in the quiet room the talker leaves the chain at a median -39.2 dBFS; with content at volume 15 the default output of talker plus residual echo is -41.1 dBFS (NS: -48.1), i.e. below the talker alone, while the linear output keeps -30.4 dBFS (talker plus more residual echo).
- **The linear filter alone removes little.** In-band ERLE (200 to 4000 Hz) of the linear output: 3.8 dB on session 1's pink noise at volume 10 (full output 18.0 dB), 6.0 dB over the 10-minute content at volume 10 (full 10.3 dB). Most of AEC3's echo reduction on this TV comes from the suppressor, which is why `linear` keeps the talker but not enough of the echo at the reference level. Why AEC3's linear filter converges so little here (the session 1 analysis estimated about 14 dB of linear cancellation on test signals) is open; the TV's levelling and AI Sound processing after the tap are candidates (**hypothesis**).
- **AEC3's near-end tuning is not enough**: using it all the time (`nearend`) recovers a few items at volume 10 (16 % against 12 %) and nothing at volume 15.

### Gain staging

- More gain after the canceller changes nothing beyond one to three items in any condition (quiet 42 → 43, volume 10 `linear` 20 → 23 → 20 at +24/+30/+34 dB), and it saturates: `linear+34` clipped samples in 25 of 97 items at volume 10 and 56 of 97 at volume 15. **Gain alone is not the limit**; the detector is level-tolerant above about -40 dBFS. Session 2's "+10 dB speech" gain (43 % → 57 %) came from the better **signal-to-noise ratio** of a louder talker, not from the level. That refutes session 2's hypothesis that the fixed +24 dB is on the low side.
- Without a canceller, +30 dB clips heavily at volume 15 (29,687 samples in 10 minutes of content).

### NS

- Quiet room: NS raises detection by 7 to 10 items (43 % → 51 to 54 %, p 0.04 to 0.14), and by 4 items with the louder talker (57 % → 61 %, 4 gained, 0 lost). It removes the -69 dBFS room and mic noise that the reference-level talker sits only 8 dB above.
- With content it is neutral: within ±5 items of the same variant without NS in every condition, none significant (largest: `render2+ns` 15 against `render2` 10 at volume 10; `linear+ns` 32 against `linear` 37 at volume 15 +10 dB).

### Two-channel reference

- **ERLE** over the 10-minute content (in-band, mean of the per-minute segments), default against `render2`: volume 10 13.3 → 15.9 dB, volume 15 20.2 → 23.7 dB; the action scene (the lowest coherence in session 2) 7.6/5.6 → 17.1/19.5 dB at volume 10 and 9.9/10.7 → 25.5/27.5 dB at volume 15. Over the whole recording at volume 10: 10.3 → 16.7 dB. **But** on session 1's near-mono pink noise (tap3-v10) it fell from 18.0 to 8.4 dB: AEC3's stereo detection (threshold 0) switches to its multichannel configuration as soon as the channels differ, and its two-channel filters converge worse on near-mono input.
- **Wake**: no gain at the reference level (volume 10 10 %, 15 5 %); with the louder talker 45 % and 20 % (against 41 % and 15 % for `aec`, n.s.). It is not available with the linear output in this implementation.

### False wakes

Content-only recordings (no corpus speech), one long wake run per variant:

| Variant | Quiet, 2.5 min | Volume 10, 10 min | Volume 15, 10 min | Highest score, volume 10 / 15 | Mean in-band ERLE, volume 10 / 15 (dB) |
| --- | --- | --- | --- | --- | --- |
| `noaec` | 0 | 0 | 0 | 0.05 / 0.11 | 0 / 0 |
| `aec` | 0 | 0 | 0 | 0.03 / 0.04 | 13.3 / 20.2 |
| `ns` | 0 | 0 | 0 | 0.02 / 0.03 | 23.6 / 30.9 |
| `render2` | 0 | 0 | 0 | 0.03 / 0.04 | 15.9 / 23.7 |
| `render2+ns` | 0 | 0 | 0 | 0.01 / 0.02 | 27.7 / 35.3 |
| `linear` | 0 | 0 | 0 | 0.28 / 0.37 | 7.8 / 10.2 |
| `linear+ns` | 0 | **1** | 0 | 0.62 / 0.15 | 13.2 / 13.4 |
| `nearend` | 0 | 0 | 0 | 0.03 / 0.04 | 12.7 / 19.4 |
| `nearend+ns` | 0 | 0 | 0 | 0.01 / 0.04 | 20.9 / 27.7 |

- Zero in 10 minutes is a bound of 18 per hour (rule of three), so this proxy can only flag a large increase. `linear+ns` had one false wake at volume 10 (6.0 per hour [0.2, 33.6]); live, session 2 measured 0 in 1.14 h at volume 10 and 1.8 per hour at volume 15 without AEC.
- **The linear output looks more like speech to the detector**: its highest scores on content (0.28 to 0.62) are far above those of no AEC (0.05 to 0.11) or the full output (0.02 to 0.04). The full output, by contrast, makes content false wakes less likely than no AEC.
- Negatives and near-misses mixed into the three beds (28 and 14 items, 7 variants, both levels): 0 to 3 items with a detection per condition and variant, mostly in the lead-in (content-triggered, before the item) and scattered across variants without a pattern. Item-caused: "Alex, ¿qué hora es?" at +10 dB in the quiet room with NS (HA also woke on it digitally).

### STT (faster-whisper large-v3-turbo, command segment, 70 commands)

Corpus WER v1 on the command segment of the 50 commands of the five non-MLS Spanish voices (269 reference words), independent of the wake result. The MLS voices are left out because Whisper's output on them (greedy decoding) changed with sub-millisecond shifts of the segment even where the audio was otherwise identical (quiet room, all variants: 36.4 to 48.7 % with them, 8.9 % for every variant without them); tables with them are in the private result file. Clean digital items, same segment: **8.6 %** (23/269).

| Variant | Quiet | Content volume 10 | Content volume 15 | Quiet +10 dB | Volume 10 +10 dB | Volume 15 +10 dB |
| --- | --- | --- | --- | --- | --- | --- |
| `noaec` | 8.9 % | 64.3 % | 107.8 % | 7.4 % | 25.7 % | 57.2 % |
| `aec` | 8.9 % | 52.8 % | 94.4 % | 7.4 % | 15.6 % | 51.3 % |
| `ns` | 8.9 % | 49.1 % | 92.6 % | 9.3 % | 17.8 % | 59.1 % |
| `render2` | 8.9 % | 53.2 % | 92.6 % | 7.4 % | 17.1 % | 47.2 % |
| `render2+ns` | 8.9 % | 49.8 % | 91.8 % | 9.3 % | 17.1 % | 48.7 % |
| `linear` | 8.9 % | **29.4 %** | **61.3 %** | 7.4 % | 8.9 % | **17.8 %** |
| `linear+ns` | 8.9 % | 30.9 % | 64.3 % | 8.6 % | **7.8 %** | 18.2 % |
| `nearend` | 8.9 % | 38.3 % | 92.6 % | 7.4 % | 14.5 % | 46.8 % |
| *`oracle-aec`* | *9.3 %* | *13.4 %* | *14.5 %* | *7.4 %* | *7.1 %* | *7.8 %* |

- In a quiet room every variant transcribes as well as the clean items (7.4 to 9.3 %): neither the canceller nor NS hurts STT.
- With content, Whisper transcribes the TV instead of the talker (as in session 2); the **linear output** halves the WER of the default canceller at volume 10 (29.4 against 52.8 %) and brings the louder talker to the clean level at volume 10 (7.8 to 8.9 %) and to 17.8 % at volume 15 (default 51.3 %, no AEC 57.2 %). The default output's suppressor costs STT as much as it costs the wake word: the oracle, with the talker kept out of the canceller, stays at 7 to 15 % everywhere.
- NS adds nothing to STT with content and costs up to 2 points in quiet with the louder talker (n.s.; one run, no per-item test).

## Conclusion and confidence

1. **At the rig reference level no variant restores wake detection over TV content.** The best variants reach 21 to 24 % at volume 10 (`linear`, `linear+30`; no AEC 11 %, quiet room 43 %) and at most 7 % at volume 15. Confidence: moderate (one recording per volume, synthetic voices, no room path for the talker; but the offline scorer reproduces HA's results item for item).
2. **The default canceller does not hurt in quiet** (42/97 identical items: with a silent reference AEC3 is transparent) and makes content false wakes less likely, but at the reference level it removes the talker with the echo (oracle 34 % / 26 % against 12 % / 4 %).
3. **With a talker 10 dB louder** (closer, or raised voice), echo cancellation helps clearly, and the **linear output** most: volume 15 from 11 % (no AEC) and 15 % (default AEC) to 38 %; volume 10 from 34 % to 46 to 49 %. Confidence: moderate to high for the direction (p < 0.001 at volume 15), low for the size.
4. **STT follows the same pattern**: every variant matches the clean items in quiet (about 9 % WER without the MLS voices); with content the linear output halves the default output's WER at volume 10 (29 against 53 %) and, for the louder talker, reaches 8 % at volume 10 and 18 % at volume 15 (default 16 % and 51 %).
5. **Gain staging is not the bottleneck**; keep +24 dB. **NS helps the wake word in quiet** and is neutral with content and for STT.
6. The two-channel reference raises ERLE on stereo content (action scene +10 to +17 dB) but lowers it on near-mono input and did not raise wake rates at the reference level: keep it off by default.

### Recommended `[aec]` defaults for the live A/B (proposal, for the owner to decide)

| Key | A/B arm A (candidate) | A/B arm B | Why |
| --- | --- | --- | --- |
| `enabled` | true | true (and a no-AEC control block) | never worse than no AEC; helps a louder talker |
| `output` | `"linear"` | `"full"` | best wake rates over content (p ≤ 0.01 at volume 10 reference level and volume 15 +10 dB) and the lowest STT WER with content; B keeps the lower false-wake scores |
| `ns` | true | true | +7 to 10 wake items in quiet; neutral with content and for STT |
| `render_channels` | 1 | 1 | `render2` not available with `linear`; mixed ERLE results |
| gain (`[dsp] gain_db`) | 24 | 24 | more gain only clips |
| `reference_gain_db`, `bypass_when_silent`, `delay_ms` | 0, false, "auto" | same | not evaluated here; P3 measured the bypass's CPU saving |

Keep the shipped default `enabled = false` until the live A/B passes ([design, section 9 step 3](../design/aec-frontend.md#step-3-live-ab-with-the-aec-set-during-content)). The live A/B must measure false wakes per hour with `output = "linear"` over the full content set, because of its higher detector scores on content.

## Negative and inconclusive results

- No variant gives a usable wake rate at volume 15 for a reference-level talker (≤ 7 %).
- The default AEC3 output does not raise the reference-level wake rate at volume 10 (12 % against 11 %, p 1.0).
- More gain after the canceller does not help (H2 rejected); session 2's "fixed gain too low" hypothesis is refuted.
- AEC3's near-end tuning used all the time does not fix the near-end suppression.
- The two-channel reference halves ERLE on near-mono test signals (18.0 → 8.4 dB).
- AEC3's linear filter removes only 4 to 6 dB in-band on these recordings; the cause is unknown.
- The false-wake proxy (10 minutes per volume) cannot detect rates below about 18 per hour.

## Residual risks and open questions

- **Double talk with real voices**: every positive here is double talk (talker over content), but with clean synthetic speech added digitally. A real talker reaches the mic through the room, at a level and spectrum the rig never measured with content playing.
- **The TV's program-audio levelling** (session 2): content at any digital level reaches the mic at about the same level for a given master volume, so the master volume sets the echo-to-talker ratio. The reference tap is after the levelling (it follows it), so the canceller sees it; the effect on AEC3's linear filter convergence is unknown.
- **Voice dependency**: two of seven Spanish voices never trigger `alexa`; real voices are needed before any rate is user-facing.
- **Linear output and false wakes**: higher detector scores on content; one false wake in 10 minutes. Needs hours of content in the live A/B.
- **CPU**: the direct engine runs the same AEC3 code (the full output is bit-identical), so its cost should match P3's 11 % of a core on armv7 (**hypothesis**); `render2` costs about 15 % more on the host, not measured on the TV.
- **A stereo render for the linear output** and a gentler suppressor (between `linear` and `full`) are untested; the oracle shows room for up to 34 % / 26 % at the reference level and 54 % / 46 % with the louder talker.
- One recording per volume and one content set; items reuse session 2's bed segments.

## Reproduction

Recordings and outputs stay in the gitignored `devices/private/`. Python: a separate venv with `numpy scipy soundfile pyopen-wakeword==1.1.0 faster-whisper` (and `nvidia-cublas-cu12` for the GPU).

```sh
cargo build --release -p webos-capture --example aec_offline
P=devices/private/c3-a/P2/session2; W=devices/private/c3-a/P2/aec-offline
# wake, per bed (quiet uses semisyn/A-quiet.jsonl); add --speech-gain-db -29 --condition <bed>+10dB for the louder talker
python tools/audio-rig/aec_eval/semisyn_ab.py --corpus devices/private/corpus/p2-v1 \
  --session-rows $P/semisyn/content-v10.jsonl --capdir $P/aec/content-v10 --work-dir $W/work --out $W/final.jsonl \
  --keep-wavs --variant noaec=--no-aec --variant aec= --variant ns=--ns --variant "render2=--render-channels 2" \
  --variant "linear=--output linear" --variant "linear+ns=--output linear --ns" --variant "nearend=--output nearend" \
  --variant "oracle-aec=@oracle"
python tools/audio-rig/aec_eval/ab_report.py $W/final.jsonl --by-voice --paired noaec
# calibration against session 2's HA results
python tools/audio-rig/aec_eval/semisyn_ab.py ... --variant py=@py --variant noaec=--no-aec --out $W/calib.jsonl
python tools/audio-rig/aec_eval/ab_report.py $W/calib.jsonl --calibration
# content-only false wakes and ERLE per segment
python tools/audio-rig/aec_eval/content_eval.py --work-dir $W/work-c --manifest $P/aec/manifest.json \
  --variant noaec=--no-aec --variant aec= --variant "linear=--output linear" $P/aec/content-v10
# STT on the kept WAVs
python tools/audio-rig/aec_eval/stt_eval.py --results $W/final.jsonl --variant noaec --variant aec \
  --variant linear --digital devices/private/corpus/p2-v1 --out $W/stt.jsonl
```

Tests: `python3 -m unittest discover -s tools/audio-rig/aec_eval -p 'test_*.py'`; `cargo test -p webos-dsp`.

## Rollback and cleanup

Nothing on a device changed. The new `[aec]` keys default to the previous behaviour (`render_channels = 1`, `output = "full"`). Private outputs under `devices/private/c3-a/P2/aec-offline/` can be deleted.

## Follow-ups / unresolved questions

- Live A/B on the TV with the arms above, real voices at a measured distance, and hours of content for false wakes ([design, section 9 step 3](../design/aec-frontend.md#step-3-live-ab-with-the-aec-set-during-content)).
- Why AEC3's linear filter converges to only 4 to 6 dB on c3-a; whether a longer filter or another sound mode changes it.
- A suppressor setting between the linear and the full output; a stereo render for the linear output.
