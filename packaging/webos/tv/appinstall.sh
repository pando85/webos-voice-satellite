#!/bin/sh
# Runs ON THE TV, as root, piped through ssh (nothing is copied):
#   ssh root@TV sh -s -- install /tmp/<id>_<version>_all.ipk < packaging/webos/tv/appinstall.sh
#   ssh root@TV sh -s -- remove <id> < packaging/webos/tv/appinstall.sh
#
# install: hands the ipk to the TV's installer (appinstalld) with
#   luna://com.webos.appInstallService/dev/install, as the Homebrew Channel's
#   README and tools/install.sh do, and waits for "installed" (what that
#   script waits for) or statusValue 30 (what its service waits for).
# remove: luna://com.webos.appInstallService/dev/remove, as the Homebrew
#   Channel's uninstall method does; done at statusValue 31. Removing the app
#   runs no script of ours: run the app's `payload/ctl.sh uninstall` or
#   `purge` first.
set -eu
[ $# -eq 2 ] || { echo "usage: appinstall.sh install IPK | remove APP_ID" >&2; exit 2; }
case $2 in
*[!A-Za-z0-9._/+-]*) echo "unsupported characters in $2" >&2; exit 2 ;;
esac
case $1 in
install)
    [ -f "$2" ] || { echo "no such file: $2" >&2; exit 1; }
    uri=luna://com.webos.appInstallService/dev/install
    params="{\"id\":\"com.ares.defaultName\",\"ipkUrl\":\"$2\",\"subscribe\":true}"
    ok='installed|"statusValue": ?30[^0-9]'
    ;;
remove)
    uri=luna://com.webos.appInstallService/dev/remove
    params="{\"id\":\"$2\",\"subscribe\":true}"
    ok='"statusValue": ?31[^0-9]'
    ;;
*) echo "usage: appinstall.sh install IPK | remove APP_ID" >&2; exit 2 ;;
esac
fifo=/tmp/webos-satellite-appinstall.$$
rm -f "$fifo"
mkfifo "$fifo"
# stdin from /dev/null: under `sh -s` stdin is this script.
luna-send-pub -w 60000 -i "$uri" "$params" </dev/null >"$fifo" &
lp=$!
result=$(grep -m 1 -E -e "$ok" -e 'failed|"returnValue": ?false|"errorCode"' "$fifo") || result=
kill -TERM "$lp" 2>/dev/null || true
rm -f "$fifo"
if [ -z "$result" ]; then
    echo "appinstall: no answer within 60 s" >&2
    exit 1
elif echo "$result" | grep -Eq 'failed|"returnValue": ?false|"errorCode"'; then
    echo "appinstall: $1 failed: $result" >&2
    exit 1
fi
echo "appinstall: $1 done"
