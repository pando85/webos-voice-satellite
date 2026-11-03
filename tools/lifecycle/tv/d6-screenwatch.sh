#!/bin/sh
# D6 experiment: stamp every change that the panel controller reports about the screen.
# usage: sh d6-screenwatch.sh LOG        (run through `ssh -tt`; stop: touch /tmp/d6/screenwatch.stop)
# Polls panelcontroller/getScreenMuteStatus as fast as luna-send allows (the subscription does not
# push changes on this firmware: seen on c3-b) and subscribes to isOledScreenVisible. Writes
# "EPOCH_MS mute|visible RAW" lines when the value changes. What these getters mean physically is NOT
# established (mute was false in Active and true in Screen Off; visible stayed true in both);
# treat as indirect evidence.
log=${1:?log}
mkdir -p /tmp/d6
up_ms() { read -r u _ </proc/uptime; s=${u%.*}; f=${u#*.}; f=${f#0}; f=${f:-0}; echo $((s * 1000 + f * 10)); }
s0=$(date +%s); while [ "$(date +%s)" = "$s0" ]; do :; done
off=$(( (s0 + 1) * 1000 - $(up_ms) ))
rm -f /tmp/d6/screenwatch.stop
sub() { # name uri json
    luna-send -i "$2" "$3" 2>&1 | while IFS= read -r l; do
        echo "$(( $(up_ms) + off )) $1 $(printf '%s' "$l" | tr -d '\r' | tr -s ' ')" >>"$log"
        [ -e /tmp/d6/screenwatch.stop ] && break
    done
}
sub visible luna://com.webos.service.panelcontroller/isOledScreenVisible '{"subscribe":true}' &
prev=
while [ ! -e /tmp/d6/screenwatch.stop ]; do
    v=$(luna-send -n 1 -w 1000 luna://com.webos.service.panelcontroller/getScreenMuteStatus '{}' 2>&1 | cat | tr -d '\r\n ')
    if [ "$v" != "$prev" ]; then echo "$(( $(up_ms) + off )) mute $v" >>"$log"; prev=$v; fi
done
for p in /proc/[0-9]*; do
    c=$(tr '\0' ' ' <"$p/cmdline" 2>/dev/null)
    case "$c" in *panelcontroller/*subscribe*) kill "${p#/proc/}" 2>/dev/null ;; esac
done
