# AEC front-end: echo cancellation in the satellite

- **Status:** implemented in the daemon, **off by default** (`[aec] enabled = false`). Verified on the PC only: unit tests with synthetic echo, the fake-HA end-to-end harness with a file mic and file reference, and real c3-a recordings processed offline through the daemon's exact front-end. On c3-a (2026-11-02, [P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md)): the CPU benchmark (section 9 step 1) and a silent 10-minute daemon smoke (step 2, without a reference signal) are done; the live A/B (step 3) is pending. The rest of this document is PC evidence unless labelled **verified-on-target**.
- **Date:** 2026-11-02.
- **Scope:** [backlog item 1](../project/backlog.md#1-professional-audio-front-end), the audio front-end (owner priority 1). Device facts are from c3-a (LG C3, webOS TV 8.3.0, firmware 03.30.74, TV speaker, power state "Screen Saver"). Other models, firmware, outputs and power states are **not** covered.
- **Related:** [AEC candidates research](../research/aec-candidates.md) (why `sonora`), [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md) (the reference taps), [EXP-006](../experiments/EXP-006-aec-reference.md), [D3 mic identification](../discovery/c3-a/D3-mic-identification.md), [ADR 0002](../decisions/0002-rust-implementation.md), [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md), [development](../development.md).

Labels follow [AGENTS.md](../../AGENTS.md): **verified-on-target** (measured on c3-a, cited from the discovery records), **verified-on-host** (unit tests or runs on the PC), **verified-on-host, device recordings** (the PC processing real c3-a recordings offline; still not a TV measurement), **documented** (crate source), **hypothesis**.

## 1. Summary

- The mic front-end gains an optional echo canceller: WebRTC AEC3 through the pure Rust [`sonora`](https://github.com/dignifiedquire/sonora) crate (0.2.0, BSD-3-Clause), at 16 kHz in 10 ms frames, **after the high-pass filter and before the +24 dB gain**. Without `[aec] enabled` the chain is the one that shipped before.
- The reference is the TV's own output from `SoundEngineCapture` (`hw:LGSIC,3` on c3-a), matched by PCM id on the mic's card, captured as S32_LE 2 ch 48 kHz (the parameters that worked in P2 session 1), linked with the mic so one trigger starts both, and decimated to 16 kHz.
- On five real mic + reference recordings from P2 session 1, the integrated chain reaches **14.9 to 21.8 dB ERLE in 200 to 4000 Hz** with the defaults (18.0 dB on the pink-noise recording where the research measured 17.9 dB for standalone `sonora`). **verified-on-host, device recordings**
- Measured design choices: the gain stays after the canceller (the canceller on the saturated +24 dB signal leaves 12 to 18 dB more residual echo), the GCC-PHAT delay goes to AEC3 as a hint (no measurable difference from AEC3 alone; pre-delaying the reference was harmful), NS and the silent-reference bypass stay off by default.
- Host cost: 61 µs per 10 ms frame for the whole stage (0.6 % of one Ryzen 9 5950X core), +0.8 MB peak RSS; the static armv7 binary grows by 400 KiB (+15.4 %). **On the TV (armv7) the stage costs about 1.0 ms per frame, 11 % of one core (13 % for the whole daemon in wake mode, 2.8 % with `bypass_when_silent`); the aarch64 build 0.21 ms per frame** (section 3.10, **verified-on-target**).

## 2. Signal flow

```text
            one START (SNDRV_PCM_IOCTL_LINK)
hw:LGSIC,0 "WoV PDM Mic"         S24_LE 2 ch 16 kHz ─┐
hw:LGSIC,3 "SoundEngineCapture"  S32_LE 2 ch 48 kHz ─┤ AlsaDuplex (capture crate)
                                                     │  mic: blocking read, 256 frames
                                                     │  ref: every ready period, no blocking
                                                     │  250 ms discard on both, same duration
                                                     ▼
mic  ── channel 0 ── HPF 120 Hz ──┬──────────► EchoCanceller ──► gain +24 dB ──► S16 (saturating) ──► 512-sample chunks ──► HA
                                  │            (sonora AEC3,          ▲
                                  │             10 ms frames)         │ (no canceller: HPF output goes here directly)
ref  ── >>8 to S24 ── downmix ── FIR 145 taps, 48→16 kHz ──► index-matched 10 ms frames ──► sonora render
                                  │
                                  └──► GCC-PHAT (mic, ref as fed) ──► delay: log, hint to AEC3
```

- File mode (PC tests): `FilePair` reads `mic.path` and `aec.reference_path` in lockstep instead of `AlsaDuplex`; everything after it is the same code.
- The decision of what to run is per microphone open: if the reference is unavailable, the front-end is built without the canceller (section 6).

## 3. Components and decisions

### 3.1 Reference capture (capture crate)

- **PCM and parameters.** P2 session 1 opened `hw:LGSIC,3` with S32_LE, 2 ch, 48 kHz, period 256, buffer 8192 next to the mic (S24_LE, 2 ch, 16 kHz, period 256, buffer 8192), linked them (`snd_pcm_link` returned 0) and started both with one trigger; the trigger timestamps matched. Both LGSIC taps accept S16_LE or S32_LE, 2 to 8 ch, 48 kHz only, with `PERIOD_BYTES` fixed at 2048, so 8 ch S32 was refused (`tv_dualcap.py` metadata in the private capture tree; [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md#other-lgsic-taps-mixercapture-soundenginecapture)). **verified-on-target** (with alsa-lib through the Python rig). The satellite issues the same `HW_PARAMS`, `LINK` and `START` ioctls itself (no alsa-lib); that it behaves the same is a **hypothesis** until the TV smoke run.
- **Selection.** By `aec.reference_pcm_id` (substring of the PCM id or name) on `mic.card_id`, as the mic is selected; never by index.
- **Format.** S32_LE samples are shifted right by 8 bits into the 24-bit scale the rest of the program uses. Only 4-byte container formats (S24_LE, S32_LE) are supported.
- **Start.** Link, then one `START`. If `LINK` or the linked `START` fails, the streams are started back to back, reference first: a skew then keeps the reference ahead of the mic, which a causal canceller needs, and the skew is kept as an upper bound (`AlsaDuplex::start_skew`). The skew was not measured on hardware (the PC has no comparable pair of capture PCMs free).
- **Reading.** The mic read blocks (with the existing stall timeout). After each mic read every reference period that is ready is read without blocking (`poll` with a zero timeout, then one period). The mic is never blocked by the reference.
- **Discard.** The 250 ms startup discard is applied to both streams for the same duration (4000 mic frames, 12000 reference frames) by the duplex itself, so the first frames delivered are aligned. A 250 ms discard is longer than the reference ring (8192 frames at 48 kHz, 171 ms), so the reference is drained during the discard as well.
- **Overruns.** An overrun of either stream restarts both (one group `PREPARE` + `START` when linked) and bumps the reference epoch; the front-end then restarts its alignment and its delay estimate.

### 3.2 Reference resampling

- Downmix to mono, then a 145-tap Kaiser-windowed (beta 8) linear-phase FIR with a 7 kHz cutoff, evaluated only at every third input sample. Group delay 72 input samples = 24 output samples = 1.5 ms (compensated when delays are reported). Passband flat within 0.05 dB at 1 kHz, above -1 dB at 6 kHz, 10 kHz aliases below -60 dB. **verified-on-host** (unit tests).
- On the pink-noise recording the decimated reference gave 18.0 dB ERLE, against 17.9 dB for the same reference resampled offline with SciPy's `resample_poly` and fed at 16 kHz, so the decimator loses nothing measurable (**verified-on-host, device recordings**). The decimator, the delay estimator and the framing together cost about 17 µs per frame on the host, 40 % of sonora's share (section 3.10).

### 3.3 Framing and alignment

- Mic and reference are matched by absolute sample index from the stream start: the reference frame given to sonora with mic frame `k` covers the same 10 ms. Both started on one trigger, so the echo in the mic lags its reference by the acoustic delay only (12.9 to 19.1 ms on c3-a, **verified-on-target**), which keeps the canceller causal.
- A mic frame waits for its reference at most 30 ms (`MAX_WAIT_SAMPLES`); then the missing reference is zero-filled and counted as an underrun. With the reference drained after every mic read, waiting is at most one read (16 ms). **verified-on-host**
- Output comes in whole 10 ms frames, so the canceller adds 0 to 10 ms of framing on top of sonora's own latency.

### 3.4 Delay estimation

- **Estimator.** GCC-PHAT on blocks of 4096 samples (256 ms) zero-padded to 8192, with the cross-spectra of 4 blocks whose reference is active (RMS above -70 dBFS) summed in 200 to 4000 Hz, then whitened. Search window -16 to +160 ms. An estimate is accepted when the peak is at least 8 times the RMS of the correlation in the window. It runs at start until it locks, then again every 30 s of audio. **verified-on-host** (exact lags 0 to 2400 samples in synthetic tests, negative lags, low confidence on unrelated signals).
- **On the recordings** (**verified-on-host, device recordings**): it locked on all five, with confidence 16 to 20: about 1 s after continuous pink noise started (tap3-v10, tap2-v10), but only after several chirps when the reference is sparse (3.2 s into the chirp sequence on tap3-v20a, 31 s on tap3-drift-v10 with one 0.25 s chirp every 10 s), because it waits for four active 256 ms blocks.

  | Recording | Delay found (ms, 200 to 4000 Hz) | Session 1 (full band) |
  | --- | --- | --- |
  | tap3-v10 | 12.43 | 12.9 |
  | tap3-v20a | 16.29 | 16.8 |
  | tap3-v20b | 18.67 | 19.1 |
  | tap3-drift-v10 | 16.52 | 17.0 |
  | tap2-v10 (MixerCapture) | 47.32 | 47.8 |

  The 0.43 to 0.51 ms difference is the band, not an error: the same block GCC-PHAT in NumPy on the raw recording gives 12.44 ms in 200 to 4000 Hz, and the session 1 method gives 12.93 ms over the full band. So the c3-a echo path's delay depends on frequency by about 0.5 ms (**verified-on-host, device recordings**; the cause, for example a speaker crossover, is a **hypothesis**).
- **How the delay is used** (`DelayApply`), measured on the same five recordings (ERLE in dB, 200 to 4000 Hz):

  | Recording | Hint (default) | AEC3 alone | Pre-delay to a 2 ms residual |
  | --- | --- | --- | --- |
  | tap3-v10 | 18.0 | 18.0 | 4.0 |
  | tap3-v20a | 21.8 | 21.8 | 16.7 |
  | tap3-v20b | 15.1 | 15.1 | 13.6 |
  | tap3-drift-v10 | 14.9 | 14.9 | 4.2 |
  | tap2-v10 | 17.7 | 17.7 | 16.1 |

  The hint changes nothing measurable: sonora uses it only to initialise its render buffer after a reset, and AEC3 finds the delay itself within the first second (**documented**, sonora-aec3 `render_delay_buffer.rs`; **verified-on-host, device recordings**). Pre-delaying the reference is harmful: on tap3-v10 the ERLE stayed near 27 dB (wideband) for 5 s after the jump, then fell to 0 dB with AEC3's output equal to its input. The cause inside AEC3 is not known (**hypothesis:** a filter peak too close to the start of AEC3's filter fails its sanity checks). The pre-delay is kept in the code for evaluation only. The default is the hint; `delay_ms` can fix the value instead of estimating it.

### 3.5 The canceller (sonora)

- `sonora::AudioProcessing` with AEC3 at its defaults (transparent mode Legacy), **AEC3's own high-pass filter off** (`enforce_high_pass_filtering = false`, no `HighPassFilter`): the front-end already high-passes at 120 Hz, and without the second filter sonora is transparent with a silent reference (below). Optional NS (`NoiseSuppression::default()`, moderate). AGC2 is not used.
- Latency: 128 samples (8 ms) with AEC3, 224 samples (14 ms) with NS. With a zero reference and no high-pass filter the output equals the input delayed by exactly 128 samples (residual -139 dB). **verified-on-host** (a unit test pins the constant).

### 3.6 Gain staging

- The canceller sees the mic after the high-pass filter, in floating point with full scale 1.0, **before** the +24 dB gain. On c3-a the raw mic never clipped (peak -13.6 dBFS at volume 25), but the +24 dB gain saturates the S16 stream on the satellite's own replies from volume 15 (**verified-on-target**, [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md#piper-announcements-through-ha-and-the-satellite)). sonora clamps at full scale like the C++ library, so the canceller would see that saturation if it ran after the gain.
- Measurement (**verified-on-host, device recordings**): in-band output level after +24 dB on active-reference frames, and S16 samples clipped.

  | Recording | No canceller | Canceller, then gain (the chain) | Gain with saturation, then canceller |
  | --- | --- | --- | --- |
  | tap3-v20a (volume 20 chirps) | -19.7 dBFS, 1428 clipped | -42.2 dBFS, 0 clipped | -30.1 dBFS |
  | tap3-v20b (volume 20 chirps) | -19.9 dBFS, 1393 clipped | -33.4 dBFS, 448 clipped | -29.8 dBFS |
  | tap3-v10 (volume 10 pink noise) | -30.4 dBFS, 0 clipped | -48.4 dBFS, 0 clipped | -30.8 dBFS |

  So the gain stays after the canceller. Headroom: the canceller's input is the raw mic level (peak -13.6 dBFS at volume 25 on c3-a, 13.6 dB of headroom); after it, residual echo can still saturate the S16 stream at volume 20 (tap3-v20b: 448 clipped samples). Lowering the gain while a reply plays, or a limiter instead of hard saturation, are open options (section 10).

### 3.7 Silent-reference bypass (off by default)

- When the reference peak has stayed below -80 dBFS for 2 s (`BYPASS_HOLD`, 200 frames), and never in the first 5 s of a stream (`BYPASS_WARMUP`), sonora is skipped and the mic goes through a 128-sample delay line, so the output timing does not change. On resume, the previous frame is replayed into sonora (output discarded) so its internal buffer holds what the delay line held. A unit test checks that the output stays equal to the delayed mic through the bypass and frame by frame across the resume, and fails when the replay is removed. NS disables the bypass (NS is not transparent). **verified-on-host**
- First tuning (500 ms hold, no warm-up) lost ERLE: tap3-v10 16.2 dB against 18.0 dB (the bypass started while AEC3 was in its first second), tap3-v20b 11.9 against 15.1 dB (short gaps between chirps). With the final values (**verified-on-host, device recordings**):

  | Recording | ERLE without bypass | ERLE with bypass | Frames bypassed | Host CPU without / with |
  | --- | --- | --- | --- | --- |
  | tap3-v10 | 18.0 | 18.0 | 242 | 0.45 / 0.39 % |
  | tap3-v20a | 21.8 | 21.8 | 103 | 0.43 / 0.38 % |
  | tap3-v20b | 15.1 | 15.1 | 103 | 0.43 / 0.38 % |
  | tap3-drift-v10 (3 min) | 14.9 | 14.7 | 14034 of 18375 | 0.46 / 0.10 % |
  | tap2-v10 | 17.7 | 17.7 | 243 | 0.56 / 0.50 % |

- It stays off by default because the losses of the first tuning are not explained and the TV CPU budget is unknown. Turn it on only if the TV benchmark shows the CPU matters.

### 3.8 Noise suppression (off by default)

- NS adds 10 to 12 dB to the measured ERLE on the stationary recordings (tap3-v10 30.7 dB, tap2-v10 30.2 dB, tap3-drift-v10 25.2 dB), because it also lowers the -68 dBFS noise floor; it adds 6 ms of latency and about 14 µs per frame on the host. The research measured about 2 dB more near-end loss in synthetic double talk ([research](../research/aec-candidates.md#synthetic-results-not-device-evidence)). Its effect on wake word and STT is **not measured**, so it stays off until the wake and STT metrics show it helps.

### 3.9 Reference gain

- The research suspected AEC3 ignores quiet references. On these recordings the effect of digital gain on the reference is inconsistent (ERLE in dB):

  | Recording (reference level) | 0 dB | +12 dB | +20 dB |
  | --- | --- | --- | --- |
  | tap3-v10 (-47.2 dBFS) | 18.0 | 18.0 | 18.2 |
  | tap3-v20a (-41.5 dBFS) | 21.8 | 18.3 | 26.8 |
  | tap3-v20b (-41.5 dBFS) | 15.1 | 11.1 | 21.1 |
  | tap3-drift-v10 (-62.0 dBFS) | 14.9 | 10.0 | 14.8 |
  | tap2-v10 (-20.8 dBFS) | 17.7 | 17.6 | 17.5 |

  +20 dB also removed all 448 clipped samples on tap3-v20b. But +12 dB was worse than both on three of five, and the chirp recordings are short and non-stationary (the research did not rank on them either). So the default stays 0 dB and `reference_gain_db` is a candidate for the content A/B. A large gain can clip the reference inside sonora at high volume (**hypothesis**: SoundEngineCapture is about 17 dB below SpeakerFeedback at the same volume, so at volume 25 its peaks would be near -19 dBFS).

### 3.10 CPU, memory, size

Host: AMD Ryzen 9 5950X, one thread, release build (opt-level 3 for `webos-dsp` and the sonora crates through `[profile.release.package.*]`), best of 5. **verified-on-host**

| Measure | Value |
| --- | --- |
| sonora (render + capture) per 10 ms frame | 43 to 46 µs mean, 75 to 200 µs max (55 µs mean on the louder MixerCapture recording); with NS 57 to 69 µs |
| Whole stage per frame (decimator, estimator, framing, sonora), 3-minute recording | 61 µs (1.12 s for 183.75 s of audio, 0.61 % of one core); with NS 75 µs; with the bypass 27 µs |
| Peak RSS of the offline chain (24 s recording) | 3.6 MB without, 4.3 MB with the canceller (+0.8 MB), the same with NS |
| Static armv7 musl binary (`webos-satellite`) | 2,652,420 → 3,061,796 bytes (+409,376, +15.4 %) |
| Static aarch64 musl binary | 2,771,448 → 3,153,528 bytes (+382,080, +13.8 %) |
| `aec_offline` example, armv7 | 751,812 bytes |

- The armv7 binary of `aec_offline`, run under `qemu-arm-static` on tap3-v10 and tap2-v10, gave exactly the same ERLE, delay and lock time as the host build, with and without NS (emulated time is meaningless). The armv7 daemon's `--self-test` passes under qemu.
- **TV cost (measured, [P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md#1-aec-cpu-benchmark-section-9-step-1)):** on c3-a (Cortex-A76 at 1.4 GHz, armv7 build) sonora takes 1.0 ms per frame (23 times the host), the whole stage runs at a real-time factor of 0.11 (NS 0.16, `--no-aec` 0.003), single frames up to 8 ms, peak RSS of `aec_offline` 6.9 MB. The aarch64 build of the same code on the same TV: 0.21 ms per frame, real-time factor 0.028. The daemon with the canceller in wake mode: 13.0 % of one core and 1.86 MB RSS over 10 minutes, 0 xruns; 2.8 % with `bypass_when_silent`. **verified-on-target**. sonora has SIMD for x86 and aarch64 but scalar code on armv7 (**documented**, research), which fits the gap.
- On the TV the armv7 binary gives exactly the ERLE and delay it gives under `qemu-arm`; against x86_64 the broadband ERLE differs by up to 0.9 dB (float codegen), so "the same ERLE as the host" holds only for the recordings named above.

## 4. Real-pair results with the integrated chain

ERLE in 200 to 4000 Hz over frames where the reference is active, the output lag compensated, computed as in the research's [`real_eval.py`](../../tools/audio-rig/aec_eval/real_eval.py). "Mic" is the same chain with the canceller off (high-pass filtered, 0 dB gain). The canceller found the delay itself; no manual alignment. One recording per condition, no double talk. **verified-on-host, device recordings**

| Recording (P2 session 1) | Content, TV volume, segment | Research, standalone (dB) | Chain default (dB) | First 1 s | After 2 s | Clipped at +24 dB, no AEC → AEC |
| --- | --- | --- | --- | --- | --- | --- |
| tap3-v10 (SoundEngineCapture) | pink noise, 10, 2.5 to 12 s | sonora 17.9, C++ 17.8 | **18.0** | 19.7 | 17.7 | 0 → 0 |
| tap3-v20a | chirps, 20, 1 to 9 s | 14 to 28 (AEC3 family, not ranked) | **21.8** | 30.9 | 23.4 | 1428 → 0 |
| tap3-v20b | chirps, 20, 1 to 9 s | same | **15.1** | 30.9 | 12.4 | 1393 → 448 |
| tap3-drift-v10 | one chirp every 10 s, 10, 1 to 184 s | not evaluated | **14.9** | 20.1 | 14.8 | 0 → 0 |
| tap2-v10 (MixerCapture) | pink noise, 10, 2.5 to 12 s | AEC3 family 17.4 to 17.7 | **17.7** | 19.2 | 17.4 | 0 → 0 |

- On the one recording the research ranked (tap3-v10), the integrated chain matches standalone sonora (18.0 against 17.9 dB) with its own reference capture path, decimator, high-pass filter and framing.
- Content recordings with a SoundEngineCapture reference do not exist yet: session 1 played content only with SpeakerFeedback and at volume 6, where the echo is noise-limited. The Phase 2 session 2 "AEC set" (corpus positives over TV content) is what the defaults should be picked on (section 9 step 3).

## 5. Failure modes

| Failure | Behaviour | Log | Evidence |
| --- | --- | --- | --- |
| Reference PCM busy, missing, or parameters refused (other model or firmware) | Mic runs alone, no canceller; retried at the next mic open | `warn aec: echo reference "…" unavailable: …; running WITHOUT echo cancellation` | unit test of the selection; degradation path **verified-on-host** for files only |
| `LINK` or linked `START` refused | Back-to-back start, reference first | `info aec: echo cancellation on: … NOT linked (reason), started reference first` | not exercised on hardware |
| Reference read error or more than 8 overruns in a row while running | Reference closed (unlinked first, so the mic is not stopped); canceller dropped until the mic reopens | `warn aec: echo reference lost: …` | **hypothesis** for the ALSA path (no hardware test) |
| Overrun of either stream | Both restarted, 250 ms discarded, alignment and delay estimate restarted | none beyond the existing xrun counter | **hypothesis** for the ALSA path |
| Reference late or missing for a frame | That frame's reference zero-filled after 30 ms, counted | `debug … reference underruns N` | **verified-on-host** |
| Reference file missing (file mode) | Mic file runs alone | `warn aec: reference file …` | **verified-on-host** (test) |
| Delay not found (reference silent, unrelated) | No lock; AEC3 still estimates on its own | no `echo delay` line | **verified-on-host** |
| Delay jumps (reopen, unknown causes) | Re-estimated every 30 s; AEC3 tracks it itself | `info aec: echo delay changed A -> B ms` | jump behaviour on the TV **unknown** |
| Unsupported reference rate | Rejected by `--check-config`; at runtime, no canceller | config error / `warn` | **verified-on-host** |
| Mute, kill switch, strict firmware guard, failed self-check, unpaired | Both PCMs closed with the mic (the duplex is one source) | existing `microphone closed` lines | **verified-on-target** for HA mute, the kill switch and `stop.sh` (both `closed` in the same 10 ms poll, reopened linked; [P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md#2-daemon-smoke-with-aec-section-9-step-2)); firmware guard and unpaired not exercised with AEC on |

## 6. Configuration

`[aec]` in [config/example.toml](../../config/example.toml); validation in [development](../development.md#configuration-reference). Keys: `enabled` (false), `reference_pcm_id` ("SoundEngineCapture"), `reference_format` ("s32_le"), `reference_channels` (2), `reference_rate` (48000; or 16000), `reference_period_frames` (256), `reference_buffer_frames` (8192), `link` (true), `delay_ms` ("auto" or 0 to 500), `ns` (false), `bypass_when_silent` (false), `reference_gain_db` (0, -20 to 40), `reference_path` (file mode). `ns` and `bypass_when_silent` without `enabled` are rejected, as is `enabled` with a file mic and no `reference_path`.

## 7. Logs and metrics

Never audio. At info: the canceller state at each mic open (reference, format, linked or the reason not), each delay lock and change. At debug every 60 s: delay and confidence, ERLE (mic over output power on active-reference frames) and sonora's own ERLE statistic, sonora µs per frame mean and max and the share of one core, bypassed frames, reference underruns. sonora's ERLE and delay statistics stayed at their initial values in every offline run (0.18 dB, none); **hypothesis**: the port does not update them in this configuration. The satellite's own ERLE figure is the one to read.

## 8. Tools

- [`crates/capture/examples/aec_offline.rs`](../../crates/capture/examples/aec_offline.rs): the daemon's front-end (file pair reading, 256-frame reads, the reference pushed before each mic read, `Frontend` with the canceller) on a recorded mic + reference pair. Writes the 16 kHz mono S16 stream HA would receive and prints JSON metrics. Builds for armv7 (`cargo zigbuild --release -p webos-capture --example aec_offline --target armv7-unknown-linux-musleabi`).
- [`tools/audio-rig/aec_eval/chain_eval.py`](../../tools/audio-rig/aec_eval/chain_eval.py): runs it on `tv_dualcap.py` captures and prints the ERLE, lock, CPU and clipping tables above; `--keep-wavs` keeps each variant's output for openWakeWord and STT scoring.
- The fake-HA harness ([`satellite_e2e.py`](../../tools/esphome-fake-ha/satellite_e2e.py), scenario A) runs the daemon with `[aec] enabled` on a synthetic file pair and checks the lock and the cancelled stream.

To repeat the tables (recordings stay in the gitignored `devices/private/`):

```sh
cargo build --release -p webos-capture --example aec_offline
python3 tools/audio-rig/aec_eval/chain_eval.py --tool target/release/examples/aec_offline --work-dir <private dir> \
  devices/private/c3-a/P2/cap/tap3-v10:2.5:12 devices/private/c3-a/P2/cap/tap3-v20a:1:9 \
  devices/private/c3-a/P2/cap/tap3-v20b:1:9 devices/private/c3-a/P2/cap/tap3-drift-v10:1:184 \
  devices/private/c3-a/P2/cap/tap2-v10:2.5:12
```

## 9. On-TV verification plan (pending)

**Status (2026-11-02):** step 1 done; step 2 done except the delay lock and ERLE, which need a reference signal (the TV played nothing); step 3 pending. Results: [P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md). Each step needs the owner's authorization for that TV session ([AGENTS.md](../../AGENTS.md)). Order: after Phase 2 session 2 has finished with the TV. Nothing is installed persistently; binaries go to `/tmp` and are removed. Opening `hw:LGSIC,3` was done in P2 session 1 without side effects; LG hands-free must stay off ([ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md)). Do not read ASoC/DAPM sysfs or debugfs nodes.

### Step 1: CPU benchmark (no audio device opened)

- **Question:** what does the stage cost on the TV's cores in the 32-bit userland, with and without NS, and how much of a core is left?
- **Commands** (PC, then TV):

  ```sh
  # PC: build and checksum
  cargo zigbuild --release -p webos-capture --example aec_offline --target armv7-unknown-linux-musleabi
  B=target/armv7-unknown-linux-musleabi/release/examples/aec_offline
  sha256sum $B
  # copy the binary and ONE recorded pair (no speech: tap3-drift-v10, 3 min of chirps) to the TV's /tmp
  scp $B devices/private/c3-a/P2/cap/tap3-drift-v10/{mic,ref}.raw <tv>:/tmp/
  # TV: quiet system, note the load first
  cat /proc/loadavg
  for v in --no-aec "" --ns --bypass; do /tmp/aec_offline --mic /tmp/mic.raw --ref /tmp/ref.raw --out /tmp/out.wav $v; done
  # repeat 3 times; then once with a second core busy:  (yes > /dev/null &) ; ... ; kill %1
  # also the synthetic benchmark of the research: docs/research/aec-candidates.md#benchmark-on-the-tv-prepared-not-run
  # rollback
  rm -f /tmp/aec_offline /tmp/mic.raw /tmp/ref.raw /tmp/out.wav
  ```

- **Signals:** `wall_s` against 183.75 s of audio, `cpu_us_mean` and `cpu_us_max` per frame, and the ERLE and lock lines, which must equal the host's (14.9 dB, 16.52 ms). The recording holds no speech.
- **Result (2026-11-02):** 1.0 ms per frame, 11 % of one core (armv7), 0.21 ms with the aarch64 build; ERLE and delay identical to the armv7 build under `qemu-arm` (the host's 14.9 dB in-band figure was not recomputed on the TV). That is the 10 to 30 % band below.
- **Interpretation:** below 10 % of one core with the canceller: keep the defaults. 10 to 30 %: turn on `bypass_when_silent`, check thermal behaviour and the daemon's watchdog. Above 30 %, or `cpu_us_max` close to 10 000 µs: the scalar armv7 path is too slow; try the aarch64 build, then the C++ WebRTC fallback with NEON ([research](../research/aec-candidates.md#recommendation)). Different ERLE from the host means a float or codegen difference on the TV: stop and investigate.

### Step 2: daemon smoke with the reference (silent, no speech)

- **Question:** does the satellite open, link and read `hw:LGSIC,3` with its own ioctls, and does it release it?
- **Setup:** the installed daemon in `/tmp` ([development](../development.md#install-on-a-tv-non-persistent)) with `[aec] enabled = true`, log level debug, `sink.gain = 0.0`, push-to-talk mode, HA connected or the bridge. Nobody speaks; the TV plays a test signal over HDMI at volume 10 (the session 1 stimuli).
- **Expected:** `aec: echo cancellation on: reference PCM "SoundEngineCapture" (2 ch, 48000 Hz, s32_le), linked start`; an `aec: echo delay` line between 11 and 19 ms (200 to 4000 Hz, about 0.5 ms below the session 1 figures); debug lines with ERLE above 10 dB during the stimulus and no reference underruns; `/proc/asound/card*/pcm3c/sub0/status` `RUNNING` while open; both `pcm0c` and `pcm3c` `closed` within 250 ms of `touch /tmp/webos-satellite.disable` and of HA's mute, and reopened after.
- **Failure interpretation:** `unavailable: busy` means another process holds the tap (stop and record it); `NOT linked` means the raw `LINK` differs from alsa-lib's (the fallback still runs; record the reason); underruns mean the reference period differs from the request (record `hw_params`); a missing lock with a playing stimulus means a format or scaling error.
- **Rollback:** stop the daemon with the ops scripts, check that every LGSIC capture PCM is `closed`, remove `/tmp/webos-satellite`.
- **Result (2026-11-02, wake mode, silent TV, no stimulus):** reference opened and linked (trigger timestamps within 2 µs), 0 xruns and 0 reference underruns in 10 minutes, 13.0 % of one core, RSS 1.86 MB flat; mute, kill switch and stop closed both PCMs, unmute reopened both linked. Delay and ERLE stayed `n/a` (0 active-reference frames): still to verify with a stimulus.

### Step 3: live A/B with the AEC set during content

- **Question:** do the canceller (and NS, and reference gain) improve wake word false rejects, false accepts per hour and STT accuracy while TV content plays, at the owner's volumes, and do they hurt anything in quiet conditions?
- **Recording first (offline A/B):** with the session 2 AEC set (corpus positives and negatives played over TV content, consented or synthetic voices only), record mic plus `hw:LGSIC,3` with `tv_dualcap.py --ref hw:LGSIC,3 --ref-format S32_LE --ref-channels 2` at volumes 6, 10 and 20 into `devices/private/c3-a/P2/session2/`. Then on the PC: `chain_eval.py --keep-wavs` with the variants `default`, `ns`, `ref+12dB`, `ref+20dB`, `bypass` and the no-AEC output, and score every kept WAV with the session 2 openWakeWord and STT scoring (the same scorer for every variant).
- **Live A/B (online):** the daemon on the TV in wake mode, alternating `enabled = false` and `enabled = true` (same config otherwise) every block of the same played stimuli, randomised order, HA's real pipeline. Record wake detections, STT text and the latency line per run.
- **Pass criteria (proposal, for the owner to decide):** with content at volume 10, false rejects at least halved against no AEC and false accepts per hour not higher; in quiet conditions no worse than no AEC within the corpus noise; STT WER not higher. NS and reference gain are enabled only if they improve the same metrics.
- **Then:** record the decision on the defaults (and on enabling the canceller by default) in an ADR, as the [backlog](../project/backlog.md#1-professional-audio-front-end) asks, and add the result to the [compatibility table](../compatibility/README.md) scoped to model, firmware and power state.

## 10. Open questions

- Whether to ship the aarch64 build (four times cheaper canceller on c3-a) or to enable `bypass_when_silent` by default on armv7; the daemon has not run as aarch64 with capture and `pacat` on the TV yet.
- Double talk: near-end loss on real speech over content (only synthetic double talk exists, from the research).
- The defaults for NS and reference gain, from the session 2 AEC set (step 3).
- Residual saturation of the +24 dB stream during loud replies (tap3-v20b): a lower gain while the satellite plays, a limiter, or an automatic gain stage.
- Why the first bypass tuning and the pre-delay cost ERLE inside AEC3.
- Whether SoundEngineCapture's delay jumps within long sessions or on volume, sound-mode or output changes, and how fast AEC3 follows (P2 open question).
- Behaviour with other outputs (eARC, Bluetooth) and in other power states; on other models the reference PCM may have another name or not exist (the mic then runs without the canceller).
- Using the satellite's own TTS stream as a second, digital reference during replies (research open question).

## 11. Offline evaluation results

From the [P2 offline A/B](../experiments/P2-aec-offline-ab.md) (2026-11-02): corpus positives mixed into the raw mic of the session 2 AEC recordings, the reference untouched, processed by `aec_offline` and scored with a Wyoming openWakeWord 2.1.0 replica that matched HA's session 2 results on 387 of 388 items. **observed-on-other-device, semi-synthetic**; nothing here ran on a TV.

- **Reference-level talker (about -61 dBFS at the mic), wake rate quiet / volume 10 / volume 15:** no AEC 43 / 11 / 1 %; default canceller 43 / 12 / 4 %; with NS 51 / 14 / 4 %; linear output 43 / 21 / 4 %; linear output + NS 54 / 20 / 6 %. No variant restores detection over content.
- **Talker 10 dB louder:** no AEC 57 / 34 / 11 %; default canceller 57 / 41 / 15 %; linear output 57 / 46 / **38 %**; linear + NS 61 / **49** / 33 %.
- **Why:** AEC3's residual-echo suppressor removes a quiet talker together with the echo. Keeping the talker out of the canceller (diagnostic) would give 34 % / 26 % at the reference level. AEC3's linear filter alone removes only 4 to 6 dB in-band on c3-a (full output 10 to 18 dB), so the linear output keeps the talker but more echo.
- **STT** (faster-whisper large-v3-turbo, WER without the MLS voices; clean items 8.6 %): quiet room 7 to 9 % for every variant; volume 10 no AEC 64 %, default 53 %, linear 29 %; louder talker at volume 15 no AEC 57 %, default 51 %, linear 18 %.
- **Gain:** +30 or +34 dB after the canceller changes nothing beyond one to three items and clips; the fixed +24 dB stays. **NS** helps in quiet (+7 to +10 of 97) and is neutral with content.
- **Two-channel reference:** +2.6 to +3.5 dB mean ERLE on the content (+10 to +17 dB on the action scene), but 18.0 → 8.4 dB on session 1's near-mono pink noise; no wake gain at the reference level.
- **False wakes:** 0 in 10 minutes of content per volume for every variant but `linear+ns` (1 at volume 10); the linear output gives the detector much higher scores on content (up to 0.62 against 0.11 without AEC).
- **New, experimental keys** (defaults unchanged): `render_channels = 1 | 2` (stereo render to AEC3) and `output = "full" | "linear" | "nearend"` (AEC3 driven directly through sonora-aec3; the direct path with AEC3's defaults is sample-for-sample identical to sonora's `AudioProcessing`, unit-tested). See [configuration](#6-configuration) and [config/example.toml](../../config/example.toml).
- **Proposal for the live A/B (step 3):** `enabled = true`, `ns = true`, `render_channels = 1`, gain +24 dB; arm A `output = "linear"`, arm B `output = "full"`, plus a no-AEC control. Keep the shipped default off until the A/B passes; measure false wakes per hour with the linear output over the full content set.

## 12. Tuning results and parked state

**Status (2026-11-03): PARKED** by the owner. The echo-cancellation work and TV session 3 stop here. The shipped default stays **`[aec] enabled = false`**, and every key below is experimental with unchanged defaults.

From the [P2 AEC tuning](../experiments/P2-aec-tuning.md) (offline, semi-synthetic, c3-a recordings; **observed-on-other-device**): parameters were chosen on four corpus voices and reported on three other voices plus the two MLS voices.

- **AEC3's own suppressor parameters do not rescue a quiet talker.** That covers the masks, near-end detection, echo-path strength, audibility, ERLE bounds, comfort noise and filter lengths. **Bounding the suppression does.** The new `linear_mix` blends the aligned linear output back into the final output.
- **Best-known configuration ("B"):**
  - `[aec]`: `enabled = true`, `render_channels = 2`, optionally `ns = true`;
  - `[aec.aec3]`: `"linear_mix" = 0.4`, `"ep_strength.default_len" = 0.0`.
- **What B gives offline:**
  - On the tuning voices, 42 % on content against 38 % for the linear output and 26 % for the default. Its ERLE is at the default's level (14.9/20.4 against 13.3/20.2 dB at volume 10/15), and its detector scores on content are back near the default's.
  - Held-out, quiet / volume 10 / volume 15 at the reference level, then 10 dB louder:

    | Variant | Reference level | +10 dB |
    | --- | --- | --- |
    | B | 25 / 13 / 0 % | 42 / 45 / 25 % |
    | B + NS | 35 / 16 / 7 % | 44 / 44 / 25 % |
    | default | 25 / 7 / 2 % | 42 / 31 / 11 % |

  - B is better than the default with a louder talker (p 0.008 to 0.06). It is not significantly better than the plain linear output on the held-out voices. Nothing works at volume 15 for a reference-level talker.
- **Rejected or null:**
  - RNNoise (nnnoiseless) removes the talker and costs about 4× the canceller.
  - AGC2 as a limiter or adaptive gain had no effect.
  - Longer filters did not help.
- **Wake threshold:** keep HA's 0.5 and trigger level 1 (the owner's GitOps values).
  - Level 2 costs 14 to 17 points in quiet, and its count never resets within a long stream.
  - 0.4 gains 0 to 7 points at about 1 % more stray wakes on negatives.
  - The content false wakes for B were 0 at every threshold from 0.2 to 0.6, but 22.5 minutes bounds the rate only at ≤ 8 per hour.
- **CPU (host):** B costs about 1.2× the default (1.4× with NS). Scaled to the TV's measured 1.0 ms per frame (armv7) and 0.21 ms (aarch64), that is about 1.2 ms and 0.25 ms per frame (**hypothesis**, not measured on the TV).
- **Code:**
  - `[aec.aec3]` (39 named AEC3 overrides plus `linear_mix`);
  - the direct engine now takes a stereo render (sample-identical to sonora's `AudioProcessing` for the defaults, mono and stereo);
  - an AGC2 stage in the dsp crate only, not in the daemon config;
  - `aec_offline --aec3/--agc2-*/--rnnoise`;
  - `tune_eval.py` and `tune_report.py`.

**Open items when the work resumes:**

1. On the TV: the delay lock and ERLE with a playing stimulus (step 2 left them `n/a`), and the CPU of B on armv7 and aarch64.
2. A false-wake soak of several hours with B (and with NS) over TV content, which offline can only bound at ≤ 8 per hour.
3. A JBL talker A/B (the rig loudspeaker at a measured distance, through the room) of no AEC, the default and B.
4. A live-person test with consenting speakers.
5. STT on the kept tuning WAVs, which was not run.

**Session 3** was started and stopped with the parking: partial raw data from c3-a (`armL-v10-part1/2`, `t1-default` and logs) is in the gitignored `devices/private/c3-a/P2/session3/`. It has not been analysed or documented.
