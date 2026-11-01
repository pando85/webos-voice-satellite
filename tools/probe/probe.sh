#!/bin/sh
# Read-only inventory for rooted LG webOS TVs.
# No audio capture, config changes, root escalation or persistent install.
set -u
LC_ALL=C
export LC_ALL

section() { printf '\n===== %s =====\n' "$1"; }
run() {
    label=$1
    shift
    section "$label"
    if command -v "$1" >/dev/null 2>&1; then
        "$@" 2>&1 || printf 'Command failed (non-fatal): %s\n' "$1"
    else
        printf 'Unavailable: %s\n' "$1"
    fi
}
show_file() {
    section "$1"
    if [ -r "$2" ]; then cat "$2" 2>&1; else echo 'Unavailable'; fi
}

section "NOTICE"
echo 'READ-ONLY. Treat output as sensitive; redact before sharing.'
run 'Kernel' uname -a
run 'Machine' uname -m
run 'CPU word size' getconf LONG_BIT
show_file 'ALSA cards' /proc/asound/cards
show_file 'ALSA PCM endpoints' /proc/asound/pcm
run 'Audio device entries' ls -l /dev/snd
run 'Memory' cat /proc/meminfo
section 'Relevant process names only (no command arguments)'
if command -v ps >/dev/null 2>&1; then
    ps 2>/dev/null | grep -Ei 'voice|audio|mic|speech|alwaysready|pulse|pipewire' | grep -v grep || :
else
    echo 'Unavailable: ps'
fi
section 'Tool availability'
for tool in arecord aplay amixer pactl pulseaudio luna-send systemctl strace readelf file; do
    if command -v "$tool" >/dev/null 2>&1; then
        printf '%s: available\n' "$tool"
    else
        printf '%s: unavailable\n' "$tool"
    fi
done
section 'End of read-only inventory'
