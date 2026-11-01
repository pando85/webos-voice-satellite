# c3-a (LG C3): runtime smoke test of the Rust workspace (roadmap 1.1)

- Date: 2026-11-01
- Device: c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74, kernel `5.4.96-307` aarch64, 32-bit ARM userland ([D0-D1](D0-D1-runtime.md)).
- Naming note: `P1-*` logs are Phase 1 validation runs; `D<n>` logs are roadmap discovery steps. This one is the runtime smoke test of roadmap item 1.1.
- TV state: powered on, OS fully up (uptime about 1 h at the start). Screen state and foreground app were not read. LG hands-free state was not changed or read.
- Method: two binaries built from this repository were copied to `/tmp` (tmpfs), run with `--version` and `--self-test`, and deleted. Everything else was read-only inspection of `/proc` and `iptables -S`/`-L -n`. No audio device, no ALSA/DAPM/debugfs path, no socket creation by us, no writes outside `/tmp`, no service change, no reboot, no persistence.
- Raw output was not stored in the repository. Hostnames, addresses and identifiers are omitted here.
- Build and deploy procedure: [development](../../development.md). Builder: rustc 1.99.1, cargo-zigbuild 0.24.0, zig 0.16.0, release profile (`opt-level=z`, LTO, `strip`, `panic=abort`).
- Labels: `verified-on-target` means observed directly on c3-a in this run. `inferred` means deduced from verified facts. `unknown` means not established.

## Binaries that run

| Binary | Build | ELF (checked on the host before copying) | Size | sha256 (first 16 hex) | Result on c3-a |
| --- | --- | --- | --- | --- | --- |
| armv7 | `armv7-unknown-linux-musleabi`, `-C target-feature=-soft-float,+vfp3,+neon` | ELF32 ARM EABI5, statically linked, `e_flags` `0x5000200`, no `PT_INTERP` | 338,464 B | `04d7ee812aff502d` | `--version` and `--self-test` exit 0 |
| aarch64 | `aarch64-unknown-linux-musl` | ELF64 AArch64, statically linked | 321,984 B | `3cc58d4f4e9860b8` | `--version` and `--self-test` exit 0 |

**Finding.** Both static binaries run from `/tmp` on c3-a. The armv7 binary's `e_flags` (`0x5000200`, EABI5 soft-float ABI) equal the value D1 read from the TV's own binaries. The aarch64 result answers open question 1 of the [ESPHome/Rust research](../../research/esphome-rust-satellite.md#open-questions): the C3 kernel executes 64-bit EL0 ELF files, even though the userland has no aarch64 loader (a static binary needs none).

**Evidence.** The checksums on the TV matched the host before running. `float_check=155.0000` was identical on host (under `qemu-arm-static`) and on the TV, so VFP-compiled code produced the same result and raised no `SIGILL`. The armv7 build reports `machine=aarch64` from `uname` on the TV (a 32-bit process on an aarch64 kernel without a personality mask); software must not use `uname -m` to detect the process ABI, and should use the compile-time target.

**Confidence.** verified-on-target for "runs", sizes and `float_check`. Not tested: sustained load, threads, sockets, any audio path.

**Residual unknowns.** Whether the armv7 `+neon` code paths are used by later DSP code on the Cortex-A76 AArch32 mode is not shown (the self-test uses scalar VFP only; NEON instructions were not executed). The prebuilt `std` is soft-float, so the linked file's `readelf -A` shows `Tag_FP_arch: VFPv3-D16` and no NEON tag.

## Memory and limits from `--self-test`

| Item | armv7 | aarch64 |
| --- | --- | --- |
| `VmRSS` (= `VmHWM`) | 148 kB | 152 kB |
| `VmSize` / `VmPeak` | 900 kB / 960 kB | 904 kB / 964 kB |
| `VmData` | 424 kB | 452 kB |
| Threads | 1 | 1 |
| Page size | 4096 | 4096 |

Limits seen by a process started from an SSH session as root: stack 8 MiB soft, `nofile` 524288, `nproc` 5394, `memlock` 65536 bytes (64 KiB), `core` soft 0, address-space and data unlimited.

**Confidence.** verified-on-target. These numbers are for an idle process that prints and exits; they are not a budget for the daemon. The `memlock` limit of 64 KiB is relevant for any future `mlock` use. A daemon started by the Homebrew startup hook may see different limits (unknown).

## UDP 5353 (mDNS)

**Finding.** UDP 5353 is **already bound by several processes on all addresses, so it is shared**. On IPv4 the bound sockets belong to `mdnsd` (uid 99), `avahi-daemon` (uid 101) and `iconnectivity` (uid 0). On IPv6 they belong to `mdnsd` and to `chip-service` (uid 0, three sockets). All bind the wildcard address. The kernel allows this only with address or port reuse flags, which the `/proc` view does not show, so **how** each process shares the port was not read.

**Evidence.** `/proc/net/udp` and `/proc/net/udp6` entries with local port `0x14E9`, mapped from socket inode to process through `/proc/<pid>/fd` in a single pass (`readlink`). `mdnsd` re-created its sockets between two snapshots (different inode numbers, same process), so the socket set changes at runtime.

**Implications (inferred).**
- A second mDNS responder inside the satellite needs `SO_REUSEADDR` and `SO_REUSEPORT` (and multicast group membership) to coexist, and must not assume it receives unicast responses.
- Advertising `_esphomelib._tcp` ourselves duplicates `avahi-daemon` and `mdnsd` on the same host; adding a service through avahi's configuration would need a file write outside `/tmp`, which is not allowed in this step. Home Assistant can add the device by IP address, which avoids mDNS for the first experiments.

**Confidence.** verified-on-target for the binders and addresses. unknown for the reuse flags and for whether LG's `mdnsd` answers queries for foreign services.

## TCP 6053 (ESPHome native API)

**Finding.** TCP port 6053 (`0x17A5`) is **free**: no socket in any state, IPv4 or IPv6.

**Evidence.** `/proc/net/tcp` and `/proc/net/tcp6` have no entry with local port `0x17A5`. For context, the only listener on IPv6 besides loopback DNS is SSH; IPv4 has about twenty listeners, several on all addresses.

**Confidence.** verified-on-target at one instant. Nothing was bound or connected by us, so whether a listener on 6053 is **reachable from the LAN** is **unknown** and is left to the first experiment with the API server (roadmap 1.3/1.5). ADR 0001 requires the TV to accept inbound connections.

## Firewall

**Finding.** No inbound filtering is configured that could block TCP 6053 or mDNS.
- IPv4 `iptables` (v1.8.4, legacy): tables `filter`, `nat` and `mangle` exist, all with policy ACCEPT and **no rules** (`INPUT`, `FORWARD`, `OUTPUT` counters show traffic accepted by policy).
- IPv6: `ip6tables` cannot initialize (`ip6_tables` kernel module not found), so there is no IPv6 filter table.
- `nft` and `ss` are not installed. `/proc/net/ip_tables_names` lists only `nat`, `mangle`, `filter`; no `nf_tables` module is loaded (`/proc/modules`).

**Evidence.** `/proc/net/ip_tables_names`, `iptables -t filter|nat|mangle -S`, `iptables -L -n -v`, `ip6tables -S` (error), `/proc/modules`. The tables were already loaded, so the listing did not load kernel modules.

**Confidence.** verified-on-target at one instant. Not established: firewalling by other mechanisms (for example eBPF or LG middleware), the state after reboot or after the network-profile changes, and behaviour in standby or Always Ready states (phase 3, [D5](../../project/roadmap.md#phase-3-lifecycle-and-robustness)).

## Cleanup and rollback

Both binaries were deleted from `/tmp` after the runs (verified: no matching files, no matching processes). Nothing else was changed; the SSH ControlMaster socket lived on the host only.

## Consequences for the roadmap

- Roadmap 1.1 exit signal is met for the build and run part: a binary built by the repository runs from `/tmp` on c3-a without writes elsewhere. The candidate `armv7-unknown-linux-gnueabi` in the roadmap is not needed; the static musl target runs, and ADR 0002 lists static musl as an acceptable target.
- Open for 1.1 follow-up: confirm the choice of default target (armv7 versus aarch64) after the first audio and crypto dependencies are tried; `ring` or `aws-lc-rs` on armv7 musl is still unverified (research open question 10).
- Open for 1.3: reachability of 6053 from the LAN, and how Home Assistant discovers the device (mDNS versus manual IP).
