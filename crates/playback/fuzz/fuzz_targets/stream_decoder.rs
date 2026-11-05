//! The music stream decoder (`StreamDecoder`, the code the `music-decode`
//! thread runs) on arbitrary bytes.
//!
//! Input layout: byte 0 picks the format hint, byte 1 the largest read the
//! source returns (1 to 256 KiB, so short reads as from a socket are
//! covered), the rest is the stream. A panic or a hang is a finding.
#![no_main]

use std::io::{self, Read};

use libfuzzer_sys::fuzz_target;
use webos_playback::stream::StreamDecoder;

/// A source that returns at most `max` bytes per read.
struct Dribble {
    data: Vec<u8>,
    pos: usize,
    max: usize,
}

impl Read for Dribble {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.max).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

const HINTS: [Option<&str>; 8] = [
    None,
    Some("flac"),
    Some("audio/flac"),
    Some("mp3"),
    Some("audio/mpeg"),
    Some("wav"),
    Some("ogg"),
    Some("audio/ogg"),
];

fuzz_target!(|input: &[u8]| {
    if input.len() < 2 {
        return;
    }
    let hint = HINTS[usize::from(input[0]) % HINTS.len()];
    let max = match input[1] {
        0 => 256 * 1024,
        n => usize::from(n) * 64,
    };
    let src = Dribble {
        data: input[2..].to_vec(),
        pos: 0,
        max,
    };
    let Ok(mut d) = StreamDecoder::new(Box::new(src), hint, 20) else {
        return;
    };
    // Finite input: the loop must end by itself; the bound only turns a
    // hang into a crash libFuzzer reports.
    for _ in 0..10_000_000 {
        match d.next_chunk() {
            Ok(Some(c)) => {
                assert!(matches!(c.channels, 1 | 2));
                assert_eq!(c.samples.len() % usize::from(c.channels), 0);
                assert!((8_000..=192_000).contains(&c.rate));
            }
            Ok(None) | Err(_) => return,
        }
    }
    panic!("decoder never finished");
});
