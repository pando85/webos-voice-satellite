# P2 corpus and metrics

**Status:** IN USE (since 2026-11-02, [P2 session 2](../discovery/c3-a/P2-session2-wake-accuracy.md): digital and semi-synthetic runs and false wakes with content; the rig runs are still open) · **Phase:** [Phase 2, sound tests](../project/roadmap.md#phase-2-sound-tests) · **Risk:** none to build (PC only, nothing played); every playback run needs the owner's consent and the safety rules in [AGENTS.md](../../AGENTS.md)

This page defines the test material and the metrics for the Phase 2 accuracy runs, so that every run is scored the same way and results are comparable. It replaces the single-run results in [D3](../discovery/c3-a/D3-mic-identification.md#conclusions-from-step-5a) with measured rates. Nothing here is a result; no claim in this page is `verified-on-target`.

## Material (all generated or fetched by committed scripts, never committed)

The generators are described in [the audio rig README](../../tools/audio-rig/README.md#phase-2-test-corpus-tv-content-set-and-echo-stimuli). Output is under `devices/private/` (gitignored).

| Set | Script | Content | Count / duration (p2-v1) |
| --- | --- | --- | --- |
| Corpus, positives | `make_corpus.py` | "alexa" alone (9 Piper speakers, 3 length/noise variations) and "alexa" + 0.4 s + one of 15 Spanish commands (7 Spanish speakers, 10 commands each) | 97 items: 27 alone, 70 with a command; 7.3 min of audio |
| Corpus, negatives | `make_corpus.py` | 28 Spanish sentences without the wake word (24 conversation, 4 commands without "alexa") | 28 items; 1.9 min |
| Corpus, near-misses | `make_corpus.py` | 14 Spanish near-misses ("Alexis", "a la mesa", "relaja", "Alexandria", ...) | 14 items; 0.7 min |
| Session files | `make_corpus.py` | One long WAV per kind (`pos` 13.7 min, `neg` 3.8 min, `nearmiss` 1.7 min), 4 s gaps, `session_<kind>_timestamps.csv` | 3 files, each in 48 kHz stereo and 16 kHz mono |
| TV content | `make_content.py` | Spanish speech (LibriVox), English speech and film dialogue (LibriVox, Tears of Steel), music with vocals (Josh Woodward), music without vocals (Kevin MacLeod), one action scene (Tears of Steel), all CC BY or public domain, -23 LUFS | 68.4 min in 13 clips (Spanish 18.0, English 13.2, vocals 16.4, instrumental 15.9, action 5.0) plus a 10 min sampler |
| Echo stimuli | `make_p2_stimuli.py` | 10 s sweep 50 Hz-8 kHz, 30 s pink noise, 5 s 1 kHz tone at -20 dBFS, click/chirp alignment train | 4 signals (+ inverse filter), 48 kHz stereo and 16 kHz mono |

Properties that the metrics rely on:

- The voices are **synthetic** (Piper). They are consent-free and reproducible in settings, but they are not human speech: rates measured with them are indicators for comparing conditions, not a prediction for people. Piper's noise sampling is not seedable, so a regenerated corpus is equivalent but not identical. A run must therefore name the corpus by `manifest_sha256` from `corpus.json` and, for the content set, by `manifest.csv`.
- The Spanish voices pronounce "alexa" with Spanish phonetics. Report wake results per voice and per language of the voice, not only pooled.
- Items are scaled to equal active-speech RMS (-22 dBFS) under a -6 dBFS peak ceiling. `play_record.py --gain-db G` sets the file **peak** to -30 + G dBFS (the reference +12 dB gives -18 dBFS peak for the loudest item). Record the gain, the sink and the sink volume of every run.
- Timestamps in `session_<kind>_timestamps.csv` (`item_start_s`, `item_end_s`, `wake_start_s`, `wake_end_s`, `command_start_s`) are relative to the first sample of the session file and are identical in the 48 kHz and 16 kHz variants. `play_record.py` adds its own lead-in silence; add it to every timestamp (or align by cross-correlation, below).

## Time alignment

A detection has a time `t_det` on the timeline of the audio that the wake-word service received (for example the openWakeWord `Detection` timestamp, which marks the end of the word). It must be mapped to the session timeline:

1. Locate the session in the recording that was scored (the TV capture, or the stream sent to the service) by cross-correlation against the session file (as `wyoming_check.py --reference` does) and compute the offset `t_session = t_det - offset`.
2. Record the offset and its uncertainty. A run whose offset cannot be established to within 0.3 s is **invalid**, not a lower score.

## Metrics

### Wake detection rate

`wake_detection_rate = detected_positives / positives`, over the `pos` items of the session(s) scored.

- A positive is **detected** when at least one detection of the expected wake word has `wake_start_s - 0.2 <= t_session <= wake_end_s + 1.5` (seconds, from the timestamps CSV). Detections of any other model name are ignored for this metric and reported separately.
- Further detections in `[item_start_s, item_end_s]` after the first are **repeats**: counted and reported, not false wakes.
- Report the overall rate and the rate by voice, by `subkind` (alone / with command) and by condition (clean digital, TV mic silent, TV mic with content playing, level). Also report the median and the 90th percentile of `t_session - wake_end_s` (detection latency in the audio timeline; it is not end-to-end latency).
- A baseline of the same session sent digitally to the wake-word service (no speaker, no microphone) belongs in every report, so that losses caused by the acoustic path can be separated from losses caused by the model and the voices.

### False wakes per hour of content

`false_wakes_per_hour = false_wakes / hours_played`.

- A **false wake** is any wake detection (of the wake word the satellite listens for) that is not inside a positive window and not a repeat. On `neg` and `nearmiss` sessions, and on all content clips and idle/silence runs, every detection is a false wake (they contain no "alexa").
- `hours_played` is the duration of the audio actually played (lead-in and tail excluded), per category and in total. Report counts, hours and the rate per category (Spanish speech, English speech/film dialogue, music with vocals, music without vocals, action scene, sampler, near-misses, negatives), and a total.
- Because the whole content set is 68.4 min, one false wake is already about 0.9 per hour. When the count is zero, report the one-sided 95 % upper bound `3 / hours_played` (rule of three), never "0 per hour". For counts above zero give the exact Poisson 95 % interval.
- The content must be played at the level the run states (the sink volume and the TV's own volume if it is the source), because loudness at the microphone drives the result. Record whether the content came from the PC over HDMI to the TV, and whether the TV was silent or not.

### STT word error rate (WER)

Scored on the 70 `pos_cmd` items with the reference `expected_command_text`. The audio scored is the command segment (from `command_start_s` to the item end, with 0.5 s of margin before), or the audio the satellite actually streamed after the wake detection; say which.

Normalization, applied to both reference and hypothesis, in this order:

1. Unicode NFD, drop combining marks (accents and diaeresis; `ñ` becomes `n`).
2. Lowercase.
3. Replace every character that is not `a-z`, `0-9` or a space with a space (this removes punctuation and the inverted `¿ ¡`).
4. Spell out numerals 0-99 in the hypothesis in Spanish (for example `5` becomes `cinco`), because the references are spelled out; report how many numerals were converted.
5. Collapse whitespace and split into words.

`WER = (S + D + I) / N` from the word-level Levenshtein distance. **Corpus WER** is the sum of errors divided by the sum of reference words over all scored items; it is not the mean of per-item WERs. Report it overall and by voice, plus the share of items with WER 0. For the reference test reuse the known D3 baseline: the STT service gave WER 0.00 on clean and on TV-mic audio for one phrase set, so a corpus WER well above 0 is a finding to explain, not noise.

### Intent success

For each `pos_cmd` item the manifest gives an `expected_intent` class (`light_on`, `light_off`, `light_off_all`, `light_set`, `time_query`, `timer_start`, `timer_cancel`, `timer_status`, `temperature_query`, `weather_query`, `climate_set`). Before a run, write down the mapping from each class to the Home Assistant outcome that counts (intent name, targeted area or entity, slots such as the duration, and the response type); do not assume the mapping here.

- **Intent success** = the pipeline's recognized intent and slots match the mapping and the response is not a "no intent matched" or error response. For commands with an effect, check the effect on a designated test entity only if the run was set up for it; otherwise score the intent result. TTS is not part of this metric.
- Report two rates: conditional on the wake word being detected (`successes / detected pos_cmd items`) and end to end (`successes / all pos_cmd items`). Report them by `expected_intent` class when the counts allow.
- The conversation agent in use (built-in agent or other) is part of the condition and must be named, since the same transcript can succeed or fail by agent.

## Reporting rules

Every table of results states, per row: **n** (items or hours), the count **k**, the rate, and a 95 % interval (Wilson for proportions; exact Poisson for rates per hour). A bare percentage is not accepted.

Every run states the conditions, in the result file or the linked issue:

- Date; experiment ID; who was present; the TV model, firmware and power state (see the [compatibility records](../compatibility/README.md)); capture device, channel and fixed gain of the satellite; wake word model name and server version; STT model; conversation agent.
- Corpus (`manifest_sha256`) and content set (`manifest.csv` hash) versions; file variant played; `play_record.py` gain, sink, sink volume, sample rate and channels; room and distances if known.
- Number of independent runs (a run is one complete play of a session) and how many were pooled. A single run per condition is `inconclusive` for any difference smaller than the combined intervals; do not rank two conditions on overlapping intervals.
- With 97 positives the 95 % Wilson interval is about +-3 points near 97 % and about +-10 points near 50 %; plan repeats if a decision depends on a smaller difference.

Evidence labels follow [AGENTS.md](../../AGENTS.md): results from the TV microphone with the satellite pipeline are `verified-on-target` for that TV, firmware, power state and room only; results from digital or PC-microphone audio are `observed-on-other-device`. Report negative and inconclusive outcomes explicitly. Do not commit audio, recordings, raw logs with identifiers, or the generated corpus; commit the scripts, this definition, and sanitized result tables.

## Implementation and scoring notes (2026-11-02)

- Scoring code: `tools/audio-rig/p2_score.py` (normalization, WER, Wilson and exact Poisson intervals, wake windows, per-category false wakes), with unit tests. Runners: `p2_digital.py` (the digital baseline, and semi-synthetic beds), `p2_runner.py` (live, one item at a time, waits for the re-arm), `ha_assist_watch.py` with `content_play.py` (false wakes). See [the audio rig README](../../tools/audio-rig/README.md#phase-2-accuracy-and-false-wake-runs-through-home-assistant).
- **Intent mapping** (written before the first run): `INTENT_MAP` in `p2_score.py`. Intent and slots are checked on the built-in agent's recognition result (`conversation/agent/homeassistant/debug`, which executes nothing), on the slot text in `slots` or `unmatched_slots`. Report the reference text's own result next to it ("ceiling"): in a given HA, some reference sentences do not reach the mapped intent (unknown areas are parsed as names, for example).
- **Do not let a live run act.** The built-in agent executes what it recognizes, and corpus commands can match real, exposed entities. Check with the debug endpoint before a run; session 2 used a pipeline whose conversation language matched nothing for live runs.
- **WER v1.1** (reported next to v1, not replacing it): the hypothesis' "%" becomes "por ciento" before numerals are spelled, and "veintiun" equals "veintiuno". Whisper writes "21 grados" and "50%"; under v1 these cost 1 and 2 errors per item.
- **Live detection time:** `p2_runner.py` and `ha_assist_watch.py` time a detection by the PC receive time of the satellite entity's `idle` → `listening` change, about 0.3 s after the end of "alexa" in session 2's two English-voice controls over HDMI. The item start is the PC play start plus the lead-in and an assumed sink latency, so the -0.2/+1.5 s window holds for HDMI; for Bluetooth, set `--sink-latency` from a measurement.
- **HA's WebSocket pipeline** (`assist_pipeline/run`) defaults to a 3 s wake word timeout; `p2_digital.py` sets 60 s. Audio must be sent at real time: faster sending loses the audio between the detection and the start of STT.

## Open points

- The `+1.5 s` detection window and the 0.3 s alignment tolerance are initial choices; revise them (with the reason) if the first runs show a different latency of the pipeline, and keep the old definition for runs scored with it.
- Real voices (consented, short, local) are a separate, later set: they are required by the roadmap for the reference-level wake test and are not covered here.
