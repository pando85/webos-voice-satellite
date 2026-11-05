//! M3a: streaming fetch and decode against local HTTP servers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    missing_docs
)]

mod common;

use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::{
    Framing, flac_bytes, flac_frame, flac_header, noise, read_head, respond, s16le, serve,
    serve_body, sine, wav_stream_header, zero_crossings,
};
use webos_playback::stream::{self, MusicStream, StreamConfig};
use webos_playback::{Pcm, PlaybackError, wav_bytes};

fn cfg() -> StreamConfig {
    StreamConfig {
        idle_timeout: Duration::from_secs(2),
        // The test servers listen on loopback (refused in production).
        insecure_allow_loopback: true,
        ..StreamConfig::default()
    }
}

/// Reads the whole stream; returns the samples, the format and the end.
fn drain(mut s: MusicStream) -> (Vec<i16>, Option<(u32, u16)>, Option<PlaybackError>) {
    let mut out = Vec::new();
    let mut fmt = None;
    while let Some(item) = s.blocking_recv() {
        match item {
            Ok(c) => {
                let f = (c.rate, c.channels);
                assert!(
                    fmt.is_none() || fmt == Some(f),
                    "format changed between chunks"
                );
                fmt = Some(f);
                out.extend_from_slice(&c.samples);
            }
            Err(e) => {
                assert!(s.blocking_recv().is_none(), "nothing may follow the error");
                return (out, fmt, Some(e));
            }
        }
    }
    (out, fmt, None)
}

fn play(url: &str) -> (Vec<i16>, Option<(u32, u16)>, Option<PlaybackError>) {
    drain(stream::open(url, &cfg()).unwrap())
}

// ------------------------------------------------------- formats and framing

#[test]
fn flac_with_length_is_bit_exact() {
    let src = noise(48_000, 2, 48_000, 7);
    let srv = serve_body("audio/flac", flac_bytes(&src, 4096), Framing::Length);
    let (out, fmt, end) = play(&srv.url("/a.flac"));
    assert!(end.is_none(), "{end:?}");
    assert_eq!(fmt, Some((48_000, 2)));
    assert_eq!(out, src.samples);
}

#[test]
fn chunked_body_without_length_plays() {
    let src = sine(44_100, 2, 440.0, 1.0);
    let srv = serve_body("audio/flac", flac_bytes(&src, 4096), Framing::Chunked);
    let (out, fmt, end) = play(&srv.url("/api/esphome/ffmpeg_proxy/dev/x.flac"));
    assert!(end.is_none(), "{end:?}");
    assert_eq!(fmt, Some((44_100, 2)));
    assert_eq!(out, src.samples);
}

#[test]
fn close_delimited_wav_of_unknown_length_plays() {
    let src = sine(22_050, 1, 300.0, 1.0);
    let mut body = wav_stream_header(22_050, 1);
    body.extend_from_slice(&s16le(&src.samples));
    let srv = serve_body("audio/wav", body, Framing::Close);
    let (out, fmt, end) = play(&srv.url("/flow"));
    assert!(end.is_none(), "{end:?}");
    assert_eq!(fmt, Some((22_050, 1)));
    assert_eq!(out, src.samples);
}

#[test]
fn ordinary_wav_with_length_plays() {
    let src = sine(48_000, 2, 1000.0, 0.5);
    let srv = serve_body("audio/x-wav", wav_bytes(&src), Framing::Length);
    let (out, fmt, end) = play(&srv.url("/t.wav"));
    assert!(end.is_none(), "{end:?}");
    assert_eq!(fmt, Some((48_000, 2)));
    assert_eq!(out, src.samples);
}

#[test]
fn rate_and_channels_come_from_the_stream_header() {
    for (rate, ch) in [
        (44_100, 2),
        (48_000, 2),
        (22_050, 1),
        (96_000, 2),
        (8_000, 1),
    ] {
        let src = sine(rate, ch, 500.0, 0.3);
        for (ctype, body) in [
            ("audio/flac", flac_bytes(&src, 1152)),
            ("application/octet-stream", wav_bytes(&src)),
        ] {
            let srv = serve_body(ctype, body, Framing::Chunked);
            let (out, fmt, end) = play(&srv.url("/s"));
            assert!(end.is_none(), "{rate} {ch} {ctype}: {end:?}");
            assert_eq!(fmt, Some((rate, ch)), "{ctype}");
            assert_eq!(out, src.samples, "{rate} {ch} {ctype}");
        }
    }
}

#[test]
fn more_than_two_channels_are_downmixed_to_stereo() {
    let src = sine(48_000, 6, 200.0, 0.2);
    let srv = serve_body("audio/flac", flac_bytes(&src, 4096), Framing::Length);
    let (out, fmt, end) = play(&srv.url("/6ch.flac"));
    assert!(end.is_none(), "{end:?}");
    assert_eq!(fmt, Some((48_000, 2)));
    assert_eq!(out.len(), src.frames() * 2);
}

#[test]
fn chunks_have_the_configured_duration() {
    let src = sine(48_000, 2, 440.0, 0.51);
    let srv = serve_body("audio/flac", flac_bytes(&src, 4096), Framing::Chunked);
    let mut s = stream::open(&srv.url("/c.flac"), &cfg()).unwrap();
    let mut sizes = Vec::new();
    while let Some(c) = s.blocking_recv() {
        sizes.push(c.unwrap().frames());
    }
    let (last, full) = sizes.split_last().unwrap();
    assert!(full.iter().all(|&n| n == 960), "{sizes:?}");
    assert!(*last <= 960 && *last > 0);
    assert_eq!(sizes.iter().sum::<usize>(), src.frames());
}

// ----------------------------------------------------- endless and memory

/// Writes an endless FLAC stream (no length, never closes) until the client
/// goes away; counts body bytes in `written`.
fn endless_flac(rate: u32, channels: u16) -> common::Server {
    serve(move |mut s, written| {
        let _ = read_head(&mut s);
        let head = "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\n";
        if s.write_all(head.as_bytes()).is_err() {
            return;
        }
        let hdr = flac_header(rate, channels, 4096);
        if s.write_all(&hdr).is_err() {
            return;
        }
        let block = noise(rate, channels, 4096, 3);
        let mut n = 0u64;
        loop {
            let f = flac_frame(n, &block.samples, channels, 0);
            if s.write_all(&f).is_err() {
                return;
            }
            written.fetch_add(f.len() as u64, Ordering::Relaxed);
            n += 1;
        }
    })
}

fn consume_bytes(total: u64) {
    let srv = endless_flac(44_100, 2);
    let mut s = stream::open(&srv.url("/endless"), &cfg()).unwrap();
    let mut frames = 0u64;
    let mut rss0 = 0;
    let t = Instant::now();
    loop {
        let c = s.blocking_recv().expect("endless stream ended").unwrap();
        assert_eq!((c.rate, c.channels), (44_100, 2));
        frames += c.frames() as u64;
        // Measure after the buffers have filled once.
        if rss0 == 0 && frames > 44_100 * 10 {
            rss0 = common::rss_kib();
        }
        if s.stats().bytes_received >= total {
            break;
        }
    }
    let grown = common::rss_kib().saturating_sub(rss0);
    eprintln!(
        "endless: {} MiB in {:?}, {frames} frames, RSS growth {grown} KiB",
        total >> 20,
        t.elapsed()
    );
    // RSS is only meaningful natively: under qemu-user (the armv7 test run)
    // RSS climbs with the bytes streamed while the live heap, measured with
    // a counting allocator, stays flat at about 910 KiB (2026-11-05), so it
    // is an emulator and allocator effect. EXP-013 measures RSS on the TV.
    if cfg!(target_arch = "x86_64") {
        assert!(grown < 3 * 1024, "RSS grew by {grown} KiB");
    }
    assert!(s.close(Duration::from_secs(1)));
}

#[test]
fn endless_stream_plays_with_bounded_memory() {
    consume_bytes(64 << 20);
}

/// The 1 GiB variant of the plan (M3a acceptance 3); run in release:
/// `cargo test -p webos-playback --release --test stream -- --ignored`.
#[test]
#[ignore = "1 GiB through the decoder; run with --release --ignored"]
fn endless_stream_one_gib() {
    consume_bytes(1 << 30);
}

#[test]
fn slow_consumer_bounds_what_is_read_from_the_socket() {
    let srv = endless_flac(48_000, 2);
    let c = cfg();
    let s = stream::open(&srv.url("/bp"), &c).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let a = s.stats();
    std::thread::sleep(Duration::from_millis(500));
    let b = s.stats();
    // Nothing is consumed: the reads stop once the ring and the queue are
    // full (plus one socket read and symphonia's 64 KiB read-ahead).
    assert_eq!(
        a.bytes_received, b.bytes_received,
        "still reading: {a:?} {b:?}"
    );
    let bound = (c.ring_bytes + 16 * 1024 + 64 * 1024 + stream::MAX_QUEUE_BYTES) as u64;
    assert!(b.bytes_received <= bound, "{b:?}");
    assert!(b.queued_samples * 2 <= stream::MAX_QUEUE_BYTES, "{b:?}");
    assert!(b.ring_bytes <= c.ring_bytes);
    eprintln!(
        "backpressure: {b:?}, server wrote {}",
        srv.written.load(Ordering::Relaxed)
    );
    assert!(s.close(Duration::from_secs(1)));
}

// ------------------------------------------------ stall, early end, cancel

/// Sends a header and half a second of audio, then holds the connection
/// open without sending anything for `hold`.
fn stalling(hold: Duration) -> common::Server {
    serve(move |mut s, _| {
        let _ = read_head(&mut s);
        let head = "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\n";
        let src = sine(48_000, 2, 440.0, 0.5);
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&flac_bytes(&src, 4096));
        let _ = s.flush();
        std::thread::sleep(hold);
    })
}

#[test]
fn a_silent_connection_ends_within_idle_timeout_plus_one_second() {
    let srv = stalling(Duration::from_secs(30));
    let idle = Duration::from_secs(1);
    let c = StreamConfig {
        idle_timeout: idle,
        ..cfg()
    };
    let t = Instant::now();
    let s = stream::open(&srv.url("/stall"), &c).unwrap();
    let (out, _, end) = drain(s);
    let took = t.elapsed();
    // The audio before the stall is delivered, except the last FLAC frame:
    // the demuxer only knows a frame is complete when the next one starts.
    assert!(out.len() >= 48_000 - 4096 * 2, "{}", out.len());
    assert_eq!(out[..], sine(48_000, 2, 440.0, 0.5).samples[..out.len()]);
    assert!(
        matches!(end, Some(PlaybackError::IdleTimeout(d)) if d == idle),
        "{end:?}"
    );
    assert!(took < idle + Duration::from_secs(1), "took {took:?}");
}

#[test]
fn a_silent_server_before_the_headers_times_out() {
    let srv = serve(|mut s, _| {
        let _ = read_head(&mut s);
        std::thread::sleep(Duration::from_secs(30));
    });
    let c = StreamConfig {
        idle_timeout: Duration::from_millis(500),
        response_timeout: Duration::from_secs(20),
        ..cfg()
    };
    let t = Instant::now();
    let (_, _, end) = drain(stream::open(&srv.url("/h"), &c).unwrap());
    assert!(
        matches!(end, Some(PlaybackError::IdleTimeout(_))),
        "{end:?}"
    );
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn cancel_joins_both_threads_while_the_read_is_blocked() {
    let srv = stalling(Duration::from_secs(60));
    let c = StreamConfig {
        idle_timeout: Duration::from_secs(60),
        ..cfg()
    };
    let mut s = stream::open(&srv.url("/cancel"), &c).unwrap();
    // Take the buffered audio so both threads are waiting: decode on the
    // ring, fetch inside the socket read.
    let mut got = 0;
    while got < 30_000 {
        got += s.blocking_recv().unwrap().unwrap().samples.len();
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(!s.is_finished());
    let t = Instant::now();
    assert!(s.close(Duration::from_secs(1)), "threads still running");
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

#[test]
fn cancel_while_the_consumer_is_paused() {
    let srv = endless_flac(48_000, 2);
    let s = stream::open(&srv.url("/p"), &cfg()).unwrap();
    std::thread::sleep(Duration::from_millis(500)); // queue and ring full
    let canceller = s.canceller();
    let t = Instant::now();
    canceller.cancel();
    assert!(s.close(Duration::from_secs(1)));
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn recv_after_cancel_returns_none() {
    let srv = endless_flac(48_000, 2);
    let mut s = stream::open(&srv.url("/n"), &cfg()).unwrap();
    assert!(s.blocking_recv().unwrap().is_ok());
    s.cancel();
    assert!(s.blocking_recv().is_none());
    assert!(s.try_recv().is_err());
}

#[test]
fn dropping_the_stream_closes_the_connection() {
    let srv = endless_flac(48_000, 2);
    let mut s = stream::open(&srv.url("/d"), &cfg()).unwrap();
    assert!(s.blocking_recv().unwrap().is_ok());
    drop(s);
    // The server's writes fail once the socket is gone, so `written` stops.
    std::thread::sleep(Duration::from_millis(300));
    let a = srv.written.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(a, srv.written.load(Ordering::Relaxed));
}

#[test]
fn shorter_than_content_length_is_ended_early() {
    let src = sine(48_000, 2, 440.0, 0.2);
    let body = flac_bytes(&src, 4096);
    let srv = serve(move |mut s, _| {
        let _ = read_head(&mut s);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nContent-Length: {}\r\n\r\n",
            body.len() + 5000
        );
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&body);
    });
    let (out, _, end) = play(&srv.url("/short"));
    assert!(out.len() >= src.samples.len() - 4096 * 2, "{}", out.len());
    assert_eq!(out[..], src.samples[..out.len()]);
    assert!(matches!(end, Some(PlaybackError::EndedEarly)), "{end:?}");
}

#[test]
fn http_errors_are_terminal_and_not_retried() {
    let srv = serve(|mut s, _| {
        let _ = read_head(&mut s);
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    });
    let (_, _, end) = play(&srv.url("/missing"));
    assert!(matches!(end, Some(PlaybackError::Status(404))), "{end:?}");
    assert_eq!(srv.hits.load(Ordering::SeqCst), 1, "a URL is fetched once");

    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x", l.local_addr().unwrap());
    drop(l);
    let (_, _, end) = play(&url);
    assert!(matches!(end, Some(PlaybackError::Http(_))), "{end:?}");
}

#[test]
fn errors_never_contain_the_url() {
    let srv = serve(|mut s, _| {
        let _ = read_head(&mut s);
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\nthis is not audio!!!");
    });
    let url = srv.url("/secret-token-123/x.flac?auth=abc");
    let (_, _, end) = play(&url);
    let msg = end.unwrap().to_string();
    assert!(!msg.contains("secret") && !msg.contains("auth"), "{msg}");
    assert!(matches!(
        stream::open("file:///etc/passwd", &cfg()),
        Err(PlaybackError::BadUrl(_))
    ));
}

#[test]
fn https_is_validated() {
    // A plain HTTP server behind an https URL: the handshake fails.
    let srv = serve(|mut s, _| {
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        std::thread::sleep(Duration::from_millis(500));
    });
    let url = format!("https://{}/x.flac", srv.addr);
    let (_, _, end) = play(&url);
    assert!(
        matches!(end, Some(PlaybackError::Tls(_) | PlaybackError::Http(_))),
        "{end:?}"
    );
    // A bad CA file is refused before any connection.
    let dir = std::env::temp_dir().join(format!("webos-stream-ca-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ca = dir.join("ca.pem");
    std::fs::write(&ca, "not a certificate").unwrap();
    let c = StreamConfig {
        ca_file: Some(ca),
        ..cfg()
    };
    let (_, _, end) = drain(stream::open(&url, &c).unwrap());
    assert!(matches!(end, Some(PlaybackError::Tls(_))), "{end:?}");
}

// ------------------------------------------------------------ format change

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn a_mid_stream_rate_change_never_plays_at_the_wrong_rate() {
    // Two MP3 streams back to back, 44.1 kHz then 48 kHz. Observed with
    // symphonia 0.6.1: the MP3 demuxer drops the frames that disagree with
    // the first header, so a finite body ends after the first part (13230
    // frames here). The decoder's own rate check would turn a decoder that
    // does emit the change into `Unsupported`.
    let srv = serve_body("audio/mpeg", fixture("rate-change.mp3"), Framing::Chunked);
    let (out, fmt, end) = play(&srv.url("/change.mp3"));
    assert_eq!(fmt, Some((44_100, 2)));
    assert!(
        !out.is_empty() && out.len() / 2 <= 13_230 + 1152,
        "{}",
        out.len()
    );
    assert!(
        end.is_none() || matches!(end, Some(PlaybackError::Unsupported(_))),
        "{end:?}"
    );
}

#[test]
fn an_endless_stream_after_a_format_change_fails_within_the_gap_budget() {
    // 48 kHz frames, then endless frames declaring 22.05 kHz that the
    // demuxer never accepts: the gap budget ends the stream.
    let srv = serve(|mut s, written| {
        let _ = read_head(&mut s);
        let head = "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\n";
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&flac_header(48_000, 2, 4096));
        let block = noise(48_000, 2, 4096, 9);
        let mut n = 0u64;
        loop {
            let code = if n < 3 { 0 } else { 0b0110 };
            let f = flac_frame(n, &block.samples, 2, code);
            if s.write_all(&f).is_err() {
                return;
            }
            written.fetch_add(f.len() as u64, Ordering::Relaxed);
            n += 1;
        }
    });
    let (out, fmt, end) = play(&srv.url("/changed-endless.flac"));
    assert_eq!(fmt, Some((48_000, 2)));
    assert!(out.len() <= 3 * 4096 * 2);
    assert!(matches!(end, Some(PlaybackError::Decode(_))), "{end:?}");
    let sent = srv.written.load(Ordering::Relaxed);
    assert!(sent < 8 << 20, "the server kept sending: {sent}");
}

#[test]
fn flac_frames_disagreeing_with_streaminfo_are_not_played() {
    // Observed with symphonia 0.6.1: the FLAC demuxer does not resync onto
    // frames whose header rate differs from STREAMINFO, so the stream ends
    // after the matching frames. Either that or an error is acceptable;
    // audio at the wrong rate is not.
    let src = sine(48_000, 2, 440.0, 1.0);
    let mut body = flac_header(48_000, 2, 4096);
    let mut n = 0;
    for c in src.samples.chunks(4096 * 2).take(5) {
        body.extend_from_slice(&flac_frame(n, c, 2, 0));
        n += 1;
    }
    for c in src.samples.chunks(4096 * 2).take(5) {
        body.extend_from_slice(&flac_frame(n, c, 2, 0b0110)); // 22.05 kHz
        n += 1;
    }
    let srv = serve_body("audio/flac", body, Framing::Chunked);
    let (out, fmt, end) = play(&srv.url("/change.flac"));
    assert_eq!(fmt, Some((48_000, 2)));
    assert!(out.len() <= 5 * 4096 * 2, "{}", out.len());
    assert!(
        end.is_none()
            || matches!(
                end,
                Some(PlaybackError::Unsupported(_) | PlaybackError::Decode(_))
            ),
        "{end:?}"
    );
}

#[test]
fn ffmpeg_pipe_flac_equals_its_wav() {
    let flac = serve_body("audio/flac", fixture("tone-44100-2.flac"), Framing::Chunked);
    let wav = serve_body("audio/wav", fixture("tone-44100-2.wav"), Framing::Close);
    let (a, fa, ea) = play(&flac.url("/p.flac"));
    let (b, fb, eb) = play(&wav.url("/p.wav"));
    assert!(ea.is_none() && eb.is_none(), "{ea:?} {eb:?}");
    assert_eq!(fa, Some((44_100, 2)));
    assert_eq!(fa, fb);
    assert_eq!(a.len(), 44_100 / 4 * 2);
    assert_eq!(a, b);
}

#[test]
fn lossy_formats_decode_at_their_own_rate() {
    for (name, ctype, rate, ch) in [
        ("tone-44100-2.mp3", "audio/mpeg", 44_100, 2),
        ("tone-22050-1.mp3", "audio/mpeg", 22_050, 1),
        ("tone-48000-2.ogg", "audio/ogg", 48_000, 2),
    ] {
        let srv = serve_body(ctype, fixture(name), Framing::Close);
        let (out, fmt, end) = play(&srv.url("/stream"));
        assert!(end.is_none(), "{name}: {end:?}");
        assert_eq!(fmt, Some((rate, ch)), "{name}");
        let frames = out.len() / usize::from(ch);
        let secs = frames as f64 / f64::from(rate);
        assert!((0.45..0.6).contains(&secs), "{name}: {secs} s");
        for c in 0..usize::from(ch) {
            let x = common::channel(&out, ch, c);
            // 440 Hz then 880 Hz: two zero crossings per period.
            let hz = zero_crossings(&x) as f64 / secs / 2.0;
            let want = 440.0 * (c + 1) as f64;
            assert!((hz - want).abs() < want * 0.05, "{name} ch{c}: {hz} Hz");
            let level = common::rms(&x);
            assert!(level > 1000.0, "{name} ch{c}: rms {level}");
        }
    }
}

// ------------------------------------------------------------------ threads

#[test]
fn threads_are_named() {
    let srv = endless_flac(48_000, 2);
    let mut s = stream::open(&srv.url("/names"), &cfg()).unwrap();
    assert!(s.blocking_recv().unwrap().is_ok());
    let names = common::thread_names();
    assert!(
        names.iter().any(|n| n == stream::FETCH_THREAD_NAME),
        "{names:?}"
    );
    assert!(
        names.iter().any(|n| n == stream::DECODE_THREAD_NAME),
        "{names:?}"
    );
    assert!(s.close(Duration::from_secs(1)));
}

// ----------------------------------------------------------------- async API

#[tokio::test(flavor = "current_thread")]
async fn async_recv_from_a_tokio_task() {
    let src = sine(48_000, 2, 440.0, 0.5);
    let srv = serve_body("audio/flac", flac_bytes(&src, 4096), Framing::Chunked);
    let mut s = stream::open(&srv.url("/async.flac"), &cfg()).unwrap();
    let mut out = Vec::new();
    while let Some(c) = s.recv().await {
        out.extend_from_slice(&c.unwrap().samples);
    }
    assert_eq!(out, src.samples);
}

#[tokio::test(flavor = "current_thread")]
async fn async_recv_is_woken_by_cancel_from_another_thread() {
    let srv = stalling(Duration::from_secs(30));
    let c = StreamConfig {
        idle_timeout: Duration::from_secs(30),
        ..cfg()
    };
    let mut s = stream::open(&srv.url("/w"), &c).unwrap();
    let canceller = s.canceller();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        canceller.cancel();
    });
    let t = Instant::now();
    let r = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(x) = s.recv().await {
            x.unwrap();
        }
    })
    .await;
    assert!(r.is_ok(), "recv was not woken");
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[test]
fn pcm_chunk_contract_holds_for_a_short_stream() {
    // A stream shorter than one chunk still yields one (short) chunk.
    let src = sine(48_000, 1, 440.0, 0.005);
    let srv = serve_body("audio/flac", flac_bytes(&src, 240), Framing::Length);
    let (out, fmt, end) = play(&srv.url("/tiny.flac"));
    assert!(end.is_none());
    assert_eq!(fmt, Some((48_000, 1)));
    assert_eq!(
        out,
        Pcm {
            rate: 48_000,
            channels: 1,
            samples: src.samples.clone()
        }
        .samples
    );
    let _ = respond; // used by other suites
}

// ------------------------------------------- through the music engine (M4)

mod engine {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc::UnboundedReceiver;
    use webos_playback::music::{
        EndReason, MusicConfig, MusicEngine, MusicEvent, MusicVolume, WavStreamFactory, read_wav,
    };

    use super::*;

    type Paths = Arc<Mutex<Vec<PathBuf>>>;

    fn wav_engine(tag: &str) -> (MusicEngine, UnboundedReceiver<MusicEvent>, Paths) {
        let dir =
            std::env::temp_dir().join(format!("webos-stream-engine-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let factory = WavStreamFactory::new(&dir, "music");
        let paths = factory.paths();
        let (e, ev) = MusicEngine::spawn(
            MusicConfig::default(),
            Box::new(factory),
            MusicVolume::new(1.0),
        );
        (e, ev, paths)
    }

    async fn next_event(ev: &mut UnboundedReceiver<MusicEvent>) -> MusicEvent {
        tokio::time::timeout(Duration::from_secs(10), ev.recv())
            .await
            .expect("no engine event")
            .expect("engine gone")
    }

    /// Plays `url` to the end and returns the WAV the engine wrote.
    async fn play_to_end(tag: &str, url: &str) -> ((u32, u16), Vec<i16>) {
        let (e, mut ev, paths) = wav_engine(tag);
        let id = e.play(Box::new(stream::open(url, &cfg()).unwrap()));
        let MusicEvent::Started {
            id: sid,
            rate,
            channels,
        } = next_event(&mut ev).await
        else {
            panic!("expected Started")
        };
        assert_eq!(sid, id);
        let MusicEvent::Ended { id: eid, reason } = next_event(&mut ev).await else {
            panic!("expected Ended")
        };
        assert_eq!((eid, reason), (id, EndReason::Finished));
        let path = paths.lock().unwrap()[0].clone();
        let (fmt, samples) = read_wav(&path).unwrap();
        assert_eq!((fmt.rate_hz, fmt.channels), (rate, channels));
        let _ = std::fs::remove_file(path);
        ((rate, channels), samples)
    }

    /// The shape of HA's ffmpeg proxy (fake_proxy.py ha-proxy mode): FLAC
    /// at the advertised 48 kHz stereo, chunked in 2048-byte chunks, no
    /// `Content-Length`, the URL ending in `.flac`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ha_proxy_shaped_stream_plays_through_the_engine() {
        let src = sine(48_000, 2, 440.0, 0.6);
        let srv = serve_body("audio/flac", flac_bytes(&src, 4096), Framing::Chunked);
        let url = srv.url("/api/esphome/ffmpeg_proxy/0123abcd/4f5e6d.flac");
        let (fmt, out) = play_to_end("proxy", &url).await;
        assert_eq!(fmt, (48_000, 2));
        // Unity gain: the WAV holds the decoded stream, sample for sample
        // (allowing one LSB of Q15 rounding).
        assert!(out.len() >= src.samples.len(), "{}", out.len());
        let max_err = src
            .samples
            .iter()
            .zip(&out)
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(max_err <= 1, "max error {max_err}");
    }

    /// The shape of Music Assistant's flow stream (fake_proxy.py ma-flow
    /// mode): another host and port, no length, 44.1 kHz, one continuous
    /// stream of several "tracks" that ends at connection close.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ma_flow_shaped_stream_plays_through_the_engine() {
        let mut pcm = sine(44_100, 2, 440.0, 0.3).samples;
        pcm.extend(sine(44_100, 2, 660.0, 0.3).samples);
        let all = Pcm {
            rate: 44_100,
            channels: 2,
            samples: pcm,
        };
        let srv = serve_body("audio/flac", flac_bytes(&all, 4096), Framing::Close);
        let (fmt, out) = play_to_end("flow", &srv.url("/flow/queue-1.flac")).await;
        assert_eq!(fmt, (44_100, 2));
        assert!(out.len() >= all.samples.len());
    }

    /// Stopping the engine drops the source, which must close the socket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn engine_stop_closes_the_connection() {
        let srv = endless_flac(48_000, 2);
        let (e, mut ev, _paths) = wav_engine("stop");
        e.play(Box::new(
            stream::open(&srv.url("/endless.flac"), &cfg()).unwrap(),
        ));
        assert!(matches!(
            next_event(&mut ev).await,
            MusicEvent::Started { .. }
        ));
        tokio::time::sleep(Duration::from_millis(300)).await;
        e.stop();
        assert!(matches!(
            next_event(&mut ev).await,
            MusicEvent::Ended {
                reason: EndReason::Stopped,
                ..
            }
        ));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let a = srv.written.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            a,
            srv.written.load(Ordering::Relaxed),
            "server still writing"
        );
    }

    /// A stream error reaches the engine as `EndReason::Error`, without URL.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stream_errors_end_the_track_with_an_error() {
        let srv = stalling(Duration::from_secs(30));
        let c = StreamConfig {
            idle_timeout: Duration::from_millis(500),
            ..cfg()
        };
        let (e, mut ev, _paths) = wav_engine("err");
        e.play(Box::new(
            stream::open(&srv.url("/token-xyz/s.flac"), &c).unwrap(),
        ));
        assert!(matches!(
            next_event(&mut ev).await,
            MusicEvent::Started { .. }
        ));
        let MusicEvent::Ended { reason, .. } = next_event(&mut ev).await else {
            panic!("expected Ended")
        };
        let EndReason::Error(msg) = reason else {
            panic!("expected an error, got {reason:?}")
        };
        assert!(msg.contains("stalled") && !msg.contains("token"), "{msg}");
    }
}
