#!/bin/sh
# Boot-time start, launched detached by the init.d hook (init.d/webos-satellite).
# Waits, checks every "do not start" condition, applies a pending package
# update, then starts the supervised daemon. Writes DIR/log/autostart.log
# (plus what start.sh and an update write).
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
#   the app in DIR/app-dir was removed       (no appinfo.json there)
#   `--check-firmware` fails AND the config says `firmware_policy = "strict"`
#                                            (the old hard gate; with the
#                                             default "warn" policy an
#                                             unverified firmware is logged
#                                             and the daemon starts anyway:
#                                             owner decision 2026-11-04)
#
# Pending update: when the app's payload (DIR/app-dir/payload) differs from
# the installed one (SHA256SUMS against DIR/package.sums), its ctl.sh install
# runs first (it keeps config and key). A failed update keeps the old install.
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
    -h | --help) sed -n '2,29p' "$0"; exit 0 ;;
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
app=
[ ! -f "$PREFIX/app-dir" ] || read -r app <"$PREFIX/app-dir" || true
if [ -n "$app" ] && [ ! -f "$app/appinfo.json" ]; then
    alog "the app ($app) was removed; not starting (remove everything with: sh $PREFIX/bin/ctl.sh purge)"
    exit 0
fi
lock_take 120
trap lock_release EXIT
trap 'lock_release; exit 1' INT TERM HUP
if [ -n "$app" ] && [ -f "$app/payload/ctl.sh" ] && [ -f "$app/payload/SHA256SUMS" ] &&
    ! cmp -s "$app/payload/SHA256SUMS" "$PREFIX/package.sums"; then
    alog "package update pending ($(cat "$PREFIX/VERSION" 2>/dev/null || echo unknown) -> $(cat "$app/payload/VERSION" 2>/dev/null || echo unknown)); applying"
    if sh "$app/payload/ctl.sh" install >>"$PREFIX/log/autostart.log" 2>&1; then
        alog "update applied"
    else
        alog "update failed (status $?); starting the installed version"
    fi
fi
if command -v timeout >/dev/null 2>&1; then
    fw=$(timeout 10 "$BIN" --config "$CFG" --check-firmware 2>&1)
else
    fw=$("$BIN" --config "$CFG" --check-firmware 2>&1)
fi
rc=$?
policy=$(sed -n 's/^[[:space:]]*firmware_policy[[:space:]]*=[[:space:]]*"\{0,1\}\([a-z]*\).*/\1/p' "$CFG" 2>/dev/null | head -n 1)
if [ "$rc" -ne 0 ] && [ "$policy" = strict ]; then
    alog "firmware check failed (status $rc): $fw; firmware_policy = strict, not starting"
    exit 0
fi
if [ "$rc" -ne 0 ]; then
    alog "firmware check (status $rc): $fw; starting anyway (firmware_policy ${policy:-warn})"
else
    alog "$fw; starting"
fi
sh "$PREFIX/bin/start.sh" --supervise >>"$PREFIX/log/autostart.log" 2>&1
alog "start.sh exited with status $?"
exit 0
