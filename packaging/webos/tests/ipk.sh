#!/bin/sh
# Host test of the webOS app package: builds .ipk files with the host build
# of webos-satellite standing in for both TV builds, checks their structure
# and reproducibility, unpacks them the way the TV's installer lays them out
# (usr/palm/applications/<id>/), and drives the package's ctl.sh through
# first install, start, boot hook, update (by ctl.sh and at boot), app
# removal, uninstall and purge. Everything stays in a temporary directory
# (install directory, init.d, lock, kill switch); the daemon listens on
# 127.0.0.1 with a file microphone and the null sink.
#
# usage: packaging/webos/tests/ipk.sh [BINARY]   (default: builds it)
# The checks are strings run with eval, so single quotes are intended.
# shellcheck disable=SC2016,SC2034,SC1091
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
PKG=$(cd "$HERE/.." && pwd)
ROOT=$(cd "$PKG/../.." && pwd)
BUILD=$PKG/ipk/build_ipk.py
HOST_BIN=${1:-}
if [ -z "$HOST_BIN" ]; then
    (cd "$ROOT" && cargo build --quiet -p webos-satellite)
    HOST_BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug/webos-satellite
fi
[ -x "$HOST_BIN" ] || { echo "binary not found: $HOST_BIN" >&2; exit 2; }

T=$(mktemp -d "${TMPDIR:-/tmp}/webos-sat-ipktest.XXXXXX")
ID=io.github.pando85.voicesatellite
S=$T/state
APPS=$T/root/usr/palm/applications
APP=$APPS/$ID
WEBOS_SAT_PREFIX=$S
WEBOS_SAT_INITD=$T/initd
WEBOS_SAT_LOCK=$T/lock
WEBOS_SAT_KILL_SWITCH=$T/kill
export WEBOS_SAT_PREFIX WEBOS_SAT_INITD WEBOS_SAT_LOCK WEBOS_SAT_KILL_SWITCH
export WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_TEST_ANY_TRIPLE=1 WEBOS_SAT_UNAME_M=aarch64
CTL="sh $APP/payload/ctl.sh"
fails=0
cleanup() {
    pkill -f "$S/bin/autostart.sh" 2>/dev/null || true
    if [ -f "$S/bin/stop.sh" ]; then sh "$S/bin/stop.sh" >/dev/null 2>&1 || true; fi
    rm -rf "$T"
}
trap cleanup EXIT INT TERM

check() {
    if eval "$2"; then
        echo "PASS  $1"
    else
        echo "FAIL  $1"
        fails=$((fails + 1))
    fi
}

wait_for() { # SECONDS CONDITION
    i=0
    while [ "$i" -lt "$(($1 * 10))" ]; do
        eval "$2" && return 0
        sleep 0.1
        i=$((i + 1))
    done
    return 1
}

# Binaries: the host build, plus a trailing marker so each "release" has
# different bytes (trailing data does not change how an ELF runs).
mkbin() { # OUT TAG
    cp "$HOST_BIN" "$1"
    printf '\n# test build %s\n' "$2" >>"$1"
    chmod 755 "$1"
}
build() { # VERSION TAG OUTDIR
    mkbin "$T/a32-$2" "$2"
    mkbin "$T/a64-$2" "$2"
    SOURCE_DATE_EPOCH=1700000000 python3 "$BUILD" build --no-elf-check --version "$1" \
        --armv7 "$T/a32-$2" --aarch64 "$T/a64-$2" --out "$3" >/dev/null
}
# Lays the package out as the TV's installer does: the app directory is
# replaced as a whole on every install.
unpack() { # IPK
    rm -rf "$APP"
    mkdir -p "$T/root"
    tmp=$T/unpack
    rm -rf "$tmp"
    mkdir -p "$tmp"
    (cd "$tmp" && ar x "$1" && tar -xzf data.tar.gz -C "$T/root")
}
json() { # PYTHON_EXPR on the status JSON as `s`
    $CTL status --json | python3 -c "import json,sys; s=json.load(sys.stdin); sys.exit(0 if ($1) else 1)"
}
daemons() { # number of live processes running the installed binary
    n=0
    for d in /proc/[0-9]*; do
        [ "$(readlink "$d/exe" 2>/dev/null)" != "$S/bin/webos-satellite" ] || n=$((n + 1))
    done
    echo "$n"
}
supervisors() {
    n=0
    for d in /proc/[0-9]*; do
        { tr '\0' ' ' <"$d/cmdline"; } 2>/dev/null | grep -q "^sh $S/bin/supervise.sh" && n=$((n + 1))
    done
    echo "$n"
}
running_pid() { read -r p <"$S/run/webos-satellite.pid" 2>/dev/null && echo "$p"; }

echo "== build and structure =="
build 0.0.1 one "$T/v1"
IPK1=$T/v1/${ID}_0.0.1_all.ipk
check "ipk built with the standard name" '[ -f "$IPK1" ] && [ -f "$T/v1/$ID.manifest.json" ]'
check "check: structure, control, appinfo, payload sums" 'python3 "$BUILD" check --no-elf-check --app-id $ID --version 0.0.1 "$IPK1" >/dev/null'
check "check: a wrong expected version is refused" '! python3 "$BUILD" check --no-elf-check --version 9.9.9 "$IPK1" >/dev/null 2>&1'
check "check: host binaries are refused as TV builds without --no-elf-check" '! python3 "$BUILD" check "$IPK1" >/dev/null 2>&1'
SOURCE_DATE_EPOCH=1700000000 python3 "$BUILD" build --no-elf-check --version 0.0.1 \
    --armv7 "$T/a32-one" --aarch64 "$T/a64-one" --out "$T/v1b" >/dev/null
check "reproducible: a second build is byte-identical" 'cmp -s "$IPK1" "$T/v1b/${ID}_0.0.1_all.ipk"'
check "manifest: HBC fields and the ipk hash" 'python3 -c "
import json,hashlib,sys
m=json.load(open(sys.argv[1])); h=hashlib.sha256(open(sys.argv[2],\"rb\").read()).hexdigest()
need={\"id\",\"version\",\"type\",\"title\",\"iconUri\",\"ipkUrl\",\"ipkHash\"}
sys.exit(0 if need<=set(m) and m[\"ipkHash\"][\"sha256\"]==h and m[\"rootRequired\"] is True else 1)" "$T/v1/$ID.manifest.json" "$IPK1"'
check "invalid app id refused" '! python3 "$BUILD" build --no-elf-check --app-id com.webos.x --version 0.0.1 --armv7 "$T/a32-one" --aarch64 "$T/a64-one" --out "$T/bad" >/dev/null 2>&1'
check "invalid version refused" '! python3 "$BUILD" build --no-elf-check --version 1.02.0 --armv7 "$T/a32-one" --aarch64 "$T/a64-one" --out "$T/bad" >/dev/null 2>&1'

echo "== first install from the unpacked app =="
unpack "$IPK1"
check "status before install: valid JSON, not installed" 'json "s[\"installed\"] is False and s[\"package_version\"]==\"0.0.1\""'
$CTL install >"$T/out" 2>&1
check "install: daemon, scripts, hook template, version, app-dir recorded" '[ -x "$S/bin/webos-satellite" ] && [ -f "$S/bin/ctl.sh" ] && [ -f "$S/bin/webos-satellite.hook" ] && [ "$(cat "$S/VERSION")" = 0.0.1 ] && [ "$(cat "$S/app-dir")" = "$APP" ]'
check "install: generated config with a random name and MAC seed, mode 0600" 'grep -Eq "^name = \"webos-satellite-[0-9a-f]{6}\"" "$S/etc/config.toml" && grep -Eq "^mac_seed = \"[0-9a-f]{32}\"" "$S/etc/config.toml" && [ "$(stat -c %a "$S/etc/config.toml")" = 600 ]'
check "install: the generated config passes --check-config" 'grep -q "config is valid" "$T/out"'
check "install: no boot hook unless asked (opt-in)" '[ ! -e "$T/initd/webos-satellite" ]'
check "install: does not start the daemon" '! json "s[\"running\"]"'
mac1=$(grep '^mac_seed' "$S/etc/config.toml")

# A host-runnable config (file mic, null sink, plaintext on 127.0.0.1) and a
# key file, as an owner would set them with --config and --psk-file.
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
head -c 96000 /dev/zero >"$T/mic.raw"
printf '{\n  "webos_manufacturing_version": "99.99.99"\n}\n' >"$T/os_info.json"
cat >"$T/sat.toml" <<EOF
mode = "wake"
[device]
name = "pkg-test"
bind = "127.0.0.1"
port = $port
allow_plaintext = true
mac_seed = "keep-me"
[mic]
source = "file"
path = "$T/mic.raw"
loop = true
[sink]
type = "null"
[safety]
kill_switch = "$T/kill"
mute_file = "muted"
validated_firmware = ["99.99.99"]
firmware_info = "$T/os_info.json"
EOF
head -c 32 /dev/urandom | base64 >"$T/psk"
$CTL install --config "$T/sat.toml" --psk-file "$T/psk" >/dev/null
check "install --config/--psk-file: replaced config, key mode 0600" 'grep -q "pkg-test" "$S/etc/config.toml" && cmp -s "$T/psk" "$S/etc/noise_psk" && [ "$(stat -c %a "$S/etc/noise_psk")" = 600 ]'
cfg_sum=$(sha256sum <"$S/etc/config.toml")
psk_sum=$(sha256sum <"$S/etc/noise_psk")

echo "== start, status, boot hook =="
$CTL start >/dev/null
check "start: one supervised daemon" 'json "s[\"running\"] and s[\"supervised\"] and s[\"firmware_ok\"]" && [ "$(daemons)" = 1 ] && [ "$(supervisors)" = 1 ]'
check "status --json: no key material" '! $CTL status --json | grep -q "$(cat "$T/psk")"'
$CTL start >/dev/null
check "a second start does nothing" '[ "$(daemons)" = 1 ] && [ "$(supervisors)" = 1 ]'
$CTL mic off >/dev/null
check "mic off: kill switch file, status reports it" '[ -e "$T/kill" ] && json "s[\"mic_off\"]"'
$CTL mic on >/dev/null
check "mic on: kill switch removed" '[ ! -e "$T/kill" ]'
$CTL autostart on >/dev/null
H=$T/initd/webos-satellite
check "autostart on: hook 0755 with the install directory filled in" '[ "$(stat -c %a "$H")" = 755 ] && grep -q "^PREFIX=$S\$" "$H" && ! grep -q @PREFIX@ "$H" && sh -n "$H" && json "s[\"autostart\"]"'
t0=$(date +%s)
out=$(sh "$H" 2>&1) && rc=0 || rc=$?
check "hook: exit 0 at once, no output, autostart.sh detached" '[ "$rc" -eq 0 ] && [ -z "$out" ] && [ $(($(date +%s) - t0)) -le 1 ] && wait_for 2 "pgrep -f \"$S/bin/autostart.sh\" >/dev/null"'
pkill -f "$S/bin/autostart.sh" || true

echo "== update by ctl.sh (app opened, or by hand) =="
pid_v1=$(running_pid)
build 0.0.2 two "$T/v2"
unpack "$T/v2/${ID}_0.0.2_all.ipk"
check "new package unpacked: update pending, old daemon still running" 'json "s[\"update_pending\"] and s[\"installed_version\"]==\"0.0.1\" and s[\"package_version\"]==\"0.0.2\" and s[\"running\"]"'
$CTL install >"$T/out" 2>&1
pid_v2=$(running_pid)
check "update: restarted with the new build (new pid, one daemon, one supervisor)" '[ -n "$pid_v2" ] && [ "$pid_v2" != "$pid_v1" ] && ! kill -0 "$pid_v1" 2>/dev/null && [ "$(daemons)" = 1 ] && [ "$(supervisors)" = 1 ]'
check "update: installed binary is the new one" 'cmp -s "$S/bin/webos-satellite" "$T/a64-two" && [ "$(cat "$S/VERSION")" = 0.0.2 ] && ! json "s[\"update_pending\"]"'
check "update: config and key unchanged" '[ "$(sha256sum <"$S/etc/config.toml")" = "$cfg_sum" ] && [ "$(sha256sum <"$S/etc/noise_psk")" = "$psk_sum" ]'
check "update: boot hook refreshed and still on" 'grep -q "^PREFIX=$S\$" "$H"'

echo "== update at boot (autostart.sh) =="
$CTL stop >/dev/null
check "stop: nothing running" '[ "$(daemons)" = 0 ] && [ "$(supervisors)" = 0 ]'
build 0.0.3 three "$T/v3"
unpack "$T/v3/${ID}_0.0.3_all.ipk"
sh "$S/bin/autostart.sh" --delay 0
check "autostart: applied the pending update, then started it" 'grep -q "package update pending (0.0.2 -> 0.0.3); applying" "$S/log/autostart.log" && grep -q "update applied" "$S/log/autostart.log" && [ "$(cat "$S/VERSION")" = 0.0.3 ] && json "s[\"running\"]" && [ "$(daemons)" = 1 ]'
check "autostart: config and key unchanged" '[ "$(sha256sum <"$S/etc/config.toml")" = "$cfg_sum" ] && [ "$(sha256sum <"$S/etc/noise_psk")" = "$psk_sum" ]'
$CTL stop >/dev/null

echo "== concurrency and the lock =="
$CTL start >/dev/null 2>&1 &
a=$!
$CTL start >/dev/null 2>&1 &
b=$!
wait "$a" || true
wait "$b" || true
check "two concurrent starts: one daemon, one supervisor" '[ "$(daemons)" = 1 ] && [ "$(supervisors)" = 1 ]'
$CTL stop >/dev/null
mkdir "$T/lock"
echo 999999 >"$T/lock/pid"
check "a stale lock (dead owner) is taken over" '$CTL start >/dev/null && [ ! -e "$T/lock" ] && [ "$(daemons)" = 1 ]'
$CTL stop >/dev/null

echo "== app removed without uninstall =="
mv "$APP" "$T/app-away"
sh "$S/bin/autostart.sh" --delay 0
check "autostart: does not start when the app is gone" 'grep -q "was removed; not starting" "$S/log/autostart.log" && [ "$(daemons)" = 0 ]'
sh "$H"
check "hook: launches nothing when the app is gone" '! wait_for 1 "pgrep -f \"$S/bin/autostart.sh\" >/dev/null"'
mv "$T/app-away" "$APP"

echo "== uninstall (keep data), reinstall, purge =="
$CTL start >/dev/null
$CTL uninstall >/dev/null
check "uninstall: hook gone, daemon stopped and removed" '[ ! -e "$H" ] && [ "$(daemons)" = 0 ] && [ ! -e "$S/bin" ]'
check "uninstall: config, key and logs kept" '[ "$(sha256sum <"$S/etc/config.toml")" = "$cfg_sum" ] && [ -f "$S/etc/noise_psk" ] && [ -f "$S/log/webos-satellite.log" ]'
check "status after uninstall: not installed" 'json "s[\"installed\"] is False"'
$CTL install >/dev/null
check "reinstall: keeps the config (same identity) and key" '[ "$(sha256sum <"$S/etc/config.toml")" = "$cfg_sum" ] && grep -q "keep-me" "$S/etc/config.toml" && [ "$(sha256sum <"$S/etc/noise_psk")" = "$psk_sum" ]'
$CTL start >/dev/null
$CTL autostart on >/dev/null
$CTL purge >/dev/null
check "purge: everything removed (install directory, hook), nothing running" '[ ! -e "$S" ] && [ ! -e "$H" ] && [ "$(daemons)" = 0 ]'
check "purge: unrelated directory is refused" 'mkdir -p "$S" && : >"$S/x" && ! $CTL purge >/dev/null 2>&1 && [ -e "$S/x" ]'
rm -rf "$S"

echo "== identity and root =="
$CTL install >/dev/null
name1=$(grep '^name' "$S/etc/config.toml")
$CTL purge >/dev/null
$CTL install >/dev/null
name2=$(grep '^name' "$S/etc/config.toml")
check "two fresh installs get different names and MAC seeds" '[ "$name1" != "$name2" ] && [ "$mac1" != "$(grep '"'"'^mac_seed'"'"' "$S/etc/config.toml")" ]'
$CTL purge >/dev/null
if [ "$(id -u)" != 0 ]; then
    out=$(WEBOS_SAT_ALLOW_NONROOT='' $CTL install 2>&1) && rc=0 || rc=$?
    check "without root: install refused with a clear message" '[ "$rc" -ne 0 ] && echo "$out" | grep -q "needs root" && [ ! -e "$S" ]'
fi

if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) failed"
    exit 1
fi
