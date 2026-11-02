#!/bin/sh
# Measures kill-switch latency: N cycles of create /tmp/webos-satellite.disable,
# wait for the capture PCM to show "closed", remove it, wait for "RUNNING".
# usage: sh killcycle.sh N
# Times come from /proc/uptime (10 ms ticks) plus a ~5 ms polling step.
st=/proc/asound/LGSIC/pcm0c/sub0/status
ks=/tmp/webos-satellite.disable
n=${1:-10}
up_ms() { read -r u _ </proc/uptime; s=${u%.*}; f=${u#*.}; f=${f#0}; echo $((s * 1000 + f * 10)); }
state() { read -r l <"$st"; echo "$l"; }
waitfor() { # want-pattern timeout_s ; prints uptime ms when reached
    end=$(( $(up_ms) + $2 * 1000 ))
    while :; do
        l=$(state)
        case $l in "$1") up_ms; return 0 ;; esac
        [ "$(up_ms)" -lt "$end" ] || { echo TIMEOUT; return 1; }
        sleep 0.005
    done
}
i=1
while [ "$i" -le "$n" ]; do
    waitfor "state: RUNNING" 10 >/dev/null || { echo "cycle $i: not running"; exit 1; }
    sleep 1
    t0=$(up_ms); : >"$ks"
    t1=$(waitfor "closed" 5)
    sleep 2
    t2=$(up_ms); rm -f "$ks"
    t3=$(waitfor "state: RUNNING" 10)
    echo "cycle $i close_ms=$((t1 - t0)) reopen_ms=$((t3 - t2))"
    i=$((i + 1))
done
