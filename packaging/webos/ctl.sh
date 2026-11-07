#!/bin/sh
# Control entry point of the webOS app package. The app calls it through the
# Homebrew Channel's root `exec` with fixed arguments only; it also works by
# hand over SSH. Design: docs/design/packaging.md.
#
# usage: ctl.sh COMMAND
#
#   status [--json]   state of the package, the install and the daemon
#                     (never prints the key or other secrets)
#   install [--config FILE] [--psk-file FILE] [--arch auto|armv7|aarch64]
#           [--force]
#                     first install or update from the app's payload into DIR:
#                     stops a running daemon, installs, starts it again if it
#                     was running. Keeps etc/ (config, key, pairing state)
#                     and log/. A first install writes a config from
#                     config.default.toml with a random name and MAC seed;
#                     --config replaces the config, --psk-file the key.
#                     Does nothing when DIR already holds this payload (two
#                     triggers applying one update), unless --force
#   start | stop | restart
#                     the supervised daemon (start refuses while DIR/disable
#                     or DIR/crashloop exists)
#   autostart on|off  write or delete the boot hook (opt-in, off by default)
#   mic off|on        create or delete the kill switch (the mic is released
#                     at once; lost at reboot)
#   pair              open the pairing window (daemon running, pairing on)
#   report [--full|--json]
#                     the compatibility report and a prefilled GitHub issue
#                     link (nothing is sent: you open the link, review it and
#                     press Submit). --full: the whole report text; --json:
#                     what the status app shows. Uses the daemon's last
#                     report (run/report.*), or builds a static one
#   selfcheck         re-run the microphone self-check (daemon running)
#   ack-firmware      acknowledge a firmware change (clears the warning)
#   tv-settings [--json]
#                     read Always Ready, LG hands-free (Hi LG), the remote
#                     power-off setting and the microphone state (read only)
#   set always-ready on|off | hi-lg on|off | remote-off normal|keep
#       | reply-volume N
#                     change one setting on an explicit request: the two LG
#                     settings are written once, read back and logged to
#                     etc/tv-settings.log (ADR 0008); remote-off and
#                     reply-volume (N = 0 to 100) are the satellite's own
#                     settings, written to etc/ and picked up by the running
#                     daemon within a second. The daemon never does this
#   config aec on|off | music on|off | mic-button on|off | overlay on|off
#                     [aec] and [music] enabled, [remote] mic_button (on:
#                     "ptt", off: "off"), [overlay] enabled (the on-screen
#                     assistant); the daemon reads them at start:
#                     rewrites only that line of etc/config.toml (checked with
#                     --check-config first, never installed when the check
#                     fails) and restarts a running daemon, which ends a
#                     phone setup session (so it is not a `set`: nothing the
#                     setup server runs may stop it). The guards live here, so
#                     the TV app and Home Assistant's switch behave the same:
#                     a change within 30 s of the previous one is refused
#                     (run/config-change.stamp), and so is one while the
#                     daemon reports a voice request, music or the timer
#                     alarm in run/voice.json. One JSON line (result,
#                     daemon, error, reason cooldown|busy, retry_in_s)
#   setup start|status --json|answer ID allow|refuse|stop
#                     the phone setup web server (ADR 0009): start runs it
#                     (only on this explicit call, never at boot), status is
#                     the session state and the app's heartbeat, answer
#                     allows or refuses the request waiting for the TV
#                     remote's OK (ID: 8 lowercase hex characters), stop ends
#                     the session. All print JSON (schema 1)
#   reset-pairing     forget the stored key and open the window
#   clear-crashloop   delete DIR/crashloop after fixing the cause
#   uninstall         delete the boot hook, stop, delete the daemon; KEEP
#                     etc/ and log/ (a reinstall reuses them)
#   purge             delete the boot hook, stop, delete DIR entirely
#                     (config, key, pairing state, logs)
#   restore-lg-voice  give the Magic Remote's mic button back to LG's
#                     assistant (ADR 0013): repeatable; refused while running
#
# DIR is /var/lib/webos-satellite. Environment (tests, other layouts):
# WEBOS_SAT_PREFIX (DIR), WEBOS_SAT_INITD (default /var/lib/webosbrew/init.d),
# WEBOS_SAT_LOCK, WEBOS_SAT_ALLOW_NONROOT=1 (skip the root check),
# WEBOS_SAT_LUNA_SEND (default luna-send), WEBOS_SAT_SETTLE (seconds before a
# read-back, default 0.5), WEBOS_SAT_SETUP_DIR (the setup session directory,
# default /tmp/webos-satellite-setup), WEBOS_SAT_SETUP_BIN (the setup server),
# WEBOS_SAT_SOURCE (ha: the daemon asked for `config`, for the history line;
# anything else is logged as ctl).
# Exit status: 0 done, 1 failed (message on stderr), 2 usage.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

usage() {
    sed -n '2,85p' "$0" >&2
    exit 2
}

P=${WEBOS_SAT_PREFIX:-$STATE_PREFIX_DEFAULT}
case $P in
*[!A-Za-z0-9._/-]*) die "unsupported characters in the install directory: $P" ;;
esac
P=$(check_prefix "$P") || exit 1
set_paths "$P"
INITD=${WEBOS_SAT_INITD:-$INITD_DIR_DEFAULT}
HOOK=$INITD/$HOOK_NAME
PAIR_DIR=$P/etc/pairing

# Payload: the package files to install from. ctl.sh runs either from the
# app's payload directory or from DIR/bin (then the app is found through
# DIR/app-dir, recorded at install).
APPDIR=
PAYLOAD=
if [ -f "$here/SHA256SUMS" ] && [ -f "$here/install.sh" ]; then
    PAYLOAD=$here
    [ ! -f "$here/../appinfo.json" ] || APPDIR=$(cd "$here/.." && pwd)
elif [ -f "$P/app-dir" ]; then
    read -r APPDIR <"$P/app-dir" || true
    if [ -f "$APPDIR/payload/SHA256SUMS" ]; then
        PAYLOAD=$APPDIR/payload
    fi
fi

need_root() {
    [ "$(id -u)" = 0 ] || [ "${WEBOS_SAT_ALLOW_NONROOT:-}" = 1 ] ||
        die "needs root (from the app: the Homebrew Channel service must be elevated)"
}

locked() {
    lock_take 60
    trap lock_release EXIT
    trap 'lock_release; exit 1' INT TERM HUP
}

installed() { [ -f "$P/$MARKER" ] && [ -x "$BIN" ]; }

running() { is_ours "$(read_pid "$PIDFILE")" || supervisor_alive; }

# Random lowercase hex, $1 bytes (od or hexdump; date and pid as a last resort).
rand_hex() {
    h=$(od -An -N"$1" -tx1 /dev/urandom 2>/dev/null | tr -d ' \n') || h=
    [ -n "$h" ] || h=$(hexdump -n "$1" -e '1/1 "%02x"' /dev/urandom 2>/dev/null) || h=
    [ -n "$h" ] || h=$(printf '%x%x' "$(date +%s)" "$$")
    echo "$h"
}

write_hook() {
    tmpl=$P/bin/webos-satellite.hook
    [ -f "$tmpl" ] || die "hook template missing: $tmpl (reinstall)"
    mkdir -p "$INITD"
    sed "s|@PREFIX@|$P|" "$tmpl" >"$HOOK.new.$$"
    chmod 755 "$HOOK.new.$$"
    mv -f "$HOOK.new.$$" "$HOOK"
}

# --- the overlay app (docs/design/assistant-overlay.md, ADR 0014) ------------
# A second web app, <app id>.overlay, shipped as payload/overlay.ipk: SAM
# registers a second app of one package only after it restarts (EXP-024), so
# it is installed as its own package through appinstalld, the way the
# Homebrew Channel installs (tv/appinstall.sh). Its result is a note only: the
# daemon runs without it (it logs that the overlay app did not start).
OVERLAY_SUMS=$P/overlay.sums

overlay_id() { [ -n "$APPDIR" ] && echo "$(basename "$APPDIR").overlay"; }
overlay_dir() { echo "$(dirname "$APPDIR")/$(overlay_id)"; }

# The wait for appinstalld's end state, in ms: an install of the overlay app
# (about 20 KB) is done in a few seconds; this bounds a hung call.
APPINSTALL_WAIT_MS=20000

# A private working directory for appinstall: mktemp -d under /tmp (never
# TMPDIR from the environment: the path goes into the luna parameters and is
# read by appinstalld). Prints the directory.
overlay_workdir() {
    d=$(mktemp -d /tmp/webos-satellite-overlay.XXXXXX 2>/dev/null) || return 1
    case $d in /tmp/webos-satellite-overlay.*) ;; *) return 1 ;; esac
    case $d in *[!A-Za-z0-9._/-]*) rm -rf "$d"; return 1 ;; esac
    echo "$d"
}

# One appinstalld call (install IPK | remove ID) with the fifo in directory
# DIR; waits for the end state. Status 0 when done.
appinstall() { # install|remove IPK|ID DIR
    lsp=${WEBOS_SAT_LUNA_SEND_PUB:-luna-send-pub}
    command -v "$lsp" >/dev/null 2>&1 || { echo "no $lsp"; return 1; }
    case $1 in
    install)
        uri=luna://com.webos.appInstallService/dev/install
        params="{\"id\":\"com.ares.defaultName\",\"ipkUrl\":\"$2\",\"subscribe\":true}"
        ok='installed|"statusValue": ?30[^0-9]'
        ;;
    *)
        uri=luna://com.webos.appInstallService/dev/remove
        params="{\"id\":\"$2\",\"subscribe\":true}"
        ok='"statusValue": ?31[^0-9]'
        ;;
    esac
    fifo=$3/reply
    mkfifo -m 600 "$fifo" || { echo "cannot make a fifo"; return 1; }
    "$lsp" -w "$APPINSTALL_WAIT_MS" -i "$uri" "$params" </dev/null >"$fifo" 2>/dev/null &
    lp=$!
    res=$(grep -m 1 -E -e "$ok" -e 'failed|"returnValue": ?false|"errorCode"' "$fifo") || res=
    kill -TERM "$lp" 2>/dev/null || true
    wait "$lp" 2>/dev/null || true
    rm -f "$fifo"
    if [ -z "$res" ]; then
        echo "no answer from appinstalld within $((APPINSTALL_WAIT_MS / 1000)) s"
        return 1
    fi
    if printf '%s' "$res" | grep -Eq 'failed|"returnValue": ?false|"errorCode"'; then
        printf '%s\n' "$res" | tr -c 'A-Za-z0-9 ._:,"{}-' '?' | cut -c1-160
        return 1
    fi
    return 0
}

# Installs or updates the overlay app when the package's overlay.ipk differs
# from the one installed last (or its app directory is gone). Run after the
# daemon is started again, so it never keeps the voice service down.
overlay_sync() {
    [ -n "$APPDIR" ] && [ -f "$PAYLOAD/overlay.ipk" ] || return 0
    want=$(sed -n 's/^\([0-9a-f]\{64\}\)  overlay\.ipk$/\1/p' "$PAYLOAD/SHA256SUMS" 2>/dev/null | head -n 1)
    have=
    [ ! -f "$OVERLAY_SUMS" ] || read -r have <"$OVERLAY_SUMS" || true
    if [ -n "$want" ] && [ "$want" = "$have" ] && [ -f "$(overlay_dir)/appinfo.json" ]; then
        return 0
    fi
    # appinstalld gets its own copy (it may move or delete the file), in a
    # fresh private directory it can read.
    if ! wd=$(overlay_workdir); then
        note "overlay app: cannot make a working directory; not installed"
        return 0
    fi
    if ! cp "$PAYLOAD/overlay.ipk" "$wd/overlay.ipk" 2>/dev/null; then
        rm -rf "$wd"
        note "overlay app: cannot copy the package; not installed"
        return 0
    fi
    chmod 755 "$wd" 2>/dev/null || true
    chmod 644 "$wd/overlay.ipk" 2>/dev/null || true
    if why=$(appinstall install "$wd/overlay.ipk" "$wd"); then
        echo "$want" >"$OVERLAY_SUMS.new.$$" && mv -f "$OVERLAY_SUMS.new.$$" "$OVERLAY_SUMS"
        note "overlay app $(overlay_id) installed"
    else
        rm -f "$OVERLAY_SUMS"
        note "overlay app: not installed ($why); the satellite works without the on-screen assistant"
    fi
    rm -rf "$wd"
}

overlay_app_present() { [ -n "$APPDIR" ] && [ -f "$(overlay_dir)/appinfo.json" ]; }
# The overlay can show: its app is installed and luna-send is there (the
# daemon also needs its power watch).
overlay_can_show() { overlay_app_present && command -v "$LUNA" >/dev/null 2>&1; }

# Removes the overlay app (uninstall, purge).
overlay_remove() {
    rm -f "$OVERLAY_SUMS"
    [ -n "$APPDIR" ] && [ -d "$(overlay_dir)" ] || return 0
    if ! wd=$(overlay_workdir); then
        note "overlay app: cannot make a working directory; not removed"
        return 0
    fi
    if why=$(appinstall remove "$(overlay_id)" "$wd"); then
        note "overlay app $(overlay_id) removed"
    else
        note "overlay app: not removed ($why); remove $(overlay_id) with the Homebrew Channel"
    fi
    rm -rf "$wd"
}

cmd_install() {
    cfg=
    psk=
    arch=auto
    force=0
    while [ $# -gt 0 ]; do
        case $1 in
        --config) [ $# -ge 2 ] || usage; cfg=$2; shift 2 ;;
        --psk-file) [ $# -ge 2 ] || usage; psk=$2; shift 2 ;;
        --arch) [ $# -ge 2 ] || usage; arch=$2; shift 2 ;;
        --force) force=1; shift ;;
        *) usage ;;
        esac
    done
    need_root
    [ -n "$PAYLOAD" ] || die "no package payload found (run the ctl.sh inside the app's payload directory)"
    locked
    if [ "$force" -eq 0 ] && [ -z "$cfg" ] && [ -z "$psk" ] && installed &&
        cmp -s "$PAYLOAD/SHA256SUMS" "$P/package.sums"; then
        note "already up to date ($(cat "$P/VERSION" 2>/dev/null || echo unknown)); nothing to do (--force reinstalls)"
        overlay_sync
        return 0
    fi
    stop_setup_server
    was_running=0
    if [ -f "$P/$MARKER" ] && running; then
        was_running=1
        note "stopping the running daemon for the update"
        stop_daemon 10
        lg_restore_after_stop
    fi
    tmpcfg=
    if [ -n "$cfg" ]; then
        set -- --config "$cfg"
    elif [ -f "$CFG" ]; then
        set -- --keep-config
    else
        tmpcfg=$(mktemp "${TMPDIR:-/tmp}/webos-satellite-config.XXXXXX")
        sed -e "s/@NAME@/webos-satellite-$(rand_hex 3)/" -e "s/@MAC_SEED@/$(rand_hex 16)/" \
            "$PAYLOAD/config.default.toml" >"$tmpcfg"
        set -- --config "$tmpcfg"
        note "first install: writing a new config with a random device name"
    fi
    [ -z "$psk" ] || set -- "$@" --psk-file "$psk"
    rc=0
    sh "$PAYLOAD/install.sh" --prefix "$P" --persistent --arch "$arch" "$@" || rc=$?
    [ -z "$tmpcfg" ] || rm -f "$tmpcfg"
    if [ "$rc" -ne 0 ]; then
        if [ "$was_running" -eq 1 ] && installed; then
            note "install failed; starting the previous install again"
            sh "$P/bin/start.sh" --supervise || true
        fi
        die "install failed (status $rc)"
    fi
    if [ -n "$APPDIR" ]; then
        echo "$APPDIR" >"$P/app-dir.new.$$" && mv -f "$P/app-dir.new.$$" "$P/app-dir"
    else
        rm -f "$P/app-dir"
    fi
    if [ -e "$HOOK" ]; then
        write_hook
        note "boot hook refreshed: $HOOK"
    fi
    note "installed $(cat "$P/VERSION" 2>/dev/null || echo "(no version)") under $P"
    if [ "$was_running" -eq 1 ]; then
        sh "$P/bin/start.sh" --supervise
    fi
    # After the restart: the voice service is not kept down by appinstalld.
    overlay_sync
}

cmd_start() {
    need_root
    installed || die "not installed (ctl.sh install)"
    locked
    sh "$P/bin/start.sh" --supervise
}

cmd_stop() {
    need_root
    installed || die "not installed"
    locked
    stop_daemon 10
    lg_restore_after_stop
    note "stopped"
}

cmd_autostart() {
    [ $# -eq 1 ] || usage
    need_root
    case $1 in
    on)
        installed || die "not installed (ctl.sh install)"
        locked
        write_hook
        note "start at boot: on ($HOOK; the daemon starts about 60 s after boot)"
        ;;
    off)
        rm -f "$HOOK"
        note "start at boot: off"
        ;;
    *) usage ;;
    esac
}

cmd_mic() {
    [ $# -eq 1 ] || usage
    case $1 in
    off) : >"$KILL_SWITCH_DEFAULT" && note "microphone off (kill switch $KILL_SWITCH_DEFAULT)" ;;
    on) rm -f "$KILL_SWITCH_DEFAULT" && note "kill switch removed" ;;
    *) usage ;;
    esac
}

cmd_trigger() { # pair | reset-pairing
    need_root
    installed || die "not installed"
    [ -d "$PAIR_DIR" ] || die "pairing is not enabled in $CFG (no $PAIR_DIR)"
    running || die "the daemon is not running; start it first"
    : >"$PAIR_DIR/$1"
    note "$1 requested; the daemon acts on it within a second"
}

# Directory the daemon writes its report and status files to, and polls for
# the trigger files ([report] dir; <config dir>/../run for etc/config.toml).
RUN_DIR=$P/run

cmd_report() {
    mode=short
    case ${1:-} in
    '') ;;
    --full) mode=full ;;
    --json) mode=json ;;
    *) usage ;;
    esac
    [ $# -le 1 ] || usage
    installed || die "not installed (ctl.sh install)"
    if [ -f "$RUN_DIR/report.json" ] && [ -f "$RUN_DIR/report.short.txt" ] && [ -f "$RUN_DIR/report.url" ]; then
        # A withheld report has an "error" and no usable text.
        if grep -q '"error"' "$RUN_DIR/report.json" 2>/dev/null; then
            if [ "$mode" = json ]; then
                cat "$RUN_DIR/report.json"
                return 0
            fi
            die "the report was withheld: $(sed -n 's/.*"error":"\([^"]*\)".*/\1/p' "$RUN_DIR/report.json")"
        fi
        if [ "$mode" != json ] && ! running; then
            note "(the daemon is not running: this is the last saved report)"
        fi
        case $mode in
        json) cat "$RUN_DIR/report.json" ;;
        full) cat "$RUN_DIR/report.full.txt" ;;
        short)
            cat "$RUN_DIR/report.short.txt"
            echo
            echo "Create the GitHub issue (prefilled; you review it and press Submit):"
            cat "$RUN_DIR/report.url"
            ;;
        esac
        return 0
    fi
    case $mode in
    short) exec "$BIN" --config "$CFG" --report ;;
    *) exec "$BIN" --config "$CFG" --report "--$mode" ;;
    esac
}

cmd_selfcheck() {
    installed || die "not installed"
    if running; then
        mkdir -p "$RUN_DIR"
        : >"$RUN_DIR/selfcheck-run"
        note "microphone self-check requested; the result shows in Home Assistant (sensor Microphone self-check) and in the app within about 15 seconds"
    else
        note "the daemon is not running: the microphone self-check runs at the next start"
    fi
}

cmd_ack_firmware() {
    installed || die "not installed"
    if running; then
        mkdir -p "$RUN_DIR"
        : >"$RUN_DIR/firmware-ack"
        note "firmware change acknowledged; the daemon clears the warning within a few seconds"
    else
        fwv=$(sed -n 's/.*"webos_manufacturing_version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
            "${WEBOS_SAT_OS_INFO:-/var/run/nyx/os_info.json}" 2>/dev/null | head -n 1)
        [ -n "$fwv" ] || die "cannot read the firmware version"
        mkdir -p "$P/etc"
        echo "$fwv" >"$P/etc/firmware_seen.new.$$" && mv -f "$P/etc/firmware_seen.new.$$" "$P/etc/firmware_seen"
        note "firmware $fwv recorded as the one this daemon runs on"
    fi
}

remove_hook_and_stop() {
    rm -f "$HOOK"
    stop_setup_server
    if [ -f "$P/$MARKER" ]; then
        stop_daemon 10
        lg_restore_after_stop removing
    fi
}

cmd_uninstall() {
    need_root
    [ -f "$P/$MARKER" ] || die "$P is not installed"
    locked
    remove_hook_and_stop
    overlay_remove
    rm -rf "${P:?}/bin" "${P:?}/run" "${P:?}/out"
    rm -f "$P/VERSION" "$P/package.sums" "$P/app-dir"
    note "removed the boot hook and the daemon; kept config, key and logs in $P/etc and $P/log"
    note "remove those too with: sh <app>/payload/ctl.sh purge, or rm -rf $P"
}

cmd_purge() {
    need_root
    locked
    rm -f "$HOOK"
    stop_setup_server
    if [ -e "$P" ]; then
        [ -f "$P/$MARKER" ] || die "$P exists but is not a webos-satellite install; not removing it"
        stop_daemon 10
        lg_restore_after_stop removing
        overlay_remove
        cd /
        rm -rf "$P"
    fi
    note "removed $P and the boot hook"
}

# --- status ------------------------------------------------------------------

json_str() {
    printf '"%s"' "$(printf '%s' "$1" | tr -d '\000-\037' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
}

json_bool() { if "$@"; then printf true; else printf false; fi; }

# Value of a top-level `key = "value"` or `key = number` line in the config.
cfg_value() {
    sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\{0,1\}\([^\"#]*\)\"\{0,1\}.*/\1/p" "$CFG" 2>/dev/null |
        head -n 1 | sed 's/[[:space:]]*$//'
}

# The header line of TOML table $1 (a plain name: letters, digits, _ and -),
# as a basic regular expression: "[aec]", "[ aec ]", "[aec]  # note".
# "[aec.aec3]" and "[[aec]]" are other tables.
cfg_header_re() {
    printf '^[[:space:]]*\\[[[:space:]]*%s[[:space:]]*\\][[:space:]]*\\(#.*\\)\\{0,1\\}$' "$1"
}

# The bare value of `$2 = value` inside table [$1] of config file $3
# (default $CFG): from the table's header to the next header; "" when the
# table or the key is not there. One sed, no loop (the status runs every 10 s).
cfg_section_value() {
    sed -n "/$(cfg_header_re "$1")/,/^[[:space:]]*\\[/{
s/^[[:space:]]*$2[[:space:]]*=[[:space:]]*\"\\{0,1\\}\\([^\"#[:space:]]*\\).*/\\1/p
}" "${3:-$CFG}" 2>/dev/null | head -n 1
}

# Whether [aec] enabled and [music] enabled are on in the config file, as
# true or false: echo cancellation is on unless the key says false (the
# daemon's default since 0.4.3, ADR 0011), music only when it says true
# (default off, ADR 0010). The daemon reads both at start only.
aec_enabled_json() { if [ "$(cfg_section_value aec enabled)" = false ]; then printf false; else printf true; fi; }
music_enabled_json() { if [ "$(cfg_section_value music enabled)" = true ]; then printf true; else printf false; fi; }
# [overlay] enabled (the on-screen assistant, docs/design/assistant-overlay.md):
# off unless the key says true (the daemon's default until the owner's check).
overlay_enabled_json() { if [ "$(cfg_section_value overlay enabled)" = true ]; then printf true; else printf false; fi; }
# [remote] mic_button (the Magic Remote's mic button, ADR 0013 proposed): off
# unless it says ptt.
# Who handles the Magic Remote's mic button (ADR 0013): what a running
# daemon reports, else the state file: "lg", "ours (configd)" or "unknown"
# (a state file is left and no daemon confirms it: restore-lg-voice clears it).
LG_VOICE_STATE=lg_voice_override
lg_voice_key_word() {
    if running && voice_fresh; then
        lk=$(sed -n 's/.*"lg_voice_key"[[:space:]]*:[[:space:]]*"\([a-z ()]*\)".*/\1/p' "$RUN_DIR/voice.json" 2>/dev/null | head -n 1)
        case $lk in lg | 'ours (configd)' | unknown) printf '%s' "$lk"; return 0 ;; esac
    fi
    if [ -f "$P/run/$LG_VOICE_STATE" ]; then printf unknown; else printf lg; fi
}

# A TOML string may be in double or single quotes.
mic_button_word() {
    mb=$(cfg_section_value remote mic_button)
    mb=${mb#\'}
    mb=${mb%\'}
    if [ "$mb" = ptt ]; then printf ptt; else printf off; fi
}

# The reply volume file: sink.volume_file of the config, relative to etc/
# (the daemon's default is etc/reply_volume).
volume_file() {
    f=$(cfg_value volume_file)
    f=${f:-reply_volume}
    case $f in /*) ;; *) f=$P/etc/$f ;; esac
    echo "$f"
}

# The open pairing window as JSON, from run/voice.json: {"open":true,
# "ends_in_s":412}, else {"open":false,"ends_in_s":null} (closed, daemon
# stopped, or a status file older than 60 s). The daemon publishes the end
# as seconds since the epoch (written on open and close only).
pairing_json() {
    until=
    if running && voice_fresh; then
        until=$(sed -n 's/.*"pairing_until"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\)[,}].*/\1/p' "$RUN_DIR/voice.json" 2>/dev/null | head -n 1)
    fi
    now=$(date +%s)
    case $until$now in *[!0-9]*) until= ;; esac
    if [ -n "$until" ] && [ "$until" -gt "$now" ]; then
        printf '{"open":true,"ends_in_s":%s}' "$((until - now))"
    else
        printf '{"open":false,"ends_in_s":null}'
    fi
}

# The Home Assistant API connection as JSON, from run/voice.json:
# {"state":"connected|disconnected|unpaired","since_s":N} (N seconds since the
# state began), or {"state":"unknown","since_s":null} when the daemon is
# stopped, the file is stale or an older daemon does not write it. Only the
# state: no address, name or version.
ha_json() {
    st=
    since=
    if running && voice_fresh; then
        st=$(voice_field ha)
        since=$(sed -n 's/.*"ha_since"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\)[,}].*/\1/p' "$RUN_DIR/voice.json" 2>/dev/null | head -n 1)
    fi
    now=$(date +%s)
    case $st in connected | disconnected | unpaired) ;; *) st= ;; esac
    case $since$now in *[!0-9]*) since= ;; esac
    if [ -n "$st" ]; then
        if [ -n "$since" ] && [ "$since" -le "$now" ]; then
            printf '{"state":"%s","since_s":%s}' "$st" "$((now - since))"
        else
            printf '{"state":"%s","since_s":null}' "$st"
        fi
    else
        printf '{"state":"unknown","since_s":null}'
    fi
}

# Key state: fixed (etc/noise_psk named in the config), paired (a key set by
# Home Assistant), cleared (HA removed the device), unpaired, or none.
key_state() {
    if [ -n "$(cfg_value noise_psk_file)" ] || [ -n "$(cfg_value noise_psk)" ]; then
        echo fixed
    elif [ -f "$PAIR_DIR/noise_psk" ]; then
        echo paired
    elif [ -f "$PAIR_DIR/noise_psk.cleared" ]; then
        echo cleared
    elif [ -d "$PAIR_DIR" ] || [ "$(cfg_section_value pairing enabled)" = true ]; then
        echo unpaired
    else
        echo none
    fi
}

firmware_version() {
    sed -n 's/.*"webos_manufacturing_version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        /var/run/nyx/os_info.json 2>/dev/null | head -n 1
}

cmd_status() {
    json=0
    [ "${1:-}" != --json ] || json=1
    pkg_version=$(cat "$PAYLOAD/VERSION" 2>/dev/null || true)
    inst_version=$(cat "$P/VERSION" 2>/dev/null || true)
    is_inst=false
    installed && is_inst=true
    update=false
    if [ "$is_inst" = true ] && [ -n "$PAYLOAD" ] && ! cmp -s "$PAYLOAD/SHA256SUMS" "$P/package.sums"; then
        update=true
    fi
    pid=$(read_pid "$PIDFILE")
    is_ours "$pid" || pid=
    build=
    fw=
    fw_ok=false
    fw_verified=false
    if [ "$is_inst" = true ]; then
        build=$("$BIN" --version 2>/dev/null || true)
        rc=0
        fw=$("$BIN" --config "$CFG" --check-firmware 2>&1) || rc=$?
        case $rc in
        0) fw_verified=true ;;
        esac
        # May the daemon start the microphone? Always, except with the strict
        # policy (the old hard gate) on a firmware that is not in its list.
        if [ "$(cfg_value firmware_policy)" = strict ]; then
            case $rc in
            0 | 3) fw_ok=true ;;
            esac
        else
            fw_ok=true
        fi
    else
        fw="firmware $(firmware_version) (compared with the verified list after set up)"
        fw_ok=true
    fi
    compat=null
    if [ -f "$RUN_DIR/compat.json" ]; then
        compat=$(tr -d '\n\r' <"$RUN_DIR/compat.json" 2>/dev/null || true)
        case $compat in
        '{'*'}') ;;
        *) compat=null ;;
        esac
    fi
    keys=$(key_state)
    # Written by the daemon when Home Assistant changes them (etc/). `1{...;p;}`
    # prints even when the line is already clean (`s///gp` printed nothing then).
    remote_off=$(sed -n '1{s/[^a-z_]//g;p;}' "$P/etc/remote_off" 2>/dev/null || true)
    reply_volume=$(sed -n '1{s/[^0-9]//g;p;}' "$(volume_file)" 2>/dev/null || true)
    # Until Home Assistant stores one, the daemon uses sink.volume (percent).
    reply_volume_default=$(cfg_value volume | sed 's/[^0-9]//g')
    name=$(cfg_value name)
    port=$(cfg_value port)
    aec_on=$(aec_enabled_json)
    music_on=$(music_enabled_json)
    mic_button=$(mic_button_word)
    lg_voice_key=$(lg_voice_key_word)
    mic_active=null
    mic_error=
    if running && voice_fresh; then
        case $(voice_field mic_button) in
        ptt) mic_active=true ;;
        off | failed) mic_active=false ;;
        esac
        mic_error=$(sed -n 's/.*"mic_button_error"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$RUN_DIR/voice.json" 2>/dev/null | head -n 1 | tr -d '\n' | tr -c "A-Za-z0-9 ._:;,()/=+'-" ' ' | cut -c1-200)
    fi
    overlay_on=$(overlay_enabled_json)
    if [ "$json" -eq 0 ]; then
        note "package: ${pkg_version:-none} ($PAYLOAD)"
        note "installed: $is_inst ${inst_version:+$inst_version }in $P${build:+ ($build)}"
        [ "$update" = false ] || note "an update is pending: run ctl.sh install (or open the app)"
        if [ -n "$pid" ]; then note "running: pid $pid"; else note "running: no"; fi
        if [ -e "$HOOK" ]; then note "start at boot: on ($HOOK)"; else note "start at boot: off"; fi
        [ ! -e "$DISABLE" ] || note "marker $DISABLE present: starts are refused"
        [ ! -e "$CRASHLOOP" ] || note "marker $CRASHLOOP present: the supervisor gave up"
        [ ! -e "$KILL_SWITCH_DEFAULT" ] || note "microphone off (kill switch $KILL_SWITCH_DEFAULT)"
        if [ "$fw_verified" = true ]; then
            note "firmware: $fw"
        else
            note "firmware: $fw (not verified yet: the microphone runs anyway, press Report compatibility in the app if it does not work)"
        fi
        if [ "$compat" != null ]; then
            cstat=$(printf '%s' "$compat" | sed -n 's/.*"compat_status":"\([^"]*\)".*/\1/p')
            csel=$(printf '%s' "$compat" | sed -n 's/.*"selfcheck":"\([^"]*\)".*/\1/p')
            note "compatibility: ${cstat:-?}; microphone self-check: ${csel:-?} (ctl.sh report: the report and the issue link)"
        fi
        note "device: ${name:-?} port ${port:-6053}, key: $keys"
        if [ -n "$reply_volume" ]; then
            vol_text="$reply_volume %"
        elif [ -n "$reply_volume_default" ]; then
            vol_text="$reply_volume_default % (config default; none stored yet)"
        else
            vol_text="default (none stored yet)"
        fi
        note "reply volume: $vol_text, remote power-off: ${remote_off:-normal}"
        on_off() { if [ "$1" = true ]; then echo on; else echo off; fi; }
        note "echo cancellation: $(on_off "$aec_on"), music: $(on_off "$music_on"), remote mic button: $mic_button (config; ctl.sh config aec|music|mic-button on|off)"
        if [ "$mic_active" = false ] && [ "$mic_button" = ptt ]; then
            note "remote mic button NOT running${mic_error:+: $mic_error}"
        fi
        note "LG voice key: $lg_voice_key (ctl.sh restore-lg-voice gives it back to LG)"
        if ! overlay_app_present; then
            ova="unavailable: its app is not installed (ctl.sh install)"
        elif ! command -v "$LUNA" >/dev/null 2>&1; then
            ova="unavailable: no luna-send"
        else
            ova="app installed"
        fi
        note "on-screen assistant: $(on_off "$overlay_on") (config; ctl.sh config overlay on|off), $ova"
        return 0
    fi
    printf '{"package_version":%s,' "$(json_str "$pkg_version")"
    printf '"installed":%s,"installed_version":%s,' "$is_inst" "$(json_str "$inst_version")"
    printf '"build":%s,"update_pending":%s,' "$(json_str "$build")" "$update"
    printf '"uid":%s,"machine":%s,' "$(id -u)" "$(json_str "$(uname -m)")"
    printf '"running":%s,"pid":%s,' "$(json_bool test -n "$pid")" "${pid:-null}"
    printf '"supervised":%s,' "$(json_bool supervisor_alive)"
    printf '"autostart":%s,' "$(json_bool test -e "$HOOK")"
    printf '"disabled":%s,"crashloop":%s,' "$(json_bool test -e "$DISABLE")" "$(json_bool test -e "$CRASHLOOP")"
    printf '"mic_off":%s,' "$(json_bool test -e "$KILL_SWITCH_DEFAULT")"
    printf '"firmware":%s,"firmware_ok":%s,"firmware_verified":%s,' "$(json_str "$fw")" "$fw_ok" "$fw_verified"
    printf '"compat":%s,' "$compat"
    printf '"key":%s,"name":%s,"port":%s,' "$(json_str "$keys")" "$(json_str "$name")" "$(json_str "${port:-6053}")"
    printf '"remote_off":%s,"reply_volume":%s,' "$(json_str "${remote_off:-normal}")" "$(json_str "$reply_volume")"
    printf '"reply_volume_default":%s,' "$(json_str "$reply_volume_default")"
    printf '"aec_enabled":%s,"music_enabled":%s,' "$aec_on" "$music_on"
    printf '"mic_button":%s,"mic_button_active":%s,"mic_button_error":%s,"lg_voice_key":%s,' \
        "$(json_str "$mic_button")" "$mic_active" "$( [ -n "$mic_error" ] && json_str "$mic_error" || printf null)" "$(json_str "$lg_voice_key")"
    printf '"overlay_enabled":%s,"overlay_app":%s,"overlay_available":%s,' "$overlay_on" "$(json_bool overlay_app_present)" "$(json_bool overlay_can_show)"
    printf '"pairing_window_s":%s,' "$(json_str "$(cfg_value window_s | sed 's/[^0-9]//g')")"
    printf '"pairing":%s,"ha":%s,' "$(pairing_json)" "$(ha_json)"
    printf '"prefix":%s,"log":[' "$(json_str "$P")"
    sep=
    if [ -f "$LOG" ]; then
        tail -n 8 "$LOG" | while IFS= read -r line; do
            printf '%s%s' "$sep" "$(json_str "$line")"
            sep=,
        done
    fi
    printf ']}\n'
}

# --- tv-settings and set (docs/design/voice-and-power-screen.md, section 6) ---

LUNA=${WEBOS_SAT_LUNA_SEND:-luna-send}
HISTORY=$P/etc/tv-settings.log
HISTORY_MAX=200
LUNA_MODE= # plain | pty, found by the first successful read
LUNA_REPLY=
LUNA_ERR=

# Runs one luna call on a pseudo-terminal: luna-send printed nothing over a
# pipe for the audio methods (docs/design/tv-control.md section 5), and the
# daemon runs its calls on a pty. python3 is on the TVs (c3-a record); a TV
# without it gets the plain pipe only. os.openpty, not the pty module: c3-a's
# python3 3.8 has no pty module (P4, 0.3.0) but has os.openpty.
# argv: timeout seconds, then the command.
PTY_PY='import os, select, subprocess, sys, time
end = time.time() + float(sys.argv[1])
m, s = os.openpty()
p = subprocess.Popen(sys.argv[2:], stdin=s, stdout=s, stderr=s, close_fds=True)
os.close(s)
out = b""
while time.time() < end and len(out) < 65536:
    r = select.select([m], [], [], 0.3)[0]
    if r:
        try:
            d = os.read(m, 4096)
        except OSError:
            break
        if not d:
            break
        out += d
    elif p.poll() is not None:
        break
if p.poll() is None:
    p.kill()
p.wait()
sys.stdout.buffer.write(out)'

luna_raw() { # MODE URI PAYLOAD
    case $1 in
    pty) python3 -c "$PTY_PY" 6 "$LUNA" -n 1 -w 3000 "$2" "$3" 2>/dev/null ;;
    *)
        if command -v timeout >/dev/null 2>&1; then
            timeout 6 "$LUNA" -n 1 -w 3000 "$2" "$3" </dev/null 2>/dev/null
        else
            "$LUNA" -n 1 -w 3000 "$2" "$3" </dev/null 2>/dev/null
        fi
        ;;
    esac
}

# One luna call. Sets LUNA_REPLY (one line, from the first brace) and
# LUNA_ERR; returns 0 when the TV answered returnValue true. Only the error
# code and text of a failure are kept, never a reply body.
luna_call() { # URI PAYLOAD
    LUNA_REPLY=
    LUNA_ERR=
    raw=
    if [ -n "$LUNA_MODE" ]; then
        raw=$(luna_raw "$LUNA_MODE" "$1" "$2") || raw=
    else
        raw=$(luna_raw plain "$1" "$2") || raw=
        case $raw in
        *'{'*) LUNA_MODE=plain ;;
        *)
            if command -v python3 >/dev/null 2>&1; then
                raw=$(luna_raw pty "$1" "$2") || raw=
                case $raw in *'{'*) LUNA_MODE=pty ;; esac
            fi
            ;;
        esac
    fi
    LUNA_REPLY=$(printf '%s' "$raw" | tr -d '\r\n' | sed 's/^[^{]*//')
    case $LUNA_REPLY in
    '') LUNA_ERR=timeout; return 1 ;;
    '{'*) ;;
    *) LUNA_ERR="bad reply"; return 1 ;;
    esac
    case $LUNA_REPLY in
    *'"returnValue"'*[:]*true*) return 0 ;;
    esac
    code=$(printf '%s' "$LUNA_REPLY" | sed -n 's/.*"errorCode"[[:space:]]*:[[:space:]]*\(-\{0,1\}[0-9]*\).*/\1/p')
    text=$(printf '%s' "$LUNA_REPLY" | sed -n 's/.*"errorText"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
        tr -c 'A-Za-z0-9 ._:-' '?' | cut -c1-80)
    LUNA_ERR=${text:-${code:+error $code}}
    LUNA_ERR=${LUNA_ERR:-failed}
    return 1
}

# The luna-send binary exists and the caller may use it; prints a reason if not.
luna_unavailable() {
    if [ "$(id -u)" != 0 ] && [ "${WEBOS_SAT_ALLOW_NONROOT:-}" != 1 ]; then
        echo "not root"
    elif ! command -v "$LUNA" >/dev/null 2>&1; then
        echo "no luna-send"
    fi
}

# Reads Always Ready: sets AR_STATE (off|on|unknown|error), AR_RAW, AR_ERR.
read_always_ready() {
    AR_STATE=error
    AR_RAW=
    AR_ERR=
    if luna_call luna://com.webos.settingsservice/getSystemSettings \
        '{"category":"general","keys":["lifeOnScreenMode"]}'; then
        AR_RAW=$(printf '%s' "$LUNA_REPLY" |
            sed -n 's/.*"lifeOnScreenMode"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | tr -c 'A-Za-z0-9\n' '?' | cut -c1-24)
        case $AR_RAW in
        off) AR_STATE=off ;;
        alwaysReady) AR_STATE=on ;;
        '') AR_ERR="no value in the reply" ;;
        *) AR_STATE=unknown ;;
        esac
    else
        AR_ERR=$LUNA_ERR
    fi
}

# settingStatus of one provider object of a getActivation reply; the top
# level when $2 is empty. Prints true, false or nothing.
hl_flag() { # REPLY PROVIDER
    if [ -n "$2" ]; then
        body=$(printf '%s' "$1" | sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*{\\([^}]*\\)}.*/\\1/p")
    else
        body=$(printf '%s' "$1" | sed 's/"[A-Za-z]*"[[:space:]]*:[[:space:]]*{[^}]*}//g')
    fi
    printf '%s' "$body" | sed -n 's/.*"settingStatus"[[:space:]]*:[[:space:]]*\(true\|false\).*/\1/p'
}

# Reads LG hands-free: sets HL_STATE (off|on|error), HL_THINQ, HL_AMAZON,
# HL_DANGBEI (true|false|null), HL_ERR. `on` is any provider on; the toggle
# of the app addresses thinQ only (EXP-011).
read_hi_lg() {
    HL_STATE=error
    HL_THINQ=null
    HL_AMAZON=null
    HL_DANGBEI=null
    HL_ERR=
    if luna_call luna://com.webos.service.voiceinput/setting/getActivation '{}'; then
        HL_THINQ=$(hl_flag "$LUNA_REPLY" thinQ)
        HL_AMAZON=$(hl_flag "$LUNA_REPLY" amazon)
        HL_DANGBEI=$(hl_flag "$LUNA_REPLY" dangbei)
        [ -n "$HL_THINQ" ] || HL_THINQ=$(hl_flag "$LUNA_REPLY" '')
        HL_THINQ=${HL_THINQ:-null}
        HL_AMAZON=${HL_AMAZON:-null}
        HL_DANGBEI=${HL_DANGBEI:-null}
        if [ "$HL_THINQ" = null ]; then
            HL_ERR="no settingStatus in the reply"
        elif [ "$HL_THINQ" = true ] || [ "$HL_AMAZON" = true ] || [ "$HL_DANGBEI" = true ]; then
            HL_STATE=on
        else
            HL_STATE=off
        fi
    else
        HL_ERR=$LUNA_ERR
    fi
}

utc_now() { date -u +%Y-%m-%dT%H:%M:%SZ; }

# The last applied change in the history file, as JSON or null.
last_change_json() {
    [ -f "$HISTORY" ] || { printf null; return 0; }
    l=$(grep ' result .*outcome=applied' "$HISTORY" 2>/dev/null | tail -n 1) || l=
    [ -n "$l" ] || { printf null; return 0; }
    at=${l%% *}
    from=
    to=
    st=$(printf '%s' "$l" | cut -d' ' -f2)
    # shellcheck disable=SC2086
    for w in $l; do
        case $w in
        from=*) from=${w#from=} ;;
        to=*) to=${w#to=} ;;
        esac
    done
    printf '{"setting":%s,"from":%s,"to":%s,"at":%s}' "$(json_str "$st")" "$(json_str "$from")" "$(json_str "$to")" "$(json_str "$at")"
}

# run/voice.json (written by the daemon); a missing or old (60 s) file is unknown.
voice_field() { # KEY (string or bool value)
    sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\\{0,1\\}\\([A-Za-z_ ]*\\)\"\\{0,1\\}[,}].*/\\1/p" "$RUN_DIR/voice.json" 2>/dev/null | head -n 1
}

voice_fresh() {
    [ -f "$RUN_DIR/voice.json" ] || return 1
    mt=$(stat -c %Y "$RUN_DIR/voice.json" 2>/dev/null) || return 1
    now=$(date +%s)
    case $mt$now in *[!0-9]*) return 1 ;; esac
    [ $((now - mt)) -le 60 ]
}

remote_off_word() {
    w=$(sed -n '1{s/[^a-z_]//g;p;}' "$P/etc/remote_off" 2>/dev/null || true)
    case $w in keep_listening) echo keep_listening ;; *) echo normal ;; esac
}

cmd_tv_settings() {
    case ${1:-} in '' | --json) ;; *) usage ;; esac
    [ $# -le 1 ] || usage
    installed || die "not installed (ctl.sh install)"
    why=$(luna_unavailable)
    if [ -n "$why" ]; then
        AR_STATE=error
        AR_RAW=
        AR_ERR=$why
        HL_STATE=error
        HL_THINQ=null
        HL_AMAZON=null
        HL_DANGBEI=null
        HL_ERR=$why
        luna=false
    else
        luna=true
        read_always_ready
        read_hi_lg
    fi
    ro=$(remote_off_word)
    mic=unknown
    ro_active=null
    pstate=null
    if voice_fresh; then
        m=$(voice_field mic)
        case $m in listening | busy | muted | closed) mic=$m ;; esac
        a=$(voice_field remote_off_active)
        case $a in true | false) ro_active=$a ;; esac
        ps=$(voice_field power_state)
        case $ps in '' | null) ;; *) pstate=$(json_str "$ps") ;; esac
    fi
    running || ro_active=null
    mic_kill=false
    [ ! -e "$KILL_SWITCH_DEFAULT" ] || mic_kill=true
    if [ "$mic_kill" = true ] && [ "$mic" = unknown ]; then mic=muted; fi
    printf '{"schema":1,"luna":%s,"daemon_running":%s,' "$luna" "$(json_bool running)"
    printf '"always_ready":{"state":%s,"raw":%s,"error":%s},' "$(json_str "$AR_STATE")" "$(json_str "$AR_RAW")" "$(json_str "$AR_ERR")"
    printf '"hi_lg":{"state":%s,"thinq":%s,"amazon":%s,"dangbei":%s,"error":%s},' "$(json_str "$HL_STATE")" "$HL_THINQ" "$HL_AMAZON" "$HL_DANGBEI" "$(json_str "$HL_ERR")"
    printf '"remote_off":{"state":%s,"active":%s},' "$(json_str "$ro")" "$ro_active"
    printf '"mic":{"state":%s,"kill_switch":%s},' "$(json_str "$mic")" "$mic_kill"
    printf '"power_state":%s,"last_change":%s}\n' "$pstate" "$(last_change_json)"
}

# Appends one line to the history file (mode 0600, capped); fails when it cannot.
history_add() {
    mkdir -p "$P/etc" 2>/dev/null || return 1
    (
        umask 077
        printf '%s %s\n' "$(utc_now)" "$1" >>"$HISTORY"
    ) || return 1
    chmod 600 "$HISTORY" 2>/dev/null || true
    n=$(wc -l <"$HISTORY" 2>/dev/null | tr -d ' ') || return 0
    case $n in '' | *[!0-9]*) return 0 ;; esac
    if [ "$n" -gt "$HISTORY_MAX" ]; then
        (
            umask 077
            tail -n "$HISTORY_MAX" "$HISTORY" >"$HISTORY.new.$$" && mv -f "$HISTORY.new.$$" "$HISTORY"
        ) || rm -f "$HISTORY.new.$$"
    fi
    return 0
}

# Prints the result of a set, on stdout as JSON and on stderr as a sentence.
set_emit() { # SETTING REQUESTED BEFORE AFTER RESULT ERROR
    printf '{"schema":1,"setting":%s,"requested":%s,"before":%s,"after":%s,"result":%s,"error":%s}\n' \
        "$(json_str "$1")" "$(json_str "$2")" "$(json_str "$3")" "$(json_str "$4")" "$(json_str "$5")" "$(json_str "$6")"
    case $5 in
    applied) echo "$1 is now $4 (was $3)" >&2 ;;
    already) echo "$1 is already $4; nothing changed" >&2 ;;
    mismatch) echo "$1: the TV reports $4, not $2; the change did not stick" >&2 ;;
    *) echo "$1: $5${6:+: $6} (the TV holds ${4:-$3})" >&2 ;;
    esac
    case $5 in applied | already) return 0 ;; *) return 1 ;; esac
}

settle() { sleep "${WEBOS_SAT_SETTLE:-0.5}" 2>/dev/null || sleep 1; }

# set always-ready|hi-lg on|off
cmd_set_lg() { # SETTING REQUESTED
    st=$1
    want=$2
    installed || die "not installed (ctl.sh install)"
    why=$(luna_unavailable)
    [ -z "$why" ] || { set_emit "$st" "$want" "" "" refused "$why"; return; }
    locked
    case $st in
    always-ready) read_always_ready; before=$AR_STATE; berr=$AR_ERR; braw=$AR_RAW ;;
    hi-lg) read_hi_lg; before=$HL_STATE; berr=$HL_ERR; braw= ;;
    esac
    if [ "$before" = error ]; then
        set_emit "$st" "$want" "" "" refused "${berr:-cannot read}"
        return
    fi
    if [ "$st" = hi-lg ]; then
        before=off
        [ "$HL_THINQ" != true ] || before=on
    fi
    if [ "$before" = unknown ]; then
        set_emit "$st" "$want" "" "$braw" refused "unknown value"
        return
    fi
    if [ "$before" = "$want" ]; then
        set_emit "$st" "$want" "$before" "$before" already ""
        return
    fi
    history_add "$st request from=$before to=$want source=ctl" ||
        { set_emit "$st" "$want" "$before" "$before" refused "cannot write $HISTORY"; return; }
    wrote=
    case $st in
    always-ready)
        v=off
        [ "$want" != on ] || v=alwaysReady
        luna_call luna://com.webos.settingsservice/setSystemSettings \
            "{\"category\":\"general\",\"settings\":{\"lifeOnScreenMode\":\"$v\"}}" || wrote=$LUNA_ERR
        ;;
    hi-lg)
        v=false
        [ "$want" != on ] || v=true
        luna_call luna://com.webos.service.voiceinput/setting/setActivation \
            "{\"settingStatus\":$v}" || wrote=$LUNA_ERR
        ;;
    esac
    settle
    after=
    aerr=
    case $st in
    always-ready)
        read_always_ready
        case $AR_STATE in error) aerr=$AR_ERR ;; *) after=$AR_STATE ;; esac
        [ "$AR_STATE" != unknown ] || after=$AR_RAW
        ;;
    hi-lg)
        read_hi_lg
        case $HL_STATE in error) aerr=$HL_ERR ;; *) after=off; [ "$HL_THINQ" != true ] || after=on ;; esac
        ;;
    esac
    if [ -n "$wrote" ]; then
        res=failed
        err=$wrote
    elif [ -n "$aerr" ]; then
        res=failed
        err="read-back: $aerr"
    elif [ "$after" = "$want" ]; then
        res=applied
        err=
    else
        res=mismatch
        err=
    fi
    history_add "$st result from=$before to=$want read=${after:-unknown} outcome=$res" || true
    if [ "$res" = applied ] && [ -d "$RUN_DIR" ] && running; then
        : >"$RUN_DIR/lg-settings-refresh" 2>/dev/null || true
    fi
    set_emit "$st" "$want" "$before" "$after" "$res" "$err"
}

# set remote-off normal|keep: the satellite's own setting; no luna.
cmd_set_remote_off() {
    case $1 in
    normal) word=normal ;;
    keep) word=keep_listening ;;
    *) usage ;;
    esac
    installed || die "not installed (ctl.sh install)"
    locked
    f=$(cfg_value remote_off_file)
    f=${f:-remote_off}
    case $f in /*) ;; *) f=$P/etc/$f ;; esac
    before=normal
    [ "$(sed -n '1{s/[^a-z_]//g;p;}' "$f" 2>/dev/null || true)" != keep_listening ] || before=keep_listening
    mkdir -p "$(dirname "$f")"
    if echo "$word" >"$f.new.$$" && mv -f "$f.new.$$" "$f"; then
        :
    else
        rm -f "$f.new.$$"
        printf '{"schema":1,"setting":"remote-off","requested":%s,"before":%s,"after":%s,"result":"failed","error":"cannot write the file","daemon":"unknown"}\n' \
            "$(json_str "$word")" "$(json_str "$before")" "$(json_str "$before")"
        return 1
    fi
    history_add "remote-off result from=$before to=$word read=$word outcome=applied" || true
    d="not running"
    mkdir -p "$RUN_DIR"
    : >"$RUN_DIR/remote-off-reload"
    if running; then
        d=pending
        i=0
        while [ "$i" -lt 6 ]; do
            if [ ! -e "$RUN_DIR/remote-off-reload" ]; then
                d=applied
                break
            fi
            sleep 0.5 2>/dev/null || sleep 1
            i=$((i + 1))
        done
    fi
    printf '{"schema":1,"setting":"remote-off","requested":%s,"before":%s,"after":%s,"result":"applied","error":"","daemon":%s}\n' \
        "$(json_str "$word")" "$(json_str "$before")" "$(json_str "$word")" "$(json_str "$d")"
    echo "remote-off is now $word (daemon: $d)" >&2
}

# set reply-volume N: the satellite's own setting. Writes etc/reply_volume
# (atomically) and asks the running daemon to read it again; HA's media player
# then follows. No sound is played.
cmd_set_reply_volume() { # N, already validated: 0 to 100 without leading zeros
    want=$1
    installed || die "not installed (ctl.sh install)"
    locked
    f=$(volume_file)
    before=$(sed -n '1{s/[^0-9]//g;p;}' "$f" 2>/dev/null || true)
    case $before in '' | *[!0-9]*) before= ;; esac
    if [ -z "$before" ]; then
        # Nothing stored: the daemon runs at the config's sink.volume.
        before=$(cfg_value volume | sed 's/[^0-9]//g')
    fi
    if [ "$before" = "$want" ]; then
        volume_emit "$want" "$before" "$want" already "" none
        return 0
    fi
    mkdir -p "$(dirname "$f")"
    if (umask 077 && echo "$want" >"$f.new.$$") && mv -f "$f.new.$$" "$f"; then
        :
    else
        rm -f "$f.new.$$"
        volume_emit "$want" "$before" "$before" failed "cannot write the file" unknown
        return 1
    fi
    chmod 600 "$f" 2>/dev/null || true
    history_add "reply-volume result from=${before:-unknown} to=$want read=$want outcome=applied source=ctl" || true
    d="not running"
    mkdir -p "$RUN_DIR"
    : >"$RUN_DIR/reply-volume-reload"
    if running; then
        d=pending
        i=0
        while [ "$i" -lt 6 ]; do
            if [ ! -e "$RUN_DIR/reply-volume-reload" ]; then
                d=applied
                break
            fi
            sleep 0.5 2>/dev/null || sleep 1
            i=$((i + 1))
        done
    fi
    volume_emit "$want" "$before" "$want" applied "" "$d"
    echo "reply-volume is now $want % (daemon: $d)" >&2
}

volume_emit() { # REQUESTED BEFORE AFTER RESULT ERROR DAEMON
    printf '{"schema":1,"setting":"reply-volume","requested":%s,"before":%s,"after":%s,"result":%s,"daemon":%s,"error":%s}\n' \
        "$(json_str "$1")" "$(json_str "$2")" "$(json_str "$3")" "$(json_str "$4")" "$(json_str "$6")" "$(json_str "$5")"
}

# --- restore-lg-voice: the mic button back to LG (ADR 0013) -------------------
# The daemon takes LG's voice key with an in-memory configd override
# (com.webos.keyaction.voice = {"action":"ignore"}, "volatile": true) while
# the mic button runs, and writes run/lg_voice_override before it does. This
# removes the override (null, volatile) and confirms LG's "launch" with a read
# of that one config name; only then is the state file removed (if unchanged
# since the start: a token), all under the lock shared with the daemon. It never writes
# without "volatile" and never reads a whole category. Repeatable: without
# the state file it does nothing. Refused while the daemon runs (it holds the
# key; turn the mic button off instead). Called by stop, restart, uninstall,
# purge, the config restart, and by the supervisor after every daemon exit.
# A reboot clears the override by itself.
LG_SET=luna://com.webos.service.config/setConfigs
LG_GET=luna://com.webos.service.config/getConfigs
LG_NULL='{"configs":{"com.webos.keyaction.voice":null},"volatile":true}'
LG_KEY='{"configNames":["com.webos.keyaction.voice"]}'

lg_emit() { # RESULT LG_VOICE_KEY ERROR
    printf '{"schema":1,"restore":%s,"lg_voice_key":%s,"error":%s}\n' "$(json_str "$1")" "$(json_str "$2")" "$(json_str "$3")"
}

# Prints the result line; status 0 when LG has the key (or nothing to do).
# The lock shared with the daemon's acquire and release (flock(2) on
# run/lg_voice.lock, held through file descriptor 9 until lg_unlock): a
# restore never interleaves with an acquire, by the supervisor, by hand or by
# a stop. python3 (on the TVs) or flock(1) takes it; a lock that cannot be
# taken in 30 s fails the restore (nothing is changed).
lg_lock() {
    mkdir -p "$P/run" 2>/dev/null || return 1
    exec 9>>"$P/run/lg_voice.lock" || return 1
    if command -v python3 >/dev/null 2>&1; then
        python3 -c 'import fcntl, sys, time
end = time.time() + 30
while True:
    try:
        fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)
        sys.exit(0)
    except OSError:
        if time.time() > end:
            sys.exit(1)
        time.sleep(0.1)' && return 0
        return 1
    fi
    if command -v flock >/dev/null 2>&1; then
        flock -x 9 && return 0
    fi
    return 1
}

lg_unlock() { exec 9>&-; }

# The action of com.webos.keyaction.voice in a getConfigs reply (one line):
# only from that key's object; empty when it is missing (missingConfigs),
# null or not readable.
lg_action() {
    printf '%s' "$1" | sed -n 's/.*"com\.webos\.keyaction\.voice"[[:space:]]*:[[:space:]]*{[^}]*"action"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1
}

# Removes the override and confirms "launch" (LG's base value; anything else,
# a missing key or an unreadable reply, is no success). The state file goes
# only then, and only if it still holds what was read at the start (a newer
# acquire's file stays). Prints the result line; status 0 when LG has the key.
lg_restore() {
    sf=$P/run/$LG_VOICE_STATE
    if [ ! -f "$sf" ]; then
        lg_emit nothing lg ""
        return 0
    fi
    if [ ! -x "$(command -v "$LUNA" 2>/dev/null)" ]; then
        lg_emit failed unknown "luna-send not found"
        return 1
    fi
    if ! lg_lock; then
        lg_emit failed unknown "the lock run/lg_voice.lock is held by an acquire or another restore"
        return 1
    fi
    if [ ! -f "$sf" ]; then
        lg_unlock
        lg_emit nothing lg ""
        return 0
    fi
    tok=$(cat "$sf" 2>/dev/null)
    seterr=
    luna_call "$LG_SET" "$LG_NULL" || seterr="setConfigs: $LUNA_ERR; "
    if luna_call "$LG_GET" "$LG_KEY"; then
        act=$(lg_action "$LUNA_REPLY")
        if [ "$act" = launch ]; then
            if [ "$(cat "$sf" 2>/dev/null)" = "$tok" ]; then
                rm -f "$sf"
            else
                note "run/$LG_VOICE_STATE changed during the restore (a newer acquire); kept"
            fi
            lg_unlock
            note "LG's voice key given back (action launch)"
            lg_emit done lg ""
            return 0
        fi
        lg_unlock
        lg_emit failed unknown "${seterr}the key reads ${act:-not set} after the restore, not launch; try again or reboot the TV"
        return 1
    fi
    lg_unlock
    lg_emit failed unknown "${seterr}getConfigs: $LUNA_ERR; try again or reboot the TV"
    return 1
}

# After a stop in this script: LG's key back, quietly (the line goes to
# stderr), never failing the caller.
lg_restore_after_stop() { # [removing]: uninstall or purge (ctl.sh goes away)
    [ -f "$P/run/$LG_VOICE_STATE" ] || return 0
    if ! lg_restore >&2; then
        if [ "${1:-}" = removing ]; then
            note "LG's voice key could not be given back: reboot the TV to give the mic button back to LG"
        else
            note "LG's voice key could not be given back now: run ctl.sh restore-lg-voice or reboot the TV"
        fi
    fi
    return 0
}

cmd_restore_lg_voice() {
    need_root
    # The daemon itself (the supervisor calls this between two daemon runs).
    if is_ours "$(read_pid "$PIDFILE")"; then
        lg_emit refused "$(lg_voice_key_word)" "the satellite is running: turn the mic button off (ctl.sh config mic-button off) or stop it first"
        return 1
    fi
    lg_restore
}

# --- config aec|music|mic-button: one config key and a restart (ADR 0011,
# ADR 0010, docs/design/remote-mic-button.md) ---
# [aec] enabled, [music] enabled and [remote] mic_button are read by the
# daemon at start only, so
# a change rewrites etc/config.toml and restarts a running daemon. The new
# config is built in a temporary file next to it (only the one key's line
# changes, or one line or table is added; comments, other keys and other
# tables are copied byte for byte), checked with the daemon's own
# --check-config, and renamed over the config only when the check passes and
# the daemon reports the requested state. A daemon that does not come back
# with the new config gets the previous config back and is started again.

# The daemon's view of config file $1: prints the --check-config summary line
# (without "config ok: "); when the check fails, its last output line and
# status 1.
cfg_check() {
    co=$("$BIN" --config "$1" --check-config 2>&1) && crc=0 || crc=$?
    cs=$(printf '%s\n' "$co" | sed -n 's/^config ok: //p' | head -n 1)
    if [ "$crc" -eq 0 ] && [ -n "$cs" ]; then
        printf '%s\n' "$cs"
        return 0
    fi
    printf '%s\n' "$co" | sed '/^[[:space:]]*$/d' | tail -n 1 | cut -c 1-200
    return 1
}

# on or off for setting $1 (aec | music | mic-button) in summary line $2, ""
# when the line does not say. "aec=[off (file mic without ...)]" is enabled.
cfg_state() {
    case "$1 $2" in
    "aec "*" aec=[off]"*) echo off ;;
    "aec "*" aec=["*) echo on ;;
    "music "*" music=[off]"*) echo off ;;
    "music "*" music=[on"*) echo on ;;
    "mic-button "*" remote=[mic_button=off"*) echo off ;;
    "mic-button "*" remote=[mic_button=ptt"*) echo on ;;
    "overlay "*" overlay=[off]"*) echo off ;;
    "overlay "*" overlay=[on"*) echo on ;;
    *) echo "" ;;
    esac
}

# The table, key and TOML value that put setting $1 (aec | music |
# mic-button) to $2 (on | off): "TABLE KEY VALUE". The values are fixed
# literals without spaces (a quoted mic_button word is one token).
cfg_target() {
    case "$1 $2" in
    "mic-button on") echo 'remote mic_button "ptt"' ;;
    "mic-button off") echo 'remote mic_button "off"' ;;
    *" on") echo "$1 enabled true" ;;
    *) echo "$1 enabled false" ;;
    esac
}

# Writes $CFG with `$2 = $3` in table [$1] to file $4: the key's line is
# changed in place (indent and trailing comment kept), or the line is added
# right under the table's header, or the table is appended at the end.
cfg_write_key() { # TABLE KEY VALUE OUT
    hre=$(cfg_header_re "$1")
    if [ -n "$(cfg_section_value "$1" "$2")" ]; then
        sed "/$hre/,/^[[:space:]]*\\[/s/^\\([[:space:]]*\\)$2[[:space:]]*=[[:space:]]*[^#[:space:]]*/\\1$2 = $3/" "$CFG" >"$4"
    elif grep -q "$hre" "$CFG"; then
        sed "/$hre/a\\
$2 = $3" "$CFG" >"$4"
    else
        {
            cat "$CFG"
            [ -z "$(tail -c 1 "$CFG")" ] || echo
            printf '\n[%s]\n%s = %s\n' "$1" "$2" "$3"
        } >"$4"
    fi
}

# Stops the daemon (and a phone setup session) and starts it supervised;
# status 1 when it is not running afterwards. Notes go to stderr.
cfg_restart() {
    (stop_daemon 10) >&2 || return 1
    lg_restore_after_stop
    sh "$P/bin/start.sh" --supervise >&2 || return 1
}

# The one JSON line (schema 1) and a note on stderr. REASON is cooldown or
# busy for a refusal by a guard, else empty; RETRY is a number of seconds
# (cooldown) or empty (null). Both are appended last.
cfg_emit() { # SETTING REQUESTED BEFORE AFTER RESULT ERROR DAEMON [REASON [RETRY]]
    printf '{"schema":1,"setting":%s,"requested":%s,"before":%s,"after":%s,"result":%s,"daemon":%s,"error":%s,"reason":%s,"retry_in_s":%s}\n' \
        "$(json_str "$1")" "$(json_str "$2")" "$(json_str "$3")" "$(json_str "$4")" "$(json_str "$5")" "$(json_str "$7")" "$(json_str "$6")" \
        "$(json_str "${8:-}")" "${9:-null}"
    case $5 in
    applied) echo "$1 is now $4 (was $3; daemon: $7)" >&2 ;;
    already) echo "$1 is already $4; nothing changed" >&2 ;;
    *) echo "$1: $5${6:+: $6}" >&2 ;;
    esac
    case $5 in applied | already) return 0 ;; *) return 1 ;; esac
}

# The cooldown between two config changes, whoever asks (the TV app, Home
# Assistant's switch, a shell): a change restarts the daemon, which ends a
# phone setup session and interrupts listening. The stamp holds the unix time
# of the last change, written just before the config is replaced. It blocks
# while t <= now+30 and now < t+30; a missing, garbled or far-future stamp
# (a clock set back) blocks nothing, so a wrong file cannot lock the
# settings for long.
CFG_COOLDOWN=30

# Prints the seconds left of the cooldown (0 when it does not run).
cfg_cooldown_left() {
    t=
    IFS= read -r t 2>/dev/null <"$RUN_DIR/config-change.stamp" || true
    case $t in '' | 0* | *[!0-9]*) echo 0; return 0 ;; esac
    [ ${#t} -le 12 ] || { echo 0; return 0; }
    now=$(date +%s)
    case $now in '' | *[!0-9]*) echo 0; return 0 ;; esac
    if [ "$t" -le $((now + CFG_COOLDOWN)) ] && [ "$now" -lt $((t + CFG_COOLDOWN)) ]; then
        echo $((t + CFG_COOLDOWN - now))
    else
        echo 0
    fi
}

# Writes the stamp (now, mode 0600) through a temporary file and a rename.
cfg_stamp_write() {
    sf=$RUN_DIR/config-change.stamp
    mkdir -p "$RUN_DIR" 2>/dev/null || return 1
    now=$(date +%s)
    case $now in '' | *[!0-9]*) return 1 ;; esac
    if (umask 077 && echo "$now" >"$sf.new.$$") && mv -f "$sf.new.$$" "$sf"; then
        chmod 600 "$sf" 2>/dev/null || true
        return 0
    fi
    rm -f "$sf.new.$$"
    return 1
}

# Prints voice, music or alarm when the running daemon reports that activity
# in a fresh run/voice.json (the daemon's one definition of busy). A missing,
# stale or null field, a daemon that is not running or one that does not
# write the field yet (an older version) prints nothing: proceed.
cfg_busy() {
    running && voice_fresh || return 0
    bz=$(voice_field busy)
    case $bz in voice | music | alarm) echo "$bz" ;; esac
    return 0
}

# Who asked, for the history line: the daemon sets WEBOS_SAT_SOURCE=ha when
# it runs this for Home Assistant's switch (ADR 0012); anything else is ctl.
case ${WEBOS_SAT_SOURCE:-} in
ha) CFG_SOURCE=ha ;;
*) CFG_SOURCE=ctl ;;
esac

cmd_config_set() { # aec|music|mic-button on|off, already validated
    st=$1
    want=$2
    # shellcheck disable=SC2046 # three fixed words
    set -- $(cfg_target "$st" "$want")
    tb=$1 key=$2 v=$3
    installed || die "not installed (ctl.sh install)"
    locked
    [ -f "$CFG" ] || { cfg_emit "$st" "$want" "" "" refused "no config file" none; return; }
    if ! sum=$(cfg_check "$CFG"); then
        cfg_emit "$st" "$want" "" "" refused "the config does not pass the check now: $sum" none
        return
    fi
    before=$(cfg_state "$st" "$sum")
    [ -n "$before" ] || { cfg_emit "$st" "$want" "" "" refused "the daemon does not report this setting" none; return; }
    # The mic button is on only while it runs (LG's key taken over): a
    # running daemon that reports it not running (the takeover failed) makes
    # "on" a retry (the same config, a restart), and "off" still writes off.
    eff=$before
    if [ "$st" = mic-button ] && running && voice_fresh; then
        case $(voice_field mic_button) in
        ptt) eff=on ;;
        off | failed) eff=off ;;
        esac
    fi
    if [ "$before" = "$want" ] && [ "$eff" = "$want" ]; then
        cfg_emit "$st" "$want" "$before" "$before" already "" none
        return
    fi
    before=$eff
    left=$(cfg_cooldown_left)
    if [ "$left" -gt 0 ]; then
        ago=$((CFG_COOLDOWN - left))
        [ "$ago" -ge 0 ] || ago=0
        cfg_emit "$st" "$want" "$before" "$before" refused "the last change was $ago s ago; try again in $left s" none cooldown "$left"
        return
    fi
    new=$CFG.new.$$
    prev=$CFG.prev.$$
    err=
    if ! (umask 077 && cfg_write_key "$tb" "$key" "$v" "$new"); then
        err="cannot write the config"
    elif ! sum=$(cfg_check "$new"); then
        err="the changed config did not pass the check, nothing changed: $sum"
    elif [ "$(cfg_state "$st" "$sum")" != "$want" ]; then
        err="the config could not be changed automatically, nothing changed: set [$tb] $key = $v by hand"
    elif ! chmod 600 "$new" || ! cp -p "$CFG" "$prev"; then
        err="cannot keep a copy of the config"
    fi
    if [ -n "$err" ]; then
        rm -f "$new" "$prev"
        history_add "$st result from=$before to=$want read=$before outcome=failed source=$CFG_SOURCE" || true
        cfg_emit "$st" "$want" "$before" "$before" failed "$err" none
        return
    fi
    bz=$(cfg_busy)
    if [ -n "$bz" ]; then
        rm -f "$new" "$prev"
        case $bz in
        voice) err="a voice request is in progress" ;;
        music) err="music is loaded (playing or paused)" ;;
        *) err="a timer alarm is active" ;;
        esac
        cfg_emit "$st" "$want" "$before" "$before" refused "$err" none busy
        return
    fi
    # The stamp goes first, so a restart that follows cannot lose it. Without
    # it the cooldown is void, so a stamp that cannot be written stops here.
    if ! cfg_stamp_write; then
        err="cannot write the cooldown file"
    elif ! mv -f "$new" "$CFG"; then
        err="cannot replace the config"
        # Nothing changed, so the cooldown must not run either.
        rm -f "$RUN_DIR/config-change.stamp"
    fi
    if [ -n "$err" ]; then
        rm -f "$new" "$prev"
        history_add "$st result from=$before to=$want read=$before outcome=failed source=$CFG_SOURCE" || true
        cfg_emit "$st" "$want" "$before" "$before" failed "$err" none
        return
    fi
    res=applied
    after=$want
    d="not running"
    if running; then
        if cfg_restart; then
            d=restarted
        else
            # Back to the config the daemon ran with, and start it again.
            mv -f "$prev" "$CFG"
            res=failed
            after=$before
            err="the satellite did not start with the new setting; the previous setting is back"
            d=failed
            if cfg_restart; then d=restarted; fi
        fi
    fi
    rm -f "$prev"
    history_add "$st result from=$before to=$want read=$after outcome=$res source=$CFG_SOURCE" || true
    cfg_emit "$st" "$want" "$before" "$after" "$res" "$err" "$d"
}

cmd_set() {
    [ $# -eq 2 ] || usage
    case "$1 $2" in
    "always-ready on" | "always-ready off" | "hi-lg on" | "hi-lg off")
        if [ "$(id -u)" != 0 ] && [ "${WEBOS_SAT_ALLOW_NONROOT:-}" != 1 ]; then
            set_emit "$1" "$2" "" "" refused "not root"
            return
        fi
        cmd_set_lg "$1" "$2"
        ;;
    "remote-off normal" | "remote-off keep")
        need_root
        cmd_set_remote_off "$2"
        ;;
    reply-volume\ *)
        # Whole percent, 0 to 100, no sign, no leading zeros.
        case $2 in
        0 | [1-9] | [1-9][0-9] | 100) ;;
        *) usage ;;
        esac
        need_root
        cmd_set_reply_volume "$2"
        ;;
    *) usage ;;
    esac
}

# config aec|music|mic-button on|off. Not part of `set` on purpose: it restarts the
# daemon, which ends a phone setup session, and nothing the setup server may
# run reaches that (review C7, tests/no_self_stop.py).
cmd_config() {
    [ $# -eq 2 ] || usage
    case "$1 $2" in
    "aec on" | "aec off" | "music on" | "music off" | "mic-button on" | "mic-button off") ;;
    "overlay on" | "overlay off") ;;
    *) usage ;;
    esac
    need_root
    cmd_config_set "$1" "$2"
}

# --- setup (docs/design/phone-setup.md, ADR 0009) ------------------------------
# The phone setup web server `webos-satellite-setup` is started only here, on
# an explicit call (the app's "Set up with your phone" button, or by hand),
# never by the boot hook or `ctl.sh start`. It makes its own one-time token
# and writes it to $SETUP_DIR/state.json (volatile, mode 0700): nothing
# secret is passed to it, and nothing reaches an argument, the environment,
# a log or the flash. Files (design 4.1): setup.pid, state.json (the server),
# heartbeat (touched by `setup status`), answer.json (`setup answer`).

# The state JSON for "no session" and for states ctl.sh itself found
# (STATE REASON), with every key of the server's state.json.
setup_json() { # STATE REASON: fixed words of this script, no escaping needed
    printf '{"schema":1,"state":"%s","reason":"%s","port":null,"expires_in_s":null,"peer":null,"peer_other_subnet":null,"pending":null,"last_result":null}\n' "$1" "$2"
}

# Prints a file with shell built-ins (no cat: the status poll runs once a second).
print_file() {
    while IFS= read -r pf_line || [ -n "$pf_line" ]; do
        printf '%s\n' "$pf_line"
    done <"$1"
}

# The top-level "state" word of state.json ("" when unreadable). Not by
# position in the text: pending and last_result are objects of their own and
# may carry a "state" too. python3 (json) when there is one; otherwise the
# nested objects, which are flat, are cut out first and the first "state"
# left is the top-level one.
setup_state_word() {
    w=
    if command -v python3 >/dev/null 2>&1; then
        w=$(python3 -c 'import json, sys
try:
    d = json.load(open(sys.argv[1]))
    print(d.get("state", "") if isinstance(d, dict) else "")
except Exception:
    pass' "$SETUP_DIR/state.json" 2>/dev/null) || w=
    else
        w=$(sed -e 's/{[^{}]*}//g' "$SETUP_DIR/state.json" 2>/dev/null |
            sed -n 's/.*"state"[[:space:]]*:[[:space:]]*"\([a-z_-]*\)".*/\1/p' | head -n 1)
    fi
    case $w in *[!a-z_-]*) w= ;; esac
    echo "$w"
}

# One state.json, the server's content while it runs. The poll path (a live
# server, or one that said "ended") only reads and prints: built-ins, no
# luna, no daemon, no `ctl.sh status`. A server that is gone without having
# written "ended" has crashed: state.json (which may still hold the one-time
# path) is replaced by an ended state with reason "crashed".
setup_print_state() {
    if setup_alive; then
        if [ -s "$SETUP_DIR/state.json" ]; then
            print_file "$SETUP_DIR/state.json"
        else
            setup_json starting ""
        fi
        return 0
    fi
    if [ ! -e "$SETUP_DIR/state.json" ] && [ ! -e "$SETUP_PID" ]; then
        setup_json none ""
        return 0
    fi
    st=
    if [ -f "$SETUP_DIR/state.json" ]; then IFS= read -r st <"$SETUP_DIR/state.json" || true; fi
    # The server's own end marker: the compact JSON it writes ("state" of
    # pending is only pending or running, so this text is the top-level one).
    case $st in
    *'"state":"ended"'*) print_file "$SETUP_DIR/state.json"; return 0 ;;
    esac
    if setup_json ended crashed >"$SETUP_DIR/state.json.new.$$" 2>/dev/null; then
        mv -f "$SETUP_DIR/state.json.new.$$" "$SETUP_DIR/state.json"
    else
        rm -f "$SETUP_DIR/state.json.new.$$"
    fi
    rm -f "$SETUP_DIR/answer.json" "$SETUP_PID"
    note "the setup server ended without saying so (see $SETUP_LOG)" >&2
    if [ -s "$SETUP_DIR/state.json" ]; then
        print_file "$SETUP_DIR/state.json"
    else
        setup_json ended crashed
    fi
}

cmd_setup_start() {
    need_root
    installed || die "not installed (ctl.sh install)"
    [ -x "$SETUP_BIN" ] || die "the setup server is missing: $SETUP_BIN (reinstall the package)"
    locked
    if setup_alive; then
        case $(setup_state_word) in
        waiting) # the app was reopened: hand out the same session again
            touch "$SETUP_DIR/heartbeat" 2>/dev/null || true
            setup_print_state
            return 0
            ;;
        *) die "a phone setup session is already active (setup stop ends it)" ;;
        esac
    fi
    # A leftover directory (a finished or crashed session) is replaced.
    rm -rf "${SETUP_DIR:?}"
    (umask 077 && mkdir -p "$(dirname "$SETUP_DIR")" && mkdir -m 700 "$SETUP_DIR" && : >"$SETUP_DIR/heartbeat") ||
        die "cannot create $SETUP_DIR"
    mkdir -p "$P/log"
    if [ -f "$SETUP_LOG" ] && [ "$(wc -c <"$SETUP_LOG" 2>/dev/null || echo 0)" -gt 65536 ]; then
        (umask 077 && : >"$SETUP_LOG")
    fi
    # Own session, no inherited descriptors (a Homebrew Channel exec waits for
    # them), no stdin, the server's own log. The environment is empty except
    # for the session directory, the install directory and a fixed PATH
    # (env -i): nothing from the caller (a Homebrew Channel exec, a shell)
    # reaches the server or the root commands it starts. Tools are named by
    # their full path where found; env runs the command itself (no fork), so
    # $! is the server's pid.
    close_inherited_fds "$0"
    sbin=nohup
    if command -v nohup >/dev/null 2>&1; then sbin=$(command -v nohup); fi
    ssid=
    if [ -n "$SETSID" ]; then
        ssid=$SETSID
        if command -v "$SETSID" >/dev/null 2>&1; then ssid=$(command -v "$SETSID"); fi
    fi
    (
        umask 077
        # shellcheck disable=SC2086 # $ssid is empty or one word
        env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin WEBOS_SAT_SETUP_DIR="$SETUP_DIR" WEBOS_SAT_PREFIX="$P" \
            $ssid "$sbin" "$SETUP_BIN" --ctl "$P/bin/ctl.sh" </dev/null >>"$SETUP_LOG" 2>&1 &
        echo $! >"$SETUP_PID"
    )
    i=0
    while [ "$i" -lt 25 ]; do
        if [ -s "$SETUP_DIR/state.json" ] && [ "$(setup_state_word)" = waiting ]; then
            setup_print_state
            return 0
        fi
        # Gone? Only the pid decides here: right after the fork the process
        # is still a copy of this script until it execs the server, so its
        # command line does not name the server yet (setup_alive would say no).
        sp=
        if [ -f "$SETUP_PID" ]; then read -r sp <"$SETUP_PID" || true; fi
        case $sp in '' | *[!0-9]*) ;; *) kill -0 "$sp" 2>/dev/null || break ;; esac
        sleep 0.2 2>/dev/null || sleep 1
        i=$((i + 1))
    done
    err=$(tail -n 3 "$SETUP_LOG" 2>/dev/null | tr '\n' ' ') || err=
    stop_setup_server
    die "the setup server did not start${err:+: $err}"
}

cmd_setup_status() {
    [ $# -eq 1 ] && [ "$1" = --json ] || usage
    # The app polls this once a second through exec and it is the heartbeat,
    # so it stays light (EXP-012: `ctl.sh status --json` costs about 950 ms
    # and 0.57 CPU-seconds on c3-a, `--help` 80 ms): no root check (a
    # non-root caller cannot read the 0700 directory and gets "none"), no
    # luna, no daemon, no touch/cat/tr/grep processes; the heartbeat is a
    # truncating write of the empty file, which sets its mtime.
    if setup_alive; then
        : >"$SETUP_DIR/heartbeat" 2>/dev/null || true
    fi
    setup_print_state
}

cmd_setup_answer() {
    [ $# -eq 2 ] || usage
    case $1 in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
    *) usage ;;
    esac
    case $2 in allow | refuse) ;; *) usage ;; esac
    need_root
    setup_alive || die "no phone setup session is running"
    (
        umask 077
        printf '{"id":"%s","verdict":"%s"}\n' "$1" "$2" >"$SETUP_DIR/answer.json.new.$$" &&
            mv -f "$SETUP_DIR/answer.json.new.$$" "$SETUP_DIR/answer.json"
    ) || { rm -f "$SETUP_DIR/answer.json.new.$$"; die "cannot write the answer"; }
    note "answer $2 recorded for request $1; the server reads it within a second"
}

cmd_setup_stop() {
    need_root
    # stdout is the JSON alone: the notes go to stderr.
    setup_alive || note "no phone setup session was running" >&2
    stop_setup_server >&2
    setup_json none ""
}

cmd_setup() {
    [ $# -ge 1 ] || usage
    sub=$1
    shift
    case $sub in
    start) [ $# -eq 0 ] || usage; cmd_setup_start ;;
    status) cmd_setup_status "$@" ;;
    answer) cmd_setup_answer "$@" ;;
    stop) [ $# -eq 0 ] || usage; cmd_setup_stop ;;
    *) usage ;;
    esac
}

[ $# -ge 1 ] || usage
cmd=$1
shift
case $cmd in
status) cmd_status "$@" ;;
install | update) cmd_install "$@" ;;
start) [ $# -eq 0 ] || usage; cmd_start ;;
stop) [ $# -eq 0 ] || usage; cmd_stop ;;
restart)
    [ $# -eq 0 ] || usage
    need_root
    installed || die "not installed"
    locked
    stop_daemon 10
    lg_restore_after_stop
    sh "$P/bin/start.sh" --supervise
    ;;
autostart) cmd_autostart "$@" ;;
mic) cmd_mic "$@" ;;
pair) [ $# -eq 0 ] || usage; cmd_trigger pair ;;
reset-pairing) [ $# -eq 0 ] || usage; cmd_trigger reset-pairing ;;
report) cmd_report "$@" ;;
selfcheck) [ $# -eq 0 ] || usage; cmd_selfcheck ;;
ack-firmware) [ $# -eq 0 ] || usage; cmd_ack_firmware ;;
tv-settings) cmd_tv_settings "$@" ;;
set) cmd_set "$@" ;;
config) cmd_config "$@" ;;
setup) cmd_setup "$@" ;;
clear-crashloop) [ $# -eq 0 ] || usage; need_root; rm -f "$CRASHLOOP"; note "crash-loop marker removed" ;;
uninstall) [ $# -eq 0 ] || usage; cmd_uninstall ;;
purge) [ $# -eq 0 ] || usage; cmd_purge ;;
restore-lg-voice) [ $# -eq 0 ] || usage; cmd_restore_lg_voice ;;
-h | --help) usage ;;
*) usage ;;
esac
