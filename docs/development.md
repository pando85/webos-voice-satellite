# Development

How to build, test, package and run the Rust workspace. Status: Phase 1 plus the non-persistent Phase 3 work (kill switch, watchdog, supervisor, firmware guard, resource limits; [P3 robustness](discovery/c3-a/P3-robustness.md)). `webos-satellite` is the working daemon (capture → front-end → ESPHome API → reply playback). For what was verified, see [P1 HA integration](discovery/c3-a/P1-ha-integration.md) and [P1 TV live](discovery/c3-a/P1-tv-live.md); for what comes next, see the [roadmap](project/roadmap.md).

## Layout

| Path | Purpose |
| --- | --- |
| `Cargo.toml` | Workspace, shared `[workspace.dependencies]`, lint config, size-tuned `[profile.release]` (opt-level 3 for `webos-dsp` and the `sonora` crates, which run per 10 ms frame) |
| `rust-toolchain.toml` | Pinned toolchain (`1.99.1`), `rustfmt`, `clippy`, the two TV cross targets |
| `.cargo/config.toml` | Softfp codegen flags for `armv7-unknown-linux-musleabi` |
| `Makefile` | Entry points listed below |
| `crates/satellite` | Binary `webos-satellite`: the daemon (config, capture thread, player, latency log, microphone gate, firmware guard, watchdog, resource limits), plus `--self-test` and `--check-firmware` |
| `crates/capture` | ALSA capture through raw ioctls (no alsa-lib), the mic plus an echo-reference PCM started on one trigger (`AlsaDuplex`), file sources for tests (`FileSource`, `FilePair`), the `aec_offline` example |
| `crates/dsp` | Mic front-end: channel select, high-pass, optional echo canceller (WebRTC AEC3 through `sonora`, [design](design/aec-frontend.md)), gain, S24 to S16. Declares `rust-version = 1.91` (sonora's); the other crates keep the workspace's 1.85, and the pinned toolchain is newer than both |
| `crates/esphome-api` | ESPHome native API device server and voice state machine ([details](development/esphome-api.md)) |
| `crates/playback` | TTS fetch, decode, resample, `pacat`/WAV/null sinks |
| `config/example.toml` | Commented configuration with all defaults |
| `packaging/webos/` | POSIX sh install, start, stop, status, uninstall and supervisor scripts for a TV ([below](#install-on-a-tv-non-persistent)); `ctl.sh`, `app/`, `config.default.toml` and `ipk/build_ipk.py` make the installable webOS app package ([below](#install-the-app-package-on-a-rooted-tv-persistent), [packaging design](design/packaging.md)); `init.d/` holds the boot hook template ([autostart design](design/autostart.md)); `tv/appinstall.sh` runs appinstalld on the TV; `tests/` tests all of it on the host |
| `tools/esphome-fake-ha/` | Fake-HA harnesses (`run.sh`) and the HA bridge (`ha_bridge.py`) |
| `tools/lifecycle/` | Microphone-gate check of the real binary (`local_check.py`), an HA-side client for mute cycles, state watching and reconnect loops against a TV (`lifecycle_client.py`), `run.sh` for both host checks |

Keep the root files minimal and stable; add per-crate dependencies in the crate's own `Cargo.toml`, with versions declared once in `[workspace.dependencies]`. Workspace lints forbid `unsafe` code; a crate that genuinely needs it (for example tinyalsa ioctls in `capture`) must opt out locally and say why.

## Target decision

The TV userland is 32-bit ARM EABI5 with the soft-float `e_flags` `0x05000200`, glibc 2.31 and no hard-float or aarch64 loader ([D0-D1](discovery/c3-a/D0-D1-runtime.md#32-bit-vs-64-bit-userland)). Following the [research recommendation](research/esphome-rust-satellite.md#c1-target-triple-linking-and-audio):

- **armv7 (default, runs on every supported TV):** static `armv7-unknown-linux-musleabi` with `-C target-feature=-soft-float,+vfp3,+neon` (set in `.cargo/config.toml`). Static linking means the TV's glibc, loader and libraries are not involved.
- **aarch64 (TVs with a 64-bit kernel):** static `aarch64-unknown-linux-musl`. A static binary needs no loader, so it runs on a 64-bit kernel even with a 32-bit userland.

Both run on c3-a, whose kernel is aarch64 under a 32-bit userland ([P1 runtime smoke](discovery/c3-a/P1-runtime-smoke.md)). The echo canceller costs four times less in the aarch64 build there (0.21 against 1.0 ms per 10 ms frame, because sonora has SIMD for aarch64 and not for armv7; [P3 TV verification](discovery/c3-a/P3-tv-verify-aec-pairing.md#1-aec-cpu-benchmark-section-9-step-1)). The package therefore carries both and `install.sh` chooses by `uname -m` ([below](#choosing-the-build-armv7-or-aarch64)).

**Status of the aarch64 daemon: pending.** On c3-a only `--self-test` (P1) and the offline `aec_offline` tool (P3) have run as aarch64. The daemon itself (ALSA capture with the satellite's ioctls, the linked reference PCM, `pacat`, the watchdog, pairing) has run on a TV only as armv7. Until that run, `--arch armv7` pins the verified build.

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
| `make build-tv64` | Static aarch64 musl release build (for TVs with a 64-bit kernel) |
| `make build-tv-all` | Both builds |
| `make package-tv` | Both builds plus the ops scripts in `target/webos-package/` (binaries named `webos-satellite-armv7` and `webos-satellite-aarch64`, `SHA256SUMS`) and as `target/webos-satellite-package.tar.gz` |
| `make test` | `cargo test --workspace` on the host |
| `make lint` | `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` |
| `make fmt` | `cargo fmt` |
| `make deploy-tv` | Builds both, then `scp` to the TV's `/tmp` as `webos-satellite-armv7` and `webos-satellite-aarch64` |
| `make run-tv-selftest` | Runs `--self-test` of each deployed binary over `ssh` |
| `make clean-tv` | Deletes the deployed binaries from the TV's `/tmp` |
| `make tv-install-tmp` | Packages, copies the package, then installs under `TV_PREFIX` (default `/tmp/webos-satellite`) with `TV_CONFIG=<toml>` and `TV_PSK=<key file>`; `TV_ARCH` (default `auto`, or `armv7`, `aarch64`) is passed to `install.sh --arch`; starts nothing ([below](#install-on-a-tv-non-persistent)) |
| `make tv-uninstall-tmp` | Runs the installed `uninstall.sh` (stop, remove the prefix) and removes the staging directory |
| `make ipk` | Both builds, then the webOS app package `target/ipk/<APP_ID>_<VERSION>_all.ipk` with its `.sha256` and an HBC manifest, and the structure check. `VERSION` defaults to the workspace version in `Cargo.toml`; `APP_ID` to `io.github.pando85.voicesatellite`. Reproducible: the same inputs and commit give the same bytes ([packaging design](design/packaging.md)) |
| `make ipk-check` | Structure check of the built ipk (ar/tar layout, control fields, appinfo.json, icons, payload sums, ELF of both builds) |
| `make test-packaging` | Host tests: `tests/run.sh` (ops scripts), `tests/ipk.sh` (package lifecycle), `tests/app_smoke.js` (status app; needs `node`) |
| `make tv-install-ipk` | Builds the ipk, copies it to the TV's `/tmp`, installs it with appinstalld (`dev/install`, waits for the result), deletes the copy and prints `ctl.sh status`. Starts nothing. Persistent: owner authorization |
| `make tv-remove-ipk` | Removes the app with appinstalld (`dev/remove`); run `ctl.sh uninstall` or `purge` first |

Variables: `TV_PREFIX`, `TV_CONFIG`, `TV_PSK`, `TV_ARCH` (install targets), `APP_ID`, `VERSION`, `TV_APP_DIR` (ipk targets), `TV` (default `root@webos-tv`; set your TV's address with `TV=root@<host>` or in a gitignored `local.mk` containing `TV = root@<host>`), `TV_DIR` (default `/tmp`), `TIMEOUT`. All `ssh` and `scp` calls use `timeout` and an SSH ControlMaster socket.

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
- **Echo cancellation (off by default).** With `[aec] enabled = true` the satellite also captures the TV's own output from the reference PCM named by `aec.reference_pcm_id` on the mic's card (`SoundEngineCapture` on c3-a), starts it together with the mic, and runs WebRTC AEC3 (`sonora`) between the high-pass filter and the +24 dB gain. If the reference cannot be opened, the mic runs without it and a `warn` line says why. In file mode, `aec.reference_path` gives the recorded reference, so a recorded mic + reference pair runs on a PC. On c3-a (armv7 build, [P3 TV verification](discovery/c3-a/P3-tv-verify-aec-pairing.md#2-daemon-smoke-with-aec-section-9-step-2)) the reference opened and was linked to the mic, the canceller cost about 13 % of one core (2.8 % with `bypass_when_silent` while the TV was silent), there were no xruns, reference underruns or drops in 10 minutes, and HA's mute, the kill switch and `stop.sh` closed both PCMs. **Not verified on a TV:** the delay lock and the in-daemon ERLE (the TV played nothing during that run, so the reference had no active frames). Design, measurements on real c3-a recordings and the rest of the on-TV plan: [AEC front-end](design/aec-frontend.md). To process a recorded pair offline with the same front-end: `cargo build --release -p webos-capture --example aec_offline`, then [`chain_eval.py`](../tools/audio-rig/aec_eval/chain_eval.py) (ERLE, delay, CPU per variant, and the 16 kHz outputs for wake word and STT scoring).
- Replies: HA streams the TTS as 16 kHz mono PCM. The `pacat` sink writes it to PulseAudio as it arrives, multiplied by `sink.gain` (use `0.0` for silent tests; `pacat --volume` is ignored on c3-a). `pacat` runs with `--latency-msec` from `sink.latency_msec` (default 200) for streamed replies and announcements: with PulseAudio's default buffering (`0`) a short streamed reply started about 1.5 s late on c3-a ([P2 session 1](discovery/c3-a/P2-session1-audible-and-echo.md#end-to-end-run-through-ha-at-volume-10)); with 200 ms replies started 85 to 332 ms after the stream, without underruns ([P2 session 2](discovery/c3-a/P2-session2-wake-accuracy.md#0-reply-start-latency)). The `wav` sink writes one file per reply. Each playback logs its count of non-zero samples.
- Logs go to stderr: one latency line per run. Transcripts and reply text appear only at `debug`. Keys and token URLs are never logged ([policy](#logging-policy)).
- SIGTERM/SIGINT stop playback (killing `pacat`), release the PCM and disconnect HA.

On the TV, use the scripts in [Install on a TV](#install-on-a-tv-non-persistent) rather than copying files by hand.

### Pairing without typing a key

Status: implemented and tested on a PC (aioesphomeapi harnesses, and one run against a real Home Assistant 2026.10.4 in a local venv, below). **Verified on c3-a with the owner's HA 2026.10.4** (armv7 build, a test device, 2026-11-02, [P3 TV verification](discovery/c3-a/P3-tv-verify-aec-pairing.md#3-pairing-on-the-tv-with-the-owners-ha)): pairing without a typed key, the key file modes, the Noise reconnect, the microphone closed until the key, no key in logs, restart and reload reconnects, and reset with HA's re-authentication. Two results of that session were negative and are changed since, **not yet re-verified with a real HA:** the window timeout (HA showed `connection_error`; the race behind it is fixed, see Window below) and HA's key clear on entry removal (refused then; accepted now, see Removal below). Not done on a TV: an HA restart (a reload stood in) and mDNS discovery by an HA on the TV's L2. Design and threat model: [install and configuration UX](design/install-and-config-ux.md#5-pairing-mode-design-option-c-detail); protocol details: [ESPHome API crate](development/esphome-api.md#pairing-ha-driven-key-provisioning).

Instead of creating a key, set `[pairing] enabled = true` and leave `device.noise_psk` and `device.noise_psk_file` unset. Home Assistant 2026.9 or newer then generates the key and sets it on the device (HA calls this a dynamic encryption key):

1. Start the daemon. With no stored key it logs `NOT PAIRED` and `PAIRING WINDOW OPEN for 600 s`, and the microphone stays closed (`capture: microphone off (not paired)`).
2. In HA add the ESPHome integration with the TV's address and port 6053 (or accept the discovered device if mDNS works). HA reads the device info without a key, sees that the device is provisionable, sends a random key over a short Noise session with the all-zero key, stores the key and reconnects with it. No key is shown or typed.
3. The daemon logs `PAIRED by "Home Assistant 2026.10.4" from <address>` and opens the microphone. Check that this is your HA.

Rules and controls:

- **Precedence.** An explicit `device.noise_psk` or `noise_psk_file` disables pairing and works as before (`--check-config` warns that `pairing.enabled` is ignored). `pairing.enabled` together with `device.allow_plaintext` is rejected.
- **State directory.** `pairing.state_dir` (default `pairing`, relative to the config file, created with mode 0700) holds the stored key `noise_psk` (mode 0600, written atomically: temporary file, `fsync`, `rename`). It is the same format as `noise_psk_file`. Keep it on persistent storage if the pairing must survive a reboot.
- **Window.** It opens at start when no key is stored (`pairing.open_on_start`, default true), with `--pair`, or when the file `<state_dir>/pair` appears. It closes after `pairing.window_s` (default 600 s, 1 to 86400) or as soon as a key is set. Closed means: a new keyless connection gets the hello and then a disconnect with the reason HA shows as "closed its setup window". The disconnect is sent after the client's next message (HA asks for the device info), or after 2 s if it sends nothing: sent right behind the hello, as ESPHome does, it raced in aioesphomeapi 46.6.0 and the owner's HA showed `connection_error` instead (fixed; reproduced and tested on a PC with a proxy that batches the device's frames, not yet re-tested with a real HA). The closed state is not stored; a restart reopens the window if `open_on_start` is true, except after a removal (below).
- **While unpaired** a keyless connection gets the device identity, an empty entity list and the key exchange only. No voice pipeline, no audio, no entities, and the microphone PCM stays closed.
- **Reset.** `--reset-pairing` (at startup) or the file `<state_dir>/reset-pairing` (while running) deletes the stored key, closes every connection, closes the microphone and reopens the window. The daemon deletes trigger files when it acts on them (checked every second). An HA that knew the old key starts a re-authentication: in HA, confirm "encryption removed" for the device and HA sets the key it stored before (seen with a local HA 2026.10.4; it does not need the device to be removed).
- **Removal in HA.** When the device's entry is deleted, HA (2026.10.4) connects with the key and sends an empty key to clear it. The daemon accepts that only from a session that proved the stored key, never from a keyless session, and never when the key comes from `device.noise_psk` or `noise_psk_file`. It deletes the stored key, writes `<state_dir>/noise_psk.cleared`, closes the connection and the microphone (`pairing CLEARED by ...`), and stays unpaired with the window **closed**, also across restarts, so nobody else on the LAN can pair it. HA then forgets the key it generated. To pair again, open the window locally (`--pair` or the `pair` file; a reset also works), which removes the marker.
- **Older HA** (before 2026.9) cannot pair this way; set `device.noise_psk_file` and type the key in HA instead.
- **Security.** The key exchange stops passive sniffing but authenticates nobody: whoever on the LAN connects first while the window is open can set the key and later receive microphone audio. Open the window only when you are about to add the device, keep it short, and check the `PAIRED by` line. See [SECURITY.md](../SECURITY.md#pairing).

### Configuration reference

[config/example.toml](../config/example.toml) is the reference: every key with its default and a comment, and a unit test fails if the file and the code's defaults drift apart. Rules:

- Every key is optional. Unknown sections and keys are **rejected** with the key name and line (`deny_unknown_fields`), so a typo cannot silently fall back to a default.
- Relative paths in the file resolve against the file's directory; on the command line against the current directory. Command-line options override the file whatever their position.
- `validate()` (also run by `--check-config`) rejects, naming the key: an unknown `log.level`, `mic.rate` other than 16000, zero `mic.channels`, `period_frames` or `port`, `buffer_frames` smaller than `period_frames`, `mic.source = "file"` without `mic.path`, `dsp.channel = "right"` on a mono mic, out-of-range `dsp.gain_db` (-20 to 60) or `sink.gain` (0 to 4), zero `sink.rate` or `sink.channels`, `sink.latency_msec` above 10000 (0 means PulseAudio's default buffering), both `noise_psk` and `noise_psk_file`, no key without `pairing.enabled` or `allow_plaintext`, `pairing.enabled` together with `allow_plaintext`, `pairing.window_s` outside 1 to 86400, an empty `pairing.state_dir`, `--pair` or `--reset-pairing` without active pairing, and a node name or key the ESPHome layer rejects.
- `[device]` `announce` (default true), `timers` (false) and `start_conversation` (false) choose the advertised voice feature flags: 16, 8 and 32 on top of the fixed 7 (voice assistant, speaker, API audio), so the default is 23, `timers = true` gives 31 and `start_conversation = true` gives 55 (63 with both). `start_conversation = true` with `announce = false` is rejected. The satellite behaves the same whichever bits are advertised; they decide what HA offers. With `timers = true` HA forwards its voice timers: the daemon keeps a `TimerTable` and logs each change as `timer <started|updated|cancelled|finished> id=… seconds_left=…` at info (never the timer name). There is no chime yet; see [Timers](development/esphome-api.md#timers). The startup summary and `--check-config` print `voice_features=N`.
- `[aec]`: it rejects an empty `reference_pcm_id`, a `reference_rate` other than 48000 or 16000, `reference_channels` outside 1 to 8, a zero `reference_period_frames` or a `reference_buffer_frames` below it, a `delay_ms` that is not `"auto"` or a number from 0 to 500, `reference_gain_db` outside -20 to 40, `render_channels` other than 1 or 2 or above `reference_channels`, `output` other than `"full"`, `"linear"` or `"nearend"`, `output = "linear"` or `"nearend"` with `render_channels = 2`, `ns` or `bypass_when_silent` without `enabled`, and `enabled` with `mic.source = "file"` but no `reference_path`. It warns when `reference_path` is set with the ALSA mic (ignored), when the reference file is missing, and when `ns` is on (evaluated offline only, see [P2 offline AEC A/B](experiments/P2-aec-offline-ab.md)).
- It also rejects `limits.nice` outside -20 to 19, `limits.max_open_files` below 32 (0 = inherit), `limits.max_rss_mb` below 8 (0 = off), `limits.loop_stall_s` below 5 (0 = off), and an empty or identical `safety.kill_switch` and `safety.mute_file`.
- Non-fatal **warnings** (printed by `--check-config`, logged at `warn` at startup, never containing a secret): `noise_psk_file` or the stored pairing key readable by group or other (with the actual mode and `chmod 600`), `noise_psk` set inline, `pairing.enabled` ignored because a key is set, plaintext allowed, `mic.path` missing, an empty `safety.validated_firmware` (guard off).

### Logging policy

Logs go to **stderr**. The ops scripts redirect it to `<prefix>/log/webos-satellite.log`, a file in the (non-persistent) install directory, with size-based rotation (see below); never to syslog or the journal, which may persist and are read by other tools. Format: `UTC-timestamp LEVEL message`.

| Level | Content |
| --- | --- |
| `error` | Capture failures other than a busy mic, fatal errors |
| `warn` | Mic busy past a few retries, HA disconnects, pipeline error **codes**, SPEAKER-mode oddities, config warnings, test triggers enabled |
| `info` (default) | Startup (version, target, config summary without secrets), HA connect and disconnect with the peer address, run lifecycle, one latency line per run, character **counts** of transcripts and replies, playback statistics, the echo canceller's state and delay lock, shutdown |
| `debug` | Transcript text, reply text, announcement text, HA's error message text, event names with payload sizes, retry timing, capture levels and echo canceller metrics every 60 s (delay, ERLE, CPU per frame; no audio) |
| `trace` | Protocol message names (never payloads) |

Never logged at any level:

- audio (the capture path logs counts only; `AppEvent` and `AppCommand` have a hand-written `Debug` that prints sizes, so `{ev:?}` cannot dump PCM);
- the Noise key, configured or provisioned by HA (`Config` and `DeviceSection` redact it in `Debug`, the pairing state machine never prints it, `SetKey` refusals name the reason only; the summary prints only `encryption=noise` or `encryption=pairing(paired)`);
- Home Assistant tokens and TTS or announcement URLs, which may carry `authSig` or token parameters: only `scheme://host/first-segment/…` is logged (`redact_url`), and the HTTP client's malformed-URL message is replaced because it can echo the URL;
- the conversation id (`<set>` only).

What does appear: the LAN address of HA (peer of the API connection) and the device name. A leak review of the logs is part of every Phase 1 TV run: start with `level = "debug"`, send canary strings through a client (transcript, reply, URL with token, error message) and grep the log for them and for the key (done for 1.7, see [P1 ops](discovery/c3-a/P1-ops.md)).

## Install on a TV (non-persistent)

The scripts in [`packaging/webos/`](../packaging/webos) are POSIX sh (checked with `sh -n` and ShellCheck, BusyBox ash compatible). Everything lives under one prefix directory; nothing is written elsewhere. Layout: `bin/` (daemon, scripts), `etc/` (`config.toml`, `noise_psk`, mode 0600), `run/` (pidfiles), `log/`, `out/` (a place for a WAV sink), and a marker file `.webos-satellite-install`.

| Script | Purpose |
| --- | --- |
| `install.sh --prefix DIR --config FILE [--arch auto\|armv7\|aarch64] [--binary FILE] [--psk-file FILE]` | Chooses the build ([below](#choosing-the-build-armv7-or-aarch64)), copies binary, scripts, config and key (0700 dirs, 0600 files), validates the config with `--check-config`. Refuses a non-empty directory that is not a previous install. Refuses prefixes outside `/tmp`, `/run` and `/dev/shm` unless `--persistent` (owner authorization required). The config has no default on purpose: the example config plays audio |
| `start.sh [--supervise]` | Starts the daemon in the background (`nohup`, stdin from `/dev/null`), pidfile in `run/`, log in `log/`; checks after 1 s that it is still alive and prints the log tail if not. `--supervise` runs it under `supervise.sh`. Refuses while `DIR/disable` exists, and with `--supervise` while `DIR/crashloop` exists |
| `supervise.sh` | Restart loop (started by `start.sh --supervise`): a daemon exit other than 0 restarts it after 1, 2, 4, ... s (at most `WEBOS_SAT_BACKOFF_MAX`, 300 s); `WEBOS_SAT_MAX_CRASHES` (5) exits within `WEBOS_SAT_CRASH_WINDOW` (600 s) write `DIR/crashloop` and stop it for good. Exit 0 (the daemon stopped on SIGTERM) ends it. Logs `SUPER` lines into the daemon log |
| `autostart.sh` | Boot-time start for the draft `init.d` hook: waits (`--delay`, 60 s), then starts `start.sh --supervise` unless `DIR/disable`, `DIR/crashloop` or the kill switch exists or `--check-firmware` fails; logs to `log/autostart.log`. Not wired into boot ([autostart design](design/autostart.md)) |
| `stop.sh` | Stops the supervisor first (so nothing restarts), then SIGTERM to the daemon, waits up to 10 s, then SIGKILL; removes the pidfiles. The pid is only trusted if `/proc/<pid>/exe` is the installed binary |
| `status.sh` | Running or not (exit 0 or 3), the installed build (`--version`), supervisor, markers and kill switch, RSS, fd and thread counts, log size, last log lines |
| `uninstall.sh` | Stops, then `rm -rf` of the prefix. Refuses `/`, empty or relative paths, `..` components, symbolic links, system directories (`/tmp` itself, `/var`, `/media/developer`, ...) and any directory without the marker file |

The installed scripts find their prefix from their own location (`DIR/bin/..`); from the repository they take `--prefix DIR`.

### Choosing the build (armv7 or aarch64)

`make package-tv` puts both builds next to `install.sh`. `install.sh --arch` (default `auto`) picks one:

| `uname -m` on the TV | `--arch auto` tries | Why |
| --- | --- | --- |
| `aarch64` | aarch64, then armv7 | 64-bit kernel; c3-a reports this for every process although its userland is 32-bit (P1) |
| `armv7l` | armv7 | 32-bit kernel: an aarch64 binary cannot run |
| `armv8l` | armv7 | 32-bit personality on a 64-bit kernel |
| anything else | armv7 | unknown: the build that runs on every supported TV |

- `--arch armv7` or `--arch aarch64` overrides the choice; `--binary FILE` installs one given binary without any choice.
- The candidate is copied into `DIR/bin` under a temporary name and must answer `--version` with its own target triple (`aarch64-unknown-linux-musl` or `armv7-unknown-linux-musleabi`) before it replaces the installed binary. An aarch64 build that does not run, or is missing, falls back to armv7 with a note. If no build runs, the install stops and an existing install is left unchanged (a new prefix is removed again).
- `install.sh` prints the choice (`uname -m: aarch64, --arch auto: installing the aarch64 build`) and `status.sh` shows the installed build.
- Not every LG TV has a 64-bit kernel; nothing here assumes one. Verified: the selection logic on a PC (`packaging/webos/tests/run.sh`, with stand-in binaries and `uname -m` replaced, and once with the real cross builds under qemu). **Pending:** a run of the aarch64 daemon on c3-a (see [Target decision](#target-decision)); until then use `--arch armv7` (or `TV_ARCH=armv7`) for the build verified there.

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

**No autostart.** These scripts do not hook into boot. The boot hook comes with the app package ([below](#install-the-app-package-on-a-rooted-tv-persistent)); its review is in the [autostart design](design/autostart.md). Nothing is installed in `/var/lib/webosbrew/init.d` on any TV yet, and `--persistent` has not been used on a TV.

**Markers** (files at the top of the prefix): `disable` (do not start; honoured by `start.sh`, `supervise.sh` and `autostart.sh`), `crashloop` (written by the supervisor when it gives up; remove it by hand after fixing the cause). The kill switch is separate: it keeps a running daemon's microphone closed.

The before/after result on c3-a is in [P1 ops](discovery/c3-a/P1-ops.md).

## Install the app package on a rooted TV (persistent)

The webOS app package (`make ipk`) is the permanent, updatable install: an app "Voice Satellite" in the launcher plus the satellite under `/var/lib/webos-satellite`, which keeps config, key, pairing state and logs across updates and reinstalls. Design, sources and what is verified: [packaging design](design/packaging.md). **Status: tested on a PC only; not yet installed on any TV** ([EXP-009](experiments/EXP-009-ipk-install-update.md)). Installing it is a persistent change: owner authorization per TV.

Needs: a TV rooted with the Homebrew Channel, its service elevated (HBC settings show root), SSH as root.

**Install** (from the PC):

```sh
make ipk                                   # target/ipk/io.github.pando85.voicesatellite_<version>_all.ipk
make tv-install-ipk TV=root@<host>         # scp to /tmp, dev/install, wait, delete the copy, print status
```

or by hand, with the command the Homebrew Channel README documents (it subscribes; stop it with Ctrl-C after `"installed"`):

```sh
scp target/ipk/io.github.pando85.voicesatellite_0.0.1_all.ipk root@<host>:/tmp/
ssh root@<host> "luna-send-pub -i 'luna://com.webos.appInstallService/dev/install' '{\"id\":\"com.ares.defaultName\",\"ipkUrl\":\"/tmp/io.github.pando85.voicesatellite_0.0.1_all.ipk\",\"subscribe\":true}'"
ssh root@<host> 'rm -f /tmp/io.github.pando85.voicesatellite_0.0.1_all.ipk'
```

`ares-install` also works once ares-cli is set up for the rooted TV as the HBC README describes. The package is not in a Homebrew Channel repository yet.

**Set up:** open "Voice Satellite" on the TV and press **Set up** (installs under `/var/lib/webos-satellite`, writes a config with a random device name, starts the supervised daemon, opens a 10-minute pairing window), then add the device in Home Assistant (ESPHome, the TV's address, port 6053); HA 2026.9 or newer sets the key. Over SSH instead (`APP` is the app directory, `/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite` on HBC 0.6; `luna-send -n 1 luna://com.webos.applicationManager/dev/listApps '{}'` shows `folderPath` elsewhere):

```sh
ssh root@<host> "sh $APP/payload/ctl.sh install && sh $APP/payload/ctl.sh start"
ssh root@<host> "sh $APP/payload/ctl.sh status"             # or: status --json
# own config or fixed key instead of the generated ones:
ssh root@<host> "sh $APP/payload/ctl.sh install --config /tmp/tv.toml --psk-file /tmp/noise_psk"
```

**Start at boot** (off by default): the app's "Start at boot: turn on", or `ctl.sh autostart on`. It writes `/var/lib/webosbrew/init.d/webos-satellite`; the daemon starts about 60 s after boot, never on an unvalidated firmware, after a crash loop, with the kill switch present or with `/var/lib/webos-satellite/disable` present. `ctl.sh autostart off` deletes the hook.

**Update:** bump `version` in the workspace `Cargo.toml`, `make ipk`, install the new ipk exactly as above. The running daemon is not touched by the install; the update is applied when the app is opened next (automatically), at the next boot if start at boot is on, or now with `ssh root@<host> "sh $APP/payload/ctl.sh install"` (stops, installs, keeps `etc/` and `log/`, starts again if it was running; never two daemons).

**Uninstall:**

```sh
ssh root@<host> "sh $APP/payload/ctl.sh uninstall"   # hook, daemon and binaries removed; config, key, logs KEPT
ssh root@<host> "sh $APP/payload/ctl.sh purge"       # everything in /var/lib/webos-satellite removed as well
make tv-remove-ipk TV=root@<host>                    # then remove the app (dev/remove)
```

The app has the same two buttons. Removing the app in the launcher or HBC without them runs nothing of ours: the boot hook and autostart then start nothing (the app is gone), and the data stays until `sh /var/lib/webos-satellite/bin/ctl.sh purge` (after `uninstall`: `rm -rf /var/lib/webos-satellite`) and `rm -f /var/lib/webosbrew/init.d/webos-satellite`.

**Rollback:**

| Goal | Action |
| --- | --- |
| Stop listening now | app "Microphone off", or `ctl.sh mic off` (`touch /tmp/webos-satellite.disable`); undo with `mic on` |
| Stop the satellite | app "Stop", or `ctl.sh stop` |
| No start at next boot | `ctl.sh autostart off`, or `touch /var/lib/webos-satellite/disable` |
| Go back to an older version | install the older ipk the same way, then `ctl.sh install`. Whether appinstalld accepts a downgrade is not verified; if not: `make tv-remove-ipk` (data kept), install the older ipk, `ctl.sh install` |
| Remove everything | `ctl.sh purge`, then `make tv-remove-ipk` |
| HBC failsafe mode after a boot | the hooks were skipped; delete `/var/lib/webosbrew/init.d/webos-satellite` over SSH or telnet and reboot ([autostart rollback](design/autostart.md#rollback)) |

### Network prerequisite

Home Assistant's ESPHome integration opens the connection: HA must be able to reach the TV on **TCP 6053** (or the configured `device.port`). With HA in a container or Kubernetes pod, check the path from HA's network to the TV, including router rules between network segments. On the owner's network this needed a router rule, added on 2026-11-02 ([P1 HA integration](discovery/c3-a/P1-ha-integration.md#permanent-entry-direct-connection-2026-11-02-morning)). A failed config flow shows `connection_error`, and HA's log shows `[Errno 111] Connect call failed`. Firewall details specific to one installation belong with that installation's infrastructure docs, not in this repository.

### Testing without HA reaching the device

`tools/esphome-fake-ha/run.sh` runs the protocol harness and an end-to-end check of the real binary (file mic, WAV sink, wake and push-to-talk runs, SIGTERM, log policy, and pairing: HA's provisioning flow, restart, reset file, window timeout and pair file). Everything stays on 127.0.0.1.

**Pairing against a real HA on the PC (2026-11-02).** HA 2026.10.4 from PyPI in a scratch venv (Python 3.14.7), onboarded through its REST API, with only `config`, `onboarding` and `http` in its configuration; the satellite (debug build of commit 849a1dd) with a file mic and `[pairing] enabled = true` on 127.0.0.1. Observed (label: `observed-on-other-device`, a PC, not a TV):

- The ESPHome config flow by host and port created the entry without asking for a key.
- HA's manager logged `Generating new encryption key`, set it over the zero-PSK session, and logged `Generated and stored encryption key`. The daemon stored the same key (mode 0600) that HA put in the config entry and in its `esphome.encryption_keys` storage.
- HA's next reconnect without a key got "requires encryption", HA's re-authentication found the stored key by itself, and about 5 s after the key was set HA was connected with Noise: entry loaded, assist satellite, media player, mute switch and the pipeline and wake word selects present, voice subscription active. No flow waited for the user.
- After a daemon restart HA reconnected with the key.
- After `reset-pairing` HA's old key failed and HA opened a re-authentication flow at `reauth_encryption_removed_confirm`. Confirming it made HA set the key from its storage (the same key) and reconnect.
- The key appeared in neither log (satellite at debug, HA's ESPHome logger at debug).

To repeat: `pip install homeassistant==2026.10.4` in a venv, start `hass -c <dir>` with that minimal configuration, create the first user with `POST /api/onboarding/users` and `/auth/token`, then `POST /api/config/config_entries/flow` with `{"handler": "esphome"}` and the host and port. Never point such a test at a production HA.

When HA cannot open a connection to the satellite (the situation on the owner's network as of 2026-11-02), `tools/esphome-fake-ha/ha_bridge.py` plays HA's ESPHome integration from the PC. It connects to the satellite and runs HA's real pipeline over the WebSocket API with an access token read from a file. `--synthetic-reply-every SECS` (with `--reply-on-intent-failure TEXT`) is a soak aid that answers every device run with a real TTS-only reply instead of running a wake word or STT. See its docstring and [P1 HA integration](discovery/c3-a/P1-ha-integration.md#bridge-stands-in-for-has-esphome-integration) for what it does and does not cover.

## CI

- [`.github/workflows/validate.yml`](../.github/workflows/validate.yml): documentation checks (`sh -n tools/probe/probe.sh`, `sh -n` of the packaging and lifecycle scripts, `python3 tools/check_docs.py`).
- [`.github/workflows/rust.yml`](../.github/workflows/rust.yml): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `packaging/webos/tests/run.sh`, `tests/ipk.sh` and `tests/app_smoke.js` on the host; then the armv7 and aarch64 cross-builds, ELF checks, a run of the armv7 binary under `qemu-user-static`, `make ipk` (built and checked, not uploaded), and the armv7 binary uploaded as a short-lived workflow artifact.

Actions are pinned to commit SHAs with the version in a comment. Update them deliberately.

## Quality gates before a PR

```sh
make lint test
python3 tools/check_docs.py
sh -n tools/probe/probe.sh
for f in packaging/webos/*.sh packaging/webos/init.d/* packaging/webos/tests/*.sh packaging/webos/tv/*.sh tools/lifecycle/*.sh; do sh -n "$f"; done
shellcheck -x -P SCRIPTDIR -s sh packaging/webos/*.sh packaging/webos/init.d/* packaging/webos/tests/*.sh packaging/webos/tv/*.sh   # if installed
make test-packaging              # ops scripts, ipk lifecycle, status app (node)
make ipk                         # cross builds, package and its structure check
tools/esphome-fake-ha/run.sh     # protocol and daemon e2e harness
tools/lifecycle/run.sh           # microphone gate and ops scripts on the host
```
