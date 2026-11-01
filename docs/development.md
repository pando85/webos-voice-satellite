# Development

How to build, test and run the Rust workspace. Status: Phase 1.1 (workspace, cross-build, CI, TV smoke test). The binary does nothing useful yet; see the [roadmap](project/roadmap.md) for what comes next and [P1 runtime smoke](discovery/c3-a/P1-runtime-smoke.md) for what was verified on the TV.

## Layout

| Path | Purpose |
| --- | --- |
| `Cargo.toml` | Workspace, shared `[workspace.dependencies]`, lint config, size-tuned `[profile.release]` |
| `rust-toolchain.toml` | Pinned toolchain (`1.99.1`), `rustfmt`, `clippy`, the two TV cross targets |
| `.cargo/config.toml` | Softfp codegen flags for `armv7-unknown-linux-musleabi` |
| `Makefile` | Entry points listed below |
| `crates/satellite` | Binary `webos-satellite` (`--version`, `--self-test`) |
| `crates/capture`, `dsp`, `esphome-api`, `playback` | Stub libraries; each is implemented in its own directory by a later roadmap item (1.2, 1.2, 1.3, 1.6) |

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
