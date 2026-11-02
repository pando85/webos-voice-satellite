#!/bin/sh
# Prints whether the daemon runs. Exit 0: running, 3: not running.
#
# usage: status.sh [--prefix DIR]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    -h | --help) sed -n '2,5p' "$0"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done
resolve_prefix "$0" "$prefix_arg"
require_installed

if supervisor_alive; then
    note "supervisor running, pid $(read_pid "$SUPPID")"
fi
[ ! -e "$DISABLE" ] || note "marker $DISABLE present: starts are refused"
[ ! -e "$CRASHLOOP" ] || note "marker $CRASHLOOP present: the supervisor gave up ($(head -n 1 "$CRASHLOOP"))"
[ ! -e "$KILL_SWITCH_DEFAULT" ] || note "kill switch $KILL_SWITCH_DEFAULT present: the microphone is off"
note "binary: $("$BIN" --version 2>/dev/null || echo "$BIN does not run")"
pid=$(read_pid "$PIDFILE")
if is_ours "$pid"; then
    # shellcheck disable=SC2012 # fd names are numbers
    fds=$(ls "/proc/$pid/fd" 2>/dev/null | wc -l)
    thr=$(grep '^Threads:' "/proc/$pid/status" 2>/dev/null || true)
    note "fds $fds, ${thr:-Threads: ?}"
    rss=$(grep '^VmRSS:' "/proc/$pid/status" 2>/dev/null || true)
    note "running, pid $pid ${rss:+($rss)}"
    note "log: $LOG ($(wc -c <"$LOG" 2>/dev/null || echo 0) bytes)"
    tail -n 5 "$LOG" 2>/dev/null || true
    exit 0
fi
note "not running"
exit 3
