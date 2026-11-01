# Static tools for a rooted webOS TV (strace first, alsa-utils optional)

Reproducible build of fully static Linux binaries from pinned upstream source, so they run on the TV without depending on its libc. Nothing here touches a TV. Built binaries are never committed: they are written to `artifacts/` (git-ignored).

## What is built

| Target | Binaries | Purpose |
| --- | --- | --- |
| `armv7` (32-bit ARM EABI5, static musl) | `strace` | Primary tracer for the TV's 32-bit userland. Decodes 32-bit structures correctly. |
| `aarch64` (static musl) | `strace` | Fallback for 64-bit processes (and 32-bit ones with limited decoding, see below). |
| `aarch64` (optional, `WITH_ALSA=1`, default on) | `aplay`, `arecord`, `amixer` | Static alsa-utils linked against a static alsa-lib. |

`arecord` is byte-identical to `aplay`: alsa-utils selects capture mode from `argv[0]`, so keep the name `arecord`.

Not built: `pactl` / `parec`. The TV already ships them, and PulseAudio 17 only builds `libpulse` / `libpulsecommon` as shared libraries (static linking would require patching its Meson build). Skipped as not cheap and not needed. alsa-utils is also not required, since the TV already has `arecord`, `aplay` and `amixer`; the static copy is only useful as a libc-independent reference.

## Pinned versions

Defined in [versions.env](versions.env), the single source of truth.

| Component | Version | SHA-256 of tarball | Signature |
| --- | --- | --- | --- |
| alsa-lib | 1.2.16.1 | `f740db7f488255944ffd4428416ee3390a96742856916433df468c281436480e` | GPG, ALSA release key F04D F507 37AC 1A88 4C4B 3D71 8380 596D A6E5 9C91 |
| alsa-utils | 1.2.16 | `092399d5e8749a1d5e188e393157521cec4b75693b60ebb79bbce728cff2232c` | same ALSA key |
| strace | 7.3 | `529634c40910895a9a1f3c5c1e65da2ca9ed72fb158dfc4bcc89efdf05e39b72` | GPG, key 7BEC FE3A F7B2 80BB 52FF 77F1 04BA 4521 C996 DDE1 (signed with its subkey) |
| Build image | `alpine:3.23` pinned by index digest (gcc 15.2.0, musl) | | |

`build.sh` always checks the SHA-256 pins, and additionally verifies the detached GPG signature against the pinned primary-key fingerprint when `gpg` is installed (otherwise it refuses unless `ALLOW_NO_GPG=1`). The public keys are downloaded from the upstream projects at build time; the fingerprints are pinned here. Alpine packages used for the toolchain (`apk add`) are not version-pinned individually; the compiler version is recorded in `BUILDINFO`. Bit-for-bit reproducibility across Alpine package updates is therefore not guaranteed; use the SHA256SUMS of a given run for auditing.

## How to run

Host requirements: `docker`, `curl`, `sha256sum`, `tar`, recommended `gpg`, and qemu-user binfmt for arm64 and arm (for example `qemu-user-static`) because the build runs the target-architecture Alpine image natively under emulation. No root is needed beyond docker access.

```sh
./tools/static-tools/build.sh                       # armv7 + aarch64 (strace everywhere, alsa-utils on aarch64)
TARGETS=armv7 ./tools/static-tools/build.sh         # only the 32-bit strace
WITH_ALSA=0 ./tools/static-tools/build.sh           # skip alsa-utils
```

Emulated builds are slow (about 10 minutes for the aarch64 alsa build). Files: [build.sh](build.sh) (host driver: download, verify, run container), [container-build.sh](container-build.sh) (in-container build), [Dockerfile](Dockerfile), [versions.env](versions.env).

## Output

```text
artifacts/static-tools/aarch64/{strace,aplay,arecord,amixer,SHA256SUMS,BUILDINFO}
artifacts/static-tools/armv7/{strace,SHA256SUMS,BUILDINFO}
```

Source tarballs are cached in `artifacts/static-tools/src/`.

## Verification

```sh
cd artifacts/static-tools/aarch64 && sha256sum -c SHA256SUMS && file *
cd ../armv7 && sha256sum -c SHA256SUMS && file strace
```

Expected: `ELF 64-bit LSB executable, ARM aarch64 ... statically linked` and `ELF 32-bit LSB executable, ARM, EABI5 ... statically linked`. Binaries are non-PIE and have no interpreter. On a host with qemu-user binfmt they can be smoke-tested with `./strace -V`, `./aplay --version`, `./amixer --version`. `strace` cannot trace anything under qemu-user (ptrace is not implemented there), so only `-V` is a meaningful emulated test; tracing needs a real kernel.

## strace and the TV (32-bit userland on an aarch64 kernel)

- The aarch64 build enables the 32-bit ARM personality, so it can attach to 32-bit processes and prints `[ Process PID=N runs in 32 bit mode. ]`, but it was built without mpers (that needs a 32-bit compiler), so structures of 32-bit tracees are decoded with 64-bit layouts and strace warns about it. Syscall names and scalar arguments are right; struct contents may be wrong.
- The armv7 build runs as a 32-bit process and decodes 32-bit tracees correctly. Prefer it for the TV's 32-bit services. It is built hard-float (Alpine armv7) rather than soft-float; this is self-contained in a static binary and relies only on VFP, which an aarch64 kernel provides to 32-bit tasks. Whether it runs on the TV is unverified until tested.
- Neither was run on a TV. Static tracing needs ptrace permission for the target, and tracing native LG services can disturb them; follow the repository safety rules in `AGENTS.md`.

## Limitations of the static alsa-utils

- A static binary cannot `dlopen` ALSA plugins (`.so`). Anything routed through the `pulse` plugin (a `pcm.!default` of type `pulse`, `pulse` PCM or `ctl`) will fail with an error about the plugin or unknown PCM type. Only built-in plugin types work (`hw`, `plug`, `dmix`, `dsnoop`, `softvol`, `file`, `null`, ...). Use explicit hardware devices, or talk to the TV's own `pactl`/`parec` for Pulse paths.
- alsa-lib is configured with `--prefix=/usr`, so the compiled-in config location is `/usr/share/alsa/alsa.conf`. It uses the TV's own files if present. If they are missing, or reference plugins this binary cannot load, override at runtime:
  - `ALSA_CONFIG_DIR=/some/dir` makes alsa-lib look for `alsa.conf` there.
  - `ALSA_CONFIG_PATH=/some/alsa.conf` points at a complete top-level file (a minimal one with only `hw` devices is enough).
  - Device names such as `hw:CARD,DEV` bypass most config but still need `alsa.conf` to load.
- The binaries do not hard-code card indexes; list devices with `arecord -l` / `aplay -l` on the target (no recording is done by listing).
- Built without NLS, alsamixer, topology/UCM, rawmidi and seq support.

## Copy to the TV and clean up (documentation only, do not run without authorization)

Run only on an authorized TV. Everything stays in `/tmp` (tmpfs, gone after reboot), so no persistence or system change is made:

```sh
scp artifacts/static-tools/armv7/strace root@TV_IP:/tmp/strace-armv7
ssh root@TV_IP 'chmod 700 /tmp/strace-armv7 && /tmp/strace-armv7 -V'

# cleanup
ssh root@TV_IP 'rm -f /tmp/strace-armv7 /tmp/strace-aarch64 /tmp/aplay /tmp/arecord /tmp/amixer'
```

Check the checksum on the TV with `sha256sum /tmp/strace-armv7` against `SHA256SUMS`. Traces can contain audio data and identifiers: keep them local and out of git.
