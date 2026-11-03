#!/bin/sh
# Starts the daemon in the background. PID in DIR/run, log in DIR/log
# (rotated by size, see rotate.sh). Extra arguments go to the daemon.
#
# usage: start.sh [--prefix DIR] [--supervise] [-- DAEMON_ARGS...]
#
#   --supervise   run the daemon under supervise.sh (restart with backoff,
#                 crash-loop guard); refuses while DIR/crashloop exists
#
# Refuses to start while DIR/disable exists.
#
# Environment: WEBOS_SAT_LOG_MAX (bytes, default 262144) is the log size at
# which the log is moved to webos-satellite.log.1 (one generation is kept).
# supervise.sh reads WEBOS_SAT_MAX_CRASHES, WEBOS_SAT_CRASH_WINDOW and
# WEBOS_SAT_BACKOFF_MAX.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
supervise=0
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    --supervise) supervise=1; shift ;;
    -h | --help) sed -n '2,17p' "$0"; exit 0 ;;
    --) shift; break ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done
close_inherited_fds "$0"
resolve_prefix "$0" "$prefix_arg"
require_installed
[ -x "$BIN" ] || die "binary missing: $BIN"
[ -f "$CFG" ] || die "config missing: $CFG"

pid=$(read_pid "$PIDFILE")
if is_ours "$pid"; then
    note "already running (pid $pid)"
    exit 0
fi
if supervisor_alive; then
    note "supervisor already running (pid $(read_pid "$SUPPID")); the daemon is between restarts"
    exit 0
fi
[ ! -e "$DISABLE" ] || die "$DISABLE exists: not starting (remove it to allow starts)"
if [ "$supervise" -eq 1 ] && [ -e "$CRASHLOOP" ]; then
    die "$CRASHLOOP exists (the supervisor gave up after a crash loop): fix the cause, remove the file, then start"
fi
rm -f "$PIDFILE" "$ROTPID" "$SUPPID"

umask 077
max=${WEBOS_SAT_LOG_MAX:-262144}
case $max in
'' | *[!0-9]*) die "WEBOS_SAT_LOG_MAX must be a number of bytes" ;;
esac
if [ -f "$LOG" ] && [ "$(wc -c <"$LOG")" -gt "$max" ]; then
    mv -f "$LOG" "$LOG.1"
fi

cd "$PREFIX"
# New session (setsid when present, see lib.sh): the daemon must not depend
# on the caller's session, for example a Homebrew Channel exec from the app.
# shellcheck disable=SC2086 # $SETSID is empty or one word
if [ "$supervise" -eq 1 ]; then
    $SETSID nohup sh "$PREFIX/bin/supervise.sh" -- "$@" </dev/null >/dev/null 2>&1 &
    watch=$!
    echo "$watch" >"$SUPPID"
else
    $SETSID nohup "$BIN" --config "$CFG" "$@" </dev/null >>"$LOG" 2>&1 &
    watch=$!
    echo "$watch" >"$PIDFILE"
fi
# The rotator exits with the process it watches (the supervisor or the daemon).
# shellcheck disable=SC2086
$SETSID nohup sh "$PREFIX/bin/rotate.sh" "$watch" "$LOG" "$max" </dev/null >/dev/null 2>&1 &
echo $! >"$ROTPID"

sleep 1
pid=$(read_pid "$PIDFILE")
if ! is_ours "$pid"; then
    note "the daemon is not running 1 s after start; last log lines:"
    tail -n 10 "$LOG" >&2 || true
    if [ "$supervise" -eq 1 ] && supervisor_alive; then
        note "the supervisor (pid $watch) keeps retrying with backoff; stop it with stop.sh"
        exit 1
    fi
    rm -f "$PIDFILE"
    exit 1
fi
if [ "$supervise" -eq 1 ]; then
    note "started pid $pid under supervisor pid $watch, log $LOG"
else
    note "started pid $pid, log $LOG"
fi
