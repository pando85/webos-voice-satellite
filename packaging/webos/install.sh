#!/bin/sh
# Installs webos-satellite under one prefix directory. Writes nothing outside
# it. No autostart, no init integration (see docs/development.md).
#
# usage: install.sh --prefix DIR --config FILE [--binary FILE] [--psk-file FILE]
#                   [--persistent]
#
#   --prefix DIR    install directory; must be under /tmp, /run or /dev/shm
#                   (non-persistent) unless --persistent is given, which needs
#                   the owner's authorization on a real TV
#   --config FILE   TOML config (see config/example.toml); installed as etc/config.toml
#   --binary FILE   the static armv7 binary (default: ./webos-satellite next to this script)
#   --psk-file FILE Noise key; installed as etc/noise_psk (config key
#                   device.noise_psk_file = "noise_psk" resolves to it)
#
# Layout: DIR/{bin,etc,run,log,out}; DIR/.webos-satellite-install marks it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
config=
binary=
psk=
persistent=0
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    --config) [ $# -ge 2 ] || die "--config needs a value"; config=$2; shift 2 ;;
    --binary) [ $# -ge 2 ] || die "--binary needs a value"; binary=$2; shift 2 ;;
    --psk-file) [ $# -ge 2 ] || die "--psk-file needs a value"; psk=$2; shift 2 ;;
    --persistent) persistent=1; shift ;;
    -h | --help) sed -n '2,17p' "$0"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done

[ -n "$prefix_arg" ] || die "missing --prefix DIR"
[ -n "$config" ] || die "missing --config FILE (there is no default: the example config plays audio)"
[ -n "$binary" ] || binary=$here/webos-satellite
[ -f "$binary" ] || die "binary not found: $binary"
[ -f "$config" ] || die "config not found: $config"
[ -z "$psk" ] || [ -f "$psk" ] || die "key file not found: $psk"

resolve_prefix "$0" "$prefix_arg"
if ! is_volatile_prefix "$PREFIX" && [ "$persistent" -ne 1 ]; then
    die "$PREFIX is not under /tmp, /run or /dev/shm; pass --persistent only with the owner's authorization"
fi

if [ -e "$PREFIX" ]; then
    [ -d "$PREFIX" ] || die "$PREFIX exists and is not a directory"
    if [ -f "$PREFIX/$MARKER" ]; then
        pid=$(read_pid "$PIDFILE")
        ! is_ours "$pid" || die "the daemon is running (pid $pid); run stop.sh first"
        note "re-installing over $PREFIX"
    elif [ -n "$(ls -A "$PREFIX")" ]; then
        die "$PREFIX exists, is not empty and is not a webos-satellite install"
    fi
fi

umask 077
mkdir -p "$PREFIX/bin" "$PREFIX/etc" "$PREFIX/run" "$PREFIX/log" "$PREFIX/out"
chmod 700 "$PREFIX" "$PREFIX/bin" "$PREFIX/etc" "$PREFIX/run" "$PREFIX/log" "$PREFIX/out"

cp "$binary" "$PREFIX/bin/webos-satellite"
chmod 700 "$PREFIX/bin/webos-satellite"
for f in lib.sh start.sh stop.sh status.sh uninstall.sh rotate.sh supervise.sh autostart.sh; do
    cp "$here/$f" "$PREFIX/bin/$f"
    chmod 700 "$PREFIX/bin/$f"
done
cp "$config" "$PREFIX/etc/config.toml"
chmod 600 "$PREFIX/etc/config.toml"
if [ -n "$psk" ]; then
    cp "$psk" "$PREFIX/etc/noise_psk"
    chmod 600 "$PREFIX/etc/noise_psk"
fi
echo "webos-satellite install; remove with bin/uninstall.sh" >"$PREFIX/$MARKER"
chmod 600 "$PREFIX/$MARKER"

note "installed under $PREFIX"
if "$PREFIX/bin/webos-satellite" --config "$PREFIX/etc/config.toml" --check-config; then
    note "config is valid; start with: sh $PREFIX/bin/start.sh"
else
    note "the config is NOT valid; fix $PREFIX/etc/config.toml (or uninstall: sh $PREFIX/bin/uninstall.sh)"
    exit 1
fi
