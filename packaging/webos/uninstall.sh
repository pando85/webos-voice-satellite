#!/bin/sh
# Stops the daemon and removes the whole prefix directory. Refuses anything
# that is not a webos-satellite install (marker file), the root directory, an
# empty path, a relative path, a symbolic link or a system directory.
#
# usage: uninstall.sh [--prefix DIR]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

prefix_arg=
while [ $# -gt 0 ]; do
    case $1 in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix_arg=$2; shift 2 ;;
    -h | --help) sed -n '2,8p' "$0"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
done
resolve_prefix "$0" "$prefix_arg"
require_installed
stop_daemon 10
lg_restore_after_stop_script removing
# This script lives inside the prefix; the shell keeps its open file, so it
# can remove its own directory.
cd /
rm -rf "$PREFIX"
note "removed $PREFIX"
