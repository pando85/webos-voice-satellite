# Development

How to build, test and run the Rust workspace. Status: Phase 1 plus the non-persistent Phase 3 work (kill switch, watchdog, supervisor, firmware guard, resource limits; [P3 robustness](discovery/c3-a/P3-robustness.md)). `webos-satellite` is the working daemon (capture → front-end → ESPHome API → reply playback). For what was verified, see [P1 HA integration](discovery/c3-a/P1-ha-integration.md) and [P1 TV live](discovery/c3-a/P1-tv-live.md); for what comes next, see the [roadmap](project/roadmap.md).

## Layout

| Path | Purpose |
| --- | --- |
| `Cargo.toml` | Workspace, shared `[workspace.dependencies]`, lint config, size-tuned `[profile.release]` |
| `rust-toolchain.toml` | Pinned toolchain (`1.99.1`), `rustfmt`, `clippy`, the two TV cross targets |
| `.cargo/config.toml` | Softfp codegen flags for `armv7-unknown-linux-musleabi` |
| `Makefile` | Entry points listed below |
| `crates/satellite` | Binary `webos-satellite`: the daemon (config, capture thread, player, latency log, microphone gate, firmware guard, watchdog, resource limits), plus `--self-test` and `--check-firmware` |
| `crates/capture` | ALSA capture through raw ioctls (no alsa-lib), file source for tests |
| `crates/dsp` | Mic front-end: channel select, high-pass, gain, S24 to S16 |
| `crates/esphome-api` | ESPHome native API device server and voice state machine ([details](development/esphome-api.md)) |
| `crates/playback` | TTS fetch, decode, resample, `pacat`/WAV/null sinks |
| `config/example.toml` | Commented configuration with all defaults |
| `packaging/webos/` | POSIX sh install, start, stop, status, uninstall and supervisor scripts for a TV ([below](#install-on-a-tv-non-persistent)); `init.d/` holds the **draft** boot hook ([autostart design](design/autostart.md)); `tests/run.sh` tests the scripts on the host |
| `tools/esphome-fake-ha/` | Fake-HA harnesses (`run.sh`) and the HA bridge (`ha_bridge.py`) |
| `tools/lifecycle/` | Microphone-gate check of the real binary (`local_check.py`), an HA-side client for mute cycles, state watching and reconnect loops against a TV (`lifecycle_client.py`), `run.sh` for both host checks |

Keep the root files minimal and stable; add per-crate dependencies in the crate's own `Cargo.toml`, with versions declared once in `[workspace.dependencies]`. Workspace lints forbid `unsafe` code; a crate that genuinely needs it (for example tinyalsa ioctls in `capture`) must opt out locally and say why.

## Target decision

The TV userland is 32-bit ARM EABI5 with the soft-float `e_flags` `0x05000200`, glibc 2.31 and no hard-float or aarch64 loader ([D0-D1](discovery/c3-a/D0-D1-runtime.md#32-bit-vs-64-bit-userland)). Following the [research recommendation](research/esphome-rust-satellite.md#c1-target-triple-linking-and-audio):

- **Default:** static `armv7-unknown-linux-musleabi` with `-C target-feature=-soft-float,+vfp3,+neon` (set in `.cargo/config.toml`). Static linking means the TV's glibc, loader and libraries are not involved.
- **Comparison:** static `aarch64-unknown-linux-musl`.

Both run on c3-a ([P1 runtime smoke](discovery/c3-a/P1-runtime-smoke.md)). Choosing between them is a later decision; the armv7 build stays the default because it matches the userland that every LG binary on the TV uses.

Notes:

- rustc accepts the softfp flags but warns `unstable feature specified for -Ctarget-feature` for `soft-float`, `vfp3` and `neon`. This is expected on stable rustc.
- The flags change code generation of the workspace crates (checked: a `f32` multiply-add compiles to `vmla.f32` with the flags and to `__aeabi_fmul`/`__aeabi_fadd` calls without). The prebuilt `std` stays soft-float; both use the same core-register calling convention, so mixing is call-compatible. The linked binary's `readelf -A` therefore shows `Tag_FP_arch: VFPv3-D16` and no NEON tag.
- `cfg!(target_feature = "neon" | "vfp3")` is always false on stable rustc, so the binary cannot report them; `--self-test` runs a small float loop instead to exercise the FPU at runtime.
- zig prints `argument unused during compilation: '-nostartfiles'` as a linker warning. It is harmless.

## Tools (user-local, no sudo)

```sh
rustup toolchain install          # reads rust-toolchain.toml, including both targets
cargo install cargo-zigbuild --locked --version 0.24.0
pip install --user ziglang==0.16.0   # or any zig 0.16.x on PATH (a venv also works)
```

Versions used for the recorded builds: rustc 1.99.1, cargo-zigbuild 0.24.0, zig 0.16.0. Optional: `qemu-user-static` to run the armv7 binary on a PC (`qemu-arm-static target/armv7-unknown-linux-musleabi/release/webos-satellite --self-test`).

## Make targets

| Target | What it does |
| --- | --- |
| `make build-tv` | Static armv7 musl release build with cargo-zigbuild; prints `file` and sha256 |
| `make build-tv64` | Static aarch64 musl release build, for comparison |
| `make test` | `cargo test --workspace` on the host |
| `make lint` | `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` |
| `make fmt` | `cargo fmt` |
| `make deploy-tv` | Builds both, then `scp` to the TV's `/tmp` as `webos-satellite-armv7` and `webos-satellite-aarch64` |
| `make run-tv-selftest` | Runs `--self-test` of each deployed binary over `ssh` |
| `make clean-tv` | Deletes the deployed binaries from the TV's `/tmp` |
| `make tv-install-tmp` | Builds, then installs under `TV_PREFIX` (default `/tmp/webos-satellite`) with `TV_CONFIG=<toml>` and `TV_PSK=<key file>`; starts nothing ([below](#install-on-a-tv-non-persistent)) |
| `make tv-uninstall-tmp` | Runs the installed `uninstall.sh` (stop, remove the prefix) and removes the staging directory |

Variables: `TV_PREFIX`, `TV_CONFIG`, `TV_PSK` (install targets), `TV` (default `root@webos-tv`; set your TV's address with `TV=root@<host>` or in a gitignored `local.mk` containing `TV = root@<host>`), `TV_DIR` (default `/tmp`), `TIMEOUT`. All `ssh` and `scp` calls use `timeout` and an SSH ControlMaster socket.

## Running on a TV: rules

- Binaries go to `/tmp` (tmpfs, lost at reboot) only, and are deleted afterwards (`make clean-tv`). Inspect and checksum before copying (`file`, `readelf -h`, `sha256sum`). Never commit binaries.
- Each TV session needs owner authorization ([roadmap](project/roadmap.md#phase-1-silent-build-and-integration-no-sound-played)). Follow the [discovery safety rules](discovery/README.md#safety-rules-learned): no ASoC/DAPM sysfs nodes, no `/sys/kernel/debug`, no writes outside `/tmp`, no service changes.
- The `--self-test` is read-only: it reads `/proc/self/status`, calls `uname`, `getrlimit` and the page size, and opens no audio device and no socket.

## Running the satellite

Configuration is a TOML file; every key is optional and [config/example.toml](../config/example.toml) lists all of them with their defaults. Command-line options override the file (`webos-satellite --help`). `--check-config` validates and prints a summary without starting.

1. Create a Noise key and keep it private (it is also the "encryption key" HA asks for):

   ```sh
   umask 077; head -c 32 /dev/urandom | base64 > devices/private/<device>/noise_psk
   ```

2. Write a config next to it (in `devices/private/`, which is gitignored). Example for a PC test with a recorded TV-mic file and WAV output:

   ```toml
   mode = "wake"                       # or "ptt"
   [device]
   name = "webos-satellite-dev"        # unique among HA's ESPHome devices
   mac_seed = "<random stable string>"
   noise_psk_file = "noise_psk"
   [mic]
   source = "file"
   path = "<recording>.raw"            # S24_LE, 2 ch, 16 kHz
   [sink]
   type = "wav"
   dir = "<output dir>"
   ```

3. Run it: `target/release/webos-satellite --config devices/private/<device>/<file>.toml`.

Behaviour:

- `mode = "wake"`: as soon as HA subscribes, the satellite streams the mic continuously and HA's pipeline runs the wake word. `mode = "ptt"`: `kill -USR1 <pid>` starts one run at speech-to-text. A SIGUSR1 also works in wake mode, where it aborts the wake word run. `--ptt-on-connect` and `--ptt-every SECS` are test aids.
- Audio is sent only while HA asks for it. If the mic is busy (LG hands-free holds the PCM), the satellite logs `MIC BUSY` and retries with a backoff of up to 30 s.
- **Microphone gate.** The capture PCM is open only while the microphone is wanted. It is closed (the hardware mic is released, `/proc/asound/.../status` shows `closed`) when HA's "Mute microphone" switch is on, while the **kill-switch** file exists (`safety.kill_switch`, default `/tmp/webos-satellite.disable`), or when the **firmware guard** refused the firmware. The kill switch and the guard force the switch on in HA, and an unmute from HA is refused until they clear; the API connection stays up so HA still shows the device. HA's mute is remembered in `safety.mute_file` (default `/tmp/webos-satellite.muted`) so a restarted daemon comes back muted. Measured on c3-a: PCM closed 3 to 18 ms after the mute reached the satellite, 30 to 70 ms after the kill-switch file appeared ([P3 robustness](discovery/c3-a/P3-robustness.md)).
  - Stop listening now: `touch /tmp/webos-satellite.disable`; resume: `rm /tmp/webos-satellite.disable`. The capture thread checks the file itself every 250 ms, so this works even if the event loop hangs.
- **Firmware guard.** At startup the daemon reads `webos_manufacturing_version` from `/var/run/nyx/os_info.json` (read-only; `03.30.74` on c3-a) and compares it with `safety.validated_firmware`. On another version, or if the file cannot be read, the ALSA microphone stays disabled (reported muted) and an `ERROR FIRMWARE GUARD` line is logged. `--check-firmware` prints the verdict and exits 0 (validated), 1 (not validated or unreadable) or 3 (guard off: empty list). File-mic runs are not gated.
- **Watchdog and limits.** An ALSA read that sees no audio for `limits.capture_stall_s` (5 s) closes and reopens the device with backoff. A watchdog thread exits the process with code 70 (event loop stalled), 71 (capture thread stalled) after `limits.loop_stall_s` (30 s), or 72 when the resident size exceeds `limits.max_rss_mb` (64 MiB), so a supervisor restarts it; a SIGSTOP/SIGCONT or system suspend is detected and not counted. At startup the daemon sets its niceness (`limits.nice`, 10) and the soft open-file limit (`limits.max_open_files`, 256), inherited by `pacat`. `RLIMIT_AS` is not set because `pacat` would inherit it (PulseAudio clients map large shared-memory pools). `--test-hang loop|capture` freezes a thread to test the watchdog. The watchdog also logs `memory: resident size …` at info for every 2 MiB of RSS growth (debug: every 10 s). The musl (TV) builds use the pure-Rust `dlmalloc` as global allocator because musl's malloc kept growing under TTS replies ([P3 robustness](discovery/c3-a/P3-robustness.md)); streamed TTS waits for the sink in a queue limited to 2 MiB, dropping audio beyond that.
- Replies: HA streams the TTS as 16 kHz mono PCM. The `pacat` sink writes it to PulseAudio as it arrives, multiplied by `sink.gain` (use `0.0` for silent tests; `pacat --volume` is ignored on c3-a). The `wav` sink writes one file per reply. Each playback logs its count of non-zero samples.
- Logs go to stderr: one latency line per run. Transcripts and reply text appear only at `debug`. Keys and token URLs are never logged ([policy](#logging-policy)).
- SIGTERM/SIGINT stop playback (killing `pacat`), release the PCM and disconnect HA.

On the TV, use the scripts in [Install on a TV](#install-on-a-tv-non-persistent) rather than copying files by hand.

### Configuration reference

[config/example.toml](../config/example.toml) is the reference: every key with its default and a comment, and a unit test fails if the file and the code's defaults drift apart. Rules:

- Every key is optional. Unknown sections and keys are **rejected** with the key name and line (`deny_unknown_fields`), so a typo cannot silently fall back to a default.
- Relative paths in the file resolve against the file's directory; on the command line against the current directory. Command-line options override the file whatever their position.
- `validate()` (also run by `--check-config`) rejects, naming the key: an unknown `log.level`, `mic.rate` other than 16000, zero `mic.channels`, `period_frames` or `port`, `buffer_frames` smaller than `period_frames`, `mic.source = "file"` without `mic.path`, `dsp.channel = "right"` on a mono mic, out-of-range `dsp.gain_db` (-20 to 60) or `sink.gain` (0 to 4), zero `sink.rate`, `sink.channels` or `sink.latency_msec`, both `noise_psk` and `noise_psk_file`, no key without `allow_plaintext`, and a node name or key the ESPHome layer rejects.
- `[device]` `announce` (default true), `timers` (false) and `start_conversation` (false) choose the advertised voice feature flags: 16, 8 and 32 on top of the fixed 7 (voice assistant, speaker, API audio), so the default is 23, `timers = true` gives 31 and `start_conversation = true` gives 55 (63 with both). `start_conversation = true` with `announce = false` is rejected. The satellite behaves the same whichever bits are advertised; they decide what HA offers. With `timers = true` HA forwards its voice timers: the daemon keeps a `TimerTable` and logs each change as `timer <started|updated|cancelled|finished> id=… seconds_left=…` at info (never the timer name). There is no chime yet; see [Timers](development/esphome-api.md#timers). The startup summary and `--check-config` print `voice_features=N`.
- It also rejects `limits.nice` outside -20 to 19, `limits.max_open_files` below 32 (0 = inherit), `limits.max_rss_mb` below 8 (0 = off), `limits.loop_stall_s` below 5 (0 = off), and an empty or identical `safety.kill_switch` and `safety.mute_file`.
- Non-fatal **warnings** (printed by `--check-config`, logged at `warn` at startup, never containing a secret): `noise_psk_file` readable by group or other (with the actual mode and `chmod 600`), `noise_psk` set inline, plaintext allowed, `mic.path` missing, an empty `safety.validated_firmware` (guard off).

### Logging policy

Logs go to **stderr**. The ops scripts redirect it to `<prefix>/log/webos-satellite.log`, a file in the (non-persistent) install directory, with size-based rotation (see below); never to syslog or the journal, which may persist and are read by other tools. Format: `UTC-timestamp LEVEL message`.

| Level | Content |
| --- | --- |
| `error` | Capture failures other than a busy mic, fatal errors |
| `warn` | Mic busy past a few retries, HA disconnects, pipeline error **codes**, SPEAKER-mode oddities, config warnings, test triggers enabled |
| `info` (default) | Startup (version, target, config summary without secrets), HA connect and disconnect with the peer address, run lifecycle, one latency line per run, character **counts** of transcripts and replies, playback statistics, shutdown |
| `debug` | Transcript text, reply text, announcement text, HA's error message text, event names with payload sizes, retry timing |
| `trace` | Protocol message names (never payloads) |

Never logged at any level:

- audio (the capture path logs counts only; `AppEvent` and `AppCommand` have a hand-written `Debug` that prints sizes, so `{ev:?}` cannot dump PCM);
- the Noise key (`Config` and `DeviceSection` redact it in `Debug`; the summary prints only `encryption=noise`);
- Home Assistant tokens and TTS or announcement URLs, which may carry `authSig` or token parameters: only `scheme://host/first-segment/…` is logged (`redact_url`), and the HTTP client's malformed-URL message is replaced because it can echo the URL;
- the conversation id (`<set>` only).

What does appear: the LAN address of HA (peer of the API connection) and the device name. A leak review of the logs is part of every Phase 1 TV run: start with `level = "debug"`, send canary strings through a client (transcript, reply, URL with token, error message) and grep the log for them and for the key (done for 1.7, see [P1 ops](discovery/c3-a/P1-ops.md)).

## Install on a TV (non-persistent)

The scripts in [`packaging/webos/`](../packaging/webos) are POSIX sh (checked with `sh -n` and ShellCheck, BusyBox ash compatible). Everything lives under one prefix directory; nothing is written elsewhere. Layout: `bin/` (daemon, scripts), `etc/` (`config.toml`, `noise_psk`, mode 0600), `run/` (pidfiles), `log/`, `out/` (a place for a WAV sink), and a marker file `.webos-satellite-install`.

| Script | Purpose |
| --- | --- |
| `install.sh --prefix DIR --config FILE [--binary FILE] [--psk-file FILE]` | Copies binary, scripts, config and key (0700 dirs, 0600 files), validates the config with `--check-config`. Refuses a non-empty directory that is not a previous install. Refuses prefixes outside `/tmp`, `/run` and `/dev/shm` unless `--persistent` (owner authorization required). The config has no default on purpose: the example config plays audio |
| `start.sh [--supervise]` | Starts the daemon in the background (`nohup`, stdin from `/dev/null`), pidfile in `run/`, log in `log/`; checks after 1 s that it is still alive and prints the log tail if not. `--supervise` runs it under `supervise.sh`. Refuses while `DIR/disable` exists, and with `--supervise` while `DIR/crashloop` exists |
| `supervise.sh` | Restart loop (started by `start.sh --supervise`): a daemon exit other than 0 restarts it after 1, 2, 4, ... s (at most `WEBOS_SAT_BACKOFF_MAX`, 300 s); `WEBOS_SAT_MAX_CRASHES` (5) exits within `WEBOS_SAT_CRASH_WINDOW` (600 s) write `DIR/crashloop` and stop it for good. Exit 0 (the daemon stopped on SIGTERM) ends it. Logs `SUPER` lines into the daemon log |
| `autostart.sh` | Boot-time start for the draft `init.d` hook: waits (`--delay`, 60 s), then starts `start.sh --supervise` unless `DIR/disable`, `DIR/crashloop` or the kill switch exists or `--check-firmware` fails; logs to `log/autostart.log`. Not wired into boot ([autostart design](design/autostart.md)) |
| `stop.sh` | Stops the supervisor first (so nothing restarts), then SIGTERM to the daemon, waits up to 10 s, then SIGKILL; removes the pidfiles. The pid is only trusted if `/proc/<pid>/exe` is the installed binary |
| `status.sh` | Running or not (exit 0 or 3), supervisor, markers and kill switch, RSS, fd and thread counts, log size, last log lines |
| `uninstall.sh` | Stops, then `rm -rf` of the prefix. Refuses `/`, empty or relative paths, `..` components, symbolic links, system directories (`/tmp` itself, `/var`, `/media/developer`, ...) and any directory without the marker file |

The installed scripts find their prefix from their own location (`DIR/bin/..`); from the repository they take `--prefix DIR`.

**Log rotation.** Logs go to a file, not to syslog (`logger`), see the policy above. `start.sh` moves a log larger than `WEBOS_SAT_LOG_MAX` bytes (default 262144) to `webos-satellite.log.1` at each start, and a small sidecar (`rotate.sh`, one `sh` process, stopped with the daemon) checks every 30 s and copies the log to `.log.1` and truncates it above the limit. At most about twice the limit is on the tmpfs. At `info` the volume is a few lines per run.

```sh
# From the PC, with the owner's authorization for the TV session:
make tv-install-tmp TV=root@<host> TV_CONFIG=devices/private/<device>/tv.toml TV_PSK=devices/private/<device>/noise_psk
ssh root@<host> 'sh /tmp/webos-satellite/bin/start.sh'
ssh root@<host> 'sh /tmp/webos-satellite/bin/status.sh'
ssh root@<host> 'sh /tmp/webos-satellite/bin/stop.sh'
make tv-uninstall-tmp TV=root@<host>
```

The config's `noise_psk_file = "noise_psk"` resolves to `etc/noise_psk`. For silent tests use `[sink] type = "null"` or `"wav"` with `dir = "../out"`, or `"pacat"` with `gain = 0.0`. `pacat --volume` is ignored on c3-a.

**No autostart.** These scripts do not hook into boot. The reviewed design, the draft hook `packaging/webos/init.d/webos-satellite` and the owner-approval step are in [autostart design](design/autostart.md). Nothing is installed in `/var/lib/webosbrew/init.d`, and `--persistent` has not been used on a TV.

**Markers** (files at the top of the prefix): `disable` (do not start; honoured by `start.sh`, `supervise.sh` and `autostart.sh`), `crashloop` (written by the supervisor when it gives up; remove it by hand after fixing the cause). The kill switch is separate: it keeps a running daemon's microphone closed.

The before/after result on c3-a is in [P1 ops](discovery/c3-a/P1-ops.md).

### Network prerequisite

Home Assistant's ESPHome integration opens the connection: HA must be able to reach the TV on **TCP 6053** (or the configured `device.port`). With HA in a container or Kubernetes pod, check the path from HA's network to the TV, including router rules between network segments. The owner's network blocked it as of 2026-11-02 ([P1 HA integration](discovery/c3-a/P1-ha-integration.md#network-check)). A failed config flow shows `connection_error`, and HA's log shows `[Errno 111] Connect call failed`. Firewall details specific to one installation belong with that installation's infrastructure docs, not in this repository.

### Testing without HA reaching the device

`tools/esphome-fake-ha/run.sh` runs the protocol harness and an end-to-end check of the real binary (file mic, WAV sink, wake and push-to-talk runs, SIGTERM, log policy). Everything stays on 127.0.0.1.

When HA cannot open a connection to the satellite (the situation on the owner's network as of 2026-11-02), `tools/esphome-fake-ha/ha_bridge.py` plays HA's ESPHome integration from the PC. It connects to the satellite and runs HA's real pipeline over the WebSocket API with an access token read from a file. See its docstring and [P1 HA integration](discovery/c3-a/P1-ha-integration.md#bridge-stands-in-for-has-esphome-integration) for what it does and does not cover.

## CI

- [`.github/workflows/validate.yml`](../.github/workflows/validate.yml): documentation checks (`sh -n tools/probe/probe.sh`, `sh -n` of the packaging and lifecycle scripts, `python3 tools/check_docs.py`).
- [`.github/workflows/rust.yml`](../.github/workflows/rust.yml): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` and `packaging/webos/tests/run.sh` on the host; then the armv7 and aarch64 cross-builds, ELF checks, a run of the armv7 binary under `qemu-user-static`, and the armv7 binary uploaded as a short-lived workflow artifact.

Actions are pinned to commit SHAs with the version in a comment. Update them deliberately.

## Quality gates before a PR

```sh
make lint test
python3 tools/check_docs.py
sh -n tools/probe/probe.sh
for f in packaging/webos/*.sh packaging/webos/init.d/* packaging/webos/tests/*.sh tools/lifecycle/*.sh; do sh -n "$f"; done
shellcheck -x -P SCRIPTDIR -s sh packaging/webos/*.sh packaging/webos/init.d/* packaging/webos/tests/*.sh   # if installed
tools/esphome-fake-ha/run.sh     # protocol and daemon e2e harness
tools/lifecycle/run.sh           # microphone gate and ops scripts on the host
```
