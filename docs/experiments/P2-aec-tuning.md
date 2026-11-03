# P2 offline AEC tuning: AEC3 suppressor and echo model, combinations, wake threshold sweep

**Status:** PARTIAL POSITIVE, semi-synthetic. A tuned configuration matches the linear output's talker preservation and removes as much echo as the default canceller. No variant restores reference-level detection at volume 15. The echo-cancellation work is **parked** (owner decision, 2026-11-03), and the shipped default stays `enabled = false`.
**Date:** 2026-11-02 to 2026-11-03
**Device alias / model / firmware / webOS / architecture:** recordings from c3-a (LG C3, firmware 03.30.74, webOS TV 8.3.0, TV speaker, `aiSoundPlus`), processed on the PC (x86_64)
**Power and playback state:** the recordings of [P2 session 2](../discovery/c3-a/P2-session2-wake-accuracy.md#4-aec-recording-set-and-offline-baseline): "Screen Saver" over HDMI 1, content set `p2-v1` at TV volume 10 and 15, and a quiet room
**Related:** [P2 offline A/B](P2-aec-offline-ab.md) (the method and baseline this builds on), [AEC front-end design](../design/aec-frontend.md#12-tuning-results-and-parked-state), [P2 corpus](P2-corpus.md), [backlog item 1](../project/backlog.md#1-professional-audio-front-end)
**Risk:** none (PC only: no TV, no playback, no Home Assistant)
**Approval needed / obtained:** none needed

Labels follow [AGENTS.md](../../AGENTS.md). Every rate is **observed-on-other-device, semi-synthetic**. Clean Piper speech was added digitally to the raw mic of real c3-a recordings and processed on the PC by the daemon's own front-end code. The scoring used the Wyoming openWakeWord 2.1.0 replica, which matched HA on 387 of 388 items in the [A/B](P2-aec-offline-ab.md#calibration-against-home-assistant). Nothing here ran on a TV, and no real talker was in a room.

## Question and hypotheses

The [A/B](P2-aec-offline-ab.md#why-the-default-canceller-does-not-help-a-quiet-talker) found that AEC3's residual-echo suppressor removes a quiet talker along with the echo. With the talker kept out of the canceller, the diagnostic reached 34 % at volume 10 and 26 % at volume 15. The default output reached 12 % and 4 %, and the linear output 21 % and 4 %, but the linear output leaves 6 to 10 dB more echo. This experiment asks whether a configuration closes that gap.

- H1: a suppressor or echo-model setting keeps most of the linear output's wake rate and removes more echo than the linear output. **Supported**, through a blend of the linear output and not through AEC3's masks (below).
- H2: a stereo render, RNNoise, a limiter or AGC adds to that. **Supported for the stereo render** (with the blend). **Rejected for RNNoise** (it removes the talker). **No effect** from the limiter or AGC.
- H3: a lower wake threshold or a higher trigger level gives a better trade-off than HA's default (0.5, 1). **Not supported** on this data (see the DET sweep).

## Method

1. **Same mix and chain as the A/B.** Each corpus positive was added to the raw mic of the session 2 AEC recordings at the rig reference level (-39 dB re full scale, about -61 dBFS at the mic) and 10 dB louder (-29 dB). The beds were the quiet room and content at volume 10 and 15. Each mix went through `aec_offline` with 10 s of warm-up. `tune_eval.py` reproduced the A/B's volume 10 result (11, 12 and 20 of 97 for no AEC, the default and linear). Its trace-based detections were identical to the A/B's on all 291 rows compared (times and maximum scores).
2. **Scoring as traces.** `tune_eval.py` stores every model probability of at least 0.1 per item. `oww_wyoming.detect_from_trace` replays the server loop (strictly above the threshold, `trigger_level` counting probabilities, 2 s refractory) for any threshold and trigger level, so the sweep needs no re-run. The trigger count in wyoming-openwakeword 2.1.0 (`handler.py` at tag v2.1.0) is reset only at the start of an audio stream. It is not reset after a probability below the threshold, so level 2 means "the second probability above the threshold since the stream started" (**documented**, source).
3. **Voice split against overfitting.** Parameters were chosen on the **tuning split**, the voices davefx, sharvard F, ald and lessac (42 positives). Results are reported on the **held-out split**, the voices sharvard M, claude and ryan plus the two MLS voices (55 positives, 29 without MLS), which was run only once, after the candidates were fixed.
4. **Echo removal.** The in-band ERLE (200 to 4000 Hz, reference-active frames) of each variant at 0 dB gain, against the same chain without canceller, over the whole 10-minute content recordings. It is reported as the mean of the per-segment values (as in the A/B), per volume. The ERLE does not distinguish echo from talker removal, so RNNoise, which removes everything, shows a very high "ERLE". It is read together with the wake rates.
5. **Search.** The search ran in three stages, on the four content conditions of the tuning split (168 trials per variant).
   - Screen: one parameter group at a time around AEC3's defaults, 31 variants.
     - normal-tuning masks: LF and HF `enr_transparent` and `enr_suppress` scaled by 2, 4, 8, 16 and 32;
     - dominant near-end detection: `enr_threshold` 1, 4 and 16 with `snr_threshold` 3 or 1;
     - `ep_strength.default_gain` 0.5 and 0.25, `default_len` 0.5 and 0, `bounded_erl`;
     - echo audibility: render limits ×4 and ×16, audibility thresholds ×10 and ×100;
     - comfort noise floor -60 dBFS;
     - ERLE bounds `max_l`/`max_h` 8/4 and 32/16;
     - filter lengths 20, 30 and 40 blocks;
     - `linear_mix` 0.1 to 0.5.
   - Full factorial of the useful ones: `linear_mix` 0.2, 0.3 and 0.4 × near-end detection on or off × `default_len` 0 or default × masks ×4 or default (24 variants).
   - Combinations with the stereo render, NS, RNNoise, the AGC2 limiter and adaptive AGC2, and the stereo render with `linear_mix` 0.3 to 0.5.
   - The quiet room was included where NS, RNNoise or AGC could change it. The canceller is transparent there, so every canceller-only variant gives the same quiet result.
6. **False wakes.**
   - The content-only recordings (quiet 2.5 min, volume 10 and 15 at 10 min each, 22.5 min in total) ran as consecutive Wyoming streams, each re-armed 2 s after a detection, for every threshold from 0.2 to 0.6 in steps of 0.05 and trigger level 1 and 2.
   - The 28 negatives and 14 near-misses (all voices) were mixed into the three beds at both levels (252 items per variant).
7. **Statistics.** Wilson 95 % intervals. Paired exact McNemar tests against the default canceller, uncorrected for multiple comparisons. Exact Poisson bounds for false wakes (rule of three for zero).

### Code added (defaults unchanged)

- **`[aec.aec3]`** (dsp `Aec3Tuning`, `aec_offline --aec3 NAME=VALUE`): experimental overrides of 39 AEC3 parameters, named by sonora-aec3's config paths and range-checked against sonora-aec3's own validation limits. They cover:
  - the suppressor's normal and near-end tuning masks and gain-change factors;
  - dominant near-end detection;
  - `ep_strength`, echo audibility and the comfort-noise floor;
  - the ERLE bounds and the refined and coarse filter lengths.

  There is one extra knob, `linear_mix` (0 to 1). It blends AEC3's linear output, aligned to the final output, into the final output as `(1 - m) * final + m * linear`, which bounds the suppressor's attenuation at about `20 log10(m)` dB. Any override runs AEC3 through the direct engine. With values equal to the defaults the output is sample-identical to the default path (unit test).
- **The direct engine now takes a stereo render.** It repeats sonora's `EchoCanceller3` handling: stereo-content detection, a rebuild with AEC3's multichannel configuration, and a downmix with an echo-path-change flag while the stereo content is only temporary. A unit test checks that it is **sample-for-sample identical** to sonora's `AudioProcessing` with a two-channel render, through the switch to stereo. So `output = "linear"` and `"nearend"` and any tuning now work with `render_channels = 2`.
- **`AecConfig::agc2`** (dsp, `aec_offline --agc2-fixed-db G`, `--agc2-adaptive`): sonora's AGC2 after the canceller and NS, as a fixed gain into its limiter, optionally with the adaptive digital gain, used with a front-end gain of 0 dB. It is not exposed in the daemon config.
- **`aec_offline --rnnoise`**: nnnoiseless 0.5.2 (RNNoise) on the S16 output, upsampled to 48 kHz and back, with its latency removed. It is a dev-dependency of the example only and is not linked into the daemon.
- **Tools.** `tune_eval.py` (items with traces, content ERLE and re-armed false wakes, voice splits, resumable) and `tune_report.py` (rates, paired tests, Pareto, DET tables). Unit tests cover the trace replay (strict threshold, refractory time, the never-reset trigger count), the splits, the latencies and the scoring.

## Observations

### Tuning split (42 positives, four content conditions = 168 trials)

| Variant | Wake on content | ERLE vol 10 / 15 (dB) |
| --- | --- | --- |
| no AEC | 32/168 = 19 % | n/a |
| default (`aec`) | 43/168 = 26 % | 13.3 / 20.2 |
| `linear` | 63/168 = 38 % | 7.8 / 10.2 |
| `linear`, stereo render | 68/168 = 40 % | 9.7 / 14.0 |
| masks ×2 … ×32 | 43 to 49 (26 to 29 %) | 12.9/19.7 … 9.9/15.0 |
| near-end detection (`enr_threshold` 1, `snr_threshold` 3) | 53/168 = 32 % | 12.5 / 19.6 |
| `ep_strength.default_len` = 0 | 44/168 = 26 % | 15.1 / 21.4 |
| default_gain, bounded_erl, audibility, ERLE bounds, filter length 20 to 40 | 39 to 45 | within 1.1 dB of the default |
| comfort noise floor -60 dBFS | 41/168 = 24 % | 9.3 / 14.6 |
| `linear_mix` 0.1 / 0.2 / 0.3 / 0.5 | 50 / 55 / 61 / 60 | 13.3/19.9, 13.1/18.9, 12.7/17.6, 11.4/15.1 |
| **A**: `linear_mix` 0.2 + `default_len` 0 | 61/168 = 36 % | 15.2 / 19.9 |
| `linear_mix` 0.4 + near-end detection + `default_len` 0 | 65/168 = 39 % | 13.9 / 17.3 |
| **B**: stereo render + `linear_mix` 0.4 + `default_len` 0 | **71/168 = 42 %** | **14.9 / 20.4** |
| stereo render + `linear_mix` 0.3 / 0.5 + `default_len` 0 | 63 / 69 | 15.6/21.7, 14.0/19.1 |
| *diagnostic: talker kept out of the default canceller* | *94/168 = 56 %* | *13.3 / 20.2* |

- **AEC3's own suppressor knobs do not help.** Higher masking thresholds trade echo removal for at most 6 items. Forcing the near-end state helps a little (53). The echo-path strength, audibility, ERLE bounds and longer filters (20 to 40 blocks) change nothing, neither the rate nor the ERLE of the full output. The suppressor's estimate of the residual echo is above the quiet talker in most bins whatever the thresholds.
- **A bound on the suppression works.** `linear_mix` blends the linear output back in, which bounds the suppressor's attenuation (0.2 is about -14 dB, 0.4 about -8 dB). That recovers most of the linear output's rate. `ep_strength.default_len = 0` adds 1 to 2 dB of ERLE with or without the blend. The stereo render adds about 1 to 3.5 dB of ERLE and some detections.
- **B is on the Pareto front.** It has more detections than the linear output, the linear output with a stereo render, and A, with an ERLE at the default's level (14.9/20.4 against 13.3/20.2). Adding near-end detection gave the same rate (71) and the same ERLE, so the simpler B was kept.

### Combinations (tuning split, A as base)

| Variant | Quiet -39 / -29 | Vol 10 -39 / -29 | Vol 15 -39 / -29 |
| --- | --- | --- | --- |
| A | 28 / 32 | 12 / 25 | 4 / 20 |
| A + sonora NS | **32 / 35** | 6 / 25 | 4 / 18 |
| `linear` + sonora NS | **31 / 36** | 12 / 26 | 4 / 19 |
| A + RNNoise | 23 / 24 | 5 / 21 | 0 / 8 |
| A + sonora NS + RNNoise | 16 / 27 | 7 / 16 | 0 / 8 |
| `linear` + RNNoise | 14 / 21 | 12 / 27 | 1 / 15 |
| A + AGC2 limiter (+24 dB) | 28 / 32 | 12 / 25 | 4 / 20 |
| A + adaptive AGC2 | 27 / 32 | 12 / 25 | 4 / 19 |
| `linear` + AGC2 limiter | 28 / 32 | 13 / 26 | 3 / 21 |

(of 42 each)

- **RNNoise removes the talker.** In the quiet room it halves detection with the linear output (28 → 14). On content it attenuates the canceller's output by a further 20 to 30 dB or more, talker included. It also costs about 175 µs per 10 ms on the host at opt-level 3, more than four times the canceller, before resampling. **Rejected**, both instead of sonora's NS and after it.
- **sonora NS** helps in quiet (+3 to +4 of 42) and costs items at volume 10 with the reference-level talker (A: 12 → 6). It is mixed on content, as in the A/B.
- **The limiter and adaptive AGC change nothing** (within one item). Clipping after +24 dB was rare with any canceller (0 to 2 clipped samples in 10 minutes at volume 10, 0 to 90 at volume 15). The detector is level-tolerant, as the A/B found for gain.

### Held-out split (55 positives; 29 without the MLS voices), run once

All values are wake rates at threshold 0.5 and trigger level 1.

| Variant | Quiet | Vol 10 | Vol 15 | Quiet +10 dB | Vol 10 +10 dB | Vol 15 +10 dB |
| --- | --- | --- | --- | --- | --- | --- |
| no AEC | 14/55 = 25 % | 5/55 = 9 % | 0/55 = 0 % | 23/55 = 42 % | 14/55 = 25 % | 5/55 = 9 % |
| default (`aec`) | 14/55 = 25 % | 4/55 = 7 % | 1/55 = 2 % | 23/55 = 42 % | 17/55 = 31 % | 6/55 = 11 % |
| `linear` | 14/55 = 25 % | 7/55 = 13 % | 1/55 = 2 % | 23/55 = 42 % | 19/55 = 35 % | 16/55 = 29 % |
| `linear` + NS | 21/55 = 38 % | 7/55 = 13 % | 2/55 = 4 % | 23/55 = 42 % | 22/55 = 40 % | 13/55 = 24 % |
| `linear`, stereo render | 14/55 = 25 % | 7/55 = 13 % | 3/55 = 5 % | 23/55 = 42 % | 23/55 = 42 % | 14/55 = 25 % |
| A | 14/55 = 25 % | 5/55 = 9 % | 1/55 = 2 % | 23/55 = 42 % | 20/55 = 36 % | 10/55 = 18 % |
| B, mono render | 14/55 = 25 % | 6/55 = 11 % | 3/55 = 5 % | 23/55 = 42 % | 19/55 = 35 % | 13/55 = 24 % |
| **B** | 14/55 = 25 % | 7/55 = 13 % | 0/55 = 0 % | 23/55 = 42 % | **25/55 = 45 %** | 14/55 = 25 % |
| **B + NS** | **19/55 = 35 %** | **9/55 = 16 %** | **4/55 = 7 %** | 24/55 = 44 % | 24/55 = 44 % | 14/55 = 25 % |
| *diagnostic (A/B run)* | | *11/55 = 20 %* | *7/55 = 13 %* | | *22/55 = 40 %* | *21/55 = 38 %* |

Without the MLS voices (29 items each):

| Variant | Quiet | Vol 10 | Vol 15 | Quiet +10 dB | Vol 10 +10 dB | Vol 15 +10 dB |
| --- | --- | --- | --- | --- | --- | --- |
| default | 48 % | 14 % | 3 % | 79 % | 59 % | 17 % |
| `linear` | 48 % | 24 % | 3 % | 79 % | 66 % | 55 % |
| B | 48 % | 21 % | 0 % | 79 % | **86 %** | 48 % |
| B + NS | **66 %** | **31 %** | **14 %** | 83 % | 83 % | 48 % |

- Paired against the default canceller on the same items, giving gained/lost and the McNemar p:
  - B at volume 10 +10 dB: 8/0, p 0.008; at volume 15 +10 dB: 11/3, p 0.06.
  - B + NS at volume 10 +10 dB: 8/1, p 0.04; at volume 15 +10 dB: 10/2, p 0.04; in quiet at the reference level: 7/2, p 0.18.
  - At the reference level over content, every variant stays within 0 to 6 items of the default (p ≥ 0.12).
- **The held-out split confirms the direction but not a gain over the linear output.** Over the four content conditions, the totals of 220 are:
  - B 46, B + NS 51, `linear` 43, linear with stereo render 47, A 36, default 28 and no AEC 24;
  - the diagnostic reaches 61.

  B keeps the linear output's talker preservation and gives back the default's echo removal (see below). It does not clearly beat the linear output on these voices.
- At volume 15 with the reference-level talker, nothing works (0 to 7 %; the diagnostic 13 %).

### Echo removal and content false wakes (final variants)

| Variant | ERLE vol 10 / 15, mean of segments (dB) | Whole recording (dB) | Max wake score vol 10 / 15 | Content false wakes, any threshold 0.2 to 0.6, level 1 or 2 |
| --- | --- | --- | --- | --- |
| default | 13.3 / 20.2 | 10.3 / 14.9 | 0.02 / 0.04 | 0 |
| `linear` | 7.8 / 10.2 | 6.0 / 7.0 | 0.28 / 0.37 | 1 to 3 at threshold ≤ 0.35 (level 1) |
| `linear` + NS | 13.2 / 13.4 | 10.2 / 10.2 | **0.62** / 0.15 | 1 at every threshold ≤ 0.6 (level 1) |
| `linear`, stereo render | 9.7 / 14.0 | 10.0 / 13.8 | 0.03 / 0.20 | 1 at 0.2 |
| A | 15.2 / 19.9 | 13.5 / 16.7 | 0.06 / 0.04 | 0 |
| **B** | **14.9 / 20.4** | **15.4 / 20.6** | 0.03 / 0.10 | **0** |
| B + NS | 23.2 / 26.1 | 22.2 / 24.7 | 0.02 / 0.23 | 1 at 0.2 |

- Exposure is 22.5 minutes per variant (quiet 2.5, volume 10 and 15 at 10 each). Zero false wakes bounds the rate at **≤ 8 per hour** over all three recordings, and at ≤ 18 per hour per volume. That can only flag a large increase.
- B brings the detector's highest content scores back to the default's range (0.03/0.10 against 0.28/0.37 for the linear output). This addresses the A/B's concern about the linear output on content.
- Negatives and near-misses (252 items per variant, threshold 0.5): 1 to 8 items with a detection for every variant (no AEC 6, default 4, linear 1, B 6, B + NS 6), scattered over the content beds without a pattern. Only the NS variants had any in the quiet room (3 each), where the item itself triggered, as in the A/B.

### Wake threshold and trigger level (DET, held-out without MLS)

| Variant | Level | Threshold | Quiet | Vol 10 | Vol 15 | Vol 10 +10 dB | Vol 15 +10 dB | Neg + near items (of 252) | Content false wakes (22.5 min) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| default | 1 | 0.3 / 0.4 / 0.5 / 0.6 | 62 / 52 / 48 / 45 % | 21 / 21 / 14 / 14 % | 7 / 3 / 3 / 3 % | 66 / 62 / 59 / 48 % | 17 / 17 / 17 / 10 % | 13 / 7 / 4 / 4 | 0 |
| default | 2 | 0.3 / 0.4 / 0.5 / 0.6 | 52 / 38 / 31 / 28 % | 17 / 14 / 14 / 14 % | 3 / 3 / 3 / 3 % | 59 / 52 / 52 / 48 % | 17 / 14 / 7 / 7 % | 7 / 2 / 1 / 1 | 0 |
| B | 1 | 0.3 / 0.4 / 0.5 / 0.6 | 62 / 52 / 48 / 45 % | 31 / 28 / 21 / 17 % | 3 / 0 / 0 / 0 % | 86 / 86 / 86 / 83 % | 52 / 52 / 48 / 45 % | 16 / 9 / 6 / 6 | 0 |
| B | 2 | 0.3 / 0.4 / 0.5 / 0.6 | 52 / 38 / 31 / 28 % | 28 / 21 / 17 / 17 % | 3 / 0 / 0 / 0 % | 83 / 79 / 79 / 76 % | 45 / 45 / 45 / 45 % | 9 / 6 / 5 / 5 | 0 |
| B + NS | 1 | 0.3 / 0.4 / 0.5 / 0.6 | 76 / 76 / 66 / 55 % | 34 / 34 / 31 / 24 % | 17 / 14 / 14 / 7 % | 86 / 83 / 83 / 79 % | 52 / 48 / 48 / 48 % | 17 / 8 / 6 / 3 | 1 at 0.2, else 0 |
| B + NS | 2 | 0.3 / 0.4 / 0.5 / 0.6 | 76 / 62 / 52 / 52 % | 31 / 28 / 24 / 21 % | 14 / 14 / 10 / 7 % | 83 / 83 / 79 / 69 % | 48 / 48 / 48 / 45 % | 5 / 4 / 3 / 3 | 1 at 0.2, else 0 |

(29 items per rate. The full sweep from 0.2 to 0.6 in steps of 0.05 is in the private result files.)

- **Trigger level 2 costs detection and buys little.** In the quiet room it costs 14 to 17 points (48 → 31 % for the default at 0.5). Because the count is never reset within a stream, an HA wake stream that runs for hours accumulates stray single activations, so level 2 is only a weak guard against content false wakes in continuous streaming (**inferred from the source**, not measured live).
- **Lowering the threshold to 0.4** gains 0 to 7 points (quiet +4, volume 10 +7 for B) and about 3 more negative or near-miss items of 252 at threshold 0.4. It did not add content false wakes, but 22.5 minutes cannot show that.

## Conclusion and confidence

1. **Best-known configuration** (for a live A/B, not a default):

   ```toml
   [aec]
   enabled = true
   render_channels = 2
   ns = true          # optional: B + NS was best in quiet and at the reference level
   [aec.aec3]
   "linear_mix" = 0.4
   "ep_strength.default_len" = 0.0
   ```

   - On the tuning voices: 42 % on content against 38 % (linear) and 26 % (default), with the default's echo removal (ERLE 14.9/20.4 against 13.3/20.2 dB) and content scores back near the default's.
   - Held-out, wake rates for quiet / volume 10 / volume 15 at the reference level, then the talker +10 dB:
     - B: 25 / 13 / 0 % and 42 / 45 / 25 %;
     - B + NS: 35 / 16 / 7 % and 44 / 44 / 25 %;
     - default: 25 / 7 / 2 % and 42 / 31 / 11 %.
   - Confidence: moderate for "better than the default with a louder talker" (p 0.008 to 0.06). Low for "better than the linear output" (not significant on the held-out voices). Low for any absolute rate (synthetic voices, no room path, one recording per volume).
2. **AEC3's own suppressor parameters cannot rescue a talker 8 to 16 dB below the echo** on this TV; bounding the suppression (`linear_mix`) can. AEC3's linear filter still removes only 4 to 6 dB here, and longer filters did not raise the full output's ERLE.
3. **Rejected:** RNNoise (removes the talker, about 4× the canceller's CPU), the limiter and AGC (no effect), and longer filters, masks, audibility and ERLE bounds (no gain).
4. **Wake threshold: keep HA's defaults** (threshold 0.5, trigger level 1). This is a recommendation for the owner's GitOps values, not a change.
   - Level 2 costs 14 to 17 points of detection in quiet (up to 10 elsewhere) and protects little in long streams.
   - A trial at 0.4 is defensible only together with a live false-wake soak of several hours. Its expected gain is 0 to 7 points, at about 1 % more items with a stray wake.

### CPU on the host (x86_64, 10-minute content recording, mean per 10 ms frame)

| Variant | µs per frame | Relative to default | Hypothetical TV cost (armv7 1.0 ms, aarch64 0.21 ms per frame for the default, [P3](../discovery/c3-a/P3-tv-verify-aec-pairing.md)) |
| --- | --- | --- | --- |
| default | 39 to 41 | 1.0 | 1.0 ms (11 %) / 0.21 ms |
| `linear` | 37 to 40 | 1.0 | about the same |
| A (mono, tuned) | 38 to 39 | 1.0 | about the same |
| `linear`, stereo render | 47 | 1.2 | ≈ 1.2 ms / 0.25 ms |
| **B** | 46 to 48 | 1.2 | ≈ 1.2 ms (about 13 %) / 0.25 ms |
| B + NS | 56 to 59 | 1.4 | ≈ 1.45 ms (about 16 %) / 0.30 ms |
| default + NS | 51 to 55 | 1.3 | |
| B + adaptive AGC2 | 67 to 74 | 1.75 | |
| RNNoise alone (nnnoiseless, opt-level 3, without resampling) | 175 | 4.4 | |

The TV columns scale the TV's measured default by the host ratio. That is a **hypothesis**: the TV cost of these variants was not measured.

## Negative and inconclusive results

- No configuration gives a usable rate at volume 15 for a reference-level talker (held-out ≤ 7 %, the diagnostic 13 %).
- B does not significantly beat the plain linear output on the held-out voices (46 against 43 of 220 on content).
- The masking thresholds, echo-path strength, audibility thresholds, ERLE bounds, comfort noise and filter lengths gave no useful gain.
- RNNoise, the limiter and AGC2: negative or null.
- Content false wakes: 22.5 minutes per variant bounds the rate at ≤ 8 per hour only.
- STT was **not run** for the tuned variants (the work was parked first). The kept WAVs are in the private work directory for `stt_eval.py`.

## Residual risks and open questions

- Synthetic double talk only: real voices at a measured distance, through the room, are untested (the JBL rig was unreachable in session 2).
- `linear_mix` passes more residual echo through in the bins where the suppressor would have removed it. Its content false-wake rate needs hours of live content.
- The stereo render's benefit depends on the content's stereo image; with near-mono content AEC3 stays in its mono configuration.
- The CPU of B on the TV, the delay lock and ERLE with a playing stimulus, and long sessions on the TV are unverified.

## Reproduction

Everything private stays in the gitignored `devices/private/c3-a/P2/aec-tuning/`. The Python venv is the A/B's.

```sh
cargo build --release -p webos-capture --example aec_offline
P=devices/private/c3-a/P2/session2; W=devices/private/c3-a/P2/aec-tuning; T=tools/audio-rig/aec_eval
B10="content-v10=$P/aec/content-v10:$P/semisyn/content-v10.jsonl"; B15="content-v15=$P/aec/content-v15:$P/semisyn/content-v15.jsonl"
BQ="quiet=$P/aec/quiet:$P/semisyn/A-quiet.jsonl"
# a search stage on the tuning voices (variants as [label, flags] JSON, e.g. ["B", "--render-channels 2 --aec3 linear_mix=0.4 --aec3 ep_strength.default_len=0"])
python $T/tune_eval.py items --corpus devices/private/corpus/p2-v1 --bed $B10 --bed $B15 --gain -39 --gain -29 \
  --kind pos --split tune --variants-file stage.json --work-dir $W/work --out $W/stage.jsonl
python $T/tune_eval.py content --manifest $P/aec/manifest.json --variants-file stage.json --work-dir $W/work \
  --out $W/stage-content.jsonl $P/aec/content-v10 $P/aec/content-v15
python $T/tune_report.py pareto $W/stage.jsonl --content $W/stage-content.jsonl
# held-out, negatives (rows without bed offsets from the A/B), content false wakes per threshold and level
python $T/tune_eval.py items ... --bed $BQ --bed $B10 --bed $B15 --split held --keep-wavs --out $W/held.jsonl
python $T/tune_eval.py items ... --bed quiet=$P/aec/quiet:<rows-all-nobed.jsonl> [...] --kind neg --kind nearmiss --out $W/negs.jsonl
python $T/tune_eval.py content ... --wake --threshold 0.2 [...] --threshold 0.6 --trigger-level 1 --trigger-level 2 \
  $P/aec/quiet $P/aec/content-v10 $P/aec/content-v15 --out $W/final-content.jsonl
python $T/tune_report.py rates $W/held.jsonl --paired aec [--no-mls]
python $T/tune_report.py det $W/held.jsonl --negatives $W/negs.jsonl --content $W/final-content.jsonl --variant B --no-mls
```

Tests: `python3 -m unittest discover -s tools/audio-rig/aec_eval -p 'test_*.py'`; `cargo test -p webos-dsp`.

## Rollback and cleanup

Nothing on a device changed. The `[aec.aec3]` table is empty by default, `agc2` is not exposed in the daemon, and the stereo render for the linear and near-end outputs only removes a validation error. The private outputs can be deleted.

## Follow-ups (parked)

These are listed in the [design](../design/aec-frontend.md#12-tuning-results-and-parked-state): the TV delay lock and ERLE with a stimulus, a false-wake soak of hours with B, a JBL talker A/B, a live-person test, STT on the kept WAVs, and the TV CPU of B.
