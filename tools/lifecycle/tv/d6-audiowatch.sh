#!/bin/sh
# D6 experiment: stamp every change of two read-only audio indicators while the power state changes:
#   the ALSA mixer control 'Gain Input' of card 0 (read with `amixer cget`), and the avmutemanager audio
#   mute flags (`getMuteInfo`, audioMuteStatus values). Polls about every 0.2 s.
# usage: sh d6-audiowatch.sh LOG        (run through `ssh -tt`; stop: touch /tmp/d6/audiowatch.stop)
# Reads only: amixer cget of one named control, one luna getter. Nothing is set.
log=${1:?log}
mkdir -p /tmp/d6
up_ms() { read -r u _ </proc/uptime; s=${u%.*}; f=${u#*.}; f=${f#0}; f=${f:-0}; echo $((s * 1000 + f * 10)); }
s0=$(date +%s); while [ "$(date +%s)" = "$s0" ]; do :; done
off=$(( (s0 + 1) * 1000 - $(up_ms) ))
rm -f /tmp/d6/audiowatch.stop
pg=; pm=
while [ ! -e /tmp/d6/audiowatch.stop ]; do
    g=$(amixer -c 0 cget name='Gain Input' 2>/dev/null | sed -n 's/^ *: values=//p')
    if [ "$g" != "$pg" ]; then echo "$(( $(up_ms) + off )) gain $g" >>"$log"; pg=$g; fi
    m=$(luna-send -n 1 -w 1000 luna://com.webos.service.avmutemanager/getMuteInfo '{}' 2>&1 | cat | tr -d ' \r\n' | grep -o '"audioMuteStatus":[a-z]*' | tr '\n' ' ')
    if [ "$m" != "$pm" ]; then echo "$(( $(up_ms) + off )) avmute $m" >>"$log"; pm=$m; fi
done
