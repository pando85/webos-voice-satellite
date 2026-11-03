#!/bin/sh
# D6 experiment: read-only snapshot of the audio and screen state, to compare Active,
# Screen Off and Active Standby (why is a reply silent in Active Standby?).
# usage: sh d6-snapshot.sh LABEL        (run through `ssh -tt`: luna getters need a terminal)
# Writes /tmp/d6/snap-LABEL.txt. Reads only luna getters, `pactl list`, `amixer contents` of
# card 0 and /proc/asound status files. NEVER reads ASoC/DAPM sysfs or /sys/kernel/debug, and
# walks nothing. Sets nothing.
label=${1:?label}
out=/tmp/d6/snap-$label.txt
mkdir -p /tmp/d6
L() { # uri json
    echo "--- $1 $2"
    luna-send -n 1 -w 4000 -f "$1" "$2" 2>&1 | cat | tr -d '\r'
}
{
    echo "=== snapshot $label at $(date +%s)"
    L luna://com.webos.service.tvpower/power/getPowerState '{}'
    L luna://com.webos.service.alwaysready/aod/getStatus '{}'
    L luna://com.webos.service.audio/master/getVolume '{}'
    L luna://com.webos.service.apiadapter/audio/getSoundOutput '{}'
    L luna://com.webos.service.audiooutput/audio/getStatus '{}'
    L luna://com.webos.service.audiooutput/audio/volume/getStatus '{}'
    L luna://com.webos.service.avmutemanager/getMuteStatus '{}'
    L luna://com.webos.service.avmutemanager/getMuteOwnerList '{}'
    L luna://com.webos.service.avmutemanager/getMuteInfo '{}'
    L luna://com.webos.service.avmutemanager/getRegisterDB '{}'
    L luna://com.webos.settingsservice/getSystemSettings '{"category":"sound","keys":["soundOutput","externalDeviceControl"]}'
    L luna://com.webos.settingsservice/getSystemSettings '{"category":"picture","keys":["energySaving","backlight","oledLight"]}'
    echo "--- pactl list sinks (name, state, mute, volume, active port)"
    pactl list sinks 2>&1 | grep -E "^Sink|Name:|State:|Mute:|Volume:|Active Port:|Suspend" | cut -c1-160
    echo "--- pactl list short sink-inputs"
    pactl list short sink-inputs 2>&1
    echo "--- /proc/asound card0 playback PCM 6 status"
    cat /proc/asound/card0/pcm6p/sub0/status 2>&1
    echo "--- amixer -c 0 contents (controls and values; read through the ALSA control API)"
    timeout 10 amixer -c 0 contents 2>&1 | cut -c1-200
    echo "--- end"
} >"$out" 2>&1
echo "$out"
