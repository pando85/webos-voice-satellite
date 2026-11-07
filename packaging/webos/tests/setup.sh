#!/bin/sh
# Host test of `ctl.sh setup start|status|answer|stop` (ADR 0009, design
# docs/design/phone-setup.md section 4) against a FAKE setup server, and of
# `ctl.sh set reply-volume` with its daemon trigger and the pairing window
# fields of `ctl.sh status --json`. The fake is a shell script that writes a
# synthetic state.json (a made-up path with a recognisable marker in place of
# the token), consumes answer.json, and is controlled by files in a directory.
# Also: the kill switch, the install lock and the session directory in /tmp
# are never written through a link or into a directory ctl.sh did not make.
# No real server, no network, no TV output; everything stays in a temporary
# directory.
#
# usage: sh packaging/webos/tests/setup.sh
# The checks are strings run with eval, so single quotes are intended.
# shellcheck disable=SC2016,SC2034,SC1091,SC2086,SC2126
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
PKG=$(cd "$HERE/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/webos-sat-setup.XXXXXX")
P=$T/prefix
F=$T/fake
D=$T/setup-dir
MARK=FAKETOKEN-0123456789
HOLD=
fails=0
cleanup() {
    for p in $(pgrep -f "$P/bin/webos-satellite-setup" 2>/dev/null || true); do kill "$p" 2>/dev/null || true; done
    [ -z "$HOLD" ] || kill "$HOLD" 2>/dev/null || true
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

mkdir -p "$P/bin" "$P/etc" "$P/run" "$P/log" "$F"
: >"$P/.webos-satellite-install"
printf '#!/bin/sh\nexit 0\n' >"$P/bin/webos-satellite"
chmod 755 "$P/bin/webos-satellite"
printf 'name = "setup-test"\nvolume = 40\n[pairing]\nwindow_s = 600\n' >"$P/etc/config.toml"

# --- the fake setup server -----------------------------------------------------
write_fake() {
    cat >"$P/bin/webos-satellite-setup" <<FAKE
#!/bin/sh
# Records its arguments and environment names, writes state.json like the
# real server (waiting, a made-up one-time path), consumes answer.json unless
# \$FAKE_DIR/noconsume exists.
# The server starts with an empty environment (env -i): it finds the test
# directory next to the prefix, not through a variable.
F=\$(dirname "\${WEBOS_SAT_PREFIX:?}")/fake
D=\${WEBOS_SAT_SETUP_DIR:?}
echo "\$*" >"\$F/argv"
env | sed -n 's/=.*//p' | sort >"\$F/envnames"
[ "\$(cat "\$F/mode" 2>/dev/null || echo ok)" != exit ] || { echo "fake: refusing to start" >&2; exit 1; }
printf '{"schema":1,"state":"waiting","reason":"","port":49731,"path":"/s/$MARK","expires_in_s":587,"peer":null,"peer_other_subnet":null,"pending":null,"last_result":null}\n' >"\$D/state.json.new" &&
    mv "\$D/state.json.new" "\$D/state.json"
while :; do
    if [ -e "\$D/stop" ] && [ ! -e "\$F/ignorestop" ]; then
        echo stop >"\$F/stopseen"
        exit 0
    fi
    if [ -f "\$D/answer.json" ] && [ ! -e "\$F/noconsume" ]; then
        cat "\$D/answer.json" >>"\$F/answers"
        rm -f "\$D/answer.json"
    fi
    sleep 0.1
done
FAKE
    chmod 755 "$P/bin/webos-satellite-setup"
}
write_fake

ctl() {
    WEBOS_SAT_PREFIX=$P WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LOCK=$T/lock \
        WEBOS_SAT_KILL_SWITCH=$T/kill WEBOS_SAT_SETUP_DIR=$D FAKE_DIR=$F \
        WEBOS_SAT_INITD=$T/initd sh "$PKG/ctl.sh" "$@"
}
setup_pid() { sed -n '1p' "$D/setup.pid" 2>/dev/null; }
servers() { # number of live fake servers of this test
    # One python3 process (itself excluded) instead of a tr and a grep per
    # pid: that shell loop cost about 20 s and 30 s of CPU per call on a
    # machine with 1000 processes. Command lines are read with NULs as spaces.
    python3 -c '
import os, sys
n = 0
for p in os.listdir("/proc"):
    if not p.isdigit() or int(p) == os.getpid():
        continue
    try:
        with open(f"/proc/{p}/cmdline", "rb") as f:
            n += sys.argv[1] in f.read().replace(b"\0", b" ").decode("utf-8", "replace")
    except OSError:
        pass
print(n)' "$P/bin/webos-satellite-setup"
}
# No fake server left. A killed server can take a moment to vanish from
# /proc on a loaded machine (stop_setup_server does not wait after KILL), so
# poll for up to 10 s instead of looking once.
gone() { wait_for 10 '[ "$(servers)" = 0 ]'; }
wait_for() { # SECONDS CONDITION
    i=0
    while [ "$i" -lt "$(($1 * 10))" ]; do
        eval "$2" && return 0
        sleep 0.1
        i=$((i + 1))
    done
    return 1
}
jq() { # JSON PYTHON_EXPR on `s`
    printf '%s' "$1" | python3 -c "import json,sys; s=json.load(sys.stdin); sys.exit(0 if ($2) else 1)"
}

echo "== setup status without a session =="
out=$(ctl setup status --json) && rc=0 || rc=$?
check "no server: exit 0, state none, every key of the schema" '[ "$rc" -eq 0 ] && jq "$out" "s[\"schema\"]==1 and s[\"state\"]==\"none\" and s[\"port\"] is None and s[\"pending\"] is None and s[\"last_result\"] is None and \"path\" not in s"'
check "no server: nothing was created" '[ ! -e "$D" ]'
ctl setup status >/dev/null 2>&1 && rc=0 || rc=$?
check "status without --json: usage, exit 2" '[ "$rc" -eq 2 ]'
out=$(ctl setup stop 2>/dev/null) && rc=0 || rc=$?
check "stop without a session: exit 0, state none" '[ "$rc" -eq 0 ] && jq "$out" "s[\"state\"]==\"none\""'

echo "== setup start =="
echo ok >"$F/mode"
out=$(ctl setup start 2>"$T/err") && rc=0 || rc=$?
check "start: exit 0, one JSON line, schema 1, waiting, port and path" '[ "$rc" -eq 0 ] && [ "$(printf "%s\n" "$out" | wc -l)" -eq 1 ] && jq "$out" "s[\"schema\"]==1 and s[\"state\"]==\"waiting\" and s[\"port\"]==49731 and s[\"path\"].startswith(\"/s/\") and s[\"expires_in_s\"]>0 and s[\"pending\"] is None"'
check "start: one server, pid file, directory mode 0700, heartbeat file" '[ "$(servers)" = 1 ] && [ -n "$(setup_pid)" ] && kill -0 "$(setup_pid)" && [ "$(stat -c %a "$D")" = 700 ] && [ -e "$D/heartbeat" ]'
check "start: the server got only --ctl <install>/bin/ctl.sh as arguments" '[ "$(cat "$F/argv")" = "--ctl $P/bin/ctl.sh" ]'
check "start: the server gets the session and install directories and no token variable (WEBOS_SAT_* names only)" 'grep -q "^WEBOS_SAT_SETUP_DIR$" "$F/envnames" && grep -q "^WEBOS_SAT_PREFIX$" "$F/envnames" && ! grep -i "^WEBOS_SAT.*\(token\|secret\|psk\)" "$F/envnames"'
check "start (C2): the server's environment is only PATH, WEBOS_SAT_PREFIX and WEBOS_SAT_SETUP_DIR (shell bookkeeping aside)" '[ -z "$(grep -vx "PATH\|WEBOS_SAT_PREFIX\|WEBOS_SAT_SETUP_DIR\|PWD\|OLDPWD\|SHLVL\|_" "$F/envnames")" ] && grep -qx PATH "$F/envnames"'
# The same, with hostile variables in the caller's environment.
ctl setup stop >/dev/null 2>&1
WEBOS_SAT_LUNA_SEND=/bin/evil WEBOS_SAT_INITD=/x WEBOS_SAT_SETTLE=0 WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LOCK_HELD=1 SOME_SECRET=hunter2 LD_PRELOAD=/nonexistent.so \
    ctl setup start >/dev/null 2>&1
check "start (C2): WEBOS_SAT_LUNA_SEND, _INITD, _SETTLE, _ALLOW_NONROOT, _LOCK_HELD, LD_PRELOAD and other variables of the caller do not reach the server" '[ -z "$(grep -vx "PATH\|WEBOS_SAT_PREFIX\|WEBOS_SAT_SETUP_DIR\|PWD\|OLDPWD\|SHLVL\|_" "$F/envnames")" ] && ! tr "\0" "\n" <"/proc/$(setup_pid)/environ" | grep -q "LUNA_SEND\|INITD\|SETTLE\|ALLOW_NONROOT\|SOME_SECRET\|LD_PRELOAD\|LOCK_HELD" && tr "\0" "\n" <"/proc/$(setup_pid)/environ" | grep -qx "PATH=/usr/sbin:/usr/bin:/sbin:/bin"'
pid=$(setup_pid)
pid=$(setup_pid)
check "the one-time path is in no argument, no environment, no log and not on the install directory" '! tr "\0" " " </proc/$pid/cmdline | grep -q "$MARK" && ! tr "\0" " " </proc/$pid/environ | grep -q "$MARK" && ! cat "$T/err" "$P/log/setup.log" 2>/dev/null | grep -q "$MARK" && ! grep -rl "$MARK" "$P/etc" "$P/run" "$P/log" >/dev/null 2>&1'
out2=$(ctl setup start 2>/dev/null) && rc=0 || rc=$?
check "start while waiting: the same session again, no second server" '[ "$rc" -eq 0 ] && [ "$(servers)" = 1 ] && [ "$(setup_pid)" = "$pid" ] && [ "$out" = "$out2" ]'

# The server's state.json has sorted keys, so a nested "state" (pending) comes
# before the top-level one; the word must not depend on the order.
cp "$D/state.json" "$T/state.keep"
for order in sorted unsorted; do
    if [ "$order" = sorted ]; then
        printf '{"last_result":null,"pending":{"action":"hi-lg","confirm":true,"id":"a1b2c3d4","peer":"192.168.1.23","state":"pending","timeout_in_s":47,"value":"off"},"peer":"192.168.1.23","port":49731,"reason":"","schema":1,"state":"waiting"}\n' >"$D/state.json"
    else
        printf '{"state":"waiting","pending":{"state":"running","id":"a1b2c3d4"},"last_result":{"state":"x"},"schema":1}\n' >"$D/state.json"
    fi
    out3=$(ctl setup start 2>/dev/null) && rc=0 || rc=$?
    check "start with a nested state ($order keys): the top-level state decides, the same session again" '[ "$rc" -eq 0 ] && [ "$out3" = "$(cat "$D/state.json")" ] && [ "$(servers)" = 1 ]'
done
printf '{"last_result":{"action":"mic","error":"","id":"0badcafe","peer":"192.168.1.23","result":"ok","value":"off","verdict":"auto"},"pending":{"action":"mic","confirm":false,"id":"0badcafe","peer":"192.168.1.23","state":"running","timeout_in_s":null,"value":"off"},"schema":1,"state":"active"}\n' >"$D/state.json"
out3=$(ctl setup status --json)
check "status passes the mic-off fields through (confirm false, verdict auto)" 'jq "$out3" "s[\"pending\"][\"confirm\"] is False and s[\"last_result\"][\"verdict\"]==\"auto\" and s[\"state\"]==\"active\""'
cp "$T/state.keep" "$D/state.json"

echo "== setup status (the heartbeat) =="
touch -d '1 minute ago' "$D/heartbeat"
before=$(stat -c %Y "$D/heartbeat")
out=$(ctl setup status --json) && rc=0 || rc=$?
after=$(stat -c %Y "$D/heartbeat")
check "status: prints the server's state.json and touches the heartbeat" '[ "$rc" -eq 0 ] && [ "$after" -gt "$before" ] && [ "$out" = "$(cat "$D/state.json")" ]'
printf '{"schema":1,"state":"active","reason":"","port":49731,"expires_in_s":500,"peer":"192.168.1.23","peer_other_subnet":false,"pending":{"id":"a1b2c3d4","action":"hi-lg","value":"off","peer":"192.168.1.23","timeout_in_s":47},"last_result":null}\n' >"$D/state.json"
out=$(ctl setup status --json)
check "status: an active state with a pending request comes through unchanged" 'jq "$out" "s[\"state\"]==\"active\" and s[\"pending\"][\"id\"]==\"a1b2c3d4\" and s[\"peer\"]==\"192.168.1.23\""'
ctl setup start >/dev/null 2>"$T/err" && rc=0 || rc=$?
check "start while active: refused, exit 1, the session is kept" '[ "$rc" -eq 1 ] && grep -q "already active" "$T/err" && [ "$(servers)" = 1 ] && [ "$(setup_pid)" = "$pid" ]'

echo "== setup status stays light (EXP-012: the app polls it once a second) =="
# Wrappers record every call of the commands a light poll must not use, and a
# spy stands in for luna-send and the daemon binary.
SHIM=$T/shim
mkdir -p "$SHIM"
for tool in luna-send cat touch tr grep sed awk python3 id stat mv rm head tail wc date readlink cp ls sort; do
    real=$(command -v "$tool" 2>/dev/null) || continue
    printf '#!/bin/sh\necho "%s" >>"%s/extcalls"\nexec "%s" "$@"\n' "$tool" "$F" "$real" >"$SHIM/$tool"
    chmod 755 "$SHIM/$tool"
done
cp "$P/bin/webos-satellite" "$T/daemon-stub"
printf '#!/bin/sh\necho run >>"%s/daemoncalls"\nexit 0\n' "$F" >"$P/bin/webos-satellite"
light() { # runs ctl.sh setup status --json with the wrappers first in PATH
    PATH=$SHIM:$PATH WEBOS_SAT_LUNA_SEND=$SHIM/luna-send ctl setup status --json
}
rm -f "$F/extcalls" "$F/daemoncalls"
out=$(PATH=$SHIM:$PATH WEBOS_SAT_PREFIX=$P WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LOCK=$T/lock WEBOS_SAT_SETUP_DIR=$T/nodir sh "$PKG/ctl.sh" setup status --json)
check "no session: the none state, no external command at all" 'jq "$out" "s[\"state\"]==\"none\"" && [ ! -e "$F/extcalls" ]'
rm -f "$F/extcalls"
out=$(light)
out=$(light)
check "a waiting session: the server's state.json printed, heartbeat touched, no external command" '[ "$out" = "$(cat "$D/state.json")" ] && [ ! -e "$F/extcalls" ] && [ ! -e "$F/daemoncalls" ]'
printf '{"last_result":null,"pending":{"action":"hi-lg","confirm":true,"id":"a1b2c3d4","peer":"192.168.1.23","state":"pending","timeout_in_s":47,"value":"off"},"schema":1,"state":"active"}\n' >"$D/state.json"
rm -f "$F/extcalls"
touch -d '1 minute ago' "$D/heartbeat"
hb0=$(stat -c %Y "$D/heartbeat")
out=$(light)
check "an active session with a pending request: printed unchanged, heartbeat mtime moves, no external command, no luna-send, no daemon run" '[ "$out" = "$(cat "$D/state.json")" ] && [ "$(stat -c %Y "$D/heartbeat")" -gt "$hb0" ] && [ ! -e "$F/extcalls" ] && [ ! -e "$F/daemoncalls" ]'
check "the status code path does not mention luna, the daemon binary or ctl.sh status" '! sed -n "/^cmd_setup_status()/,/^}/p;/^setup_print_state()/,/^}/p" "$PKG/ctl.sh" | grep -v "^ *#" | grep -q "luna\|BIN\|cmd_status\|tv-settings\|\$CFG"'
# A finished session: the server wrote ended and exited; the poll only prints it.
kill "$pid" 2>/dev/null || true
wait_for 2 '! kill -0 "$pid" 2>/dev/null' || true
printf '{"last_result":null,"pending":null,"reason":"done","schema":1,"state":"ended"}\n' >"$D/state.json"
rm -f "$F/extcalls"
out=$(light)
check "an ended session (server gone): its ended state is printed, no external command" '[ "$out" = "$(cat "$D/state.json")" ] && [ ! -e "$F/extcalls" ]'
cp "$T/daemon-stub" "$P/bin/webos-satellite"
# Back to a running waiting session for the next sections.
ctl setup stop >/dev/null 2>&1
ctl setup start >/dev/null 2>&1
pid=$(setup_pid)

echo "== setup answer =="
for bad in "A1B2C3D4" "a1b2c3d" "a1b2c3d4e" "a1b2c3dg" "a1b2c3d4;id" "" "../../x" "a1b2 c3d4"; do
    ctl setup answer "$bad" allow >/dev/null 2>&1 && rc=0 || rc=$?
    check "answer id '$bad': usage, exit 2, nothing written" '[ "$rc" -eq 2 ] && [ ! -e "$D/answer.json" ] && [ ! -e "$F/answers" ]'
done
for bad in ALLOW yes "allow;id" "" deny; do
    ctl setup answer a1b2c3d4 "$bad" >/dev/null 2>&1 && rc=0 || rc=$?
    check "answer verdict '$bad': usage, exit 2" '[ "$rc" -eq 2 ] && [ ! -e "$D/answer.json" ]'
done
ctl setup answer a1b2c3d4 >/dev/null 2>&1 && rc=0 || rc=$?
check "answer without a verdict: exit 2" '[ "$rc" -eq 2 ]'
ctl setup answer a1b2c3d4 allow extra >/dev/null 2>&1 && rc=0 || rc=$?
check "answer with extra arguments: exit 2" '[ "$rc" -eq 2 ]'
: >"$F/noconsume"
ctl setup answer a1b2c3d4 refuse >/dev/null 2>&1 && rc=0 || rc=$?
check "answer refuse: exit 0, answer.json exact, mode 0600, no temporary file" '[ "$rc" -eq 0 ] && [ "$(cat "$D/answer.json")" = "{\"id\":\"a1b2c3d4\",\"verdict\":\"refuse\"}" ] && [ "$(stat -c %a "$D/answer.json")" = 600 ] && [ -z "$(ls "$D" | grep "\.new\.")" ]'
rm -f "$F/noconsume"
check "the server reads and consumes it" 'wait_for 3 "[ ! -e \"$D/answer.json\" ]" && grep -qx "{\"id\":\"a1b2c3d4\",\"verdict\":\"refuse\"}" "$F/answers"'
ctl setup answer 0f0f0f0f allow >/dev/null 2>&1 && rc=0 || rc=$?
check "answer allow: exit 0 and the server gets it" '[ "$rc" -eq 0 ] && wait_for 3 "grep -qx \"{\\\"id\\\":\\\"0f0f0f0f\\\",\\\"verdict\\\":\\\"allow\\\"}\" \"$F/answers\""'
rm -f "$F/answers"

echo "== crashed and stop =="
kill -KILL "$pid"
wait_for 2 '! kill -0 "$pid" 2>/dev/null' || true
out=$(ctl setup status --json 2>/dev/null) && rc=0 || rc=$?
check "server gone without ended: status says ended, reason crashed, exit 0" '[ "$rc" -eq 0 ] && jq "$out" "s[\"state\"]==\"ended\" and s[\"reason\"]==\"crashed\""'
check "crashed: state.json no longer holds a path or token" '! grep -q "$MARK" "$D/state.json" && ! grep -q "\"path\"" "$D/state.json"'
out=$(ctl setup status --json 2>/dev/null)
check "crashed: the same answer on the next poll" 'jq "$out" "s[\"state\"]==\"ended\" and s[\"reason\"]==\"crashed\""'
out=$(ctl setup start 2>/dev/null) && rc=0 || rc=$?
pid2=$(setup_pid)
check "start after a crash: a new session replaces the old directory" '[ "$rc" -eq 0 ] && [ "$pid2" != "$pid" ] && jq "$out" "s[\"state\"]==\"waiting\"" && [ "$(servers)" = 1 ]'
rm -f "$F/stopseen"
out=$(ctl setup stop 2>/dev/null) && rc=0 || rc=$?
check "stop: the stop file asks the server, which ends; exit 0, state none, directory removed" '[ "$rc" -eq 0 ] && jq "$out" "s[\"state\"]==\"none\"" && gone && [ ! -e "$D" ] && [ -e "$F/stopseen" ]'
ctl setup start >/dev/null 2>&1
: >"$F/ignorestop"
rm -f "$F/stopseen"
t0=$(date +%s)
out=$(ctl setup stop 2>/dev/null) && rc=0 || rc=$?
t1=$(date +%s)
check "stop with a server that ignores the stop file: TERM after about 2 s, still ends" '[ "$rc" -eq 0 ] && gone && [ ! -e "$D" ] && [ $((t1 - t0)) -le 6 ]'
rm -f "$F/ignorestop"
ctl setup answer a1b2c3d4 allow >/dev/null 2>&1 && rc=0 || rc=$?
check "answer without a session: exit 1" '[ "$rc" -eq 1 ]'

echo "== start refusals =="
echo exit >"$F/mode"
ctl setup start >/dev/null 2>"$T/err" && rc=0 || rc=$?
check "a server that exits at once: start fails, nothing left behind" '[ "$rc" -eq 1 ] && grep -q "did not start" "$T/err" && [ "$(servers)" = 0 ] && [ ! -e "$D" ]'
echo ok >"$F/mode"
mv "$P/bin/webos-satellite-setup" "$T/hold-bin"
ctl setup start >/dev/null 2>"$T/err" && rc=0 || rc=$?
check "no server binary: refused, exit 1" '[ "$rc" -eq 1 ] && grep -q "setup server is missing" "$T/err" && [ ! -e "$D" ]'
mv "$T/hold-bin" "$P/bin/webos-satellite-setup"
mv "$P/.webos-satellite-install" "$T/marker"
ctl setup start >/dev/null 2>"$T/err" && rc=0 || rc=$?
check "not installed: refused, exit 1" '[ "$rc" -eq 1 ] && grep -q "not installed" "$T/err" && [ "$(servers)" = 0 ]'
mv "$T/marker" "$P/.webos-satellite-install"
if [ "$(id -u)" != 0 ]; then
    out=$(WEBOS_SAT_PREFIX=$P WEBOS_SAT_LOCK=$T/lock WEBOS_SAT_SETUP_DIR=$D FAKE_DIR=$F sh "$PKG/ctl.sh" setup start 2>&1) && rc=0 || rc=$?
    check "not root: start refused" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "needs root" && [ "$(servers)" = 0 ]'
fi
ctl setup bogus >/dev/null 2>&1 && rc=0 || rc=$?
check "unknown setup subcommand: exit 2" '[ "$rc" -eq 2 ]'
ctl setup start now >/dev/null 2>&1 && rc=0 || rc=$?
check "start with an argument: exit 2" '[ "$rc" -eq 2 ]'
ctl setup stop now >/dev/null 2>&1 && rc=0 || rc=$?
check "stop with an argument: exit 2" '[ "$rc" -eq 2 ]'
ctl setup >/dev/null 2>&1 && rc=0 || rc=$?
check "setup alone: exit 2" '[ "$rc" -eq 2 ]'

echo "== never started by the boot hook, autostart or ctl.sh start =="
check "the hook template, autostart.sh, start.sh and supervise.sh do not mention the setup server" '! grep -il "setup" "$PKG/init.d/webos-satellite" "$PKG/autostart.sh" "$PKG/start.sh" "$PKG/supervise.sh"'
check "ctl.sh start and restart never call the setup start" '! sed -n "/^cmd_start()/,/^}/p" "$PKG/ctl.sh" | grep -q "setup" && ! sed -n "/^restart)/,/;;/p" "$PKG/ctl.sh" | grep -q "setup start\|cmd_setup"'
check "only the setup dispatch calls cmd_setup_start" '[ "$(grep -c "cmd_setup_start" "$PKG/ctl.sh")" -eq 2 ]'

echo "== the commands the server runs never end a session (review C7) =="
python3 "$HERE/no_self_stop.py" "$PKG/ctl.sh" "$PKG/lib.sh" >"$T/c7.out" 2>&1 && rc=0 || rc=$?
check "set, mic, pair, status and tv-settings (with every function they call) never reach stop_daemon, stop_setup_server, stop, start, install, uninstall or purge; the checker sees them in cmd_stop and cmd_setup_stop" '[ "$rc" -eq 0 ] && grep -q "^cmd_set: " "$T/c7.out" && grep -q "^cmd_tv_settings: " "$T/c7.out"'
[ "$rc" -eq 0 ] || cat "$T/c7.out"

echo "== stop, uninstall and purge end a session =="
ctl setup start >/dev/null 2>&1
check "setup: a session is up" '[ "$(servers)" = 1 ]'
ctl stop >/dev/null 2>&1
check "ctl.sh stop ends the session and removes the directory" 'gone && [ ! -e "$D" ]'
ctl setup start >/dev/null 2>&1
WEBOS_SAT_SETUP_DIR=$D sh "$PKG/stop.sh" --prefix "$P" >/dev/null 2>&1 || true
check "stop.sh ends the session too" 'gone && [ ! -e "$D" ]'
ctl setup start >/dev/null 2>&1
ctl uninstall >/dev/null 2>&1 || true
check "uninstall ends the session" 'gone && [ ! -e "$D" ]'
# uninstall removed bin/ and run/: rebuild what the next checks need.
mkdir -p "$P/bin" "$P/run" "$P/etc"
printf '#!/bin/sh\nexit 0\n' >"$P/bin/webos-satellite"
chmod 755 "$P/bin/webos-satellite"
write_fake
ctl setup start >/dev/null 2>&1
check "setup: a session is up again" '[ "$(servers)" = 1 ]'
ctl purge >/dev/null 2>&1 || true
check "purge ends the session and removes the install" 'gone && [ ! -e "$D" ] && [ ! -e "$P" ]'

echo "== set reply-volume =="
mkdir -p "$P/bin" "$P/etc" "$P/run" "$P/log"
: >"$P/.webos-satellite-install"
printf '#!/bin/sh\nexit 0\n' >"$P/bin/webos-satellite"
chmod 755 "$P/bin/webos-satellite"
printf 'name = "setup-test"\nvolume = 40\n[pairing]\nwindow_s = 600\n' >"$P/etc/config.toml"
out=$(ctl set reply-volume 30 2>"$T/err") && rc=0 || rc=$?
check "30: applied, exit 0, one JSON line, before from the config default, daemon not running" '[ "$rc" -eq 0 ] && [ "$out" = "{\"schema\":1,\"setting\":\"reply-volume\",\"requested\":\"30\",\"before\":\"40\",\"after\":\"30\",\"result\":\"applied\",\"daemon\":\"not running\",\"error\":\"\"}" ]'
check "30: etc/reply_volume holds 30, mode 0600, no temporary file, trigger created" '[ "$(cat "$P/etc/reply_volume")" = 30 ] && [ "$(stat -c %a "$P/etc/reply_volume")" = 600 ] && [ -z "$(ls "$P/etc" | grep "\.new\.")" ] && [ -e "$P/run/reply-volume-reload" ]'
check "30: a human line on stderr and one history line" 'grep -q "reply-volume is now 30 %" "$T/err" && [ "$(wc -l <"$P/etc/tv-settings.log")" -eq 1 ] && grep -q " reply-volume result from=40 to=30 read=30 outcome=applied source=ctl\$" "$P/etc/tv-settings.log"'
rm -f "$P/run/reply-volume-reload"
out=$(ctl set reply-volume 30 2>/dev/null) && rc=0 || rc=$?
check "the same value again: already, exit 0, no trigger, no history line" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"before\":\"30\",\"after\":\"30\",\"result\":\"already\",\"daemon\":\"none\"" && [ ! -e "$P/run/reply-volume-reload" ] && [ "$(wc -l <"$P/etc/tv-settings.log")" -eq 1 ]'
rm -f "$P/etc/reply_volume"
out=$(ctl set reply-volume 40 2>/dev/null) && rc=0 || rc=$?
check "the config default with nothing stored: already, nothing written" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"already\"" && [ ! -e "$P/etc/reply_volume" ]'
for v in 0 5 99 100; do
    out=$(ctl set reply-volume "$v" 2>/dev/null) && rc=0 || rc=$?
    check "$v is accepted and stored" '[ "$rc" -eq 0 ] && [ "$(cat "$P/etc/reply_volume")" = "$v" ] && echo "$out" | grep -q "\"requested\":\"$v\""'
done
rm -f "$P/run/reply-volume-reload"
echo 55 >"$P/etc/reply_volume"
for bad in 101 1000 007 00 -1 1.5 "" abc "50%" 1e2 " 5" "5 " "+5" 0x10 "5;id" "a b" "$(printf '1\n2')"; do
    ctl set reply-volume "$bad" >/dev/null 2>&1 && rc=0 || rc=$?
    check "reply-volume '$bad': usage, exit 2, nothing changed" '[ "$rc" -eq 2 ] && [ "$(cat "$P/etc/reply_volume")" = 55 ] && [ ! -e "$P/run/reply-volume-reload" ]'
done
ctl set reply-volume >/dev/null 2>&1 && rc=0 || rc=$?
check "reply-volume without a value: exit 2" '[ "$rc" -eq 2 ]'
ctl set reply-volume 10 20 >/dev/null 2>&1 && rc=0 || rc=$?
check "reply-volume with two values: exit 2" '[ "$rc" -eq 2 ] && [ "$(cat "$P/etc/reply_volume")" = 55 ]'
echo 'volume_file = "vol"' >>"$P/etc/config.toml"
ctl set reply-volume 20 >/dev/null 2>&1
check "volume_file from the config is honoured (relative to etc/)" '[ "$(cat "$P/etc/vol")" = 20 ] && [ "$(cat "$P/etc/reply_volume")" = 55 ]'
out=$(ctl status --json)
check "status --json reads the same file" 'jq "$out" "s[\"reply_volume\"]==\"20\""'
sed -i '$d' "$P/etc/config.toml"
rm -f "$P/run/reply-volume-reload"
if [ "$(id -u)" != 0 ]; then
    WEBOS_SAT_PREFIX=$P WEBOS_SAT_LOCK=$T/lock sh "$PKG/ctl.sh" set reply-volume 10 >/dev/null 2>&1 && rc=0 || rc=$?
    check "not root: refused" '[ "$rc" -ne 0 ] && [ "$(cat "$P/etc/reply_volume")" = 55 ]'
fi
chmod 555 "$P/etc"
if [ ! -w "$P/etc" ] && [ "$(id -u)" != 0 ]; then
    out=$(ctl set reply-volume 61 2>/dev/null) && rc=0 || rc=$?
    check "etc/ not writable: failed, exit 1, JSON with the error" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"failed\"" && echo "$out" | grep -q "cannot write the file" && [ "$(cat "$P/etc/reply_volume")" = 55 ]'
fi
chmod 755 "$P/etc"

# A running daemon: a copy of sleep stands in for it (is_ours looks at /proc/PID/exe).
cp "$(command -v sleep)" "$P/bin/webos-satellite"
"$P/bin/webos-satellite" 120 &
HOLD=$!
echo "$HOLD" >"$P/run/webos-satellite.pid"
rm -f "$P/run/reply-volume-reload"
( sleep 1; rm -f "$P/run/reply-volume-reload" ) &
out=$(ctl set reply-volume 62 2>/dev/null) && rc=0 || rc=$?
check "the daemon consumes the trigger: daemon applied" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"daemon\":\"applied\"" && [ "$(cat "$P/etc/reply_volume")" = 62 ]'
out=$(ctl set reply-volume 63 2>/dev/null) && rc=0 || rc=$?
check "the daemon does not consume it within 3 s: pending, the file holds the value for the next start" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"daemon\":\"pending\"" && [ -e "$P/run/reply-volume-reload" ] && [ "$(cat "$P/etc/reply_volume")" = 63 ]'

echo "== the pairing window in status --json =="
voice() { # PAIRING_UNTIL_JSON_VALUE [HA [HA_SINCE]]
    printf '{"schema":1,"mic":"listening","remote_off":"normal","remote_off_active":true,"power_state":"Active","pairing_until":%s,"ha":"%s","ha_since":%s}\n' "$1" "${2:-connected}" "${3:-$(date +%s)}" >"$P/run/voice.json"
}
rm -f "$P/run/voice.json"
out=$(ctl status --json)
check "status --json: closed without voice.json, window_s from the config" 'jq "$out" "s[\"pairing\"]=={\"open\":False,\"ends_in_s\":None} and s[\"pairing_window_s\"]==\"600\""'
now=$(date +%s)
voice "$((now + 412))"
out=$(ctl status --json)
check "status --json: an open window with the seconds left" 'jq "$out" "s[\"pairing\"][\"open\"] is True and 408 <= s[\"pairing\"][\"ends_in_s\"] <= 412"'
voice "$((now - 5))"
out=$(ctl status --json)
check "status --json: an end in the past is closed" 'jq "$out" "s[\"pairing\"]=={\"open\":False,\"ends_in_s\":None}"'
voice null
out=$(ctl status --json)
check "status --json: null is closed" 'jq "$out" "s[\"pairing\"][\"open\"] is False"'
voice "$((now + 412))"
touch -d '3 minutes ago' "$P/run/voice.json"
out=$(ctl status --json)
check "status --json: a stale voice.json counts as closed" 'jq "$out" "s[\"pairing\"][\"open\"] is False"'
voice "$((now + 412))"
kill "$HOLD" 2>/dev/null || true
wait "$HOLD" 2>/dev/null || true
HOLD=
rm -f "$P/run/webos-satellite.pid"
out=$(ctl status --json)
check "status --json: a stopped daemon has no open window" 'jq "$out" "s[\"pairing\"][\"open\"] is False"'

echo "== the Home Assistant connection in status --json =="
cp "$(command -v sleep)" "$P/bin/webos-satellite"
"$P/bin/webos-satellite" 120 &
HOLD=$!
echo "$HOLD" >"$P/run/webos-satellite.pid"
for st in connected disconnected unpaired; do
    # The time per state, and the slack grows by what ctl.sh status took (a
    # loaded machine took longer than the 3 s of slack).
    now=$(date +%s)
    voice null "$st" "$((now - 90))"
    out=$(ctl status --json)
    took=$(($(date +%s) - now))
    check "ha $st, began 90 s ago" 'jq "$out" "s[\"ha\"][\"state\"]==\"$st\" and 90 <= s[\"ha\"][\"since_s\"] <= 93 + $took"'
done
voice null bogus "$now"
out=$(ctl status --json)
check "an unknown word is unknown, not passed through" 'jq "$out" "s[\"ha\"]=={\"state\":\"unknown\",\"since_s\":None}"'
printf '{"schema":1,"mic":"listening","remote_off":"normal","remote_off_active":true,"power_state":"Active","pairing_until":null}\n' >"$P/run/voice.json"
out=$(ctl status --json)
check "an older daemon's file without ha: unknown" 'jq "$out" "s[\"ha\"][\"state\"]==\"unknown\""'
voice null connected "$((now + 1000))"
out=$(ctl status --json)
check "a start time in the future: the state without a time" 'jq "$out" "s[\"ha\"]=={\"state\":\"connected\",\"since_s\":None}"'
voice null connected "$now"
touch -d '3 minutes ago' "$P/run/voice.json"
out=$(ctl status --json)
check "a stale file: unknown" 'jq "$out" "s[\"ha\"][\"state\"]==\"unknown\""'
voice null connected "$now"
kill "$HOLD" 2>/dev/null || true
wait "$HOLD" 2>/dev/null || true
HOLD=
rm -f "$P/run/webos-satellite.pid"
out=$(ctl status --json)
check "daemon stopped: unknown" 'jq "$out" "s[\"ha\"][\"state\"]==\"unknown\""'
check "no address, version or name in the ha object" 'jq "$out" "set(s[\"ha\"])=={\"state\",\"since_s\"}"'

echo "== links and other users' directories in /tmp (docs/security/install-permissions.md) =="
# The kill switch: never created through a link planted at its name.
echo keep >"$T/victim"
ln -s "$T/victim" "$T/kill"
ctl mic off >/dev/null 2>&1
check "mic off: a link planted at the kill switch's name is replaced by a file; its target is untouched" '[ -f "$T/kill" ] && [ ! -L "$T/kill" ] && [ "$(cat "$T/victim")" = keep ]'
ctl mic on >/dev/null 2>&1
# The install lock: a link at its name (to a directory with a live pid) is
# not a lock of ours: removed at once, its target untouched.
mkdir "$T/elsewhere"
echo $$ >"$T/elsewhere/pid"
ln -s "$T/elsewhere" "$T/lock"
out=$(ctl set reply-volume 31 2>&1) && rc=0 || rc=$?
check "lock: a link at the lock's name is removed at once and the command runs; the target is untouched" '[ "$rc" -eq 0 ] && [ ! -e "$T/lock" ] && [ "$(cat "$T/elsewhere/pid")" = $$ ] && echo "$out" | grep -q "not a lock of ours"'
# Another user's session directory in /tmp (WEBOS_SAT_TEST_TRUST_UID makes
# the test's own uid "another user"), with a live process that looks like
# the server and a link at the heartbeat's name: status writes nothing into
# it, stop removes it without signalling the process.
ctl setup stop >/dev/null 2>&1
rm -rf "$D"
mkdir -m 700 "$D"
printf '{"schema":1,"state":"waiting"}\n' >"$D/state.json"
sh -c 'sleep 30; :' webos-satellite-setup &
LOOK=$!
echo "$LOOK" >"$D/setup.pid"
echo keep >"$T/victim2"
ln -s "$T/victim2" "$D/heartbeat"
out=$(WEBOS_SAT_TEST_TRUST_UID=99999 ctl setup status --json) && rc=0 || rc=$?
check "setup status: another user's session directory is neither trusted nor written: state none, the link's target untouched" '[ "$rc" -eq 0 ] && jq "$out" "s[\"state\"]==\"none\"" && [ "$(cat "$T/victim2")" = keep ] && [ "$(cat "$D/state.json")" = "{\"schema\":1,\"state\":\"waiting\"}" ]'
WEBOS_SAT_TEST_TRUST_UID=99999 ctl setup stop >/dev/null 2>&1
check "setup stop: the foreign directory is removed, the link's target untouched, the look-alike process not signalled" '[ ! -e "$D" ] && [ "$(cat "$T/victim2")" = keep ] && kill -0 "$LOOK"'
kill "$LOOK" 2>/dev/null || true

echo
if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) failed"
    exit 1
fi
