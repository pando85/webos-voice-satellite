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
    / | /bin | /boot | /dev | /etc | /home | /lib | /media | /media/developer | /mnt | /opt | /proc | /root | /run | /sbin | /sys | /tmp | /usr | /var)
        die "refusing to use a system directory as the prefix: $p"
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
# shellcheck disable=SC2034
KILL_SWITCH_DEFAULT=/tmp/webos-satellite.disable

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
