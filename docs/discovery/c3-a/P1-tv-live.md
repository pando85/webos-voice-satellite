# P1: The satellite on the TV, live mic, silent (roadmap 1.5)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74; TV on, normal use state, LG hands-free off (the capture PCM was `closed` before each start)
- **Date:** 2026-11-02
- **Binary:** `webos-satellite` 0.0.1, static `armv7-unknown-linux-musleabi`, 2.5 MB, run from `/tmp` only. Two builds were used: the first for test (a) and the first test (b) attempt, and a second with the disconnect fix described below, for the rest. Each was checksummed before and after copying.
- **Home Assistant side:** HA cannot open connections to the TV ([P1 HA integration](P1-ha-integration.md#network-check)). The TV was again refused by the config flow on 6053 (`Errno 111`, and the satellite saw no connection), so `tools/esphome-fake-ha/ha_bridge.py` on the dev PC stood in for HA's ESPHome integration. It drove the owner's real "Kiwi" pipeline (openWakeWord `alexa`, faster-whisper, Piper).
- **Related:** [P1 capture validation](P1-capture-validation.md), [P1 playback silent validation](P1-playback-silent-validation.md), [development: running the satellite](../../development.md#running-the-satellite)
- **Privacy and safety:** no sound was played. Every reply went to the WAV sink or to `pacat` with in-process gain 0. Nobody spoke during the live-mic tests. Logs and samples are in the gitignored `artifacts/p1-tv/`. No ASoC/DAPM sysfs or debugfs node was read. Nothing outside `/tmp` was written.

Labels: **verified-on-target**, **hypothesis**, **blocked**.

## Results

| Check | Result | Label |
| --- | --- | --- |
| Live ALSA mic → HA pipeline (push-to-talk via SIGUSR1) | 15.04 s of room audio streamed (481 280 bytes = exactly 16 kHz × 2 B). HA's VAD never started (silent room); STT ended at its timeout after 15.06 s. Whisper returned `Gracias por ver el video.`, a typical Whisper hallucination on noise. Room level: input RMS -68 dBFS, peak -54 dBFS; after +24 dB, RMS -44 dBFS, no clipping | verified-on-target |
| Full chain on the TV from TV-mic audio (file mic, wake mode) | Wake word detected at 6.98 s of audio; STT `¿Qué va eso?` (same as on the PC); intent failed on HA's side (missing Ollama model); the bridge's labelled substitute reply (2612 ms of Piper audio) was streamed to the TV | verified-on-target (intent substituted) |
| Reply reaches PulseAudio silently | `pactl list sink-inputs` showed our stream (`application.name = webos-voice-satellite`, `media.name = tts-reply`, binary `pacat`), `s16le 1ch 16000Hz`, not corked, volume 100 % (the requested volume is ignored on c3-a, as known), buffer latency 576 ms, sink latency 26 ms, in 26 consecutive polls. The satellite logged **0 non-zero samples** out of 41 796 written; `pacat` drained and exited normally | verified-on-target |
| Playback timing with `pacat` | TTS stream start 3806 ms after the wake word; `pacat` finished 8011 ms after it, so 4.2 s for 2.6 s of audio (PulseAudio's default buffering, see [P1 playback](P1-playback-silent-validation.md)). The media player went back to idle 1969 ms after HA's stream ended | verified-on-target |
| Stability, 11 min 20 s connected in wake mode with the live mic | 21 435 chunks sent, **0 dropped**, 0 clipped, no errors. 1 xrun, caused deliberately by a 5 s `kill -STOP`; capture recovered by itself (re-prepare plus 250 ms discard) and streaming continued. The audio counts match real time: 9 600 000 bytes in the first 300 s window, 9 431 040 in the second (300 s minus the 5 s stop and the discard) | verified-on-target |
| Re-arm behaviour | After HA's 300 s WebSocket run limit ended a wake run (a bridge artefact), the satellite re-armed in 250 ms. After a bridge disconnect and reconnect it re-armed at once. With HA disconnected it keeps the mic open but sends nothing | verified-on-target |
| Shutdown | SIGTERM: playback stopped, capture released (`chunks sent 21435, dropped 0, xruns 1`), HA told to disconnect, process gone within 2 s, PCM status `closed` | verified-on-target |
| HA's ESPHome integration connects to the TV | Refused (network) | blocked (needs owner) |
| A phrase spoken by a live person is transcribed and answered | Not done: nobody spoke, and no sound may be played in Phase 1 | open (Phase 2) |

## Resource use (from `/proc` every 2 s, CPU as % of one core, `CLK_TCK` 100)

| State | CPU | RSS | Threads | Network (wlan0, whole TV) |
| --- | --- | --- | --- | --- |
| Connected, idle (push-to-talk mode, mic open, not streaming), 37 s | 0.44 % | 848 kB | 2 | tx 0.1 kbit/s, rx 0.8 kbit/s |
| HA disconnected, mic open, 20 s | 0.39 % | 1364 kB | 2 | tx 1.4 kbit/s |
| Streaming the live mic (wake mode), 676 s | 1.63 % (1.67 % and 1.63 % in the two halves) | 836 kB at start, 1204 kB after 1 min, 1364 kB from minute 7 to the end (VmHWM 1364 kB) | 2 | tx 281 to 317 kbit/s, rx 13 kbit/s |
| Streaming a file mic (wake mode), 8 s | 2.0 % | 1144 kB | 2 | tx 280 kbit/s |
| Receiving a TTS stream to `pacat` | 2 to 4 % (4 s window, coarse) | 1148 kB | 2 (3 briefly when the WAV sink writes its file) | rx about 100 kbit/s |

- The mic payload is 256 kbit/s (16 kHz × 16 bit). The rest of the tx rate is Noise, ESPHome framing and TCP, plus whatever else the TV sent, since wlan0 counts the whole TV.
- `pacat`'s own CPU was not measured. PulseAudio's CPU for the 16 kHz to sink-rate conversion was not measured either.
- RSS grew in steps (+32 kB after the SIGSTOP, +128 kB at minute 7) and then stayed flat for the last 4+ minutes. 11 minutes do not settle whether there is a slow leak. The multi-day run is Phase 3.
- File descriptors: 11 while connected.
- The TV's load average was about 13 throughout, from LG's own processes, before and after the satellite ran.

## Issues found and fixed during this step

- **Reply cut off when HA disconnects right after a reply.** In the first test (b) run, the bridge disconnected 1 s after the TTS stream ended. The voice state machine then emitted `StopPlayback`, which killed `pacat` while it was still draining PulseAudio's buffer. Fix (in `webos-esphome-api` and the satellite): in SPEAKER mode a reply whose stream has fully arrived keeps playing after a disconnect. A partial stream, URL playback and announcements still stop. Unit test added. The rerun showed the full drain and 0 non-zero samples.
- **HA bridge:** it now waits for the device's media player to return to idle before ending a run. It no longer sends audio or an end marker after HA has closed a run, which had caused two "binary message for non-existing handler" errors in HA's log. Wake runs now ask for a 3600 s run timeout instead of HA's 300 s default.

## Procedure (for repetition)

1. `make build-tv`; checksum; copy the binary, a config (`[mic] source = "alsa"` or a file, `[sink] type = "wav"` with `dir` in `/tmp`, or `"pacat"` with `gain = 0.0`) and the Noise key to `/tmp/<dir>` on the TV (mode 0700, key and config 0600).
2. Check read-only: `/proc/asound/card*/pcm*c/sub0/status` is `closed` (hands-free off), `iptables -S` is all ACCEPT, nothing of ours is running.
3. Start in the background with the log in `/tmp`, and a `/proc` sampler loop (`stat` fields 14/15, `VmRSS`, `Threads`, `/proc/net/dev`) for the PID.
4. On the PC: `ha_bridge.py --device <tv>:6053 --psk-file … --ha-url … --token-file … --pipeline Kiwi`. Push-to-talk: `kill -USR1 <pid>` on the TV. While a reply plays: poll `pactl list sink-inputs`.
5. Stop with SIGTERM. Copy logs back. `rm -rf /tmp/<dir>`. Check that no process, sink-input or file is left.

## Rollback and leftovers

- TV: `/tmp/wsat` removed; no `webos-satellite`, sampler, `pactl` loop or `pacat` process left; no sink-inputs; iptables unchanged (ACCEPT); capture PCM `closed`. Nothing outside `/tmp` was written; no service, volume or PulseAudio setting was changed.
- HA: at the time of this step no `lg-c3-a` ESPHome entry existed, because HA could not reach the TV. The permanent entry was added later the same day, see [P1 HA integration](P1-ha-integration.md#permanent-entry-direct-connection-2026-11-02-morning). The aborted config flow left no state. Once the owner opens the path, add the entry with the TV's key (stored privately with the owner's files, not in this repository).

## Residual unknowns

- HA's real integration against the TV: the entity, reconnects, HA's own TTS pacing, and whether HA's wake word stage has a run limit like the WebSocket API's 300 s.
- A live person speaking at normal distance (Phase 2), and wake and STT rates (Phase 2 corpus).
- Audible behaviour of the reply path, the latency added by `pacat --latency-msec`, and pops at stream start (Phase 2).
- Memory over days, behaviour in screen-off and standby (Phase 3).
