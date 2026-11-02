# Development

How to build, test and run the Rust workspace. Status: Phase 1.4/1.5. `webos-satellite` is the working daemon (capture → front-end → ESPHome API → reply playback). For what was verified, see [P1 HA integration](discovery/c3-a/P1-ha-integration.md) and [P1 TV live](discovery/c3-a/P1-tv-live.md); for what comes next, see the [roadmap](project/roadmap.md).

## Layout

| Path | Purpose |
| --- | --- |
| `Cargo.toml` | Workspace, shared `[workspace.dependencies]`, lint config, size-tuned `[profile.release]` |
| `rust-toolchain.toml` | Pinned toolchain (`1.99.1`), `rustfmt`, `clippy`, the two TV cross targets |
| `.cargo/config.toml` | Softfp codegen flags for `armv7-unknown-linux-musleabi` |
| `Makefile` | Entry points listed below |
| `crates/satellite` | Binary `webos-satellite`: the daemon (config, capture thread, player, latency log), plus `--self-test` |
| `crates/capture` | ALSA capture through raw ioctls (no alsa-lib), file source for tests |
| `crates/dsp` | Mic front-end: channel select, high-pass, gain, S24 to S16 |
| `crates/esphome-api` | ESPHome native API device server and voice state machine ([details](development/esphome-api.md)) |
| `crates/playback` | TTS fetch, decode, resample, `pacat`/WAV/null sinks |
| `config/example.toml` | Commented configuration with all defaults |
| `tools/esphome-fake-ha/` | Fake-HA harnesses (`run.sh`) and the HA bridge (`ha_bridge.py`) |

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

Variables: `TV` (default `root@webos-tv`; set your TV's address with `TV=root@<host>` or in a gitignored `local.mk` containing `TV = root@<host>`), `TV_DIR` (default `/tmp`), `TIMEOUT`. All `ssh` and `scp` calls use `timeout` and an SSH ControlMaster socket.

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
- Replies: HA streams the TTS as 16 kHz mono PCM. The `pacat` sink writes it to PulseAudio as it arrives, multiplied by `sink.gain` (use `0.0` for silent tests; `pacat --volume` is ignored on c3-a). The `wav` sink writes one file per reply. Each playback logs its count of non-zero samples.
- Logs go to stderr: one latency line per run. Transcripts and reply text appear only at `debug`. Keys and token URLs are never logged.
- SIGTERM/SIGINT stop playback (killing `pacat`), release the PCM and disconnect HA.

On the TV (rules in "Running on a TV" above): copy the static armv7 binary, the config and the key to `/tmp`, set `[mic] source = "alsa"` (defaults are the c3-a values), and start it in the background with its log in `/tmp`. Delete everything afterwards. Nothing is installed or persisted.

### Testing without HA reaching the device

`tools/esphome-fake-ha/run.sh` runs the protocol harness and an end-to-end check of the real binary (file mic, WAV sink, wake and push-to-talk runs, SIGTERM, log policy). Everything stays on 127.0.0.1.

When HA cannot open a connection to the satellite (the situation on the owner's network as of 2026-11-02), `tools/esphome-fake-ha/ha_bridge.py` plays HA's ESPHome integration from the PC. It connects to the satellite and runs HA's real pipeline over the WebSocket API with an access token read from a file. See its docstring and [P1 HA integration](discovery/c3-a/P1-ha-integration.md#bridge-stands-in-for-has-esphome-integration) for what it does and does not cover.

## CI

- [`.github/workflows/validate.yml`](../.github/workflows/validate.yml): documentation checks (`sh -n tools/probe/probe.sh`, `python3 tools/check_docs.py`).
- [`.github/workflows/rust.yml`](../.github/workflows/rust.yml): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on the host; then the armv7 and aarch64 cross-builds, ELF checks, a run of the armv7 binary under `qemu-user-static`, and the armv7 binary uploaded as a short-lived workflow artifact.

Actions are pinned to commit SHAs with the version in a comment. Update them deliberately.

## Quality gates before a PR

```sh
make lint test
python3 tools/check_docs.py
sh -n tools/probe/probe.sh
```
