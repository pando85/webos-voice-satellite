#!/bin/sh
# Runs the daemon in a restart loop with exponential backoff and a crash-loop
# guard. Started in the background by `start.sh --supervise`; not meant to be
# run by hand. Installs nothing and writes only under the prefix.
#
# usage: supervise.sh [--prefix DIR] [-- DAEMON_ARGS...]
#
# - Daemon exit 0 (it stopped on SIGTERM/SIGINT): supervision ends.
# - Any other exit (error, watchdog codes 70-72, killed by a signal): restart
#   after 1, 2, 4, ... s, at most WEBOS_SAT_BACKOFF_MAX s (default 300).
# - WEBOS_SAT_MAX_CRASHES (default 5) exits within WEBOS_SAT_CRASH_WINDOW s
#   (default 600): writes DIR/crashloop and stops for good. Nothing starts
#   again (here or from autostart) until that file is removed by hand.
# - DIR/disable present: does not (re)start.
# - SIGTERM/SIGINT/SIGHUP: stops the daemon (SIGTERM) and exits.
# - After every daemon exit (before a restart, and before supervision ends):
#   if run/lg_voice_override exists, LG's voice key is given back with
#   `ctl.sh restore-lg-voice` (ADR 0013); the next daemon takes it again.
set -u
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    -h | --help) sed -n '2,20p' "$0"; exit 0 ;;
    --) shift; break ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done
resolve_prefix "$0" "$prefix_arg"
require_installed

max_crashes=${WEBOS_SAT_MAX_CRASHES:-5}
window=${WEBOS_SAT_CRASH_WINDOW:-600}
backoff_max=${WEBOS_SAT_BACKOFF_MAX:-300}
for v in "$max_crashes" "$window" "$backoff_max"; do
    case $v in
    '' | *[!0-9]*) die "WEBOS_SAT_MAX_CRASHES, WEBOS_SAT_CRASH_WINDOW and WEBOS_SAT_BACKOFF_MAX must be numbers" ;;
    esac
done
[ "$max_crashes" -ge 1 ] || die "WEBOS_SAT_MAX_CRASHES must be at least 1"

slog() {
    echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) SUPER $*" >>"$LOG"
}

# LG's voice key back after a daemon exit, when the daemon left its state
# file (a crash, a kill, a watchdog exit; a clean exit removes it itself).
lg_restore() {
    [ -f "$PREFIX/run/lg_voice_override" ] || return 0
    slog "the daemon left LG's voice key taken over: giving it back (ctl.sh restore-lg-voice)"
    if [ -f "$here/ctl.sh" ] &&
        WEBOS_SAT_PREFIX=$PREFIX sh "$here/ctl.sh" restore-lg-voice >>"$LOG" 2>&1; then
        slog "LG's voice key given back"
    else
        slog "LG's voice key could NOT be given back: run ctl.sh restore-lg-voice or reboot the TV"
    fi
}

stopping=0
child=
# shellcheck disable=SC2329 # invoked by the trap
on_signal() {
    stopping=1
    [ -z "$child" ] || kill -TERM "$child" 2>/dev/null
}
trap on_signal TERM INT HUP

echo $$ >"$SUPPID"
if [ -e "$CRASHLOOP" ]; then
    slog "crash-loop marker $CRASHLOOP present: not starting (remove it after fixing the cause)"
    rm -f "$SUPPID"
    exit 1
fi
slog "supervisor started (pid $$; give up after $max_crashes exits within ${window}s)"

crashes=
while [ "$stopping" -eq 0 ]; do
    if [ -e "$DISABLE" ]; then
        slog "$DISABLE present: not starting"
        break
    fi
    "$BIN" --config "$CFG" "$@" </dev/null >>"$LOG" 2>&1 &
    child=$!
    echo "$child" >"$PIDFILE"
    started=$(date +%s)
    # A trapped signal interrupts `wait`; wait again until the child is reaped.
    while :; do
        wait "$child"
        rc=$?
        kill -0 "$child" 2>/dev/null || break
    done
    child=
    rm -f "$PIDFILE"
    lg_restore
    if [ "$stopping" -ne 0 ]; then
        slog "stopped by signal (daemon exit $rc)"
        break
    fi
    if [ "$rc" -eq 0 ]; then
        slog "daemon exited normally; supervision ends"
        break
    fi
    now=$(date +%s)
    kept=
    n=0
    for t in $crashes $now; do
        if [ $((now - t)) -lt "$window" ]; then
            kept="$kept $t"
            n=$((n + 1))
        fi
    done
    crashes=$kept
    slog "daemon exited with status $rc after $((now - started))s ($n of $max_crashes within ${window}s)"
    if [ "$n" -ge "$max_crashes" ]; then
        {
            echo "webos-satellite crash loop: $n exits within ${window}s, last status $rc"
            date -u +%Y-%m-%dT%H:%M:%SZ
            echo "remove this file to allow starting again"
        } >"$CRASHLOOP"
        slog "CRASH LOOP: giving up; wrote $CRASHLOOP"
        rm -f "$SUPPID"
        exit 1
    fi
    delay=1
    i=1
    while [ "$i" -lt "$n" ] && [ "$delay" -lt "$backoff_max" ]; do
        delay=$((delay * 2))
        i=$((i + 1))
    done
    [ "$delay" -le "$backoff_max" ] || delay=$backoff_max
    slog "restarting in ${delay}s"
    i=0
    while [ "$i" -lt "$delay" ] && [ "$stopping" -eq 0 ]; do
        sleep 1
        i=$((i + 1))
    done
done
rm -f "$SUPPID"
exit 0
