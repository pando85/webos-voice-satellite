#!/bin/sh
# Host test of `ctl.sh tv-settings` and `ctl.sh set ...` (ADR 0008, design
# docs/design/voice-and-power-screen.md section 6) against a fake luna-send.
# The fake keeps its state in files and serves synthetic JSON written by hand
# (no TV output). It can answer nothing (a timeout), refuse (returnValue
# false), ignore a write (mismatch), or print only on a terminal. Nothing
# outside a temporary directory. `config aec|music` (config keys and a daemon
# restart) runs against the host build of webos-satellite: its own
# --check-config judges every changed config, and a real daemon (file mic,
# null sink, plaintext on 127.0.0.1) is restarted.
#
# usage: sh packaging/webos/tests/tv_settings.sh [BINARY]   (default: builds it)
# The checks are strings run with eval, so single quotes are intended.
# shellcheck disable=SC2016,SC2034,SC1091,SC2086,SC2126
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
PKG=$(cd "$HERE/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/webos-sat-tvset.XXXXXX")
P=$T/prefix
F=$T/fake
Q=$T/cfg
fails=0
cleanup() {
    [ ! -f "$Q/.webos-satellite-install" ] || cq stop >/dev/null 2>&1 || true
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

mkdir -p "$P/bin" "$P/etc" "$P/run" "$F"
: >"$P/.webos-satellite-install"
printf '#!/bin/sh\nexit 0\n' >"$P/bin/webos-satellite"
chmod 755 "$P/bin/webos-satellite"
echo 'name = "tvset-test"' >"$P/etc/config.toml"

# --- the fake luna-send ---------------------------------------------------
cat >"$T/luna-send" <<'FAKE'
#!/bin/sh
# args: -n 1 -w 3000 URI PAYLOAD
F=${FAKE_DIR:?}
uri=$5
payload=$6
echo "$uri $payload" >>"$F/calls"
[ ! -e "$F/needs_tty" ] || [ -t 1 ] || exit 0
mode=$(cat "$F/mode" 2>/dev/null || echo ok)
fail() { printf '{\n    "returnValue": false,\n    "errorCode": -1,\n    "errorText": "%s"\n}\n' "$1"; }
flag() { cat "$F/$1" 2>/dev/null || echo false; }
[ "$mode" != gettimeout ] || case $uri in */get*) exit 0 ;; esac
case $uri in
*/getSystemSettings)
    printf '{\n    "subscribed": false,\n    "category": "general",\n    "method": "getSystemSettings",\n    "returnValue": true,\n    "settings": {\n        "lifeOnScreenMode": "%s"\n    }\n}\n' "$(cat "$F/ar")"
    ;;
*/setSystemSettings)
    case $mode in
    timeout) exit 0 ;;
    rvfalse) fail "not exist"; exit 0 ;;
    esac
    [ "$mode" = ignore ] || echo "$payload" | sed -n 's/.*"lifeOnScreenMode":"\([^"]*\)".*/\1/p' >"$F/ar"
    printf '{\n    "returnValue": true,\n    "method": "setSystemSettings"\n}\n'
    ;;
*/getActivation)
    [ "$payload" = '{}' ] || { fail "bad params"; exit 0; }
    t=$(flag thinq)
    a=$(flag amazon)
    d=$(flag dangbei)
    printf '{\n    "dangbei": {\n        "settingStatus": %s,\n        "activationState": %s,\n        "systemStatus": true\n    },\n    "returnValue": true,\n    "systemStatus": true,\n    "settingStatus": %s,\n    "thinQ": {\n        "settingStatus": %s,\n        "supportLanguage": true,\n        "activationState": %s\n    },\n    "activationState": %s,\n    "amazon": {\n        "settingStatus": %s,\n        "activationState": false\n    }\n}\n' "$d" "$d" "$t" "$t" "$t" "$t" "$a"
    ;;
*/setActivation)
    case $mode in
    timeout) exit 0 ;;
    rvfalse) fail "not exist"; exit 0 ;;
    esac
    [ "$mode" = ignore ] || echo "$payload" | sed -n 's/.*"settingStatus":\(true\|false\).*/\1/p' >"$F/thinq"
    printf '{\n    "serviceName": "thinQ",\n    "returnValue": true,\n    "settingStatus": true\n}\n'
    ;;
*/setConfigs)
    # configd's in-memory value of com.webos.keyaction.voice in $F/kav
    # (absent: LG's "launch"); kamode: ok | refuse | stuck (set ignored).
    km=$(cat "$F/kamode" 2>/dev/null || echo ok)
    case $payload in *'"volatile":true'*) ;; *) echo nonvolatile >>"$F/kaviolation" ;; esac
    [ "$km" != refuse ] || { fail "denied"; exit 0; }
    if [ "$km" = ok ]; then
        case $payload in
        *'"action":"ignore"'*) echo ignore >"$F/kav" ;;
        *null*) rm -f "$F/kav" ;;
        esac
    fi
    printf '{\n    "returnValue": true\n}\n'
    ;;
*/getConfigs)
    case $payload in *voiceframework* | *categoryNames*) echo category >>"$F/kaviolation" ;; esac
    printf '{\n    "configs": {\n        "com.webos.keyaction.voice": {\n            "action": "%s"\n        }\n    },\n    "returnValue": true\n}\n' "$(cat "$F/kav" 2>/dev/null || echo launch)"
    ;;
*)
    echo "unexpected luna call" >&2
    fail "unexpected"
    ;;
esac
FAKE
chmod 755 "$T/luna-send"

ctl() {
    WEBOS_SAT_PREFIX=$P WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LOCK=$T/lock \
        WEBOS_SAT_KILL_SWITCH=$T/kill WEBOS_SAT_LUNA_SEND=${LUNA_FAKE:-$T/luna-send} \
        WEBOS_SAT_SETTLE=0.05 FAKE_DIR=$F sh "$PKG/ctl.sh" "$@"
}
reset() { # AR THINQ [AMAZON]
    echo "$1" >"$F/ar"
    echo "$2" >"$F/thinq"
    echo "${3:-false}" >"$F/amazon"
    echo false >"$F/dangbei"
    echo ok >"$F/mode"
    rm -f "$F/calls" "$F/needs_tty" "$P/etc/tv-settings.log"
}
field() { # JSON KEY: first string/bool value of "KEY":
    printf '%s' "$1" | sed -n "s/.*\"$2\":\"\\{0,1\\}\\([^\",}]*\\)\"\\{0,1\\}.*/\\1/p"
}
nwrites() { grep 'setSystemSettings\|setActivation' "$F/calls" 2>/dev/null | wc -l | tr -d ' '; }

echo "== tv-settings --json =="
reset alwaysReady false
out=$(ctl tv-settings --json)
check "one JSON line, schema 1, luna true" '[ "$(printf "%s\n" "$out" | wc -l)" -eq 1 ] && echo "$out" | grep -q "^{\"schema\":1,\"luna\":true,"'
check "valid JSON (python)" 'printf "%s" "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); assert d[\"schema\"]==1"'
check "always ready alwaysReady is on, raw kept" 'echo "$out" | grep -q "\"always_ready\":{\"state\":\"on\",\"raw\":\"alwaysReady\",\"error\":\"\"}"'
check "hi lg off, providers listed" 'echo "$out" | grep -q "\"hi_lg\":{\"state\":\"off\",\"thinq\":false,\"amazon\":false,\"dangbei\":false,\"error\":\"\"}"'
check "remote_off normal, daemon not running: active null" 'echo "$out" | grep -q "\"remote_off\":{\"state\":\"normal\",\"active\":null}" && echo "$out" | grep -q "\"daemon_running\":false"'
check "no voice.json: mic unknown, power_state null, last_change null" 'echo "$out" | grep -q "\"mic\":{\"state\":\"unknown\",\"kill_switch\":false}" && echo "$out" | grep -q "\"power_state\":null,\"last_change\":null}"'
check "reads only the two getters" '[ "$(wc -l <"$F/calls")" -eq 2 ] && ! grep -q "setSystemSettings\|setActivation\|apiadapter\|getSystemInfo" "$F/calls"'
check "getter calls: exact payloads" 'grep -q "getSystemSettings {\"category\":\"general\",\"keys\":\[\"lifeOnScreenMode\"\]}" "$F/calls" && grep -q "getActivation {}\$" "$F/calls"'

for raw in none lifeOnScreen allEnabled; do
    reset "$raw" false
    out=$(ctl tv-settings --json)
    check "always ready $raw: unknown with the raw word" 'echo "$out" | grep -q "\"always_ready\":{\"state\":\"unknown\",\"raw\":\"$raw\""'
done
reset off true
out=$(ctl tv-settings --json)
check "always ready off, hi lg thinQ on" 'echo "$out" | grep -q "\"always_ready\":{\"state\":\"off\"" && echo "$out" | grep -q "\"hi_lg\":{\"state\":\"on\",\"thinq\":true"'
reset off false true
out=$(ctl tv-settings --json)
check "amazon on alone: hi lg state on, thinq false, amazon true" 'echo "$out" | grep -q "\"hi_lg\":{\"state\":\"on\",\"thinq\":false,\"amazon\":true"'

reset off false
echo gettimeout >"$F/mode"
out=$(ctl tv-settings --json); rc=$?
check "timeout: both fields error, exit 0" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"always_ready\":{\"state\":\"error\",\"raw\":\"\",\"error\":\"timeout\"" && echo "$out" | grep -q "\"hi_lg\":{\"state\":\"error\".*\"error\":\"timeout\"}"'
out=$(LUNA_FAKE=$T/missing ctl tv-settings --json)
check "no luna-send: luna false, fields error 'no luna-send'" 'echo "$out" | grep -q "\"luna\":false" && echo "$out" | grep -q "\"error\":\"no luna-send\""'
if [ "$(id -u)" != 0 ]; then
    out=$(WEBOS_SAT_PREFIX=$P WEBOS_SAT_LUNA_SEND=$T/luna-send sh "$PKG/ctl.sh" tv-settings --json)
    check "not root: luna false, error 'not root'" 'echo "$out" | grep -q "\"luna\":false" && echo "$out" | grep -q "\"error\":\"not root\""'
fi

# voice.json: fresh, stale, kill switch
reset alwaysReady false
printf '{"schema":1,"mic":"busy","remote_off":"keep_listening","remote_off_active":true,"power_state":"Screen Off"}\n' >"$P/run/voice.json"
out=$(ctl tv-settings --json)
check "voice.json: mic busy, power state" 'echo "$out" | grep -q "\"mic\":{\"state\":\"busy\",\"kill_switch\":false}" && echo "$out" | grep -q "\"power_state\":\"Screen Off\""'
check "voice.json: remote_off active stays null while the daemon is not running" 'echo "$out" | grep -q "\"remote_off\":{\"state\":\"normal\",\"active\":null}"'
touch -d '3 minutes ago' "$P/run/voice.json"
out=$(ctl tv-settings --json)
check "voice.json older than 60 s: unknown, no power state" 'echo "$out" | grep -q "\"mic\":{\"state\":\"unknown\"" && echo "$out" | grep -q "\"power_state\":null"'
rm -f "$P/run/voice.json"
: >"$T/kill"
out=$(ctl tv-settings --json)
check "kill switch file: kill_switch true, mic muted when the daemon says nothing" 'echo "$out" | grep -q "\"mic\":{\"state\":\"muted\",\"kill_switch\":true}"'
rm -f "$T/kill"
printf 'keep_listening\n' >"$P/etc/remote_off"
out=$(ctl tv-settings --json)
check "remote_off file: state keep_listening" 'echo "$out" | grep -q "\"remote_off\":{\"state\":\"keep_listening\""'
rm -f "$P/etc/remote_off"

# compact output and a terminal-only luna-send
reset off false
touch "$F/needs_tty"
out=$(ctl tv-settings --json)
if command -v python3 >/dev/null 2>&1; then
    check "luna-send that prints only on a terminal: read through a pty" 'echo "$out" | grep -q "\"always_ready\":{\"state\":\"off\"" && echo "$out" | grep -q "\"hi_lg\":{\"state\":\"off\""'
fi
# c3-a's python3 3.8 has no pty module (P4, 0.3.0): the fallback must work without it
mkdir -p "$T/nopty"
printf 'import sys\nsys.modules["pty"] = None\n' >"$T/nopty/sitecustomize.py"
if command -v python3 >/dev/null 2>&1; then
    out=$(PYTHONPATH=$T/nopty ctl tv-settings --json)
    check "pty fallback with a python3 whose pty module is missing (as on c3-a)" 'echo "$out" | grep -q "\"always_ready\":{\"state\":\"off\"" && echo "$out" | grep -q "\"hi_lg\":{\"state\":\"off\""'
fi
check "PTY_PY does not use the pty module" '! sed -n "/^PTY_PY=/,/^sys.stdout/p" "$PKG/ctl.sh" | grep -q "pty\\.\|import.*pty"'
rm -f "$F/needs_tty"

echo "== set always-ready / hi-lg =="
reset off false
out=$(ctl set always-ready on 2>"$T/err") && rc=0 || rc=$?
check "always-ready on: applied, exit 0, JSON line" '[ "$rc" -eq 0 ] && [ "$out" = "{\"schema\":1,\"setting\":\"always-ready\",\"requested\":\"on\",\"before\":\"off\",\"after\":\"on\",\"result\":\"applied\",\"error\":\"\"}" ]'
check "always-ready on: wrote alwaysReady once, with the exact payload" '[ "$(nwrites)" -eq 1 ] && grep -q "setSystemSettings {\"category\":\"general\",\"settings\":{\"lifeOnScreenMode\":\"alwaysReady\"}}" "$F/calls" && [ "$(cat "$F/ar")" = alwaysReady ]'
check "always-ready on: a human line on stderr" 'grep -q "always-ready is now on (was off)" "$T/err"'
check "history: request and result lines, mode 0600" '[ "$(wc -l <"$P/etc/tv-settings.log")" -eq 2 ] && grep -q " always-ready request from=off to=on source=ctl\$" "$P/etc/tv-settings.log" && grep -q " always-ready result from=off to=on read=on outcome=applied\$" "$P/etc/tv-settings.log" && [ "$(stat -c %a "$P/etc/tv-settings.log")" = 600 ]'
out=$(ctl tv-settings --json)
check "last_change shows the applied change" 'echo "$out" | grep -q "\"last_change\":{\"setting\":\"always-ready\",\"from\":\"off\",\"to\":\"on\",\"at\":\"20[0-9-]*T[0-9:]*Z\"}"'

rm -f "$F/calls"
out=$(ctl set always-ready on 2>/dev/null) && rc=0 || rc=$?
check "already on: result already, exit 0, no write, no new history" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"already\"" && [ "$(nwrites)" -eq 0 ] && [ "$(wc -l <"$P/etc/tv-settings.log")" -eq 2 ]'
out=$(ctl set always-ready off 2>/dev/null) && rc=0 || rc=$?
check "always-ready off: writes off, applied" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"before\":\"on\",\"after\":\"off\",\"result\":\"applied\"" && grep -q "\"lifeOnScreenMode\":\"off\"" "$F/calls"'

reset allEnabled false
out=$(ctl set always-ready off 2>/dev/null) && rc=0 || rc=$?
check "unknown current value (allEnabled): refused, no write, exit 1" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"refused\",\"error\":\"unknown value\"" && [ "$(nwrites)" -eq 0 ] && [ ! -e "$P/etc/tv-settings.log" ]'
reset none false
out=$(ctl set always-ready on 2>/dev/null) && rc=0 || rc=$?
check "unknown current value (none): refused" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"error\":\"unknown value\"" && [ "$(nwrites)" -eq 0 ]'

reset off false
echo ignore >"$F/mode"
out=$(ctl set always-ready on 2>/dev/null) && rc=0 || rc=$?
check "write ignored by the TV: mismatch, exit 1, one write, no retry" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"after\":\"off\",\"result\":\"mismatch\"" && [ "$(nwrites)" -eq 1 ] && grep -q "outcome=mismatch" "$P/etc/tv-settings.log"'
reset off false
echo rvfalse >"$F/mode"
out=$(ctl set always-ready on 2>/dev/null) && rc=0 || rc=$?
check "returnValue false: failed with the error text only, exit 1, one write" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"after\":\"off\",\"result\":\"failed\",\"error\":\"not exist\"" && [ "$(nwrites)" -eq 1 ] && grep -q "outcome=failed" "$P/etc/tv-settings.log"'
reset off false
echo gettimeout >"$F/mode"
out=$(ctl set always-ready on 2>/dev/null) && rc=0 || rc=$?
check "timeout: read of the getter fails first, refused, no write" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"refused\",\"error\":\"timeout\"" && [ "$(nwrites)" -eq 0 ]'

reset off false
echo timeout >"$F/mode"
out=$(ctl set always-ready on 2>/dev/null) && rc=0 || rc=$?
check "setter timeout: failed 'timeout', one write, read-back shows the TV value" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"after\":\"off\",\"result\":\"failed\",\"error\":\"timeout\"" && [ "$(nwrites)" -eq 1 ]'

reset off false
chmod 555 "$P/etc"
if [ ! -w "$P/etc" ] && [ "$(id -u)" != 0 ]; then
    out=$(ctl set always-ready on 2>/dev/null) && rc=0 || rc=$?
    check "history not writable: refused, no write" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"refused\"" && echo "$out" | grep -q "cannot write" && [ "$(nwrites)" -eq 0 ] && [ "$(cat "$F/ar")" = off ]'
fi
chmod 755 "$P/etc"

reset off false
out=$(ctl set hi-lg on 2>/dev/null) && rc=0 || rc=$?
check "hi-lg on: applied, thinQ switched, exact payload, one write" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"setting\":\"hi-lg\",\"requested\":\"on\",\"before\":\"off\",\"after\":\"on\",\"result\":\"applied\"" && [ "$(nwrites)" -eq 1 ] && grep -q "setActivation {\"settingStatus\":true}\$" "$F/calls"'
out=$(ctl set hi-lg off 2>/dev/null) && rc=0 || rc=$?
check "hi-lg off: applied, payload false" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"after\":\"off\",\"result\":\"applied\"" && grep -q "setActivation {\"settingStatus\":false}\$" "$F/calls"'
reset off false true
out=$(ctl set hi-lg off 2>/dev/null) && rc=0 || rc=$?
check "hi-lg off while only amazon is on: thinQ is already off, no write" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"already\"" && [ "$(nwrites)" -eq 0 ]'
reset off false
echo ignore >"$F/mode"
out=$(ctl set hi-lg on 2>/dev/null) && rc=0 || rc=$?
check "hi-lg ignored by the TV: mismatch" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"mismatch\""'
reset off false
echo rvfalse >"$F/mode"
out=$(ctl set hi-lg on 2>/dev/null) && rc=0 || rc=$?
check "hi-lg returnValue false: failed" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"failed\",\"error\":\"not exist\""'

out=$(LUNA_FAKE=$T/missing ctl set hi-lg on 2>/dev/null) && rc=0 || rc=$?
check "no luna-send: refused" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"refused\",\"error\":\"no luna-send\""'
if [ "$(id -u)" != 0 ]; then
    out=$(WEBOS_SAT_PREFIX=$P WEBOS_SAT_LUNA_SEND=$T/luna-send sh "$PKG/ctl.sh" set hi-lg on 2>/dev/null) && rc=0 || rc=$?
    check "not root: refused 'not root'" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"error\":\"not root\""'
fi

reset off false
for a in "always-ready" "hi-lg"; do
    for bad in toggle ON "on;id" "" allEnabled alwaysReady; do
        ctl set "$a" "$bad" >/dev/null 2>&1 && rc=0 || rc=$?
        check "set $a '$bad': usage, exit 2" '[ "$rc" -eq 2 ]'
    done
done
ctl set always-ready >/dev/null 2>&1 && rc=0 || rc=$?
check "set with one argument: exit 2" '[ "$rc" -eq 2 ]'
ctl set volume 5 >/dev/null 2>&1 && rc=0 || rc=$?
check "set of another setting: exit 2" '[ "$rc" -eq 2 ]'
ctl set always-ready on extra >/dev/null 2>&1 && rc=0 || rc=$?
check "set with extra arguments: exit 2" '[ "$rc" -eq 2 ]'
ctl tv-settings --text >/dev/null 2>&1 && rc=0 || rc=$?
check "tv-settings with an unknown flag: exit 2" '[ "$rc" -eq 2 ]'
check "no usage case wrote anything" '[ "$(nwrites)" -eq 0 ]'
check "no run ever asked for the system info" '! grep -q "apiadapter\|getSystemInfo" "$PKG/ctl.sh" && { [ ! -e "$F/calls" ] || ! grep -q "apiadapter\|getSystemInfo" "$F/calls"; }'

echo "== history cap =="
reset off false
i=0
: >"$P/etc/tv-settings.log"
while [ "$i" -lt 199 ]; do
    echo "2026-01-01T00:00:00Z hi-lg request from=off to=on source=ctl" >>"$P/etc/tv-settings.log"
    i=$((i + 1))
done
ctl set always-ready on >/dev/null 2>&1 || true
check "history capped at 200 lines, newest kept" '[ "$(wc -l <"$P/etc/tv-settings.log")" -eq 200 ] && tail -n 1 "$P/etc/tv-settings.log" | grep -q "always-ready result .* outcome=applied" && [ "$(stat -c %a "$P/etc/tv-settings.log")" = 600 ]'

echo "== set remote-off =="
reset off false
rm -f "$P/etc/remote_off" "$P/run/remote-off-reload"
out=$(ctl set remote-off keep 2>/dev/null) && rc=0 || rc=$?
check "remote-off keep: file holds keep_listening, trigger created, daemon not running" '[ "$rc" -eq 0 ] && [ "$(cat "$P/etc/remote_off")" = keep_listening ] && [ -e "$P/run/remote-off-reload" ] && echo "$out" | grep -q "\"setting\":\"remote-off\",\"requested\":\"keep_listening\",\"before\":\"normal\",\"after\":\"keep_listening\",\"result\":\"applied\",\"error\":\"\",\"daemon\":\"not running\""'
check "remote-off: no luna call, no temporary file left" '[ ! -e "$F/calls" ] && [ -z "$(ls "$P/etc" | grep "remote_off.new")" ]'
out=$(ctl set remote-off normal 2>/dev/null) && rc=0 || rc=$?
check "remote-off normal: file holds normal, before keep_listening" '[ "$rc" -eq 0 ] && [ "$(cat "$P/etc/remote_off")" = normal ] && echo "$out" | grep -q "\"before\":\"keep_listening\",\"after\":\"normal\""'
echo 'remote_off_file = "ro"' >>"$P/etc/config.toml"
ctl set remote-off keep >/dev/null 2>&1
check "remote_off_file from the config is honoured (relative to etc/)" '[ "$(cat "$P/etc/ro")" = keep_listening ] && [ "$(cat "$P/etc/remote_off")" = normal ]'
sed -i '$d' "$P/etc/config.toml"
ctl set remote-off darken >/dev/null 2>&1 && rc=0 || rc=$?
check "remote-off with another word: exit 2" '[ "$rc" -eq 2 ]'

# A running daemon: a copy of sleep stands in for it (is_ours looks at /proc/PID/exe).
cp "$(command -v sleep)" "$P/bin/webos-satellite"
"$P/bin/webos-satellite" 60 &
fake_pid=$!
echo "$fake_pid" >"$P/run/webos-satellite.pid"
rm -f "$P/run/remote-off-reload"
( sleep 1; rm -f "$P/run/remote-off-reload" ) &
out=$(ctl set remote-off keep 2>/dev/null) && rc=0 || rc=$?
check "daemon consumes the trigger: daemon applied" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"daemon\":\"applied\""'
out=$(ctl set remote-off normal 2>/dev/null) && rc=0 || rc=$?
check "daemon does not consume the trigger within 3 s: pending" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"daemon\":\"pending\"" && [ -e "$P/run/remote-off-reload" ]'
out=$(ctl tv-settings --json)
check "daemon running: daemon_running true" 'echo "$out" | grep -q "\"daemon_running\":true"'
printf '{"schema":1,"mic":"listening","remote_off":"normal","remote_off_active":false,"power_state":"Active"}\n' >"$P/run/voice.json"
out=$(ctl tv-settings --json)
check "daemon running: remote_off active from voice.json, mic listening" 'echo "$out" | grep -q "\"remote_off\":{\"state\":\"normal\",\"active\":false}" && echo "$out" | grep -q "\"mic\":{\"state\":\"listening\""'
reset off false
ctl set always-ready on >/dev/null 2>&1
check "an applied LG change asks the running daemon to re-read the sensors" '[ -e "$P/run/lg-settings-refresh" ]'
kill "$fake_pid" 2>/dev/null || true
wait "$fake_pid" 2>/dev/null || true
rm -f "$P/run/webos-satellite.pid" "$P/run/voice.json"
printf '#!/bin/sh\nexit 0\n' >"$P/bin/webos-satellite"

echo "== config aec / music (config keys, daemon restart) =="
ROOT=$(cd "$PKG/../.." && pwd)
HOST_BIN=${1:-}
if [ -z "$HOST_BIN" ]; then
    (cd "$ROOT" && cargo build --quiet -p webos-satellite)
    HOST_BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug/webos-satellite
fi
[ -x "$HOST_BIN" ] || { echo "binary not found: $HOST_BIN" >&2; exit 2; }
mkdir -p "$Q/bin" "$Q/etc" "$Q/run" "$Q/log"
: >"$Q/.webos-satellite-install"
cp "$PKG"/*.sh "$Q/bin/"
cp "$HOST_BIN" "$Q/bin/webos-satellite"
head -c 96000 /dev/zero >"$T/mic.raw"
qport=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
cq() {
    WEBOS_SAT_PREFIX=$Q WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LOCK=$T/lock WEBOS_SAT_KILL_SWITCH=$T/kill \
        WEBOS_SAT_LUNA_SEND=$T/luna-send WEBOS_SAT_SETUP_DIR=$T/setup FAKE_DIR=$F sh "$PKG/ctl.sh" "$@"
}
# The cooldown between two config changes lives in ctl.sh (run/config-change.
# stamp) and has no bypass: back-to-back toggles of the tests remove the stamp.
cqc() { rm -f "$Q/run/config-change.stamp"; cq "$@"; }
QC=$Q/etc/config.toml
# A config with tuning in [aec] (no enabled key: on by default), an [aec.aec3]
# override, comments, a [music] table with a comment on its enabled line, and
# pairing off (plaintext): enabled keys of three tables.
cat >"$T/base.toml" <<EOF
# webos-satellite test config
mode = "wake"

[device]
name = "cfg-test"
bind = "127.0.0.1"
port = $qport
allow_plaintext = true

[mic]
source = "file"
path = "$T/mic.raw"
loop = true

[sink]
type = "null"

[aec]
# tuned by hand: these lines must survive every toggle
ns = false
delay_ms = 20

[aec.aec3]
"linear_mix" = 0.3   # tuned

[safety]
kill_switch = "$T/kill"
mute_file = "$T/muted"

[pairing]
enabled = false

[music]
# music comment
enabled = false  # keep this comment
duck_db = -12.0
EOF
cp "$T/base.toml" "$QC"
chmod 600 "$QC"
qsum() { "$Q/bin/webos-satellite" --config "$QC" --check-config 2>/dev/null; }
leftovers() { # temporary config files left in etc/
    n=0
    for f in "$Q"/etc/*.new.* "$Q"/etc/*.prev.*; do [ ! -e "$f" ] || n=$((n + 1)); done
    echo "$n"
}

out=$(cq status --json)
check "status --json: aec_enabled true (no key), music_enabled false" 'echo "$out" | grep -q "\"aec_enabled\":true,\"music_enabled\":false,"'
check "status: the pairing key word reads [pairing] enabled, not another table's enabled" 'echo "$out" | grep -q "\"key\":\"none\""'
check "status text: echo cancellation and music" 'cq status 2>/dev/null | grep -q "echo cancellation: on, music: off"'
check "status: the mic button is off by default" 'echo "$out" | grep -q "\"mic_button\":\"off\"," && cq status 2>/dev/null | grep -q "remote mic button: off"'

out=$(cqc config aec off 2>"$T/err") && rc=0 || rc=$?
check "aec off (daemon stopped): applied, exit 0, JSON line" '[ "$rc" -eq 0 ] && [ "$out" = "{\"schema\":1,\"setting\":\"aec\",\"requested\":\"off\",\"before\":\"on\",\"after\":\"off\",\"result\":\"applied\",\"daemon\":\"not running\",\"error\":\"\",\"reason\":\"\",\"retry_in_s\":null}" ]'
check "aec off: a human line on stderr" 'grep -q "aec is now off (was on; daemon: not running)" "$T/err"'
check "aec off: enabled = false added under [aec]; every other line unchanged" 'sed -n "/^\[aec\]\$/{n;p;}" "$QC" | grep -qx "enabled = false" && sed "/^\[aec\]\$/{n;d;}" "$QC" | cmp -s - "$T/base.toml"'
check "aec off: the daemon reads it (check-config aec=[off]), tuning kept, mode 0600, no temporary file" 'qsum | grep -q " aec=\[off\] " && grep -qx "ns = false" "$QC" && grep -qx "delay_ms = 20" "$QC" && [ "$(stat -c %a "$QC")" = 600 ] && [ "$(leftovers)" = 0 ]'
check "aec off: status reports it, history line" 'cq status --json | grep -q "\"aec_enabled\":false," && grep -q " aec result from=on to=off read=off outcome=applied source=ctl\$" "$Q/etc/tv-settings.log"'
cp "$QC" "$T/off.toml"
out=$(cqc config aec off 2>/dev/null) && rc=0 || rc=$?
check "aec off again: already, daemon none, exit 0, config untouched" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"already\",\"daemon\":\"none\"" && cmp -s "$QC" "$T/off.toml"'
out=$(cqc config aec on 2>/dev/null) && rc=0 || rc=$?
check "aec on: applied, the same line now says true, everything else unchanged" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"before\":\"off\",\"after\":\"on\",\"result\":\"applied\"" && sed -n "/^\[aec\]\$/{n;p;}" "$QC" | grep -qx "enabled = true" && sed "/^\[aec\]\$/{n;d;}" "$QC" | cmp -s - "$T/base.toml"'
check "aec on: check-config shows it enabled (file mic: off for lack of a reference), tuning kept" 'qsum | grep -q " aec=\[off (file mic" && grep -qx "ns = false" "$QC" && grep -qx "\"linear_mix\" = 0.3   # tuned" "$QC" && cq status --json | grep -q "\"aec_enabled\":true,"'

out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
check "music on: applied, value changed in place, its comment kept, nothing else changed" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"setting\":\"music\",\"requested\":\"on\",\"before\":\"off\",\"after\":\"on\",\"result\":\"applied\"" && grep -qx "enabled = true  # keep this comment" "$QC" && [ "$(diff "$T/base.toml" "$QC" | grep -c "^[<>]")" -eq 3 ]'
check "music on: the daemon reads it with the tuned duck_db, status says so" 'qsum | grep -q " music=\[on .*duck_db=-12" && cq status --json | grep -q "\"music_enabled\":true,"'
out=$(cqc config music off 2>/dev/null) && rc=0 || rc=$?
check "music off: applied; the [music] table is byte for byte the original again" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"after\":\"off\",\"result\":\"applied\"" && sed "/^\[aec\]\$/{n;d;}" "$QC" | cmp -s - "$T/base.toml" && qsum | grep -q " music=\[off\]"'

# The mic button: [remote] mic_button, a quoted word, not an `enabled` key.
cp "$QC" "$T/before-mic.toml"
out=$(cqc config mic-button on 2>/dev/null) && rc=0 || rc=$?
check "mic-button on: applied, [remote] appended with mic_button = \"ptt\", nothing else changed" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"setting\":\"mic-button\",\"requested\":\"on\",\"before\":\"off\",\"after\":\"on\",\"result\":\"applied\"" && tail -n 2 "$QC" | tr "\n" "|" | grep -q "^\[remote\]|mic_button = \"ptt\"|\$" && [ "$(diff "$T/before-mic.toml" "$QC" | grep -c "^[<>]")" -eq 3 ]'
check "mic-button on: the daemon reads it, status says ptt" 'qsum | grep -q " remote=\[mic_button=ptt " && cq status --json | grep -q "\"mic_button\":\"ptt\"," && grep -q " mic-button result from=off to=on read=on outcome=applied source=ctl\$" "$Q/etc/tv-settings.log"'
out=$(cqc config mic-button on 2>/dev/null) && rc=0 || rc=$?
check "mic-button on again: already" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"already\""'
out=$(cqc config mic-button off 2>/dev/null) && rc=0 || rc=$?
check "mic-button off: the quoted value is replaced in place" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"after\":\"off\",\"result\":\"applied\"" && grep -qx "mic_button = \"off\"" "$QC" && ! grep -q ptt "$QC" && qsum | grep -q " remote=\[mic_button=off "'
# A single-quoted TOML string is the same value.
sed "s/^mic_button = \"off\"\$/mic_button = 'ptt'  # single quotes/" "$QC" >"$T/sq.toml" && cp "$T/sq.toml" "$QC"
check "mic_button = 'ptt' (single quotes): the daemon and status both read ptt" 'qsum | grep -q " remote=\[mic_button=ptt " && cq status --json | grep -q "\"mic_button\":\"ptt\"," && cq status 2>/dev/null | grep -q "remote mic button: ptt"'
out=$(cqc config mic-button off 2>/dev/null) && rc=0 || rc=$?
check "mic-button off from a single-quoted value: replaced in place, comment kept" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"before\":\"on\",\"after\":\"off\",\"result\":\"applied\"" && grep -qx "mic_button = \"off\"  # single quotes" "$QC" && qsum | grep -q " remote=\[mic_button=off "'
for bad in ptt yes ""; do
    cq config mic-button "$bad" >/dev/null 2>&1 && rc=0 || rc=$?
    check "config mic-button '$bad': usage, exit 2" '[ "$rc" -eq 2 ]'
done
cq config remote-ptt on >/dev/null 2>&1 && rc=0 || rc=$?
check "config remote-ptt: not a setting, exit 2" '[ "$rc" -eq 2 ]'

# Tables that are not there: appended at the end (also after a last line without a newline).
grep -v "^\[music\]\$\|^# music comment\$\|^enabled = false  # keep this comment\$\|^duck_db = -12.0\$" "$T/base.toml" |
    sed -e '/^\[aec\]$/,/^delay_ms/d' | sed -e '$!b' -e '/^$/d' >"$T/bare.toml"
printf '%s' "$(cat "$T/bare.toml")" >"$QC"
out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
check "music on without a [music] table (no final newline): table appended, valid" '[ "$rc" -eq 0 ] && tail -n 2 "$QC" | tr "\n" "|" | grep -q "^\[music\]|enabled = true|\$" && qsum | grep -q " music=\[on "'
out=$(cqc config aec off 2>/dev/null) && rc=0 || rc=$?
check "aec off without an [aec] table but with [aec.aec3]: [aec] appended, valid" '[ "$rc" -eq 0 ] && tail -n 2 "$QC" | tr "\n" "|" | grep -q "^\[aec\]|enabled = false|\$" && qsum | grep -q " aec=\[off\] "'
out=$(cqc config music off 2>/dev/null) && rc=0 || rc=$?
check "music off after that: enabled = false in place (the default value, written)" '[ "$rc" -eq 0 ] && sed -n "/^\[music\]\$/{n;p;}" "$QC" | grep -qx "enabled = false" && qsum | grep -q " music=\[off\]"'

# A changed config that does not pass the daemon's check is never installed.
{ echo 'aec.enabled = false'; cat "$T/bare.toml"; } >"$QC"
cp "$QC" "$T/dotted.toml"
check "dotted-key config passes as it is (aec off)" 'qsum | grep -q " aec=\[off\] "'
: >"$Q/etc/tv-settings.log"
out=$(cqc config aec on 2>/dev/null) && rc=0 || rc=$?
check "bad changed config: failed, exit 1, the check's reason, config byte for byte unchanged, no temporary file" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"before\":\"off\",\"after\":\"off\",\"result\":\"failed\",\"daemon\":\"none\",\"error\":\"the changed config did not pass the check, nothing changed: " && cmp -s "$QC" "$T/dotted.toml" && [ "$(leftovers)" = 0 ]'
check "bad changed config: history says failed" 'grep -q " aec result from=off to=on read=off outcome=failed source=ctl\$" "$Q/etc/tv-settings.log"'
printf 'mode = "nonsense"\n' >"$QC"
out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
check "a config that is invalid already: refused, untouched" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"refused\",\"daemon\":\"none\",\"error\":\"the config does not pass the check now: " && [ "$(cat "$QC")" = "mode = \"nonsense\"" ]'

for bad in yes ON "on;id" "" true enabled; do
    cq config aec "$bad" >/dev/null 2>&1 && rc=0 || rc=$?
    check "config aec '$bad': usage, exit 2" '[ "$rc" -eq 2 ]'
    cq config music "$bad" >/dev/null 2>&1 && rc=0 || rc=$?
    check "config music '$bad': usage, exit 2" '[ "$rc" -eq 2 ]'
done
for w in "aec off" "music on"; do
    # shellcheck disable=SC2086 # two words on purpose
    cq set $w >/dev/null 2>&1 && rc=0 || rc=$?
    check "set $w: usage, exit 2 (it is config $w: nothing under set may restart, review C7)" '[ "$rc" -eq 2 ] && [ "$(cat "$QC")" = "mode = \"nonsense\"" ]'
done
cqc config aec on extra >/dev/null 2>&1 && rc=0 || rc=$?
check "config aec with an extra argument: exit 2, config untouched" '[ "$rc" -eq 2 ] && [ "$(cat "$QC")" = "mode = \"nonsense\"" ]'
if [ "$(id -u)" != 0 ]; then
    cp "$T/base.toml" "$QC"
    out=$(WEBOS_SAT_PREFIX=$Q WEBOS_SAT_LOCK=$T/lock sh "$PKG/ctl.sh" config aec off 2>&1) && rc=0 || rc=$?
    check "not root: exit 1, needs root, config untouched" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "needs root" && cmp -s "$QC" "$T/base.toml"'
fi
# The daemon runs this for HA's switch with WEBOS_SAT_SOURCE=ha (ADR 0012):
# the history line says so; any other value is ctl. (Also as root.)
cp "$T/base.toml" "$QC"
out=$( (WEBOS_SAT_SOURCE=ha; export WEBOS_SAT_SOURCE; cqc config music on) 2>/dev/null) && rc=0 || rc=$?
check "WEBOS_SAT_SOURCE=ha: applied, the history line says source=ha" '[ "$rc" -eq 0 ] && tail -n 1 "$Q/etc/tv-settings.log" | grep -q " music result from=off to=on read=on outcome=applied source=ha\$"'
out=$( (WEBOS_SAT_SOURCE='"'"'ha; rm -rf /'"'"'; export WEBOS_SAT_SOURCE; cqc config music off) 2>/dev/null) && rc=0 || rc=$?
check "any other WEBOS_SAT_SOURCE value is logged as ctl" '[ "$rc" -eq 0 ] && tail -n 1 "$Q/etc/tv-settings.log" | grep -q " music result from=on to=off read=off outcome=applied source=ctl\$"'

# --- the guards of `config` (cooldown, busy) live in ctl.sh, for every entry point ---
cp "$T/base.toml" "$QC"
stamp=$Q/run/config-change.stamp
nlog() { wc -l <"$Q/etc/tv-settings.log" | tr -d ' '; }
setstamp() { echo "$1" >"$stamp"; }
rm -f "$stamp"
out=$(cq config aec off 2>"$T/err-nostamp") && rc=0 || rc=$?
check "no stamp yet: stderr has only the summary line, no shell error about the stamp (the TV app sees stderr too)" '[ "$rc" -eq 0 ] && [ "$(wc -l <"$T/err-nostamp" | tr -d " ")" -eq 1 ] && grep -q "^aec is now off" "$T/err-nostamp" && ! grep -qi "stamp\|no such file" "$T/err-nostamp"'
check "cooldown: a change writes run/config-change.stamp (unix seconds, mode 0600, no temporary file)" '[ "$rc" -eq 0 ] && [ "$(stat -c %a "$stamp")" = 600 ] && s=$(cat "$stamp") && now=$(date +%s) && [ "$s" -le "$now" ] && [ $((now - s)) -le 5 ] && [ "$(ls "$Q/run" | grep -c "config-change.stamp.new")" -eq 0 ]'
cp "$QC" "$T/cd.toml"
n0=$(nlog)
out=$(cq config aec on 2>"$T/err") && rc=0 || rc=$?
check "an immediate second change: refused, reason cooldown, exit 1, retry_in_s 1 to 30, config byte for byte unchanged" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"refused\",\"daemon\":\"none\",\"error\":\"the last change was [0-9]* s ago; try again in [0-9]* s\",\"reason\":\"cooldown\",\"retry_in_s\":[0-9]*}\$" && r=$(field "$out" retry_in_s) && [ "$r" -ge 1 ] && [ "$r" -le 30 ] && cmp -s "$QC" "$T/cd.toml" && [ "$(leftovers)" = 0 ]'
check "a refusal writes no history line and leaves the stamp alone" '[ "$(nlog)" = "$n0" ] && [ "$(cat "$stamp")" = "$s" ]'
out=$(cq config aec off 2>/dev/null) && rc=0 || rc=$?
check "the state already in force inside the cooldown: already, exit 0" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"already\"" && cmp -s "$QC" "$T/cd.toml"'
now=$(date +%s)
setstamp $((now - 28))
out=$(cq config aec on 2>/dev/null) && rc=0 || rc=$?
check "a stamp 28 s old still blocks" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"reason\":\"cooldown\"" && cmp -s "$QC" "$T/cd.toml"'
setstamp $((now + 10))
out=$(cq config aec on 2>/dev/null) && rc=0 || rc=$?
check "a stamp 10 s ahead (a clock stepped back a little) still blocks" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"reason\":\"cooldown\"" && cmp -s "$QC" "$T/cd.toml"'
setstamp $((now - 31))
out=$(cq config aec on 2>/dev/null) && rc=0 || rc=$?
check "a stamp 31 s old: applied" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"applied\"" && sed -n "/^\[aec\]\$/{n;p;}" "$QC" | grep -qx "enabled = true"'
setstamp $(($(date +%s) + 100))
out=$(cq config aec off 2>/dev/null) && rc=0 || rc=$?
check "a stamp 100 s in the future blocks nothing" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"applied\"" && sed -n "/^\[aec\]\$/{n;p;}" "$QC" | grep -qx "enabled = false"'
for junk in garbage "" "12 34" "-5" "007" "99999999999999999999"; do
    printf '%s\n' "$junk" >"$stamp"
    want=on
    sed -n "/^\[aec\]\$/{n;p;}" "$QC" | grep -qx "enabled = false" || want=off
    out=$(cq config aec $want 2>/dev/null) && rc=0 || rc=$?
    check "a garbled stamp ('$junk') blocks nothing" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"applied\""'
done
# The cooldown file cannot be written (run is a file): failed, nothing changed.
mv "$Q/run" "$Q/run.keep"
: >"$Q/run"
cp "$QC" "$T/nostamp.toml"
want=on
sed -n "/^\[aec\]\$/{n;p;}" "$QC" | grep -qx "enabled = false" || want=off
out=$(cq config aec $want 2>/dev/null) && rc=0 || rc=$?
check "no cooldown file possible: failed, cannot write the cooldown file, config untouched, no temporary file" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"failed\",\"daemon\":\"none\",\"error\":\"cannot write the cooldown file\"" && cmp -s "$QC" "$T/nostamp.toml" && [ "$(leftovers)" = 0 ]'
rm -f "$Q/run"
mv "$Q/run.keep" "$Q/run"
rm -f "$stamp"
# The final rename over the config fails (a stand-in mv): failed, nothing
# changed, and the stamp written just before it is removed again, so the
# user can retry at once.
mkdir -p "$T/fakemv"
cat >"$T/fakemv/mv" <<'FAKEMV'
#!/bin/sh
for a; do last=$a; done
case $last in */etc/config.toml) exit 1 ;; esac
exec /bin/mv "$@"
FAKEMV
chmod 755 "$T/fakemv/mv"
cp "$QC" "$T/nomv.toml"
want=on
sed -n "/^\[aec\]\$/{n;p;}" "$QC" | grep -qx "enabled = false" || want=off
out=$(PATH="$T/fakemv:$PATH" cq config aec $want 2>/dev/null) && rc=0 || rc=$?
check "the rename over the config fails: failed, cannot replace the config, config untouched, no temporary file, no stamp left" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"result\":\"failed\",\"daemon\":\"none\",\"error\":\"cannot replace the config\"" && cmp -s "$QC" "$T/nomv.toml" && [ "$(leftovers)" = 0 ] && [ ! -e "$stamp" ]'
out=$(cq config aec $want 2>/dev/null) && rc=0 || rc=$?
check "an immediate retry after that failure is not blocked by a cooldown" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"applied\""'
rm -f "$stamp"

# A running daemon is restarted with the new config (on -> off -> on).
cp "$T/base.toml" "$QC"
rm -f "$Q/log/webos-satellite.log"
cq start >/dev/null 2>&1
qpid() { read_pid "$Q/run/webos-satellite.pid"; }
. "$PKG/lib.sh"
set_paths "$Q"
p0=$(qpid)
check "daemon started (real build, supervised)" 'is_ours "$p0" && supervisor_alive'
out=$(cqc config aec off 2>/dev/null) && rc=0 || rc=$?
p1=$(qpid)
check "aec off while running: applied, daemon restarted, a new pid, the old one gone" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"after\":\"off\",\"result\":\"applied\",\"daemon\":\"restarted\"" && is_ours "$p1" && [ "$p1" != "$p0" ] && ! kill -0 "$p0" 2>/dev/null'
check "the restarted daemon logged aec=[off]" 'grep " config: " "$LOG" | tail -n 1 | grep -q " aec=\[off\] "'
out=$(cqc config aec on 2>/dev/null) && rc=0 || rc=$?
p2=$(qpid)
check "aec on while running: restarted again, its config line has the tuned aec back" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"daemon\":\"restarted\"" && is_ours "$p2" && [ "$p2" != "$p1" ] && grep " config: " "$LOG" | tail -n 1 | grep -q " aec=\[off (file mic"'
out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
p3=$(qpid)
check "music on while running: restarted, the daemon runs with music on" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"daemon\":\"restarted\"" && is_ours "$p3" && grep " config: " "$LOG" | tail -n 1 | grep -q " music=\[on "'
out=$(cqc config music off 2>/dev/null) && rc=0 || rc=$?
p4=$(qpid)
check "music off while running: restarted, music off, one daemon and one supervisor" '[ "$rc" -eq 0 ] && is_ours "$p4" && grep " config: " "$LOG" | tail -n 1 | grep -q " music=\[off\]" && [ "$(pgrep -f "$Q/bin/supervise.sh" | wc -l)" -eq 1 ]'
check "the daemon and its supervisor started under ctl.sh's lock do not inherit WEBOS_SAT_LOCK_HELD (a ctl.sh they start must take the lock)" '[ -r "/proc/$p4/environ" ] && ! tr "\0" "\n" <"/proc/$p4/environ" | grep -q "^WEBOS_SAT_LOCK_HELD=" && sp=$(read_pid "$SUPPID") && [ -r "/proc/$sp/environ" ] && ! tr "\0" "\n" <"/proc/$sp/environ" | grep -q "^WEBOS_SAT_LOCK_HELD="'
check "after on, off, on: the config is the original plus enabled = true under [aec]" 'sed "/^\[aec\]\$/{n;d;}" "$QC" | cmp -s - "$T/base.toml" && [ "$(leftovers)" = 0 ]'
cq stop >/dev/null 2>&1

# The daemon does not come back with the new config: the previous config is
# put back and started again. A stand-in runs --check-config with the host
# build and fails as a daemon; a sleeping script named supervise.sh stands in
# for the running supervisor.
printf '#!/bin/sh\ncase "$3" in --check-config) exec "%s" "$@" ;; esac\n[ "$1" != --version ] || exec "%s" --version\nexit 1\n' "$HOST_BIN" "$HOST_BIN" >"$Q/bin/webos-satellite"
chmod 755 "$Q/bin/webos-satellite"
mkdir -p "$T/fakesup"
printf 'sleep 30\n' >"$T/fakesup/supervise.sh"
sh "$T/fakesup/supervise.sh" &
echo $! >"$Q/run/supervise.pid"
cp "$QC" "$T/before-rollback.toml"
out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
check "no daemon after the restart: failed, exit 1, the previous setting is back" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"requested\":\"on\",\"before\":\"off\",\"after\":\"off\",\"result\":\"failed\",\"daemon\":\"failed\",\"error\":\"the satellite did not start with the new setting; the previous setting is back\"" && cmp -s "$QC" "$T/before-rollback.toml" && [ "$(leftovers)" = 0 ]'
check "rollback: history says failed" 'tail -n 1 "$Q/etc/tv-settings.log" | grep -q " music result from=off to=on read=off outcome=failed source=ctl\$"'
cq stop >/dev/null 2>&1
rm -f "$Q/crashloop"

# Busy: the daemon says so in a fresh run/voice.json (its one definition of
# busy, written by hand here). Refused before anything changes; a stale file,
# a null field or no running daemon proceeds. The stand-in binary above fails
# as a daemon, so a change that proceeds ends in the rollback.
fakesup() {
    sh "$T/fakesup/supervise.sh" &
    FAKEPID=$!
    echo "$FAKEPID" >"$Q/run/supervise.pid"
}
voice_json() { # BUSY-VALUE (JSON text)
    printf '{"schema":1,"mic":"listening","remote_off":"normal","remote_off_active":true,"power_state":"Active","pairing_until":null,"ha":"connected","ha_since":1762000000,"busy":%s,"music":{"enabled":true}}\n' "$1" >"$Q/run/voice.json"
}
cp "$QC" "$T/before-busy.toml"
for b in voice music alarm; do
    fakesup
    voice_json "\"$b\""
    n0=$(nlog)
    out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
    case $b in
    voice) txt="a voice request is in progress" ;;
    music) txt="music is loaded (playing or paused)" ;;
    alarm) txt="a timer alarm is active" ;;
    esac
    check "busy=$b: refused, reason busy, '$txt', retry_in_s null, exit 1" '[ "$rc" -eq 1 ] && [ "$out" = "{\"schema\":1,\"setting\":\"music\",\"requested\":\"on\",\"before\":\"off\",\"after\":\"off\",\"result\":\"refused\",\"daemon\":\"none\",\"error\":\"$txt\",\"reason\":\"busy\",\"retry_in_s\":null}" ]'
    check "busy=$b: config untouched, no temporary file, no stamp, no history line, the supervisor was not touched" 'cmp -s "$QC" "$T/before-busy.toml" && [ "$(leftovers)" = 0 ] && [ ! -e "$stamp" ] && [ "$(nlog)" = "$n0" ] && kill -0 "$FAKEPID" 2>/dev/null'
    kill "$FAKEPID" 2>/dev/null || true
done
# Not busy: a null field, a file with no such field (an older daemon), a stale file.
for case_ in null nofield stale; do
    fakesup
    case $case_ in
    null) voice_json null ;;
    nofield) voice_json null && sed -i 's/"busy":null,//' "$Q/run/voice.json" ;;
    stale) voice_json '"music"' && touch -d "@$(($(date +%s) - 120))" "$Q/run/voice.json" ;;
    esac
    out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
    check "busy file '$case_': not refused (the change goes on to the restart, rolled back by the stand-in)" '! echo "$out" | grep -q "\"result\":\"refused\"" && echo "$out" | grep -q "\"reason\":\"\",\"retry_in_s\":null}\$" && [ -e "$stamp" ] && cmp -s "$QC" "$T/before-busy.toml"'
    cq stop >/dev/null 2>&1
    rm -f "$Q/crashloop"
    kill "$FAKEPID" 2>/dev/null || true
done
# A busy file but no daemon running: proceeds, the config is changed.
rm -f "$Q/run/supervise.pid"
voice_json '"music"'
out=$(cqc config music on 2>/dev/null) && rc=0 || rc=$?
check "busy file but nothing running: applied, daemon not running" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"applied\",\"daemon\":\"not running\"" && qsum | grep -q " music=\[on "'
rm -f "$Q/run/voice.json" "$stamp"

# The mic button is on only while it runs (LG's key taken over, ADR 0013):
# with mic_button = "ptt" in the config but a daemon that reports it failed,
# "on" is a retry (the same config and a restart) and "off" writes off.
mic_json() { # MIC_BUTTON LG_VOICE_KEY ERROR(json)
    printf '{"schema":1,"mic":"listening","remote_off":"normal","remote_off_active":true,"power_state":"Active","pairing_until":null,"ha":"connected","ha_since":1762000000,"busy":null,"mic_button":"%s","lg_voice_key":"%s","mic_button_error":%s,"music":{"enabled":true}}\n' "$1" "$2" "$3" >"$Q/run/voice.json"
}
cp "$T/base.toml" "$QC"
printf '\n[remote]\nmic_button = "ptt"\n' >>"$QC"
cp "$QC" "$T/ptt.toml"
fakesup
mic_json failed lg '"the read-back says \"launch\", not ignore; LG'"'"'s voice key was restored"'
st=$(cq status --json)
check "status: configured ptt, not running, the reason, LG has the key" 'echo "$st" | grep -q "\"mic_button\":\"ptt\",\"mic_button_active\":false,\"mic_button_error\":\"the read-back says \(.*\)\",\"lg_voice_key\":\"lg\","'
check "status text: not running, with the reason" 'cq status 2>/dev/null | grep -q "remote mic button NOT running: the read-back says" && cq status 2>/dev/null | grep -q "LG voice key: lg"'
out=$(cqc config mic-button on 2>/dev/null) && rc=0 || rc=$?
check "mic-button on with ptt configured but failed: a retry, not already (before off; restart, rolled back by the stand-in)" '! echo "$out" | grep -q "\"result\":\"already\"" && echo "$out" | grep -q "\"requested\":\"on\",\"before\":\"off\"" && cmp -s "$QC" "$T/ptt.toml"'
cq stop >/dev/null 2>&1
rm -f "$Q/crashloop"
kill "$FAKEPID" 2>/dev/null || true
fakesup
mic_json failed lg null
out=$(cqc config mic-button off 2>/dev/null) && rc=0 || rc=$?
check "mic-button off with ptt configured but failed: writes off (not already)" '! echo "$out" | grep -q "\"result\":\"already\"" && echo "$out" | grep -q "\"requested\":\"off\",\"before\":\"off\""'
cq stop >/dev/null 2>&1
rm -f "$Q/crashloop"
kill "$FAKEPID" 2>/dev/null || true
cp "$T/ptt.toml" "$QC"
fakesup
mic_json ptt 'ours (configd)' null
out=$(cqc config mic-button on 2>/dev/null) && rc=0 || rc=$?
check "mic-button on while it runs: already" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"result\":\"already\""'
check "status: running, ours" 'cq status --json | grep -q "\"mic_button_active\":true,\"mic_button_error\":null,\"lg_voice_key\":\"ours (configd)\","'
kill "$FAKEPID" 2>/dev/null || true
rm -f "$Q/run/supervise.pid" "$Q/run/voice.json" "$stamp"
cp "$T/base.toml" "$QC"
cp "$HOST_BIN" "$Q/bin/webos-satellite"

# --- restore-lg-voice (ADR 0013): repeatable, only from the state file, only
# volatile writes and the one config name; refused while the daemon runs;
# run by stop, uninstall and purge ---
sf=$Q/run/lg_voice_override
rm -f "$F/calls" "$F/kav" "$F/kaviolation" "$F/kamode" "$sf"
out=$(cq restore-lg-voice 2>/dev/null) && rc=0 || rc=$?
check "restore-lg-voice without the state file: nothing to do, exit 0, no luna call" '[ "$rc" -eq 0 ] && [ "$out" = "{\"schema\":1,\"restore\":\"nothing\",\"lg_voice_key\":\"lg\",\"error\":\"\"}" ] && [ ! -e "$F/calls" ]'
check "status: LG has the key (no state file)" 'cq status --json | grep -q "\"lg_voice_key\":\"lg\","'
echo "configd 1" >"$sf"
echo ignore >"$F/kav"
check "status: a state file and no daemon: unknown" 'cq status --json | grep -q "\"lg_voice_key\":\"unknown\","'
out=$(cq restore-lg-voice 2>/dev/null) && rc=0 || rc=$?
check "restore-lg-voice with the state file: done, LG's value back, state file removed" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"restore\":\"done\",\"lg_voice_key\":\"lg\"" && [ ! -e "$F/kav" ] && [ ! -e "$sf" ]'
check "restore-lg-voice: setConfigs with null and volatile, getConfigs of the one name, nothing else" 'grep -q "setConfigs {\"configs\":{\"com.webos.keyaction.voice\":null},\"volatile\":true}" "$F/calls" && grep -q "getConfigs {\"configNames\":\[\"com.webos.keyaction.voice\"\]}" "$F/calls" && [ "$(wc -l <"$F/calls")" -eq 2 ] && [ ! -e "$F/kaviolation" ]'
out=$(cq restore-lg-voice 2>/dev/null) && rc=0 || rc=$?
check "restore-lg-voice again: nothing (repeatable)" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "\"restore\":\"nothing\""'
echo "configd 1" >"$sf"
echo ignore >"$F/kav"
echo stuck >"$F/kamode"
out=$(cq restore-lg-voice 2>/dev/null) && rc=0 || rc=$?
check "restore not confirmed (still ignore): failed, exit 1, state file kept for the next try" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"restore\":\"failed\",\"lg_voice_key\":\"unknown\",\"error\":\"the key still reads ignore" && [ -e "$sf" ]'
echo refuse >"$F/kamode"
out=$(cq restore-lg-voice 2>/dev/null) && rc=0 || rc=$?
check "restore refused by configd: failed, state file kept" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"restore\":\"failed\"" && [ -e "$sf" ]'
rm -f "$F/kamode"
out=$(WEBOS_SAT_PREFIX=$Q WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LUNA_SEND=$T/no-luna-send sh "$PKG/ctl.sh" restore-lg-voice 2>/dev/null) && rc=0 || rc=$?
check "restore without luna-send: failed, state file kept" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "luna-send not found" && [ -e "$sf" ]'
# Refused while the daemon runs (it holds the key); stop gives it back.
cq start >/dev/null 2>&1
check "daemon started for the restore checks" 'is_ours "$(qpid)"'
out=$(cq restore-lg-voice 2>/dev/null) && rc=0 || rc=$?
check "restore-lg-voice while the daemon runs: refused, nothing called" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "\"restore\":\"refused\"" && [ -e "$sf" ] && [ -e "$F/kav" ]'
echo "configd 1" >"$sf"
echo ignore >"$F/kav"
cq stop >/dev/null 2>&1
check "ctl.sh stop gives LG's key back (state file removed, value back)" '! is_ours "$(qpid)" && [ ! -e "$sf" ] && [ ! -e "$F/kav" ]'
check "no setConfigs without volatile and no category read in all of the above" '[ ! -e "$F/kaviolation" ]'

echo "== not installed, uninstall keeps the log, purge removes it =="
out=$(WEBOS_SAT_PREFIX=$T/none WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LUNA_SEND=$T/luna-send sh "$PKG/ctl.sh" tv-settings --json 2>&1) && rc=0 || rc=$?
check "tv-settings when not installed: exit 1, no JSON" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "not installed"'
out=$(WEBOS_SAT_PREFIX=$T/none WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LUNA_SEND=$T/luna-send sh "$PKG/ctl.sh" set remote-off keep 2>&1) && rc=0 || rc=$?
check "set remote-off when not installed: exit 1" '[ "$rc" -eq 1 ]'
[ -s "$P/etc/tv-settings.log" ] || echo x >"$P/etc/tv-settings.log"
mkdir -p "$P/run"
echo "configd 1" >"$P/run/lg_voice_override"
echo ignore >"$F/kav"
ctl uninstall >/dev/null 2>&1 || true
check "uninstall keeps etc/tv-settings.log" '[ -s "$P/etc/tv-settings.log" ]'
check "uninstall gives LG's voice key back first" '[ ! -e "$F/kav" ]'
: >"$P/.webos-satellite-install"
mkdir -p "$P/run"
echo "configd 1" >"$P/run/lg_voice_override"
echo ignore >"$F/kav"
ctl purge >/dev/null 2>&1 || true
check "purge removes the install, log included" '[ ! -e "$P/etc/tv-settings.log" ] && [ ! -e "$P" ]'
check "purge gives LG's voice key back first" '[ ! -e "$F/kav" ]'

echo
if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) failed"
    exit 1
fi
