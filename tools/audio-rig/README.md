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

## Privacy

Recordings can contain people's voices from the room. Keep them short, local and opt-in; tell
everyone present before recording; never commit or upload them; delete them when no longer
needed. Stimuli and analysis outputs are also gitignored. Never log ambient speech by default.
