#!/bin/sh
# Host test of the ops scripts: install, start --supervise, restart after a
# crash, crash-loop guard, autostart gating (disable marker, firmware guard),
# the permission checks before root runs anything (a world-writable binary,
# another user's files, run/ not 0700, the supervisor and the boot hook),
# stop and uninstall, and install.sh's choice between the armv7 and aarch64
# builds (stand-in binaries, `uname -m` replaced by WEBOS_SAT_UNAME_M). Runs the host build of webos-satellite with a file mic,
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
HOST_BIN=$BIN # BIN is reused by lib.sh's set_paths below
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
firmware_policy = "strict"
validated_firmware = ["99.99.99"]
firmware_info = "$T/os_info.json"
[limits]
loop_stall_s = 5
EOF

echo "== ops scripts on the host (prefix $P) =="
sh "$PKG/install.sh" --prefix "$P" --config "$T/sat.toml" --binary "$BIN" --persistent >/dev/null
check "install copies supervise.sh and autostart.sh" '[ -x "$P/bin/supervise.sh" ] && [ -x "$P/bin/autostart.sh" ]'

check "--check-firmware: in the list (exit 0)" '"$P/bin/webos-satellite" --config "$P/etc/config.toml" --check-firmware >/dev/null'
sed 's/"99.99.99"]/"01.00.00"]/' "$T/sat.toml" >"$T/bad.toml"
"$P/bin/webos-satellite" --config "$T/bad.toml" --check-firmware >/dev/null && rc=0 || rc=$?
check "--check-firmware: strict policy, not in the list (exit 1)" '[ "$rc" -eq 1 ]'
# The default policy (warn): the same firmware only reports "not verified".
sed -e 's/^firmware_policy = .*/firmware_policy = "warn"/' -e 's/^validated_firmware = .*/validated_firmware = []/' "$T/sat.toml" >"$T/warn.toml"
out=$("$P/bin/webos-satellite" --config "$T/warn.toml" --check-firmware 2>&1) && rc=0 || rc=$?
check "--check-firmware: warn policy, unverified firmware (exit 1, says not verified)" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "not verified"'
sed -e 's/^firmware_policy = .*/firmware_policy = "warn"/' "$T/sat.toml" >"$T/warn-listed.toml"
"$P/bin/webos-satellite" --config "$T/warn-listed.toml" --check-firmware >/dev/null && rc=0 || rc=$?
check "--check-firmware: warn policy, firmware in the owner's list (exit 0)" '[ "$rc" -eq 0 ]'
sed -e 's/^validated_firmware = .*/validated_firmware = []/' "$T/sat.toml" >"$T/strict-empty.toml"
"$P/bin/webos-satellite" --config "$T/strict-empty.toml" --check-firmware >/dev/null && rc=0 || rc=$?
check "--check-firmware: strict policy, empty list (exit 3)" '[ "$rc" -eq 3 ]'

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

# ADR 0013: a daemon that dies while it holds LG's voice key (state file
# run/lg_voice_override) gets the key given back by the supervisor before the
# restart (ctl.sh restore-lg-voice; a fake luna-send models configd's
# in-memory value in $T/kav).
cat >"$T/luna-send" <<'FAKE'
#!/bin/sh
echo "$5 $6" >>"${KA_DIR:?}/calls"
case $5 in
*/setConfigs) case $6 in *null*) rm -f "$KA_DIR/kav" ;; esac; printf '{"returnValue": true}\n' ;;
*/getConfigs) printf '{"configs":{"com.webos.keyaction.voice":{"action":"%s"}},"returnValue":true}\n' "$(cat "$KA_DIR/kav" 2>/dev/null || echo launch)" ;;
esac
FAKE
chmod 755 "$T/luna-send"
KA_DIR=$T WEBOS_SAT_LUNA_SEND=$T/luna-send WEBOS_SAT_ALLOW_NONROOT=1 sh "$P/bin/start.sh" --supervise >/dev/null
pk=$(read_pid "$PIDFILE")
echo ignore >"$T/kav"
echo "configd 1" >"$P/run/lg_voice_override"
kill -KILL "$pk"
wait_for 10 'pk2=$(read_pid "$PIDFILE"); [ -n "$pk2" ] && [ "$pk2" != "$pk" ] && is_ours "$pk2"' || true
check "a crash while LG's voice key is ours: the supervisor gives it back before the restart" '[ ! -e "$P/run/lg_voice_override" ] && [ ! -e "$T/kav" ] && grep -q "SUPER LG.s voice key given back" "$LOG" && [ -n "${pk2:-}" ] && [ "$pk2" != "$pk" ]'
check "the supervisor's restore was volatile and named the one key" 'grep -q "setConfigs {\"configs\":{\"com.webos.keyaction.voice\":null},\"volatile\":true}" "$T/calls"'
sh "$P/bin/stop.sh" >/dev/null

# A restore that hangs in the supervisor (configd does not answer) must not
# outlive a stop: stop_daemon kills the supervisor's whole process group, so
# no orphan restore can remove a newer acquire's state file later.
mkdir -p "$T/hang"
printf '#!/bin/sh\nexec sleep 30\n' >"$T/hang/luna-send-hang"
chmod 755 "$T/hang/luna-send-hang"
KA_DIR=$T WEBOS_SAT_LUNA_SEND=$T/hang/luna-send-hang WEBOS_SAT_ALLOW_NONROOT=1 sh "$P/bin/start.sh" --supervise >/dev/null
ph=$(read_pid "$PIDFILE")
echo "configd 1 hang" >"$P/run/lg_voice_override"
kill -KILL "$ph"
wait_for 5 'pgrep -f "$T/hang/luna-send-hang" >/dev/null' || true
check "the supervisor's restore is running (configd hangs)" 'pgrep -f "$T/hang/luna-send-hang" >/dev/null'
# The wait for a restoring supervisor (45 s on the TV) cut short, to
# exercise the kill.
WEBOS_SAT_RESTORE_WAIT=2 sh "$P/bin/stop.sh" >/dev/null 2>&1 || true
wait_for 3 '! pgrep -f "$P/bin/ctl.sh restore-lg-voice" >/dev/null' || true
check "stop kills the supervisor's process group: no ctl.sh restore is left to remove a state file later, and it was not removed" '! pgrep -f "$P/bin/ctl.sh restore-lg-voice" >/dev/null && ! supervisor_alive && [ "$(cat "$P/run/lg_voice_override")" = "configd 1 hang" ]'
# The luna-send it had started is bounded by its own timeout (6 s): it
# removes no file; at most its call reaches configd late.
wait_for 15 '! pgrep -f "$T/hang/luna-send-hang" >/dev/null' || true
check "the luna call it left ends by its own timeout" '! pgrep -f "$T/hang/luna-send-hang" >/dev/null'

# A slow restore in the supervisor (about 6 s, longer than the plain 5 s a
# stop gives the supervisor) is allowed to finish: the stop waits up to 45 s
# while the state file exists.
cat >"$T/luna-send-slow" <<'FAKE'
#!/bin/sh
sleep 3
case $5 in
*/setConfigs) case $6 in *null*) rm -f "$KA_DIR/kav" ;; esac; printf '{"returnValue": true}\n' ;;
*/getConfigs) printf '{"configs":{"com.webos.keyaction.voice":{"action":"%s"}},"returnValue":true}\n' "$(cat "$KA_DIR/kav" 2>/dev/null || echo launch)" ;;
esac
FAKE
chmod 755 "$T/luna-send-slow"
KA_DIR=$T WEBOS_SAT_LUNA_SEND=$T/luna-send-slow WEBOS_SAT_ALLOW_NONROOT=1 sh "$P/bin/start.sh" --supervise >/dev/null
echo ignore >"$T/kav"
echo "configd 1 slow" >"$P/run/lg_voice_override"
KA_DIR=$T WEBOS_SAT_LUNA_SEND=$T/luna-send-slow WEBOS_SAT_ALLOW_NONROOT=1 sh "$P/bin/stop.sh" >/dev/null 2>&1 || true
check "stop gives a slow restore in the supervisor time to finish (no kill at 5 s): LG's key back, state file gone" '[ ! -e "$P/run/lg_voice_override" ] && [ ! -e "$T/kav" ] && ! supervisor_alive'
# stop.sh with no supervisor and a state file left (an unsupervised crash):
# stop.sh itself gives the key back.
echo ignore >"$T/kav"
echo "configd 1 left" >"$P/run/lg_voice_override"
KA_DIR=$T WEBOS_SAT_LUNA_SEND=$T/luna-send WEBOS_SAT_ALLOW_NONROOT=1 sh "$P/bin/stop.sh" >/dev/null 2>&1 || true
check "stop.sh gives a leftover state file's key back itself" '[ ! -e "$P/run/lg_voice_override" ] && [ ! -e "$T/kav" ]'
rm -f "$P/run/lg_voice_override"

# The remote grab helper (ADR 0016) left by a dead daemon: stop.sh kills it
# by its pid file, only when that pid runs the installed helper binary.
cp "$(command -v sleep)" "$P/bin/remote-proxy"
chmod 755 "$P/bin/remote-proxy"
"$P/bin/remote-proxy" 300 &
GP=$!
echo "$GP" >"$P/run/remote-proxy.pid"
sleep 300 &
OTHER=$!
sh "$P/bin/stop.sh" >/dev/null 2>&1 || true
check "stop.sh kills a grab helper left behind (pid file, our binary) and removes the pid file" '! kill -0 "$GP" 2>/dev/null && [ ! -e "$P/run/remote-proxy.pid" ]'
echo "$OTHER" >"$P/run/remote-proxy.pid"
sh "$P/bin/stop.sh" >/dev/null 2>&1 || true
check "stop.sh leaves a process alone whose pid is in the file but that is not our helper" 'kill -0 "$OTHER" 2>/dev/null && [ ! -e "$P/run/remote-proxy.pid" ]'
kill "$OTHER" 2>/dev/null || true
wait "$GP" 2>/dev/null || true
rm -f "$P/bin/remote-proxy"

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
check "autostart: strict policy and a firmware not in the list prevent the start" 'grep -q "firmware check failed (status 1)" "$P/log/autostart.log" && grep -q "strict, not starting" "$P/log/autostart.log" && ! supervisor_alive'
# The default policy never blocks: an unverified firmware is logged and the
# daemon starts (owner decision 2026-11-04).
cp "$T/warn.toml" "$P/etc/config.toml"
sh "$P/bin/autostart.sh" --delay 0
pidw=$(read_pid "$PIDFILE")
check "autostart: warn policy and an unverified firmware: logged, started anyway" 'grep -q "starting anyway (firmware_policy warn)" "$P/log/autostart.log" && is_ours "$pidw" && supervisor_alive'
sh "$P/bin/stop.sh" >/dev/null
cp "$T/sat.toml" "$P/etc/config.toml"

# Permissions of what root runs (docs/security/install-permissions.md).
chmod o+w "$P/bin/webos-satellite"
out=$(sh "$P/bin/start.sh" --supervise 2>&1) && rc=0 || rc=$?
check "permissions: a world-writable binary: start.sh refuses, names it, starts nothing" '[ "$rc" -ne 0 ] && echo "$out" | grep -q "bin/webos-satellite is writable by group or others" && ! supervisor_alive && [ -z "$(read_pid "$PIDFILE")" ]'
sh "$P/bin/autostart.sh" --delay 0
check "permissions: a world-writable binary: autostart logs it and starts nothing" 'grep -q "the install is not trusted: .*writable by group or others; not starting" "$P/log/autostart.log" && ! supervisor_alive'
out=$(WEBOS_SAT_PREFIX=$P WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LOCK=$T/lock sh "$P/bin/ctl.sh" start 2>&1) && rc=0 || rc=$?
check "permissions: a world-writable binary: ctl.sh start refuses before it runs start.sh" '[ "$rc" -ne 0 ] && echo "$out" | grep -q "refusing to run from" && ! supervisor_alive'
chmod o-w "$P/bin/webos-satellite"
chmod g+w "$P/etc/config.toml"
out=$(sh "$P/bin/start.sh" --supervise 2>&1) && rc=0 || rc=$?
check "permissions: a group-writable config: start.sh refuses" '[ "$rc" -ne 0 ] && echo "$out" | grep -q "config.toml is writable by group or others" && ! supervisor_alive'
chmod g-w "$P/etc/config.toml"
# Another user's files (the test's own uid stands in for "not root").
out=$(WEBOS_SAT_TEST_TRUST_UID=99999 sh "$P/bin/start.sh" --supervise 2>&1) && rc=0 || rc=$?
check "permissions: files of another user: start.sh refuses, names the owner" '[ "$rc" -ne 0 ] && echo "$out" | grep -q "is owned by uid $(id -u), not root" && ! supervisor_alive'
# run/ not 0700, with a link planted in it: made 0700, the link deleted, logged.
chmod 755 "$P/run"
ln -s "$T/victim" "$P/run/voice.json"
out=$(sh "$P/bin/start.sh" --supervise 2>&1) && rc=0 || rc=$?
check "permissions: run/ 0755 with a planted link: start fixes it to 0700, deletes the link, says so, starts" '[ "$rc" -eq 0 ] && [ "$(stat -c %a "$P/run")" = 700 ] && [ ! -L "$P/run/voice.json" ] && [ ! -e "$T/victim" ] && echo "$out" | grep -q "fixed the permissions of:.*run (was drwxr-xr-x)" && echo "$out" | grep -q "deleted $P/run/voice.json" && supervisor_alive'
# The supervisor checks again before every restart.
pids=$(read_pid "$PIDFILE")
chmod o+w "$P/bin/webos-satellite"
kill -KILL "$pids"
check "permissions: the supervisor refuses a restart once the binary is world-writable, and ends" 'wait_for 8 "! supervisor_alive" && grep -q "SUPER the install is not trusted, nothing is run from it: .*writable by group or others" "$LOG" && grep -q "SUPER NOT STARTING the daemon" "$LOG" && [ -z "$(read_pid "$PIDFILE")" ]'
# The boot hook runs nothing from an install it cannot trust.
sed "s|@PREFIX@|$P|" "$PKG/init.d/webos-satellite" >"$T/hook"
chmod o-w "$P/bin/webos-satellite"
chmod o+w "$P/bin/autostart.sh"
out=$(sh "$T/hook" 2>&1) && rc=0 || rc=$?
check "permissions: boot hook: exit 0, no output, autostart.sh not launched when it is world-writable" '[ "$rc" -eq 0 ] && [ -z "$out" ] && ! wait_for 1 "pgrep -f \"$P/bin/autostart.sh\" >/dev/null"'
chmod o-w "$P/bin/autostart.sh"

sh "$P/bin/autostart.sh" --delay 0
pid3=$(read_pid "$PIDFILE")
check "autostart: starts the supervised daemon when all checks pass" 'is_ours "$pid3" && supervisor_alive'

# A state file left while the daemon runs (as after an earlier crash with a
# failed restore): uninstall gives LG's key back before it removes the prefix.
echo ignore >"$T/kav"
echo "configd 1 uninstall" >"$P/run/lg_voice_override"
KA_DIR=$T WEBOS_SAT_LUNA_SEND=$T/luna-send WEBOS_SAT_ALLOW_NONROOT=1 sh "$P/bin/uninstall.sh" >/dev/null 2>"$T/uninstall.err"
check "uninstall: stops everything and removes the prefix" '[ ! -e "$P" ] && ! is_ours "$pid3"'
check "uninstall: LG's voice key given back before the state file went with the prefix" '[ ! -e "$T/kav" ]'

t0=$(date +%s)
out=$(sh "$PKG/init.d/webos-satellite" 2>&1) && rc=0 || rc=$?
check "init.d hook without an install: exit 0 at once, no output" '[ "$rc" -eq 0 ] && [ -z "$out" ] && [ $(($(date +%s) - t0)) -le 1 ]'

echo "== install.sh: armv7 / aarch64 selection =="
# Stand-ins: "ok" answers --version with the build's triple and runs the host
# binary otherwise (for --check-config); "bad" fails like a binary for another
# CPU.
SEL=$T/sel
mkdir -p "$SEL"
cp "$PKG"/*.sh "$SEL/"
stand_in() { # ARCH ok|bad|ARCH_REPORTED
    f=$SEL/webos-satellite-$1
    if [ "$2" != bad ]; then
        reported=$1
        [ "$2" = ok ] || reported=$2
        printf '#!/bin/sh\n[ "$1" != --version ] || { echo "webos-satellite 0.0.1 (%s)"; exit 0; }\nexec "%s" "$@"\n' \
            "$(triple_of_arch "$reported")" "$HOST_BIN" >"$f"
    else
        printf '#!/bin/sh\necho "Exec format error" >&2\nexit 126\n' >"$f"
    fi
    chmod 755 "$f"
}
n=0
sel_install() { # UNAME_M [INSTALL ARGS...]; sets out, rc and dir
    n=$((n + 1))
    dir=$T/sel-$n
    m=$1
    shift
    out=$(WEBOS_SAT_UNAME_M=$m sh "$SEL/install.sh" --prefix "$dir" --config "$T/sat.toml" --persistent "$@" 2>&1) && rc=0 || rc=$?
}
installed() { # TRIPLE: the installed binary is that build
    "$dir/bin/webos-satellite" --version | grep -q "($1)"
}
A64=aarch64-unknown-linux-musl
A32=armv7-unknown-linux-musleabi
stand_in armv7 ok
stand_in aarch64 ok
sel_install aarch64
check "uname aarch64 (64-bit kernel): the aarch64 build" '[ "$rc" -eq 0 ] && installed $A64 && [ ! -e "$dir/bin/.webos-satellite.new" ]'
good=$dir
sel_install armv7l
check "uname armv7l (32-bit kernel): the armv7 build" '[ "$rc" -eq 0 ] && installed $A32'
sel_install armv8l
check "uname armv8l (32-bit personality): the armv7 build" '[ "$rc" -eq 0 ] && installed $A32'
sel_install x86_64
check "unknown uname: the armv7 build" '[ "$rc" -eq 0 ] && installed $A32'
sel_install aarch64 --arch armv7
check "--arch armv7 overrides an aarch64 kernel" '[ "$rc" -eq 0 ] && installed $A32'
sel_install armv7l --arch aarch64
check "--arch aarch64 is tried first on any kernel" '[ "$rc" -eq 0 ] && installed $A64'
sel_install aarch64 --arch mips
check "--arch with an unknown value is refused" '[ "$rc" -ne 0 ] && [ ! -e "$dir" ]'
stand_in aarch64 armv7
sel_install aarch64
check "a file named aarch64 that reports another target: armv7" '[ "$rc" -eq 0 ] && installed $A32'
stand_in aarch64 bad
sel_install aarch64
check "aarch64 build does not run: falls back to armv7" '[ "$rc" -eq 0 ] && installed $A32 && echo "$out" | grep -q "falling back to armv7"'
stand_in armv7 bad
sel_install aarch64
check "nothing runs: install refused, no prefix left behind" '[ "$rc" -ne 0 ] && [ ! -e "$dir" ]'
out=$(WEBOS_SAT_UNAME_M=aarch64 sh "$SEL/install.sh" --prefix "$good" --config "$T/sat.toml" --persistent 2>&1) && rc=0 || rc=$?
dir=$good
check "nothing runs: a re-install keeps the installed binary" '[ "$rc" -ne 0 ] && installed $A64 && [ ! -e "$dir/bin/.webos-satellite.new" ] && echo "$out" | grep -q "unchanged"'
rm -f "$SEL/webos-satellite-aarch64"
stand_in armv7 ok
sel_install aarch64
check "no aarch64 build in the package: armv7" '[ "$rc" -eq 0 ] && installed $A32'
sel_install aarch64 --binary "$HOST_BIN"
check "--binary installs that binary without selection" '[ "$rc" -eq 0 ] && cmp -s "$dir/bin/webos-satellite" "$HOST_BIN"'
check "status.sh names the installed build" 'sh "$dir/bin/status.sh" | grep -q "binary: webos-satellite "'

echo "== install.sh: the phone setup server =="
setup_stand_in() { # ARCH ok|bad|ARCH_REPORTED
    f=$SEL/webos-satellite-setup-$1
    if [ "$2" != bad ]; then
        reported=$1
        [ "$2" = ok ] || reported=$2
        printf '#!/bin/sh\n[ "$1" != --version ] || { echo "webos-satellite-setup 0.0.1 (%s)"; exit 0; }\nexit 0\n' \
            "$(triple_of_arch "$reported")" >"$f"
    else
        printf '#!/bin/sh\necho "Exec format error" >&2\nexit 126\n' >"$f"
    fi
    chmod 755 "$f"
}
setup_is() { # TRIPLE: the installed setup server is that build
    "$dir/bin/webos-satellite-setup" --version | grep -q "^webos-satellite-setup .*($1)"
}
stand_in armv7 ok
stand_in aarch64 ok
setup_stand_in armv7 ok
setup_stand_in aarch64 ok
sel_install aarch64
check "uname aarch64: the aarch64 daemon and the aarch64 setup server, mode 0700, no temporary file" '[ "$rc" -eq 0 ] && installed $A64 && setup_is $A64 && [ "$(stat -c %a "$dir/bin/webos-satellite-setup")" = 700 ] && [ ! -e "$dir/bin/.webos-satellite-setup.new" ]'
sel_install armv7l
check "uname armv7l: the armv7 setup server" '[ "$rc" -eq 0 ] && installed $A32 && setup_is $A32'
sel_install aarch64 --arch armv7
check "--arch armv7: the armv7 setup server follows the daemon" '[ "$rc" -eq 0 ] && setup_is $A32'
setup_stand_in aarch64 bad
sel_install aarch64
check "the aarch64 setup server does not run: the armv7 one, the daemon is aarch64" '[ "$rc" -eq 0 ] && installed $A64 && setup_is $A32 && echo "$out" | grep -q "aarch64 phone setup server does not run"'
setup_stand_in aarch64 armv7
sel_install aarch64
check "a setup file named aarch64 that reports another target: the armv7 one" '[ "$rc" -eq 0 ] && setup_is $A32'
setup_stand_in armv7 bad
setup_stand_in aarch64 bad
sel_install aarch64
check "no setup server runs: install still succeeds with a warning, no setup binary left" '[ "$rc" -eq 0 ] && installed $A64 && [ ! -e "$dir/bin/webos-satellite-setup" ] && [ ! -e "$dir/bin/.webos-satellite-setup.new" ] && echo "$out" | grep -q "phone setup is unavailable"'
rm -f "$SEL"/webos-satellite-setup-*
sel_install aarch64
check "no setup server in the package: noted, none installed" '[ "$rc" -eq 0 ] && [ ! -e "$dir/bin/webos-satellite-setup" ] && echo "$out" | grep -q "no phone setup server in this package"'
setup_stand_in armv7 ok
setup_stand_in aarch64 ok
sel_install aarch64 --binary "$HOST_BIN"
check "--binary: no setup server installed" '[ "$rc" -eq 0 ] && [ ! -e "$dir/bin/webos-satellite-setup" ]'
rm -f "$SEL"/webos-satellite-setup-*

echo "== install.sh: the remote grab helper (ADR 0016) =="
grab_stand_in() { # ARCH ok|bad
    f=$SEL/remote-proxy-$1
    if [ "$2" = ok ]; then
        printf '#!/bin/sh\n[ "$1" != --version ] || { echo "remote-proxy 0.0.1 (%s)"; exit 0; }\nexit 0\n' \
            "$(triple_of_arch "$1")" >"$f"
    else
        printf '#!/bin/sh\necho "Exec format error" >&2\nexit 126\n' >"$f"
    fi
    chmod 755 "$f"
}
grab_is() { "$dir/bin/remote-proxy" --version | grep -q "^remote-proxy .*($1)"; }
grab_stand_in armv7 ok
grab_stand_in aarch64 ok
sel_install aarch64
check "uname aarch64: the aarch64 grab helper, mode 0700, no temporary file" '[ "$rc" -eq 0 ] && grab_is $A64 && [ "$(stat -c %a "$dir/bin/remote-proxy")" = 700 ] && [ ! -e "$dir/bin/.remote-proxy.new" ]'
grab_stand_in aarch64 bad
sel_install aarch64
check "the aarch64 grab helper does not run: the armv7 one" '[ "$rc" -eq 0 ] && grab_is $A32'
grab_stand_in armv7 bad
sel_install aarch64
check "no grab helper runs: install succeeds with a warning, none left" '[ "$rc" -eq 0 ] && [ ! -e "$dir/bin/remote-proxy" ] && echo "$out" | grep -q "LG.s key override alone"'
rm -f "$SEL"/remote-proxy-*
sel_install aarch64
check "no grab helper in the package: noted, none installed" '[ "$rc" -eq 0 ] && [ ! -e "$dir/bin/remote-proxy" ] && echo "$out" | grep -q "no remote grab helper in this package"'

if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) failed; log:"
    cat "$LOG" 2>/dev/null || true
    exit 1
fi
