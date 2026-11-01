#!/bin/sh
# Builds the fake_device example and runs the fake-HA harness against it.
# Usage (from anywhere): tools/esphome-fake-ha/run.sh
# Creates tools/esphome-fake-ha/.venv (gitignored) with aioesphomeapi==46.6.0
# on first use. No HA, TV or network access beyond 127.0.0.1.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
VENV="$HERE/.venv"
: "${CARGO_TARGET_DIR:=$ROOT/target/w-esphome}"
export CARGO_TARGET_DIR

if [ ! -x "$VENV/bin/python" ]; then
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet -r "$HERE/requirements.txt"
fi

(cd "$ROOT" && cargo build --quiet -p webos-esphome-api --example fake_device)
exec "$VENV/bin/python" -I "$HERE/fake_ha.py" --device "$CARGO_TARGET_DIR/debug/examples/fake_device" "$@"
