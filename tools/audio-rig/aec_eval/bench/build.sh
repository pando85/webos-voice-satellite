#!/bin/bash
# Throwaway AEC candidate benchmark builds (not part of the workspace; own target dirs).
#
#   build.sh host|armv7|aarch64 [candidate ...]
#
# Candidates (dirs under cands/): webrtc sonora aec3 aecrs fdaf rnnoise decibri speexsys
# Output: $AEC_BENCH_WORK/target-<tag>/<cand>/.../bench-<cand>   (default work dir ~/.cache/aec-bench)
#
# armv7 = armv7-unknown-linux-musleabi with the workspace's softfp rustflags, aarch64 = aarch64-unknown-linux-musl,
# both through cargo-zigbuild. Needs: rustup toolchain with both targets, cargo-zigbuild, zig, meson, ninja,
# cmake, clang (libclang for bindgen), network for crates and the abseil wrap.
#
# webrtc: the crate's `bundled` build runs meson natively (no cross file), so it cannot cross-compile. For armv7/aarch64
# this script cross-builds the upstream C++ library itself with a meson cross file, then builds a patched copy of
# webrtc-audio-processing-sys (patch_webrtc_sys.py) that links that prebuilt tree instead of running meson.
# For armv7 the C++ is compiled by system clang with `-mfloat-abi=softfp -mfpu=neon` against zig's musl/libc++
# headers, because zig cc for arm-linux-musleabi forces soft-float (float math becomes __aeabi_* calls).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
WORK=${AEC_BENCH_WORK:-$HOME/.cache/aec-bench}
TAG=${1:?host|armv7|aarch64}; shift
CANDS=("$@"); [ ${#CANDS[@]} -gt 0 ] || CANDS=(sonora aec3 aecrs fdaf rnnoise decibri webrtc)
ZIGLIB=${ZIG_LIB:-/usr/lib/zig}
mkdir -p "$WORK"

case $TAG in
  host) TRIPLE="" ;;
  armv7) TRIPLE=armv7-unknown-linux-musleabi ;;
  aarch64) TRIPLE=aarch64-unknown-linux-musl ;;
  *) echo "bad target"; exit 2 ;;
esac

softfp_wrappers() {  # compile-only -> clang softfp+NEON, anything else -> zig
  cat > "$WORK/sfp-cc.sh" <<EOS
#!/bin/sh
Z=$ZIGLIB
for a in "\$@"; do case "\$a" in -c|-S|-E) exec clang --target=arm-linux-musleabi -march=armv7-a -mfpu=neon -mfloat-abi=softfp -Wno-unused-command-line-argument -nostdinc -isystem \$Z/include -isystem \$Z/libc/include/arm-linux-musl -isystem \$Z/libc/include/generic-musl -isystem \$Z/libc/include/arm-linux-any -isystem \$Z/libc/include/any-linux-any "\$@";; esac; done
exec zig cc -target arm-linux-musleabi -mcpu=generic+v7a+vfp3-d32+thumb2 "\$@"
EOS
  cat > "$WORK/sfp-cxx.sh" <<EOS
#!/bin/sh
Z=$ZIGLIB
for a in "\$@"; do case "\$a" in -c|-S|-E) exec clang++ --target=arm-linux-musleabi -march=armv7-a -mfpu=neon -mfloat-abi=softfp -Wno-unused-command-line-argument -nostdinc++ -nostdinc -isystem \$Z/libcxx/include -isystem \$Z/libcxxabi/include -isystem \$Z/include -isystem \$Z/libc/include/arm-linux-musl -isystem \$Z/libc/include/generic-musl -isystem \$Z/libc/include/arm-linux-any -isystem \$Z/libc/include/any-linux-any -isystem \$Z/libunwind/include -D_LIBCPP_ABI_VERSION=1 -D_LIBCPP_ABI_NAMESPACE=__1 -D_LIBCPP_HAS_THREADS=1 -D_LIBCPP_HAS_MONOTONIC_CLOCK -D_LIBCPP_HAS_TERMINAL -D_LIBCPP_HAS_MUSL_LIBC=1 -D_LIBCXXABI_DISABLE_VISIBILITY_ANNOTATIONS -D_LIBCPP_DISABLE_VISIBILITY_ANNOTATIONS -D_LIBCPP_HAS_VENDOR_AVAILABILITY_ANNOTATIONS=0 -D_LIBCPP_HAS_FILESYSTEM=1 -D_LIBCPP_HAS_RANDOM_DEVICE -D_LIBCPP_HAS_LOCALIZATION -D_LIBCPP_HAS_UNICODE -D_LIBCPP_HAS_WIDE_CHARACTERS -D_LIBCPP_HAS_NO_STD_MODULES -D_LIBCPP_HAS_TIME_ZONE_DATABASE -D_LIBCPP_PSTL_BACKEND_SERIAL -D_LIBCPP_HARDENING_MODE=_LIBCPP_HARDENING_MODE_NONE "\$@";; esac; done
exec zig c++ -target arm-linux-musleabi -mcpu=generic+v7a+vfp3-d32+thumb2 "\$@"
EOS
  printf '#!/bin/sh\nexec zig ar "$@"\n' > "$WORK/zig-ar.sh"
  chmod +x "$WORK"/sfp-cc.sh "$WORK"/sfp-cxx.sh "$WORK"/zig-ar.sh
}

webrtc_prebuilt() {  # echoes the prebuilt tree dir (src, build, prefix)
  local pre=$WORK/wap-$TAG
  if [ ! -d "$pre/build" ]; then
    (cd "$HERE/cands/webrtc" && cargo +1.99.1 fetch >/dev/null 2>&1)
    local sys; sys=$(ls -d "$HOME"/.cargo/registry/src/*/webrtc-audio-processing-sys-2.1.0 | head -1)
    mkdir -p "$pre"; rm -rf "$pre/src"; cp -a "$sys/webrtc-audio-processing" "$pre/src"
    local cpu fam cc cxx
    if [ "$TAG" = armv7 ]; then
      softfp_wrappers; fam=arm; cpu=armv7; cc=$WORK/sfp-cc.sh; cxx=$WORK/sfp-cxx.sh
    else
      printf '#!/bin/sh\nexec zig cc -target aarch64-linux-musl "$@"\n' > "$WORK/a64-cc.sh"
      printf '#!/bin/sh\nexec zig c++ -target aarch64-linux-musl "$@"\n' > "$WORK/a64-cxx.sh"
      printf '#!/bin/sh\nexec zig ar "$@"\n' > "$WORK/zig-ar.sh"
      chmod +x "$WORK"/a64-*.sh "$WORK"/zig-ar.sh; fam=aarch64; cpu=aarch64; cc=$WORK/a64-cc.sh; cxx=$WORK/a64-cxx.sh
    fi
    cat > "$pre/cross.ini" <<EOC
[binaries]
c = '$cc'
cpp = '$cxx'
ar = '$WORK/zig-ar.sh'
strip = 'true'
pkg-config = 'false'

[host_machine]
system = 'linux'
cpu_family = '$fam'
cpu = '$cpu'
endian = 'little'
EOC
    # PKG_CONFIG_LIBDIR=/nonexistent: do not pick up the host's abseil; meson fetches the wrap
    PKG_CONFIG_LIBDIR=/nonexistent meson setup --prefix "$pre/prefix" --cross-file "$pre/cross.ini" \
      -Ddefault_library=static "$pre/build" "$pre/src" >"$pre/setup.log" 2>&1
    ninja -C "$pre/build" >"$pre/ninja.log" 2>&1
    ninja -C "$pre/build" install >"$pre/install.log" 2>&1
  fi
  if [ ! -d "$WORK/patched-sys" ]; then
    local sys; sys=$(ls -d "$HOME"/.cargo/registry/src/*/webrtc-audio-processing-sys-2.1.0 | head -1)
    python3 "$HERE/patch_webrtc_sys.py" "$sys" "$WORK/patched-sys"
  fi
  echo "$pre"
}

for c in "${CANDS[@]}"; do
  tdir=$WORK/target-$TAG/$c
  cd "$HERE/cands/$c"
  if [ "$TAG" = host ]; then
    CARGO_TARGET_DIR=$tdir cargo +1.99.1 build --release
  else
    extra=(); envs=()
    if [ "$c" = webrtc ]; then
      pre=$(webrtc_prebuilt)
      envs=(WAP_PREBUILT="$pre")
      extra=(--config "patch.crates-io.webrtc-audio-processing-sys.path=\"$WORK/patched-sys\"")
    fi
    env "${envs[@]}" CARGO_TARGET_DIR=$tdir \
      CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABI_RUSTFLAGS="-C target-feature=-soft-float,+vfp3,+neon" \
      cargo +1.99.1 zigbuild --release --target "$TRIPLE" "${extra[@]}"
  fi
done
