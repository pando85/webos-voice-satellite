//! M3b: the stream decoder on corrupted, truncated and garbage input must
//! never panic and never hang (the release profile has `panic = "abort"`,
//! so a decoder panic on network data would kill the daemon). This is the
//! deterministic mutation harness that runs in `cargo test`; the coverage
//! guided fuzz target is `crates/playback/fuzz`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    missing_docs
)]

mod common;

use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use common::{flac_bytes, noise, s16le, sine, wav_stream_header};
use webos_playback::stream::{
    self, GAP_BUDGET, MAX_CONSECUTIVE_ERRORS, StreamConfig, StreamDecoder,
};
use webos_playback::{PlaybackError, wav_bytes};

/// Longest a single decode of a small input may take (debug build).
const HANG: Duration = Duration::from_secs(10);

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let mut wav_stream = wav_stream_header(22_050, 1);
    wav_stream.extend_from_slice(&s16le(&sine(22_050, 1, 300.0, 0.1).samples));
    vec![
        ("flac-own", flac_bytes(&noise(44_100, 2, 6000, 5), 1152)),
        ("flac-ffmpeg", fixture("tone-44100-2.flac")),
        ("wav", wav_bytes(&sine(48_000, 2, 440.0, 0.05))),
        ("wav-stream", wav_stream),
        ("mp3-stereo", fixture("tone-44100-2.mp3")),
        ("mp3-mono", fixture("tone-22050-1.mp3")),
        ("mp3-rate-change", fixture("rate-change.mp3")),
        ("ogg", fixture("tone-48000-2.ogg")),
    ]
}

/// Decodes everything; returns the frames and the end (never panics if the
/// decoder is sound; a panic is caught and reported by the caller).
fn run(data: Vec<u8>, hint: Option<&str>) -> (usize, Option<PlaybackError>) {
    let mut frames = 0;
    let mut d = match StreamDecoder::new(Box::new(Cursor::new(data)), hint, 20) {
        Ok(d) => d,
        Err(e) => return (0, Some(e)),
    };
    // A bound on the loop itself: no input here can yield more chunks.
    for _ in 0..1_000_000 {
        match d.next_chunk() {
            Ok(Some(c)) => {
                assert!(matches!(c.channels, 1 | 2));
                assert_eq!(c.samples.len() % usize::from(c.channels), 0);
                frames += c.frames();
            }
            Ok(None) => return (frames, None),
            Err(e) => return (frames, Some(e)),
        }
    }
    panic!("decoder did not finish");
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Runs `data` with every hint; collects panics and slow runs.
fn check(name: &str, data: &[u8], failures: &mut Vec<String>) {
    for hint in [
        None,
        Some("flac"),
        Some("audio/mpeg"),
        Some("wav"),
        Some("ogg"),
    ] {
        let t = Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| run(data.to_vec(), hint)));
        if r.is_err() {
            failures.push(format!("{name} hint {hint:?}: panic"));
        }
        if t.elapsed() > HANG {
            failures.push(format!("{name} hint {hint:?}: took {:?}", t.elapsed()));
        }
    }
}

fn report(failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn truncated_inputs() {
    let mut failures = Vec::new();
    for (name, data) in seeds() {
        let mut cuts: Vec<usize> = (0..data.len().min(64)).collect();
        cuts.extend((1..40).map(|i| data.len() * i / 40));
        for cut in cuts {
            check(&format!("{name} cut {cut}"), &data[..cut], &mut failures);
        }
    }
    report(&failures);
}

#[test]
fn bit_flips_and_overwrites() {
    let mut failures = Vec::new();
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for (name, data) in seeds() {
        for i in 0..60 {
            let mut d = data.clone();
            // Concentrate on the headers, where the parsers branch most.
            let span = if i % 2 == 0 {
                d.len().min(256)
            } else {
                d.len()
            };
            for _ in 0..=rng.below(8) {
                let p = rng.below(span);
                match rng.below(4) {
                    0 => d[p] ^= 1 << rng.below(8),
                    1 => d[p] = 0xFF,
                    2 => d[p] = 0,
                    _ => d[p] = rng.next() as u8,
                }
            }
            check(&format!("{name} mutation {i}"), &d, &mut failures);
        }
    }
    report(&failures);
}

#[test]
fn splices_and_duplications() {
    let mut failures = Vec::new();
    let mut rng = Rng(0x0dd_ba11_0002);
    let all = seeds();
    for (name, data) in &all {
        for i in 0..20 {
            let (_, other) = &all[rng.below(all.len())];
            let a = rng.below(data.len());
            let b = rng.below(other.len());
            let mut d = data[..a].to_vec();
            d.extend_from_slice(&other[b..]);
            check(&format!("{name} splice {i}"), &d, &mut failures);
            // A repeated header block in the middle of the stream.
            let mut d = data.clone();
            let h = data.len().min(64);
            let at = rng.below(d.len());
            let head = data[..h].to_vec();
            d.splice(at..at, head);
            check(&format!("{name} dup {i}"), &d, &mut failures);
        }
    }
    report(&failures);
}

#[test]
fn garbage_and_degenerate_inputs() {
    let mut failures = Vec::new();
    let mut rng = Rng(42);
    let mut inputs: Vec<(String, Vec<u8>)> = vec![
        ("empty".into(), vec![]),
        ("zeros".into(), vec![0; 100_000]),
        ("ones".into(), vec![0xFF; 100_000]),
        (
            "html".into(),
            b"<html><body>Login required</body></html>".to_vec(),
        ),
        ("flac magic only".into(), b"fLaC".to_vec()),
        ("riff only".into(), b"RIFF\xff\xff\xff\xffWAVE".to_vec()),
        ("ogg page magic".into(), b"OggS\x00\x02".repeat(1000)),
        (
            "mp3 sync words".into(),
            [0xFF, 0xFB, 0x90, 0x64].repeat(5000),
        ),
        (
            "flac sync words".into(),
            [0xFF, 0xF8, 0x69, 0x08].repeat(5000),
        ),
    ];
    for i in 0..20 {
        let n = rng.below(200_000);
        inputs.push((
            format!("random {i}"),
            (0..n).map(|_| rng.next() as u8).collect(),
        ));
    }
    for (name, d) in inputs {
        check(&name, &d, &mut failures);
    }
    report(&failures);
}

/// An endless source of garbage after a valid header must end with an
/// error after at most `GAP_BUDGET` bytes, not be searched forever.
#[test]
fn endless_garbage_ends_within_the_byte_budget() {
    struct Garbage {
        head: Cursor<Vec<u8>>,
        rng: Rng,
        read: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }
    impl Read for Garbage {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.head.read(buf)?;
            let n = if n > 0 {
                n
            } else {
                for b in buf.iter_mut() {
                    *b = self.rng.next() as u8;
                }
                buf.len()
            };
            self.read
                .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
            Ok(n)
        }
    }
    for (name, head) in [
        ("flac", flac_bytes(&sine(48_000, 2, 440.0, 0.1), 1152)),
        ("mp3", fixture("tone-44100-2.mp3")),
        ("ogg", fixture("tone-48000-2.ogg")),
    ] {
        let read = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let src = Garbage {
            head: Cursor::new(head.clone()),
            rng: Rng(7),
            read: std::sync::Arc::clone(&read),
        };
        let mut d = StreamDecoder::new(Box::new(src), None, 20).unwrap();
        let t = Instant::now();
        let end = loop {
            match d.next_chunk() {
                Ok(Some(_)) => {}
                Ok(None) => break None,
                Err(e) => break Some(e),
            }
            assert!(t.elapsed() < Duration::from_secs(60), "{name}: no end");
        };
        let total = read.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!(
            "{name}: {end:?} after {total} bytes, {} skipped",
            d.packets_skipped()
        );
        assert!(
            matches!(end, Some(PlaybackError::Decode(_))),
            "{name}: {end:?}"
        );
        // The budget plus the demuxer's read-ahead and the first packets.
        assert!(
            total < head.len() as u64 + GAP_BUDGET + 256 * 1024,
            "{name}: read {total}"
        );
        assert!(d.packets_skipped() <= u64::from(MAX_CONSECUTIVE_ERRORS) + 1);
    }
}

/// Corrupt data over the network ends the stream cleanly too.
#[test]
fn corrupted_stream_over_http_ends_cleanly() {
    let mut data = fixture("tone-44100-2.flac");
    for i in (200..data.len()).step_by(97) {
        data[i] ^= 0x5A;
    }
    let srv = common::serve_body("audio/flac", data, common::Framing::Chunked);
    let cfg = StreamConfig {
        insecure_allow_loopback: true,
        ..StreamConfig::default()
    };
    let mut s = stream::open(&srv.url("/bad.flac"), &cfg).unwrap();
    let t = Instant::now();
    while let Some(x) = s.blocking_recv() {
        if x.is_err() {
            assert!(s.blocking_recv().is_none());
            break;
        }
    }
    assert!(t.elapsed() < Duration::from_secs(10));
    assert!(s.close(Duration::from_secs(1)));
}

/// cargo-fuzz findings of 2026-11-05 (inputs in `tests/fixtures/*.fuzz`,
/// in the fuzz target's layout: hint byte, read-size byte, stream). A WAV
/// `fmt` chunk with 65282 channels makes symphonia-format-riff 0.6.1
/// (`wave/chunks.rs:100`) multiply two `u16` for a log line: a panic with
/// overflow checks (debug and test builds, cargo-fuzz's default), a
/// harmless wrap in the release profile the daemon ships with (no
/// `overflow-checks`), where the stream then fails with a decode error.
/// This test pins the release behaviour; it must stay green before
/// `overflow-checks` is ever enabled for release.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "panics only with overflow checks; run with cargo test --release"
)]
fn fuzz_findings_do_not_panic_in_release() {
    let mut failures = Vec::new();
    for name in [
        "fuzz-wav-fmt-overflow-1.fuzz",
        "fuzz-wav-fmt-overflow-2.fuzz",
    ] {
        let data = fixture(name);
        check(name, &data[2..], &mut failures);
        let (frames, end) = run(data[2..].to_vec(), Some("wav"));
        assert_eq!(frames, 0, "{name}");
        assert!(
            matches!(end, Some(PlaybackError::Decode(_))),
            "{name}: {end:?}"
        );
    }
    report(&failures);
}
