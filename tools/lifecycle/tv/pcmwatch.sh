#!/bin/sh
# Logs state changes of the capture PCM (hw:LGSIC,0) as "epoch_ms state".
# usage: sh pcmwatch.sh OUT   (stop: touch /tmp/pcmwatch.stop)
# Reads /proc/asound/LGSIC/pcm0c/sub0/status every ~5 ms (no ASoC/DAPM sysfs,
# no debugfs); the epoch offset is calibrated once from date(1) and
# /proc/uptime (about 10 to 20 ms accuracy). Writes only OUT.
st=/proc/asound/LGSIC/pcm0c/sub0/status
out=$1
up_ms() { read -r u _ </proc/uptime; s=${u%.*}; f=${u#*.}; f=${f#0}; echo $((s * 1000 + f * 10)); }
s0=$(date +%s); while [ "$(date +%s)" = "$s0" ]; do :; done; off=$(( (s0 + 1) * 1000 - $(up_ms) ))
echo "offset_ms $off" >"$out"
prev=
while [ ! -e /tmp/pcmwatch.stop ]; do
    read -r l <"$st"
    if [ "$l" != "$prev" ]; then echo "$(( $(up_ms) + off )) $l" >>"$out"; prev=$l; fi
    sleep 0.005
done
rm -f /tmp/pcmwatch.stop
