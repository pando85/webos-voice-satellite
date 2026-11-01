#!/bin/sh
# Reproducible build of static (musl) tools: strace for aarch64 and armv7, plus
# optional aarch64 arecord/aplay/amixer. See README.md.
#
# Host requirements: docker, curl, sha256sum, tar; gpg (recommended);
# qemu-user binfmt for arm64 when the host is not aarch64.
# Nothing here touches a TV; the output is only written under artifacts/.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
. "$HERE/versions.env"

OUT_BASE="${OUT_BASE:-$ROOT/artifacts/static-tools}"
TARGETS="${TARGETS:-aarch64 armv7}"
SRC_DIR="${SRC_DIR:-$ROOT/artifacts/static-tools/src}"
IMAGE_TAG="${IMAGE_TAG:-webos-static-tools-builder:local}"
WITH_ALSA="${WITH_ALSA:-1}"   # alsa-utils is optional (aarch64 target only)
ALLOW_NO_GPG="${ALLOW_NO_GPG:-0}"

die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing host tool: $1"; }

need docker; need curl; need sha256sum; need tar
docker info >/dev/null 2>&1 || die "docker daemon not reachable"

if [ "$(uname -m)" != "aarch64" ] && [ ! -e /proc/sys/fs/binfmt_misc/qemu-aarch64 ]; then
    die "arm64 emulation missing: register qemu-user binfmt for aarch64 (e.g. qemu-user-static)"
fi

mkdir -p "$SRC_DIR"

fetch() { # fetch <url> <dest>
    [ -s "$2" ] || curl -fsSL --retry 3 -o "$2" "$1"
}

check_sha() { # check_sha <sha256> <file>
    echo "$1  $2" | sha256sum -c - >/dev/null || die "SHA-256 mismatch: $2"
}

GNUPGHOME=$(mktemp -d "$SRC_DIR/gnupg.XXXXXX")
export GNUPGHOME
trap 'rm -rf "$GNUPGHOME"' EXIT INT TERM
HAVE_GPG=0
if command -v gpg >/dev/null 2>&1; then HAVE_GPG=1; fi
[ "$HAVE_GPG" = 1 ] || [ "$ALLOW_NO_GPG" = 1 ] || die "gpg missing (set ALLOW_NO_GPG=1 to rely on SHA-256 pins only)"

import_key() { # import_key <url> <primary-fingerprint>
    fetch "$1" "$SRC_DIR/$(basename "$1").$2.key"
    gpg --batch --quiet --import "$SRC_DIR/$(basename "$1").$2.key" 2>/dev/null
    gpg --batch --with-colons --fingerprint "$2" 2>/dev/null | grep -q "^fpr:::::::::$2:" \
        || die "key fingerprint $2 not found in downloaded key"
}

get_source() { # get_source <name> <url> <sha256> [sig-suffix key-url key-fpr]
    name=$1; url=$2; sha=$3
    file="$SRC_DIR/$(basename "$url")"
    fetch "$url" "$file"
    check_sha "$sha" "$file"
    if [ "$#" -ge 6 ] && [ "$HAVE_GPG" = 1 ]; then
        fetch "$url$4" "$file$4"
        import_key "$5" "$6"
        gpg --batch --status-fd 1 --verify "$file$4" "$file" 2>/dev/null \
            | grep -q '^\[GNUPG:\] VALIDSIG' || die "GPG signature check failed: $file"
        echo "ok: sha256 + gpg: $(basename "$file")"
    else
        echo "ok: sha256 only: $(basename "$file")"
    fi
}

if [ "$WITH_ALSA" = 1 ]; then
    get_source alsa-lib   "$ALSA_LIB_URL"   "$ALSA_LIB_SHA256"   .sig "$ALSA_KEY_URL"   "$ALSA_KEY_FPR"
    get_source alsa-utils "$ALSA_UTILS_URL" "$ALSA_UTILS_SHA256" .sig "$ALSA_KEY_URL"   "$ALSA_KEY_FPR"
fi
get_source strace     "$STRACE_URL"     "$STRACE_SHA256"      .asc "$STRACE_KEY_URL" "$STRACE_KEY_FPR"
WORK_BASE="$ROOT/artifacts/static-tools"
trap 'rm -rf "$GNUPGHOME"' EXIT INT TERM

build_target() { # build_target <name> <docker-platform> <with-alsa>
    t=$1; plat=$2; alsa=$3
    out="$OUT_BASE/$t"
    tag="$IMAGE_TAG-$t"
    echo "== [$t] building container image ($tag, $plat)"
    docker build --platform "$plat" --build-arg "ALPINE_IMAGE=$ALPINE_IMAGE" \
        -t "$tag" "$HERE"
    work=$(mktemp -d "$WORK_BASE/work.XXXXXX")
    echo "== [$t] building tools in container"
    docker run --rm --platform "$plat" \
        -v "$HERE:/tools:ro" -v "$SRC_DIR:/src:ro" -v "$work:/out" \
        -e "TARGET=$t" -e "WITH_ALSA=$alsa" -e "ALPINE_IMAGE=$ALPINE_IMAGE" \
        -e "JOBS=${JOBS:-$(nproc)}" -e "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1700000000}" \
        -e "HOST_UID=$(id -u)" -e "HOST_GID=$(id -g)" \
        "$tag" sh /tools/container-build.sh
    mkdir -p "$out"
    rm -f "$out"/*
    cp -p "$work"/* "$out"/
    rm -rf "$work"
    (cd "$out" && ls | grep -v -e '^BUILDINFO$' -e '^SHA256SUMS$' | xargs sha256sum > SHA256SUMS)
    echo "== [$t] done: $out"
    cat "$out/SHA256SUMS"
}

for t in $TARGETS; do
    case "$t" in
        aarch64) build_target aarch64 linux/arm64 "$WITH_ALSA" ;;
        armv7)   build_target armv7   linux/arm/v7 0 ;;  # strace only
        *) die "unknown target: $t (use aarch64 and/or armv7)" ;;
    esac
done
