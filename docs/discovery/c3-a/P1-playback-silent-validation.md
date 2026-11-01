# P1: Playback path, silent validation on c3-a (LG C3)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74, kernel aarch64 with a 32-bit ARM userland
- **Date:** 2026-11-01
- **Step goal:** show that the `webos-playback` crate (fetch, decode, resample, `pacat` sink) runs on the TV as a static `armv7-unknown-linux-musleabi` binary, reaches PulseAudio with the expected stream format, validates TLS without a system CA store, and can be cancelled, all **without any audible sound**.
- **Related:** [D2 audio stack](D2-audio-stack.md), [ESPHome Rust satellite research](../../research/esphome-rust-satellite.md) (TTS delivery section), `crates/playback`
- **Privacy:** raw outputs are in the gitignored `devices/private/c3-a/P1-playback/`. No audio was recorded. The TV only received a digitally silent WAV or short speech clips generated with a synthesizer on the dev PC.

Labels: **verified-on-target** (observed on c3-a in this step), **hypothesis**, **unknown**.

## Safety design of the trials

- Payload was an all-zero (digital silence) 8 s WAV served from the dev PC over HTTP (verified: the file's samples are all zero).
- Every `pacat` run used `--volume=0` **and** the example's software `--gain 0` for the later runs. The first run (below) used only `--volume=0` plus the silent payload, which turned out to be the only protection that run had, because the stream volume was not applied.
- Speech clips were decoded on the TV with the null sink only (no `pacat`).
- Binaries lived in `/tmp` only and were removed afterwards. No pulse modules were loaded, no volumes or defaults were changed, nothing under `/sys/devices/platform/soc-audio` or debugfs was read.

## Results

| Check | Result | Label |
| --- | --- | --- |
| `pacat` available | `/usr/bin/pacat`; `paplay`, `parec`, `parecord`, `pamon` are symlinks to it; `pactl` present. Flags used by the sink exist in this version: `--playback --raw --format --rate --channels --volume --device --client-name --stream-name --latency-msec` | verified-on-target |
| Static armv7 binary runs | 1.8 MB stripped example (release, opt-level z, LTO) runs on the TV | verified-on-target |
| HTTP fetch of the clip from the LAN | works (33 to 36 ms for 352 KB) | verified-on-target |
| Stream reaches PulseAudio | `pactl list sink-inputs` showed our stream (client name `webos-voice-satellite`, stream `tts-reply`, binary `pacat`) on sink `pcm_output` (the default sink), format **s16le 2ch 44100 Hz**, channel map front-left,front-right, role `media.type = pdefaultapp` assigned by LG's policy module, state not corked, no resampling on the input | verified-on-target |
| Sink state | `pcm_output` went SUSPENDED to RUNNING while playing and back to SUSPENDED after the stream ended. While suspended the sink had been listed as 48000 Hz, and while running it was 44100 Hz (PulseAudio reopened it at the stream rate). | verified-on-target |
| Playback timing | an 8.000 s clip played in 8054 ms, exit status 0, no leftover `pacat` or sink-input | verified-on-target |
| **Stream volume `--volume=0` was NOT applied** | the sink-input reported **100% (65536) on both channels** although `pacat --volume=0` was passed. Cause unknown: LG's `module-palm-policy` or stream-restore may reset it. The run was still silent because the samples were zero. **Do not rely on `--volume` for level control on this TV.** The crate therefore has an in-process `gain` (`PacatSink::gain`). | verified-on-target (observation), hypothesis (cause) |
| Default sink latency | with no latency flag the sink-input reported about 1.99 s buffer latency (PulseAudio default target buffer) | verified-on-target |
| Cancellation via `StopHandle` after 3 s | child killed, exit reported `playback cancelled`, no `pacat` process and no sink-input left | verified-on-target |
| Parent killed with SIGKILL at 2 s | `pacat` is not killed (no `kill_on_drop` effect on SIGKILL); it sees EOF on stdin and drains what was already written, then exits (gone within 8 s). Harmless for silent data, but a hard-killed daemon can leave a short tail of audio playing. | verified-on-target |
| Decode and resample cost on the TV | 2.44 s of speech (mono 22.05 to 24 kHz source to stereo 44.1 kHz): decode 2 ms (wav), 3 ms (flac), 15 ms (ogg), 17 ms (mp3); resample 74 to 77 ms. 8 s mono 22.05 kHz clip: resample 217 ms. About 27 to 31 ms per second of audio for the resampler. | verified-on-target |
| HTTPS with no system CA store | `https://www.home-assistant.io/` fetched OK through rustls with bundled webpki roots | verified-on-target |
| Invalid certificates rejected | `self-signed.badssl.com` and `untrusted-root.badssl.com` rejected (`UnknownIssuer`); `expired.badssl.com` rejected (expired); `wrong.host.badssl.com` rejected (name mismatch). Non-http(s) schemes are rejected before any connection. | verified-on-target |
| TV clock | correct at test time, which expiry checking depends on | verified-on-target |

## Consequences for the design

- Use `PacatSink` with `gain` for output level, not `--volume`. Whether `pactl set-sink-input-volume` on our own stream works is untested.
- The sink rate can stay at 44100 (LG's own replies use it and the sink runs at that rate while active), or be set to 48000 to match the idle sink; the effect of that choice on latency or quality is untested.
- Time to first audio for a short reply is dominated by fetch + decode + resample (well under 0.3 s on the TV for typical replies) plus PulseAudio start-up; the default 2 s PulseAudio buffer does not delay start because we write everything and then close stdin (drain), but `--latency-msec` is untested and may be worth a trial for streaming playback.
- A private CA in front of Home Assistant works through `FetchConfig::ca_file`, but a file replaces the bundled roots (it does not add to them). Not exercised on the TV.

## Not done / residual unknowns

- No audible playback was attempted (night rule). Actual loudness, pops or amp behaviour at stream start are unknown. The sink did resume (SUSPENDED to RUNNING) when the stream started; whether that is audible at the amplifier was not assessed.
- Behaviour while the screen is off, in standby, while LG voice features are active, or with TV audio playing through other apps is unknown.
- Fetching a real Home Assistant TTS URL (internal http or external https) has not been tried; only a LAN HTTP server and public HTTPS sites were used.
- mp3, flac and ogg decoding was verified on the TV with synthesizer output only; real HA TTS (ffmpeg-converted) files remain to be checked.

## Rollback

All trial files were removed from `/tmp` on the TV (verified: no `p1-*` entries, no `pacat` process, no sink-inputs). Nothing else was modified.
