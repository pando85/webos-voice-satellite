# Shared helpers for the webos-satellite ops scripts. Sourced, not executed.
# POSIX sh, BusyBox ash compatible. Everything lives under one prefix
# directory; nothing is written outside it.

MARKER=.webos-satellite-install

die() {
    echo "webos-satellite: $*" >&2
    exit 1
}

note() {
    echo "webos-satellite: $*"
}

# Rejects prefixes that are empty, relative, contain "..", or are a system
# directory. Prints the normalised prefix (no trailing slash).
check_prefix() {
    p=$1
    [ -n "$p" ] || die "empty prefix"
    case $p in
    /*) ;;
    *) die "prefix must be an absolute path: $p" ;;
    esac
    case $p in
    */../* | */.. | */./* | */.) die "prefix must not contain . or .. components: $p" ;;
    esac
    # Strip trailing slashes.
    while [ "${p%/}" != "$p" ] && [ -n "${p%/}" ]; do
        p=${p%/}
    done
    case $p in
    / | /bin | /boot | /dev | /etc | /home | /home/root | /lib | /media | /media/cryptofs | /media/developer | /media/developer/apps | /mnt | /opt | /proc | /root | /run | /sbin | /sys | /tmp | /usr | /var | /var/lib | /var/lib/webosbrew | /var/luna)
        die "refusing to use a system directory as the prefix: $p"
        ;;
    /media/developer/apps/* | /media/cryptofs/* | /var/lib/webosbrew/* | /usr/palm/*)
        die "refusing a prefix inside a directory managed by webOS or the Homebrew Channel: $p"
        ;;
    esac
    [ ! -L "$p" ] || die "prefix is a symbolic link: $p"
    echo "$p"
}

# --- Trust of what root runs (docs/security/install-permissions.md) --------
# Root runs scripts and binaries from the install directory (PREFIX/bin) and,
# to install or update, from the app's payload, which the TV's installer
# manages (its directories are world-writable on c3-a, EXP-025). Before root
# runs or reads such a file, it must be a regular file (not a symbolic link)
# owned by a trusted user and not writable by group or others. A process
# that is not root can rename or replace entries of a world-writable
# directory, but cannot make a file owned by root, so the owner is the test
# that matters.
#
# Trusted owners: root, and the caller's own uid when the caller is not root
# (host tests, a non-root layout). In host tests WEBOS_SAT_TEST_TRUST_UID
# replaces the caller's uid (a file of "another user"); root ignores it.
TRUST_ME=
trust_uid_ok() { # UID
    [ "$1" != 0 ] || return 0
    if [ -z "$TRUST_ME" ]; then
        TRUST_ME=$(id -u)
        [ "$TRUST_ME" = 0 ] || TRUST_ME=${WEBOS_SAT_TEST_TRUST_UID:-$TRUST_ME}
    fi
    [ "$1" = "$TRUST_ME" ]
}

# trust_check KIND PATH [KIND PATH]...: one `stat` for all paths. KIND:
#   f  a file root runs, sources or reads as code or config: refused when it
#      is not a regular file, not owned by a trusted user, or writable by
#      group or others (its content may have changed: reinstall)
#   p  a file of the app package (installer-managed): refused when it is not
#      a regular file or not owned by a trusted user; group/other write is
#      removed and reported (the installer may create it so)
#   d  a directory of ours: refused when not a real directory or not owned by
#      a trusted user; group/other write is removed and reported
#   r  the run directory: as d, and forced to exactly 0700; when it had to be
#      fixed, entries that are symbolic links or not owned by a trusted user
#      are deleted (they may have been planted) and reported
#   a  a directory above ours: refused when not a real directory, not owned
#      by root or a trusted user, or writable by group or others without the
#      sticky bit. Never changed (not ours)
# Status 0 when nothing is refused; otherwise 1 with TRUST_WHY. TRUST_FIXED
# lists what was changed (also on success).
TRUST_WHY=
TRUST_FIXED=
trust_check() {
    TRUST_WHY=
    TRUST_FIXED=
    tc_items=
    tc_paths=
    tc_n=0
    while [ $# -ge 2 ]; do
        case $2 in
        /*) ;;
        *) TRUST_WHY="not an absolute path: $2"; return 1 ;;
        esac
        case $2 in
        *[!A-Za-z0-9._/+-]*) TRUST_WHY="unsupported characters in $2"; return 1 ;;
        esac
        if [ ! -e "$2" ] && [ ! -L "$2" ]; then
            TRUST_WHY="$2 is missing"
            return 1
        fi
        tc_items="$tc_items $1$2"
        tc_paths="$tc_paths $2"
        tc_n=$((tc_n + 1))
        shift 2
    done
    [ "$tc_n" -gt 0 ] || return 0
    # One "uid mode" pair per path, in order; stat does not follow a link.
    # shellcheck disable=SC2086 # checked above: no spaces or glob characters
    tc_out=$(stat -c '%u %A' $tc_paths 2>/dev/null) || {
        TRUST_WHY="cannot read the owner and mode of:$tc_paths"
        return 1
    }
    set -f
    # shellcheck disable=SC2086
    set -- $tc_out
    set +f
    [ $# -eq $((tc_n * 2)) ] || { TRUST_WHY="unexpected stat output for:$tc_paths"; return 1; }
    for tc_it in $tc_items; do
        tc_k=${tc_it%%/*}
        tc_p=/${tc_it#*/}
        tc_u=$1
        tc_m=$2
        shift 2
        case $tc_u in
        '' | *[!0-9]*) TRUST_WHY="cannot read the owner of $tc_p"; return 1 ;;
        esac
        case $tc_k$tc_m in
        [fp]-*) ;;
        [fp]*) TRUST_WHY="$tc_p is not a regular file (or is a symbolic link)"; return 1 ;;
        [dra]d*) ;;
        *) TRUST_WHY="$tc_p is not a directory (or is a symbolic link)"; return 1 ;;
        esac
        trust_uid_ok "$tc_u" || { TRUST_WHY="$tc_p is owned by uid $tc_u, not root"; return 1; }
        tc_w=0
        case $tc_m in ?????w* | ????????w*) tc_w=1 ;; esac
        case $tc_k in
        f)
            [ "$tc_w" -eq 0 ] || { TRUST_WHY="$tc_p is writable by group or others"; return 1; }
            ;;
        a)
            if [ "$tc_w" -eq 1 ]; then
                case $tc_m in
                ?????????[tT]*) ;;
                *) TRUST_WHY="$tc_p (a parent directory) is writable by other users"; return 1 ;;
                esac
            fi
            ;;
        p | d)
            if [ "$tc_w" -eq 1 ]; then
                chmod go-w "$tc_p" || { TRUST_WHY="cannot remove group/other write from $tc_p"; return 1; }
                TRUST_FIXED="$TRUST_FIXED $tc_p (was $tc_m)"
            fi
            ;;
        r)
            if [ "$tc_m" != drwx------ ]; then
                chmod 700 "$tc_p" || { TRUST_WHY="cannot set $tc_p to 0700"; return 1; }
                TRUST_FIXED="$TRUST_FIXED $tc_p (was $tc_m)"
                run_dir_clean "$tc_p" || return 1
            fi
            ;;
        esac
    done
    return 0
}

# Deletes the entries of directory $1 that are symbolic links or not owned
# by a trusted user (anything planted while it was open to others).
run_dir_clean() {
    for rc_f in "$1"/* "$1"/.[!.]*; do
        [ -e "$rc_f" ] || [ -L "$rc_f" ] || continue
        rc_u=$(stat -c %u "$rc_f" 2>/dev/null) || rc_u=
        if [ -L "$rc_f" ] || ! trust_uid_ok "${rc_u:-x}"; then
            rm -rf "$rc_f" || { TRUST_WHY="cannot delete $rc_f"; return 1; }
            TRUST_FIXED="$TRUST_FIXED deleted $rc_f"
        fi
    done
    return 0
}

# Checks the install under PREFIX (set_paths first) before root runs
# anything from it: the directories above PREFIX (a), PREFIX itself, bin/,
# etc/ and log/ (d), every file in bin/ and etc/config.toml (f), the app-dir
# record and the install marker (p), and run/ (r; made 0700 when missing).
install_trust_check() {
    TRUST_WHY=
    TRUST_FIXED=
    it_pp=$(cd "$PREFIX" 2>/dev/null && pwd -P) || { TRUST_WHY="cannot enter $PREFIX"; return 1; }
    for it_d in "$PREFIX/run" "$PREFIX/log"; do
        [ -e "$it_d" ] || [ -L "$it_d" ] || mkdir -m 700 "$it_d" || { TRUST_WHY="cannot make $it_d"; return 1; }
    done
    set --
    it_a=$it_pp
    while it_a=${it_a%/*} && [ -n "$it_a" ]; do
        set -- "$@" a "$it_a"
    done
    set -- "$@" a / d "$it_pp" d "$PREFIX/bin" d "$PREFIX/etc" d "$PREFIX/log" r "$PREFIX/run"
    for it_f in "$PREFIX"/bin/*; do
        [ -e "$it_f" ] || [ -L "$it_f" ] || continue
        set -- "$@" f "$it_f"
    done
    [ ! -e "$CFG" ] && [ ! -L "$CFG" ] || set -- "$@" f "$CFG"
    # Records, not code (an older ctl.sh wrote app-dir with the caller's umask).
    for it_f in "$PREFIX/app-dir" "$PREFIX/$MARKER"; do
        [ ! -e "$it_f" ] && [ ! -L "$it_f" ] || set -- "$@" p "$it_f"
    done
    trust_check "$@"
}

# The install is trusted (install_trust_check), or the script stops with the
# reason. What had to be fixed is noted.
require_trusted_install() {
    if ! install_trust_check; then
        [ -z "$TRUST_FIXED" ] || note "fixed:$TRUST_FIXED" >&2
        die "refusing to run from $PREFIX: $TRUST_WHY (see docs/security/install-permissions.md: check the TV, then reinstall with ctl.sh install --force, or purge)"
    fi
    [ -z "$TRUST_FIXED" ] || note "fixed the permissions of:$TRUST_FIXED" >&2
}

# Checks the app's files before root runs or copies anything from them: the
# app directory $1 (may be empty: a bare payload tree) and the payload
# directory $2, and every file directly in them. The directories above the
# app directory belong to the TV and are not checked (world-writable on
# c3-a, EXP-025). Group/other write on the app's own files and directories is
# removed (the installer lays them out); a file not owned by root is refused.
payload_trust_check() { # APPDIR PAYLOAD
    pt_app=$1
    pt_pl=$2
    set --
    for pt_d in "$pt_app" "$pt_pl"; do
        [ -n "$pt_d" ] || continue
        set -- "$@" d "$pt_d"
        for pt_f in "$pt_d"/*; do
            [ -e "$pt_f" ] || [ -L "$pt_f" ] || continue
            # Subdirectories (the payload) are not run; a link is refused.
            [ ! -d "$pt_f" ] || [ -L "$pt_f" ] || continue
            set -- "$@" p "$pt_f"
        done
    done
    trust_check "$@"
}

# True if the prefix is on a non-persistent location.
is_volatile_prefix() {
    case $1 in
    /tmp/* | /run/* | /dev/shm/*) return 0 ;;
    esac
    return 1
}

# Sets PREFIX and the file names derived from it (used by the scripts that
# source this file).
# shellcheck disable=SC2034
set_paths() {
    PREFIX=$1
    BIN=$PREFIX/bin/webos-satellite
    CFG=$PREFIX/etc/config.toml
    PIDFILE=$PREFIX/run/webos-satellite.pid
    ROTPID=$PREFIX/run/rotate.pid
    SUPPID=$PREFIX/run/supervise.pid
    LOG=$PREFIX/log/webos-satellite.log
    # Markers at the top of the prefix (persistent with a persistent prefix):
    # DISABLE: do not start (autostart and supervisor honour it);
    # CRASHLOOP: written by supervise.sh when it gave up.
    DISABLE=$PREFIX/disable
    CRASHLOOP=$PREFIX/crashloop
    # The phone setup server (docs/design/phone-setup.md): its session
    # directory is volatile on purpose (the one-time token never reaches the
    # flash). WEBOS_SAT_SETUP_DIR and WEBOS_SAT_SETUP_BIN are for tests.
    SETUP_BIN=${WEBOS_SAT_SETUP_BIN:-$PREFIX/bin/webos-satellite-setup}
    SETUP_DIR=${WEBOS_SAT_SETUP_DIR:-/tmp/webos-satellite-setup}
    SETUP_PID=$SETUP_DIR/setup.pid
    SETUP_LOG=$PREFIX/log/setup.log
}

# Default kill-switch file of the daemon (safety.kill_switch in the config).
# WEBOS_SAT_KILL_SWITCH replaces it in host tests.
# shellcheck disable=SC2034
KILL_SWITCH_DEFAULT=${WEBOS_SAT_KILL_SWITCH:-/tmp/webos-satellite.disable}

# --- Persistent install of the webOS app (ctl.sh) ---------------------------
# The app's files live where the TV's installer puts them and are replaced on
# every update; config, key, logs and the running copy of the daemon live in
# STATE_PREFIX_DEFAULT, outside the app (docs/design/packaging.md).
# shellcheck disable=SC2034
STATE_PREFIX_DEFAULT=/var/lib/webos-satellite
# shellcheck disable=SC2034
INITD_DIR_DEFAULT=/var/lib/webosbrew/init.d
# shellcheck disable=SC2034
HOOK_NAME=webos-satellite
LOCK_DEFAULT=/tmp/webos-satellite.lock

# Serialises install, update, start, stop and uninstall (ctl.sh, autostart.sh).
# The lock is a directory (mkdir is atomic) holding the owner's pid; a lock
# whose owner is gone is taken over. A child of the owner inherits it through
# WEBOS_SAT_LOCK_HELD. Argument: seconds to wait (default 30).
lock_take() {
    LOCKDIR=${WEBOS_SAT_LOCK:-$LOCK_DEFAULT}
    [ "${WEBOS_SAT_LOCK_HELD:-}" != "$LOCKDIR" ] || return 0
    waited=0
    while ! mkdir -m 700 "$LOCKDIR" 2>/dev/null; do
        # The default lock is in /tmp: a directory (or link) there that a
        # user other than root made is not ours to honour or to write into.
        lk_u=$(stat -c %u "$LOCKDIR" 2>/dev/null) || lk_u=
        if [ -L "$LOCKDIR" ] || { [ -n "$lk_u" ] && ! trust_uid_ok "$lk_u"; }; then
            note "removing $LOCKDIR: a link or another user's directory, not a lock of ours" >&2
            rm -rf "$LOCKDIR"
            continue
        fi
        holder=$(read_pid "$LOCKDIR/pid")
        if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
            rm -rf "$LOCKDIR"
            continue
        fi
        waited=$((waited + 1))
        [ "$waited" -lt "${1:-30}" ] || die "another operation holds $LOCKDIR (pid ${holder:-unknown}); if none runs, remove that directory"
        sleep 1
    done
    echo $$ >"$LOCKDIR/pid"
    LOCK_OWNED=1
    WEBOS_SAT_LOCK_HELD=$LOCKDIR
    export WEBOS_SAT_LOCK_HELD
}

lock_release() {
    if [ "${LOCK_OWNED:-0}" = 1 ]; then
        rm -rf "$LOCKDIR"
        LOCK_OWNED=0
    fi
}

# Copies $1 to $2 with mode $3 through a temporary file and a rename, so a
# shell that is running the old $2 keeps reading the old file.
install_file() {
    cp "$1" "$2.new.$$" && chmod "$3" "$2.new.$$" && mv -f "$2.new.$$" "$2"
}

# Closes every descriptor above 2 that this shell inherited, except the one
# it reads its script $1 from. A Homebrew Channel `exec` hands its children
# the Luna bus sockets of its service and the sockets it reads the command's
# output from: a daemon keeping those blocks the exec's reply until the
# daemon exits and can keep a restarted service from registering (HBC's own
# startup.sh closes inherited descriptors for the same reason).
close_inherited_fds() {
    self=$(readlink -f "$1" 2>/dev/null) || self=$1
    for f in /proc/"$$"/fd/*; do
        fd=${f##*/}
        case $fd in
        0 | 1 | 2 | '' | *[!0-9]*) continue ;;
        esac
        t=$(readlink "$f" 2>/dev/null) || continue
        [ "$t" != "$self" ] || continue
        eval "exec $fd>&-" 2>/dev/null || true
    done
}

# setsid when present: started jobs get their own session, so they do not
# depend on the caller's session or process group (a Homebrew Channel exec,
# a boot hook). Used as a plain command word, never through a function: a
# function in the background runs in a forked subshell that stays behind,
# keeps copies of the caller's descriptors and makes $! the subshell's pid.
# In a non-interactive shell a background job is not a group leader, so
# setsid does not fork and $! is the job itself (checked on c3-b).
# shellcheck disable=SC2034 # used by start.sh
SETSID=
if command -v setsid >/dev/null 2>&1; then
    # shellcheck disable=SC2034
    SETSID=setsid
fi

# Picks the prefix: --prefix DIR, or the parent of the directory holding the
# installed script. Call as: resolve_prefix "$0" "$prefix_arg"
resolve_prefix() {
    script=$1
    arg=$2
    if [ -n "$arg" ]; then
        p=$arg
    else
        d=$(cd "$(dirname "$script")" && pwd) || die "cannot resolve the script directory"
        if [ -f "$d/../$MARKER" ]; then
            p=$(cd "$d/.." && pwd)
        else
            die "missing --prefix DIR"
        fi
    fi
    p=$(check_prefix "$p") || exit 1
    set_paths "$p"
}

require_installed() {
    [ -f "$PREFIX/$MARKER" ] || die "$PREFIX is not a webos-satellite install (no $MARKER)"
}

# True if $1 is a live process whose executable is our binary.
is_ours() {
    [ -n "$1" ] || return 1
    kill -0 "$1" 2>/dev/null || return 1
    exe=$(readlink "/proc/$1/exe" 2>/dev/null) || return 1
    [ "$exe" = "$BIN" ]
}

read_pid() {
    pid=
    [ -f "$1" ] && { read -r pid <"$1" || true; }
    case $pid in
    '' | *[!0-9]*) pid= ;;
    esac
    echo "$pid"
}

# True if the sidecar process in $ROTPID is alive (it is a `sh` running rotate.sh).
rotator_alive() {
    rp=$(read_pid "$ROTPID")
    [ -n "$rp" ] || return 1
    kill -0 "$rp" 2>/dev/null || return 1
    tr '\0' ' ' <"/proc/$rp/cmdline" 2>/dev/null | grep -q "rotate.sh"
}

# True if the supervisor in $SUPPID is alive (a `sh` running supervise.sh).
supervisor_alive() {
    sp=$(read_pid "$SUPPID")
    [ -n "$sp" ] || return 1
    kill -0 "$sp" 2>/dev/null || return 1
    tr '\0' ' ' <"/proc/$sp/cmdline" 2>/dev/null | grep -q "supervise.sh"
}

# True if the setup session directory is ours to read and write: a real
# directory owned by a trusted user (ctl.sh setup start makes it, mode
# 0700). It is in /tmp: another user's directory or a link there is never
# read or written (a planted link would make root truncate its target).
# Shell built-ins first (the app polls `setup status` once a second); stat
# only when the built-in test says no or lacks -O. Remembered for the rest
# of this script.
SETUP_DIR_OK=
setup_dir_ok() {
    if [ -z "$SETUP_DIR_OK" ]; then
        SETUP_DIR_OK=no
        if [ -d "$SETUP_DIR" ] && [ ! -L "$SETUP_DIR" ]; then
            # -O (owned by the effective user): BusyBox ash and dash have
            # it; a shell without it fails the test and takes the stat path.
            # shellcheck disable=SC3067
            if [ -z "${WEBOS_SAT_TEST_TRUST_UID:-}" ] && [ -O "$SETUP_DIR" ] 2>/dev/null; then
                SETUP_DIR_OK=yes
            else
                sd=$(stat -c %u "$SETUP_DIR" 2>/dev/null) || sd=
                [ -z "$sd" ] || ! trust_uid_ok "$sd" || SETUP_DIR_OK=yes
            fi
        fi
    fi
    [ "$SETUP_DIR_OK" = yes ]
}

# True if the phone setup server is running: the pid in setup.pid is alive
# and its command line names the server binary.
setup_alive() {
    setup_dir_ok || return 1
    # Shell built-ins only (the app polls this once a second through exec):
    # read the pid, then the command line. A shell drops the NUL bytes
    # between the arguments, which leaves the names in one string.
    sp=
    if [ -f "$SETUP_PID" ]; then read -r sp <"$SETUP_PID" || true; fi
    case $sp in '' | *[!0-9]*) return 1 ;; esac
    kill -0 "$sp" 2>/dev/null || return 1
    cl=
    IFS= read -r cl <"/proc/$sp/cmdline" 2>/dev/null || true
    case $cl in *"${SETUP_BIN##*/}"*) return 0 ;; esac
    # No match: a reused pid, or a shell that keeps only part of the
    # NUL-separated file. The slow way decides (rare).
    tr '\0' ' ' <"/proc/$sp/cmdline" 2>/dev/null | grep -qF "${SETUP_BIN##*/}"
}

# Ends the phone setup server and removes its session directory (token,
# state, answer). A no-op without one. Called by stop_daemon, so stop,
# restart, update, uninstall and purge all end a session.
stop_setup_server() {
    if setup_alive; then
        sp=$(read_pid "$SETUP_PID")
        note "stopping the phone setup server (pid $sp)"
        # The server polls this file every 100 ms and ends the session
        # (state ended, reason done); it has no signal handler, so TERM
        # after 2 s ends it at once.
        ! setup_dir_ok || : >"$SETUP_DIR/stop" 2>/dev/null || true
        i=0
        while setup_alive && [ "$i" -lt 10 ]; do
            sleep 0.2 2>/dev/null || sleep 1
            i=$((i + 1))
        done
        if setup_alive; then
            kill -TERM "$sp" 2>/dev/null || true
            sleep 0.2 2>/dev/null || sleep 1
        fi
        if setup_alive; then
            kill -KILL "$sp" 2>/dev/null || true
        fi
    fi
    case $SETUP_DIR in
    /?*) rm -rf "$SETUP_DIR" 2>/dev/null || true ;;
    esac
    SETUP_DIR_OK=
    return 0
}

# Stops the supervisor (if any), the daemon (TERM, wait, then KILL) and the
# log rotator, and removes the pidfiles. Argument: seconds to wait (default 10).
# Every kill tolerates a process that is already gone: the supervisor
# forwards its TERM to the daemon, which can exit before our own TERM, and
# callers run with `set -e` (found on c3-b, EXP-009).
# Gives LG's voice key back after a stop (ADR 0013) when the daemon left its
# state file: through the installed ctl.sh restore-lg-voice (the one
# implementation, with its lock and checks). For the scripts that stop the
# daemon without ctl.sh (stop.sh, uninstall.sh). $1: "removing" when the
# install goes away afterwards (then only a TV restart can help).
lg_restore_after_stop_script() {
    [ -f "$PREFIX/run/lg_voice_override" ] || return 0
    here_ctl=$(dirname "$BIN")/ctl.sh
    if [ -f "$here_ctl" ] && WEBOS_SAT_PREFIX=$PREFIX sh "$here_ctl" restore-lg-voice >&2; then
        return 0
    fi
    if [ "${1:-}" = removing ]; then
        note "LG's voice key could NOT be given back: reboot the TV to give the mic button back to LG"
    else
        note "LG's voice key could NOT be given back: run ctl.sh restore-lg-voice or reboot the TV"
    fi
    return 0
}

stop_daemon() {
    wait_s=${1:-10}
    stop_setup_server
    pid=$(read_pid "$PIDFILE")
    if supervisor_alive; then
        # The supervisor forwards TERM to the daemon and will not restart it.
        note "stopping supervisor pid $(read_pid "$SUPPID")"
        kill -TERM "$(read_pid "$SUPPID")" 2>/dev/null || true
    fi
    if is_ours "$pid"; then
        note "stopping pid $pid"
        kill -TERM "$pid" 2>/dev/null || true
        i=0
        while is_ours "$pid" && [ "$i" -lt "$wait_s" ]; do
            sleep 1
            i=$((i + 1))
        done
        if is_ours "$pid"; then
            note "pid $pid ignored SIGTERM for ${wait_s}s; sending SIGKILL"
            kill -KILL "$pid" 2>/dev/null || true
            sleep 1
            note "check 'pactl list short sink-inputs': a killed daemon cannot clean up its pacat child"
        fi
        is_ours "$pid" && die "pid $pid is still running"
    else
        note "daemon not running"
    fi
    # The supervisor may be giving LG's voice key back (ctl.sh
    # restore-lg-voice after the daemon's exit, ADR 0013): it may take its
    # lock (up to 30 s) and two luna calls, so it gets 45 s then, 5 s
    # otherwise, before the kill.
    sup_wait=5
    [ ! -f "$PREFIX/run/lg_voice_override" ] || sup_wait=${WEBOS_SAT_RESTORE_WAIT:-45}
    case $sup_wait in '' | *[!0-9]*) sup_wait=45 ;; esac
    i=0
    while supervisor_alive && [ "$i" -lt "$sup_wait" ]; do
        sleep 1
        i=$((i + 1))
    done
    if supervisor_alive; then
        # Its whole process group when it leads one (start.sh starts it with
        # setsid): a child it runs (ctl.sh restore-lg-voice) must not outlive
        # it and race the next start. Otherwise the supervisor alone.
        sp=$(read_pid "$SUPPID")
        pg=$(sed -n 's/^[0-9]* (.*) [A-Za-z] [0-9]* \([0-9]*\) .*/\1/p' "/proc/$sp/stat" 2>/dev/null)
        if [ -n "$pg" ] && [ "$pg" = "$sp" ]; then
            note "supervisor still running; sending SIGKILL to its process group $pg"
            kill -KILL -- "-$pg" 2>/dev/null || kill -KILL "$sp" 2>/dev/null || true
        else
            note "supervisor still running; sending SIGKILL"
            kill -KILL "$sp" 2>/dev/null || true
        fi
    fi
    if rotator_alive; then
        kill -TERM "$(read_pid "$ROTPID")" 2>/dev/null || true
    fi
    rm -f "$PIDFILE" "$ROTPID" "$SUPPID"
}

# --- Binary selection (install.sh) -------------------------------------------
# A package carries webos-satellite-armv7 (static armv7 musl, the build
# verified on c3-a) and webos-satellite-aarch64 (static aarch64 musl), and the
# same two builds of the phone setup server as webos-satellite-setup-armv7 and
# webos-satellite-setup-aarch64.

# Prints the build to try first for `uname -m` output $1. aarch64 means a
# 64-bit kernel: c3-a reports it for every process although its userland is
# 32-bit (P1 runtime smoke), and a static aarch64 binary ran there. armv7l
# (32-bit kernel), armv8l (32-bit personality on a 64-bit kernel) and anything
# unknown get armv7, which runs on both.
arch_for_machine() {
    case $1 in
    aarch64 | arm64) echo aarch64 ;;
    *) echo armv7 ;;
    esac
}

# Rust target triple that `--version` of the build for arch $1 reports.
# Host tests set WEBOS_SAT_TEST_ANY_TRIPLE=1 to accept a host build.
triple_of_arch() {
    if [ "${WEBOS_SAT_TEST_ANY_TRIPLE:-}" = 1 ]; then
        echo ""
        return
    fi
    case $1 in
    aarch64) echo aarch64-unknown-linux-musl ;;
    armv7) echo armv7-unknown-linux-musleabi ;;
    *) echo "" ;;
    esac
}

# True if binary $1 runs here: `--version` exits 0 and prints
# "<name> <version> (<triple>)", with triple $2 when it is given; <name> is
# $3 (default webos-satellite; the phone setup server is webos-satellite-setup).
binary_runs() {
    out=$("$1" --version 2>/dev/null) || return 1
    case $out in
    "${3:-webos-satellite} "*) ;;
    *) return 1 ;;
    esac
    [ -z "${2:-}" ] && return 0
    case $out in
    *"($2)"*) return 0 ;;
    esac
    return 1
}
