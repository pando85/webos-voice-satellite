# P1: Ops basics, install and uninstall without residue (roadmap 1.7)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74; TV on, normal use state, LG hands-free off (all capture PCMs `closed` before and after)
- **Date:** 2026-11-02
- **Binary:** `webos-satellite` 0.0.1, static `armv7-unknown-linux-musleabi`, sha256 checked on the TV after install (`49641bde...`, same as the build)
- **Prefix:** `/tmp/webos-satellite` (tmpfs, lost at reboot). Nothing was written to `/media/developer`, `/var` or any persistent path. No autostart, no service or init change, no reboot.
- **Scripts under test:** [`packaging/webos/`](../../../packaging/webos); usage in [development: Install on a TV](../../development.md#install-on-a-tv-non-persistent)
- **Privacy and safety:** no sound (`sink.type = "null"`). The live mic was open and about 180 chunks (182 kB) were streamed to a local test client that discarded them; nobody spoke and nothing was recorded. No ASoC/DAPM sysfs or debugfs node was read. Raw outputs are in the gitignored `devices/private/c3-a/P1-ops/`.

Labels: **verified-on-target**, **hypothesis**.

## Procedure

1. Snapshot (`before`): `find /tmp -xdev | sort`, `ps -eo args= | sort`, `pactl list short sink-inputs`, `netstat -ltn`, and `find /media/developer /var/lib/webosbrew -xdev -maxdepth 3` (the persistent places a mistake would most likely touch).
2. `make tv-install-tmp` (stages scripts, binary, config and key in `/tmp/webos-satellite-stage`, runs `install.sh`, removes the staging directory). Config: `mode = "wake"`, `[mic] source = "alsa"`, `[sink] type = "null"`, `level = "debug"` (the worst case for leaks), Noise key from a private file.
3. `start.sh`, `status.sh`, a client on the PC that connects with the Noise key, subscribes to the voice assistant and sends canary strings in a pipeline run (transcript, reply text, TTS URL with a token, error message), `stop.sh`, `status.sh`.
4. `start.sh` again and `uninstall.sh` while the daemon runs (the uninstall must stop it).
5. Snapshot (`after`) and `diff`.

## Results

| Check | Result | Label |
| --- | --- | --- |
| Install | Directories 0700, files 0600 (config, key, marker), scripts 0700; no staging directory left; `--check-config` printed `encryption=noise` and no warning | verified-on-target |
| Start / status | Daemon up in 1 s, listening on TCP 6053, microphone opened (`2 ch, 16000 Hz`) in wake mode; `status.sh` exit 0 with RSS (about 0.8 to 1.0 MB) | verified-on-target |
| Traffic | Three Noise connections from the test client, four pipeline start requests, audio streamed while subscribed; the run with the canary events ended with the expected `error=canary-code` latency line | verified-on-target |
| Stop | `stop.sh`: SIGTERM, process gone within the wait, `capture released (chunks sent 359, dropped 0, xruns 0)`, pidfiles removed, `status.sh` exit 3 | verified-on-target |
| Uninstall while running | Daemon stopped first, `/tmp/webos-satellite` gone, no `webos-satellite`, `rotate.sh` or `pacat` process, PCM `closed` | verified-on-target |
| Before/after `/tmp` listing | **Identical** (734 entries before and after) | verified-on-target |
| Before/after listening sockets, sink-inputs, persistent paths | Identical (24 sockets; no sink-inputs; 16 persistent entries) | verified-on-target |
| Before/after process list | Identical except names of kernel worker threads (`kworker/...`), which change with kernel activity, for example `kworker/0:0-events_power_efficient` vs `kworker/0:0-events`, and one extra `kworker/0:1H` | verified-on-target |
| SIGKILL fallback of `stop.sh` | Not exercised on the TV (the daemon always stopped on SIGTERM). The code path warns that a killed daemon cannot clean up its `pacat` child | hypothesis |
| Persistent prefix (`--persistent`) | Not used, not tested on the TV (needs the owner's authorization). The refusal without the flag was tested on the PC | not done |

Script safety was also exercised on the PC (a throwaway prefix): `install.sh` refused an empty prefix, `/`, `/tmp`, `/tmp/../etc`, a relative path and a non-volatile path without `--persistent`; `uninstall.sh` refused `/` and an existing directory without the marker file and left it untouched.

## Log leak review

Method: read every `log::` and `println!`/`eprintln!` statement in `crates/*` (about 150), then run the daemon at `debug` with canary strings. Result of the run: the log (99 lines) contained the transcript and the error message **only on `DEBUG` lines**, as the policy allows; no line contained the Noise key, the token or the URL canaries, or any audio.

Findings and fixes:

| # | Finding | Severity | Fix |
| --- | --- | --- | --- |
| 1 | `log::debug!("esphome-api: event {ev:?}")` printed the whole `AppEvent` with the derived `Debug`: **TTS audio bytes**, TTS and announcement URLs with their tokens, transcripts and reply text | Real leak at `debug` (audio and tokens must never be logged) | `AppEvent` and `AppCommand` got a hand-written `Debug` that prints names and sizes only; unit test `debug_output_never_contains_payloads` |
| 2 | HA's error `message` was logged at `info`/`warn` | Low (could echo user text) | Only the error **code** at `info`/`warn`; the message at `debug` |
| 3 | `ureq`'s malformed-URL error text was passed into `PlaybackError::BadUrl`; it can echo the URL (token) and reached a `warn` line | Low | Replaced by a fixed `malformed URL` message |
| 4 | Peer addresses (HA's LAN IP) appear in `info` lines | Accepted | Documented in the policy; logs stay on the tmpfs |
| 5 | URLs in `info`/`warn`/`debug` | Already handled | `redact_url` (scheme, host and first path segment only), with an existing unit test |

Already correct: the Noise key (redacted `Debug` for `Config` and `DeviceSection`, summary prints `encryption=noise`), the conversation id (`<set>`), audio (counts only), transcripts and reply text (`debug` only, `info` carries character counts).

## Open items

- The scripts were run only with the prefix in `/tmp`. Behaviour on a persistent prefix is untested, and no autostart exists (Phase 3, owner approval).
- Log rotation (`rotate.sh`): the threshold logic was checked on the PC (a 5000-byte log with a 1000-byte limit was copied to `.log.1` and truncated within 30 s, and the rotator exited with its daemon). It did not run long enough on the TV to cross the limit.
- The leak test used the debug level and a synthetic client. A longer real-pipeline run (Phase 2) should repeat the canary grep.
