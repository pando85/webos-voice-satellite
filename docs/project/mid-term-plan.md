# Mid-term plan: wake word, audio front-end and evaluation (November 2026 to January 2027)

- **Date:** 2026-11-07. **Status:** proposal for the owner; nothing here is decided until section 5 is answered. Desk work on the PC only: no TV access, no Home Assistant (HA) change, no audio captured or read.
- **Scope:** the two briefs pasted by the owner (A: architecture and profiles; B: using both microphones) and the coordinator's list of behaviour changes, judged against what this repository contains and has measured. Horizon about 10 weeks.
- **Companion (server side):** [pando85/homelab `docs/voice-assistant/audio-pipeline-plan.md`](https://github.com/pando85/homelab/blob/master/docs/voice-assistant/audio-pipeline-plan.md) covers the HA pipeline, Whisper, openWakeWord, profiles, other satellite classes and the data register. Both plans share the item IDs `MT-n` and one critical path (section 4).
- **Related:** [roadmap](roadmap.md), [backlog #1](backlog.md#1-professional-audio-front-end) and [#6](backlog.md#6-observability), [HA-side audio processing](../research/ha-side-audio-processing.md), [AEC front-end](../design/aec-frontend.md), [D3](../discovery/c3-a/D3-mic-identification.md), [EXP-018](../experiments/EXP-018-aec-live-c3-a.md), [EXP-021](../experiments/EXP-021-aec-live-off-vs-bns.md), [P2 AEC tuning](../experiments/P2-aec-tuning.md), [ADR 0011](../decisions/0011-aec-on-by-default.md).

Labels follow [AGENTS.md](../../AGENTS.md): **documented**, **observed-on-other-device**, **hypothesis**, **verified-on-target** (c3-a unless named), plus **verified-on-host** for PC runs. The household's custom wake word is called "the custom wake word" here; its name, data and numbers stay in the private wake word repository.

## 1. Facts that drive the plan

| # | Fact | Label, source |
| --- | --- | --- |
| F1 | HA 2026.10.4 ignores the satellite's `audio_settings`; one stream feeds the wake word and then STT; HA's enhancer cannot be set per stage; debug recordings equal what the wake word and STT receive | documented, verified-on-target (HA install), [HA-side audio §1](../research/ha-side-audio-processing.md#1-where-ha-processes-satellite-audio-ha-2026104) |
| F2 | A second channel (`data2`, `MULTI_CHANNEL_AUDIO`) reaches **STT only**, and only if the STT service reports both `prefers_*` flags false; wyoming-faster-whisper 3.8.1 reports true. This crate rejects the feature bit (`esphome-api/src/config.rs`) | documented, [HA-side audio §1.3](../research/ha-side-audio-processing.md#13-the-second-audio-channel-multi_channel_audio) |
| F3 | wyoming-openwakeword 2.1.0: one global threshold, no NS or VAD; `--trigger-level` counts frames cumulatively per run | documented, [HA-side audio §2.4](../research/ha-side-audio-processing.md#24-wake-word-side-options-no-audio-change) |
| F4 | The custom wake word was trained through an emulation of today's front-end (ch0, HPF 120 Hz, AEC3 + NS, +24 dB). **Any front-end change (channel combination, AGC, NS level) changes its input and needs an evaluation, probably a retrain.** It fails its own room false-wake target offline and its real recall is unmeasured | private wake word repository; stated in [HA-side audio](../research/ha-side-audio-processing.md) |
| F5 | AEC B + NS is on by default since 0.4.3 on product grounds; two live A/Bs found **no significant** wake gain and no harm; STT was never cleanly compared | verified-on-target, [EXP-018](../experiments/EXP-018-aec-live-c3-a.md), [EXP-021](../experiments/EXP-021-aec-live-off-vs-bns.md) |
| F6 | Two mics about 8 cm apart (fit), ch0 leads ch1 by 162 to 170 µs for one loudspeaker position. Wideband SNR ch0 9.7, ch1 8.7, mean 7.9 dB, but **swept-band median SNR ch0 25.9, ch1 27.1, mean 26.3 dB**: the wideband figure is dominated by 150 to 300 Hz room noise. Noise coherence 0.94 at 100 to 300 Hz, 0.74 at 0.3 to 1 kHz, ≤ 0.04 above 1 kHz. One run, one position, a synthetic voice | verified-on-target, [D3 Q3/Q4](../discovery/c3-a/D3-mic-identification.md#answers) |
| F7 | `[dsp] channel = left\|right\|mix` already exists (`dsp/src/frontend.rs`), one HPF after the combination; no delay-and-sum | documented (repository source) |
| F8 | `sonora` 0.2.0 supports **multichannel capture** with AEC3 (`Pipeline::multi_channel_capture`, default true; per-channel linear filters and a shared suppression gain in `sonora-aec3`). The daemon passes a mono capture (`aec.rs`: `StreamConfig::new(RATE, 1)`), and the direct engine used by B's `linear_mix` is hard-wired to one capture channel (`aec/direct.rs`: `FrameBlocker::new(1, 1)`). `render_channels = 2` is the TV reference, not the mics | documented (registry source of sonora 0.2.0 and sonora-aec3 0.2.0; this repository) |
| F9 | `hw:LGSIC,3` (the AEC reference) carries the **HDMI program and the satellite's own `pacat` stream**, post-volume, on the TV speaker. **Not checked:** built-in webOS apps, the tuner, LG system sounds, ARC/eARC, optical and Bluetooth outputs, c3-b | verified-on-target for what is listed, [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md#other-lgsic-taps-mixercapture-soundenginecapture), [EXP-018 H3](../experiments/EXP-018-aec-live-c3-a.md#4-the-satellites-own-music-in-the-reference-h3); unknown for the rest |
| F10 | The daemon already logs every 60 s (debug): input and output RMS/peak, clipped samples, xruns, chunks sent and dropped, AEC delay, ERLE, reference underruns, sonora µs per frame. Not per wake, not per channel, not exported; the ESPHome crate has no numeric sensor entity yet | documented (`satellite/src/capture.rs`, `esphome-api/src/entities.rs`) |
| F11 | The satellite receives `WAKE_WORD_END` (as `WakeWordDetected`) and the other pipeline events for the overlay; `STT_START` is only logged | documented (`esphome-api/src/voice.rs`) |
| F12 | AGC2 (limiter or adaptive) had no effect and RNNoise removed the talker at about 4× the canceller's cost, offline on semi-synthetic c3-a mixes | observed-on-other-device, [P2 AEC tuning](../experiments/P2-aec-tuning.md) |
| F13 | An offline replay of real takes and about 33 h of room audio through a second WebRTC NS, RNNoise and speex, scored with several wake word models, is **running now** in another session | pending; method in [HA-side audio §9](../research/ha-side-audio-processing.md#9-offline-replay-experiment-no-retrain) |

### 1.1 Raw multichannel replay data: what exists and what is missing

A paired offline replay (raw ch0, ch1 and the reference, recorded once and run through every front-end variant) is the key enabler: every variant sees the same audio, so the comparison is paired by construction and needs no live counterbalancing.

| Piece | Status |
| --- | --- |
| Raw capture tool | **Exists:** [`tv_dualcap.py`](../../tools/audio-rig/tv_dualcap.py) runs on the TV (stdlib Python over the TV's libasound), linked `hw:LGSIC,0` S24 2 ch 16 kHz plus `hw:LGSIC,3` S32 2 ch 48 kHz, streamed to the PC, at most 600 s per call; verified-on-target in P2 session 2, EXP-018 and EXP-021. The mic PCM is exclusive, so **the satellite must be stopped**: no live HA result during a raw recording |
| Replay through the exact front-end | **Exists:** [`aec_offline`](../../crates/capture/examples/aec_offline.rs) (the daemon's code; `--channel left\|right\|mix`, NS, B overrides, AGC2, RNNoise) and [`chain_eval.py`](../../tools/audio-rig/aec_eval/chain_eval.py) |
| Raw 2-channel recordings already on the PC (private) | **Exist:** P2 session 2 beds (quiet 150 s; content at volume 10 and 15, 600 s each; barge-in), EXP-018 talker calibration (115 s), EXP-021 capture (40 synthetic-voice items over content at volume 10, 290 s), D3 sweeps and speech (mic only). All synthetic voices through a loudspeaker; no person |
| Delay-and-sum or other combinations | **Missing** in `dsp` and `aec_offline` |
| Mic-to-mic TDOA estimator | **Missing** (the existing GCC-PHAT in `dsp/src/delay.rs` is reference-to-mic) |
| Scorer of a continuous stream with the server's exact features | **Exists outside this repository** (homelab wake word scripts, server-exact features). This repository's replica (`aec_eval/oww_wyoming.py`) scores each item as a fresh stream and read 40 to 48 % where live gave 59 to 61 % on the same items ([EXP-021 §5](../experiments/EXP-021-aec-live-off-vs-bns.md#5-offline-re-scoring-of-the-mic--reference-capture)) |
| STT scoring | **Exists:** `aec_eval/stt_eval.py` (WER against known text) |
| Real people, prompted text, consent | **Missing:** no raw recording of a person; no consented command set for WER ([HA-side audio §5](../research/ha-side-audio-processing.md#5-measurement-resources), gap a) |
| Raw negatives for false wakes per hour | **Missing:** the 33 h of room audio are post-front-end (mono) |
| Session wrapper (chained 600 s calls, prompts, manifest, checksums) | **Missing** |
| Raw tap while the satellite runs | **Missing** (would allow raw audio with the live HA result at once; privacy-heavy, section 5) |

## 2. Verdicts on the briefs

Verdicts: **do now**, **after evidence X**, **design only**, **reject**.

### 2.1 Brief A (architecture)

| Item | Verdict | Reason (label, source) |
| --- | --- | --- |
| Capability and processing profiles as YAML in homelab | **Design only** now; implement when a second device class exists (MT-11) | HA cannot consume them: it ignores `audio_settings` and has no profile concept (documented, F1). Useful as the manifest that the training, release and evaluation scripts read; with one device class today it would describe one row |
| Per-stage DSP ownership, no double NS or AGC | **Do now** (a table in MT-11's design and in the companion) | Today: the satellite owns HPF, AEC, NS and gain; HA adds nothing (documented, F1). A second NS in front of the wake word is a model change (F4) |
| Verify what HA and ESPHome support | **Done** | Per-pipeline enhancement: no. Per-satellite pipeline: yes, a select per satellite (verified-on-target, EXP-018 prerequisites). Multi-stream: STT only, conditional (F2) |
| Dual processed streams (enhanced for the wake word, lighter for STT) | **After evidence** (MT-7 shows a WER gain from lighter processing); then prefer MT-7's single-stream switch | Contradicts HA 2026.10.4: `data2` never reaches the wake word and reaches STT only with a patched STT service (F2). The run-phase NS switch gives STT the same audio with no server change |
| Keep the wake word stream continuous, no hard VAD gating | **Do now** (rule; already true) | The satellite streams continuously in wake mode; a gate before the wake word changes its input (F4) and can drop quiet speech |
| A future Wyoming audio proxy service | **Design only** (companion) | Build only after the running replay (F13) or MT-7 shows a measured gain |
| Per-satellite and per-profile observability | Device health: **do now** (MT-3). Recall, WER: **offline evaluation, not live telemetry** | Live recall and WER need ground truth that live runs do not have; false wakes per hour need labels (MT-2) |
| P0 audit, profiles, ownership, validation, docs | Audit and ownership: done here and in the companion. Profiles: design only. Validation: MT-4 | — |
| P1 dual stream, observability, benchmark tooling | Observability and tooling move to P0 (MT-3, MT-4); dual stream after evidence | Evaluation must exist before the changes it judges |
| P2 design only | Agree | — |
| ReSpeaker XVF3800 and plain ESP32 satellites | **Design only** (companion, MT-11) | Each class has its own front-end, so its own wake word evaluation and probably its own model variant (F4) |

### 2.2 Brief B (dual microphone)

| Item | Verdict | Reason (label, source) |
| --- | --- | --- |
| Use both mics instead of ch0 | **After evidence:** MT-4 on existing recordings, then MT-7 on household recordings | The only evidence is one run, one position, a synthetic voice (F6). Diffuse noise is coherent below 1 kHz, so a 2-mic delay-and-sum can gain little there and at most about 3 dB above (hypothesis from F6's coherence). The interference behind false wakes is most likely speech (TV, people), which a fixed delay-and-sum does not null |
| "ch0 is better" (9.7 against 8.7 dB) | **Corrected** | Wideband only. Swept-band median favours ch1 (27.1 against 25.9 dB) (F6). ch0 is the default because the model was trained on it, not because it is proven better |
| P0 combination abstraction: ch0, ch1, average (negative control), fixed-steering fractional-delay delay-and-sum, best-channel | **Do now as offline code** in `dsp`, used by `aec_offline` (MT-4); **daemon wiring after evidence** (MT-10) | ch0, ch1 and mean exist (F7). Same code offline and on the TV keeps the replay exact |
| Independent HPF state per channel | **Only where needed** | For fixed steering the HPF and the delays are linear and time-invariant, so one HPF after the sum is equivalent. Per-channel state is needed before per-mic AEC or a switching best-channel selector |
| No allocations in the real-time path, config validation, rollback, HA format unchanged | **Agree** (MT-10 acceptance) | Matches the current front-end and config loader |
| Unit tests with synthetic delays, offline benchmarks, geometry docs | **Do now** with the offline code (MT-4) | Geometry: which mic is ch0 and the array orientation are unknown (D3) |
| Architecture A (beamformer → AEC3 → NS → gain) first | **Agree**, with **fixed** steering only | A time-varying beamformer before AEC3 changes the echo path it tracks |
| B (per-mic AEC, then beamform) | **After A shows a gain** | About double the AEC cost: armv7 11 % of a core today, aarch64 5 to 7 % (verified-on-target, [AEC §3.10](../design/aec-frontend.md#310-cpu-memory-size)); doubled is a hypothesis |
| C (native multichannel capture in sonora) | **Exists in the library** (F8), not wired; it is B done inside AEC3 | Needs the direct engine extended to two capture channels for B's `linear_mix` |
| Mic-to-mic TDOA versus reference-to-mic GCC-PHAT | **Agree**; new estimator, offline first (MT-4) | F7, F8 |
| No RNNoise, stacked NS, aggressive VAD before the wake word, extra AGC without measurement | **Agree** | F12; [HA-side audio §3](../research/ha-side-audio-processing.md#3-interaction-with-the-device-side-processing) |
| Paired, randomized, counterbalanced evaluation with real household voices | **Agree, restructured:** record raw once (MT-5), replay every variant offline (paired by construction); counterbalance only the final live confirmation (MT-10) | A live arm-by-arm design needs 60 to 90 s per trial now that the agent answers ([EXP-021](../experiments/EXP-021-aec-live-off-vs-bns.md)); the full condition grid is not feasible live |
| Conditions grid (quiet, TV silent, speech, music, talkers, distances, angles, overlap, volumes) | **Reduced** to what one or two sessions can fill (MT-5) | With about 60 items per cell, only differences of about 15 points are detectable ([HA-side audio §9](../research/ha-side-audio-processing.md#9-offline-replay-experiment-no-retrain)) |
| Score with the deployed custom wake word and its exact streaming frontend | **Agree, plus** a stock model and model-free metrics | The custom model was trained on ch0 (F4): it is biased towards the status quo. A gain seen only with the stock model is a retrain signal, not a ship signal |
| Metrics list | **Agree**, ranked: recall at the operating point, false wakes per hour, WER first; SNR, ERLE, clipping, CPU, frame time, memory, drops second | Real behaviour first |
| Observability fields | **Do now** in MT-3 | No audio or transcripts (AGENTS.md) |
| P1 (steering calibration, AEC order, on-TV validation, opt-in) | **After MT-7 go** | — |
| P2 (adaptive steering never at the TV, multichannel AEC, MVDR/GSC) | **Design only** | Two mics at 8 cm |
| Final rule: keep ch0 unless something beats it convincingly | **Agree**; MT-7 defines "convincingly" | — |

### 2.3 The coordinator's list

| # | Item | Verdict | Reason (label, source) |
| --- | --- | --- | --- |
| 1 | Wake word operating point from real takes | **Do now** (MT-1, in progress, private) | Attacks false wakes with no input change; a threshold change ships as a recalibrated model file because the server threshold is global (F3) |
| 2 | Per-run satellite diagnostics to Grafana | **Do now** (MT-3) | Counters exist but are 60 s debug lines (F10); per-run and per-channel values and an export path are missing |
| 3 | TV-dialogue false-wake flag (tag, maybe abort) | Tag: **do now** inside MT-3. Abort: **after evidence** (MT-9) | The reference covers HDMI and the satellite's own stream on the TV speaker; built-in apps, the tuner and other outputs are **not verified** (F9). A false wake from a person or another device is not echo and is not tagged. EXP-018, EXP-021 and P2 show the reference is clean and locks within seconds, but none of them measured coverage of app audio |
| 4 | Switch NS mid-run at wake-word-end / STT start | **After evidence** (MT-7: offline WER of NS on against off) | Feasible: the satellite gets `WAKE_WORD_END` (F11). Mind: continued-conversation runs start at STT; NS changes microVAD's end-of-command input; switching NS on and off resets its noise estimate (run two NS instances instead). P2 found NS neutral to slightly helpful offline; STT never compared cleanly (F5) |
| 5 | Quiet speakers (a second household adult reaches the mic about 15 dB quieter; fixed +24 dB): test one AGC offline | **Do now, offline** (MT-4 on existing private takes; MT-7 on household data) | Post-front-end gain can be emulated exactly on HA's recordings. Any AGC in the wake path changes the model's input (F4) and can raise the floor between words; score false wakes on the 33 h too |
| 6 | One pipeline per device class with its own model variant; profiles consumed by training and release scripts, not HA | **Agree; design only** until a second class exists (MT-11) | HA selects a pipeline per satellite (verified-on-target). Caveats in the companion: the second pipeline select only works with on-device wake words, and the select restore after an HA restart was observed stale |
| 7 | Whisper settings (VAD filter on since homelab `5690fe717`; language, model, prompt) | **Do now: measure, do not change** (MT-6) | Language `es` fixed, `large-v3-turbo` int8, beam auto, a static verbs prompt plus live HA names (verified in the homelab values). No WER data on real commands exists to justify a change |

### 2.4 Where the briefs contradict measurements or HA 2026.10.4

- **Dual streams over the ESPHome API** cannot give the wake word a different stream than STT's channel 0; `data2` is STT-only and conditional (F2).
- **Profiles "consumed" by HA:** HA ignores the satellite's audio settings and has no profile mechanism (F1). The consumers are scripts.
- **Double NS:** HA's enhancer is unreachable for ESPHome satellites and would sit before both stages (F1); a second NS in front of the wake word is a model change (F4).
- **"ch0 is the better mic"** rests on a wideband SNR that the swept-band figures do not support (F6).
- **"Use the deployed custom wake word" as the judge** of a front-end change is biased towards ch0 (F4).
- **"Native multichannel capture in sonora, if it exists":** it exists in sonora 0.2.0 (F8).

## 3. Phased plan

Order: expected impact on real wake word and STT behaviour per unit of effort, with the evaluation infrastructure before the DSP changes it judges. Where: **sat** = this repository, **HA** = homelab, **priv** = private wake word repository. Effort in agent-days (d) and owner time.

| ID | Item | Where | Weeks | Effort | Depends on |
| --- | --- | --- | --- | --- | --- |
| MT-1 | Wake word operating point from real takes | priv, HA | 1-2 | 1-2 d; owner 2 × 30 min | owner takes |
| MT-2 | False-wake labels and wake counts per device | HA | 1 | 1 d; owner presses a button | — |
| MT-3 | Per-run satellite diagnostics and echo tag (0.5.2) | sat, HA | 1-3 | 4 d | 0.5.1 shipped |
| MT-4 | Paired replay harness, first pass on existing raw recordings | sat, HA scripts | 2-4 | 4-5 d | — |
| MT-5 | Household raw recording sessions and an empty-room TV soak | sat tools, owner | 3-5 | 2 d; owner 2-3 × 45 min, 4 h unattended | MT-4, consent, TV authorization |
| MT-6 | STT baseline and Whisper settings on labelled commands | HA | 4-6 | 2 d | MT-5 |
| MT-7 | Front-end variant decision: two mics, AGC, run-phase NS | sat (offline) | 5-7 | 3 d | MT-4, MT-5 |
| MT-8 | Decision on the running filter replay; STT-only denoise design | HA | 2-6 | 1 d | F13 result |
| MT-9 | Echo-dominance abort | sat | 7-9 | 2 d | MT-2 and MT-3 data |
| MT-10 | Ship the chosen change opt-in, retrain, live confirmation | sat, priv, HA | 7-11 | 6-8 d + retrain | MT-7 go, owner retrain decision |
| MT-11 | Profiles manifest, device-class pipelines, new satellite classes | HA | 6-10 | 2 d design | — |
| MT-12 | Raw tap in the daemon | sat | after MT-5 | 3 d | owner privacy decision |

### MT-1. Wake word operating point from real takes (in progress)

- **Problem:** false wakes in daily use; the model fails its offline room target and its real recall is unknown.
- **Evidence:** private (F4); the server threshold is global, so the operating point lives in the model file (F3).
- **Change:** pick the threshold on real takes and room audio, ship a recalibrated file. No satellite change.
- **Measure and accept:** the owner's release gate on held-out real takes, per speaker, and room false wakes per hour with intervals; then live false wakes per device per day from MT-2 labels, against the week before, with no rise in reported misses.
- **Owner:** record sessions on c3-a and takes on c3-b (in progress), choose recall against false wakes, approve the release.
- **Risks:** too few takes for a tight interval. **Rollback:** the previous model version or `alexa` (homelab release script).

### MT-2. False-wake labels and wake counts per device (HA)

- **Problem:** false wakes per hour in real use are known only from the owner's memory; MT-1, MT-3 and MT-9 need labels.
- **Evidence:** HA keeps every run's audio by run directory (homelab data register); an `input_button` already marks takes.
- **Change:** an HA "false wake" button whose press is joined to the device's last wake run; a script that counts wake runs per device per hour from the recording directory names (counts only). Detail in the companion.
- **Measure and accept:** in a test with the owner, ≥ 95 % of presses join the right run within 30 s; counts match HA's state history on one day.
- **Owner:** approve the helper; press it after false wakes for two weeks. **Effort:** 1 d. **Risks:** labelling fatigue (count only presses, not misses). **Rollback:** remove the helper.

### MT-3. Per-run satellite diagnostics and the echo tag (release 0.5.2)

- **Problem:** nothing tells whether a false wake or a miss came with clipping, a missing reference, heavy TV echo, drops or CPU stalls (F10).
- **Change:** at each `WakeWordDetected`, summarise the last 2 s from a fixed ring of 10 ms frame statistics (no allocation): per-channel input RMS and peak (both mics, even while only ch0 feeds the stream), output RMS, clipped samples, reference open and active share, **echo dominance** (AEC input over output power while the reference is active, the tag of item 3), xruns, reference underruns, dropped chunks. At run end add stage timings from the pipeline events and the run outcome (error code, transcript length, never text). One info log line per run; numeric diagnostic sensors through a new ESPHome sensor entity type, so HA's Prometheus export and Grafana get them (dashboard in the companion). Algorithm, channel mode and steering are reported once per mic open.
- **Measure and accept:** unit tests for the ring and the summaries; on c3-a after install, one line and one sensor update per wake run, the tag present on ≥ 95 % of wake runs with the reference open, daemon CPU up by ≤ 0.3 points of a core, a log leak review (no audio, no text).
- **Owner:** approve the new diagnostic entities. **Effort:** 4 d. **Risks:** entity churn on the HA device page (diagnostic category, rate-limited updates). **Rollback:** the previous release; the sensors go unavailable.

### MT-4. Paired replay harness, first pass on existing raw recordings (PC)

- **Problem:** no tool can run one raw recording through every variant and score it the way the server does (section 1.1).
- **Change:** in `dsp`: a mic-combination stage (ch0, ch1, mean, fixed-steering fractional-delay delay-and-sum, best-channel with per-channel HPF), a mic-to-mic TDOA estimator, unit tests with synthetic fractional delays; `aec_offline --combine ... --steer-us ...`; a driver that writes one aligned mono stream per variant (also post-front-end AGC and NS-off variants) and hands them to the homelab continuous-stream scorer, the STT scorer and signal metrics (in-band SNR, ERLE, clipping, frame time). First readouts, on existing private data: (a) TDOA of the TV echo between the mics from the P2 content beds, against the talker's from EXP-021: if they are equal, no fixed steering separates them; (b) in-band SNR of each combination on the EXP-021 talker items; (c) the AGC variant on the second speaker's existing real takes and the 33 h of negatives.
- **Measure and accept:** the ch0 variant reproduces `aec_offline`'s current output sample for sample; the scorer agrees with HA's live decisions on HA's own recordings of a known set in ≥ 95 % of items. Readout (b) of < 1 dB in-band gain for delay-and-sum is recorded as a likely no-go for MT-7.
- **Owner:** none (PC only, private data stays local). **Effort:** 4-5 d. **Risks:** the existing recordings are a loudspeaker at one position. **Rollback:** none needed (tools only; daemon untouched).

### MT-5. Household raw recording sessions and an empty-room TV soak

- **Problem:** no raw recording of a person; no labelled commands for WER; no raw negatives (section 1.1).
- **Change:** a session wrapper around `tv_dualcap.py` (chained 600 s calls, the prompter, a manifest, checksums, satellite stop and restart). Prompts are "wake phrase + command" with known text, so WER needs no human transcription. Grid: 2 speakers × 2 distances × 2 angles × {quiet, TV speech, TV music} at the household's normal TV volume, about 100 items per speaker. Plus at least 4 h of TV content in an **empty room**, raw, for false wakes per hour of TV audio.
- **Measure and accept:** 0 xruns on both PCMs, every item located by template match, manifest complete; the soak bounds false wakes at ≤ 0.75 per hour with zero events.
- **Owner:** consent from each speaker; authorize each TV session (TV access is otherwise install-only); be present to restore the satellite. **Effort:** 2 d agent, owner 2-3 × 45 min. **Risks:** the household loses the assistant during a session; the room or the TV volume differ from daily use. **Rollback:** satellite restart (`ctl.sh start`); delete the private session directory.

### MT-6. STT baseline and Whisper settings (HA)

- **Problem:** no WER on real commands; settings were chosen on synthetic audio.
- **Change:** offline WER per condition on MT-5 items through the deployed front-end; the VAD filter's effect on the noise-to-text rate (companion). Change a Whisper setting only on evidence.
- **Measure and accept:** a setting changes only if pooled WER improves by ≥ 2 points absolute and no condition gets worse by > 2 points. **Effort:** 2 d. **Rollback:** homelab values revert.

### MT-7. Front-end variant decision (offline, on MT-5 data)

- **Problem:** brief B and items 4 and 5 need one decision on one dataset.
- **Change:** score ch0 (control), ch1, mean (negative control), delay-and-sum (steering from MT-4), best-channel; AGC; NS on against off in the STT segment only (item 4); with the custom model, a stock model, STT and the signal metrics.
- **Accept (go):** against ch0 with the custom model: recall at its operating point up by ≥ 10 points pooled, paired McNemar p < 0.05, no condition down by > 5 points; false wakes on the soak not higher; WER not worse by > 2 points; CPU +≤ 1 point of a core on aarch64 and ≤ 3 on armv7 (estimated from the host ratio, then measured). A gain only with the stock model is reported as a **retrain candidate** (owner decision), not a ship decision. Otherwise **keep ch0** and record why. For item 4: WER better by ≥ 2 points with NS off in STT, else no switching.
- **Effort:** 3 d. **Risks:** small effects stay undetectable; say so. **Rollback:** nothing deployed.

### MT-8. The running filter replay (HA)

- Wait for its result (F13); then decide on an STT-only denoise stage and the Wyoming audio proxy design in the companion. No satellite work. **Effort:** 1 d.

### MT-9. Echo-dominance abort

- **Problem:** TV dialogue may cause false wakes the AEC leaves behind.
- **Evidence needed:** MT-3 tags on ≥ 30 labelled false and ≥ 30 true wakes (MT-2), and a short authorized check that `hw:LGSIC,3` carries a built-in app's audio (F9: daemon debug log shows a delay lock while only the app plays; 5 minutes).
- **Change:** if the tag separates them, abort the run on a tagged wake (setting, off by default).
- **Accept:** rejects ≥ 50 % of labelled false wakes and ≤ 2 % of true ones. **Effort:** 2 d. **Rollback:** the setting off.

### MT-10. Ship the chosen change, retrain, confirm live (only after an MT-7 go)

- **Change:** wire the combination into the daemon (`[dsp]` keys, default ch0, validated, no allocation in the real-time path), opt-in; extend the direct AEC engine if a variant needs two capture channels (F8). Retrain the wake word with the new stage in its emulation (needs a two-channel emulation, private); while c3-a and c3-b differ, one pipeline per front-end profile (MT-11).
- **Accept:** a counterbalanced live A/B with consenting people on c3-a confirms MT-7's direction (≥ 40 paired items per arm), false wakes per day (MT-2) not higher over two weeks; only then the owner may change the default.
- **Owner:** the retrain decision, the live session, the default. **Effort:** 6-8 d plus training. **Risks:** a retrain that loses recall; two model variants to maintain. **Rollback:** `[dsp]` back to ch0 and the previous model.

### MT-11. Profiles, device-class pipelines, new satellite classes (design)

- In the companion: where the profile manifest lives, who reads it, the per-stage ownership table, what an XVF3800 and a plain ESP32 own on the device. Implemented when the hardware arrives.

### MT-12. Raw tap in the daemon (optional)

- An opt-in, time-limited tap that streams raw ch0, ch1 and the reference to the PC while the satellite keeps serving HA, or keeps a short raw buffer around each wake. Only if MT-5's stop-and-record sessions prove too costly, or the owner wants raw audio around real false wakes. It records household audio: section 5.

## 4. Dependencies and critical path

```text
week   1      2      3      4      5      6      7      8      9     10     11
0.5.1  [security fix fc882a3 only, deploy to both TVs]
MT-1   [real takes -> recalibrated model]--------------------------------> (live FA/day check)
MT-2   [button + counts]-----labels accumulate------------------------------>
MT-3   [diagnostics -> 0.5.2]------tags accumulate-------->MT-9 [abort?]
MT-4          [harness + first pass on existing data]
MT-5                        [household sessions + soak]
MT-6                               [WER baseline, Whisper]
MT-7                                      [go / no-go]--->MT-10 [opt-in 0.6.0 + retrain + live A/B]
MT-8          [filter replay result]-->[STT-only denoise design]
MT-11                                      [profiles + classes design]
```

- **Critical path:** MT-4 → MT-5 (owner consent and TV sessions) → MT-7 → MT-10 (retrain, private) → live A/B. Owner availability for MT-5 is the gating item; December holidays may move it.
- **Releases:** **0.5.1** = the install permissions fix (`fc882a3`) only, first, so the security fix does not wait for features. **0.5.2** = MT-3 (and MT-9 later, off by default). **0.6.0** = MT-10, only after a go. Each release is deployed to c3-a per the deploy policy (install only).
- **Private wake word work:** MT-1 runs in parallel and does not wait for the satellite. Any MT-10 change needs a retrain before it becomes a default; until then the old model and the old front-end stay paired.
- **Competing TV time:** EXP-026 (mic-key grab trial) waits for the owner's authorization and needs its own session; schedule it apart from MT-5.

## 5. Decisions the owner must make

1. **PRs or direct commits to main** (so far direct). Suggestion: PRs for daemon changes that touch the audio path (MT-3, MT-9, MT-10), direct for docs and offline tools.
2. **Household recording sessions (MT-5):** who records, consent of each speaker, which TV content, where the raw files live and for how long (private, local, never committed), and whether **real command recordings** may be kept for WER.
3. **Empty-room TV soak:** 4 h of raw capture with the satellite stopped and nobody in the room.
4. **Retrain trigger:** may a beamforming, AGC or NS change trigger a wake word retrain, and may two model variants run during a transition (one pipeline per front-end profile)?
5. **Operating point (MT-1):** recall against false wakes per hour for the recalibrated model.
6. **Diagnostics (MT-3):** new diagnostic entities on the HA device page and their export to Prometheus and Grafana.
7. **False-wake button (MT-2):** the helper, and whether the household uses it.
8. **Abort on echo-dominated wakes (MT-9):** acceptable rate of rejected real wakes.
9. **Raw tap (MT-12):** default no; any raw buffer around real wakes records household audio.
10. **TV sessions:** each MT-5 session and the 5-minute app-audio reference check (MT-9) need authorization under the install-only rule.

## 6. Not doing, and why

| Not doing | Why |
| --- | --- |
| Dual streams over the ESPHome API now | `data2` never reaches the wake word and needs a patched STT service (F2); the run-phase NS switch covers the STT case if MT-7 justifies it |
| Profiles consumed by HA | HA has no such input (F1) |
| HA's speex enhancer or any second NS in front of the wake word | Unreachable for ESPHome satellites, before both stages, a model change (F1, F4) |
| RNNoise in the front-end | Removed the talker at about 4× the canceller's cost (F12) |
| AGC or VAD gating in the wake path without MT-7 evidence | Changes the model's input; AGC2 showed no effect offline (F4, F12) |
| Per-mic AEC, MVDR, GSC, adaptive steering | No evidence that even fixed delay-and-sum helps; two mics at 8 cm |
| Live recall and WER dashboards | No ground truth in live runs; measured offline on labelled sets |
| Building the Wyoming audio proxy | No measured benefit yet (MT-8) |
| Changing the default channel, NS or gain before a retrain | The deployed model was trained on today's chain (F4) |
