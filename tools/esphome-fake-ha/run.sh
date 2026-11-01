#!/bin/sh
# Builds the fake_device example and the webos-satellite binary and runs the
# fake-HA harnesses against them:
#   1. fake_ha.py       protocol scenarios against the fake_device example
#   2. satellite_e2e.py the real daemon with a file mic and the wav sink
# Usage (from anywhere): tools/esphome-fake-ha/run.sh [--device-only | --satellite-only]
# Creates tools/esphome-fake-ha/.venv (gitignored) with aioesphomeapi==46.6.0
# on first use. No HA, TV or network access beyond 127.0.0.1.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
VENV="$HERE/.venv"
: "${CARGO_TARGET_DIR:=$ROOT/target/w-esphome}"
export CARGO_TARGET_DIR
WHICH=${1:-all}

if [ ! -x "$VENV/bin/python" ]; then
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet -r "$HERE/requirements.txt"
fi

if [ "$WHICH" != "--satellite-only" ]; then
    (cd "$ROOT" && cargo build --quiet -p webos-esphome-api --example fake_device)
    "$VENV/bin/python" -I "$HERE/fake_ha.py" --device "$CARGO_TARGET_DIR/debug/examples/fake_device"
fi
if [ "$WHICH" != "--device-only" ]; then
    (cd "$ROOT" && cargo build --quiet -p webos-satellite)
    "$VENV/bin/python" -I "$HERE/satellite_e2e.py" --binary "$CARGO_TARGET_DIR/debug/webos-satellite"
fi
