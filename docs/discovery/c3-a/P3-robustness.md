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
| Cause of the RSS rise | Unknown. Not a steady leak at this rate (it did not repeat in 8 identical sessions). Hypothesis: a backlog buffered in user space during a slow stretch of the connection, freed afterwards but kept by the allocator. Needs a reproduction with heap instrumentation and the drop counter logged per session | hypothesis |

The rise stays far below the 64 MiB ceiling, which would turn a real leak into a restart by the supervisor (exit 72) rather than memory pressure on the TV.

## Rollback and leftovers

- `uninstall.sh` stopped the supervisor and the daemon and removed `/tmp/webos-satellite`; the measurement loops, their outputs, the kill-switch and mute files were deleted.
- The `/tmp` listing after cleanup is identical to the one taken before the session (except the listing files themselves, also deleted). No `webos-satellite`, `supervise.sh`, `rotate.sh`, sampler or `pacat` process; all 10 capture PCMs `closed`; no sink-inputs; `/var/lib/webosbrew/init.d` still empty; no failsafe flag.
- HA: no configuration was changed; the short-lived token was revoked and its files deleted.

## Residual unknowns

- Behaviour in screen-off, standby and Always Ready, and on resume: [D5 plan](D5-plan.md) (owner).
- The capture stall reopen on a real stall (expected in standby).
- HA's own integration against the TV (network block), including HA restarts and its reconnect backoff.
- Multi-day memory and descriptor behaviour (Phase 3 exit criterion); this run covered about 1 hour. The single 8 MB RSS rise in the soak is unexplained.
- Autostart after a real reboot (needs the owner's approval: [autostart design](../../design/autostart.md)).
