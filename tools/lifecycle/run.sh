#!/bin/sh
# Host lifecycle checks: builds webos-satellite, then runs
#   1. local_check.py               microphone gate (kill switch, mute release,
#                                   mute persistence) against the real binary
#   2. packaging/webos/tests/run.sh ops scripts (supervisor, crash-loop guard,
#                                   autostart gating, watchdog exits)
# Uses the venv of tools/esphome-fake-ha (created here on first use).
# Everything stays on 127.0.0.1 and in temporary directories.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
FAKE="$ROOT/tools/esphome-fake-ha"
VENV="$FAKE/.venv"
: "${CARGO_TARGET_DIR:=$ROOT/target}"
export CARGO_TARGET_DIR
if [ ! -x "$VENV/bin/python" ]; then
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet -r "$FAKE/requirements.txt"
fi
(cd "$ROOT" && cargo build --quiet -p webos-satellite)
BIN="$CARGO_TARGET_DIR/debug/webos-satellite"
"$VENV/bin/python" -I "$HERE/local_check.py" --binary "$BIN"
sh "$ROOT/packaging/webos/tests/run.sh" "$BIN"
