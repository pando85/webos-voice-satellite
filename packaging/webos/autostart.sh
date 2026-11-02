#!/bin/sh
# Boot-time start, launched detached by the init.d hook (init.d/webos-satellite).
# Waits, checks every "do not start" condition, then starts the supervised
# daemon. Writes only DIR/log/autostart.log (plus what start.sh writes).
#
# usage: autostart.sh [--prefix DIR] [--delay SECS]
#
#   --delay SECS  wait this long first (default 60): well past the Homebrew
#                 Channel failsafe window, which closes 10 s after the
#                 init.d hooks return
#
# Does not start (exit 0, one log line) if any of these hold:
#   DIR/disable exists                       (owner's "do not start")
#   DIR/crashloop exists                     (the supervisor gave up earlier)
#   /tmp/webos-satellite.disable exists      (kill switch)
#   `webos-satellite --check-firmware` fails (firmware not validated, or
#                                             the allow-list is empty)
set -u
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
delay=60
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    --delay) [ $# -ge 2 ] || die "--delay needs a value"; delay=$2; shift 2 ;;
    -h | --help) sed -n '2,19p' "$0"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done
case $delay in
'' | *[!0-9]*) die "--delay must be a number of seconds" ;;
esac
resolve_prefix "$0" "$prefix_arg"
require_installed

alog() {
    echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) autostart: $*" >>"$PREFIX/log/autostart.log"
}

sleep "$delay"
if [ -e "$DISABLE" ]; then
    alog "$DISABLE present; not starting"
    exit 0
fi
if [ -e "$CRASHLOOP" ]; then
    alog "$CRASHLOOP present (earlier crash loop); not starting"
    exit 0
fi
if [ -e "$KILL_SWITCH_DEFAULT" ]; then
    alog "kill switch $KILL_SWITCH_DEFAULT present; not starting"
    exit 0
fi
fw=$(timeout 10 "$BIN" --config "$CFG" --check-firmware 2>&1)
rc=$?
if [ "$rc" -ne 0 ]; then
    alog "firmware check failed (status $rc): $fw; not starting"
    exit 0
fi
alog "$fw; starting"
sh "$PREFIX/bin/start.sh" --supervise >>"$PREFIX/log/autostart.log" 2>&1
alog "start.sh exited with status $?"
exit 0
