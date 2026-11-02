# P2 session 1: audible reply path, volume curve, echo and the SpeakerFeedback reference (c3-a)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74, kernel taint 4097 before and after
- **Date:** 2026-11-02, about 11:40 to 12:20 local time, unattended (nobody in the room, nobody spoke)
- **Roadmap:** Phase 2 deliverables 1 (audible reply path and volume) and 2 (D4: echo and the reference); [EXP-005](../../experiments/EXP-005-native-playback.md), [EXP-006](../../experiments/EXP-006-aec-reference.md)
- **Related:** [D2 audio stack](D2-audio-stack.md), [D3 mic identification](D3-mic-identification.md), [P1 playback silent validation](P1-playback-silent-validation.md), [P1 HA integration](P1-ha-integration.md), [P2 corpus and metrics](../../experiments/P2-corpus.md)
- **Privacy:** synthetic voices (Piper) and public-domain or CC-licensed TV content only. Recordings are under the gitignored `devices/private/c3-a/P2/`; none are committed. No ambient speech was logged.

Labels follow [AGENTS.md](../../../AGENTS.md): **verified-on-target** (measured on c3-a in this session), **documented**, **hypothesis**, **unknown**. Every number is from one TV, one firmware, one room and one power state ("Screen Saver", see below); repeat counts are given where they matter.

## Summary

- The audible reply path works: HA's Piper TTS through the satellite and `pacat` at software gain 1.0 is audible at every master volume tested (5 to 25), and a full push-to-talk run (STT, built-in intent, streamed TTS) played its reply at volume 10. **verified-on-target**
- The heard level follows the TV master volume. The steps are not uniform: about +14, +9, +4 and +2.5 dB for 5→10→15→20→25. **verified-on-target**
- With `pacat`'s default buffering the reply became audible about 1.5 s after HA started the TTS stream. With `sink.latency_msec = 200` it was about 0.1 s (one run each). **verified-on-target** (n = 1 each)
- `hw:LGSIC,1` SpeakerFeedback carries both the HDMI program and our `pacat` stream, after the master volume and the TV's EQ. It opens together with the mic, links with `snd_pcm_link` and keeps a fixed 1.40 to 1.46 ms lead over the mic with zero drift. However, its content is **not a clean linear copy** of what the speakers emit: a pure tone shows spurious energy only about 7 dB below the tone. Its coherence with the mic echo is about 0.5 to 0.6, versus 0.97 for the true source signal. **Classification: unusable as a linear AEC reference on c3-a** (usable only for timing and playback-activity detection). **verified-on-target**
- Two other LGSIC taps, not in the original scope but checked because of this result, are clean. `hw:LGSIC,3` SoundEngineCapture is post-volume, has 0.999 coherence with the source, gives an adaptive-filter ERLE equal to the oracle and has zero drift, but its delay to the mic changes on every open (12.9 to 19.1 ms). It is the candidate AEC reference. `hw:LGSIC,2` MixerCapture is a bit-clean, pre-volume copy of the mix. **verified-on-target**
- No stream-based ducking of HDMI audio by our `pacat` stream was seen. A loud stream lowered the HDMI program by about 4 dB, but a quiet one did not, which points to level-dependent dynamics on the mixed output (**hypothesis** for the mechanism).
- The mic did not clip in any test (highest raw peak -13.6 dBFS at volume 25). The satellite's fixed +24 dB front-end gain, however, would saturate its S16 stream on its own reply from about volume 15. That matters for wake word during a reply (session 2).

## TV state and restore point

Read-only at the start; the same values were restored and verified at the end.

| Item | Value | How |
| --- | --- | --- |
| Power state | `Screen Saver` (TV on, screen saver over the idle PC desktop on HDMI 1) | `com.webos.service.tvpower/power/getPowerState` |
| Foreground app / input | `com.webos.app.hdmi1`; HDMI 1 is the only connected input and carries the dev PC | `com.webos.applicationManager/getForegroundAppInfo`, `com.webos.service.eim/getAllInputStatus` |
| Master volume, mute | 6, not muted, scenario `mastervolume_tv_speaker`, max 100 | `com.webos.audio/getVolume`, `com.webos.service.audio/master/getVolume` |
| Sound output | `tv_speaker` (the only entry in `settingsSoundOutputs`) | `com.webos.service.apiadapter/audio/getSoundOutput`, `com.webos.service.audio/getSoundOutput` |
| LG hands-free | off (`settingStatus` and `activationState` false for every provider) | `com.webos.service.voiceinput/setting/getActivation` |
| Capture PCMs | all `closed` before and after | `/proc/asound/card*/pcm*c/sub0/status` |

- Volume was set with `luna://com.webos.audio/setVolume {"volume": N}`. The value read back immediately in every step. This is the same service and method that the webOS TV `ssap://audio/setVolume` call maps to (**documented** for the public API, **verified-on-target** for the luna call). Mute was never changed. No input switch was needed, because the PC was already the foreground input. The luna input switch is therefore **untested**.
- **Tooling gotcha (new):** in this session `luna-send` printed its reply only when SSH allocated a terminal (`ssh -tt`). With a pipe plus `-w 5000`, it printed nothing for every service, including the audio service. This differs from the earlier note in [D0-D1](D0-D1-runtime.md).
- The power state read `ScreenSaver` (without the space) during the HDMI tests. HDMI and `pacat` audio played normally in this state. Behaviour with the screen saver off was not compared.
- **PC side:** the PC's HDMI sink feeds the TV. It was found at 22 % (`14492`, -39.3 dB), unmuted and the default sink, with four browser streams attached whose monitor was digital silence (checked for 4 s). During the HDMI tests it was set to 100 % (0 dB) and then restored to `14492`. The default sink and the other streams were not touched.

## Method

- **Linked capture:** [`tools/audio-rig/tv_dualcap.py`](../../../tools/audio-rig/tv_dualcap.py) runs on the TV from `/tmp` with the TV's own Python 3.8 and `libasound` (ctypes; no binary copied). It opens the mic (`hw:LGSIC,0`, S24_LE 2 ch 16 kHz, period 256, buffer 8192) and a reference PCM. It calls `snd_pcm_link`, starts both with one trigger and streams both over SSH to the PC, where [`tv_dualcap_demux.py`](../../../tools/audio-rig/tv_dualcap_demux.py) splits them. Nothing is written on the TV. [`analysis/tvcap.py`](../../../tools/audio-rig/analysis/tvcap.py) does the analysis: levels, stimulus location, GCC-PHAT delay, Welch coherence (8192-point segments), a coherence-bound ERLE, a 4096-tap partitioned-block frequency-domain NLMS canceller and drift. It has synthetic unit tests.
- **Playback on the TV:** plain `pacat` (48 kHz stereo, raw) for tones and noise, and the satellite (from `/tmp`, file mic, `pacat` sink at gain 1.0) for HA announcements and the end-to-end run. The satellite used a file mic, so the ALSA mic stayed free for the recorder.
- **HDMI playback:** `pw-play --target <the HDMI sink>` on the PC.
- **Stimuli** (synthetic, in `devices/private/stimuli/session1/`; a throwaway generator, not committed): a 1 kHz tone at -12 dBFS peak, pink noise at -20 dBFS RMS (and -26 dBFS RMS), 0.25 s chirps from 300 Hz to 6 kHz, a 6 s exponential sweep from 40 Hz to 20 kHz, and a 3-minute chirp train (one chirp every 10 s). The content clips are 60 s excerpts of the [P2 content set](../../experiments/P2-corpus.md) at -23 LUFS: Spanish audiobook speech, music with vocals and instrumental music.
- **Safety limits:** volume ceiling 25 for test signals and replies. Content was played at volume 6, the TV's original volume, which is lower than 20. Every test sound lasted 15 s or less, and content clips 60 s. The abort path (`pkill` on both sides, then restoring the volume) was prepared but never needed.

## 1. Audible reply path and volume curve (EXP-005)

### Tone and pink noise through `pacat`

The 1 kHz tone (-12 dBFS peak) and pink noise (-20 dBFS RMS) were played with plain `pacat`. Mic values are channel 0. The noise floor was -67 to -68 dBFS RMS throughout. SpeakerFeedback (SF) values are channel 0 (full scale = 32-bit). **verified-on-target**, one run per volume.

| Master volume | SF tone RMS | Mic tone RMS (1 kHz band) | SF pink RMS / peak | Mic pink RMS / peak (ch0) | Mic pink peak (ch1) | Mic clipping |
| --- | --- | --- | --- | --- | --- | --- |
| 5 | -57.2 | -66.6 | -42.9 / -29.4 | -56.3 / -43.5 | -41.3 | none |
| 10 | -43.3 | -52.7 | -30.3 / -17.0 | -44.7 / -32.1 | -30.0 | none |
| 15 | -34.0 | -43.6 | -21.6 / -8.7 | -36.3 / -21.5 | -21.6 | none |
| 20 | -30.2 | -39.7 | -17.7 / -4.5 | -32.3 / -18.9 | -17.0 | none |
| 25 | -27.8 | -37.4 | -15.4 / -2.2 | -30.1 / -17.3 | -14.4 | none |

- **Volume steps** (1 kHz, SF): +13.9 dB (5→10), +9.3 (10→15), +3.8 (15→20), +2.4 (20→25). The mic tracks SF exactly: mic minus SF for the tone is -9.4 dB at every volume (±0.1 dB). **verified-on-target**
- **Equalization:** the stimulus-to-SF response has a shelf about +17 dB above the mid band at 4 to 8 kHz and a bass lift at 63 Hz. The mic shows the same shape. So SF and the speakers are after the TV's EQ, and the 1 kHz tone reads about 17 dB lower than broadband pink of similar digital RMS. **verified-on-target**
- **SF headroom:** at volume 25, pink noise with a -6.4 dBFS digital peak reached -2.2 dBFS peak in SF. Louder content or volumes would clip SF. **verified-on-target** for the level; clipping above it is a **hypothesis**.

### Piper announcements through HA and the satellite

The test called `assist_satellite.announce` with the phrase "Esta es una prueba de volumen del televisor." (2.25 s, `es_ES-sharvard-medium`, no chime). The satellite fetched the media and played it through `pacat` at software gain 1.0. HA's service call returned after 2.4 to 2.6 s each time. "+24 dB peak" is the mic peak after the satellite's default front-end gain, where 0 dBFS or more means the S16 stream saturates. **verified-on-target**, one run per volume.

| Master volume | SF speech RMS / peak | Mic ch0 speech RMS / peak | Mic ch1 peak | Mic ch0 peak + 24 dB |
| --- | --- | --- | --- | --- |
| 5 | -46.3 / -28.3 | -58.0 / -41.4 | -40.1 | -17.4 |
| 10 | -32.9 / -16.8 | -44.7 / -28.8 | -28.8 | -4.8 |
| 15 | -24.0 / -7.7 | -35.6 / -19.7 | -19.5 | +4.3 (saturates) |
| 20 | -20.3 / -3.8 | -31.9 / -16.0 | -16.1 | +8.0 (saturates) |
| 25 | -18.1 / -2.4 | -29.7 / -13.6 | -14.0 | +10.4 (saturates) |

- The satellite's `pacat` stream is subject to the master volume like every other source. There is no separate level for it on this path. **verified-on-target**
- **Volume policy (proposal, not decided):** use the TV master volume as the user's level and keep `sink.gain` at 1.0. A reply loudness target needs a listener and is not set here. For barge-in, the satellite's front-end gain should drop, or capture should be gated or echo-cancelled, while it plays. Otherwise its own reply saturates the stream sent to HA from about volume 15.

### End-to-end run through HA at volume 10

- Setup: a temporary pipeline (`conversation.home_assistant`, `stt.faster_whisper` es, `tts.piper` `es_ES-sharvard-medium`), selected on the satellite's pipeline select. Push-to-talk (SIGUSR1). The file mic was the P1 TV-mic recording of a Piper "¿qué hora es?". Nobody spoke.
- Result: STT returned 14 characters, the built-in intent answered (31 characters), HA streamed the TTS (16 kHz mono), and the reply (2.09 s) was audible: SF RMS -31.7 / peak -14.9 dBFS, mic -46.0 / -25.3 dBFS, no clipping. The satellite logged `stt_end=3412 intent_end=3421 tts_start=3421 playback_end=7241 reply_audio=2089` (ms from push-to-talk). **verified-on-target**, n = 1.
- **Reply start delay:** in the capture, the reply started about 1.5 s after `tts_start`. The streamed reply is shorter than `pacat`'s default target buffer (about 2 s, [P1](P1-playback-silent-validation.md)), so playback waited for most of the stream. A second run with `[sink] latency_msec = 200` started the reply within about 0.1 s of `tts_start` (`playback_end=5030` for a 1.47 s reply), with no gaps in SF. **verified-on-target**, n = 1 each. Changing the default needs a decision and more runs (underruns under CPU load were not tested).
- Afterwards the select was set back to `preferred` and the temporary pipeline deleted. The pipeline list is back to Kiwi and Kiwi Test with Kiwi preferred.

### Ducking and stream interaction

HDMI pink noise (-26 dBFS RMS) played at volume 10. In the middle, a 6 s 3 kHz tone played through `pacat`, once at -12 dBFS peak and once at -42 dBFS peak. The table shows the HDMI program level in the 300 to 2000 Hz band, which excludes the tone. **verified-on-target**, one run each.

| `pacat` tone | HDMI band level before → during → after (SF) | Same at the mic |
| --- | --- | --- |
| -12 dBFS peak | -47.5 → -51.9 → -49.7 dB (-4.4 dB during) | -59.0 → -62.3 → -60.5 dB |
| -42 dBFS peak | -47.0 → -48.2 → -48.9 dB (about -1 dB, within the drift) | -58.7 → -59.3 → -59.7 dB |

- Both runs show a slow decline of about 2 dB over 14 s of steady noise. With the loud tone, the program dropped about 4 dB more while the tone played and recovered over seconds. With the quiet tone, no step appeared. So there is **no stream- or role-based ducking** of HDMI when our stream plays. The drop is consistent with a level-dependent dynamics stage (for example an automatic volume leveller) on the mixed output. That mechanism is a **hypothesis**: the TV's sound-mode and auto-volume settings were not read.
- SF carried the tone and the HDMI noise at the same time. **verified-on-target**

## 2. D4: echo and the SpeakerFeedback reference (EXP-006)

### Concurrency, format and timing

| Question | Result | Label |
| --- | --- | --- |
| Can the mic and SF be open at once? | Yes. Both are PCMs of the same card (`LGSIC`), opened and read together in every run, with no xruns, including two 184 s runs | verified-on-target |
| One device or two? | One card, two capture PCMs (`LGSIC,0` and `LGSIC,1`) | verified-on-target |
| Sample-aligned start | `snd_pcm_link` returns 0, and one `snd_pcm_start` starts both. Trigger timestamps match within 3 µs | verified-on-target |
| SF format | S32_LE, 8 ch, 48 kHz only, period 256 fixed (buffer 512 to 8192). 24 valid bits (the low byte is always 0). ch0 and ch1 carry the program as L/R (correlation 0.999). ch2 to ch7 hold a constant (0x100) | verified-on-target |
| SF at idle | ch0 exactly 0 and ch1 to ch7 the constant before any stream since open; all exactly 0 after a stream ends | verified-on-target |
| Content | the HDMI program **and** our `pacat` stream, mixed | verified-on-target |
| Pre- or post-volume | post-volume, and after the EQ (see the volume table) | verified-on-target |
| Delay, mic behind SF (linked) | 1.417 ms in the 3-minute run (18 chirps, standard deviation 0.1 µs); 1.40 to 1.46 ms across 8 separately opened linked runs (first-chirp method). One unlinked run gave 1.505 ms | verified-on-target |
| Clock drift SF vs mic | -0.001 ppm over 170 s (no measurable drift: one clock) | verified-on-target |
| HDMI source clock vs TV | chirps spaced exactly 10.000000 s in SF and the mic (PC clock and TV capture clock agree within 0.1 ppm) | verified-on-target |

### Signal quality: SF is not a clean copy

| Measurement (volume 10 and 20, HDMI pink noise or the `pacat` tone) | SF | Mic |
| --- | --- | --- |
| Coherence with the digital source, 200 to 4000 Hz (8192-point Welch) | 0.56 (octaves: 0.56, 0.65, 0.56, 0.72, 0.66, 0.64) | 0.97 to 0.98 |
| 1 kHz tone: energy outside the tone | -6.6 to -8.2 dBc: sidebands at ±152 and ±308 Hz at -28 to -35 dBc, harmonics at -33 to -40 dBc, about 10 % AM and up to 0.27 rad of phase jitter | -15 dBc at volume 10, -28 dBc at volume 20 (mostly room noise) |
| Energy at 16 to 24 kHz (the stimulus has none: -74 to -117 dB) | about -50 dB per band, 15 to 16 dB below the 100 Hz to 1 kHz band | n/a (16 kHz rate) |
| Gain over 100 ms windows | standard deviation 0.7 dB (the same as the mic; no gain riding detected) | 0.7 dB |

- The artefacts were the same with SF opened alone, unlinked, and linked with the mic, so they belong to the tap, not to the capture method. **verified-on-target**
- coh(SF, mic) ≈ coh(source, SF) × coh(source, mic). In other words, the extra content in SF does not reach the microphone: SF carries something the speakers do not emit. Possible causes include a non-synchronous rate converter on the feedback path or a sense signal rather than the amplifier input. Both are **hypotheses**; the cause is **unknown**.

### Echo metrics: SF versus the true source

Test signals over HDMI (chirps, the sweep, then 10 s of pink noise at -20 dBFS RMS) at volume 10 and 20, one linked capture each. The "oracle" is the played file itself, which we would have for our own replies but not for HDMI content. ERLE uses a 4096-tap PBFDAF NLMS (mu 0.5, after 2 s convergence) on the pink section. The bound is the coherence-limited ERLE for any linear filter. **verified-on-target**

| Volume | Reference | Delay mic − ref | Coherence 200-4k | Coherence bound | PBFDAF ERLE (wideband / 200-4000 Hz) |
| --- | --- | --- | --- | --- | --- |
| 10 | SF ch0+ch1 | 1.35 ms | 0.54 | 3.5 dB | 8.4 / 3.0 dB |
| 10 | digital source | 1.38 ms | 0.96 | 13.8 dB | 15.9 / 14.5 dB |
| 20 | SF ch0+ch1 | 1.43 ms | 0.59 | 4.6 dB | 8.6 / 4.2 dB |
| 20 | digital source | 1.45 ms | 0.97 | 18.0 dB | 16.0 / 22.8 dB |

- The mic echo at these levels: pink RMS -44.0 dBFS (volume 10) and -31.7 dBFS (volume 20), against a -68 dBFS floor. The speaker-to-mic impulse response is dominated by the direct part: 78 to 80 % of its energy lies within 5 ms of the 1.4 ms peak, about 10 % within 5 to 50 ms and 7 to 8 % within 50 to 250 ms (from the sweep, deconvolved by SF).
- The sweep was too short for the adaptive filter to converge, so its ERLE (about 0 dB) is not used. Its coherence bound (SF 4.9 to 6.6 dB, source 17 to 20 dB) agrees with the pink-noise result.

### Classification of `hw:LGSIC,1` SpeakerFeedback

**Unusable as a linear AEC reference on c3-a** (firmware 03.30.74, TV speaker, power state "Screen Saver"). Evidence: coherence with the mic echo of 0.5 to 0.6, a linear-cancellation ceiling of 3 to 7 dB in the speech band, and spurious content 7 dB below a pure tone, while the true source reaches 0.97 coherence and 14 to 23 dB with the same filter. It **is** usable as a timing and activity reference: it is linked, has a fixed 1.4 ms lead, zero drift, and shows when the TV is playing anything, including HDMI. Label: **verified-on-target** for the measurements; the classification is a judgement from them, for this TV, firmware and state.

### Other LGSIC taps (`MixerCapture`, `SoundEngineCapture`)

These were checked because SF failed; they were proposed as D4-b in [D2](D2-audio-stack.md#proposed-d3d4-actions). Both PCMs accept S16_LE or S32_LE, 2 to 8 ch, 48 kHz, with `PERIOD_BYTES` fixed at 2048. So 2 ch S32 gives the 256-frame period, while 8 ch S32 is refused. They were opened with the same linked capture. No side effect was seen: no xruns, and the TV audio continued. A first attempt with 8 channels failed before any capture, so its two playbacks were not recorded. **verified-on-target**

| Tap | Level / volume | Coherence with source (200-4k) | 1 kHz tone, other energy | Delay mic − tap | Coherence with mic / bound | PBFDAF ERLE (wideband / 200-4000 Hz) |
| --- | --- | --- | --- | --- | --- | --- |
| `hw:LGSIC,1` SpeakerFeedback | post-volume | 0.56 | -6.6 dBc | 1.40 to 1.46 ms, stable across opens | 0.54 / 3.5 dB | 8.3 / 3.1 dB |
| `hw:LGSIC,2` MixerCapture | **pre-volume**: -20.0 dBFS for -20 dBFS digital pink, the same at volume 10 and 20 | 1.000 | -85 dBc | 47.8, 49.9, 49.7 ms (changes per open, fixed within a run) | 0.96 / 14.3 dB | 15.6 / 14.7 dB |
| `hw:LGSIC,3` SoundEngineCapture | **post-volume**: +13.3 dB from volume 10 to 20 | 0.999 | -47 dBc | 12.9, 16.8, 19.1, 17.0 ms (changes per open); within one 3-minute run 17.001 ms, standard deviation 0.1 µs, 0 ppm drift | 0.96 / 13.9 dB | 18.7 / 14.2 dB |

- Both taps carried the HDMI program and our `pacat` tone. **verified-on-target**
- **SoundEngineCapture is the candidate AEC reference:** it gives oracle-level linear cancellation and the mic stays causal behind it. Its delay is unknown until measured after each open. An AEC must therefore estimate the delay at start (for example with GCC on the first seconds of playback or a short chirp) and handle a jump if the device is reopened. Labels: **verified-on-target** for the measurements; **hypothesis** that the delay stays fixed over sessions longer than 3 minutes, across volume, sound-mode or output changes, and in other power states.
- MixerCapture is pre-volume. A canceller using it would see a gain change of up to about 30 dB at every volume step, plus the EQ, and would have to re-adapt.

### TV content at the owner's normal volume

Three 60 s content clips at -23 LUFS were played over HDMI at volume 6 (the TV's original volume; the PC sink at 0 dB), with SF linked. **verified-on-target**, one run each.

| Clip | Mic ch0 RMS (60 s) | SF RMS | SF − mic delay drift over 60 s |
| --- | --- | --- | --- |
| Spanish audiobook speech | -63.8 dBFS | -53.6 | -0.08 ppm (spread 0.015 ms) |
| Music with vocals | -63.4 dBFS | -51.0 | -0.01 ppm |
| Instrumental music | -65.3 dBFS | -44.5 | 0.04 ppm |

- At volume 6, broadcast-loudness content reaches the mic only 3 to 5 dB above its -68 dBFS floor. For comparison, the D3 rig speech at the +12 dB reference level was about -60 dBFS RMS. So at this TV's normal listening level, the echo of normal content is below a talker at the reference level. **verified-on-target** for the levels; the comparison with a real talker is a **hypothesis** until session 2.
- Echo metrics (coherence, ERLE) on these clips are noise-limited: some were negative, because the filter adds noise. They are **not meaningful** and are not used for the classification. Content at higher volumes was not played (session limit).

## 3. Barge-in quick look

- A 10.5 s announcement played at volume 10. Mic ch0: RMS -45.8 dBFS, peak -28.1 dBFS, 0.5 s window RMS between -55 and -42 dBFS. Raw headroom 28.1 dB, no clipping (ch1: peak -26.9 dBFS). **verified-on-target**
- After the satellite's +24 dB front-end gain, the reply peaks at -4.1 dBFS (ch0) and -2.9 dBFS (ch1), close to saturation. From volume 15, the satellite's S16 stream saturates on its own reply (see the announcement table). The wake-during-reply test is left to session 2.

## Negative results, deviations and residual unknowns

- **Negative:** SpeakerFeedback fails as a linear AEC reference (see above). The previous working assumption (the [backlog](../../project/backlog.md) and [roadmap](../../project/roadmap.md) item "AEC with the SpeakerFeedback reference") should move to SoundEngineCapture, pending an AEC evaluation.
- **Not done:** content at volume 10 and 20, as the brief asked. The session's own content limit (the original volume, 6, being lower than 20) took precedence, so content ran only at volume 6. Test signals ran at 10 and 20.
- **Not tested:** the luna input switch (not needed), mute changes, outputs other than the TV speaker, the screen saver off, and the TV's sound mode and auto-volume settings (not read).
- **Unknown:** why SF is distorted; whether SoundEngineCapture's delay stays fixed beyond 3 minutes and across volume or sound-mode changes; whether opening `LGSIC,2` or `LGSIC,3` affects LG's own voice features (hands-free stayed off throughout); ERLE with real double-talk; a nonlinear echo component at higher volumes.
- One run per condition. No rates or confidence intervals are claimed.

## Rollback and cleanup (verified at the end)

- TV: volume 6, not muted, `tv_speaker`, foreground `com.webos.app.hdmi1`, hands-free off. No `webos-satellite`, `pacat` or recorder process remained, there were 0 sink-inputs, and all `LGSIC` capture PCMs were `closed`. `/tmp/webos-satellite`, the recorder script, the kill-switch and mute marker files are absent. Kernel taint 4097. No ASoC/DAPM sysfs node or debugfs path was read.
- HA: the satellite's pipeline select is back to `preferred` and the temporary pipeline is deleted. The access token was revoked and its file deleted. The `lg-c3-a` entry stays and shows unavailable while the satellite is not running, as before.
- PC: the HDMI sink is back at `14492` (22 %), unmuted, still the default sink.
