# Acoustic test rig (host side)

Purpose: find out which capture device/channel of the LG TV carries its built-in far-field
microphones, and later verify TV playback and latency, using known stimuli played in the same
room and detected by cross-correlation. This is a measurement aid for the research phase; a
detection here is evidence about one device, model, firmware, power state and room only.

Everything generated (stimuli, voices, recordings, analysis) lives under `artifacts/audio-rig/`
and `.venv/`, both gitignored together with `*.wav`, `*.raw` and `*.pcm`. Do not commit them.

## Setup

```sh
python3 -m venv .venv
.venv/bin/pip install numpy scipy soundfile matplotlib piper-tts
mkdir -p artifacts/audio-rig/voices
.venv/bin/python -m piper.download_voices --download-dir artifacts/audio-rig/voices es_ES-sharvard-medium en_US-lessac-medium
.venv/bin/python tools/audio-rig/make_stimuli.py      # falls back to espeak-ng if Piper is missing
```

Spanish phrases use `es_ES-sharvard-medium` (the voice used by the Home Assistant pipeline);
the wake word "alexa" uses the English `en_US-lessac-medium` voice. The manifest
(`artifacts/audio-rig/stimuli/manifest.json`) records the engine, order, offsets and durations.
Stimuli are written at -6 dBFS peak in 16 kHz and 48 kHz variants: exponential sweep
100 Hz-8 kHz (3 s) plus inverse filter, 500/1000/2000/3000 Hz pilots (0.5 s each), pink noise
burst, speech (`speech_wake`, `speech_es1..3`, `speech_full`) and a `sequence` of all of them.

## Host validation procedure

1. `.venv/bin/python tools/audio-rig/discover.py` lists sinks and sources (read-only) and names
   the speaker sink, the webcam mic and the USB mic. It exits with status 2 and a warning when
   there is no obvious speaker (for example only HDMI/DisplayPort). Do not play until a sink
   is confirmed by a human. HDMI sinks may be a TV; `play_record.py` refuses them unless
   `--allow-hdmi-sink` is given.
2. Play quietly and record both microphones plus the sink monitor (digital timing reference):

   ```sh
   .venv/bin/python tools/audio-rig/play_record.py --stimulus sweep \
       --sink <sink-name> --source webcam --source usb --gain-db 0
   ```

   The level is set only by `--gain-db`: playback peak = -30 dBFS + gain (default -30 dBFS,
   maximum +24 dB). Sink/system volume and default devices are never changed. Raise the gain in
   small steps and only with the listener's agreement.
3. Analyze each recording (the sidecar JSON supplies lead-in and timing; a sibling `__monitor.wav`
   becomes the delay reference automatically):

   ```sh
   .venv/bin/python tools/audio-rig/analyze.py artifacts/audio-rig/recordings/<session>/sweep__webcam.wav \
       --stimulus sweep --spectrogram
   ```

   Per channel: RMS/peak dBFS, lead-in noise floor, matched-filter peak-to-sidelobe ratio (MF PSR),
   normalized correlation, stimulus start in the recording (ms), delay versus the monitor,
   time-domain SNR, verdict. Sweep/noise/speech use the matched filter (`DETECTED` when the score,
   the peak-to-sidelobe ratio, is >= `--threshold-db`, default 20 dB; pure noise reaches about
   10-14 dB). Pilot tones use a per-band energy detector instead (a matched filter on periodic tones
   has a poor peak-to-sidelobe ratio): per-tone STFT band energy versus the noise in the same band
   (lead-in, or the quiet 25th percentile), start found by the in-slot versus other-slot contrast;
   `DETECTED` when >= 3 of 4 tones have SNR >= `--tone-snr-db` (10 dB) and >= 6 dB contrast.
   Sweeps add an impulse-response peak and first-arrival time.
4. `.venv/bin/python tools/audio-rig/selftest.py` checks `analyze.py` offline with synthetic
   multi-channel recordings in several rates and raw formats (no audio is played).

Recording robustness: `parecord` runs with `--latency-msec=20` (its default ~1 s fragments lose the
unflushed tail on stop), a silent pre-roll wakes a suspended sink (Bluetooth) before recording,
the lead-in silence is embedded in the played stream, the default tail is 2.5 s, and the tool warns
when a recording is shorter than expected. The `.json` sidecar has the details.

Timing note: capture start offsets differ per device. `analyze.py` reports "vs ref" as the delay
relative to the sink-monitor recording, aligning the recording ENDS (all recorders of a session
stop together); the value includes sink/Bluetooth latency plus the acoustic path and is good to
roughly +-30 ms. Treat latency as an estimate until a hardware-synchronous method is used.

## TV capture procedure (placeholder)

Not yet validated on the target. Outline: play a stimulus from the PC at the agreed gain while an
authorized, short TV capture runs (device and channel layout come from an experiment in
`docs/experiments/`); copy the capture back and analyze it. Raw dumps need explicit parameters:

```sh
.venv/bin/python tools/audio-rig/analyze.py tv.raw --raw --rate 16000 --channels 4 \
    --format S32_LE --stimulus sequence --leadin 1.0 --spectrogram
```

A channel with `DETECTED` and a high SNR carries the far-field microphone path; a channel that
is `SILENT` or `not detected` while others detect does not. Record results with model, firmware,
power state and experiment ID; negative results are results.

## Reverse direction (placeholder)

The TV plays a stimulus (for example the sweep) and the PC microphones record it with the
`webcam`/`usb` sources, then `analyze.py` reports detection and delay. This is intended to verify
TV playback and estimate latency. The recording side is not wrapped in a script yet: record with
`parecord --device=<source> --rate=<native> --channels=<n> --file-format=wav` and analyze as above.

## Offline Wyoming check (`wyoming_check.py`)

Streams a recording (WAV, or raw PCM with `--rate/--channels/--format`) to a Wyoming wake-word
service and sends the segment after the wake word to a Wyoming STT service. Requires
`.venv/bin/pip install wyoming`. It selects a channel (`--channel 0|1|mean`), trims
(`--trim-start`), optionally high-passes and peak-normalizes (`--normalize-db`, default off),
converts to 16 kHz mono S16_LE and prints wake detections (name, audio timestamp) plus transcript
and word error rate. `--reference <stimulus.wav>` locates the stimulus by cross-correlation and
takes the STT window and expected text from `manifest.json`; without it use `--stt-start/--stt-end`
and `--expected`. Use the model name from the server's Info (`alexa`, not `alexa_v0.1`: unknown
names give no detections and no error).

```sh
kubectl --context=<ctx> -n <ns> port-forward svc/<openwakeword> 11400:10400 &
kubectl --context=<ctx> -n <ns> port-forward svc/<whisper> 11300:10300 &
.venv/bin/python tools/audio-rig/wyoming_check.py tv.raw --raw --rate 16000 --channels 2 \
    --format S24_LE --channel mean --trim-start 2 --normalize-db -3 \
    --reference artifacts/audio-rig/stimuli/speech_full_16k.wav \
    --wake-host 127.0.0.1 --wake-port 11400 --stt-host 127.0.0.1 --stt-port 11300
```

Stop the port-forwards afterwards. The signal goes only to the hosts you name; nothing is saved
unless `--save-wav` is given (keep it local, never commit).

## Phase 2 test corpus, TV-content set and echo stimuli

Three generators prepare everything for the unattended Phase 2 runs (wake-word and STT accuracy,
false wakes with TV content, echo/AEC measurement). The scripts are committed; their output lives
under `devices/private/` (gitignored) and is never committed. Metrics and reporting rules:
[P2 corpus and metrics](../../docs/experiments/P2-corpus.md). Nothing here plays sound; playing
is a separate, owner-approved step.

```sh
# one-time: the venv above plus the voices (official rhasspy/piper-voices release on Hugging Face)
.venv/bin/python -m piper.download_voices --download-dir artifacts/audio-rig/voices \
    es_ES-sharvard-medium es_ES-davefx-medium es_ES-mls_9972-low es_ES-mls_10246-low \
    es_MX-ald-medium es_MX-claude-high en_US-lessac-medium en_US-ryan-medium

.venv/bin/python tools/audio-rig/make_corpus.py       # -> devices/private/corpus/p2-v1/   (about 40 s)
.venv/bin/python tools/audio-rig/make_content.py      # -> devices/private/content/p2-v1/  (about 130 MB of downloads)
.venv/bin/python tools/audio-rig/make_p2_stimuli.py   # -> devices/private/stimuli/p2-v1/
```

`ffmpeg` is required for the content set. Add `--force` to `make_corpus.py` to overwrite and
`--reuse` to `make_content.py` to reuse `_downloads/`.

**Corpus (`make_corpus.py`)**: synthetic Piper voices only (no human voices, no consent needed).
Spanish speakers: `es_ES-sharvard-medium` speakers M and F (two speakers of one model),
`es_ES-davefx-medium`, `es_ES-mls_9972-low`, `es_ES-mls_10246-low`, `es_MX-ald-medium`,
`es_MX-claude-high` (4 apparent female, 3 male; gender is judged from the median f0 and the model's
speaker names, see `corpus.json`). English voices for the wake word: `en_US-lessac-medium`,
`en_US-ryan-medium`. Positives: "alexa" alone for all 9 speakers with 3 length/noise variations
(27 items; the Spanish voices say it with Spanish phonetics, a deliberate accent test), and "alexa" + 0.4 s
+ one of 15 Spanish home-assistant commands for each Spanish speaker (70 items). Negatives: 28 Spanish
sentences (24 conversation, 4 commands without the wake word) and 14 near-misses ("Alexis", "a la mesa",
"relaja", "Alexandria", ...), rotated over the Spanish speakers. Every item is scaled to the same active-speech
RMS (-22 dBFS) under a -6 dBFS peak ceiling, so `play_record.py --gain-db 12` gives -18 dBFS peak for the
loudest item and about -18 to -22 dBFS for the others. Format: 16-bit PCM, 48 kHz stereo (dual mono) and 16 kHz
mono variants of every item and of one session file per kind (`pos`, `neg`, `nearmiss`; 4 s gap between items)
with `session_<kind>_timestamps.csv`. Play a session file with `play_record.py --stimulus <path>`
(any channel count is handled; the gain rescales the file peak). Piper's noise sampling cannot be seeded, so
a regeneration is equivalent, not bit-identical: identify a corpus by `corpus.json["manifest_sha256"]` and keep
the generated directory for comparisons. Voice licenses are in each voice's model card (sharvard CC BY 3.0,
davefx CC0, MLS voices CC BY 4.0, ald Unlicense, claude Apache-2.0, lessac and ryan have their own
research/non-commercial terms); this is why the corpus stays private and uncommitted.

**TV-content set (`make_content.py`)**: only Creative Commons (BY, CC0) or public-domain sources, each item's
license and URL recorded in `manifest.csv`; only HTTPS and `archive.org` hosts are fetched, only media files, into
a fresh `_downloads/` directory, never executed (ffmpeg decodes with an explicit demuxer and without network
protocols). Sources: LibriVox Spanish audiobooks (Lazarillo de Tormes, Caperucita Roja), a LibriVox English
audiobook (Sherlock Holmes), Blender Foundation's *Tears of Steel* (CC BY 3.0: dialogue scenes and the robot
battle as the action scene), Josh Woodward songs (CC BY 3.0, music with vocals) and Kevin MacLeod tracks
(CC BY 3.0, music without vocals). Every clip is converted to WAV 48 kHz stereo with two-pass `loudnorm` to
-23 LUFS (true peak <= -2 dBTP, linear), plus `sampler_10min.wav`, ten 60 s excerpts. Attribution strings are in the
manifest; add a new source only if its license is clearly CC or public domain, and never a source of
doubtful provenance (for example uploads with unclear film copyright).

**Stimuli (`make_p2_stimuli.py`)**: 10 s log sweep 50 Hz-8 kHz (plus its inverse filter, float32, analysis only),
30 s pink noise, 5 s 1 kHz tone at -20 dBFS, and an alignment train of clicks and chirps with irregular spacing
(event times in `manifest.json`), each as 48 kHz stereo and 16 kHz mono. Fixed seed.

## Linked TV mic + reference capture and echo analysis

Used in [P2 session 1](../../docs/discovery/c3-a/P2-session1-audible-and-echo.md). Running it on a
TV needs the owner's authorization for the session, LG hands-free off, and nothing else holding the
PCMs.

- `tv_dualcap.py` runs **on the TV** (its own Python 3.8 and `libasound` through ctypes; nothing
  is compiled or copied besides the script). It opens the mic (`hw:LGSIC,0`, S24_LE 2 ch 16 kHz,
  period 256, buffer 8192) and a reference PCM (default `hw:LGSIC,1` SpeakerFeedback, S32_LE 8 ch
  48 kHz), links them with `snd_pcm_link` so one trigger starts both, and streams both to stdout in
  a framed format with `snd_pcm_htimestamp` records once per second. It writes nothing on the TV.
  Options: `--ref hw:LGSIC,3 --ref-channels 2 --ref-format S32_LE` for SoundEngineCapture
  (`PERIOD_BYTES` is fixed at 2048 there), `--ref none`, `--no-mic`, `--no-link`.
- `tv_dualcap_demux.py` splits the stream on the PC into `mic.raw`, `ref.raw` and `meta.jsonl`.
- `analysis/tvcap.py` (rig venv): `levels` (windowed RMS per channel), `locate DIR STIM.wav`
  (stimulus position and RMS/peak, optionally band-limited), `echo` (GCC-PHAT delay mic versus
  reference, coherence and the coherence-bound ERLE, a 4096-tap partitioned-block NLMS ERLE
  baseline, windowed drift, and with `--digital WAV` the same metrics with the played file as an
  oracle reference). Tests: `.venv/bin/python -m unittest discover -s tools/audio-rig/analysis`.

```sh
scp tools/audio-rig/tv_dualcap.py root@<tv>:/tmp/
ssh root@<tv> 'python3 /tmp/tv_dualcap.py --seconds 30' > cap.tc &
pw-play --target <hdmi-sink> stimulus.wav          # or pacat on the TV, after owner approval
wait
python3 tools/audio-rig/tv_dualcap_demux.py cap.tc --out-dir capdir
.venv/bin/python tools/audio-rig/analysis/tvcap.py echo capdir --start 1 --digital stimulus.wav
ssh root@<tv> 'rm /tmp/tv_dualcap.py'
```

Levels are dBFS of each PCM's own full scale, so mic and reference levels are not comparable in
absolute terms. Coherence uses 8192-point Welch segments; shorter segments underestimate it for
reverberant paths.

## Phase 2 accuracy and false-wake runs through Home Assistant

Used in [P2 session 2](../../docs/discovery/c3-a/P2-session2-wake-accuracy.md). All of them talk to
HA's WebSocket API with a token file (a bare access token, or the JSON from HA's `/auth/token`
with a refresh token, renewed in place with mode 0600; tokens are never printed). They need
`aiohttp` in the rig venv (`.venv/bin/pip install aiohttp`). Scoring follows
[P2 corpus and metrics](../../docs/experiments/P2-corpus.md).

- `ha_ws.py`: the small WebSocket client, plus `summarize_run()` (wake word id and audio
  timestamp, STT text, intent response type, errors of one `assist_pipeline/pipeline_debug` run).
- `p2_digital.py`: the **digital baseline**. Streams each corpus item (16 kHz mono, 2 s lead-in,
  3 s tail, at real-time speed) into HA's pipeline as `assist_pipeline/run` from `wake_word` to
  `stt`, so the conversation agent is never called. Faster than real time loses the audio between
  the detection and the start of STT. The pipeline's wake word timeout is set to 60 s (the API's
  default is 3 s). `--bed mic.raw` mixes each item, at `--speech-gain-db` (default -39 dB: the rig
  reference level at the TV mic, from D3), into a real TV-mic recording and applies the satellite's
  front-end emulation before streaming ("semi-synthetic" conditions); `--per-voice N` selects a
  stratified subset of positives.
- `p2_runner.py`: live runs through the satellite and HA's real integration. Plays one item at a
  time with `pw-play --target <sink>` at the `play_record.py` level convention (`--gain-db 12` is
  the reference), waits until the satellite is idle **and** the next wake word run is armed (newest
  pipeline run in the `wake_word` stage) plus `--settle`, and records detections (entity `idle` →
  `listening`, PC time), the pipeline run (STT text, intent response) and the built-in agent's
  recognition of the transcript and of the reference. `--per-voice N` picks a stratified subset
  of positives (every voice, alternating alone and with a command).
- `ha_assist_watch.py`: logs the satellite entity's state changes and condensed pipeline runs with
  PC times, for false-wake runs while content plays.
- `content_play.py`: plays content clips from `manifest.csv` one after another on a named sink
  (`pw-play`), logging start and end times and the category of each.
- `p2_score.py`: WER normalization (with Spanish numerals), Wilson and exact Poisson intervals,
  the wake windows, the intent mapping (`INTENT_MAP`, written before the runs), per-condition
  tables (`--by voice|subkind|expected_intent`) and `--false-wakes WATCH PLAYLOG` per category.
- `oww_offline.py`: offline python `openwakeword` (ONNX) over a TV-mic recording with the
  satellite's front-end emulated (ch0, 120 Hz high-pass, +24 dB, S16 saturation). A baseline for
  recordings the satellite did not stream; it is not HA's Wyoming openWakeWord service.

Tests (synthetic, no network): `.venv/bin/python -m unittest discover -s tools/audio-rig -p 'test_p2_*.py'`.

**No actuation.** HA's built-in agent executes what it recognizes. Several corpus commands match
real, exposed entities of a home (for example "apaga todas las luces"), so live runs must not use
a pipeline that can act on uncontrolled transcripts. Session 2 used a temporary pipeline whose
conversation language matches nothing in Spanish or English (checked with the debug endpoint
first) and scored intents with the recognition-only endpoint. Check this on your own HA before a
run.

## Offline front-end A/B (`aec_eval/`)

Used in [P2 offline A/B](../../docs/experiments/P2-aec-offline-ab.md). PC only: no TV, no Home
Assistant. Separate venv: `numpy scipy soundfile pyopen-wakeword==1.1.0 faster-whisper`.

- `aec_eval/oww_wyoming.py`: wake word scorer that repeats HA's Wyoming openWakeWord 2.1.0 loop
  (`pyopen-wakeword` TFLite models, threshold strictly above 0.5, trigger level 1, 2 s refractory,
  fresh state per stream). It matched session 2's HA results on 387 of 388 items.
- `aec_eval/semisyn_ab.py`: mixes corpus items into the raw mic of a `tv_dualcap` recording with
  session 2's bed offsets and level, runs `aec_offline` variants with a warm-up, and scores them
  (`@py` and `@digital` reproduce session 2's inputs; `@oracle` adds the speech after the canceller).
- `aec_eval/content_eval.py`: content-only false-wake proxy and in-band ERLE per content segment.
- `aec_eval/ab_report.py`: tables with Wilson intervals, per voice, paired McNemar tests, and the
  calibration against HA. `aec_eval/stt_eval.py`: faster-whisper WER on the command segments.
- Tests: `python3 -m unittest discover -s tools/audio-rig/aec_eval -p 'test_*.py'`.

## Privacy

Recordings can contain people's voices from the room. Keep them short, local and opt-in; tell
everyone present before recording; never commit or upload them; delete them when no longer
needed. Stimuli and analysis outputs are also gitignored. Never log ambient speech by default.
