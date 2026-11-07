#!/bin/sh
# Stops the daemon: SIGTERM, wait up to 10 s, then SIGKILL; removes the pidfiles.
#
# usage: stop.sh [--prefix DIR]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    -h | --help) sed -n '2,5p' "$0"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done
resolve_prefix "$0" "$prefix_arg"
require_installed
stop_daemon 10
lg_restore_after_stop_script
note "stopped"
