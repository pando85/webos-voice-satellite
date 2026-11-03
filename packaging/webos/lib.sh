# Shared helpers for the webos-satellite ops scripts. Sourced, not executed.
# POSIX sh, BusyBox ash compatible. Everything lives under one prefix
# directory; nothing is written outside it.

MARKER=.webos-satellite-install

die() {
    echo "webos-satellite: $*" >&2
    exit 1
}

note() {
    echo "webos-satellite: $*"
}

# Rejects prefixes that are empty, relative, contain "..", or are a system
# directory. Prints the normalised prefix (no trailing slash).
check_prefix() {
    p=$1
    [ -n "$p" ] || die "empty prefix"
    case $p in
    /*) ;;
    *) die "prefix must be an absolute path: $p" ;;
    esac
    case $p in
    */../* | */.. | */./* | */.) die "prefix must not contain . or .. components: $p" ;;
    esac
    # Strip trailing slashes.
    while [ "${p%/}" != "$p" ] && [ -n "${p%/}" ]; do
        p=${p%/}
    done
    case $p in
    / | /bin | /boot | /dev | /etc | /home | /home/root | /lib | /media | /media/cryptofs | /media/developer | /media/developer/apps | /mnt | /opt | /proc | /root | /run | /sbin | /sys | /tmp | /usr | /var | /var/lib | /var/lib/webosbrew | /var/luna)
        die "refusing to use a system directory as the prefix: $p"
        ;;
    /media/developer/apps/* | /media/cryptofs/* | /var/lib/webosbrew/* | /usr/palm/*)
        die "refusing a prefix inside a directory managed by webOS or the Homebrew Channel: $p"
        ;;
    esac
    [ ! -L "$p" ] || die "prefix is a symbolic link: $p"
    echo "$p"
}

# True if the prefix is on a non-persistent location.
is_volatile_prefix() {
    case $1 in
    /tmp/* | /run/* | /dev/shm/*) return 0 ;;
    esac
    return 1
}

# Sets PREFIX and the file names derived from it (used by the scripts that
# source this file).
# shellcheck disable=SC2034
set_paths() {
    PREFIX=$1
    BIN=$PREFIX/bin/webos-satellite
    CFG=$PREFIX/etc/config.toml
    PIDFILE=$PREFIX/run/webos-satellite.pid
    ROTPID=$PREFIX/run/rotate.pid
    SUPPID=$PREFIX/run/supervise.pid
    LOG=$PREFIX/log/webos-satellite.log
    # Markers at the top of the prefix (persistent with a persistent prefix):
    # DISABLE: do not start (autostart and supervisor honour it);
    # CRASHLOOP: written by supervise.sh when it gave up.
    DISABLE=$PREFIX/disable
    CRASHLOOP=$PREFIX/crashloop
}

# Default kill-switch file of the daemon (safety.kill_switch in the config).
# WEBOS_SAT_KILL_SWITCH replaces it in host tests.
# shellcheck disable=SC2034
KILL_SWITCH_DEFAULT=${WEBOS_SAT_KILL_SWITCH:-/tmp/webos-satellite.disable}

# --- Persistent install of the webOS app (ctl.sh) ---------------------------
# The app's files live where the TV's installer puts them and are replaced on
# every update; config, key, logs and the running copy of the daemon live in
# STATE_PREFIX_DEFAULT, outside the app (docs/design/packaging.md).
# shellcheck disable=SC2034
STATE_PREFIX_DEFAULT=/var/lib/webos-satellite
# shellcheck disable=SC2034
INITD_DIR_DEFAULT=/var/lib/webosbrew/init.d
# shellcheck disable=SC2034
HOOK_NAME=webos-satellite
LOCK_DEFAULT=/tmp/webos-satellite.lock

# Serialises install, update, start, stop and uninstall (ctl.sh, autostart.sh).
# The lock is a directory (mkdir is atomic) holding the owner's pid; a lock
# whose owner is gone is taken over. A child of the owner inherits it through
# WEBOS_SAT_LOCK_HELD. Argument: seconds to wait (default 30).
lock_take() {
    LOCKDIR=${WEBOS_SAT_LOCK:-$LOCK_DEFAULT}
    [ "${WEBOS_SAT_LOCK_HELD:-}" != "$LOCKDIR" ] || return 0
    waited=0
    while ! mkdir "$LOCKDIR" 2>/dev/null; do
        holder=$(read_pid "$LOCKDIR/pid")
        if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
            rm -rf "$LOCKDIR"
            continue
        fi
        waited=$((waited + 1))
        [ "$waited" -lt "${1:-30}" ] || die "another operation holds $LOCKDIR (pid ${holder:-unknown}); if none runs, remove that directory"
        sleep 1
    done
    echo $$ >"$LOCKDIR/pid"
    LOCK_OWNED=1
    WEBOS_SAT_LOCK_HELD=$LOCKDIR
    export WEBOS_SAT_LOCK_HELD
}

lock_release() {
    if [ "${LOCK_OWNED:-0}" = 1 ]; then
        rm -rf "$LOCKDIR"
        LOCK_OWNED=0
    fi
}

# Copies $1 to $2 with mode $3 through a temporary file and a rename, so a
# shell that is running the old $2 keeps reading the old file.
install_file() {
    cp "$1" "$2.new.$$" && chmod "$3" "$2.new.$$" && mv -f "$2.new.$$" "$2"
}

# Runs "$@" in a new session when setsid exists, so the job does not share
# the caller's session or process group (a Homebrew Channel exec, a hook).
# In a non-interactive shell a background job is not a group leader, so
# setsid does not fork and $! stays the job's pid.
detached() {
    if command -v setsid >/dev/null 2>&1; then
        setsid "$@"
    else
        "$@"
    fi
}

# Picks the prefix: --prefix DIR, or the parent of the directory holding the
# installed script. Call as: resolve_prefix "$0" "$prefix_arg"
resolve_prefix() {
    script=$1
    arg=$2
    if [ -n "$arg" ]; then
        p=$arg
    else
        d=$(cd "$(dirname "$script")" && pwd) || die "cannot resolve the script directory"
        if [ -f "$d/../$MARKER" ]; then
            p=$(cd "$d/.." && pwd)
        else
            die "missing --prefix DIR"
        fi
    fi
    p=$(check_prefix "$p") || exit 1
    set_paths "$p"
}

require_installed() {
    [ -f "$PREFIX/$MARKER" ] || die "$PREFIX is not a webos-satellite install (no $MARKER)"
}

# True if $1 is a live process whose executable is our binary.
is_ours() {
    [ -n "$1" ] || return 1
    kill -0 "$1" 2>/dev/null || return 1
    exe=$(readlink "/proc/$1/exe" 2>/dev/null) || return 1
    [ "$exe" = "$BIN" ]
}

read_pid() {
    pid=
    [ -f "$1" ] && { read -r pid <"$1" || true; }
    case $pid in
    '' | *[!0-9]*) pid= ;;
    esac
    echo "$pid"
}

# True if the sidecar process in $ROTPID is alive (it is a `sh` running rotate.sh).
rotator_alive() {
    rp=$(read_pid "$ROTPID")
    [ -n "$rp" ] || return 1
    kill -0 "$rp" 2>/dev/null || return 1
    tr '\0' ' ' <"/proc/$rp/cmdline" 2>/dev/null | grep -q "rotate.sh"
}

# True if the supervisor in $SUPPID is alive (a `sh` running supervise.sh).
supervisor_alive() {
    sp=$(read_pid "$SUPPID")
    [ -n "$sp" ] || return 1
    kill -0 "$sp" 2>/dev/null || return 1
    tr '\0' ' ' <"/proc/$sp/cmdline" 2>/dev/null | grep -q "supervise.sh"
}

# Stops the supervisor (if any), the daemon (TERM, wait, then KILL) and the
# log rotator, and removes the pidfiles. Argument: seconds to wait (default 10).
stop_daemon() {
    wait_s=${1:-10}
    pid=$(read_pid "$PIDFILE")
    if supervisor_alive; then
        # The supervisor forwards TERM to the daemon and will not restart it.
        note "stopping supervisor pid $(read_pid "$SUPPID")"
        kill -TERM "$(read_pid "$SUPPID")" 2>/dev/null
    fi
    if is_ours "$pid"; then
        note "stopping pid $pid"
        kill -TERM "$pid" 2>/dev/null
        i=0
        while is_ours "$pid" && [ "$i" -lt "$wait_s" ]; do
            sleep 1
            i=$((i + 1))
        done
        if is_ours "$pid"; then
            note "pid $pid ignored SIGTERM for ${wait_s}s; sending SIGKILL"
            kill -KILL "$pid" 2>/dev/null
            sleep 1
            note "check 'pactl list short sink-inputs': a killed daemon cannot clean up its pacat child"
        fi
        is_ours "$pid" && die "pid $pid is still running"
    else
        note "daemon not running"
    fi
    i=0
    while supervisor_alive && [ "$i" -lt 5 ]; do
        sleep 1
        i=$((i + 1))
    done
    if supervisor_alive; then
        note "supervisor still running; sending SIGKILL"
        kill -KILL "$(read_pid "$SUPPID")" 2>/dev/null
    fi
    if rotator_alive; then
        kill -TERM "$(read_pid "$ROTPID")" 2>/dev/null
    fi
    rm -f "$PIDFILE" "$ROTPID" "$SUPPID"
}

# --- Binary selection (install.sh) -------------------------------------------
# A package carries webos-satellite-armv7 (static armv7 musl, the build
# verified on c3-a) and webos-satellite-aarch64 (static aarch64 musl).

# Prints the build to try first for `uname -m` output $1. aarch64 means a
# 64-bit kernel: c3-a reports it for every process although its userland is
# 32-bit (P1 runtime smoke), and a static aarch64 binary ran there. armv7l
# (32-bit kernel), armv8l (32-bit personality on a 64-bit kernel) and anything
# unknown get armv7, which runs on both.
arch_for_machine() {
    case $1 in
    aarch64 | arm64) echo aarch64 ;;
    *) echo armv7 ;;
    esac
}

# Rust target triple that `--version` of the build for arch $1 reports.
# Host tests set WEBOS_SAT_TEST_ANY_TRIPLE=1 to accept a host build.
triple_of_arch() {
    if [ "${WEBOS_SAT_TEST_ANY_TRIPLE:-}" = 1 ]; then
        echo ""
        return
    fi
    case $1 in
    aarch64) echo aarch64-unknown-linux-musl ;;
    armv7) echo armv7-unknown-linux-musleabi ;;
    *) echo "" ;;
    esac
}

# True if binary $1 runs here: `--version` exits 0 and prints
# "webos-satellite <version> (<triple>)", with triple $2 when it is given.
binary_runs() {
    out=$("$1" --version 2>/dev/null) || return 1
    case $out in
    "webos-satellite "*) ;;
    *) return 1 ;;
    esac
    [ -z "${2:-}" ] && return 0
    case $out in
    *"($2)"*) return 0 ;;
    esac
    return 1
}
