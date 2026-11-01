# D3: Built-in far-field microphone identification, c3-a (LG C3)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74
- **Step goal:** identify which ALSA capture path, if any, carries the built-in far-field microphones, and which path carries voice from the Magic Remote.
- **Related:** [D2 audio stack inventory](D2-audio-stack.md), [D0-D1 runtime](D0-D1-runtime.md), [02-audio-capture](../../research/02-audio-capture.md), [EXP-002 microphone mapping](../../experiments/EXP-002-microphone-mapping.md)
- **Privacy:** raw logs stay in the gitignored `devices/private/c3-a/D3/`. Nothing from them is committed. No ambient speech was recorded.

Confidence labels follow D2: **verified** (observed on c3-a in this step), **inferred** (derived from observations, not directly tested), **hypothesis** (proposed, needs an experiment), **unknown**.

## 1. Preconditions

- On 2026-11-01 the TV was rebooted with owner approval. The reboot followed the D2 probe, which triggered kernel oopses by reading ASoC `dapm_widget` sysfs nodes (see D2 incident section).
- After the reboot, `/proc/sys/kernel/tainted` = `4097` (proprietary and out-of-tree modules only; the oops flag was cleared). The pre-reboot value was `4225`.
- At 57 s uptime no process held any `/dev/snd` PCM. `pulseaudio`, `audiod` and `voiceinput` were running.
- Still forbidden, unchanged from D2: reading `dapm_widget` files and `/sys/kernel/debug/asoc/*`.

## 2. Step 5a: Magic Remote mic button (2026-11-01)

**Method.** A read-only watcher polled `/proc/asound/card*/pcm*/sub0/status` every 0.2–0.3 s for 120 s. On any state change it dumped `hw_params` and scanned `/proc/*/fd` for `/dev/snd/pcm*` holders. The owner held the Magic Remote mic button and spoke a short phrase twice.

**Run 1.** Inconclusive about the press count. One playback burst was seen. Not used for conclusions.

**Run 2.** Times are seconds since boot.

| Press | Observed `card0 pcm6p` RUNNING | Holder | `hw_params` | Capture PCM opened |
| --- | --- | --- | --- | --- |
| 1 | 284.5–290.0 and 294.2–298.7 | pulseaudio | RW_INTERLEAVED, S16_LE, 2 ch, 44100 Hz, period 441 frames (10 ms), buffer 882 | none on any card |
| 2 | 313.1–318.5 and 323.0–326.6 | pulseaudio | same as press 1 | none on any card |

Observations: **verified** for the status transitions, holders and `hw_params` above. In both presses the only PCM activity was playback on `card0 pcm6p`. No capture PCM on any card was opened during either press.

### Conclusions from step 5a

- **(a) Magic Remote voice audio does not use any ALSA capture PCM on the TV.** It arrives by another path, most likely Bluetooth via `remote.monitor`. Confidence: **verified** for "no ALSA capture" during the two presses. **inferred** for the Bluetooth path, which was not observed directly.
- **(b) LG voice responses and earcons play through the system PulseAudio default sink, which maps to `card0 pcm6p`.** Confidence: **verified** on target for the mapping. This makes it a candidate native route for our TTS playback. Confidence: **hypothesis**, needs a playback experiment.
- **(c) Each press yields two playback bursts,** likely an earcon or prompt followed by the response. Confidence: **inferred** from the timing. Content of the bursts is **unknown**.
- **(d) The remote-mic path says nothing about the built-in microphones.** The hands-free test (step 5b) is next.

## 3. Step 5b: Hands-free ("Hi LG")

**Status: PENDING.**

Planned: enable hands-free on the TV only with owner consent. Use the same watcher as 5a, extended to log which capture PCMs are opened and by whom. Keep the run short. Run the hands-free test with the TV-side playback as in step 5a, or with silence as the default. Hands-free activation state is recorded before and after from `voiceinput/setting/getActivation`, read-only.

Expected signal if the built-in array feeds hands-free: a capture PCM on `card*` held by `voiceinput` or `audiod`, with non-trivial `hw_params`. Failure interpretation: no capture PCM opened during the wake phrase means the wake path is not ALSA-based, or hands-free did not trigger. Record the trigger outcome either way.

## Mic capture with external stimulus — run 1 (sweep, -30 dBFS, JBL)

**Status: done 2026-11-01. One sweep run plus three speech runs at +0/+6/+12 dB (see "Level linearity"); one speaker position, one power state.**

### Setup and method

- **TV state:** on, HDMI input idle, the TV played nothing. LG hands-free was **disabled**, so `voiceinput` did not hold the device. With hands-free enabled, `voiceinput` opened `hw:LGSIC,0` ("WoV PDM Mic", S24_LE, 2 ch, 16 kHz, period 256, buffer 8192) together with `hw:LGSIC,1` ("SpeakerFeedback", S32_LE, 8 ch, 48 kHz). The same parameters were used here.
- **Capture on the TV (owner-authorized, 20 s, room told beforehand):**

  ```sh
  arecord -D hw:LGSIC,0 -f S24_LE -c 2 -r 16000 --period-size=256 --buffer-size=8192 -t raw -d 20 - > tv-wov-sweep-g0.raw
  ```

  The file has 2,560,000 bytes (20 s × 16 kHz × 2 ch × 4 bytes). It stays in the gitignored `devices/private/c3-a/D3/`.
- **Stimulus:** the rig's `sweep_48k.wav` (exponential sweep 100 Hz–8 kHz, 3 s), played by `play_record.py` through a JBL Go 3 Bluetooth speaker in the same room at -30 dBFS peak (gain 0 dB). The PC USB mic (Samson Q2U), the webcam mic (C920) and the sink monitor were recorded at the same time as references. Speaker-to-TV and speaker-to-PC-mic distances were **not recorded**.
- **Analysis (host only):** the rig's `analyze.py` first, then a one-off script. The script did a word-level inspection, 20 ms RMS, deconvolution with the played sweep's inverse filter (resampled to 16 kHz), swept-band SNR, Welch PSDs, coherence and GCC-PHAT. Outputs (PNGs and `d3r1_metrics.json`) are in the gitignored `devices/private/c3-a/D3/analysis/`. Swept-band SNR is the sweep power in the five 31 Hz STFT bins that follow its instantaneous frequency, compared with the median power of the same bins in the quiet window. It is used because broadband room noise at 150–300 Hz dominates wideband SNR.
- **Timing note:** the sweep starts 6.61 s into the TV capture, not about 3 s. The gap comes from SSH and `arecord` startup latency plus Bluetooth latency. The capture clock is not synchronized with the host.

### Decode conclusion (Q1)

The data is **proper S24_LE: 24 valid bits, LSB-justified in a 32-bit little-endian word, with full sign extension into the top byte.** Confidence: **verified**.

- int32 min/max are exactly -8388608/+8388607. Byte 3 only ever holds `0x00` or `0xFF`, and in 100 % of words it equals the sign extension of bit 23. Byte 0 takes all 256 values, and only 0.47 % of words have a zero low byte, so the data is not zero-padded or MSB-justified.
- Read as S32_LE, the signal is 48.2 dB too low: the peak after startup is -92.6 dBFS instead of -44.4 dBFS. S32_LE is not a saner decode.
- After the startup burst, the largest magnitude is 50381, which uses 17 of the 24 bits (sign included).
- Every sample, in every 2 s block, lies on the lattice `floor(k × 101 / 16)` for an integer `k`. The values seen are 0, 6, 12, 18, 25, -7, -13, -19, ... This means a **fixed digital gain of 101/16 = +16.0 dB**, with an arithmetic-shift (floor) rounding, is applied after an integer stage. Confidence: **verified** for the lattice; the gain's location (codec, DSP or driver) is **unknown**.
- `analyze.py`'s S24_LE path (`((v & 0xFFFFFF) ^ 0x800000) - 0x800000`, scaled by 2^-23) gives bit-identical results to a plain int32 read of this file. **There is no decode bug.** The misleading numbers (0 dBFS peak, -27 dBFS lead-in "noise", SNR -31 dB) come from the startup burst falling inside the default 0–2 s lead-in window (see Q2).

### Metrics (quiet window after the transient)

Noise window: TV 0.25–6.51 s (after the transient, 0.1 s before the sweep); PC mics 0.1 s to 0.1 s before the sweep. All signals were resampled to 16 kHz. dBFS values use each device's own full scale and gain, so absolute levels **cannot** be compared across devices. SNRs can be compared, with the caveat that the mic distances differ and are unknown. "Sweep" is the RMS over the 3 s sweep window with the noise power subtracted.

| Capture | Noise dBFS | Noise dBFS(A) | Sweep dBFS | Sweep peak dBFS | SNR wideband dB | SNR swept-band median dB | Swept-band SNR 100–300 / 0.3–1k / 1–3k / 3–7.6k Hz |
| --- | --- | --- | --- | --- | --- | --- | --- |
| TV `LGSIC,0` ch0 | -68.4 | -74.4 | -58.8 | -44.4 | 9.7 | 25.9 | 6.1 / 20.3 / 37.9 / 47.9 |
| TV `LGSIC,0` ch1 | -68.4 | -74.4 | -59.7 | -46.4 | 8.7 | 27.1 | 5.0 / 20.9 / 36.4 / 45.3 |
| TV (ch0+ch1)/2 | -68.6 | n/a | -60.8 | n/a | 7.9 | 26.3 | n/a |
| PC USB mic (Q2U) | -56.0 | -65.8 | -50.5 | -36.2 | 5.6 | 26.3 | 4.4 / 21.9 / 32.3 / 43.4 |
| PC webcam ch0 (C920) | -34.3 | -42.1 | -28.0 | -13.4 | 6.3 | 26.3 | 4.2 / 21.5 / 32.4 / 49.4 |
| PC webcam ch1 (C920) | -32.4 | -40.3 | -27.8 | -12.2 | 4.6 | 24.6 | 4.0 / 20.9 / 34.1 / 43.5 |

Other TV numbers: matched-filter score 38.0/37.1 dB, IR peak to pre-noise 49/47 dB (rig output). Noise after the sweep was -67.9 dBFS (+0.5 dB versus before). The spread of 100 ms noise RMS was 1.1 dB standard deviation (PC mics: 1.2 dB). DC after settling was about -158 dBFS. The A-weighting filter is a bilinear transform at 16 kHz and is approximate near Nyquist.

### Level linearity (speech at +0/+6/+12 dB)

**Question:** does the TV mic level follow the acoustic level, or does vendor AGC/DSP hold it flat?

**Method.** Three more TV captures were made with the same `arecord` command (`tv-wov-speech-g0/g6/g12.raw`, 25/22/22 s). The host played the rig's `speech_full` stimulus on the JBL at playback gains of +0, +6 and +12 dB (-30, -24 and -18 dBFS peak; sink volume unchanged). The stimulus is TTS "alexa" followed by three Spanish phrases. The PC USB mic, webcam and sink monitor were recorded in each session.

For each capture:

- The stimulus was located by matched filter. It starts at 6.59–6.62 s in each TV file.
- The **speech level** is the mean power over the four manifest speech segments, band-passed to 300–4000 Hz, with the noise power subtracted.
- The **noise floor** is the median of 50 ms frame powers in the quiet window (TV from 0.25 s to 0.1 s before the stimulus; PC mics from 0.1 s). The median rejects isolated clicks, such as a pop seen just before the g0 stimulus on both the TV and the USB mic.
- A noise-robust cross-check is the 95th percentile of 50 ms band power over the stimulus span.

Outputs: `d3r2_speech_linearity.json` and `.png` in `devices/private/c3-a/D3/analysis/`.

**Absolute levels (dBFS, ch0 for the TV)**

| Playback gain | TV speech 300–4k | TV noise (wideband / 300–4k) | TV SNR 300–4k | USB speech 300–4k | USB noise (wideband / 300–4k) | USB SNR 300–4k | Monitor speech 300–4k |
| --- | --- | --- | --- | --- | --- | --- | --- |
| +0 dB | -72.7 | -68.2 / -71.1 | -1.6 dB | -63.1 | -55.1 / -64.3 | 1.2 dB | -46.5 |
| +6 dB | -66.6 | -68.7 / -72.0 | 5.5 dB | -57.0 | -58.4 / -65.6 | 8.6 dB | -40.5 |
| +12 dB | -60.7 | -69.4 / -72.5 | 11.8 dB | -51.1 | -57.6 / -65.8 | 14.6 dB | -34.5 |

**Change relative to +0 dB (dB)**

| Capture | Speech 300–4k, +6 / +12 | Per segment at +12 (wake / es1 / es2 / es3) | p95 50 ms, +6 / +12 | Noise (wideband), +6 / +12 |
| --- | --- | --- | --- | --- |
| TV ch0 | +6.2 / +12.1 | 13.2 / 12.0 / 11.5 / 12.0 | +4.6 / +10.1 | -0.5 / -1.2 |
| TV ch1 | +6.1 / +11.9 | 12.4 / 11.7 / 11.8 / 12.1 | +4.6 / +10.2 | -0.5 / -1.2 |
| PC USB mic | +6.1 / +12.0 | 12.8 / 11.4 / 12.2 / 12.6 | +4.7 / +10.2 | -3.3 / -2.5 |
| PC webcam ch0 | +5.9 / +11.9 | 12.8 / 11.0 / 11.9 / 12.6 | +3.9 / +10.0 | -1.2 / -1.4 |
| Sink monitor (digital) | +6.0 / +12.0 | 12.0 / 12.0 / 12.0 / 12.0 | +6.0 / +12.0 | n/a |

**Result.** **The TV mic level scales linearly with the acoustic level**: +6.2/+12.1 dB for ch0 and +6.1/+11.9 dB for ch1. This matches the USB mic (+6.1/+12.0), the webcam (+5.9/+11.9) and the digital monitor (+6.0/+12.0) within about 0.2 dB. The TV noise floor stays flat (-0.5/-1.2 dB, a quieter room over time), so no gain rides up in the pauses. The fixed `floor(k × 101/16)` lattice holds in 100 % of the unique sample values in all three files, so the post-integer gain is constant. Confidence: **verified** on c3-a for playback from -30 to -18 dBFS peak in this geometry; **no AGC, compressor or noise gate is active on `hw:LGSIC,0` in this range**. The p95 cross-check scales slightly less (+4.6/+10.1 dB), and does so equally on every mic. This is expected for a percentile that still contains noise power, and it is not a TV-specific effect.

- At +0 dB the speech is at or below the noise floor (300–4k SNR -1.6 dB on the TV, +1.2 dB on the USB mic), so the +0 dB reference point is the least precise. The +6 to +12 dB step alone gives +5.9 dB on the TV and +5.9 dB on the USB mic.
- **Why the naive `analyze.py` numbers looked flat:**
  - The overall RMS of each TV file (-38.3/-38.1/-38.1 dBFS) is dominated by the startup burst. Excluding the first 250 ms gives -67.5/-67.1/-64.6 dBFS.
  - The 0 dBFS peak is the burst.
  - The speech matched-filter score (25–27 dB) is limited by the speech autocorrelation sidelobes, not by level, and the USB mic shows the same spread (21–28 dB).
- **Untested:** levels above -18 dBFS playback peak (the TV speech peak reached only -44 dBFS), so a limiter or compressor near full scale is still possible. Behaviour with LG hands-free active is also untested.
- Speech SNR (300–4k) on the TV is about 3 dB below the USB mic and about 1 dB below the webcam at +12 dB, with unknown and different distances.

### Answers

**Q2: start-of-capture transient.** Confidence: **verified** for this capture.

| Frames (ms) | What happens |
| --- | --- |
| 0–2 | digital zero |
| 3–82 (0.2–5 ms) | full-scale garbage, identical on both channels (the channels match sample-for-sample for the first 66 frames) |
| 83–160 | the garbage decays |
| 160–500 (10–31 ms) | near-silence (median 12 LSB, about -117 dBFS) |
| 501 (31 ms) | a step to about -33 dBFS, which decays in about 30 ms |

- The 20 ms RMS is within 6 dB of the steady floor by 60 ms. DC is settled by about 100–200 ms.
- After that there are no dropouts, repeated-sample runs longer than 3 frames, or digital-zero gaps.
- The |Δx| at period boundaries (every 256 frames) is 0.94–1.04 × its value elsewhere, so there are no period-boundary glitches.
- Discrete broadband events at about 12.3, 13.9, 15.4 and 18.5 s are seen on both channels. They are most likely room sounds, not periodic (**inferred**).
- **Discard at least the first 100 ms of every stream open, and 250 ms to be safe.**

**Q3: noise floor and SNR.** The true floor is **-68.4 dBFS (-74.4 dBFS A-weighted)** on both channels, not -27 dBFS. The sweep sits at about -59 dBFS RMS and -44 dBFS peak, which gives about 9–10 dB wideband SNR and 26–27 dB swept-band SNR.

- At this playback level, the TV mic path is **on par with or slightly better than the PC mics.** Wideband SNR is 3–4 dB higher. Swept-band SNR is equal overall, and 2–5 dB higher above 1 kHz.
- Confidence: **verified** for this geometry. The comparison is not distance-normalized.
- The level is low in absolute terms, because the stimulus was quiet and the TV path keeps about 24 dB of headroom below its 24-bit full scale even after the +16 dB digital gain.

**Q4: two distinct microphones.** Yes, the two channels come from two physically spaced mics. Confidence: **verified** that the channels are distinct; **inferred** for the spacing.

- Only 0.19 % of samples are identical after startup.
- In the quiet window the difference signal has energy 8.7 dB below the channel mean, and in the band-passed sweep it is 1.8 dB above it.
- Coherence in quiet room noise is 0.94 at 100–300 Hz and 0.74 at 0.3–1 kHz, then falls to ≤ 0.04 above 1 kHz. This follows the diffuse-field curve sinc²(2fd/c) with a best fit of d ≈ 8 cm. A duplicated or beamformed stream would show coherence near 1 at all frequencies.
- The sweep arrives at **ch0 about 162–170 µs before ch1** (GCC-PHAT and IR cross-correlation agree, 2.6–2.7 samples). That is a path difference of 56–58 mm, so the spacing is at least 5.8 cm, consistent with the 8 cm fit.
- Which physical mic is ch0, and the array geometry, are **unknown**.

**Q5: raw or vendor-processed.** Overall judgement: **near-raw DMIC decimator output: DC-blocking high-pass plus a fixed +16 dB digital gain, with no adaptive processing detected.** Confidence: **inferred**, medium-high. Supporting context: LG's own `voiceinput` opens the 8-channel `SpeakerFeedback` stream next to this PCM, which suggests it runs AEC and other processing itself on a raw mic feed (**inferred**).

- *High-pass:* DC is removed (about -158 dBFS) and the startup step decays in about 30 ms, so a DC blocker or high-pass is present (**verified** for DC removal; filter type **inferred**). The sweep response tracks the USB mic within about ±4 dB down to 100 Hz. The noise PSD below 200 Hz runs parallel to the USB mic's, with no extra roll-off. So any corner is below about 40–100 Hz, and the JBL's lack of output below about 100 Hz prevents a closer measurement.
- *Band limit:* the response is flat within a few dB up to 7.9 kHz. The noise PSD falls about 7–14 dB between 7.5 and 7.95 kHz. This is the normal anti-alias edge of a 16 kHz decimator, not speech band-limiting (**inferred**).
- *Notches and shape:* relative to the PC mics, the TV shows +6–12 dB around 1.2–2.5 kHz and above 6 kHz, and a narrow dip near 2.7 kHz. With one position, enclosure, placement and room effects cannot be separated from electronics. No notch can be attributed to processing (**unknown**).
- *Noise suppression or gating:* not detected. In the quiet window, the normalized variance of STFT power is 1.02/1.01, against 1.0 for Gaussian noise and 0.99/1.00 for the PC mics. 0.72 %/0.69 % of time-frequency bins sit ≥ 20 dB below their bin median, against 0.69 % expected for Gaussian noise and 0.72 %/0.65 % for the PC mics. A gated or musical-noise floor raises both numbers. The floor is not unusually low or pumping: 1.1 dB spread, and +0.5 dB after the sweep versus before (**verified** for these statistics).
- *AGC or compression:*
  - The +16 dB gain lattice is constant in every 2 s block, including during the sweep. No gain change happens after the integer stage (**verified**).
  - The TV-minus-USB swept-level spread is 6.3 dB, against 5.6 dB for USB minus webcam. These spreads are normal for different mics and positions and show no extra level tracking.
  - The speech runs at +0/+6/+12 dB scale by +6.1/+12.0 dB on the TV, the same as the PC mics, with a flat noise floor. **No AGC is active from -30 to -18 dBFS playback peak** (**verified**, see "Level linearity").
  - A limiter or compressor at high input levels (TV peaks reached only -44 dBFS) is **not tested**.
- *Beamforming or AEC:* the channels are not beamformed (Q4). AEC is untested because the TV was silent.

**Q6: suitability for an HA voice pipeline (16 kHz mono S16).** Usable as a mic source at the ALSA level. Confidence: **inferred**; no speech, STT or wake-word test yet. Conversion needed:

1. Open `hw:LGSIC,0` as S24_LE, 2 ch, 16 kHz. Treat the values as sign-extended int32, and drop the first ≥ 250 ms after each open.
2. **Channel:** select one channel (ch0) for now. A naive (ch0+ch1)/2 mix lowered wideband SNR from 9.7 to 7.9 dB: low-frequency room noise adds coherently, while high-frequency signal partly cancels at the 2.7-sample inter-mic offset. A delay-and-sum or beamformer can be evaluated later.
3. **Bit depth and gain:** S16 = int24 >> 8 would leave room noise at about 12 LSB RMS. The loudest test speech (+12 dB run) would sit at about -60 dBFS RMS and -44 dBFS peak. The path is linear and has no AGC, so the satellite must add gain itself: a fixed gain before truncating, about +20–30 dB (for example >> 5 to >> 3 with saturation), chosen from a run with a real talker at the intended distance that checks clipping. Use an AGC in the satellite or HA only if the chosen version provides one (not verified here).
4. **Filtering:** a high-pass at about 100–150 Hz would remove the dominant 150–300 Hz room noise band without harming speech intelligibility (hypothesis for STT benefit).
5. **Quality concerns:** no on-path noise suppression or AEC means TV playback will be in the signal; the `SpeakerFeedback` reference exists for AEC work (see [03-playback-aec](../../research/03-playback-aec.md)). The mic PCM is exclusive at the `hw:` level, so it conflicts with LG hands-free while that feature uses it. Its behaviour in standby and with TV audio playing is **unknown**.

### Caveats

- One run, one position, one quiet night-time room, a low stimulus level, and unknown distances. The TV and PC SNRs are not distance-normalized.
- The PC-mic recordings started about 0.55 s before playback, while the TV capture started about 6.6 s before. Noise windows differ in length (TV 6.3 s, PC about 2.3 s).
- The JBL speaker limits the response below about 100 Hz. The 30 ms IR window gives about 1/3-octave reliability at low frequency.
- Only `hw:LGSIC,0` was captured. `hw:LGSIC,1` (SpeakerFeedback) was not tested.
- Hands-free disabled; the effect of LG's own pipeline on this PCM while it is active is unknown.
- `analyze.py` limitation, not a decode bug: for raw TV captures it should exclude the startup burst from the lead-in noise window and the peak. It also reports SNR as a dB difference rather than with the noise power subtracted, which matters at low SNR.
- Raw capture and analysis outputs are not committed.

## 5. Open questions

1. Which capture path, if any, carries the built-in far-field array? Not answered by 5a.
2. Is the Magic Remote Bluetooth voice path visible from `remote.monitor` or another service log? **unknown**.
3. What are the two playback bursts per press (earcon and response)? **unknown**.
4. Does hands-free ("Hi LG") open an ALSA capture PCM? Pending 5b.
5. Is the PulseAudio default sink route (`card0 pcm6p`) stable across power states, and usable for our TTS? **hypothesis**, not tested.
6. Does the `dapm_widget` denylist need to cover any other ASoC debug paths for this firmware? Carried over from D2.

## Offline HA pipeline check (Wyoming), 2026-11-01

**Question:** is audio captured by the TV built-in mic (D3, 25 s, run `tv-wov-speech-g0`) usable by the Home Assistant voice services (wake word `alexa` via wyoming-openwakeword 2.1.0, STT via wyoming-faster-whisper large-v3-turbo, language `es`)? Confidence: **verified-on-target** for the capture; the TTS-over-speaker stimulus is a proxy for human speech, not the same thing.

Method: tool `tools/audio-rig/wyoming_check.py`, services reached through `kubectl port-forward` to the cluster services `home-assistant-openwakeword:10400` and `home-assistant-whisper:10300` (removed afterwards). The whole file goes to openwakeword (Detect, AudioStart, 80 ms AudioChunks, AudioStop, 1 s of silence appended); the Spanish phrases only (stimulus start + 1.5 s to end + 0.5 s) go to whisper. Expected text: "enciende la luz del salón, qué hora es, apaga la luz de la cocina" (14 words). Score = word error rate after lowercasing and stripping accents and punctuation; 0.00 is a perfect transcript.

Decode of the TV raw capture: S24_LE, 24-bit **LSB-justified** and sign-extended (all values within +-2^23, `|int32|` peaks at 2^23 = 0.0039 of 2^31; the high byte is a pure sign extension). All 24 bits are populated (the low byte takes all 256 values), so it is real 24-bit data, not 16-bit padded. DC offset is about zero. The two channels are correlated (0.82) but not identical. The first 1 ms contains a full-scale pop (peak at sample 14); the first 2 s were trimmed. Noise floor afterwards is about -68 dBFS RMS.

Variants: C0 / C1 = channel 0 / 1, Cm = mean; 16 kHz mono S16_LE; "natural" = no gain, "-3" = peak-normalized to -3 dBFS. Timestamps are in the processed signal (for C, add 2 s for the position in the raw file). A "wake time" is the Detection timestamp (end of the word); the expected word is at the stimulus start (A 0.00 s, B 2.81 s, C 4.61 s after trimming, found by cross-correlation; stimulus start in the C raw file is therefore about 6.6 s, as expected).

| Input | Variant | Peak / RMS dBFS | Wake "alexa" (expected at) | Transcript | WER |
|---|---|---|---|---|---|
| A clean reference (digital) | natural (-6 dBFS peak) | -6.0 / -23.7 | detected, 0.56 s (0.00-0.60) | Enciende la luz del salón. ¿Qué hora es? Apaga la luz de la cocina. | 0.00 |
| A clean reference | attenuated to -30 dBFS (playback level) | -30.0 / -47.7 | detected, 0.56 s | same, exact | 0.00 |
| Sink monitor (digital, as played, -30 dBFS) | natural | -30.0 / -49.9 | detected, 3.12 s (2.61 start) | same, exact | 0.00 |
| B PC USB mic | ch0 natural | -28.0 / -55.1 | not detected (2.81) | Enciende la luz del sábado, ¿qué hora es? Apaga la luz de la continuación. | 0.14 |
| B PC USB mic | ch1 natural | -28.0 / -55.1 | not detected | same as ch0 | 0.14 |
| B PC USB mic | mean natural | -28.0 / -55.1 | not detected | Enciende la luz del sábado, ¿qué hora es? Apaga la luz de la luz de la luz. | 0.36 |
| B PC USB mic | ch0 / ch1 / mean, -3 | -3.0 / -30.1 | not detected | "...del sábado/Señor. ¿Qué onda es? Apaga la luz de la continuación." | 0.21 each |
| C TV mic | C0 natural | -37.7 / -67.4 | not detected (4.61) | Enciende la luz del salón. ¿Qué hora es? Apaga la luz de la cocina. | 0.00 |
| C TV mic | C1 natural | -35.2 / -67.4 | not detected | ¿Enciende la luz del salón? ¿Qué onda es? Apaga la luz de la cocina. | 0.07 |
| C TV mic | Cm natural | -37.1 / -67.8 | not detected | Enciende la luz del salón. ¿Qué hora es? Apaga la luz de la cocina. | 0.00 |
| C TV mic | C0 -3 | -3.0 / -32.7 | not detected | Enciende la luz del salón. ¿Qué hora es? Apaga la luz de la cocina. | 0.00 |
| C TV mic | C1 -3 | -3.0 / -35.2 | not detected | Enciende la luz del salón. ¿Qué onda es? Apaga la luz de la cocina. | 0.07 |
| C TV mic | Cm -3 | -3.0 / -33.7 | not detected | Enciende la luz del salón. ¿Qué hora es? Apaga la luz de la cocina. | 0.00 |

Additional wake-word diagnostics (detected or not, nothing else changed):
- Streaming in real time instead of as fast as possible, and a 150 Hz or 300 Hz high-pass before normalization: still not detected for C0, Cm and B.
- The wake word is acoustically present in both recordings: a band-limited (300-4000 Hz) matched filter of the "alexa" segment alone peaks at the same stimulus start as the phrases (normalized correlation B 0.26, C 0.43, versus a median of 0.02).
- Clean reference mixed with the TV's own noise floor (taken from a speech-free part of C) detects "alexa" at every tested SNR from 30 dB down to 0 dB (wake-segment RMS versus noise RMS), and whisper keeps WER <= 0.07 at 0 dB. So the TV noise floor and level alone do not explain the missed wake word.
- Wake-word name: the server lists the model as `alexa`. Requesting `alexa_v0.1` returns no detections and no error. Check how the HA pipeline actually names it before trusting the result above.

Conclusions:
- **STT works** on the TV mic: four of the six C variants are word-perfect (C0 and Cm, natural and -3), the two C1 variants have one error ("hora" to "onda"). The TV mic recording is better than the PC USB mic recording in this run (B WER 0.14-0.36), most likely because of mic placement and distance, which are unknown. Normalization to -3 dBFS did not hurt and did not change C0/Cm.
- **Wake word did not fire** on any acoustic recording (B or C), but fires on the digital copies at the same level. The cause is therefore not the level; likely candidates are the Bluetooth-speaker plus room coloration of a synthetic English voice (Piper) and the low speech-to-noise ratio (speech in C is only about 3-5 dB above the floor in the 300-4000 Hz band), with openwakeword's fixed threshold hiding the actual score. This is **inconclusive** for human speech at normal level and distance, and must not be read as "the TV mic cannot support wake word".
- Residual unknowns: real human voice, speaking level and distance, which channel is better in the room, effect of the TV playing audio or in different power states, and the model name the HA pipeline requests.

Caveats: TTS, not a human voice; a Bluetooth speaker at -30 dBFS playback (system volume 82 %) in a quiet room; mic distance unknown; one take per input; no recordings are committed; the services are the user's own HA instance, reached only through cluster service names.

### +6/+12 dB rerun (2026-11-01)

Louder captures of the same stimulus (playback gain +6 dB and +12 dB relative to the -30 dBFS used above; the owner considers +12 dB realistic), same services and method. Only the first 250 ms were discarded (the startup pop is about 5 ms), decode as before (S24_LE, LSB-justified). "Gain" is a fixed +20 dB with clipping protection (peak kept at or below -1 dBFS; it was never limited here). Timestamps are in the processed signal (raw file position = +0.25 s); the word "alexa" ends at about 6.94 s (stimulus start 6.34-6.37 s).

| Input | Variant | Peak / RMS dBFS | Wake "alexa" | WER |
|---|---|---|---|---|
| C TV mic +6 dB | C0 natural | -48.6 / -67.1 | detected 7.04 s | 0.00 |
| C TV mic +6 dB | C0 +20 dB | -28.6 / -47.1 | detected 6.96 s | 0.00 |
| C TV mic +6 dB | Cm natural | -50.2 / -67.4 | **not detected** | 0.00 |
| C TV mic +6 dB | Cm +20 dB | -30.2 / -47.4 | detected 6.96 s | 0.00 |
| C TV mic +12 dB | C0 natural | -43.8 / -64.6 | detected 6.96 s | 0.00 |
| C TV mic +12 dB | C0 +20 dB | -23.8 / -44.6 | detected 6.96 s | 0.00 |
| C TV mic +12 dB | Cm natural | -44.9 / -65.0 | detected 6.88 s | 0.00 |
| C TV mic +12 dB | Cm +20 dB | -24.9 / -45.0 | detected 6.96 s | 0.00 |
| B PC USB mic +6 dB (control) | mean natural / +20 dB | -38.6 / -55.7 | detected 3.12 s (both) | 0.00 |
| B PC USB mic +12 dB (control) | mean natural / +20 dB | -33.2 / -52.5 | detected 3.36 s (both) | 0.00 |

Findings:
- With louder playback the wake word fires on the TV mic in seven of eight variants, at the expected time (detection timestamp is at the end of the word), and the whole phrase set is transcribed word-perfectly in all eight. The PC USB control behaves the same. The earlier miss at -30 dBFS playback is therefore consistent with a too low speech-to-noise ratio (speech about 3-5 dB above the floor), not with the TV mic or decode path. Confidence: **verified-on-target** for this room, TTS source and Bluetooth speaker; still no human voice.
- The only miss (+6 dB, Cm, natural) is borderline: the same signal with +20 dB, and C0 natural, both fire. Wake detection near the threshold is level-sensitive; digital gain after capture helps, so a software gain stage before the wake engine is worth keeping in mind. One take per variant; no repeated trials, so no detection rate is claimed.
- Oddity: the natural-level peak of the quiet capture analysed earlier (about -37 dBFS) is higher than the +6 dB capture's (about -49 dBFS) despite lower playback gain. That earlier peak came from a non-speech transient (visible near 5 s of the raw file), so peak is a poor level measure; compare RMS or band energy instead.
- Wake word model name sent by the HA "Kiwi" pipeline: **not checked**. Reading `/config/.storage/assist_pipeline.pipelines` needs `kubectl exec` into the cluster, which the original task rules forbade, and no user approval for it has been given. This stays open (the server lists the model as `alexa`; `alexa_v0.1` gives no detections).
