#!/bin/sh
# Installs webos-satellite under one prefix directory. Writes nothing outside
# it. No autostart, no init integration (see docs/development.md; the webOS
# app's ctl.sh calls this script and adds the boot hook on request).
#
# usage: install.sh --prefix DIR (--config FILE | --keep-config)
#                   [--arch auto|armv7|aarch64] [--binary FILE]
#                   [--psk-file FILE] [--persistent]
#
#   --prefix DIR    install directory; must be under /tmp, /run or /dev/shm
#                   (non-persistent) unless --persistent is given, which needs
#                   the owner's authorization on a real TV
#   --config FILE   TOML config (see config/example.toml); installed as etc/config.toml
#   --keep-config   keep an existing etc/config.toml (an update); with
#                   --config as well, FILE is used only when none exists yet
#   --arch A        which build of the package to install (default auto):
#                   auto: aarch64 if `uname -m` is aarch64 (64-bit kernel),
#                   else armv7; armv7 or aarch64: that build first. An aarch64
#                   build that does not run falls back to armv7. Builds are
#                   webos-satellite-armv7 and webos-satellite-aarch64 next to
#                   this script (a lone ./webos-satellite is used as is)
#   --binary FILE   install this binary instead (no selection)
#   --psk-file FILE Noise key; installed as etc/noise_psk (config key
#                   device.noise_psk_file = "noise_psk" resolves to it)
#
# The chosen binary is copied into DIR/bin under a temporary name and must
# answer --version before it replaces the installed one. If nothing runs, an
# existing install is left unchanged.
#
# Layout: DIR/{bin,etc,run,log,out}; DIR/.webos-satellite-install marks it.
# Files are replaced by rename, so a script that runs from DIR/bin keeps
# its old copy. A package with VERSION and SHA256SUMS next to this script
# records them as DIR/VERSION and DIR/package.sums (update detection).
# Test hook: WEBOS_SAT_UNAME_M replaces `uname -m`.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
config=
binary=
arch=auto
psk=
persistent=0
keep_config=0
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    --config) [ $# -ge 2 ] || die "--config needs a value"; config=$2; shift 2 ;;
    --binary) [ $# -ge 2 ] || die "--binary needs a value"; binary=$2; shift 2 ;;
    --arch) [ $# -ge 2 ] || die "--arch needs a value"; arch=$2; shift 2 ;;
    --psk-file) [ $# -ge 2 ] || die "--psk-file needs a value"; psk=$2; shift 2 ;;
    --persistent) persistent=1; shift ;;
    --keep-config) keep_config=1; shift ;;
    -h | --help) sed -n '2,34p' "$0"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done

[ -n "$prefix_arg" ] || die "missing --prefix DIR"
[ -n "$config" ] || [ "$keep_config" -eq 1 ] || die "missing --config FILE (there is no default: the example config plays audio)"
case $arch in
auto | armv7 | aarch64) ;;
*) die "--arch must be auto, armv7 or aarch64" ;;
esac
bin_a32=$here/webos-satellite-armv7
bin_a64=$here/webos-satellite-aarch64
if [ -z "$binary" ] && [ ! -f "$bin_a32" ] && [ ! -f "$bin_a64" ]; then
    # Older layout: one binary next to the script.
    binary=$here/webos-satellite
fi
[ -z "$binary" ] || [ -f "$binary" ] || die "binary not found: $binary"
[ -z "$config" ] || [ -f "$config" ] || die "config not found: $config"
[ -z "$psk" ] || [ -f "$psk" ] || die "key file not found: $psk"

resolve_prefix "$0" "$prefix_arg"
if [ "$keep_config" -eq 1 ] && [ -f "$CFG" ]; then
    config=
elif [ -z "$config" ]; then
    die "--keep-config: $CFG does not exist; give --config FILE"
fi
if ! is_volatile_prefix "$PREFIX" && [ "$persistent" -ne 1 ]; then
    die "$PREFIX is not under /tmp, /run or /dev/shm; pass --persistent only with the owner's authorization"
fi

created=1
had_install=0
if [ -e "$PREFIX" ]; then
    created=0
    [ -d "$PREFIX" ] || die "$PREFIX exists and is not a directory"
    if [ -f "$PREFIX/$MARKER" ]; then
        had_install=1
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

# Binary: copy the candidate next to its final name, check that it runs,
# then rename it over the installed one.
new=$PREFIX/bin/.webos-satellite.new
give_up() {
    rm -f "$new"
    if [ "$had_install" -eq 0 ]; then
        # Only what this run created: the directory was new or empty.
        rm -rf "${PREFIX:?}/bin" "${PREFIX:?}/etc" "${PREFIX:?}/run" "${PREFIX:?}/log" "${PREFIX:?}/out"
        [ "$created" -eq 0 ] || rmdir "$PREFIX"
    fi
    die "$* (an existing install is unchanged)"
}
try_binary() { # FILE [TRIPLE]
    rm -f "$new"
    cp "$1" "$new" && chmod 700 "$new" && binary_runs "$new" "${2:-}"
}
if [ -n "$binary" ]; then
    try_binary "$binary" || give_up "$binary does not run here (--version failed)"
    chosen="$binary"
else
    machine=${WEBOS_SAT_UNAME_M:-$(uname -m)}
    want=$arch
    [ "$want" != auto ] || want=$(arch_for_machine "$machine")
    chosen=
    if [ "$want" = aarch64 ]; then
        if [ ! -f "$bin_a64" ]; then
            note "no aarch64 build in the package; using armv7"
        elif try_binary "$bin_a64" "$(triple_of_arch aarch64)"; then
            chosen=aarch64
        else
            note "the aarch64 build does not run here (uname -m: $machine); falling back to armv7"
        fi
    fi
    if [ -z "$chosen" ]; then
        [ -f "$bin_a32" ] || give_up "no armv7 build in the package: $bin_a32"
        try_binary "$bin_a32" "$(triple_of_arch armv7)" || give_up "the armv7 build does not run here (--version failed)"
        chosen=armv7
    fi
    note "uname -m: $machine, --arch $arch: installing the $chosen build"
fi
mv -f "$new" "$PREFIX/bin/webos-satellite"
for f in lib.sh start.sh stop.sh status.sh uninstall.sh rotate.sh supervise.sh autostart.sh ctl.sh; do
    # ctl.sh is part of the webOS app package; older trees lack it.
    [ -f "$here/$f" ] || [ "$f" = ctl.sh ] || die "missing $here/$f"
    [ ! -f "$here/$f" ] || install_file "$here/$f" "$PREFIX/bin/$f" 700
done
# The boot hook template: init.d/webos-satellite in the tree, renamed in a package.
for h in "$here/webos-satellite.hook" "$here/init.d/webos-satellite"; do
    if [ -f "$h" ]; then
        install_file "$h" "$PREFIX/bin/webos-satellite.hook" 600
        break
    fi
done
if [ -n "$config" ]; then
    install_file "$config" "$PREFIX/etc/config.toml" 600
else
    note "keeping $PREFIX/etc/config.toml"
fi
if [ -n "$psk" ]; then
    install_file "$psk" "$PREFIX/etc/noise_psk" 600
fi
if [ -f "$here/SHA256SUMS" ]; then
    install_file "$here/SHA256SUMS" "$PREFIX/package.sums" 600
fi
if [ -f "$here/VERSION" ]; then
    install_file "$here/VERSION" "$PREFIX/VERSION" 600
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
