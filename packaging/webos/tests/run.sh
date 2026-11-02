#!/bin/sh
# Host test of the ops scripts: install, start --supervise, restart after a
# crash, crash-loop guard, autostart gating (disable marker, firmware guard),
# stop and uninstall. Runs the host build of webos-satellite with a file mic,
# the null sink and plaintext on 127.0.0.1. Touches nothing outside a
# temporary directory, except that it refuses to run while the default kill
# switch /tmp/webos-satellite.disable exists.
#
# usage: packaging/webos/tests/run.sh [BINARY]   (default: builds it)
# The checks are strings run with eval, so single quotes are intended.
# shellcheck disable=SC2016,SC2034,SC1091
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
PKG=$(cd "$HERE/.." && pwd)
ROOT=$(cd "$PKG/../.." && pwd)
BIN=${1:-}
if [ -z "$BIN" ]; then
    (cd "$ROOT" && cargo build --quiet -p webos-satellite)
    BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug/webos-satellite
fi
[ -x "$BIN" ] || { echo "binary not found: $BIN" >&2; exit 2; }
[ ! -e /tmp/webos-satellite.disable ] || { echo "/tmp/webos-satellite.disable exists; remove it first" >&2; exit 2; }

T=$(mktemp -d "${TMPDIR:-/tmp}/webos-sat-pkgtest.XXXXXX")
P=$T/prefix
fails=0
cleanup() {
    [ ! -f "$P/.webos-satellite-install" ] || sh "$P/bin/stop.sh" >/dev/null 2>&1 || true
    [ -n "${HOLD:-}" ] && kill "$HOLD" 2>/dev/null || true
    rm -rf "$T"
}
trap cleanup EXIT INT TERM

check() {
    if eval "$2"; then
        echo "PASS  $1"
    else
        echo "FAIL  $1"
        fails=$((fails + 1))
    fi
}

wait_for() { # SECONDS CONDITION
    i=0
    while [ "$i" -lt "$(($1 * 10))" ]; do
        eval "$2" && return 0
        sleep 0.1
        i=$((i + 1))
    done
    return 1
}

port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
head -c 96000 /dev/zero >"$T/mic.raw"
printf '{\n  "webos_manufacturing_version": "99.99.99",\n  "webos_release": "0.0.0"\n}\n' >"$T/os_info.json"
cat >"$T/sat.toml" <<EOF
mode = "wake"
[device]
name = "pkg-test"
bind = "127.0.0.1"
port = $port
allow_plaintext = true
[mic]
source = "file"
path = "$T/mic.raw"
loop = true
[sink]
type = "null"
[safety]
kill_switch = "$T/kill"
mute_file = "$T/muted"
validated_firmware = ["99.99.99"]
firmware_info = "$T/os_info.json"
[limits]
loop_stall_s = 5
EOF

echo "== ops scripts on the host (prefix $P) =="
sh "$PKG/install.sh" --prefix "$P" --config "$T/sat.toml" --binary "$BIN" --persistent >/dev/null
check "install copies supervise.sh and autostart.sh" '[ -x "$P/bin/supervise.sh" ] && [ -x "$P/bin/autostart.sh" ]'

check "--check-firmware: validated (exit 0)" '"$P/bin/webos-satellite" --config "$P/etc/config.toml" --check-firmware >/dev/null'
sed 's/"99.99.99"]/"01.00.00"]/' "$T/sat.toml" >"$T/bad.toml"
"$P/bin/webos-satellite" --config "$T/bad.toml" --check-firmware >/dev/null && rc=0 || rc=$?
check "--check-firmware: not validated (exit 1)" '[ "$rc" -eq 1 ]'

sh "$P/bin/start.sh" --supervise >/dev/null
. "$PKG/lib.sh"
set_paths "$P"
pid1=$(read_pid "$PIDFILE")
check "start --supervise: daemon and supervisor running" 'is_ours "$pid1" && supervisor_alive'

kill -KILL "$pid1"
wait_for 5 'pid2=$(read_pid "$PIDFILE"); [ -n "$pid2" ] && [ "$pid2" != "$pid1" ] && is_ours "$pid2"' || true
check "crash (SIGKILL) restarted after the backoff" '[ -n "${pid2:-}" ] && [ "$pid2" != "$pid1" ]'
check "supervisor logged the exit status" 'grep -q "SUPER daemon exited with status 137" "$LOG"'

: >"$T/kill"
check "kill switch: daemon logs the microphone closed" 'wait_for 3 "grep -q \"KILL SWITCH\" \"\$LOG\""'
rm -f "$T/kill"

sh "$P/bin/stop.sh" >/dev/null
check "stop.sh stops daemon and supervisor, no restart" '! supervisor_alive && ! is_ours "$pid2" && sleep 2 && [ -z "$(read_pid "$PIDFILE")" ]'

for what in loop capture; do
    code=70
    [ "$what" = loop ] || code=71
    sh "$P/bin/start.sh" --supervise -- --test-hang "$what" >/dev/null
    check "watchdog: frozen $what -> exit $code, supervisor restarts" 'wait_for 20 "grep -q \"SUPER daemon exited with status $code\" \"\$LOG\""'
    sh "$P/bin/stop.sh" >/dev/null
done

# Crash loop: the port is taken, every start fails at once.
python3 -c "import socket,time; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('127.0.0.1',$port)); s.listen(); time.sleep(60)" &
HOLD=$!
sleep 0.5
WEBOS_SAT_MAX_CRASHES=3 WEBOS_SAT_CRASH_WINDOW=60 sh "$P/bin/start.sh" --supervise >/dev/null 2>&1 || true
check "crash loop: supervisor gives up and writes the marker" 'wait_for 10 "[ -e \"$P/crashloop\" ] && ! supervisor_alive"'
check "crash loop: 3 exits logged, backoff 1 s then 2 s" 'grep -q "3 of 3" "$LOG" && grep -q "restarting in 1s" "$LOG" && grep -q "restarting in 2s" "$LOG"'
sh "$P/bin/start.sh" --supervise >/dev/null 2>&1 && rc=0 || rc=$?
check "start --supervise refuses while the crash-loop marker exists" '[ "$rc" -ne 0 ] && ! supervisor_alive'
kill "$HOLD" 2>/dev/null || true
HOLD=
rm -f "$P/crashloop"

: >"$P/disable"
sh "$P/bin/autostart.sh" --delay 0
check "autostart: disable marker honoured" 'grep -q "disable present; not starting" "$P/log/autostart.log" && ! supervisor_alive'
sh "$P/bin/start.sh" >/dev/null 2>&1 && rc=0 || rc=$?
check "start.sh refuses while the disable marker exists" '[ "$rc" -ne 0 ]'
rm -f "$P/disable"

cp "$T/bad.toml" "$P/etc/config.toml"
sh "$P/bin/autostart.sh" --delay 0
check "autostart: firmware guard failure prevents the start" 'grep -q "firmware check failed (status 1)" "$P/log/autostart.log" && ! supervisor_alive'
cp "$T/sat.toml" "$P/etc/config.toml"

sh "$P/bin/autostart.sh" --delay 0
pid3=$(read_pid "$PIDFILE")
check "autostart: starts the supervised daemon when all checks pass" 'is_ours "$pid3" && supervisor_alive'

sh "$P/bin/uninstall.sh" >/dev/null
check "uninstall: stops everything and removes the prefix" '[ ! -e "$P" ] && ! is_ours "$pid3"'

t0=$(date +%s)
out=$(sh "$PKG/init.d/webos-satellite" 2>&1) && rc=0 || rc=$?
check "init.d hook without an install: exit 0 at once, no output" '[ "$rc" -eq 0 ] && [ -z "$out" ] && [ $(($(date +%s) - t0)) -le 1 ]'

if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) failed; log:"
    cat "$LOG" 2>/dev/null || true
    exit 1
fi
