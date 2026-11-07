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
# CTL is "sh <path>", split on purpose.
# shellcheck disable=SC2016,SC2034,SC1091,SC2086
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
WEBOS_SAT_SETUP_DIR=$T/setup
export WEBOS_SAT_PREFIX WEBOS_SAT_INITD WEBOS_SAT_LOCK WEBOS_SAT_KILL_SWITCH WEBOS_SAT_SETUP_DIR
export WEBOS_SAT_ALLOW_NONROOT=1 WEBOS_SAT_TEST_ANY_TRIPLE=1 WEBOS_SAT_UNAME_M=aarch64
CTL="sh $APP/payload/ctl.sh"
OV=$APPS/$ID.overlay
# A fake appinstalld behind luna-send-pub (-w MS -i URI PARAMS): dev/install
# unpacks the ipk into the fake root as the TV's installer lays it out and
# answers "installed"; dev/remove deletes the app directory. Each call is
# logged to $T/appinstall.calls.
cat >"$T/luna-send-pub" <<FAKEPUB
#!/bin/sh
uri=\$4
params=\$5
echo "\$uri \$params" >>"$T/appinstall.calls"
case \$uri in
*/dev/install)
    ipk=\$(echo "\$params" | sed -n 's/.*"ipkUrl":"\\([^"]*\\)".*/\\1/p')
    d=\$(mktemp -d "$T/fakeinst.XXXXXX")
    (cd "\$d" && ar x "\$ipk" && tar -xzf data.tar.gz -C "$T/root") && echo '{"details":{"state":"installed"},"statusValue":30}'
    rm -rf "\$d"
    ;;
*/dev/remove)
    id=\$(echo "\$params" | sed -n 's/.*"id":"\\([^"]*\\)".*/\\1/p')
    rm -rf "$APPS/\$id"
    echo '{"statusValue": 31}'
    ;;
esac
exec sleep 30
FAKEPUB
chmod 755 "$T/luna-send-pub"
export WEBOS_SAT_LUNA_SEND_PUB=$T/luna-send-pub
installs() { grep -c "/dev/install" "$T/appinstall.calls" 2>/dev/null || true; }
fails=0
cleanup() {
    pkill -f "$S/bin/autostart.sh" 2>/dev/null || true
    if [ -f "$S/bin/stop.sh" ]; then sh "$S/bin/stop.sh" >/dev/null 2>&1 || true; fi
    rm -rf "$T"
}
trap cleanup EXIT INT TERM
# The host debug build carries debug info and symbols that these checks never
# use; with them, each package build below compressed about 260 MB at gzip -9
# in Python (minutes per build). Package a stripped copy (same code, 131 MB
# down to 17 MB for 0.4.3); the full binary if objcopy is missing.
if command -v objcopy >/dev/null 2>&1 && objcopy --strip-all "$HOST_BIN" "$T/host-bin" 2>/dev/null; then
    HOST_BIN=$T/host-bin
fi

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
# Stand-in phone setup server: answers --version, otherwise writes a waiting
# state.json in WEBOS_SAT_SETUP_DIR and idles until the stop file appears (as
# the real one does).
mksetup() { # OUT TAG
    cat >"$1" <<SETUPEOF
#!/bin/sh
# test build $2
[ "\$1" != --version ] || { echo "webos-satellite-setup 0.0.1 (test)"; exit 0; }
D=\${WEBOS_SAT_SETUP_DIR:?}
printf '{"schema":1,"state":"waiting","reason":"","port":1,"path":"/s/x","expires_in_s":5,"peer":null,"peer_other_subnet":null,"pending":null,"last_result":null}\n' >"\$D/state.json"
while [ ! -e "\$D/stop" ]; do sleep 0.1; done
SETUPEOF
    chmod 755 "$1"
}
build() { # VERSION TAG OUTDIR
    mkbin "$T/a32-$2" "$2"
    mkbin "$T/a64-$2" "$2"
    mksetup "$T/s32-$2" "$2"
    mksetup "$T/s64-$2" "$2"
    SOURCE_DATE_EPOCH=1700000000 python3 "$BUILD" build --no-elf-check --version "$1" \
        --armv7 "$T/a32-$2" --aarch64 "$T/a64-$2" --setup-armv7 "$T/s32-$2" --setup-aarch64 "$T/s64-$2" \
        --out "$3" >/dev/null
}
# Number of live processes (this one excluded) whose command line, NULs read
# as spaces, contains (`in`) or starts with (`start`) TEXT, or whose
# executable is the path TEXT (`exe`). One python3 process instead of a tr
# and a grep (or a readlink) per pid: that shell loop cost about 20 s and
# 30 s of CPU per call on a machine with 1000 processes.
procs() { # in|start|exe TEXT
    python3 -c '
import os, sys
mode, text, n = sys.argv[1], sys.argv[2], 0
for p in os.listdir("/proc"):
    if not p.isdigit() or int(p) == os.getpid():
        continue
    try:
        if mode == "exe":
            n += os.readlink(f"/proc/{p}/exe") == text
            continue
        with open(f"/proc/{p}/cmdline", "rb") as f:
            c = f.read().replace(b"\0", b" ").decode("utf-8", "replace")
    except OSError:
        continue
    n += c.startswith(text) if mode == "start" else text in c
print(n)' "$1" "$2"
}
setup_servers() { # number of live processes of the installed setup server
    procs in "$S/bin/webos-satellite-setup"
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
    procs exe "$S/bin/webos-satellite"
}
supervisors() {
    procs start "sh $S/bin/supervise.sh"
}
running_pid() { read -r p <"$S/run/webos-satellite.pid" 2>/dev/null && echo "$p"; }

echo "== build and structure =="
build 0.0.1 one "$T/v1"
IPK1=$T/v1/${ID}_0.0.1_all.ipk
check "ipk built with the standard name" '[ -f "$IPK1" ] && [ -f "$T/v1/$ID.manifest.json" ]'
check "check: structure, control, appinfo, payload sums" 'python3 "$BUILD" check --no-elf-check --app-id $ID --version 0.0.1 "$IPK1" >/dev/null'
check "check: with --require-setup the package passes; the setup builds are in SHA256SUMS" 'python3 "$BUILD" check --no-elf-check --require-setup "$IPK1" >/dev/null && python3 -c "
import sys
sys.path.insert(0, sys.argv[1])
import build_ipk as b
m = dict(b.ar_members(open(sys.argv[2], \"rb\").read()))
d = b.untar(m[\"data.tar.gz\"])
base = \"usr/palm/applications/\" + sys.argv[3] + \"/payload/\"
sums = d[base + \"SHA256SUMS\"][1].decode()
names = (\"webos-satellite-setup-armv7\", \"webos-satellite-setup-aarch64\")
sys.exit(0 if all(base + n in d and d[base + n][0].mode == 0o755 and n in sums for n in names) else 1)" "$PKG/ipk" "$IPK1" "$ID"'
SOURCE_DATE_EPOCH=1700000000 python3 "$BUILD" build --no-elf-check --version 0.0.1 \
    --armv7 "$T/a32-one" --aarch64 "$T/a64-one" --out "$T/nosetup" >/dev/null
check "check: a package without the setup server passes, and is refused with --require-setup" 'python3 "$BUILD" check --no-elf-check "$T/nosetup/${ID}_0.0.1_all.ipk" >/dev/null && ! python3 "$BUILD" check --no-elf-check --require-setup "$T/nosetup/${ID}_0.0.1_all.ipk" >/dev/null 2>&1'
check "build: only one setup server build is refused" '! python3 "$BUILD" build --no-elf-check --version 0.0.1 --armv7 "$T/a32-one" --aarch64 "$T/a64-one" --setup-armv7 "$T/s32-one" --out "$T/bad" >/dev/null 2>&1'
check "check: a wrong expected version is refused" '! python3 "$BUILD" check --no-elf-check --version 9.9.9 "$IPK1" >/dev/null 2>&1'
check "check: host binaries are refused as TV builds without --no-elf-check" '! python3 "$BUILD" check "$IPK1" >/dev/null 2>&1'
SOURCE_DATE_EPOCH=1700000000 python3 "$BUILD" build --no-elf-check --version 0.0.1 \
    --armv7 "$T/a32-one" --aarch64 "$T/a64-one" --setup-armv7 "$T/s32-one" --setup-aarch64 "$T/s64-one" \
    --out "$T/v1b" >/dev/null
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
check "appinfo.json sets disableBackHistoryAPI true (the app handles Back itself; the runtime would close it otherwise)" 'python3 -c "
import json, sys
sys.exit(0 if json.load(open(sys.argv[1]))[\"disableBackHistoryAPI\"] is True else 1)" "$APP/appinfo.json"'
check "status before install: valid JSON, not installed" 'json "s[\"installed\"] is False and s[\"package_version\"]==\"0.0.1\""'
$CTL install >"$T/out" 2>&1
check "install: daemon, scripts, hook template, version, app-dir recorded" '[ -x "$S/bin/webos-satellite" ] && [ -f "$S/bin/ctl.sh" ] && [ -f "$S/bin/webos-satellite.hook" ] && [ "$(cat "$S/VERSION")" = 0.0.1 ] && [ "$(cat "$S/app-dir")" = "$APP" ]'
check "install: generated config with a random name and MAC seed, mode 0600" 'grep -Eq "^name = \"webos-satellite-[0-9a-f]{6}\"" "$S/etc/config.toml" && grep -Eq "^mac_seed = \"[0-9a-f]{32}\"" "$S/etc/config.toml" && [ "$(stat -c %a "$S/etc/config.toml")" = 600 ]'
check "install: the generated config passes --check-config" 'grep -q "config is valid" "$T/out"'
check "install: no boot hook unless asked (opt-in)" '[ ! -e "$T/initd/webos-satellite" ]'
check "install: does not start the daemon" '! json "s[\"running\"]"'
check "install: the setup server comes from the aarch64 build (uname aarch64), mode 0700, answers --version" 'cmp -s "$S/bin/webos-satellite-setup" "$T/s64-one" && [ "$(stat -c %a "$S/bin/webos-satellite-setup")" = 700 ] && "$S/bin/webos-satellite-setup" --version | grep -q "^webos-satellite-setup "'
check "install: no temporary setup file, no session directory, no setup process" '[ ! -e "$S/bin/.webos-satellite-setup.new" ] && [ ! -e "$T/setup" ] && [ "$(setup_servers)" = 0 ]'
mac1=$(grep '^mac_seed' "$S/etc/config.toml")
check "install: the overlay app is installed through appinstalld (its own package, overlay window), status says so" '[ "$(installs)" = 1 ] && python3 -c "
import json, sys
a = json.load(open(sys.argv[1]))
sys.exit(0 if a[\"id\"] == sys.argv[2] and a[\"defaultWindowType\"] == \"overlay\" and a[\"transparent\"] is True else 1)" "$OV/appinfo.json" "$ID.overlay" && [ -f "$OV/overlay.js" ] && json "s[\"overlay_app\"] is True and s[\"overlay_enabled\"] is True" && grep -q "overlay app $ID.overlay installed" "$T/out"'
check "install: the overlay ipk was a copy (the payload keeps its own, no copy left in TMPDIR)" '[ -f "$APP/payload/overlay.ipk" ] && ! ls "${TMPDIR:-/tmp}"/webos-satellite-overlay.*.ipk >/dev/null 2>&1'

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
# As the Homebrew Channel's exec runs it: output read through a pipe until
# it closes, and descriptors of the caller (its Luna sockets) inherited.
exec 7>"$T/inherited"
t0=$(date +%s)
out=$(timeout 20 $CTL start 2>&1) && rc=0 || rc=$?
exec 7>&-
check "start with captured output returns at once (HBC exec waits for the pipe)" '[ "$rc" -eq 0 ] && [ $(($(date +%s) - t0)) -le 5 ] && echo "$out" | grep -q "started pid"'
leaks=0
for p in $(pgrep -f "$S/bin/"); do
    for f in /proc/"$p"/fd/*; do
        [ "$(readlink "$f" 2>/dev/null)" != "$T/inherited" ] || leaks=$((leaks + 1))
    done
done
check "daemon, supervisor and rotator keep no descriptor of the caller" '[ "$leaks" -eq 0 ] && [ -n "$(pgrep -f "$S/bin/supervise.sh")" ]'
check "no start.sh process stays behind; the supervisor pidfile names supervise.sh" '[ -z "$(pgrep -f "$S/bin/start.sh")" ] && tr "\0" " " </proc/$(cat "$S/run/supervise.pid")/cmdline | grep -q supervise.sh'
check "start: one supervised daemon" 'json "s[\"running\"] and s[\"supervised\"] and s[\"firmware_ok\"]" && [ "$(daemons)" = 1 ] && [ "$(supervisors)" = 1 ]'
check "start: the phone setup server is not started (only by ctl.sh setup start)" '[ "$(setup_servers)" = 0 ] && [ ! -e "$T/setup" ]'
check "status --json: firmware_ok means may start, firmware_verified and compat are present" 'json "s[\"firmware_ok\"] is True and s[\"firmware_verified\"] is True and \"compat\" in s"'
check "status --json: no key material" '! $CTL status --json | grep -q "$(cat "$T/psk")"'
check "status text: reply volume names the default, never 'default %'" '! $CTL status | grep -q "default %" && $CTL status | grep -q "reply volume: .*none stored yet" && json "\"reply_volume_default\" in s and s[\"reply_volume\"]==\"\""'
printf '55\n' >"$S/etc/reply_volume"
printf 'keep_listening\n' >"$S/etc/remote_off"
check "status text and JSON: a stored reply volume wins" '$CTL status | grep -q "reply volume: 55 %," && json "s[\"reply_volume\"]==\"55\" and s[\"remote_off\"]==\"keep_listening\""'
rm -f "$S/etc/reply_volume" "$S/etc/remote_off"
$CTL start >/dev/null
check "a second start does nothing" '[ "$(daemons)" = 1 ] && [ "$(supervisors)" = 1 ]'
# stop_daemon under `set -e` when every TERM finds its target already gone
# (the supervisor forwarded the TERM and the daemon exited first).
# A separate `sh -e`: inside `$(...) || true` errexit would be off.
out=$(sh -ec '
    . "$1/lib.sh"
    set_paths "$2"
    kill() { command kill "$@"; [ "$1" != -TERM ] || return 1; }
    stop_daemon 10
    echo stop-completed
' sh "$PKG" "$S" 2>&1) || true
check "stop under set -e survives a daemon that already exited (race on the TV)" 'echo "$out" | grep -q stop-completed && [ "$(daemons)" = 0 ] && [ "$(supervisors)" = 0 ]'
$CTL start >/dev/null
$CTL mic off >/dev/null
check "mic off: kill switch file, status reports it" '[ -e "$T/kill" ] && json "s[\"mic_off\"]"'
$CTL mic on >/dev/null
check "mic on: kill switch removed" '[ ! -e "$T/kill" ]'
echo "== compatibility status, report, triggers =="
# The daemon writes run/compat.json and run/report.* by itself.
check "daemon: run/compat.json and the report files appear" 'wait_for 20 "[ -f \"$S/run/compat.json\" ] && [ -f \"$S/run/report.json\" ] && [ -f \"$S/run/report.url\" ]"'
check "status --json: compat is the daemon's object with a status and a self-check text" 'json "isinstance(s[\"compat\"], dict) and s[\"compat\"][\"compat_status\"] and \"selfcheck\" in s[\"compat\"] and \"firmware_changed\" in s[\"compat\"]"'
check "status text: a compatibility line" '$CTL status | grep -q "^webos-satellite: compatibility: "'
$CTL report >"$T/report.txt" 2>&1
check "report: short text, the GitHub issue link, no hand-off to anything else" 'grep -q "^webos-satellite compatibility report v1" "$T/report.txt" && grep -q "^Create the GitHub issue" "$T/report.txt" && grep -q "^https://github.com/pando85/webos-voice-satellite/issues/new?template=compatibility-report.yml&" "$T/report.txt"'
$CTL report --full >"$T/report-full.txt" 2>&1
$CTL report --json >"$T/report.json" 2>&1
check "report --full and --json: the text, and JSON the app can show (url, full)" 'head -n 1 "$T/report-full.txt" | grep -q "compatibility report v1" && python3 -c "
import json,sys
r=json.load(open(sys.argv[1]))
sys.exit(0 if r[\"schema\"]==1 and r[\"url\"].startswith(\"https://github.com/\") and r[\"full\"].startswith(\"webos-satellite compatibility report\") and len(r[\"url\"])<=1500 else 1)" "$T/report.json"'
printf '%s\n' pkg-test keep-me 127.0.0.1 "$(cat "$T/psk")" "$S" >"$T/secrets"
check "report: nothing identifying (device name, MAC seed, address, key, install path)" '! cat "$T/report.txt" "$T/report-full.txt" "$T/report.json" | grep -q -F -f "$T/secrets"'
$CTL selfcheck >"$T/out" 2>&1
check "selfcheck: trigger file written, consumed by the running daemon" 'grep -q "self-check requested" "$T/out" && wait_for 6 "[ ! -e \"$S/run/selfcheck-run\" ]"'
$CTL ack-firmware >"$T/out" 2>&1
check "ack-firmware: trigger consumed by the daemon, etc/firmware_seen holds the firmware" 'grep -q "acknowledged" "$T/out" && wait_for 6 "[ ! -e \"$S/run/firmware-ack\" ]" && [ "$(cat "$S/etc/firmware_seen")" = 99.99.99 ]'
# A firmware that differs from the remembered one is shown until acknowledged.
$CTL stop >/dev/null
echo 01.00.00 >"$S/etc/firmware_seen"
rm -f "$S/run/compat.json" "$S/run/report.json"
$CTL start >/dev/null
check "firmware changed since the last run: compat says so, the microphone is not blocked" 'wait_for 20 "[ -f \"$S/run/compat.json\" ]" && json "s[\"compat\"][\"firmware_changed\"] is True and s[\"compat\"][\"compat_status\"]==\"firmware changed\" and s[\"firmware_ok\"]"'
$CTL ack-firmware >/dev/null
fw_changed_false() { grep -Eq '"firmware_changed":[[:space:]]*false' "$S/run/compat.json"; }
check "ack-firmware clears it" 'wait_for 8 fw_changed_false && [ "$(cat "$S/etc/firmware_seen")" = 99.99.99 ]'
$CTL stop >/dev/null
check "report with the daemon stopped: the last saved report, with a note" '$CTL report 2>&1 | grep -q "last saved report"'
echo 01.00.00 >"$S/etc/firmware_seen"
WEBOS_SAT_OS_INFO=$T/os_info.json $CTL ack-firmware >"$T/out" 2>&1
check "ack-firmware with the daemon stopped: writes the current firmware" 'grep -q "99.99.99 recorded" "$T/out" && [ "$(cat "$S/etc/firmware_seen")" = 99.99.99 ]'
$CTL selfcheck >"$T/out" 2>&1
check "selfcheck with the daemon stopped: says it runs at the next start" 'grep -q "next start" "$T/out" && [ ! -e "$S/run/selfcheck-run" ]'
rm -f "$S/run/report.json" "$S/run/report.short.txt" "$S/run/report.full.txt" "$S/run/report.url" "$S/run/compat.json"
$CTL report >"$T/report.txt" 2>&1
check "report with nothing saved: a static report from the binary" 'grep -q "^Create the GitHub issue" "$T/report.txt"'
$CTL report --bogus >/dev/null 2>&1 && rc=0 || rc=$?
check "report with an unknown flag: usage error (exit 2)" '[ "$rc" -eq 2 ]'
$CTL start >/dev/null
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
$CTL setup start >"$T/out" 2>&1
check "setup start from the installed package: a session is up, JSON waiting" '[ "$(setup_servers)" = 1 ] && $CTL setup status --json | grep -q "\"state\":\"waiting\""'
$CTL install >"$T/out" 2>&1
pid_v2=$(running_pid)
check "update: ends a running phone setup session, installs the new setup server build" '[ "$(setup_servers)" = 0 ] && [ ! -e "$T/setup" ] && cmp -s "$S/bin/webos-satellite-setup" "$T/s64-two"' 
check "update: restarted with the new build (new pid, one daemon, one supervisor)" '[ -n "$pid_v2" ] && [ "$pid_v2" != "$pid_v1" ] && ! kill -0 "$pid_v1" 2>/dev/null && [ "$(daemons)" = 1 ] && [ "$(supervisors)" = 1 ]'
check "update: installed binary is the new one" 'cmp -s "$S/bin/webos-satellite" "$T/a64-two" && [ "$(cat "$S/VERSION")" = 0.0.2 ] && ! json "s[\"update_pending\"]"'
check "update: config and key unchanged" '[ "$(sha256sum <"$S/etc/config.toml")" = "$cfg_sum" ] && [ "$(sha256sum <"$S/etc/noise_psk")" = "$psk_sum" ]'
check "update: boot hook refreshed and still on" 'grep -q "^PREFIX=$S\$" "$H"'
check "update: the changed overlay app is installed again" '[ "$(installs)" = 2 ] && grep -q "\"version\": \"0.0.2\"" "$OV/appinfo.json"'
out=$($CTL install 2>&1)
check "a second install of the same payload does nothing (no restart, no overlay install)" 'echo "$out" | grep -q "already up to date" && [ "$(running_pid)" = "$pid_v2" ] && [ "$(installs)" = 2 ]'
rm -rf "$OV"
out=$($CTL install 2>&1)
check "an overlay app removed by hand comes back at the next install, without a restart" '[ "$(installs)" = 3 ] && [ -f "$OV/appinfo.json" ] && [ "$(running_pid)" = "$pid_v2" ]'
rm -rf "$OV"
out=$(WEBOS_SAT_LUNA_SEND_PUB=$T/no-such-tool $CTL install 2>&1) && rc=0 || rc=$?
check "no luna-send-pub: install still succeeds, the overlay app is reported missing" '[ "$rc" -eq 0 ] && echo "$out" | grep -q "overlay app: not installed (no $T/no-such-tool)" && json "s[\"overlay_app\"] is False"'
$CTL install >/dev/null 2>&1
$CTL install --force >/dev/null
check "install --force reinstalls and restarts" '[ -n "$(running_pid)" ] && [ "$(running_pid)" != "$pid_v2" ] && [ "$(daemons)" = 1 ]'

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
$CTL setup start >/dev/null 2>&1
$CTL uninstall >/dev/null
check "uninstall: hook gone, daemon and setup session stopped, binaries removed" '[ ! -e "$H" ] && [ "$(daemons)" = 0 ] && [ ! -e "$S/bin" ] && [ "$(setup_servers)" = 0 ] && [ ! -e "$T/setup" ]'
check "uninstall: the overlay app is removed through appinstalld" '[ ! -e "$OV" ] && grep -q "/dev/remove {\"id\":\"$ID.overlay\"" "$T/appinstall.calls"'
check "uninstall: config, key and logs kept" '[ "$(sha256sum <"$S/etc/config.toml")" = "$cfg_sum" ] && [ -f "$S/etc/noise_psk" ] && [ -f "$S/log/webos-satellite.log" ]'
check "status after uninstall: not installed" 'json "s[\"installed\"] is False"'
$CTL install >/dev/null
check "reinstall: keeps the config (same identity) and key" '[ "$(sha256sum <"$S/etc/config.toml")" = "$cfg_sum" ] && grep -q "keep-me" "$S/etc/config.toml" && [ "$(sha256sum <"$S/etc/noise_psk")" = "$psk_sum" ]'
$CTL start >/dev/null
$CTL autostart on >/dev/null
$CTL setup start >/dev/null 2>&1
$CTL purge >/dev/null
check "purge: everything removed (install directory, hook, setup session), nothing running" '[ ! -e "$S" ] && [ ! -e "$H" ] && [ "$(daemons)" = 0 ] && [ "$(setup_servers)" = 0 ] && [ ! -e "$T/setup" ]'
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
