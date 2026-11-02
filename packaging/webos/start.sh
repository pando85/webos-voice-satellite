#!/bin/sh
# Starts the daemon in the background. PID in DIR/run, log in DIR/log
# (rotated by size, see rotate.sh). Extra arguments go to the daemon.
#
# usage: start.sh [--prefix DIR] [-- DAEMON_ARGS...]
#
# Environment: WEBOS_SAT_LOG_MAX (bytes, default 262144) is the log size at
# which the log is moved to webos-satellite.log.1 (one generation is kept).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    -h | --help) sed -n '2,9p' "$0"; exit 0 ;;
    --) shift; break ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done
resolve_prefix "$0" "$prefix_arg"
require_installed
[ -x "$BIN" ] || die "binary missing: $BIN"
[ -f "$CFG" ] || die "config missing: $CFG"

pid=$(read_pid "$PIDFILE")
if is_ours "$pid"; then
    note "already running (pid $pid)"
    exit 0
fi
rm -f "$PIDFILE" "$ROTPID"

umask 077
max=${WEBOS_SAT_LOG_MAX:-262144}
case $max in
'' | *[!0-9]*) die "WEBOS_SAT_LOG_MAX must be a number of bytes" ;;
esac
if [ -f "$LOG" ] && [ "$(wc -c <"$LOG")" -gt "$max" ]; then
    mv -f "$LOG" "$LOG.1"
fi

cd "$PREFIX"
nohup "$BIN" --config "$CFG" "$@" </dev/null >>"$LOG" 2>&1 &
pid=$!
echo "$pid" >"$PIDFILE"
nohup sh "$PREFIX/bin/rotate.sh" "$pid" "$LOG" "$max" </dev/null >/dev/null 2>&1 &
echo $! >"$ROTPID"

sleep 1
if ! is_ours "$pid"; then
    rm -f "$PIDFILE"
    note "the daemon exited right after start; last log lines:"
    tail -n 10 "$LOG" >&2 || true
    exit 1
fi
note "started pid $pid, log $LOG"
