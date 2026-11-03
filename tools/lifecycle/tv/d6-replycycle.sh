#!/bin/sh
# D6 experiment, variant B: from Active Standby wake the TV with the screen off for a reply, then
# return to Active Standby. Stamps every step (epoch ms) into LOG.
# usage: sh d6-replycycle.sh LOG [HOLD_S]      (run through `ssh -tt`)
#   t_req            the moment a reply "must play"
#   powerOn          tvpower/power/powerOn {"reason":"remoteKey"}, retried after "Keyblock1.5second"
#   Active           polled getPowerState
#   turnOffScreen    right after Active
#   (HOLD_S seconds pass here: the caller's beeps play)
#   power key        networkinput/controls/power (toggle, state-checked: only from Screen Off or Active)
#   Active Standby   polled again
# Needs the TV in settled Active Standby; refuses otherwise.
log=${1:?log}; hold=${2:-6}
TP=luna://com.webos.service.tvpower/power
up_ms() { read -r u _ </proc/uptime; s=${u%.*}; f=${u#*.}; f=${f#0}; f=${f:-0}; echo $((s * 1000 + f * 10)); }
s0=$(date +%s); while [ "$(date +%s)" = "$s0" ]; do :; done
off=$(( (s0 + 1) * 1000 - $(up_ms) ))
now() { echo $(( $(up_ms) + off )); }
L() { echo "$(now) $*" >>"$log"; }
st() { luna-send -n 1 -w 2000 -f "$TP/getPowerState" '{}' 2>/dev/null | cat | tr -d ' \r\n"' | sed -n 's/.*state:\([A-Za-z]*\).*/\1/p'; }
[ "$(st)" = ActiveStandby ] || { L "REFUSED state=$(st)"; exit 1; }
L "t_req"
n=0
while [ $n -lt 4 ]; do
    n=$((n + 1))
    r=$(luna-send -n 1 -w 3000 -f "$TP/powerOn" '{"reason":"remoteKey"}' 2>&1 | cat | tr -d ' \r\n')
    L "powerOn try $n -> $r"
    case "$r" in *Keyblock*) sleep 0.4 ;; *) break ;; esac
done
i=0
while [ $i -lt 100 ]; do [ "$(st)" = Active ] && break; i=$((i + 1)); sleep 0.1; done
L "state Active (polled)"
r=$(luna-send -n 1 -w 3000 -f "$TP/turnOffScreen" '{}' 2>&1 | cat | tr -d ' \r\n')
L "turnOffScreen -> $r"
sleep "$hold"
s=$(st); L "before return: state=$s"
case "$s" in
    ScreenOff|Active)
        r=$(luna-send -n 1 -w 3000 -f luna://com.webos.service.networkinput/controls/power '{}' 2>&1 | cat | tr -d ' \r\n')
        L "power key -> $r"
        i=0
        while [ $i -lt 100 ]; do [ "$(st)" = ActiveStandby ] && break; i=$((i + 1)); sleep 0.1; done
        L "state $(st) (polled)" ;;
    *) L "no return key: state=$s" ;;
esac
