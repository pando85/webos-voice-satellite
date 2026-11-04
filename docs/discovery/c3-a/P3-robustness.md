# P3: Lifecycle and robustness without sound or persistence (roadmap Phase 3)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74; TV on, normal use state, LG hands-free off (capture PCM `closed` before each start); uptime about 2.5 h at the start (the TV had been restarted since the D2 oops; not by an agent)
- **Date:** 2026-11-02
- **Binary:** `webos-satellite` 0.0.1, static `armv7-unknown-linux-musleabi`, built from the code of commit 7d0ee12 (soak and reconnection tests; earlier builds of the same branch for the first kill-switch runs); checksummed on the TV after each install. Installed with `make tv-install-tmp` under `/tmp/webos-satellite`, run with `start.sh --supervise`.
- **Config:** `mode = "wake"`, `[mic] source = "alsa"`, `[sink] type = "null"`, `level = "info"`, all `[safety]` and `[limits]` defaults (`validated_firmware = ["03.30.74"]`, kill switch `/tmp/webos-satellite.disable`, nice 10, 256 open files, 64 MiB RSS ceiling, capture stall 5 s, loop stall 30 s).
- **HA side:** HA still cannot open connections to the TV ([P1 HA integration](P1-ha-integration.md#network-check)). `tools/esphome-fake-ha/ha_bridge.py` on the dev PC stood in for HA's ESPHome integration (HA's own client library, HA's real "Kiwi" pipeline over the WebSocket API, wake word stage only). Mute cycles, state watching and rapid reconnects used `tools/lifecycle/lifecycle_client.py` (same library, no pipeline).
- **Related:** [autostart design](../../design/autostart.md), [D5 plan](D5-plan.md), [P1 TV live](P1-tv-live.md), [P1 ops](P1-ops.md), [development: running the satellite](../../development.md#running-the-satellite)
- **Privacy and safety:** no sound (null sink). The live mic was open and streamed to HA's wake word stage only; nobody spoke, nothing was recorded, and no wake word fired. No ASoC/DAPM sysfs or debugfs node was read. Nothing outside `/tmp` was written; no reboot, no power-state change, no iptables change, no LG process touched. Raw outputs are in the gitignored `devices/private/c3-a/P3/`.

Labels: **verified-on-target**, **verified-on-host**, **hypothesis**, **not done**.

## Kill switch, mute and the PCM (privacy gate F7)

Before this step HA's mute switch only stopped streaming; the PCM stayed open. Now the capture thread closes the PCM whenever the microphone is not wanted (HA mute, kill-switch file, firmware guard) and reopens it when it is. The PCM state was read from `/proc/asound/LGSIC/pcm0c/sub0/status` by a polling loop ([`tools/lifecycle/tv/`](../../../tools/lifecycle/tv)).

| Check | Result | Label |
| --- | --- | --- |
| Kill-switch file created → PCM `closed` | 10 cycles: **30 to 70 ms** (median 35 ms), timed on the TV from the `touch` (10 ms clock ticks plus a 5 ms polling step). Design bound: one 250 ms check interval plus one 16 ms read | verified-on-target |
| Kill-switch file removed → PCM `RUNNING` again | 130 to 220 ms (the daemon's 250 ms poll, open, start) | verified-on-target |
| Kill switch → HA sees the mute switch on | HA's client received `muted=true` about 60 ms after the capture thread released the PCM (PC clock corrected by the measured offset to the TV clock) | verified-on-target |
| HA unmute while the kill switch exists | Refused: HA saw `false` then `true` 3 ms later; PCM stayed `closed`; log `unmute refused: kill switch is active` | verified-on-target |
| HA mute switch → PCM `closed` | 10 cycles: PCM `closed` **3 to 18 ms** after the satellite logged the mute (polling-loop clock calibrated to the TV's wall clock, about 10 to 20 ms accuracy). The satellite logged its own release 1 to 16 ms after the mute. HA's command to state confirmation: 1 to 3 ms round trip on the LAN | verified-on-target |
| HA unmute → PCM `RUNNING` | 27 to 45 ms after the satellite logged the unmute | verified-on-target |
| Kill switch at startup, mute remembered across restarts, re-arm of the wake word after the kill switch or an unmute | `tools/lifecycle/local_check.py` (L1 to L8) against the real binary with a file mic | verified-on-host |
| API connection while the mic is off | Kept (decision): HA keeps showing the device and the forced mute state; no audio flows | verified-on-target |

## Firmware guard

> Note (2026-11-04): this is a dated finding about the old hard gate, which is now the opt-in strict mode (`safety.firmware_policy = "strict"`). The default is a soft policy with self-checks ([design](../../design/firmware-policy-and-reports.md), [ADR 0006](../../decisions/0006-soft-firmware-policy-and-compatibility-reports.md)); the new behaviour has not been run on a TV yet.

| Check | Result | Label |
| --- | --- | --- |
| Read-only source | `/var/run/nyx/os_info.json` (tmpfs, written by nyx at boot) has `"webos_manufacturing_version": "03.30.74"`, the same value as `getSystemInfo`'s `firmwareVersion` in [D0-D1](D0-D1-runtime.md#firmware-and-software-version); also `webos_release` 8.3.0 and `webos_build_datetime` 20240226022140. `/etc/starfish-release` has only the webOS release, not the LG firmware string | verified-on-target |
| `--check-firmware`, allow-list `["03.30.74"]` | `firmware 03.30.74 (webOS 8.3.0, build 20240226022140): validated`, exit 0 | verified-on-target |
| `--check-firmware`, allow-list `["03.30.99"]` | `... NOT in safety.validated_firmware ...`, exit 1 | verified-on-target |
| Daemon started on a non-validated firmware (allow-list `["03.30.99"]`) | Starts with the API up, logs `ERROR FIRMWARE GUARD`, reports muted, never opens the PCM (`closed`, no `/dev/snd` descriptor), stops cleanly on SIGTERM | verified-on-target |

## Watchdog and supervisor

| Check | Result | Label |
| --- | --- | --- |
| `--test-hang loop` (event loop frozen 5 s after start) | `watchdog: event loop made no progress for 30 s; exiting with 70`, exactly 30 s after the freeze; supervisor logged `exited with status 70 after 35s (1 of 5 within 600s)`, restarted after 1 s | verified-on-target |
| `--test-hang capture` | Exit 71 after 31 s of no capture progress; restarted after 1 s, microphone reopened | verified-on-target |
| SIGSTOP of the daemon for 60 s and 120 s | Not counted as a hang: `watchdog: the process did not run for a while (stopped or suspended); resetting` | verified-on-target |
| Stalled ALSA device (no periods for 5 s) → close and reopen | Code path only (`poll` with a timeout before each read, then reopen with backoff); a stall cannot be provoked safely on c3-a. Expected to matter in standby (D5) | hypothesis |
| Crash-loop guard | Host test: 3 exits within 60 s (`WEBOS_SAT_MAX_CRASHES=3`) with 1 s then 2 s backoff, then `crashloop` written and no further start, also refused by `start.sh --supervise`. On the TV only single restarts were exercised | verified-on-host |
| `stop.sh` with the supervisor | Supervisor stopped first, no restart afterwards; PCM `closed` | verified-on-target |

## Resource limits

| Check | Result | Label |
| --- | --- | --- |
| Niceness | `/proc/<pid>/stat` field 19 = 10 | verified-on-target |
| Open files | `Max open files 256 524288` (soft lowered from the inherited 524288; hard unchanged). The daemon uses 11 descriptors idle and 12 with HA connected | verified-on-target |
| Memory | VmSize 8 MB, VmRSS 0.85 MB at start; RSS ceiling 64 MiB (exit 72) is 50 times the observed use. `RLIMIT_AS` not set (inherited by `pacat`) | verified-on-target |
| Threads | 3 (event loop, capture, watchdog); 4 briefly while a WAV sink writes | verified-on-target |
| CPU | 1.4 % of one core averaged over 20 minutes of mixed tests (wake-mode streaming most of the time) | verified-on-target |
| TV headroom at the time | `MemAvailable` 478 MB (lower than the 750 MB in D0-D1), 3 of 4 cores online (`0-2`), load average about 12.7 from LG processes | verified-on-target |

Chosen limits versus c3-a: the daemon needs about 1 MB resident and 1.5 % of a core; 64 MiB caps a runaway at under 15 % of the memory available today and still leaves room for decoding a long announcement. Niceness 10 yields to LG's processes; the 512 ms ALSA buffer absorbs scheduling delays (one overrun was seen only under a deliberate SIGSTOP). 256 descriptors is about 20 times normal use and stops a descriptor leak long before the system limit.

## Reconnection (HA side: the bridge, HA's client library)

| Test | Satellite behaviour | Recovery | Label |
| --- | --- | --- | --- |
| T1 bridge killed (SIGKILL) and restarted 5 s later | `HA connection ... closed: connection closed by peer` at once; streaming stopped | New connection re-armed the wake word 3 ms after the voice subscription | verified-on-target |
| T2 bridge frozen (SIGSTOP) 60 s, then SIGCONT | No disconnect: the PC's TCP receive buffer absorbed 60 s of audio (about 2 MB) and HA's 60 s keepalive window was not exceeded; after SIGCONT the bridge drained the backlog and the session continued to its normal end | Not needed | verified-on-target |
| T3 satellite frozen (SIGSTOP) 60 s | Connection survived (HA's client allows 90 s for a ping reply); watchdog reset, no trip | Not needed | verified-on-target |
| T3x satellite frozen 120 s | HA's client gave up (`Ping response not received after 90.0 seconds`); on SIGCONT the satellite saw `connection closed by peer` | Re-armed 5 ms after the next connection | verified-on-target |
| T4 rapid reconnect loop, 20 connects back to back | 20 of 20 succeeded; wake word start request 16 to 73 ms (median 20 ms) after the connect began. Descriptors 11 before and after, threads 3, RSS unchanged (1124 kB); 21 sockets in `TIME_WAIT` (kernel only) | | verified-on-target |
| T5 network partition 180 s (a PC-side relay stopped moving bytes both ways without closing; the TV's network was not touched) | Satellite closed the connection **20 s** after the partition (`closed: timeout`: buffers filled, then the 10 s write timeout); HA's client gave up after 90 s | After the heal, the relay saw a reset for the old connection; a new connection re-armed at once | verified-on-target |
| HA restart, Wi-Fi loss on the TV, TV resume | Not done: HA cannot reach the TV, the TV's network must not be changed, and power states are D5 | | not done |

## Soak (35 minutes, periodic reconnects)

13 cycles of: a 150 s bridge session in wake mode (live mic streamed to HA's wake word stage), 5 rapid reconnects, one HA mute/unmute, one kill-switch cycle. The same daemon process (no restart) had already run the tests above for 20 minutes. Sampled every 10 s on the TV.

| Measure | Result | Label |
| --- | --- | --- |
| Reconnects | 13 bridge sessions and 65 rapid reconnects, **0 failures**; wake word re-armed every time (16 to 76 ms after the connect began) | verified-on-target |
| Mute and kill switch | 26 mute/unmute commands all confirmed; 13 kill-switch cycles; every one logged `PCM released` | verified-on-target |
| Descriptors | 10 to 12 (12 while HA is connected, 10 to 11 between connections); no growth | verified-on-target |
| Threads | 3 throughout | verified-on-target |
| CPU | 2.0 % of one core averaged over the soak | verified-on-target |
| Watchdog, supervisor | No trip, no restart, no `ERROR` line; 0 xruns during the soak | verified-on-target |
| Capture statistics at shutdown (whole process life, about 57 minutes) | 84 184 chunks sent, 248 dropped (most likely during the T5 partition, when writes were blocked for 10 s), 0 xruns | verified-on-target |
| RSS | 1.7 MB for the first 4 cycles, then a **single rise to 9.5 MB** during the 5th bridge session (a steady 64 kB per second for 2 minutes, ending exactly when that session ended), then flat at 9.46 to 9.65 MB for the remaining 8 cycles (22 minutes). At the end: VmRSS 9648 kB, RssAnon 8600 kB, almost all in one anonymous mapping (the heap). Nothing unusual in the satellite log or the bridge log for that session | verified-on-target (observation) |
| Cause of the RSS rise | Not reproduced exactly (see the PC investigation below), but a mechanism with the same signature was found and fixed: musl's `malloc` keeps growing under repeated TTS replies while the Rust heap stays flat. The TV step itself happened during a wake-mode session with no logged reply, so it stays **unproven for that event**; the fixed build was re-soaked with 55 streamed replies: see [re-soak](#re-soak-on-the-tv-with-the-dlmalloc-build-2026-11-02) | hypothesis (mechanism verified-on-host); fix verified-on-target |

### RSS investigation on the PC (2026-11-02, host only, no TV, no HA)

Method: the real daemon with a synthetic file microphone and a fake HA (aioesphomeapi 46.6.0, Noise), VmRSS, VmHWM and RssAnon from `/proc/<pid>/status` every 3 to 30 s. Two builds of the same commit: host glibc, and static `x86_64-unknown-linux-musl` (zigbuild), which has the same malloc (musl mallocng) as the armv7 TV build. A scratch-only counting global allocator (not committed) gave the Rust-visible live bytes. Labels below are `verified-on-host`; none is a TV measurement.

| Scenario | glibc | musl malloc (the TV's allocator) |
| --- | --- | --- |
| 160 s wake session, mic streamed, HA draining | RssAnon 400 kB, flat | 620 kB, flat after the first 10 s |
| Peer stalled 60 s (the satellite's event loop never saw backpressure; kernel buffers absorbed 2 MB), then released | flat | flat (620 kB), 0 chunks dropped |
| 150 reconnects | not run | +570 kB over the first 75, then flat (1020 kB) |
| A 300 kB (9.6 s) TTS reply every 10 s for 420 s, null sink | RssAnon 404 kB, flat | **0.4 MB to 21.6 MB, linear, about 52 kB/s, about 1.7 bytes of RSS per reply byte, no plateau** (RSS 2.8 to 24.0 MB) |
| Same, Rust live bytes (counting allocator, musl) | n/a | 87 to 94 kB peak, flat |
| Same, build with `dlmalloc` (the fix) | n/a | RssAnon 192 kB, flat for 36 replies and 300 s |

Findings:

- **No unbounded Rust-side queue in the microphone path.** Capture sends with `try_send` into a 64-chunk channel and drops (and counts) when full; a 60 s stall of the peer neither grew RSS nor dropped chunks on loopback. The server's write buffer is reused per frame.
- **The growth is the musl allocator, not a Rust leak.** With TTS replies the Rust live heap stays under 100 kB while RssAnon grows without bound; a single anonymous mapping (the heap) holds it, as on the TV (RssAnon 8600 kB in one mapping). glibc and `dlmalloc` on the same code stay flat. The suspect traffic is each reply's burst of 1 KiB chunks (events, queue blocks, decode buffers); the exact mallocng behaviour was not dissected.
- **Why it matters beyond the soak**: at that rate a 5 s reply costs about 270 kB, so the 64 MiB RSS ceiling (exit 72, supervisor restart) would be reached after a few hundred replies.
- **Does this explain the TV step (a steady 64 kB/s for 2 minutes, then flat)?** It has the same signature (a linear rise in one heap mapping that stops when the audio stops, never returned to the OS), but on the PC the rise follows TTS audio, while the soak's session is described as wake-mode streaming with nothing unusual in the logs. A reply during that session is possible but not recorded, so this is not proven. On the PC a mic-only session on musl does not grow.

Changes made (host-verified; run on the TV in the [re-soak below](#re-soak-on-the-tv-with-the-dlmalloc-build-2026-11-02)):

- `dlmalloc` (pure Rust, `global` feature) is the global allocator on musl targets only (`crates/satellite/Cargo.toml`); host builds keep the system allocator. armv7 binary +7 kB; the armv7 build under `qemu-arm-static` passes the daemon end-to-end harness (17 checks).
- The player's TTS queue is byte-bounded (2 MiB, about 65 s of 16 kHz mono audio): beyond that audio is dropped with a warning and a count instead of queueing without limit if the sink stalls (a pacat that never reads). Tested with a fake pacat.
- The microphone chunk buffer is allocated at its final size (the old `flat_map` collect regrew it eight times per chunk).
- The watchdog thread now logs `memory: resident size N KiB (last reported M KiB)` at info each time RSS grows 2 MiB beyond the last report, and the size at debug every 10 s, so a rise shows in the log with a timestamp.

Closing this on the target was done in the re-soak below.

The rise stays far below the 64 MiB ceiling, which would turn a real leak into a restart by the supervisor (exit 72) rather than memory pressure on the TV.

## Re-soak on the TV with the dlmalloc build (2026-11-02)

Question: does the musl `malloc` growth under streamed TTS replies (about 1.7 bytes of RSS per reply byte on the PC) disappear on c3-a with the `dlmalloc` build?

- **Binary:** `webos-satellite` 0.0.1 from commit 062b1c9, static armv7 musl, sha256 `7f35999dce17cc492e21176f8e86cc814d08588c81881e605170137d4d477cbf`, `make tv-install-tmp` under `/tmp/webos-satellite`, started with `start.sh` (no supervisor, so a restart could not hide a growth).
- **Config:** `mode = "wake"`, `[mic] source = "alsa"` (live mic, streamed to the bridge), `[sink] type = "pacat"` with `gain = 0.0`, `level = "info"`, defaults for `[safety]` and `[limits]`. Silence is guaranteed in the code: `gain_s16le` (`crates/satellite/src/player.rs`) applies `Pcm::apply_gain(0.0)` to every sample before they reach `pacat`, and every playback logged `0 non-zero samples`. `pacat` really ran, so the PulseAudio path was exercised (not the null sink). LG hands-free was off (PCM `closed` before the start); no `MicBusy`.
- **Replies:** `tools/esphome-fake-ha/ha_bridge.py` with the new `--synthetic-reply-every 40` soak aid (see below), against HA's real "Kiwi" pipeline: each reply is a real Piper TTS-only run of one fixed Spanish test sentence (8072 ms, 258 300 bytes of 16 kHz mono PCM), fetched from HA's `tts_proxy`, converted by ffmpeg and streamed between `TTS_STREAM_START` and `TTS_STREAM_END` in 512-sample chunks, one reply about every 50 s. This is a **labelled substitute**: no wake word was detected and no speech was involved (HA's conversation agents are still broken); the satellite saw the same event sequence and audio stream as a spoken reply. Nobody spoke; the mic audio went to the bridge, which dropped it.
- **Sampling:** every 30 s over `ssh` from `/proc/<pid>/status` (VmRSS, VmHWM, RssAnon, Threads) and `/proc/<pid>/fd`, plus the capture PCM state; 118 samples over 58.7 minutes. Raw outputs are in the gitignored `devices/private/c3-a/P3/resoak/`.

| Measure | Result | Label |
| --- | --- | --- |
| Replies played | **55** (`playback 55 done` is the last line; 35 in the first 29 minutes, 20 in a second phase of 12.6 minutes), 444 s of audio, 14.2 MB of reply PCM. The 20 retained daemon log lines (the log rotates at 256 KiB, see below) all say `0 non-zero samples`; the bridge logged 55 `TTS streamed` and a device media player `IDLE` after each | verified-on-target |
| RSS | **716 kB** before the first HA connection, then **976 kB for the whole run** (min 716, max 976, end 976); VmHWM 976 kB (= max), RssAnon 72 kB then 104 kB. One step of 260 kB when HA first connected, **no change over the 55 replies**: slope 0 kB per reply (a regression over all 118 samples gives 13 kB per hour, entirely the first step). The old build would have grown by about 24 MB over the same reply bytes (1.7 bytes per byte), ending at the 64 MiB ceiling after a few hundred replies | verified-on-target |
| `memory:` log lines | None: RSS never grew 2 MiB beyond the last report | verified-on-target |
| Threads, descriptors | 3 threads throughout; 11 to 13 descriptors (13 while HA and `pacat` are active), no growth | verified-on-target |
| Reconnects | 11 bridge sessions that played replies (7 and 4, each 5 replies, ended by the bridge and reconnected by the next one) and **770 rapid reconnects** with `lifecycle_client.py` (0 failures; see the incident below). No watchdog trip, no restart, no `ERROR` line; the only `WARN` lines in the retained log are `HA disconnected` from the reconnect loops. No `dropped` or queue-full line | verified-on-target |
| PCM after the run | RUNNING during the run (mic live), all capture PCMs `closed` after `stop.sh` | verified-on-target |

Conclusion: the RSS growth under TTS replies is **gone on the target** with the `dlmalloc` build: 55 streamed replies and 770 reconnects left RSS at 976 kB, 0 kB per reply. This confirms the host finding (musl `malloc` was the cause of that growth mechanism). The one-off 8 MB rise seen in the earlier soak was not reproduced either, but it was never explained for that event; the data say only that this build holds under the traffic that most plausibly caused it.

Incident (harness, not the satellite): the HA refresh token stopped working after 29 minutes (`invalid_grant`), so the bridge's access token expired (`auth_invalid`). The driving loop then spun without delay: about 247 bridge attempts failed at the HA login (they never connected to the TV), each followed by 3 `lifecycle_client.py` reconnects, which gave the 770 successful rapid reconnects in about 15 minutes. A new token was minted and a second phase ran 4 more bridge sessions. The reconnect spam rotated the daemon log, so the first 35 replies are counted from the bridge log (35 `TTS streamed` and 35 `run done`), and the log-based count of `playback ... done` in the sampler is not reliable. The satellite was undisturbed (RSS and descriptors flat, HA connection accepted every time).

Tooling change: `ha_bridge.py --synthetic-reply-every SECS --reply-on-intent-failure TEXT` (soak aid): the bridge opens no HA wake word run and ignores mic audio; after SECS it plays WAKE_WORD_END, STT, INTENT events and a real TTS-only reply of TEXT, labelled `synthetic: true` in the run summary. It tests the satellite's reply path, not HA's pipeline.

Rollback and cleanup of this session: `stop.sh`, `uninstall.sh` removed `/tmp/webos-satellite`; no `webos-satellite`, `supervise`, `rotate` or `pacat` process left; the `/tmp` listing is identical to the one taken before the install; all capture PCMs `closed`. Nothing else was written on the TV; no reboot, no power-state change, no iptables change, no LG process touched, no ASoC/DAPM sysfs or debugfs read. HA: no configuration change; the short-lived token was revoked and its files shredded.

## Rollback and leftovers

- `uninstall.sh` stopped the supervisor and the daemon and removed `/tmp/webos-satellite`; the measurement loops, their outputs, the kill-switch and mute files were deleted.
- The `/tmp` listing after cleanup is identical to the one taken before the session (except the listing files themselves, also deleted). No `webos-satellite`, `supervise.sh`, `rotate.sh`, sampler or `pacat` process; all 10 capture PCMs `closed`; no sink-inputs; `/var/lib/webosbrew/init.d` still empty; no failsafe flag.
- HA: no configuration was changed; the short-lived token was revoked and its files deleted.

## Residual unknowns

- Behaviour in screen-off, standby and Always Ready, and on resume: [D5 plan](D5-plan.md) (owner).
- The capture stall reopen on a real stall (expected in standby).
- HA's own integration against the TV (network block), including HA restarts and its reconnect backoff.
- Multi-day memory and descriptor behaviour (Phase 3 exit criterion); this run covered about 1 hour. The single 8 MB RSS rise in the first soak was not reproduced; the musl allocator growth is fixed on the target (re-soak above: 55 replies, RSS flat at 976 kB). The re-soak used synthetic wake and speech (a real HA TTS reply, no spoken input) and about an hour; a spoken reply through HA's conversation agent was not tested.
- Autostart after a real reboot (needs the owner's approval: [autostart design](../../design/autostart.md)).
