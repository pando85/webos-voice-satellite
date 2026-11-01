# c3-a (LG C3): validation of the `webos-capture` and `webos-dsp` crates (roadmap 1.2)

- Date: 2026-11-01
- Device: c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74, kernel 5.4 aarch64, 32-bit ARM userland ([D0-D1](D0-D1-runtime.md)).
- Naming note: `P1-*` logs are Phase 1 validation runs; `D<n>` logs are roadmap discovery steps.
- TV state: powered on, LG hands-free voice **off** (no process held `hw:LGSIC,0` at the start; `voiceinput` was running but idle on the PCM). Playback: none. Nothing was played through the TV.
- Method: the example `capture_probe` (armv7 static, built from this repository) was copied to `/tmp` (tmpfs), run, and deleted together with all outputs. Only `/proc/asound` was read besides the PCM node. No `/sys/devices/platform/soc-audio` and no `/sys/kernel/debug` access, no LG process was touched, no reboot, no persistence.
- Recordings (room noise, 10 s each, taken with the owner's knowledge in a quiet room) were copied to the gitignored `devices/private/c3-a/P1-capture/` and are not committed. Hostnames and addresses are omitted here.
- Labels: `verified-on-target` means observed directly on c3-a in this run. `inferred` means deduced from verified facts.

## What was built

- `webos-capture`: `AudioSource` trait, `AlsaCapture` (raw PCM ioctls on `/dev/snd/pcmC{card}D{dev}c`, no alsa-lib), `FileSource`, `list_pcms`, typed `CaptureError` (`MicBusy`, `DeviceNotFound`, `Unsupported`, `Disconnected`, `XrunRecoveryFailed`, `Io`). The only `unsafe` is in one module (`sys.rs`). Struct layouts are checked at compile time (`snd_pcm_hw_params` 604 B, `snd_pcm_sw_params` 104 B, `snd_xferi` 12 B on 32-bit; 608/136/24 on 64-bit) and ioctl numbers are computed from `size_of`.
- `webos-dsp`: `Frontend` (channel select, S24 sign-extension, 2nd-order Butterworth high-pass at 120 Hz, +24 dB gain, S16 hard saturation with a clip counter, level stats). Pure Rust, no `unsafe`.
- Device selection: card by id (`LGSIC`) from `/proc/asound/cards`, PCM by substring of the `id`/`name` field. On c3-a the PCM `name:` line is empty and the `id:` line is `WoV PDM Mic snd-soc-dummy-dai-0`, so the match uses `id` (**verified-on-target**).
- Host checks: unit tests with synthetic signals only (36 tests), clippy `-D warnings`, `cargo fmt --check`.

## Results on c3-a

| Check | Result | Label |
| --- | --- | --- |
| Static armv7 (EABI5, softfp flags) binary runs | yes, exit 0 | verified-on-target |
| Raw ioctl path: `PVERSION`, `HW_PARAMS` (S24_LE, 2 ch, 16 kHz, period 256, buffer 8192), `SW_PARAMS`, `PREPARE`, `START`, `READI_FRAMES` through the 32-bit compat layer of the 64-bit kernel | all succeed; 10 s capture, 0 xruns | verified-on-target |
| Raw level, channel 0, after the 250 ms discard, probe vs `arecord` (10 s each, taken one after the other) | probe -67.6 dBFS RMS, `arecord` -68.0 dBFS RMS (after dropping its first 250 ms). Both channels equal (-67.6 vs -68.0). Difference 0.4 dB, i.e. within room-noise variation | verified-on-target |
| Raw peak | probe -53.6 dBFS, `arecord` -53.8 dBFS | verified-on-target |
| Startup burst | the first 250 ms of the `arecord` capture peak at 0.0 dBFS; the first 250 ms kept by the probe peak at -57.0 dBFS, so the 250 ms discard removes the burst | verified-on-target |
| Front-end level: input RMS -67.6 dBFS, output RMS -44.0 dBFS | output = input + 24 dB - 0.4 dB (the 120 Hz high-pass removes low-frequency noise energy); 0 clipped samples; S16 output peak -30.0 dBFS | verified-on-target |
| Open while another process (`arecord`) holds the PCM | `MicBusy` immediately (no spin, no block); the probe exits with code 3 | verified-on-target |
| Reverse: `arecord` opened while the probe holds the PCM | `arecord` fails with "Device or resource busy" (exclusive, as in D2/D3) | verified-on-target |
| Overrun recovery: probe stopped (SIGSTOP) for 1.5 s then continued (ring is 8192 frames = 512 ms) | one xrun recovered (`EPIPE`, then `PREPARE` and `START`), counter 1, capture continues, no further xruns | verified-on-target |
| Burst after an xrun restart | with `--discard-ms 0` the second after the restart peaks at 0.0 dBFS with 123 clipped samples, the same as at the first start. With the default 250 ms the recovered second is clean (peak -55.2 dBFS, 0 clipped). So the startup burst also follows every **re-start** of the stream, not only the first open | verified-on-target |
| Cleanup | binary and all outputs removed from `/tmp`; PCM status `closed` afterwards | verified-on-target |

## Findings and decisions

1. Opening the node blocking would wait inside the kernel while another process holds it. The crate therefore opens with `O_NONBLOCK` to get `EBUSY` at once, then clears `O_NONBLOCK` for blocking reads. (Reasoning from the kernel's open path; the `EBUSY` result itself is **verified-on-target**.)
2. The startup discard (`discard_ms`, default 250) is re-armed after every overrun recovery, because the burst returns after `PREPARE`/`START` (verified above).
3. `SW_PARAMS` uses `avail_min` = period, `start_threshold` = buffer (the stream is started explicitly) and `stop_threshold` = buffer (overrun when the ring is full).
4. The `FileSource` does not discard anything, since recorded files from `arecord` still contain the burst; callers that replay raw `arecord` files should skip the first 250 ms themselves.

## Not tested

- LG hands-free **on** (not enabled in this run; the `MicBusy` path was exercised with `arecord` as the holder instead of `voiceinput`).
- Speech levels and wake-word behaviour through the front-end (room noise only). The +24 dB default gain is the ADR 0004/D3 recommendation, not re-tuned here.
- Long runs (more than 10 s), CPU and memory cost of the probe, power states, standby, and playback while capturing (D4/D5).
- `hw:LGSIC,1` (SpeakerFeedback) as a capture source.
- `SIGSTOP` was the only overrun trigger; real scheduling stalls were not provoked.

## Rollback

Nothing persistent exists on the TV: the binary and its output files were in `/tmp` and were deleted. Local raw recordings are in the gitignored `devices/private/c3-a/P1-capture/` and can be deleted at any time.
