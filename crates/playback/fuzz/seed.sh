#!/bin/sh
# Seeds the stream_decoder corpus from the synthetic test fixtures. Each
# seed gets the two control bytes of the target (hint 0 = none, read size
# 0 = 256 KiB) in front of the file.
#
#   sh crates/playback/fuzz/seed.sh
set -eu
cd "$(dirname "$0")"
mkdir -p corpus/stream_decoder corpus/redirect_location
for f in ../tests/fixtures/*.flac ../tests/fixtures/*.wav ../tests/fixtures/*.mp3 ../tests/fixtures/*.ogg; do
  { printf '\000\000'; cat "$f"; } > "corpus/stream_decoder/$(basename "$f")"
  # The same with 64-byte reads (as from a slow socket) and a format hint.
  { printf '\001\001'; cat "$f"; } > "corpus/stream_decoder/short-$(basename "$f")"
done
printf 'http://ha.lan:8123/api/esphome/ffmpeg_proxy/d/x.flac\n/other?t=1' > corpus/redirect_location/abs-path
printf 'https://ma.lan:8097/flow/q.flac\n//cdn.lan/x' > corpus/redirect_location/scheme-relative
printf 'https://h/a/b\nhttp://h/c' > corpus/redirect_location/downgrade
ls corpus/stream_decoder | wc -l
