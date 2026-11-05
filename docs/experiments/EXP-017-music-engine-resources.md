# EXP-017: Music engine resources on c3-a (the music plan's "EXP-013")

**Status:** TARGET_VERIFIED for CPU, memory, underruns and mic dropouts (Active, 5 and 10 minute runs); standby entry, suspend and HA-disconnect cases NOT TESTED
**Date:** 2026-11-05 (11:03 to 11:44 CET)
**Device alias / model / firmware / webOS / architecture:** c3-a, LG C3 42 inch (OLED42C34LA), firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland, daemon `webos-satellite 0.4.1 (aarch64-unknown-linux-musl)` (package 0.4.1, commit `a904f08`, the permanent install), Home Assistant 2026.10.4
**Power and playback state:** `Active`, screen on (the owner's PC display, in use). `[music] enabled = true` for the run, everything else the installed config. The satellite volume was 0 % for every run in this file (no sound; PulseAudio still ran the stream).
**Related research / requirements / issue:** [music playback plan](../design/music-playback.md) (section G "EXP-013", milestone M6, gate G2, risks R3 to R6, R9), [ADR 0010](../decisions/0010-music-playback.md), [P4 permanent install](../discovery/c3-a/P4-permanent-install.md)
**Risk:** configuration change (`[music] enabled`, one daemon restart, restored), no audio recorded, no TV setting touched
**Approval needed / obtained:** owner, 2026-11-05 11:00 CET: "you can run tests now" (c3-a only, sound allowed but these runs were silent, no standby entry, no 3 hour soak)

**ID note.** The plan reserved the number EXP-013 for this experiment. EXP-013 was taken the same day by the Home Assistant fresh-setup test ([EXP-013](EXP-013-ha-fresh-setup.md)), so this one is filed as EXP-017. Plan text that says "EXP-013" means this file.

## Question and falsifiable hypothesis

Does the music path (fetch thread, decode thread, the engine task on the daemon's single tokio thread, a second `pacat`, PulseAudio) fit the proposed thresholds on c3-a: decode thread at most 5 % of one core; PulseAudio plus `pacat` #2 at most 10 %; daemon RSS at most 8 MiB; 0 underruns in 10 minutes at `latency_msec = 100`, `write_ahead_ms = 60`; no microphone dropouts?

## Prerequisites and safe baseline

Baseline read before any change (2026-11-05 11:01): daemon 0.4.1 running (pid 14577), RSS 1868 kB, `[music]` absent (flags 5124), reply volume 40, remote power-off `normal`, Always Ready On, LG hands-free off, `voice.json` `music: {"enabled": false}`, config md5 `cd688f93...`, `tainted` 4097, load average about 12 to 14 (the TV's own software). No `pacat` running. The original config was copied to the PC and restored byte for byte at the end (md5 equal).

## Commands / steps

1. Append `[music]` and `enabled = true` after `[pairing]` (the packaged `ctl.sh` `cfg_value` is not section aware), `--check-config` (printed `music=[on format=flac 48000Hz/2ch announce=true ... latency_msec=100 write_ahead_ms=60 ...]`), `ctl.sh restart`. HA reconnected 4 s later (`media_player` features 5124 to 1201669).
2. HA volume 0 on the satellite's `media_player` (the stored reply volume goes to 0 and was restored to 40 at the end).
3. A read-only sampler (a shell script copied to the TV's `/tmp` and deleted afterwards, reading `/proc` only) every 5 s: ticks of the daemon, its main thread, the threads named `music-decode` and `music-fetch`, PulseAudio and the `pacat` whose command line has `--stream-name=music`; RSS of the three processes; `voice.json` music state and underruns; `wlan0` receive bytes; load average. `CLK_TCK` = 100, so ticks per second equal percent of one core; the TV has 4 cores.
4. **Run A:** Radio Browser "Classic Vinyl HD" through HA's ffmpeg proxy (FLAC 48000 Hz 2 ch, endless), 305 s, a 60 s pause (`media_pause`, then `media_play`) from t = 120 s.
5. **Run B:** an endless 44.1 kHz stereo FLAC stream from a stand-in for Music Assistant's flow mode (`fake_proxy.py --mode ma-flow` on the PC, generated tone plus noise, `--realtime 4`), played with `bypass_proxy`, 330 s, a 60 s pause from t = 120 s. A first attempt at `--realtime 1` showed 1 underrun after the stream started (the generator produced exactly at real time, no headroom); the later runs used 4.
6. **Formats:** 60 s each of an endless 44.1 kHz WAV, an endless 48 kHz FLAC (both from the PC stand-in) and a 128 kbit/s 44.1 kHz stereo MP3 file (served by `python3 -m http.server` on the PC).
7. **Idle baseline:** 60 s with music enabled and nothing playing.
8. **Pause timeout:** radio paused for 10 minutes (D8, `pause_timeout_s = 600`).
9. `dmesg` tail and `tainted` after the runs; the daemon's capture counters from the log.

## Expected evidence and failure interpretation

Thresholds from plan section G. Amber (5 to 15 % decode CPU) means try `opt-level = 3` for the decoder crates, then WAV; underruns mean raise `latency_msec`; any mic dropout or an open `pacat` that blocks standby stops the feature.

## Observations and redacted evidence links

Percent of one core (4 cores on the TV), mean over the window; "play" windows exclude the first and last 5 s of the run.

| Run (state) | Daemon total | Daemon main thread | `music-decode` | `music-fetch` | PulseAudio | `pacat` #2 | Daemon RSS |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Idle, music enabled, nothing playing (60 s) | 1.82 | 1.17 | n/a | n/a | 0.02 | n/a | 6.0 to 6.3 MiB (see below) |
| A: HA-proxy FLAC 48 kHz stereo, playing (t 0 to 115 s) | 4.18 | 2.63 | **0.85** | 0.11 | 3.89 | 1.26 | 3.8 to 4.0 MiB |
| A: same, paused (t 125 to 175 s, `pacat` stays open) | 1.90 | 1.24 | 0.02 | 0.00 | 1.26 | 0.04 | 3.9 MiB |
| A: same, playing again (t 185 to 305 s) | 4.16 | 2.61 | 0.83 | 0.11 | 3.82 | 1.23 | 3.8 to 4.1 MiB |
| B: external FLAC 44.1 kHz stereo, playing (t 0 to 115 s) | 4.17 | 2.59 | 0.88 | 0.11 | 3.83 | 1.22 | 3.8 to 4.0 MiB |
| B: same, paused (t 125 to 175 s) | 1.96 | 1.26 | 0.00 | 0.00 | 1.18 | 0.00 | 4.0 MiB |
| B: same, playing again (t 185 to 330 s) | 4.24 | 2.63 | 0.86 | 0.11 | 3.81 | 1.20 | 3.8 to 4.0 MiB |
| WAV 44.1 kHz stereo (no decode), 60 s | 3.78 | 2.55 | 0.52 | 0.15 | 3.68 | 1.15 | 6.1 to 6.2 MiB |
| FLAC 48 kHz stereo, external, 60 s | 4.13 | 2.55 | 0.92 | 0.13 | 3.75 | 1.22 | 6.1 MiB |
| MP3 128 kbit/s 44.1 kHz stereo, 60 s | 4.47 | 2.57 | **1.33** | 0.02 | 3.70 | 1.18 | 6.1 to 6.2 MiB |

- **Decode thread:** 0.5 % (WAV: only the chunking) to 1.3 % (MP3) of one core; FLAC 0.83 to 0.92 %. Threshold 5 %: **pass with a factor of 5**. `opt-level = 3` for the decoder is not needed.
- **PulseAudio plus `pacat` #2:** 3.8 to 3.9 % plus 1.2 to 1.3 % = **5.0 to 5.2 %** while playing (threshold 10 %). A 44.1 kHz stream costs the same as a 48 kHz one (3.83 against 3.89 %), so the resampling is not visible. **While paused with `pacat` #2 open, PulseAudio still used 1.2 % and `pacat` 0.0 %**; with no `pacat` it used 0.02 %. So the music costs about 2.6 points of PulseAudio over an open-but-idle stream and 3.8 over nothing.
- **Daemon:** about 2.3 points over the idle daemon while playing (main thread +1.4, decode +0.9). Everything is on the main thread plus the two named threads (`music-fetch`, `music-decode`); the engine is a task on the main thread.
- **Daemon RSS** (`VmRSS`, 1 MiB = 1024 kB): 3.8 to 4.1 MiB while playing in runs A and B (1.8 MiB before the first track, 3.1 MiB after it). **After the first long announcement (below) it sat at 6.0 to 6.7 MiB** and stayed there (the heap was not returned), so the 8 MiB threshold is **met with about 1.7 MiB of margin only**. `pacat` #2: 1612 kB (1.6 MiB). Limit 64 MiB.
- **Reply-path memory (not the music engine, found on the way):** an 18 s TTS announcement with no music made the daemon RSS go from 6.2 to **17.6 MiB** (6384 to 17996 kB) for the whole playback and back to 6.7 MiB after (silent, volume 0); two overlapping long announcements with the radio playing peaked at **28.8 MiB** (29492 kB; VmHWM 30076 kB = 29.4 MiB). About 0.65 MiB per second of reply audio is held (the reply path decodes the whole URL first); extrapolated, a reply of about 90 s would exceed the 64 MiB ceiling and the watchdog would exit with code 72 (hypothesis, not run). See the bug list in the design M6 status.
- **Underruns:** `voice.json` `music.underruns` was 0 at the start, **3 by 10:06 and 4 by 10:07**, and never changed again in the following 5 + 5.5 + 3 + 1 minutes of measured playback and the audible runs of EXP-014. The first 4 happened in the first three minutes of the session: the ENQUEUE refusal, two overlapping announcements with HA's proxy killing the radio conversation (stream ended mid-way), and the first external stream whose generator ran at exactly 1x real time. **In the two 5-minute runs (A and B), the three 60 s format runs and the idle window: 0 underruns** at `latency_msec = 100`, `write_ahead_ms = 60`.
- **Microphone:** at the restart that ended the session the daemon logged `capture released (chunks sent 75682, dropped 0, xruns 0)` for the whole 41 minute music-enabled period (this includes every run in this file and in EXP-014 and EXP-016). **0 dropped chunks, 0 xruns.** HA's microphone self-check at both restarts passed (peak -28 dBFS with music on, -47 dBFS at the end).
- **Network:** 90 to 150 KB/s received over `wlan0` while streaming (the whole TV's counter): about 0.7 to 1.2 Mbit/s for FLAC.
- **Pause:** with the HA proxy the TV kept receiving data for about 45 s of a 60 s pause (`wlan0` 85 to 105 KB/s until about 4.5 MB had arrived), then 1 KB/s; this is the kernel's TCP receive buffer filling while the satellite stopped reading (the daemon's rings are bounded). The stand-in server backed up faster (3 KB/s from the start). A 60 s pause resumed correctly on both the HA proxy stream and the external one. **A 10 minute pause (radio, HA proxy): at exactly 600.0 s the daemon logged `track 6 ended: pause_timeout`, reported `idle` to HA (HA state `idle` at 10:30:07 UTC), `last_end` = `pause_timeout`, no `pacat` left** (D8 as designed).
- **Eviction mid-stream:** HA's proxy killed the radio conversation when a third conversion was created (see EXP-016); the satellite played on for about 12 s from its buffers, then logged `track 3 ended: ended` and went `idle` once the announcement ended, with no retry and no orphan `pacat` (0 `pacat` processes after).
- **Kernel and TV:** `dmesg` shows only the Wi-Fi driver's periodic P2P warnings and a CPU hotplug line of the TV itself; no oops, no ASoC or sound warning; `tainted` 4097 (as before). The TV's load average stayed 12.2 to 12.9 (baseline 12 to 14). The foreground app, input, screen, mute and TV volume were not touched and read the same in HA at the end (TV volume 89, mute off, input `HDMI 1`, screen on).
- **`pacat` #2 while paused:** the sink input is `Corked: no` and the stream stays open (design M4: the output stays open while paused).
- **Own-address rule in this build.** The 0.4.1 package was built before `9096eda` (own addresses from `getifaddrs`), so this daemon used the UDP bind probe. On c3-a (`ip_nonlocal_bind` = 0, read) it did **not** fail closed: replies, announcements and music from HA's LAN address and from a PC on the LAN were all fetched.
- **Raw samples** (CSV per run, `/proc` readings) stayed in the session's scratch directory and are not committed.

## Conclusion and confidence

**G2 (resources): PASS for the tested items**, confidence high for CPU (5 runs, the five runs agree within 0.5 point), medium for memory (the 8 MiB margin is thin and depends on what the reply path did before). Open items, **NOT TESTED**: `Active Standby` entry with `pacat` #2 open and then closed (the owner's rule for this run: no power-state change), stream closing mid-way beyond the eviction case, HA disconnect during music (D9), suspend and resume, `Screen Off`, MP3 through HA's proxy (HA's own ffmpeg 8.1.2 rejects `-sample_fmt s16` for `libmp3lame`, so HA's proxy cannot make MP3 with width 2: risk R15 confirmed, MP3 stays unadvertised), the 3 hour soak (EXP-015, not authorized).

## Rollback and cleanup

Playback stopped; the original `config.toml` copied back (md5 `cd688f93...` equal), `ctl.sh restart`; `voice.json` `music: {"enabled": false}`; HA `media_player` features back to 5124, volume 0.4, reply volume file 40; the sampler scripts removed from the TV's `/tmp` (no file of ours left); the three PC-side stand-in servers and the file server stopped.

## Follow-ups / unresolved questions

- Standby entry with `pacat` #2 open (needs the owner's go-ahead for a power-state change).
- The reply-path buffering (about 0.65 MiB per second of reply audio) and whether to stream-decode replies, or at least cap the reply length, before enabling `announce`. **Fixed on the PC on 2026-11-05 (verified-on-host, not yet on c3-a):** replies and announcements are now streamed (see the M5 and M7 notes in the [design](../design/music-playback.md#e-milestones)); on the host a 120 s announcement grows the process by 0.8 MiB (the same as a 30 s one) against 74.6 MiB with the old whole-file path. The on-target re-measure (this file's method, an 18 s and a long announcement) is pending the next deploy.
- A baseline of the daemon's RSS over days with music enabled (heap kept at about 6.2 MiB after the first long reply).
