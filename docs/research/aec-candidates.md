# AEC, noise suppression and AGC candidates for the satellite

- **Date:** 2026-11-02. **Scope:** PC only (host measurements, cross-builds, emulated runs). Nothing was run on the TV and no sound was played.
- **Question:** which echo canceller (and optional noise suppression or AGC) can the satellite realistically ship for the static `armv7-unknown-linux-musleabi` target, and at what CPU cost?
- **Related:** [backlog item 1, audio front-end](../project/backlog.md), [EXP-006](../experiments/EXP-006-aec-reference.md), [ADR 0002](../decisions/0002-rust-implementation.md), [build setup](../development.md), [P2 session 1 on c3-a](../discovery/c3-a/P2-session1-audible-and-echo.md) (the device evidence below comes from there).

Labels follow [AGENTS.md](../../AGENTS.md): **documented** (crate docs, source, licence files), **verified-on-host** (measured on the PC in this work), **verified-on-host, device recordings** (a host run on real c3-a recordings; still not a TV measurement), **hypothesis**. Synthetic numbers are labelled **synthetic** and are not device evidence.

## Summary

- Four of the evaluated echo cancellers build as static armv7 musl binaries with the repository's `cargo-zigbuild` setup without any change: `sonora` (pure Rust WebRTC port), `aec3` (pure Rust WebRTC port), `decibri-aec` (pure Rust) and `aec-rs` (speexdsp, C, fixed point). `nnnoiseless` (RNNoise) builds too. All of them also build for `aarch64-unknown-linux-musl`. **verified-on-host**
- The reference WebRTC C++ library through `webrtc-audio-processing` does **not** build for armv7 out of the box (its `bundled` build runs meson natively). It builds with a meson cross file plus a patched copy of the `-sys` crate (recipe in [`bench/build.sh`](../../tools/audio-rig/aec_eval/bench/build.sh)), and then runs. It is the heaviest option to maintain. **verified-on-host**
- On real c3-a recordings (mic plus the `hw:LGSIC,3` SoundEngineCapture reference, pink noise at TV volume 10), the three WebRTC AEC3 implementations reach **17.6 to 18.0 dB ERLE in 200 to 4000 Hz without any manual delay alignment**, above the 13.9 dB of the oracle NLMS used in the P2 analysis and above its 13.7 dB linear coherence bound (the AEC3 residual suppressor is nonlinear, so this ERLE is not purely linear cancellation). speexdsp gives 6.5 dB (10 dB with the reference raised by 20 dB), `decibri-aec` 14.1 dB (with a delay hint), a single-partition NLMS crate 6 to 10 dB. **verified-on-host, device recordings**, one recording each, no double talk.
- Host real-time factor is about 0.004 to 0.008 for every candidate (16 kHz mono, 10 ms frames, one core of a Ryzen 9 5950X). The armv7 cost on the TV is **unknown**: the command for a 30 s on-TV benchmark is below and was not run.
- **Recommendation:** evaluate `sonora` first on the real Phase 2 recordings (same AEC3 algorithm as WebRTC, plus NS, AGC2 and a high-pass filter, pure Rust, builds out of the box, BSD-3-Clause, no `unsafe` in our code). Keep the C++ `webrtc-audio-processing` build as the reference and fallback, and `decibri-aec` (Apache-2.0, needs a delay hint, no NS) as the second Rust candidate. speexdsp is the CPU fallback only. Details in [Recommendation](#recommendation).

## Reference signal in use (device evidence, for context)

From [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md) (c3-a, firmware 03.30.74, one TV and one room): `hw:LGSIC,1` SpeakerFeedback is not a linear copy of the speaker signal and is unusable as an AEC reference. `hw:LGSIC,3` SoundEngineCapture is post-volume with coherence 0.999 against the source, zero drift against the mic and a delay of 12.9 to 19.1 ms that changes on every open. `hw:LGSIC,2` MixerCapture is pre-volume with a 47.8 to 49.9 ms delay. The mic and the reference start on the same trigger. So the AEC does not need drift tracking on this TV, but it does need delay estimation at start and tolerance for a level that depends on the master volume (SoundEngineCapture pink noise: -47 dBFS RMS at volume 10).

## Candidates

All crate versions are the latest on crates.io on 2026-11-02. "Last push" is the GitHub repository's last push date. Licence texts were read from the published crate sources (**documented**).

| Candidate (crate) | What it is | Licence | Maintenance (**documented**) | Notes |
| --- | --- | --- | --- | --- |
| `webrtc-audio-processing` 2.1.0 + `-sys` | Rust wrapper over PulseAudio's repackaging of the WebRTC audio-processing C++ (AEC3, NS, AGC1/2, HPF, VAD) | BSD-3-Clause (Google) plus the WebRTC patent grant; vendored third-party parts under permissive licences (Abseil Apache-2.0 fetched as a meson wrap, pffft, Ooura FFT, `spl_sqrt_floor`) | active (release 2026-05-13, last push 2026-07-16, 332 stars) | C++17, needs meson, ninja, libclang (bindgen). Safe `Processor` API (the crate holds the `unsafe`) |
| `sonora` 0.2.0 (+ `sonora-aec3`, `-ns`, `-agc2`, `-fft`, `-simd`) | Pure Rust port of the full WebRTC audio-processing pipeline (AEC3, NS, AGC2, HPF) | BSD-3-Clause (WebRTC, Arun Raghavan and dignifiedquire copyrights) | young (first release 2026-02-11, last release 2026-07-29, last push 2026-10-17, 98 stars) | SIMD only for x86 and aarch64 (source: `cfg(target_arch)` in `sonora-simd` and `sonora-fft`), so armv7 runs the scalar paths. Needs rustc 1.91 or newer |
| `aec3` 0.4.0 | Pure Rust port of WebRTC AEC3 with a node-graph API, NS, AGC2, HPF | MIT OR BSD-3-Clause, plus a `PATENT` file (the WebRTC grant) | active (release 2026-09-16, 61 stars) | Heavier API (graph); `pipelines::linear` is the simple path. Explicit SIMD only for x86 in the part inspected |
| `aec-rs` 1.0.0 + `aec-rs-sys` (speexdsp) | Bindings that build speexdsp's MDF echo canceller and preprocessor with CMake | MIT (wrapper), speexdsp BSD-3-Clause (Xiph.Org, Jean-Marc Valin) | last release 2024-12-08; GitHub repository moved (HTTP 301); speexdsp itself last released in 2022 | Built in **fixed point** (`USE_FIXED_POINT` defaults to ON; `-DFIXED_POINT=1` in the generated flags). `Aec` holds raw pointers, so it is not `Send`: create and use it on one thread |
| `speexdsp` 0.1.2 / `speexdsp-sys` 0.1.2 | Older bindings | MIT | unmaintained (2022 and 2018) | Does not build (below). Not a candidate |
| `decibri-aec` 0.2.0 | Pure Rust echo canceller with its own delay search ("Tau" model) and residual suppression | Apache-2.0 | very young (created 2026-07-29, last push 2026-09-30, 14 stars) | `#![forbid(unsafe_code)]`, no dependencies beyond `thiserror`. AEC only, no NS or AGC. Needs a delay hint to be usable (below) |
| `fdaf-aec` 0.1.0 | Single-partition frequency-domain NLMS, power-of-two block size | MIT | unmaintained (2025-06-22, 17 stars) | Baseline only: filter length equals the FFT size |
| `nnnoiseless` 0.5.2 | Pure Rust RNNoise (noise suppression only, 48 kHz, model weights embedded) | BSD-3-Clause (Xiph, jneem) | active (release 2025-12-18, last push 2026-09-28, 370 stars) | Needs 48 kHz frames; the 16 kHz satellite path would have to resample 3x up and down |
| `rlx-aec` 0.2.11 | FDAF-NLMS plus residual suppression | **GPL-3.0-only** | new, 167 downloads | Not evaluated: GPL conflicts with the permissive stack |
| `corti-aec` 0.5.1, `echo_nlms` 0.1.0 | Offline NLMS tools | MIT; Apache-2.0 OR MPL-2.0 | very new, tiny | Not evaluated (offline use or toy scale) |
| `deep_filter` 0.2.5, `denoize` 0.90.2 | DeepFilterNet NS / classical NS | MIT/Apache-2.0; MIT | `deep_filter` last release 2022 | Not evaluated (NS only, not needed for the first decision) |

The repository has no licence file yet (the decision is listed as open in the [roadmap](../project/roadmap.md)). Every evaluated candidate is permissive (BSD, MIT or Apache-2.0); the only GPL one is `rlx-aec`. The WebRTC family carries a Google patent grant (documented in its `PATENT`/`PATENTS` files).

## Build results (armv7 static musl, our setup)

Setup: rustc 1.99.1, cargo-zigbuild 0.24.0, zig 0.16.0, `.cargo/config.toml` flags `-C target-feature=-soft-float,+vfp3,+neon`, as in [development.md](../development.md). Each candidate was built in a throwaway crate with its own `CARGO_TARGET_DIR`, not in the workspace. **verified-on-host**

| Candidate | host x86-64 | armv7 musl (zigbuild) | aarch64 musl (zigbuild) | Static armv7 bench binary (opt-level 3, includes the 0.37 MB harness) |
| --- | --- | --- | --- | --- |
| `sonora` | builds | **builds** | builds | 0.74 MB |
| `aec3` | builds | **builds** | builds | 1.04 MB |
| `decibri-aec` | builds | **builds** | builds | 0.42 MB |
| `aec-rs` (speexdsp, fixed point) | builds | **builds** (CMake through cargo-zigbuild) | builds | 0.42 MB |
| `fdaf-aec` | builds | **builds** | builds | 0.65 MB |
| `nnnoiseless` | builds | **builds** | builds | 0.78 MB |
| `webrtc-audio-processing` (`bundled`) | builds (26 to 30 s, uses the host's system Abseil) | **fails** as shipped; **builds with a manual cross build** | same | 1.35 MB (armv7), 1.25 MB (aarch64) |
| `speexdsp-sys` 0.1.2 | **fails** | **fails** | not tried | n/a |
| `speexdsp` 0.1.2 | **fails** (dependency resolution) | not tried | not tried | n/a |

The armv7 binaries run: each was executed under `qemu-arm-static` on a synthetic scenario and produced ERLE and near-end figures matching the host builds (qualitative check; emulated time is meaningless). All are statically linked ELF32 ARM EABI5.

Exact failures:

- **`webrtc-audio-processing` 2.1.0, `bundled`, armv7:** the `webrtc-audio-processing-sys` build script runs `meson setup` natively (its log says `Build type: native build`, `C compiler for the host machine: cc (gcc ...)`, `Host machine cpu family: x86_64`, and it finds the host's system Abseil). It then builds the C++ wrapper with the zig toolchain and stops with `.../out/include/webrtc-audio-processing-2/api/audio/audio_processing.h:31:10: fatal error: 'absl/base/config.h' file not found` (reported as `error occurred in cc-rs`). The crate has no cross-compilation path for `bundled`; the non-bundled mode links a dynamic library (`rustc-link-lib=dylib`), which a static musl binary cannot use.
- **`speexdsp-sys` 0.1.2, host:** `error: missing 'fn' or 'struct' for function or struct definition` followed by `error: could not compile 'speexdsp-sys' (lib) due to 1 previous error` (the pinned bindgen 0.37 emits code the current toolchain rejects).
- **`speexdsp-sys` 0.1.2, armv7:** `build.rs:27:34: called Result::unwrap() on an Err value: Error(PkgConfig(pkg-config has not been configured to support cross-compilation.` It also expects a system `libspeexdsp` (its `build` feature needs a source directory that is not in the crate).
- **`speexdsp` 0.1.2:** `failed to select a version for the requirement speexdsp-resampler = "^0.2"; version 0.2.0 is yanked`.
- **Not a build failure, but worth knowing:** `webrtc-audio-processing-sys` and `aec-rs-sys` cannot be in one dependency graph (both declare `links = "clang"` through different bindgen versions).

### What the manual WebRTC C++ cross build needs (verified-on-host)

1. Cross-build the vendored library with `meson setup --cross-file` (zig `cc`/`c++` wrappers, `cpu_family = 'arm'`, `PKG_CONFIG_LIBDIR=/nonexistent` so the host's Abseil is not used). Meson fetches the Abseil wrap from GitHub (a CI would have to vendor it; the wrap has a `source_fallback_url`). It compiles in about 17 s on 32 cores.
2. **Float ABI.** `zig cc -target arm-linux-musleabi` forces `+soft-float` and rejects softfp; with the flags cargo-zigbuild passes, C and C++ code compiles to `__aeabi_fmul`/`__aeabi_fadd` library calls (checked on a small dot product, and counting VFP instructions in the disassembly: 28 versus 3508 after the fix). The recipe therefore compiles with the system `clang` using `-mfloat-abi=softfp -mfpu=neon` against zig's musl and libc++ headers, and uses zig only to link. Softfp objects link with Rust's soft-float calling convention. This applies to any C or C++ dependency, not only WebRTC; speexdsp is unaffected because it is built in fixed point.
3. A patched copy of `webrtc-audio-processing-sys` (a four-edit patch to `build.rs`, [`bench/patch_webrtc_sys.py`](../../tools/audio-rig/aec_eval/bench/patch_webrtc_sys.py)) that links the prebuilt static library and the Abseil archives instead of running meson, wired in with `--config patch.crates-io...`.
4. A C++ standard library in the static binary (zig's libc++); about 1.1 MB more than the pure Rust candidates.

## Real recordings (verified-on-host, device recordings)

Recordings: c3-a P2 session 1 captures under the gitignored `devices/private/c3-a/P2/cap/`, read with [`tvcap.py`](../../tools/audio-rig/analysis/tvcap.py). Mic = channel 0 (16 kHz), reference = SoundEngineCapture (`hw:LGSIC,3`) left+right averaged and resampled 48 to 16 kHz. Candidates received mic and reference as captured, with **no manual delay alignment**; each had to find the 12.9 to 19.1 ms delay itself. ERLE is the mic-to-output power ratio in 200 to 4000 Hz over frames where the reference is active. Script: [`real_eval.py`](../../tools/audio-rig/aec_eval/real_eval.py). One recording per condition, no double talk, so near-end distortion is not measured here.

**Main result, `tap3-v10`:** HDMI pink noise at TV volume 10, segment 2.5 to 12 s. Mic -44.8 dBFS RMS, reference -47.2 dBFS, noise floor about -68 dBFS (so the noise-limited ERLE ceiling is about 23 dB), delay 12.9 ms, coherence 0.96, linear coherence bound 13.7 dB, oracle 4096-tap PBFDAF 13.9 dB.

| Candidate | ERLE, reference as captured (dB) | ERLE, reference +20 dB digital gain (dB) |
| --- | --- | --- |
| `webrtc-audio-processing` (AEC3) | 17.8 | 18.2 |
| `sonora` | 17.9 | 18.2 |
| `aec3` | 18.0 | 18.2 |
| `decibri-aec` (delay hint 20 ms, suppression off) | 14.2 | 14.2 |
| speexdsp (aec-rs), echo canceller only | 6.5 | 10.0 |
| speexdsp with its preprocessor (residual echo suppression plus NS) | 14.7 | 23.6 |
| `fdaf-aec` (mu 0.1) | 6.3 | 10.4 |

- The AEC3 family is already at 17.5 dB in the first second of the noise and stays at 17.4 to 19.3 dB, so convergence is fast. The ERLE exceeds the linear coherence bound because AEC3's residual suppressor is nonlinear and also lowers the noise floor; it is not a pure linear-cancellation figure. Whether it hurts speech is the double-talk question, open on the device.
- The reference gain barely matters for AEC3 at volume 10 (reference -47 dBFS). WebRTC AEC3 ignores references it considers inactive (a level threshold, **hypothesis** for the exact value, near -50 dBFS RMS), and the SoundEngineCapture level follows the master volume (it is post-volume: +13.3 dB from volume 10 to 20; the SpeakerFeedback tap showed about 14 dB between volumes 5 and 10), so low volumes should be tested with and without digital gain.
- The fixed-point speexdsp is level sensitive: the +20 dB reference gain improved its canceller by 3.5 dB. Its preprocessor does the heavy lifting here, and it is also the component that attenuates quiet near-end speech in the synthetic runs (below).
- The same ordering holds on the pre-volume MixerCapture tap (`tap2-v10`, delay 47.8 ms, reference -20.8 dBFS): AEC3 family 17.4 to 17.7 dB, `decibri-aec` 14.4 dB, speexdsp 5.5 dB (16.6 dB with preprocessor), oracle PBFDAF 14.6 dB. The AEC3 family found the 47.8 ms delay without being told.
- The short chirp captures at volume 20 (`tap3-v20a`, `tap3-v20b`, 5 s of chirps, reference -39.5 dBFS) gave 14 to 28 dB for the AEC3 family depending on the implementation and the reference gain, with an oracle PBFDAF of only 4.4 dB. They are too short and non-stationary to rank the implementations, and are not used for the recommendation.
- Content at volume 6 (the TV's normal level) is noise limited (mic echo about -64 dBFS against a -68 dBFS floor, per P2), so no ERLE can be shown there; those captures also used SpeakerFeedback, not SoundEngineCapture. They were not used.

## Synthetic results (not device evidence)

Generator: [`synth_scenario.py`](../../tools/audio-rig/aec_eval/synth_scenario.py); metrics and runner: [`run_eval.py`](../../tools/audio-rig/aec_eval/run_eval.py). **Synthetic.** The reference is TV-like content from the gitignored P2 content set (film dialogue, music with vocals) or pink noise, at -33 dBFS active RMS. The mic is the reference through a synthetic room response (a dominant direct path with about 78 % of the energy, about 10 % in 5 to 50 ms and 8 % in 50 to 250 ms, the split measured on c3-a, plus speaker and mic colouring), a delay of 17 to 80 ms, optional clock drift and optional tanh speaker compression, plus near-end speech (TTS corpus utterances at -43 dBFS active RMS) and a -66 dBFS noise floor. 40 s at 16 kHz in three phases: far-end only (0 to 15 s), double talk (15 to 30 s), near-end only (30 to 40 s, reference silent). Everything runs at 16 kHz mono with 10 ms frames, as the satellite would.

Caveats: the near-end level (-43 dBFS) and echo level are 15 to 20 dB louder than the raw TV mic (the D3 rig speech was about -60 dBFS), the echo path is one random draw per scenario, and double talk is only 15 s of speech. Near-end metrics use the magnitude-squared coherence between the output and the clean near-end speech in 200 to 4000 Hz, so benign filtering such as a high-pass phase shift is not counted as distortion; "SDR" is explained over unexplained output power, "gain" is the explained output power against the near-end power. The mic alone scores 1.5 to 2 dB SDR in double talk and 17 to 20 dB in near-end-only.

**Steady-state ERLE (dB, 200 to 4000 Hz, last 7 s of the far-end-only phase; the noise-limited ceiling is 25 to 31 dB):**

| Candidate | S1 speech, 17 ms | S2 speech, 17 ms, +100 ppm | S3 speech, 80 ms | S4 music, 40 ms | S5 pink, 40 ms, +200 ppm | S6 speech, 40 ms, nonlinear |
| --- | --- | --- | --- | --- | --- | --- |
| `webrtc-audio-processing` | 17.7 | 17.2 | 17.8 | 15.4 | 11.9 | 15.6 |
| `sonora` | 24.3 | 22.1 | 24.8 | 21.2 | 11.8 | 21.6 |
| `aec3` | 24.0 | 21.9 | 24.1 | 20.7 | 11.9 | 21.0 |
| speexdsp, canceller only | 17.8 | 10.1 | 16.2 | 18.1 | 6.8 | 11.4 |
| speexdsp + preprocessor | 28.1 | 19.5 | 24.0 | 25.1 | 26.6 | 18.8 |
| `decibri-aec`, hint 20 ms | 22.5 | 4.5 | 19.1 | 23.5 | 1.1 | 9.0 |
| `fdaf-aec` | 10.7 | 6.8 | 0.2 | 2.5 | 0.7 | 1.8 |

**Near-end behaviour (S1; mic alone: double-talk SDR 2.0, near-end-only SDR 16.9).** "DT" is double talk, "C" is near-end only with nothing to cancel.

| Candidate | DT near-end gain (dB) | DT SDR (dB) | C near-end gain (dB) | C SDR (dB) | Added capture latency (ms) |
| --- | --- | --- | --- | --- | --- |
| `webrtc-audio-processing` (AEC) | -1.1 | 8.9 | 0.0 | 16.8 | 8 |
| `webrtc-audio-processing` + NS | -3.3 | 8.4 | -0.3 | 19.1 | 14 |
| `sonora` (AEC) | -1.5 | 9.3 | -0.1 | 17.0 | 9 |
| `sonora` + NS | -3.8 | 7.2 | -0.4 | 19.1 | 15 |
| `aec3` (AEC) | -1.3 | 9.6 | 0.0 | 16.8 | 8 |
| `aec3` + NS | +0.5 | 9.5 | +1.3 | 17.4 | 14 |
| speexdsp, canceller only | -2.5 | 13.1 | -2.2 | 15.4 | 1 |
| speexdsp + preprocessor | **-11.8** | 8.6 | **-12.7** | 10.3 | 11 |
| `decibri-aec`, hint 20 ms | 0.0 | 20.2 | 0.0 | 16.9 | 0 |
| `decibri-aec`, no hint | -2.3 | 1.3 | -2.2 | 1.6 | 1 |
| `fdaf-aec` | 0.0 | 10.3 | 0.0 | 16.9 | 30 (block buffering) |
| `nnnoiseless` (NS only) | -1.2 | 7.6 | -0.1 | 21.7 | 11 |

Observations:

- **AEC3 family (three implementations):** the same behaviour: 15 to 25 dB steady ERLE, speech preserved in near-end-only (C SDR equal to the mic), about 1 to 1.5 dB near-end loss in double talk, and a fixed 8 ms capture latency (14 to 15 ms with NS). The pure Rust ports score higher than the C++ library on S1 to S4 and S6 (24 versus 17.7 dB), and equal on real recordings (17.6 to 18.0 dB); the cause (different upstream versions or tuning) is not known. 200 ppm of drift with noise as the reference cut all three to about 12 dB; on c3-a the drift is zero, so this stress case is outside the expected conditions.
- **WebRTC NS** raises the ERLE above the noise-limited ceiling because it lowers the -66 dBFS noise floor, and costs about 2 dB more near-end level in double talk. Its effect on wake word or STT accuracy was not measured. "+AGC" with the crates' default AGC2 settings (adaptive digital gain off, fixed gain 0 dB) produced output identical to "+NS", so **AGC was not evaluated**.
- **speexdsp's preprocessor** attenuates the quiet near-end speech by 8 to 13 dB when it is the only signal: at these levels (-43 dBFS in 16-bit integers) it treats speech as noise. Whether this occurs after the satellite's +24 dB front-end gain was not tested. The canceller alone is gentle (-2 dB) but needs the level to be reasonable.
- **`decibri-aec`:** without a delay hint it attenuated near-end-only speech by 2 dB and produced SDR 1.6 (it was still in its acquisition state: **hypothesis**); with a 20 ms hint it did not touch near-end speech at all (0.0 dB, SDR 20) and cancelled well, so a hint is mandatory in practice. We would have one (a delay measured at start); it hit 14.2 dB on the real recordings. It does not track clock drift (S2, S5), which does not matter on c3-a.
- **`fdaf-aec`** (single partition, 64 ms filter, 32 ms blocks): weak and delay-limited. It is a baseline, not a candidate.
- **RNNoise** removes noise but not echo (ERLE 7 dB on S1, from noise and speech-like content suppression), and keeps near-end speech (-0.1 dB). It needs 48 kHz frames, so the figures include a 63-tap resampler in the harness. Its effect on the wake word is not measured.

## CPU, memory, size

Measured on the host, **verified-on-host**: AMD Ryzen 9 5950X, one thread, x86-64 builds with rustc 1.99.1 at opt-level 3, LTO, 40 s of audio in 10 ms frames, best of 8 runs. "RTF" is processing time divided by audio time; 0.005 is 0.5 % of one core.

| Candidate and mode | RTF | | Candidate and mode | RTF |
| --- | --- | --- | --- | --- |
| `webrtc-audio-processing` AEC | 0.0039 | | speexdsp canceller (200 ms filter) | 0.0049 |
| `webrtc-audio-processing` AEC + NS | 0.0047 | | speexdsp + preprocessor | 0.0064 |
| `sonora` AEC | 0.0046 | | `decibri-aec` | 0.0071 |
| `sonora` AEC + NS | 0.0053 | | `fdaf-aec` | 0.0003 |
| `aec3` AEC | 0.0044 | | `nnnoiseless` (including the 3x resampling) | 0.0081 |
| `aec3` AEC + NS + AGC2 | 0.0073 | | | |

- **The workspace release profile costs CPU.** The root `Cargo.toml` uses `opt-level = "z"`. Rebuilding three Rust candidates at `z` instead of 3 on the host raised the RTF from 0.0047 to 0.0071 (`sonora`), 0.0044 to 0.0078 (`aec3`) and 0.0079 to 0.0158 (`decibri-aec`), at a size saving of 10 to 20 %. A DSP crate should override this (`[profile.release.package.<crate>] opt-level = 3`). **verified-on-host**
- **Host to TV is not extrapolated.** `sonora` and `aec3` have explicit SIMD for x86 and, for `sonora`, aarch64, but not for armv7 (**documented**, source); the C++ library and speexdsp have NEON or integer code paths for ARM. So the host figures flatter the Rust ports relative to the TV (**hypothesis**). Expect the TV to be several times slower than the PC per core; the actual factor is a Phase 3 measurement.
- **Memory:** a rough upper bound, with the harness and its file buffers included, is 15 to 18 MB peak resident for every engine; the difference between the C++ library and the smallest engine was under 2 MB. The satellite's `limits.max_rss_mb` is 64 MiB. A precise figure needs the TV run. **verified-on-host**, rough.
- **Size:** the static armv7 benchmark binaries above include a 0.37 MB harness (a pass-through engine builds to 0.37 MB); subtracting it, the DSP part is about 0.05 MB (`decibri-aec`, speexdsp), 0.37 MB (`sonora`), 0.66 MB (`aec3`) and 0.98 MB (C++ library with libc++). Compare with the workspace's size-tuned release builds at `opt-level = "z"`.

### Benchmark on the TV (prepared, not run)

Do not run these without the owner's authorization ([AGENTS.md](../../AGENTS.md)). The binaries have no audio input or output, open no device and read no files except `/proc/self`; they generate deterministic synthetic signals (band-limited noise as the reference, a 40 ms delayed FIR echo of it as the mic), so no recording is involved. The generation takes a few seconds and is not timed.

```sh
# on the PC: build for armv7 (about 1 minute; the work dir is outside the repository)
export AEC_BENCH_WORK=$HOME/.cache/aec-bench
tools/audio-rig/aec_eval/bench/build.sh armv7 sonora aec3 aecrs decibri webrtc
B=$AEC_BENCH_WORK/target-armv7
sha256sum $B/*/armv7-unknown-linux-musleabi/release/bench-*   # record the hashes
scp $B/sonora/armv7-unknown-linux-musleabi/release/bench-sonora <tv>:/tmp/    # one binary at a time, as needed

# on the TV, quiet system, note `cat /proc/loadavg` first
nice -n 10 /tmp/bench-sonora --synthetic 30 --mode aec            # as fast as possible: rtf_best
/tmp/bench-sonora --synthetic 30 --realtime --mode aec            # paced at 10 ms: cpu_pct_of_realtime = share of one core
/tmp/bench-sonora --synthetic 30 --realtime --mode aec+ns         # with noise suppression (webrtc, sonora, aec3)

# rollback
rm -f /tmp/bench-*
```

The `RESULT` line reports `rtf_best`, `cpu_s`, `cpu_pct_of_realtime` and `peak_rss_kb`. Repeat three times, and also with a second core busy, to see variation. `--mode` values: `aec`, `aec+ns`, `aec+ns+agc` (speexdsp: `aec+ns` enables its preprocessor; `decibri`: `nosup+hint`).

## What shipping each option would need

- **Our code stays free of `unsafe`.** Every candidate above can be used through a safe API: `sonora`, `aec3`, `decibri-aec` and `nnnoiseless` are Rust; `webrtc-audio-processing` has a safe `Processor` (the `unsafe` is inside the crate); `aec-rs` has a safe `Aec`, which is not `Send`, so it must be created and used on the thread that runs the front-end. The workspace rule (no `unsafe` outside the capture crate's `sys.rs`) is therefore met by depending on these crates. Raw bindings such as `speexdsp-sys` would need our own FFI crate that opts out of the workspace `forbid(unsafe_code)` lint (a crate that does not set `[lints] workspace = true`), the same exception as `sys.rs`.
- **The C++ route** additionally needs: meson, ninja, libclang and a C++17 toolchain in the build and in CI; the Abseil wrap vendored for offline builds; the softfp clang wrapper above (or a different cross toolchain); a maintained fork or patch of `webrtc-audio-processing-sys`; and an amendment to the "pure Rust external crates" comment in the workspace `Cargo.toml` and to the consequences in [ADR 0002](../decisions/0002-rust-implementation.md) (which already anticipates C++ DSP bindings).
- **Placement in the front-end.** The AEC must see the mic before the +24 dB fixed gain and the S16 conversion: P2 showed the satellite's own replies saturate that S16 stream from volume 15. The reference is 48 kHz stereo (SoundEngineCapture) and needs downmixing and 3:1 resampling to the 16 kHz mic rate, plus delay estimation after every open (12.9 to 19.1 ms), with the mic and reference opened together and started on one trigger as in the P2 rig. The current [`crates/dsp`](../../crates/dsp) front-end has no reference input and no resampler; adding AEC means a second capture stream and an f32 path.

## Recommendation

**Evaluate on real Phase 2 recordings, in this order:**

1. **`sonora`** (AEC + HPF, optional NS). Same AEC3 algorithm as the C++ reference, equal ERLE on the real recordings (17.9 dB), a pure Rust `cargo zigbuild` for both TV targets with no extra toolchain, BSD-3-Clause, no `unsafe` in our code, NS and AGC2 in the same API. Risks: a young crate (first release February 2026), scalar code on armv7 (CPU on the TV unknown), and its fidelity to the C++ library is documented by the crate but not verified here.
2. **`webrtc-audio-processing` C++ as the reference and fallback**, built with the cross recipe. Use it to confirm that `sonora` matches on every Phase 2 recording, and as the production option if `sonora` is too slow on armv7 (it has NEON) or diverges. Cost: the build and maintenance burden listed above.
3. **`decibri-aec`** as the second Rust candidate, because it leaves near-end speech untouched in double talk (0.0 dB in the synthetic run), needs no drift tracking on this TV, and is small, Apache-2.0 and `forbid(unsafe_code)`. It needs a delay hint (the delay measured at start) and gave 14.2 dB on the real recording, 3.6 dB below AEC3. No NS or AGC.
4. **speexdsp** only as a low-CPU fallback: it is fixed point and builds out of the box, but it gave 6.5 dB (10 dB with a +20 dB reference) on the real recording, and its preprocessor attenuated quiet near-end speech. Do not enable its preprocessor by default.
5. **`nnnoiseless`** as the NS to test (not as an AEC): measure its effect on wake word false rejects and WER on the Phase 2 corpus, with and without it, before enabling anything. AGC: not evaluated here, and not needed until the front-end gain policy is settled.

**What the Phase 2 evaluation should add** (none of this was measured here): double talk on the TV with real near-end speech; the P2 corpus (wake word positives and negatives, TV content at volumes 10 and 20) run through the candidates, scored by false rejects, false accepts per hour and WER with and without each stage; ERLE at low volumes with and without reference gain; and the on-TV CPU and memory figures from the prepared benchmark.

## Open questions

- CPU and memory on the TV (armv7, scalar versus NEON paths, the `opt-level` override).
- Double-talk damage on real recordings, and the effect of AEC3's residual suppressor and of NS on wake word and STT.
- Does `sonora` track the C++ library on more recordings and levels? The ports beat the C++ library synthetically but tie on the one real recording.
- AEC3's activity threshold for a quiet SoundEngineCapture reference (volume 5 to 10): is digital reference gain required, and what gain?
- Does the SoundEngineCapture delay stay fixed for sessions longer than 3 minutes and across volume or sound-mode changes (a P2 open question); what happens to the AEC when it jumps? A delay hint from a start-up estimate versus a free-running estimator.
- Nonlinear echo at higher volumes (only a synthetic tanh stress here).
- Whether the satellite's own reply (the main echo source during barge-in) is better handled by using the played TTS stream as an extra digital reference.
- The repository licence is undecided; all recommended candidates are BSD-3-Clause or Apache-2.0 (the WebRTC family adds a patent grant).

## Reproduce

All code is under [`tools/audio-rig/aec_eval/`](../../tools/audio-rig/aec_eval). No audio, recordings or binaries are committed (`*.wav`, `*.raw` and `devices/private/` are gitignored).

```sh
tools/audio-rig/aec_eval/bench/build.sh host            # or armv7 | aarch64; add candidate names to limit
python3 tools/audio-rig/aec_eval/synth_scenario.py --out-dir $WORK/scen
python3 tools/audio-rig/aec_eval/run_eval.py --scen-dir $WORK/scen --work-dir $WORK/out \
  --bin sonora=$AEC_BENCH_WORK/target-host/sonora/release/bench-sonora,aec ...
python3 tools/audio-rig/aec_eval/real_eval.py --work-dir $WORK/out --bin sonora=...,aec \
  devices/private/c3-a/P2/cap/tap3-v10:2.5:12
```
