#!/bin/sh
# Host test of d6-watcher.sh with a stub luna-send (no TV involved).
# usage: sh tools/lifecycle/tv/test_d6_watcher.sh
# The stub prints a scripted getPowerState subscription ("wait_call NAME" lines block until
# the watcher has made that call, so the script behaves like a TV that reacts) and records
# every other call in $T/calls. Line formats are copied from the c3-b run (D5 results).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
fail=0

cat >"$T/luna-send" <<'EOF'
#!/bin/sh
# stub: luna-send [-i|-n 1 -w N -f] URI JSON
for a in "$@"; do case "$a" in luna://*) uri=$a ;; esac; done
case "$uri" in
    */getPowerState)
        case "$*" in
            *subscribe*) cat "$STUB_SCRIPT_RUN" ;;   # replaced below by a generated player
            *) st=$(cat "$STUB_STATE"); printf '{ "returnValue": true, "state": "%s" }\n' "$st" ;;
        esac ;;
    */powerOn)
        echo "$uri" >>"$STUB_CALLS"
        if [ -e "$STUB_BLOCK" ]; then
            n=$(cat "$STUB_BLOCK"); if [ "$n" -gt 0 ]; then echo $((n - 1)) >"$STUB_BLOCK"
                echo '{ "errorCode": -110, "returnValue": false, "errorText": "Keyblock1.5second" }'; exit 0; fi
        fi
        echo '{ "returnValue": true }' ;;
    *) echo "$uri" >>"$STUB_CALLS"; echo '{ "returnValue": true }' ;;
esac
EOF
chmod +x "$T/luna-send"

# player: reads a script of "S state" (set state file), "P json" (print a line), "W call" (wait for call), "Z secs"
cat >"$T/player.sh" <<'EOF'
#!/bin/sh
while IFS= read -r line; do
    k=${line%% *}; v=${line#* }
    case "$k" in
        S) printf '%s' "$v" >"$STUB_STATE" ;;
        P) printf '%s\n' "$v" ;;
        Z) sleep "$v" ;;
        C) i=0; while [ $i -lt 100 ] && [ "$(grep -c powerOn "$STUB_CALLS" 2>/dev/null)" -lt "$v" ]; do sleep 0.1; i=$((i + 1)); done ;;
        W) i=0; while [ $i -lt 100 ] && ! grep -q "$v" "$STUB_CALLS" 2>/dev/null; do sleep 0.1; i=$((i + 1)); done ;;
    esac
done <"$STUB_SCRIPT"
EOF
# the stub's subscription branch runs the player
sed -i 's|cat "\$STUB_SCRIPT_RUN"|sh "$STUB_PLAYER"|' "$T/luna-send"

PO='{ "returnValue": true, "state": "Active", "processing": "Request Power Off", "onOff": "off", "reason": "REASON" }'
ST() { printf '{ "returnValue": true, "state": "%s" }' "$1"; }
PR() { printf '{ "returnValue": true, "state": "%s", "processing": "%s", "onOff": "on", "reason": "remoteKey" }' "$1" "$2"; }

run_case() { # name, script file, extra env...; prints calls
    name=$1; script=$2; shift 2
    : >"$T/calls"; rm -rf "$T/d6"
    ( env STUB_CALLS="$T/calls" STUB_STATE="$T/state" STUB_SCRIPT="$script" STUB_PLAYER="$T/player.sh" \
        PATH="$T:$PATH" D6_LUNA="$T/luna-send" D6_DIR="$T/d6" D6_COOLDOWN_S=0 "$@" \
        sh "$HERE/d6-watcher.sh" run >/dev/null 2>&1 )
    sleep 1
    sed 's|luna://com.webos.service.||' "$T/calls" | tr '\n' ' ' >"$T/calls.txt"
    echo "[$name] calls: $(cat "$T/calls.txt")"
}
check() { # label, haystack file, must-contain, must-not-contain
    if grep -q -- "$3" "$2" && { [ -z "${4:-}" ] || ! grep -q -- "$4" "$2"; }; then echo "  ok: $1"; else echo "  FAIL: $1"; fail=1; fi
}
check_none() { if [ ! -s "$T/calls" ]; then echo "  ok: $1"; else echo "  FAIL: $1"; fail=1; fi; }

# 1. observe mode: no calls at all
{
    echo "S Active"; echo "P $(ST Active)"
    echo "P $(echo "$PO" | sed 's/REASON/remoteKey/')"
    echo "Z 0.2"; echo "S ActiveStandby"; echo "P $(ST 'Active Standby')"; echo "Z 0.5"
} >"$T/s1"
run_case observe "$T/s1" D6_MODE=observe
check_none "observe mode makes no call"
check "log says OBSERVE" "$T/d6/watcher.log" "OBSERVE standby settled"

# 2. darken: remoteKey power-off -> powerOn -> Active -> turnOffScreen -> Screen Off
{
    echo "S Active"; echo "P $(ST Active)"
    echo "P $(echo "$PO" | sed 's/REASON/remoteKey/')"
    echo "Z 0.2"; echo "S ActiveStandby"; echo "P $(ST 'Active Standby')"
    echo "W powerOn"
    echo "P $(PR 'Active Standby' 'Prepare Power On')"; echo "P $(PR 'Active Standby' 'Screen On')"
    echo "S Active"; echo "P $(ST Active)"
    echo "W turnOffScreen"
    echo "S ScreenOff"; echo "P $(ST 'Screen Off')"; echo "Z 0.5"
} >"$T/s2"
run_case darken "$T/s2" D6_MODE=darken
check "powerOn then turnOffScreen" "$T/calls.txt" "tvpower/power/powerOn.*tvpower/power/turnOffScreen"
check "no key fallback" "$T/calls.txt" "turnOffScreen" "controls/power"
check "log DONE darkened" "$T/d6/watcher.log" "DONE darkened"

# 3. reason not accepted: no calls
{
    echo "S Active"; echo "P $(ST Active)"
    echo "P $(echo "$PO" | sed 's/REASON/autoOff/')"
    echo "Z 0.2"; echo "S ActiveStandby"; echo "P $(ST 'Active Standby')"; echo "Z 0.5"
} >"$T/s3"
run_case reason "$T/s3" D6_MODE=darken
check_none "unaccepted reason makes no call"
check "log SKIP reason" "$T/d6/watcher.log" "SKIP reason autoOff"

# 4. second press from our own Screen Off: wake, no darkening
{
    cat "$T/s2"
    echo "Z 0.3"
    echo "P { \"returnValue\": true, \"state\": \"Screen Off\", \"processing\": \"Request Power Off\", \"onOff\": \"off\", \"reason\": \"remoteKey\" }"
    echo "Z 0.2"; echo "S ActiveStandby"; echo "P $(ST 'Active Standby')"
    echo "Z 0.6"
    echo "S Active"; echo "P $(ST Active)"; echo "Z 0.8"
} >"$T/s4"
run_case second-press "$T/s4" D6_MODE=darken
check "second press wakes" "$T/calls.txt" "powerOn.*turnOffScreen.*powerOn"
if [ "$(grep -o turnOffScreen "$T/calls.txt" | wc -l)" -eq 1 ]; then echo "  ok: second press does not darken"; else echo "  FAIL: second press darkened"; fail=1; fi
check "log second press" "$T/d6/watcher.log" "second press"

# 5. powerOn does nothing: state-checked key fallback
{
    echo "S Active"; echo "P $(ST Active)"
    echo "P $(echo "$PO" | sed 's/REASON/remoteKey/')"
    echo "Z 0.2"; echo "S ActiveStandby"; echo "P $(ST 'Active Standby')"
    echo "Z 6"
} >"$T/s5"
run_case fallback "$T/s5" D6_MODE=darken
check "key fallback after powerOn" "$T/calls.txt" "powerOn.*networkinput/controls/power"

# 5b. keyblock refusals: powerOn is retried, then the call goes through, no key
echo 2 >"$T/block"
{
    echo "S Active"; echo "P $(ST Active)"
    echo "P $(echo "$PO" | sed 's/REASON/remoteKey/')"
    echo "Z 0.2"; echo "S ActiveStandby"; echo "P $(ST 'Active Standby')"
    echo "C 3"
    echo "S Active"; echo "P $(ST Active)"
    echo "W turnOffScreen"
    echo "S ScreenOff"; echo "P $(ST 'Screen Off')"; echo "Z 0.5"
} >"$T/s5b"
run_case keyblock "$T/s5b" D6_MODE=darken STUB_BLOCK="$T/block" D6_RETRY_MS=300
check "powerOn retried 3 times, then turnOffScreen, no key" "$T/calls.txt" "powerOn.*powerOn.*powerOn.*turnOffScreen" "controls/power"

# 6. hold file blocks the action
mkdir -p "$T/d6"; : >"$T/d6/hold"
: >"$T/calls"
( env STUB_CALLS="$T/calls" STUB_STATE="$T/state" STUB_SCRIPT="$T/s2" STUB_PLAYER="$T/player.sh" PATH="$T:$PATH" \
    D6_LUNA="$T/luna-send" D6_DIR="$T/d6" D6_MODE=darken sh "$HERE/d6-watcher.sh" run >/dev/null 2>&1 )
check_none "hold file: no call"

[ "$fail" = 0 ] && echo "d6-watcher tests: PASS" || { echo "d6-watcher tests: FAIL"; exit 1; }
