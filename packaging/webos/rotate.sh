#!/bin/sh
# Size-based log rotation sidecar, started by start.sh.
# usage: rotate.sh DAEMON_PID LOGFILE MAX_BYTES
# Every 30 s: if LOGFILE exceeds MAX_BYTES, copy it to LOGFILE.1 (replacing
# the previous copy) and truncate it. The daemon appends (O_APPEND), so the
# truncation is safe. Exits when the daemon is gone. One-second sleeps keep
# the orphan sleep short-lived when this script is killed.
pid=$1
log=$2
max=$3
i=0
while kill -0 "$pid" 2>/dev/null; do
    sleep 1
    i=$((i + 1))
    if [ "$i" -ge 30 ]; then
        i=0
        size=$(wc -c <"$log" 2>/dev/null || echo 0)
        if [ "${size:-0}" -gt "$max" ]; then
            cp "$log" "$log.1" && : >"$log"
        fi
    fi
done
