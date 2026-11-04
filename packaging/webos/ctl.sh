#!/bin/sh
# Control entry point of the webOS app package. The app calls it through the
# Homebrew Channel's root `exec` with fixed arguments only; it also works by
# hand over SSH. Design: docs/design/packaging.md.
#
# usage: ctl.sh COMMAND
#
#   status [--json]   state of the package, the install and the daemon
#                     (never prints the key or other secrets)
#   install [--config FILE] [--psk-file FILE] [--arch auto|armv7|aarch64]
#           [--force]
#                     first install or update from the app's payload into DIR:
#                     stops a running daemon, installs, starts it again if it
#                     was running. Keeps etc/ (config, key, pairing state)
#                     and log/. A first install writes a config from
#                     config.default.toml with a random name and MAC seed;
#                     --config replaces the config, --psk-file the key.
#                     Does nothing when DIR already holds this payload (two
#                     triggers applying one update), unless --force
#   start | stop | restart
#                     the supervised daemon (start refuses while DIR/disable
#                     or DIR/crashloop exists)
#   autostart on|off  write or delete the boot hook (opt-in, off by default)
#   mic off|on        create or delete the kill switch (the mic is released
#                     at once; lost at reboot)
#   pair              open the pairing window (daemon running, pairing on)
#   reset-pairing     forget the stored key and open the window
#   clear-crashloop   delete DIR/crashloop after fixing the cause
#   uninstall         delete the boot hook, stop, delete the daemon; KEEP
#                     etc/ and log/ (a reinstall reuses them)
#   purge             delete the boot hook, stop, delete DIR entirely
#                     (config, key, pairing state, logs)
#
# DIR is /var/lib/webos-satellite. Environment (tests, other layouts):
# WEBOS_SAT_PREFIX (DIR), WEBOS_SAT_INITD (default /var/lib/webosbrew/init.d),
# WEBOS_SAT_LOCK, WEBOS_SAT_ALLOW_NONROOT=1 (skip the root check).
# Exit status: 0 done, 1 failed (message on stderr), 2 usage.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

usage() {
    sed -n '2,39p' "$0" >&2
    exit 2
}

P=${WEBOS_SAT_PREFIX:-$STATE_PREFIX_DEFAULT}
case $P in
*[!A-Za-z0-9._/-]*) die "unsupported characters in the install directory: $P" ;;
esac
P=$(check_prefix "$P") || exit 1
set_paths "$P"
INITD=${WEBOS_SAT_INITD:-$INITD_DIR_DEFAULT}
HOOK=$INITD/$HOOK_NAME
PAIR_DIR=$P/etc/pairing

# Payload: the package files to install from. ctl.sh runs either from the
# app's payload directory or from DIR/bin (then the app is found through
# DIR/app-dir, recorded at install).
APPDIR=
PAYLOAD=
if [ -f "$here/SHA256SUMS" ] && [ -f "$here/install.sh" ]; then
    PAYLOAD=$here
    [ ! -f "$here/../appinfo.json" ] || APPDIR=$(cd "$here/.." && pwd)
elif [ -f "$P/app-dir" ]; then
    read -r APPDIR <"$P/app-dir" || true
    if [ -f "$APPDIR/payload/SHA256SUMS" ]; then
        PAYLOAD=$APPDIR/payload
    fi
fi

need_root() {
    [ "$(id -u)" = 0 ] || [ "${WEBOS_SAT_ALLOW_NONROOT:-}" = 1 ] ||
        die "needs root (from the app: the Homebrew Channel service must be elevated)"
}

locked() {
    lock_take 60
    trap lock_release EXIT
    trap 'lock_release; exit 1' INT TERM HUP
}

installed() { [ -f "$P/$MARKER" ] && [ -x "$BIN" ]; }

running() { is_ours "$(read_pid "$PIDFILE")" || supervisor_alive; }

# Random lowercase hex, $1 bytes (od or hexdump; date and pid as a last resort).
rand_hex() {
    h=$(od -An -N"$1" -tx1 /dev/urandom 2>/dev/null | tr -d ' \n') || h=
    [ -n "$h" ] || h=$(hexdump -n "$1" -e '1/1 "%02x"' /dev/urandom 2>/dev/null) || h=
    [ -n "$h" ] || h=$(printf '%x%x' "$(date +%s)" "$$")
    echo "$h"
}

write_hook() {
    tmpl=$P/bin/webos-satellite.hook
    [ -f "$tmpl" ] || die "hook template missing: $tmpl (reinstall)"
    mkdir -p "$INITD"
    sed "s|@PREFIX@|$P|" "$tmpl" >"$HOOK.new.$$"
    chmod 755 "$HOOK.new.$$"
    mv -f "$HOOK.new.$$" "$HOOK"
}

cmd_install() {
    cfg=
    psk=
    arch=auto
    force=0
    while [ $# -gt 0 ]; do
        case $1 in
        --config) [ $# -ge 2 ] || usage; cfg=$2; shift 2 ;;
        --psk-file) [ $# -ge 2 ] || usage; psk=$2; shift 2 ;;
        --arch) [ $# -ge 2 ] || usage; arch=$2; shift 2 ;;
        --force) force=1; shift ;;
        *) usage ;;
        esac
    done
    need_root
    [ -n "$PAYLOAD" ] || die "no package payload found (run the ctl.sh inside the app's payload directory)"
    locked
    if [ "$force" -eq 0 ] && [ -z "$cfg" ] && [ -z "$psk" ] && installed &&
        cmp -s "$PAYLOAD/SHA256SUMS" "$P/package.sums"; then
        note "already up to date ($(cat "$P/VERSION" 2>/dev/null || echo unknown)); nothing to do (--force reinstalls)"
        return 0
    fi
    was_running=0
    if [ -f "$P/$MARKER" ] && running; then
        was_running=1
        note "stopping the running daemon for the update"
        stop_daemon 10
    fi
    tmpcfg=
    if [ -n "$cfg" ]; then
        set -- --config "$cfg"
    elif [ -f "$CFG" ]; then
        set -- --keep-config
    else
        tmpcfg=$(mktemp "${TMPDIR:-/tmp}/webos-satellite-config.XXXXXX")
        sed -e "s/@NAME@/webos-satellite-$(rand_hex 3)/" -e "s/@MAC_SEED@/$(rand_hex 16)/" \
            "$PAYLOAD/config.default.toml" >"$tmpcfg"
        set -- --config "$tmpcfg"
        note "first install: writing a new config with a random device name"
    fi
    [ -z "$psk" ] || set -- "$@" --psk-file "$psk"
    rc=0
    sh "$PAYLOAD/install.sh" --prefix "$P" --persistent --arch "$arch" "$@" || rc=$?
    [ -z "$tmpcfg" ] || rm -f "$tmpcfg"
    if [ "$rc" -ne 0 ]; then
        if [ "$was_running" -eq 1 ] && installed; then
            note "install failed; starting the previous install again"
            sh "$P/bin/start.sh" --supervise || true
        fi
        die "install failed (status $rc)"
    fi
    if [ -n "$APPDIR" ]; then
        echo "$APPDIR" >"$P/app-dir.new.$$" && mv -f "$P/app-dir.new.$$" "$P/app-dir"
    else
        rm -f "$P/app-dir"
    fi
    if [ -e "$HOOK" ]; then
        write_hook
        note "boot hook refreshed: $HOOK"
    fi
    note "installed $(cat "$P/VERSION" 2>/dev/null || echo "(no version)") under $P"
    if [ "$was_running" -eq 1 ]; then
        sh "$P/bin/start.sh" --supervise
    fi
}

cmd_start() {
    need_root
    installed || die "not installed (ctl.sh install)"
    locked
    sh "$P/bin/start.sh" --supervise
}

cmd_stop() {
    need_root
    installed || die "not installed"
    locked
    stop_daemon 10
    note "stopped"
}

cmd_autostart() {
    [ $# -eq 1 ] || usage
    need_root
    case $1 in
    on)
        installed || die "not installed (ctl.sh install)"
        locked
        write_hook
        note "start at boot: on ($HOOK; the daemon starts about 60 s after boot)"
        ;;
    off)
        rm -f "$HOOK"
        note "start at boot: off"
        ;;
    *) usage ;;
    esac
}

cmd_mic() {
    [ $# -eq 1 ] || usage
    case $1 in
    off) : >"$KILL_SWITCH_DEFAULT" && note "microphone off (kill switch $KILL_SWITCH_DEFAULT)" ;;
    on) rm -f "$KILL_SWITCH_DEFAULT" && note "kill switch removed" ;;
    *) usage ;;
    esac
}

cmd_trigger() { # pair | reset-pairing
    need_root
    installed || die "not installed"
    [ -d "$PAIR_DIR" ] || die "pairing is not enabled in $CFG (no $PAIR_DIR)"
    running || die "the daemon is not running; start it first"
    : >"$PAIR_DIR/$1"
    note "$1 requested; the daemon acts on it within a second"
}

remove_hook_and_stop() {
    rm -f "$HOOK"
    if [ -f "$P/$MARKER" ]; then
        stop_daemon 10
    fi
}

cmd_uninstall() {
    need_root
    [ -f "$P/$MARKER" ] || die "$P is not installed"
    locked
    remove_hook_and_stop
    rm -rf "${P:?}/bin" "${P:?}/run" "${P:?}/out"
    rm -f "$P/VERSION" "$P/package.sums" "$P/app-dir"
    note "removed the boot hook and the daemon; kept config, key and logs in $P/etc and $P/log"
    note "remove those too with: sh <app>/payload/ctl.sh purge, or rm -rf $P"
}

cmd_purge() {
    need_root
    locked
    rm -f "$HOOK"
    if [ -e "$P" ]; then
        [ -f "$P/$MARKER" ] || die "$P exists but is not a webos-satellite install; not removing it"
        stop_daemon 10
        cd /
        rm -rf "$P"
    fi
    note "removed $P and the boot hook"
}

# --- status ------------------------------------------------------------------

json_str() {
    printf '"%s"' "$(printf '%s' "$1" | tr -d '\000-\037' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
}

json_bool() { if "$@"; then printf true; else printf false; fi; }

# Value of a top-level `key = "value"` or `key = number` line in the config.
cfg_value() {
    sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\{0,1\}\([^\"#]*\)\"\{0,1\}.*/\1/p" "$CFG" 2>/dev/null |
        head -n 1 | sed 's/[[:space:]]*$//'
}

# Key state: fixed (etc/noise_psk named in the config), paired (a key set by
# Home Assistant), cleared (HA removed the device), unpaired, or none.
key_state() {
    if [ -n "$(cfg_value noise_psk_file)" ] || [ -n "$(cfg_value noise_psk)" ]; then
        echo fixed
    elif [ -f "$PAIR_DIR/noise_psk" ]; then
        echo paired
    elif [ -f "$PAIR_DIR/noise_psk.cleared" ]; then
        echo cleared
    elif [ -d "$PAIR_DIR" ] || [ "$(cfg_value enabled)" = true ]; then
        echo unpaired
    else
        echo none
    fi
}

firmware_version() {
    sed -n 's/.*"webos_manufacturing_version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        /var/run/nyx/os_info.json 2>/dev/null | head -n 1
}

cmd_status() {
    json=0
    [ "${1:-}" != --json ] || json=1
    pkg_version=$(cat "$PAYLOAD/VERSION" 2>/dev/null || true)
    inst_version=$(cat "$P/VERSION" 2>/dev/null || true)
    is_inst=false
    installed && is_inst=true
    update=false
    if [ "$is_inst" = true ] && [ -n "$PAYLOAD" ] && ! cmp -s "$PAYLOAD/SHA256SUMS" "$P/package.sums"; then
        update=true
    fi
    pid=$(read_pid "$PIDFILE")
    is_ours "$pid" || pid=
    build=
    fw=
    fw_ok=false
    if [ "$is_inst" = true ]; then
        build=$("$BIN" --version 2>/dev/null || true)
        rc=0
        fw=$("$BIN" --config "$CFG" --check-firmware 2>&1) || rc=$?
        case $rc in
        0 | 3) fw_ok=true ;; # validated, or the guard is off
        esac
    else
        fw="$(firmware_version) (checked against the allow-list after set up)"
    fi
    keys=$(key_state)
    # Written by the daemon when Home Assistant changes them (etc/). `1{...;p;}`
    # prints even when the line is already clean (`s///gp` printed nothing then).
    remote_off=$(sed -n '1{s/[^a-z_]//g;p;}' "$P/etc/remote_off" 2>/dev/null || true)
    reply_volume=$(sed -n '1{s/[^0-9]//g;p;}' "$P/etc/reply_volume" 2>/dev/null || true)
    # Until Home Assistant stores one, the daemon uses sink.volume (percent).
    reply_volume_default=$(cfg_value volume | sed 's/[^0-9]//g')
    name=$(cfg_value name)
    port=$(cfg_value port)
    if [ "$json" -eq 0 ]; then
        note "package: ${pkg_version:-none} ($PAYLOAD)"
        note "installed: $is_inst ${inst_version:+$inst_version }in $P${build:+ ($build)}"
        [ "$update" = false ] || note "an update is pending: run ctl.sh install (or open the app)"
        if [ -n "$pid" ]; then note "running: pid $pid"; else note "running: no"; fi
        if [ -e "$HOOK" ]; then note "start at boot: on ($HOOK)"; else note "start at boot: off"; fi
        [ ! -e "$DISABLE" ] || note "marker $DISABLE present: starts are refused"
        [ ! -e "$CRASHLOOP" ] || note "marker $CRASHLOOP present: the supervisor gave up"
        [ ! -e "$KILL_SWITCH_DEFAULT" ] || note "microphone off (kill switch $KILL_SWITCH_DEFAULT)"
        note "firmware: $fw"
        note "device: ${name:-?} port ${port:-6053}, key: $keys"
        if [ -n "$reply_volume" ]; then
            vol_text="$reply_volume %"
        elif [ -n "$reply_volume_default" ]; then
            vol_text="$reply_volume_default % (config default; none stored yet)"
        else
            vol_text="default (none stored yet)"
        fi
        note "reply volume: $vol_text, remote power-off: ${remote_off:-normal}"
        return 0
    fi
    printf '{"package_version":%s,' "$(json_str "$pkg_version")"
    printf '"installed":%s,"installed_version":%s,' "$is_inst" "$(json_str "$inst_version")"
    printf '"build":%s,"update_pending":%s,' "$(json_str "$build")" "$update"
    printf '"uid":%s,"machine":%s,' "$(id -u)" "$(json_str "$(uname -m)")"
    printf '"running":%s,"pid":%s,' "$(json_bool test -n "$pid")" "${pid:-null}"
    printf '"supervised":%s,' "$(json_bool supervisor_alive)"
    printf '"autostart":%s,' "$(json_bool test -e "$HOOK")"
    printf '"disabled":%s,"crashloop":%s,' "$(json_bool test -e "$DISABLE")" "$(json_bool test -e "$CRASHLOOP")"
    printf '"mic_off":%s,' "$(json_bool test -e "$KILL_SWITCH_DEFAULT")"
    printf '"firmware":%s,"firmware_ok":%s,' "$(json_str "$fw")" "$fw_ok"
    printf '"key":%s,"name":%s,"port":%s,' "$(json_str "$keys")" "$(json_str "$name")" "$(json_str "${port:-6053}")"
    printf '"remote_off":%s,"reply_volume":%s,' "$(json_str "${remote_off:-normal}")" "$(json_str "$reply_volume")"
    printf '"reply_volume_default":%s,' "$(json_str "$reply_volume_default")"
    printf '"prefix":%s,"log":[' "$(json_str "$P")"
    sep=
    if [ -f "$LOG" ]; then
        tail -n 8 "$LOG" | while IFS= read -r line; do
            printf '%s%s' "$sep" "$(json_str "$line")"
            sep=,
        done
    fi
    printf ']}\n'
}

[ $# -ge 1 ] || usage
cmd=$1
shift
case $cmd in
status) cmd_status "$@" ;;
install | update) cmd_install "$@" ;;
start) [ $# -eq 0 ] || usage; cmd_start ;;
stop) [ $# -eq 0 ] || usage; cmd_stop ;;
restart)
    [ $# -eq 0 ] || usage
    need_root
    installed || die "not installed"
    locked
    stop_daemon 10
    sh "$P/bin/start.sh" --supervise
    ;;
autostart) cmd_autostart "$@" ;;
mic) cmd_mic "$@" ;;
pair) [ $# -eq 0 ] || usage; cmd_trigger pair ;;
reset-pairing) [ $# -eq 0 ] || usage; cmd_trigger reset-pairing ;;
clear-crashloop) [ $# -eq 0 ] || usage; need_root; rm -f "$CRASHLOOP"; note "crash-loop marker removed" ;;
uninstall) [ $# -eq 0 ] || usage; cmd_uninstall ;;
purge) [ $# -eq 0 ] || usage; cmd_purge ;;
-h | --help) usage ;;
*) usage ;;
esac
