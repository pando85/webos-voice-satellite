#!/bin/sh
# D6 experiment: watch the power state and, after a power-off, bring the TV back
# to Active and darken the screen ("wake and darken"). EXPERIMENT ONLY: this is
# not the daemon, it is a measurement tool to be run from /tmp on the TV.
#
# usage: sh d6-watcher.sh run      (foreground; needs a terminal: ssh -tt)
#        sh d6-watcher.sh stop     (kills the subscription and the loop)
#
# Modes (D6_MODE): observe (default: log only, never calls anything) or darken.
# State machine, driven by the getPowerState subscription lines:
#   "Request Power Off" (onOff off, reason R)  -> remember R and the settled state before it
#   settled "Active Standby" within 20 s of it  -> if mode darken, reason accepted, not on hold:
#       wake (D6_WAKE) ; when settled "Active" (or the D6_DARKEN_AT line) -> turnOffScreen ;
#       settled "Screen Off" -> done, DARK=1
#   A power-off that starts from a "Screen Off" that this script made (DARK=1) is read as a
#   "second press = turn on": wake, do NOT darken (so the user is never locked out).
# Safety: D6_MAX_ACTIONS, D6_COOLDOWN_S, a hold file (touch $D6_DIR/hold: observe only) and a
# stop file (touch $D6_DIR/stop). The wake by key is a toggle, so it is only sent while a
# fresh getPowerState still says Active Standby.
# Reads: luna getPowerState. Writes (mode darken only): tvpower powerOn or networkinput
# controls/power, tvpower turnOffScreen. Files: only under $D6_DIR.
#
# Environment (all optional):
#   D6_DIR=/tmp/d6          work directory (log, flags)
#   D6_MODE=observe|darken
#   D6_WAKE=poweron|key     poweron = tvpower/power/powerOn {"reason":D6_WAKE_REASON} (powerOn needs a reason from
#                           tvpowerd's powerOnReasons list; {} is refused), key = controls/power
#   D6_WAKE_REASON=remoteKey  D6_RETRY_MS=1600 (pause before a powerOn retry after a keyblock refusal)
#   D6_FALLBACK_KEY=1       if powerOn left the TV in Active Standby for 4 s, send the key (state-checked)
#   D6_DARKEN_AT=active|screenon|lastinput|preparepoweron   which line triggers turnOffScreen
#   D6_DELAY_MS=0           pause between "Active Standby" and the wake
#   D6_ACCEPT=remoteKey     space-separated reasons of "Request Power Off" that are acted on
#   D6_MAX_ACTIONS=10  D6_COOLDOWN_S=20
#   D6_LUNA=luna-send       the luna-send binary (tests use a stub)

D6_DIR=${D6_DIR:-/tmp/d6}
D6_MODE=${D6_MODE:-observe}
D6_WAKE=${D6_WAKE:-poweron}
D6_WAKE_REASON=${D6_WAKE_REASON:-remoteKey}
D6_RETRY_MS=${D6_RETRY_MS:-1600}
D6_FALLBACK_KEY=${D6_FALLBACK_KEY:-1}
D6_DARKEN_AT=${D6_DARKEN_AT:-active}
D6_DELAY_MS=${D6_DELAY_MS:-0}
D6_ACCEPT=${D6_ACCEPT:-remoteKey}
D6_MAX_ACTIONS=${D6_MAX_ACTIONS:-10}
D6_COOLDOWN_S=${D6_COOLDOWN_S:-20}
D6_LUNA=${D6_LUNA:-luna-send}
D6_UPTIME=${D6_UPTIME:-/proc/uptime}

LOG=$D6_DIR/watcher.log
TP=luna://com.webos.service.tvpower/power

mkdir -p "$D6_DIR" || exit 1

# Epoch milliseconds from /proc/uptime (10 ms resolution) plus an offset calibrated once
# against date(1), so lines can be ordered with sub-second precision. The offset is
# recomputed from date(1) only at start.
up_ms() { read -r u _ <"$D6_UPTIME"; s=${u%.*}; f=${u#*.}; f=${f#0}; f=${f:-0}; echo $((s * 1000 + f * 10)); }
calibrate() {
    s0=$(date +%s)
    while [ "$(date +%s)" = "$s0" ]; do :; done
    OFF=$(( (s0 + 1) * 1000 - $(up_ms) ))
}
now() { echo $(( $(up_ms) + OFF )); }
log() { echo "$(now) $*" >>"$LOG"; }

# One getPowerState without subscription; prints the compact state name (ActiveStandby, ...)
getstate() {
    "$D6_LUNA" -n 1 -w 2000 -f "$TP/getPowerState" '{}' 2>/dev/null | cat | tr -d ' \r\n"' |
        sed -n 's/.*state:\([A-Za-z]*\).*/\1/p'
}

# Background job: wake the TV from Active Standby.
wake_job() {
    d=$1
    [ "$d" -gt 0 ] && sleep "$(( d / 1000 )).$(( d % 1000 / 100 ))"
    if [ "$D6_WAKE" = key ]; then
        st=$(getstate)
        if [ "$st" = ActiveStandby ]; then
            r=$("$D6_LUNA" -n 1 -w 3000 -f luna://com.webos.service.networkinput/controls/power '{}' 2>&1 | cat | tr -d ' \r\n')
            log "CALL wake(key) -> $r"
        else
            log "CALL wake(key) SKIPPED state=$st"
        fi
        return
    fi
    # tvpowerd refuses a power-on in the first 1.5 s after the standby settled ("Keyblock1.5second", seen on c3-b);
    # retry after D6_RETRY_MS, up to 3 times
    n=0
    while [ $n -lt 3 ]; do
        n=$((n + 1))
        r=$("$D6_LUNA" -n 1 -w 3000 -f "$TP/powerOn" "{\"reason\":\"$D6_WAKE_REASON\"}" 2>&1 | cat | tr -d ' \r\n')
        log "CALL wake(powerOn) try $n -> $r"
        case "$r" in *Keyblock*) sleep "$(( D6_RETRY_MS / 1000 )).$(( D6_RETRY_MS % 1000 / 100 ))" ;; *) break ;; esac
    done
    if [ "$D6_FALLBACK_KEY" = 1 ]; then
        i=0
        while [ $i -lt 8 ]; do
            sleep 0.5
            st=$(getstate)
            [ "$st" != ActiveStandby ] && return
            i=$((i + 1))
        done
        st=$(getstate)
        if [ "$st" = ActiveStandby ]; then
            r=$("$D6_LUNA" -n 1 -w 3000 -f luna://com.webos.service.networkinput/controls/power '{}' 2>&1 | cat | tr -d ' \r\n')
            log "CALL wake(key fallback) -> $r"
        fi
    fi
}

# Background job: turnOffScreen, then check; one retry if the TV stayed Active.
dark_job() {
    r=$("$D6_LUNA" -n 1 -w 3000 -f "$TP/turnOffScreen" '{}' 2>&1 | cat | tr -d ' \r\n')
    log "CALL turnOffScreen -> $r"
    sleep 3
    st=$(getstate)
    # only while still waiting for Screen Off (a second press meanwhile must not be darkened)
    if [ "$st" = Active ] && [ -e "$D6_DIR/await.ScreenOff" ]; then
        r=$("$D6_LUNA" -n 1 -w 3000 -f "$TP/turnOffScreen" '{}' 2>&1 | cat | tr -d ' \r\n')
        log "CALL turnOffScreen (retry, state still Active) -> $r"
    fi
}

# Background guard: log if the awaited state did not arrive.
guard_job() { # $1 awaited name, $2 seconds
    sleep "$2"
    [ -e "$D6_DIR/await.$1" ] && log "TIMEOUT waiting for $1 after $2 s (state now $(getstate))"
    rm -f "$D6_DIR/await.$1"
}

stop_all() {
    touch "$D6_DIR/stop"
    for p in /proc/[0-9]*; do
        pid=${p#/proc/}
        c=$(tr '\0' ' ' 2>/dev/null <"$p/cmdline")
        case "$c" in
            *getPowerState*subscribe*) kill "$pid" 2>/dev/null ;;
            "sh "*d6-watcher.sh*run*) [ "$pid" != $$ ] && kill "$pid" 2>/dev/null ;;
        esac
    done
}

run() {
    rm -f "$D6_DIR/stop" "$D6_DIR"/await.*
    calibrate
    log "START mode=$D6_MODE wake=$D6_WAKE darken_at=$D6_DARKEN_AT delay_ms=$D6_DELAY_MS accept='$D6_ACCEPT' max=$D6_MAX_ACTIONS cooldown=$D6_COOLDOWN_S offset=$OFF"
    PHASE=idle; DARK=0; POFF=0; POFF_AT=0; POFF_REASON=; POFF_FROM=; NACT=0; LAST_ACT=0
    DO_DARKEN=1; DARKEN_DONE=0
    "$D6_LUNA" -i "$TP/getPowerState" '{"subscribe":true}' 2>&1 | while IFS= read -r raw; do
        [ -e "$D6_DIR/stop" ] && break
        l=$(printf '%s' "$raw" | tr -d ' \r')
        case "$l" in *'"state"'*) ;; *) continue ;; esac
        t=$(now)
        echo "$t PWR $raw" | tr -d '\r' >>"$LOG"
        s=${l#*\"state\":\"}; s=${s%%\"*}
        p=; case "$l" in *'"processing":"'*) p=${l#*\"processing\":\"}; p=${p%%\"*} ;; esac
        o=; case "$l" in *'"onOff":"'*) o=${l#*\"onOff\":\"}; o=${o%%\"*} ;; esac
        r=; case "$l" in *'"reason":"'*) r=${l#*\"reason\":\"}; r=${r%%\"*} ;; esac

        if [ -z "$p" ]; then
            case "$s" in
                ScreenOff)
                    if [ "$PHASE" = darkening ]; then
                        DARK=1; PHASE=idle; rm -f "$D6_DIR/await.ScreenOff"
                        log "DONE darkened: power-off request to Screen Off $(( t - POFF_AT )) ms"
                    fi ;;
                Active|ScreenSaver)
                    rm -f "$D6_DIR/await.Active"
                    if [ "$PHASE" = idle ]; then DARK=0; fi ;;
            esac
        fi

        # a power-off request: remember who/what and from which settled state
        if [ "$p" = RequestPowerOff ] && [ "$o" = off ]; then
            POFF=1; POFF_AT=$t; POFF_REASON=$r; POFF_FROM=$s
            case " $D6_ACCEPT " in *" $r "*) ok=yes ;; *) ok=no ;; esac
            log "POWEROFF request reason=$r from=$s accepted=$ok dark_by_us=$DARK"
            POFF_OK=$ok
        fi

        # act on the settled Active Standby that follows the request
        if [ -z "$p" ] && [ "$s" = ActiveStandby ] && [ "$POFF" = 1 ] && [ $(( t - POFF_AT )) -lt 20000 ]; then
            POFF=0
            if [ "$D6_MODE" != darken ]; then log "OBSERVE standby settled $(( t - POFF_AT )) ms after the request (no action: mode observe)"
            elif [ "$POFF_OK" != yes ]; then log "SKIP reason $POFF_REASON not accepted"
            elif [ -e "$D6_DIR/hold" ]; then log "SKIP hold file present"
            elif [ "$NACT" -ge "$D6_MAX_ACTIONS" ]; then log "SKIP max actions $NACT reached"
            elif [ $(( t / 1000 - LAST_ACT )) -lt "$D6_COOLDOWN_S" ]; then log "SKIP cooldown"
            else
                NACT=$((NACT + 1)); LAST_ACT=$((t / 1000))
                if [ "$POFF_FROM" = ScreenOff ] && [ "$DARK" = 1 ]; then
                    DO_DARKEN=0; log "ACT second press (power-off from our Screen Off): wake without darkening"
                else
                    DO_DARKEN=1; log "ACT wake-and-darken #$NACT, standby settled $(( t - POFF_AT )) ms after the request"
                fi
                PHASE=waking; DARKEN_DONE=0; : >"$D6_DIR/await.Active"
                wake_job "$D6_DELAY_MS" &
                guard_job Active 15 &
            fi
        fi

        # waking: decide when to darken
        if [ "$PHASE" = waking ]; then
            go=0
            case "$D6_DARKEN_AT" in
                active) [ -z "$p" ] && [ "$s" = Active ] && go=1 ;;
                screenon) [ "$p" = ScreenOn ] && go=1 ;;
                lastinput) [ "$p" = LastInputReady ] && go=1 ;;
                preparepoweron) [ "$p" = PreparePowerOn ] && go=1 ;;
            esac
            if [ "$go" = 1 ] && [ "$DARKEN_DONE" = 0 ]; then
                DARKEN_DONE=1
                if [ "$DO_DARKEN" = 1 ]; then
                    PHASE=darkening; : >"$D6_DIR/await.ScreenOff"
                    log "ACT darken trigger '$D6_DARKEN_AT' $(( t - POFF_AT )) ms after the request"
                    dark_job &
                    guard_job ScreenOff 15 &
                fi
            fi
            if [ -z "$p" ] && [ "$s" = Active ]; then
                rm -f "$D6_DIR/await.Active"
                if [ "$DO_DARKEN" = 0 ]; then
                    PHASE=idle; DARK=0
                    log "DONE woke without darkening: $(( t - POFF_AT )) ms"
                fi
            fi
        fi
    done
    log "END watcher"
}

case "${1:-run}" in
    run) run ;;
    stop) stop_all ;;
    *) echo "usage: sh $0 run|stop" >&2; exit 2 ;;
esac
