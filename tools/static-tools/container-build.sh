#!/bin/sh
# Runs INSIDE the Alpine (musl) container for one target. Not meant to be run on
# the host. Inputs : /src (read-only verified tarballs), /tools (this directory).
# Outputs: static ELF binaries plus BUILDINFO in /out.
# Env    : TARGET (aarch64|armv7), WITH_ALSA (1|0), JOBS, SOURCE_DATE_EPOCH.
set -eu

. /tools/versions.env

TARGET="${TARGET:?TARGET not set}"
WITH_ALSA="${WITH_ALSA:-1}"
JOBS="${JOBS:-$(nproc)}"
BUILD=/build
OUT=/out
mkdir -p "$BUILD" "$OUT"

# Reproducibility knobs.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1700000000}"
export TZ=UTC LC_ALL=C
# Non-PIE fully static executables (no interpreter, no libc dependency on the target).
export CFLAGS="-Os -g0 -fno-pie -ffile-prefix-map=$BUILD=. -fno-ident"
export LDFLAGS="-static -no-pie"

extract() { tar -C "$BUILD" -xf "$1"; }

echo "== strace ${STRACE_VERSION} ($TARGET)"
extract "/src/strace-${STRACE_VERSION}.tar.xz"
cd "$BUILD/strace-${STRACE_VERSION}"
# mpers needs a second (32-bit) compiler, which this container does not have:
# on aarch64 the arm (32-bit) personality is still compiled in, but structures of
# 32-bit tracees are decoded with 64-bit layouts. Use the armv7 build for those.
./configure --enable-mpers=no --disable-gcc-Werror --without-libunwind \
    --without-libiberty --without-libdw
make -j"$JOBS"
install -m 0755 src/strace "$OUT/strace"

if [ "$WITH_ALSA" = 1 ]; then
    echo "== alsa-lib ${ALSA_LIB_VERSION} (static, prefix=/usr)"
    extract "/src/alsa-lib-${ALSA_LIB_VERSION}.tar.bz2"
    cd "$BUILD/alsa-lib-${ALSA_LIB_VERSION}"
    # prefix=/usr => default config dir is /usr/share/alsa (alsa.conf), matching a
    # typical target rootfs. Runtime override: ALSA_CONFIG_DIR / ALSA_CONFIG_PATH.
    ./configure --prefix=/usr --sysconfdir=/etc --disable-shared --enable-static \
        --disable-python --disable-topology --disable-ucm --disable-alisp \
        --disable-old-symbols --disable-rawmidi --disable-seq --disable-aload \
        --disable-mixer-modules --disable-resmgr
    make -j"$JOBS"
    make install

    echo "== alsa-utils ${ALSA_UTILS_VERSION} (aplay/arecord, amixer)"
    extract "/src/alsa-utils-${ALSA_UTILS_VERSION}.tar.bz2"
    cd "$BUILD/alsa-utils-${ALSA_UTILS_VERSION}"
    ./configure --prefix=/usr --disable-shared --disable-nls --disable-alsamixer \
        --disable-alsaconf --disable-bat --disable-alsaloop --disable-xmlto \
        --disable-rst2man --disable-topology --disable-nhlt --disable-alsaucm \
        --with-udev-rules-dir=/nonexistent --with-systemdsystemunitdir=/nonexistent
    # libtool swallows plain -static for programs; -all-static forces a static link.
    make -j"$JOBS" -C aplay LDFLAGS="-all-static -no-pie"
    make -j"$JOBS" -C amixer LDFLAGS="-all-static -no-pie"
    install -m 0755 aplay/aplay "$OUT/aplay"
    # arecord is the same program as aplay; it selects capture mode from argv[0].
    install -m 0755 aplay/aplay "$OUT/arecord"
    install -m 0755 amixer/amixer "$OUT/amixer"
fi

strip --strip-all "$OUT"/* 2>/dev/null || true

{
    echo "built_by=tools/static-tools/build.sh"
    echo "target=$TARGET"
    echo "source_date_epoch=$SOURCE_DATE_EPOCH"
    echo "alpine_image=$ALPINE_IMAGE"
    echo "strace=$STRACE_VERSION"
    [ "$WITH_ALSA" = 1 ] && echo "alsa_lib=$ALSA_LIB_VERSION" && echo "alsa_utils=$ALSA_UTILS_VERSION"
    echo "compiler=$(gcc --version | head -n1)"
    echo "musl=$(apk info -v musl 2>/dev/null | head -n1)"
    echo "machine=$(uname -m)"
} > "$OUT/BUILDINFO"

# Container runs as root (needed for "make install" into the disposable /usr);
# hand the results back to the invoking host user.
if [ -n "${HOST_UID:-}" ]; then chown -R "$HOST_UID:${HOST_GID:-$HOST_UID}" "$OUT"; fi
