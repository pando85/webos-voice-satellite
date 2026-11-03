#!/bin/sh
# D6 experiment: short, quiet beeps at a fixed interval, to see when sound becomes audible
# while the TV changes power state. Each beep is one `pacat` of a 150 ms tone file made by
# `tools/lifecycle/d6_tools.py tone` (0.2 of full scale at 44.1 kHz, the D5 tone level).
# usage: sh d6-beeper.sh LOG SECONDS INTERVAL_MS [TONE_FILE]
# Writes "EPOCH_MS beep N" to LOG just before each pacat starts. Total sound time is
# SECONDS / INTERVAL * 0.15 s; keep SECONDS <= 15 and INTERVAL >= 500. The caller sets the TV volume
# (about 9, original restored) and pauses the satellite's capture (kill switch) if the TV
# microphone is to record the beeps. Stop early: touch /tmp/d6/beeper.stop.
log=$1; secs=${2:-10}; iv=${3:-500}; tone=${4:-/tmp/d6/tone150.raw}
[ -r "$tone" ] || { echo "no tone file $tone" >&2; exit 1; }
[ "$secs" -le 15 ] && [ "$iv" -ge 500 ] || { echo "limits: SECONDS <= 15, INTERVAL_MS >= 500" >&2; exit 2; }
up_ms() { read -r u _ </proc/uptime; s=${u%.*}; f=${u#*.}; f=${f#0}; f=${f:-0}; echo $((s * 1000 + f * 10)); }
s0=$(date +%s); while [ "$(date +%s)" = "$s0" ]; do :; done
off=$(( (s0 + 1) * 1000 - $(up_ms) ))
rm -f /tmp/d6/beeper.stop
end=$(( $(up_ms) + secs * 1000 ))
n=0
while [ "$(up_ms)" -lt "$end" ] && [ ! -e /tmp/d6/beeper.stop ]; do
    n=$((n + 1))
    echo "$(( $(up_ms) + off )) beep $n" >>"$log"
    timeout 3 pacat --playback --format=s16le --rate=44100 --channels=2 --latency-msec=100 "$tone" >/dev/null 2>&1 &
    sleep "$(( iv / 1000 )).$(( iv % 1000 / 100 ))"
done
wait
echo "$(( $(up_ms) + off )) done $n" >>"$log"
