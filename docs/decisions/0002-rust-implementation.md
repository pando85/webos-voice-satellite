# ADR 0002: Implement the satellite in Rust

- **Status:** Accepted (2026-11-01, owner)
- **Scope:** implementation language and build approach of the on-TV daemon.
- **Related:** [ADR 0001](0001-esphome-native-api.md), [esphome-rust-satellite research](../research/esphome-rust-satellite.md)

## Context

- Userland on c3-a is **32-bit ARM EABI5, soft-float flag (`e_flags` 0x05000200), glibc 2.31, loader `/lib/ld-linux.so.3`**, on an aarch64 5.4.96 kernel. There is no hard-float loader and no aarch64 loader, so an `arm-linux-gnueabihf` or aarch64 build does not run ([D0-D1](../discovery/c3-a/D0-D1-runtime.md#32-bit-vs-64-bit-userland), [toolchain implications](../discovery/c3-a/D0-D1-runtime.md#implications-for-the-toolchain)).
- Resources are tight: about 750 MB available memory, nearly exhausted CMA, 3 of 4 cores online ([constraints](../discovery/c3-a/rooted-tv-improvements.md#constraints-not-changes)).
- No `gcc`, `strace`, `gdb` or `tcpdump` on the TV. Python 3.8, Node 12.22 and Perl exist but are poor fits for a long-running low-footprint daemon.
- The satellite must capture audio, parse a network protocol and run for days. Memory safety and a single static or near-static binary are valuable.

## Decision

Implement the satellite in **Rust**, as a Cargo workspace with separable crates for capture, preprocessing, ESPHome API, playback and lifecycle (matching the separation in [AGENTS.md](../../AGENTS.md)). The candidate target is `armv7-unknown-linux-gnueabi` (softfp) against a glibc 2.31 sysroot, or a static musl target. The exact target is a **hypothesis** that roadmap item 1.1 must verify on c3-a (including `readelf -A` attributes, which D1 did not read).

## Consequences

- A cross-compilation toolchain and CI are needed before any feature work (roadmap 1.1). No binary is committed.
- Audio access needs a binding to ALSA or PulseAudio; native C libraries must be cross-built for the soft-float target. Pure-Rust options reduce this burden.
- DSP components in the backlog (for example WebRTC AEC3, RNNoise) are C or C++ and would need bindings or Rust ports.
- Contributors need Rust knowledge.
- The binary size and RAM use must be measured on the TV, not assumed.

## Alternatives considered

- **Python 3.8 on the TV:** available now and good for prototypes (and used by the audio rig tooling on the host), but heavy for continuous audio, and dependency installation on the TV is restricted.
- **C or C++:** matches the vendor toolchain, with weaker memory safety and no gain in available libraries for the ESPHome protocol.
- **Node 12.22:** old runtime, not suited to audio processing at this footprint.
- **Go:** possible, but 32-bit ARM soft-float support and binary size are less certain. Not evaluated.

## Revisit when

- The 32-bit softfp target cannot be built or run reliably (roadmap 1.1 smoke test fails).
- A required DSP or wake word library has no workable Rust path and bindings prove too costly.
- Measured memory or CPU use on c3-a exceeds the Phase 3 resource limits.
