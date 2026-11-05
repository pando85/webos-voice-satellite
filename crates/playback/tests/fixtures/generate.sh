#!/bin/sh
# Regenerates the synthetic test fixtures of the music stream tests (tones
# only: no recordings, no music). Needs ffmpeg with libmp3lame and libvorbis.
# The committed files were made with ffmpeg n9.0.2; another version may
# change the bytes but not what the tests check.
#
#   sh crates/playback/tests/fixtures/generate.sh
set -eu
cd "$(dirname "$0")"
ff() { ffmpeg -hide_banner -loglevel error -nostdin -y "$@"; }
# 440 Hz (left) and 880 Hz (right) sine tones.
tone() { # rate channels seconds
  if [ "$2" = 1 ]; then
    echo "sine=frequency=440:sample_rate=$1:duration=$3"
  else
    echo "sine=frequency=440:sample_rate=$1:duration=$3[l];sine=frequency=880:sample_rate=$1:duration=$3[r];[l][r]join=inputs=2:channel_layout=stereo"
  fi
}
# No metadata or encoder tags in the files.
common="-map_metadata -1 -fflags +bitexact -flags:a +bitexact"

# FLAC written to a pipe, as HA's ffmpeg proxy does (no seek back, so the
# STREAMINFO has no length and no MD5), and the same PCM as WAV.
ff -f lavfi -i "$(tone 44100 2 0.25)" $common -sample_fmt s16 -f flac pipe: > tone-44100-2.flac
ff -f lavfi -i "$(tone 44100 2 0.25)" $common -sample_fmt s16 -f wav tone-44100-2.wav
# Lossy formats (checked by pitch and level, not bit-exact).
ff -f lavfi -i "$(tone 44100 2 0.5)" $common -c:a libmp3lame -b:a 96k -f mp3 tone-44100-2.mp3
ff -f lavfi -i "$(tone 22050 1 0.5)" $common -c:a libmp3lame -b:a 32k -f mp3 tone-22050-1.mp3
ff -f lavfi -i "$(tone 48000 2 0.5)" $common -c:a libvorbis -q:a 0 -f ogg tone-48000-2.ogg
# A rate change in the middle of a stream: two MP3 streams back to back.
ff -f lavfi -i "$(tone 44100 2 0.3)" $common -c:a libmp3lame -b:a 64k -f mp3 a.tmp
ff -f lavfi -i "$(tone 48000 2 0.3)" $common -c:a libmp3lame -b:a 64k -f mp3 b.tmp
cat a.tmp b.tmp > rate-change.mp3
rm -f a.tmp b.tmp
ls -l
