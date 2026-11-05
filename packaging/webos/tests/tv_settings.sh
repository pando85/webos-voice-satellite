#!/bin/sh
# Host test of `ctl.sh tv-settings` and `ctl.sh set ...` (ADR 0008, design
# docs/design/voice-and-power-screen.md section 6) against a fake luna-send.
# The fake keeps its state in files and serves synthetic JSON written by hand
# (no TV output). It can answer nothing (a timeout), refuse (returnValue
# false), ignore a write (mismatch), or print only on a terminal. No binary,
# no network, nothing outside a temporary directory.
#
# usage: sh packaging/webos/tests/tv_settings.sh
# The checks are strings run with eval, so single quotes are intended.
# shellcheck disable=SC2016,SC2034,SC1091,SC2086,SC2126
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
PKG=$(cd "$HERE/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/webos-sat-tvset.XXXXXX")
P=$T/prefix
F=$T/fake
fails=0
trap 'rm -rf "$T"' EXIT INT TERM

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

echo "== not installed, uninstall keeps the log, purge removes it =="
out=$(WEBOS_SAT_PREFIX=$T/none WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LUNA_SEND=$T/luna-send sh "$PKG/ctl.sh" tv-settings --json 2>&1) && rc=0 || rc=$?
check "tv-settings when not installed: exit 1, no JSON" '[ "$rc" -eq 1 ] && echo "$out" | grep -q "not installed"'
out=$(WEBOS_SAT_PREFIX=$T/none WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_LUNA_SEND=$T/luna-send sh "$PKG/ctl.sh" set remote-off keep 2>&1) && rc=0 || rc=$?
check "set remote-off when not installed: exit 1" '[ "$rc" -eq 1 ]'
[ -s "$P/etc/tv-settings.log" ] || echo x >"$P/etc/tv-settings.log"
ctl uninstall >/dev/null 2>&1 || true
check "uninstall keeps etc/tv-settings.log" '[ -s "$P/etc/tv-settings.log" ]'
: >"$P/.webos-satellite-install"
ctl purge >/dev/null 2>&1 || true
check "purge removes the install, log included" '[ ! -e "$P/etc/tv-settings.log" ] && [ ! -e "$P" ]'

echo
if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) failed"
    exit 1
fi
