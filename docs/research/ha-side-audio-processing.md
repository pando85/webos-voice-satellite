# Home Assistant side audio processing: options against false wakes and noise transcribed as words

- **Date:** 2026-11-07. **Scope:** desk research on the PC only. Read-only reading of the Home Assistant (HA) core package installed in the reference deployment (HA **2026.10.4**, Python 3.14), of the installed `wyoming-faster-whisper` **3.8.1** and `faster-whisper` **1.2.1** packages in the STT server, of `wyoming-openwakeword` **2.1.0** and `pyopen-wakeword` **1.1.0** source, of HA core history on GitHub, and of public documentation. **Nothing was changed on HA, its pipelines or the cluster, and nothing was run on a TV.** No audio, transcript or conversation history was read.
- **Owner question (2026-11-07):** which options exist to process the audio at the HA level to remove noise, after the device's own filters and before the wake word.
- **Problem being addressed:** false wakes, and room noise transcribed as words by STT.
- **Key trade-off:** the deployed custom wake word was trained on audio passed through an emulation of exactly this satellite's front-end (high-pass, WebRTC AEC3 with NS, +24 dB). Any processing that changes the audio before the wake word changes the model's input distribution and needs a re-evaluation, possibly a retrain.
- **Related:** [AEC front-end design](../design/aec-frontend.md), [ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md) (server-side wake word), [ADR 0011](../decisions/0011-aec-on-by-default.md) (AEC and NS on by default), [EXP-021](../experiments/EXP-021-aec-live-off-vs-bns.md), [HA ESPHome integration notes](ha-esphome-integration-notes.md), [wake word and VAD](04-activation.md).

Labels follow [AGENTS.md](../../AGENTS.md): **documented** (upstream source or documentation, with version), **observed-on-other-device**, **hypothesis**, **verified-on-target**. In this note **verified-on-target** means read from the reference HA deployment's installed code or configuration (not from a TV), on 2026-11-07.

## Summary

1. **HA applies no noise suppression to this satellite today, and the satellite cannot ask for it.** Since HA 2024.10 the ESPHome integration ignores the `audio_settings` (`noise_suppression_level`, `auto_gain`, `volume_multiplier`) of `VoiceAssistantRequest`. Every ESPHome satellite run gets `AudioSettings(silence_seconds=<VAD sensitivity>)`, so speex NS and auto gain are off, volume is 1.0, and only microVAD runs. Our satellite sends 0, 0, 1.0 anyway. (documented, verified-on-target)
2. **Where HA's enhancer would sit:** once, on the single audio stream, **before both** the wake word and STT. It cannot be enabled for STT only. microVAD runs on the chunk **before** speex. (documented, verified-on-target)
3. **There is no HA setting** (pipeline, YAML or UI) for NS or auto gain of a satellite. The only per-satellite audio knob is the **VAD sensitivity** select (end-of-command silence 0.25, 0.7 or 1.25 s). (documented)
4. **wyoming-openwakeword 2.1.0 has no NS or VAD option.** It uses `pyopen-wakeword`, not the `openwakeword` Python package, so `enable_speex_noise_suppression` and `vad_threshold` do not exist there. Its knobs are `--threshold` (global, all models), `--trigger-level` and `--refractory-seconds`. `--trigger-level` counts frames above the threshold **cumulatively since the stream started**, not consecutively. (documented)
5. **The STT server runs without its VAD filter.** `wyoming-faster-whisper` 3.8.1 offers `--vad-filter` (Silero, documented as reducing hallucinations), `--vad-threshold`, `--vad-min-speech-ms`, `--vad-min-silence-ms`, `--vad-endpointing`, `--vad-clip` and `--beam-size`; the reference deployment uses none of them. `no_speech_threshold` and `log_prob_threshold` are not exposed (faster-whisper defaults: 0.6 and -1.0). (documented, verified-on-target)
6. **No maintained Wyoming "denoise proxy" exists.** wyoming-satellite has `--mic-noise-suppression` and `--mic-auto-gain` (`webrtc-noise-gain`), but on the satellite and not as a server. The closest server-side project is a speaker-verification proxy in front of STT ([wyoming-voice-match](https://github.com/jxlarrea/wyoming-voice-match)). (documented)
7. **Recommendation:** fix the decision stages first, then STT-only denoising. Leave the audio before the wake word alone until the data show that non-speech noise drives the false wakes. Ranked plan in [section 6](#6-recommendation-a-staged-plan).
8. **"Filter everything centrally, then use a normal model"** is not better by default ([section 8](#8-normalise-then-detect-one-central-filter-and-one-normal-model)): a central filter cannot do AEC, cannot undo what each device already did, does not equalise microphones or distance, and keeps TV dialogue and people talking, which a wake word false accept is most likely made of. The literature finds that enhancement before keyword spotting helps models trained on clean speech, not models trained with noise augmentation (as openWakeWord-recipe models are). Whether it helps here is cheap to measure offline with no retrain ([section 9](#9-offline-replay-experiment-no-retrain)).
9. **Offline replay result (2026-11-07, [section 10](#10-results-of-the-offline-replay-2026-11-07)):** no filter (HA's speex at level 2, RNNoise, a second WebRTC NS pass) lowered the deployed custom wake word's false accepts on 8.3 h of held-out room audio (0.96/h unfiltered, 0.96 to 1.44/h filtered), and speex and RNNoise lowered the recall of the custom models in every take set (not significant individually). Only the stock openWakeWord model gained: RNNoise cut its false accepts from 3.6/h to 0.6/h (p < 0.001), recall not measurable. **Pre-wake filtering: no help for the custom models, a likely recall cost; do not add it.**

## 1. Where HA processes satellite audio (HA 2026.10.4)

### 1.1 Chain order

Source: `homeassistant/components/esphome/assist_satellite.py`, `assist_satellite/entity.py`, `assist_pipeline/default_pipeline.py`, `assist_pipeline/audio_enhancer.py`, `assist_pipeline/models.py`, `assist_pipeline/vad.py` (installed 2026.10.4, verified-on-target).

```text
satellite (HPF -> AEC3 + NS -> +24 dB), ESPHome VoiceAssistantAudio
  -> esphome handle_audio: channel `data` (or `data2`, see 1.3) -> per-run queue
  -> assist_satellite.async_accept_pipeline_from_satellite
       audio_settings = AudioSettings(silence_seconds = VAD sensitivity)   # nothing else
  -> _DefaultPipelineProcessor.process_enhance_audio          (needs_processor: VAD is on)
       1. volume_multiplier (1.0: skipped)
       2. split into 10 ms chunks (160 samples)
       3. MicroVadSpeexEnhancer.enhance_chunk
            a. microVAD speech probability   (on the chunk as received)
            b. speex NS + AGC                (not created: both levels are 0)
  -> WAKE_WORD stage: debug recording 00_wake-*  ->  wyoming wake entity  ->  wyoming-openwakeword
  -> on detection, the SAME processed stream continues into
     STT stage: debug recording 01_stt-*
       -> VoiceCommandSegmenter on microVAD probabilities (end of command)
       -> wyoming STT entity -> faster-whisper -> text
```

- **Enhancement runs once, before the wake word and before STT.** `async_execute` builds `stt_processed_stream` once; the wake word consumes it, and STT continues on the same generator after detection. There is no per-stage choice. (documented, verified-on-target)
- **microVAD scores the audio before speex.** In `enhance_chunk`, `self.vad.Process10ms(audio)` runs first, then `self.audio_processor.Process10ms(audio)`. (documented, verified-on-target)
- **HA's debug recordings (`assist_pipeline: debug_recording_dir`) store the enhanced audio** (`capture_audio(chunk.audio)` after enhancement). With today's settings this equals the satellite's stream, so such recordings are exactly what the wake word and STT receive. (documented, verified-on-target)
- **No pre-roll is forwarded to STT for satellites.** `WakeWordSettings()` defaults to `audio_seconds_to_buffer = 0` and `timeout = None`; ESPHome runs do not set it. STT only gets the wake service's `queued_audio` (wyoming-openwakeword sends none) and the audio after detection. (documented, verified-on-target)
- **End of command:** `VoiceCommandSegmenter` (speech 0.3 s to start, command at least 1.0 s, silence 0.25/0.7/1.25 s from the VAD sensitivity select, **15 s timeout**, thresholds 0.2 before and 0.5 during the command) runs only if the STT service declares `requires_external_vad`. `wyoming-faster-whisper` declares it unless `--vad-endpointing` is set. (documented, verified-on-target)

### 1.2 How settings are chosen per run

| Setting | Who decides in 2026.10.4 | Value for ESPHome satellites |
| --- | --- | --- |
| `noise_suppression_level` (0 to 4, speex at `-15 dB × level`) | Hard-coded default in `AudioSettings` | 0 (off) |
| `auto_gain_dbfs` (0 to 31, speex AGC at `300 × level`) | Hard-coded default | 0 (off) |
| `volume_multiplier` | Hard-coded default | 1.0 |
| `is_vad_enabled` | Hard-coded default | True (microVAD) |
| `silence_seconds` | VAD sensitivity `select` of the satellite device | 0.7 (default), 0.25 (aggressive), 1.25 (relaxed) |

- **History:** up to HA 2024.9 `esphome/voice_assistant.py` copied `noise_suppression_level`, `auto_gain` and `volume_multiplier` from the request, and `USE_VAD` from the flags, into `AudioSettings` ([2024.8.0 source](https://github.com/home-assistant/core/blob/2024.8.0/homeassistant/components/esphome/voice_assistant.py)). From 2024.10.0, when ESPHome moved to `assist_satellite`, `entity.py` builds `AudioSettings(silence_seconds=...)` only ([2024.10.0 source](https://github.com/home-assistant/core/blob/2024.10.0/homeassistant/components/assist_satellite/entity.py)). In 2026.10.4 the parameter `audio_settings` of `handle_pipeline_start` is received and not used. (documented, verified-on-target)
- **ESPHome's documentation still describes the options** as "the noise suppression level to apply to the assist pipeline" ([voice_assistant](https://esphome.io/components/voice_assistant/)); it does not say that HA ignores them. (documented)
- **Our satellite** sends `audio_settings { noise_suppression_level: 0, auto_gain: 0, volume_multiplier: 1.0 }` and `wake_word_phrase: ""` in every start request (`crates/esphome-api/src/server.rs`, `send_to_ha`). Sending other values would change nothing on 2026.10.4. (documented, repository source)
- **No pipeline or YAML option** sets NS or AGC for satellite runs. The `assist_pipeline/run` WebSocket command is a different entry point (browser and API clients) and is not used by ESPHome satellites. (documented)

### 1.3 The second audio channel (`MULTI_CHANNEL_AUDIO`)

- If a device sets `VoiceAssistantFeature.MULTI_CHANNEL_AUDIO` (64), HA accepts `data2` in `VoiceAssistantAudio` as a non-enhanced channel. It starts every run on channel 0 (`data`), and switches to channel 1 at `STT_START` **only if** the STT service reports both `prefers_auto_gain_enabled` and `prefers_noise_reduction_enabled` as false. The wake word therefore always receives channel 0. (documented, verified-on-target)
- `wyoming-faster-whisper` 3.8.1 leaves both flags at the Wyoming default `True`, so HA would keep channel 0 for STT too. (documented, verified-on-target)
- Consequence: a satellite could send its front-end output on channel 0 (unchanged for the wake word) and a different variant on channel 1 for STT, but only with an STT service that sets both preferences to false. Device-side option; listed for completeness in [section 2.6](#26-other-credible-options).

## 2. Options at the HA level

Costs are per audio stream at 16 kHz mono. The wake word stream is **continuous** (one per TV, all day); the STT stream only exists between a wake and the end of the command.

### 2.1 HA's built-in enhancer (speex NS, AGC, volume)

- **Mechanism:** `pyspeex-noise` 1.0.2 (`AudioProcessor(auto_gain, noise_suppression)`), the speexdsp preprocessor: a stationary-noise spectral suppressor with AGC. (documented, verified-on-target)
- **Position:** before the wake word **and** STT, not separable (section 1.1).
- **Availability:** not reachable for ESPHome satellites on 2026.10.4. Enabling it would need a modified HA core or a custom integration that overrides `assist_satellite`, which is fragile and out of scope for a public project.
- **Quality evidence:** in this repository's offline AEC work the speexdsp preprocessor attenuated quiet near-end speech by 8 to 13 dB at low input levels ([aec-candidates.md](aec-candidates.md)); untested after the +24 dB gain. It suppresses stationary noise only; TV dialogue and other people's speech are not "noise" to it. (verified-on-host per that note; hypothesis for this chain)
- **Latency and CPU:** 10 ms frames, no lookahead; small (hypothesis: well under one percent of a core).
- **Risk:** double noise suppression in front of the wake word (section 3). **Not recommended.**

### 2.2 A Wyoming proxy that denoises

A small Wyoming server that HA uses as the wake word service, the STT service, or both. It accepts `AudioStart`/`AudioChunk`/`AudioStop`, processes the PCM, and forwards it to the real service, relaying `Detection`/`NotDetected` or `Transcript` back. Each HA run opens its own connection.

| Engine | Rate and frame | Notes | Label |
| --- | --- | --- | --- |
| `webrtc-noise-gain` (WebRTC NS + AGC, the same family as our on-device NS) | 16 kHz, 10 ms | Used by wyoming-satellite for `--mic-noise-suppression 0..4` and `--mic-auto-gain 0..31`; the same API as `pyspeex-noise` | documented ([repo](https://github.com/rhasspy/webrtc-noise-gain), [wyoming-satellite](https://github.com/rhasspy/wyoming-satellite)) |
| speexdsp preprocessor (`pyspeex-noise`) | 16 kHz, 10 ms | What HA itself uses | documented |
| RNNoise (`nnnoiseless`, `pyrnnoise`) | 48 kHz, 10 ms | Small RNN, non-stationary noise; needs 16 to 48 to 16 kHz resampling (adds a few ms) | documented (rate); CPU hypothesis |
| DeepFilterNet (`deepfilternet`, MIT or Apache-2.0) | 48 kHz only | Deep filtering, best quality of the four in the literature; STFT plus model lookahead adds tens of ms; GPU optional | documented ([repo](https://github.com/Rikorose/DeepFilterNet)); latency and CPU hypothesis |

- **Existing projects:** none found that do this as a server-side Wyoming service. [wyoming-voice-match](https://github.com/jxlarrea/wyoming-voice-match) is a proxy in front of STT that keeps only an enrolled speaker's speech (ECAPA-TDNN; about 15 ms verification and 20 to 30 ms extraction on a GPU, author-reported; development reported idle for about six months). wyoming-satellite is satellite-side and its README reports it replaced by Linux Voice Assistant. (documented, [HA community thread](https://community.home-assistant.io/t/i-built-a-speaker-verification-proxy-that-filters-out-tv-noise-from-voice-commands/986233))
- **In front of STT only:** the wake word input is unchanged, so **no wake word re-evaluation**. It cannot change HA's end-of-command decision, which microVAD makes on the undenoised stream, unless STT endpointing is moved to the server (`--vad-endpointing`, section 2.3). Latency: the STT stage is not real-time critical; any of the four engines fits (hypothesis). Effort: a small Python service and one more deployment. Alternative with no extra service: a patch in the self-built STT image that denoises the buffered command before transcription.
- **In front of the wake word:** changes the model's input, so it needs a re-evaluation and probably a retrain with the new stage added to the training emulation. Runs continuously for every TV. Highest risk to wake word recall.
- **Quality caveat:** all four engines target non-speech noise. TV speech that AEC leaves behind and other people talking are speech; only AEC (on the device) or speaker selection (wyoming-voice-match) removes them. (hypothesis grounded in what each algorithm models)

### 2.3 STT-side settings that stop noise turning into words

Reference deployment (verified-on-target): `wyoming-faster-whisper` 3.8.1, `large-v3-turbo`, CUDA `int8`, fixed language, a static `--initial-prompt` plus HA entity-name biasing (`--hass-api`), beam size auto (5 on non-ARM), no VAD options.

| Option | Mechanism | Effect on noise to words | Cost and risk | Label |
| --- | --- | --- | --- | --- |
| `--vad-filter` (+ `--vad-threshold`, `--vad-min-speech-ms`, `--vad-min-silence-ms`) | faster-whisper runs Silero VAD on the command and drops non-speech before decoding; no speech gives an empty text, HA then ends with `stt-no-text-recognized` | Removes hallucinations on non-speech audio; does not help with TV or background speech | Silero on a few seconds of audio: milliseconds; risk of cutting soft or short commands at high thresholds | documented (CLI help: "can reduce hallucinations") |
| `--vad-clip` (+ threshold, pad) | Trims leading and trailing non-speech before transcription | Less noise around the command; does not remove noise inside it | Small | documented |
| `--vad-endpointing S` | The server ends the command after S s of Silero silence; HA then stops using microVAD (`requires_external_vad = false`) | Could end commands sooner under steady noise if Silero separates noise better than microVAD | **No maximum duration in the server's segmenter** (HA's own has a 15 s timeout): with continuous TV speech a command could run until the satellite or HA stops it | documented (source); behaviour hypothesis |
| Initial prompt and name biasing | Text context for the first window | A long prompt of command words and entity names makes Whisper more likely to emit those words on unclear audio | Shorter prompt may cost accuracy on names | hypothesis (known Whisper prompt behaviour, not measured here) |
| `--beam-size` | Decoding search width | Not a noise control; 1 is faster, 5 slightly more accurate | — | documented |
| `no_speech_threshold`, `log_prob_threshold`, `hallucination_silence_threshold` | faster-whisper drops a segment when no-speech probability > 0.6 **and** average log-probability < -1.0 | Tuning them could reject more noise-only segments | **Not exposed** by the Wyoming server: a patch to the self-built image | documented (faster-whisper 1.2.1 defaults) |
| HA VAD sensitivity select | End-of-command silence 0.25 / 0.7 / 1.25 s | Aggressive ends earlier and captures less trailing noise; under TV speech microVAD may never see silence and the 15 s timeout ends the command | Free; per satellite in HA's UI | documented |
| Model and language | Fixed language avoids language-ID errors; larger models hallucinate differently, not necessarily less | — | Already fixed | — |

### 2.4 Wake-word-side options (no audio change)

- **Threshold** (`--threshold`, default 0.5): global for every model served by the instance. A per-model operating point is set by calibrating the model file so that its intended raw threshold maps to 0.5 (the custom wake word already uses this). Raising it trades recall for fewer false accepts; measurable offline. (documented)
- **Trigger level** (`--trigger-level`, default 1): the handler decrements `triggers_left` for every 80 ms frame above the threshold and **never re-arms it until the next `AudioStart`**. An HA satellite run is one stream that can last hours, so `--trigger-level N` means "N frames above the threshold since the run started", not N in a row; after a few stray frames it behaves like 1. It only helps against single-frame spikes at the beginning of a run. A consecutive-frames rule would need a patch or a proxy. (documented, `wyoming_openwakeword/handler.py` 2.1.0)
- **Refractory** (`--refractory-seconds`, default 2.0): suppresses repeated detections within a stream; HA starts a new run after a detection anyway, so it does not reduce first false wakes. (documented)
- **No VAD gate or speex NS:** `wyoming-openwakeword` 2.x uses `pyopen-wakeword`, which has neither. The older `openwakeword` Python package has `enable_speex_noise_suppression` and `vad_threshold` (Silero; zeroes a prediction when the maximum VAD score 0.40 to 0.56 s earlier is below the threshold). (documented)
- **A VAD gate in a wake word proxy:** forward the audio unchanged, and drop a `Detection` unless a speech detector (Silero, or the satellite-side microVAD equivalent) saw speech in the wake window. The model's input is unchanged, so no retrain, but recall must be re-measured (a gate can reject quiet or distant speech). It only rejects false wakes on non-speech noise. (hypothesis)
- **A second-stage verifier in a wake word proxy:** on a detection, score the last 1 to 2 s again with a stronger detector (a second wake word model trained on false accepts, or the GPU STT checking for the phrase) before relaying it. Latency: one extra inference on about 1.5 s of audio. (hypothesis; no existing Wyoming project found)

### 2.5 VAD inside HA

- HA's microVAD (`pymicro-vad` 1.0.1) is fixed: no threshold option, only the silence duration via the select. Its probabilities decide the start and end of the command. It is not used during the wake word stage for satellites (no wake word timeout). (documented, verified-on-target)

### 2.6 Other credible options

- **Speaker verification before STT** ([wyoming-voice-match](https://github.com/jxlarrea/wyoming-voice-match)): the only option found that targets TV speech and other talkers in STT. Needs voice enrollment (privacy and consent), a GPU or CPU image, and its maintenance status is unclear. (documented, author-reported results)
- **A closed-vocabulary STT** (HA's speech-to-phrase) instead of Whisper: noise cannot become arbitrary words, but it can become the nearest valid command. Not evaluated. (hypothesis)
- **Two channels from the satellite** (section 1.3): a lighter-processed channel 1 for STT, only with an STT service that declares both preferences false. Device-side, no HA change. (documented mechanism)
- **Reporting HA's ignored `audio_settings` upstream** (HA or ESPHome documentation), so the documented ESPHome options match behaviour. (hypothesis about upstream interest)

## 3. Interaction with the device-side processing

- **The device already runs WebRTC NS (moderate, about 12 dB per `sonora` 0.2.0's documentation) after AEC3, and +24 dB fixed gain.** A second stationary-noise suppressor (speex or WebRTC NS) in HA would estimate its noise floor from an already suppressed signal: expect little extra gain on steady noise, more musical noise and more attenuation of soft speech (hypothesis, consistent with the speexdsp observation in section 2.1).
- **Fixed gain before AGC:** HA's AGC after +24 dB could pump levels during silence and raise the residual noise between words (hypothesis).
- **What must stay on the device:** AEC (only the device has the TV reference signal) and the high-pass filter. Nothing on the server can cancel the TV's own sound as well.
- **What can move or be added server-side:** non-stationary denoising (RNNoise, DeepFilterNet) and speaker selection, **in front of STT only**. These do not overlap with AEC and only partly with stationary NS.
- **On-device NS is part of the wake word's training input.** Turning it off on the device, or replacing it with server-side NS in front of the wake word, is the same kind of change as adding a stage: re-evaluate the model.

## 4. Which steps need a wake word re-evaluation

| Step | Changes the wake word's input? | Re-evaluation | Retrain likely? |
| --- | --- | --- | --- |
| STT options (`--vad-filter`, clip, endpointing, prompt), VAD sensitivity | No | No | No |
| STT-only denoise proxy or STT image patch | No | No | No |
| Threshold or calibration change | No (decision only) | Recall and false accepts per hour on the existing sets | No |
| VAD gate or second-stage verifier in a wake proxy | No (decision only) | Yes, recall especially | No |
| Any NS, AGC or gain in front of the wake word (HA enhancer, wake proxy) | **Yes** | Yes | **Probably**, with the new stage added to the training emulation |
| Changing the device front-end (NS off, other gain) | **Yes** | Yes | **Probably** |

## 5. Measurement resources

- **HA debug recordings are the exact wake word and STT input** (section 1.1). They can be replayed offline through any candidate processing and scored with the server's own wake word runtime (`pyopen-wakeword` with the deployed models).
- **Existing private data (not in this repository):** about 30 h of real room-audio negatives from the deployed TVs' wake streams (post front-end), and labelled real sessions with the custom wake word. Scoring tools already report recall and false accepts per hour with intervals, from aggregates only. Rule: processed by script only; no audio is listened to, transcribed by people, printed or committed.
- **Gaps:** (a) no labelled set of real **commands** to measure STT accuracy, so a change that removes noise words could also remove real words unnoticed; (b) the false wakes are not yet classified by noise type (TV speech, music, non-speech household noise), which decides whether a non-speech denoiser can help at all; (c) the rate of noise-only STT runs that produce text is not measured.

## 6. Recommendation: a staged plan

Order: changes that do not touch the wake word input first, cheapest first. Each stage is an experiment with a written procedure, criteria and rollback before it runs; config changes go through the deployment's GitOps.

| Stage | Change | Measure | Wake word re-eval |
| --- | --- | --- | --- |
| **0. Baseline** | None. Classify false wakes on the existing negatives by context (TV playing or not, speech-like or not) with aggregate audio features; count, offline, how many false-wake windows the current STT turns into non-empty text (counts and word counts only, no text output) | False accepts per hour per TV; share of false wakes in non-speech noise; noise-to-text rate | — |
| **1. STT decision settings** (`--vad-filter` applied in the reference deployment on 2026-11-07; effect not yet measured) | `--vad-filter` with default Silero settings, then a threshold sweep; try a shorter initial prompt; VAD sensitivity aggressive vs default | Noise-to-text rate on the false-wake windows; command accuracy on a small consented command set (gap a); STT latency | No |
| **2. Wake word decision settings** | Threshold or calibration sweep on the existing sets; a consecutive-frames trigger rule and a VAD gate simulated in the offline scorer | Recall on the real sessions and false accepts per hour with intervals; pick an operating point | Yes, but offline on existing data and no retrain |
| **2b. Offline replay through candidate filters** (**done 2026-11-07**, [section 10](#10-results-of-the-offline-replay-2026-11-07)) | The existing real takes and negatives through 2 to 3 filters, scored with the current models ([section 9](#9-offline-replay-experiment-no-retrain)); can run in parallel with stage 1 | Recall and false accepts per hour, filtered vs unfiltered, per model kind | Offline only; decides whether stage 5 is worth a retrain |
| **3. STT-only denoising** | A Wyoming STT proxy (or an STT image patch) with RNNoise or DeepFilterNet, then WebRTC NS for comparison | As stage 1, plus CPU or GPU cost per command | No |
| **4. Wake proxy with a gate or verifier** | Only if stage 2's offline results justify it | Live false wakes per day and recall checks | Yes (decision only) |
| **5. Denoising in front of the wake word** (**not recommended after stage 2b**) | Only if stage 0 shows non-speech noise drives the false wakes, stage 2b shows a gain, and stages 2 to 4 are not enough. Stage 2b showed no gain for the deployed model | Offline replay of negatives and sessions through the candidate; then retrain with the stage in the emulation | **Yes, retrain likely** |

**Top three by expected impact for effort:**

1. **STT `--vad-filter`** (stage 1): one argument, no wake word impact; expected to stop most non-speech noise becoming words. It will not stop TV speech being transcribed.
2. **Wake word operating point and trigger rule** (stage 2): attacks false wakes directly, measurable on data that already exists, no input change.
3. **STT-only DNN denoising** (stage 3): moderate effort, no wake word impact; helps where the command itself is noisy. Speaker verification is the alternative if stage 0 shows TV or background speech dominates.

**Not recommended now:** HA's speex enhancer (not reachable, before the wake word, stationary-only, double NS); any NS or AGC before the wake word without a retrain plan.

## 7. Open questions and owner decisions

- Whether an offline script may run STT on the stored negatives and report only counts (stage 0 and 1).
- Whether to record a small consented command set for STT accuracy (gap a).
- STT and wake word configuration changes in the HA deployment (stages 1 and 2), and the recall versus false-accept operating point.
- Whether to add a new service (STT proxy, wake proxy, speaker verification) or patch the self-built STT image.
- Whether to report the ignored ESPHome `audio_settings` upstream.
- Unverified: microVAD's behaviour under TV speech (does the 15 s timeout end most noisy commands?); `--vad-endpointing` with continuous speech; CPU cost of RNNoise or DeepFilterNet on the cluster.

## 8. "Normalise, then detect": one central filter and one normal model

Owner question (2026-11-07): "Isn't it better to filter all audio before the wake word and use a normal model there?" The architecture: one HA-side front-end applied to every device's stream before the wake word, and a single model trained with the stock recipe (synthetic, unfiltered audio with noise augmentation).

### 8.1 What a central filter can and cannot normalise

| Variable across devices and rooms | Can a server-side filter remove it? | Why |
| --- | --- | --- |
| Echo of the device's own playback | **No** | AEC needs the playback reference, sample-aligned with the mic, which only the device has. Must stay on the device. (documented: the AEC design of this repository) |
| Stationary noise (fans, hum, hiss) | Yes | Speex, WebRTC NS, RNNoise and DeepFilterNet all target it. (documented) |
| Non-stationary non-speech noise (dishes, steps, some music) | Partly | DNN suppressors (RNNoise, DeepFilterNet) target it; speex and WebRTC NS mostly do not. (documented by design; quality on music: hypothesis) |
| TV dialogue that AEC left, other people talking | **No** | Every speech-preserving denoiser keeps speech by design. Only AEC (device) or speaker selection removes it. |
| Microphone frequency response | **No**, not with one filter | Equalising needs a measured response per device model; a denoiser does not equalise. (hypothesis) |
| Distance and reverberation | Little | Level is recoverable with AGC; reverberation and the direct-to-reverberant ratio are not removed by these denoisers. (hypothesis) |
| Processing the device already applied (AGC, NS, codecs, our +24 dB) | **No** | Irreversible. A second filter's output depends on its input, so devices with different front-ends still give different outputs, now with the second filter's artifacts on top. (hypothesis, consistent with section 3) |

So a central filter normalises one dimension (the non-speech noise floor) and adds another (artifacts that depend on the input's SNR and earlier processing). It does not make devices interchangeable.

### 8.2 A "normal" model on denoised input

- **openWakeWord** documents optional Speex NS that "can improve performance when relatively constant background noise is present" and "can reduce both false-reject rates and false-accept rates, though testing in a realistic deployment environment is strongly recommended"; its stock models are trained on 100 % synthetic speech with noise augmentation. It also documents a Silero VAD gate that "can significantly reduce false-positive activations in the presence of non-speech noise". No openWakeWord issue reporting gains or losses from NS was found. (documented, [openWakeWord README](https://github.com/dscripka/openWakeWord))
- **Brueggeman et al.** (Google Speech Commands with WHAM! noise): enhancement before keyword spotting "can improve KWS accuracy on noisy speech when the backend model is trained on clean speech", but with a backend trained on noisy speech "it is difficult to improve the KWS accuracy with SE". Speech interference was not tested. (documented, [arXiv 2309.16060](https://arxiv.org/abs/2309.16060))
- **Iwamoto et al.:** the artifacts that enhancement introduces, more than the residual noise, degrade downstream ASR. **Bonet et al.:** enhancement and wake word detection work best when trained jointly. (documented, [arXiv 2201.06685](https://arxiv.org/abs/2201.06685), [arXiv 2101.12732](https://arxiv.org/abs/2101.12732))
- **Raju et al. (Amazon):** after AEC, training with TV, movie and music interference at 0 to 40 dB reduced errors on playback by 40 to 47 % (AUC). The remedy for speech-like interference was training data, not a filter. (documented, [arXiv 1808.00563](https://arxiv.org/abs/1808.00563))
- **Counter-evidence that stock models tolerate a fixed front-end:** HA's Voice Preview Edition runs microWakeWord after XMOS echo cancellation and stationary noise removal, while HA's microWakeWord training recipe uses plain noise augmentation (observed-on-other-device, [HA blog](https://www.home-assistant.io/blog/2024/12/19/voice-preview-edition-the-era-of-open-voice/)). On this satellite, the stock openWakeWord models also run on post-NS audio.
- **Conclusion:** openWakeWord-recipe models are "noisy-trained", so the best evidence predicts **little gain** from a denoiser in front of them, and a **risk of loss** from artifacts they never saw; a model trained with the filter in the loop is the expected way to get a gain. Neither is measured for this satellite yet. (hypothesis grounded in the cited work)

### 8.3 Does denoising reduce the false wakes we actually see?

- A wake word false accept needs audio that resembles the phrase. Speech-like audio (TV dialogue, people talking) is the most likely source, and every denoiser keeps it. Denoising can only remove false accepts caused by non-speech noise. (hypothesis)
- What false accepts are made of on these TVs is not yet classified (section 5, gap b). The offline replay in section 9 measures the effect directly, and stage 0 classifies the cause.
- **STT turning noise into words is a different problem:** Whisper hallucinates on non-speech audio (the case for `--vad-filter`), so non-speech denoising helps STT more than the wake word, and it can be applied in front of STT only (section 2.2). TV dialogue is still transcribed either way.

### 8.4 CPU cost of an always-on stream per device at HA

| Engine | Cost per 16 kHz stream | Label |
| --- | --- | --- |
| WebRTC NS (the NS stage alone) | about 0.07 % of one core (RTF difference `sonora` AEC + NS minus AEC: 0.0053 − 0.0046) | verified-on-host ([aec-candidates.md](aec-candidates.md#cpu-memory-size), AMD Ryzen 9 5950X, one thread) |
| speexdsp preprocessor (NS + AGC) | about 0.15 % (0.0064 − 0.0049) | verified-on-host, same table |
| RNNoise (`nnnoiseless`, including 16 to 48 to 16 kHz resampling) | about 0.8 % (RTF 0.0081) | verified-on-host, same table |
| DeepFilterNet2 at 48 kHz | about 4 % of a notebook Core i5 core (RTF 0.04); DeepFilterNet3 assumed similar | documented ([arXiv 2205.05474](https://arxiv.org/abs/2205.05474)); DFN3: hypothesis |

- These are DSP costs only. In a Python Wyoming proxy, per-chunk overhead (event parsing, resampling in Python) may dominate for the light engines (hypothesis). Even so, two to ten always-on streams are small for any of them except DeepFilterNet, which is still affordable on a server but not negligible.
- HA already runs microVAD on every stream; the wake word server runs the openWakeWord feature extractor on every stream.

### 8.5 Answer

Not better by default. A central filter belongs **in front of STT**, where it cannot hurt the wake word and where non-speech noise causes hallucinated words. In front of the wake word, a filter is worth it only if the offline replay (section 9) shows a gain on the real negatives and takes; and then the model should be trained with that filter in the loop rather than relying on a "normal" model. AEC stays on the device in every case.

## 9. Offline replay experiment (no retrain)

_Run on 2026-11-07 with the owner's approval; results in [section 10](#10-results-of-the-offline-replay-2026-11-07). As run: files were filtered in 10 min pieces, each preceded by 10 s of its own audio that was filtered and dropped (some negatives are 6 h long); F4 was not run._

**Question:** does an HA-side filter in front of the wake word reduce false accepts per hour without losing recall, for (a) a stock-recipe model (synthetic, unfiltered training audio), (b) a model trained with this satellite's front-end emulation in the loop, (c) the deployed custom wake word, and (d) a stock openWakeWord model?

**Data:** HA debug recordings are the wake word's exact input (section 1.1), so replaying them through a filter is exactly what the wake word would see behind that filter. Use the private real takes (two sessions at 1.5 m and 3 m with the TV on and off, plus a button-take session on the second TV) and about 30 h of room-audio negatives from both TVs. Everything runs on the PC by script; no audio is listened to, printed or committed; outputs are aggregates only.

**Filters (each applied to the post-front-end S16 stream, delay-compensated so sample timing and alignment are unchanged, output length equal to input length):**

| ID | Filter | Implementation |
| --- | --- | --- |
| F0 | None (baseline, rescored with the same tools for a like-for-like comparison) | — |
| F1 | WebRTC NS, a second pass (what a `webrtc-noise-gain` proxy would add) | this repository's `aec_offline` example with a silent reference, `--ns --gain-db 0 --hpf-hz 0`, latency measured with a burst and removed |
| F2 | RNNoise | `aec_offline --no-aec --rnnoise --gain-db 0 --hpf-hz 0` (latency already removed by the tool; checked with the same burst) |
| F3 | HA's own speex enhancer at level 2 (`pyspeex-noise` 1.0.2, `AudioProcessor(0, -30)`, 10 ms frames), exactly what `noise_suppression_level: 2` would do | a small Python loop; needs `pyspeex-noise` 1.0.2 installed in a separate local virtual environment |
| (F4, optional) | DeepFilterNet3 (16 to 48 to 16 kHz) | only if F2 shows a gain |

**Procedure:** mirror the dataset tree per filter (symbolic links for manifests and alignment files, filtered WAVs for the raw wake files and the negatives), check every output length, report per-file RMS change as an aggregate to confirm each filter is active, then score every mirror with the existing scorer (the server's own `pyopen-wakeword` runtime, threshold sweep plus each model's calibrated point, 2 s refractory, Poisson intervals for false accepts per hour, Wilson intervals for recall). Run sequentially under the PC's resource caps. Exact commands are kept with the private wake word data.

**Readout:** per model and filter, recall at the model's calibrated point and at matched false accepts per hour, per condition (TV on or off, 1.5 m or 3 m), against F0.

- **A filter helps** only if false accepts per hour drop with disjoint 95 % intervals, or recall at matched false accepts rises by at least 15 points, for the same model. With about 60 takes and 30 h of negatives, smaller effects are not detectable; say so instead of claiming them.
- **Interpretation:** a gain for the stock-recipe model only supports "normalise, then detect"; a gain for every model supports adding the filter and retraining with it; a loss for the front-end-trained models measures the artifact cost (section 8.2); no change on false accepts supports the view that they are speech-driven (section 8.3).
- **Limits:** the takes contain one phrase, so the deployed wake word's recall is not measurable on them (only its false accepts and its response to the shorter phrase); two speakers; one room per TV.
- **Rollback:** nothing is deployed; delete the mirror directories.

## 10. Results of the offline replay (2026-11-07)

Run on the PC only, by script, under a 6 GB, 4-CPU cap, one job at a time. No audio was listened to, transcribed or printed; only counts and rates were read. The filtered audio is kept privately next to the original data (never committed).

**Set-up as run:** filters F1 (second WebRTC NS pass, 224 samples of latency removed), F2 (RNNoise, latency removed by the tool) and F3 (HA's speex enhancer at level 2, 160 samples removed) applied to 34.2 h of satellite wake-stage audio (every raw wake file of the take sessions and all room negatives). Every output has the input's length; the residual delay was checked on the loudest 10 s of each 10 min piece: speex and WebRTC NS 233 of 233 pieces aligned (WebRTC NS within 14 samples, under 1 ms, against an 80 ms scoring step), RNNoise 157 of 159 checkable pieces aligned (74 pieces were silenced by more than 30 dB and could not be checked). Scores: the server's own runtime (`pyopen-wakeword`), each model at its calibrated operating point (the raw threshold its released file maps to 0.5), 2 s refractory. Statistics: Poisson 95 % intervals for false accepts per hour, an exact conditional binomial test for comparing two rates on the same hours.

**Models (anonymised):** M1, a custom single-word model with the stock openWakeWord recipe (synthetic, unfiltered training audio); M5, the same word trained with this satellite's front-end emulation in the loop; D, the deployed custom wake word (a two-word phrase that contains M1's word, trained with the front-end emulation); S, a stock openWakeWord model. Takes: two sessions of speaker 1 on TV 1 (1.5 m and 3 m, TV on and off; session 2 is held out from all training, session 1 is in M5's training data) and one button-take session of speaker 2 on TV 2 (in M5's training data). The takes are of M1's single word, so D's recall cannot be measured; its responses to the single word ("word alone") and to near-words are false accepts. Session 1 and speaker 2's takes were among D's training negatives; the held-out session was not.

**False accepts per hour on the held-out room-audio test split (8.31 h, both TVs; k, rate, 95 % interval):**

| Model | Unfiltered | F3 speex level 2 | F2 RNNoise | F1 WebRTC NS again |
| --- | --- | --- | --- | --- |
| M1 (stock recipe) | 1 (0.12, 0.00 to 0.67) | 0 (0.00, 0.00 to 0.44) | 1 (0.12, 0.00 to 0.67) | 2 (0.24, 0.03 to 0.87) |
| M5 (front-end in the loop) | 3 (0.36, 0.07 to 1.05) | 2 (0.24, 0.03 to 0.87) | 1 (0.12, 0.00 to 0.67) | 5 (0.60, 0.20 to 1.40) |
| D (deployed) | 8 (0.96, 0.42 to 1.90) | 12 (1.44, 0.75 to 2.52) | 11 (1.32, 0.66 to 2.37) | 8 (0.96, 0.42 to 1.90) |
| S (stock openWakeWord) | 30 (3.61, 2.44 to 5.15) | 17 (2.04, 1.19 to 3.27) | **5 (0.60, 0.20 to 1.40)** | 29 (3.49, 2.34 to 5.01) |

**Recall of the single word at the calibrated operating point (hits / takes):**

| Model, take set | Unfiltered | F3 speex | F2 RNNoise | F1 WebRTC NS |
| --- | --- | --- | --- | --- |
| M1, speaker 1, held-out session | 10/40 | 7/40 | 7/40 | 14/40 |
| M1, speaker 1, session 1 | 7/40 | 5/40 | 6/40 | 6/40 |
| M1, speaker 2 | 5/21 | 5/21 | 3/21 | 7/21 |
| M5, speaker 1, held-out session | 30/40 | 26/40 | 25/40 | 28/40 |
| M5, speaker 1, session 1 (trained on) | 28/40 | 28/40 | 22/40 | 27/40 |
| M5, speaker 2 (trained on) | 12/21 | 8/21 | 10/21 | 12/21 |
| **M5, all 101 takes** | **70** | **62** | **57** | **67** |
| D, word alone (all 101 takes) | 0 | 0 | 0 | 0 |
| D, near-words (24 cues) | 0 | 0 | 1 | 0 |

**CPU per hour of audio** (the filter call alone on 2.7 h of real audio, AMD Ryzen 9 5950X, one thread; verified-on-host): speex 2.8 s (0.08 % of a core, Python frame loop); WebRTC NS 19.4 s (0.54 %, including a pass-through echo canceller and process and file overhead); RNNoise 85 s (2.4 %, including 3x resampling and process and file overhead, in a size-optimised build). These are upper bounds for the DSP itself, which section 8.4 measured lower.

**Reading:**

- **The deployed custom wake word gains nothing.** Its false accepts do not fall with any filter (8, 12, 11, 8 in 8.31 h; speex against unfiltered p = 0.50). Its responses to the word alone stay at zero, and to near-words at zero or one. This fits section 8.3: what triggers it in this room survives denoising, so it is most likely speech-like (hypothesis; the triggers were not classified).
- **The custom models lose recall with speex and RNNoise.** M5, trained with the device front-end in the loop, drops from 70 to 62 (speex) and 57 (RNNoise) of 101 takes, in the same direction in all three take sets. Each change alone is not significant with these counts, and the takes are paired, which aggregate counts cannot test. A second WebRTC NS pass is neutral (67 of 101 for M5; M1 within noise). This matches the literature in section 8.2: added processing artifacts the model never saw cost recall.
- **The stock-recipe custom model (M1)** has too little recall at its operating point (17 to 35 %) to show an effect either way; its false accepts were already near zero.
- **The stock openWakeWord model gains from RNNoise:** false accepts fall from 30 to 5 in 8.31 h (p = 2 × 10⁻⁵); speex lowers them to 17 (p = 0.08). Its recall cannot be measured on these takes. So "normalise, then detect" has some support for a stock model's false accepts on this audio, at an unknown recall cost, and none for the models trained for this front-end.
- **Limits:** one room per TV, two speakers, takes of one word, models at fixed operating points (a filter could move the best threshold), and the deployed model's own recall untested.

**Implications for stages 3 to 5:**

- **Stage 3 (denoising in front of STT only)** is unaffected by these results and stays recommended for evaluation. RNNoise removed most of the energy of the quiet, noise-only audio (median change -1.5 dB, 5th percentile -23 dB per file), which is the kind of input that STT turns into words; measure it against the STT VAD filter already applied in stage 1.
- **Stage 4 (wake word proxy with a VAD gate or a verifier)** stays a candidate: it leaves the model's input unchanged. The stock model's large gain from RNNoise suggests that part of its false accepts come from non-speech audio, which a gate could reject without touching recall on speech (hypothesis; simulate in the offline scorer first).
- **Stage 5 (denoising in front of the wake word): not recommended.** No filter lowered the deployed model's false accepts, and two of the three cost recall in the models trained for this front-end. Revisit only if the wake word is replaced by a stock model, and then measure its recall first.
