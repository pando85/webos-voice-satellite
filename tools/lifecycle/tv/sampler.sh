#!/bin/sh
# Read-only resource sampler for a satellite installed under /tmp/webos-satellite.
# usage: sh sampler.sh OUT [INTERVAL_S]   (stop: touch /tmp/p3-sampler.stop)
# One line per interval: epoch pid fds threads rss_kb cpu_ticks(utime+stime)
# pcm_state tcp6053_established. Reads /proc and netstat; writes only OUT.
out=$1; iv=${2:-10}
P=/tmp/webos-satellite
while [ ! -e /tmp/p3-sampler.stop ]; do
    pid=$(cat $P/run/webos-satellite.pid 2>/dev/null)
    if [ -n "$pid" ] && [ -d /proc/"$pid" ]; then
        # shellcheck disable=SC2012 # fd names are numbers
        fds=$(ls /proc/"$pid"/fd | wc -l)
        thr=$(sed -n 's/^Threads:\s*//p' /proc/"$pid"/status)
        rss=$(sed -n 's/^VmRSS:\s*\([0-9]*\).*/\1/p' /proc/"$pid"/status)
        cpu=$(awk '{print $14+$15}' /proc/"$pid"/stat)
    else
        fds=-; thr=-; rss=-; cpu=-
    fi
    read -r pcm < /proc/asound/LGSIC/pcm0c/sub0/status
    est=$(netstat -tn 2>/dev/null | grep -c ':6053 .*ESTABLISHED')
    echo "$(date +%s) ${pid:--} $fds $thr $rss $cpu ${pcm#state: } $est" >> "$out"
    sleep "$iv"
done
rm -f /tmp/p3-sampler.stop
