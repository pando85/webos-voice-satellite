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
    LOG=$PREFIX/log/webos-satellite.log
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

# Stops the daemon (TERM, wait, then KILL) and the log rotator, and removes
# the pidfiles. Argument: seconds to wait (default 10).
stop_daemon() {
    wait_s=${1:-10}
    pid=$(read_pid "$PIDFILE")
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
        note "not running"
    fi
    if rotator_alive; then
        kill -TERM "$(read_pid "$ROTPID")" 2>/dev/null
    fi
    rm -f "$PIDFILE" "$ROTPID"
}
