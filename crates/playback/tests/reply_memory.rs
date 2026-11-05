//! Memory of a long reply: the streamed [`play_url`] against the old
//! whole-file pipeline (`fetch_with_meta` then `play_bytes`).
//!
//! **Method.** Each measurement runs in a fresh child process (this test
//! binary again, running the ignored `child_measure` test with
//! `REPLY_MEMORY_MODE` set), so the peak is that of one playback only. The
//! child serves a generated WAV from a loopback server in the same process
//! (generated on the fly in 64 KiB pieces, so the server holds no body),
//! plays one short reply to warm up, reads `VmRSS` from `/proc/self/status`,
//! resets the peak (`5` into `/proc/self/clear_refs`), plays the long reply
//! through a `PacatSink` whose `pacat` is `cat > /dev/null` (the daemon's
//! path on the TV: 44.1 kHz stereo, software gain), and reports `VmHWM`
//! minus that `VmRSS` as the growth. The workspace forbids `unsafe`, so a
//! counting allocator is not an option; RSS is what the watchdog limits.
//!
//! The reply is 22.05 kHz mono 16-bit (HA's announcement format for this
//! satellite, as WAV so the test needs no encoder): 120 s is 5.3 MB of body,
//! well inside the 10 MiB body cap.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    missing_docs
)]

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use webos_playback::{FetchConfig, PacatSink, fetch_with_meta, play_bytes, play_url};

const RATE: u32 = 22_050;
const MODE_VAR: &str = "REPLY_MEMORY_MODE";

/// One WAV, generated piece by piece: `secs` of a 440 Hz tone at -12 dBFS.
async fn serve_wav(secs: u32) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut req = [0u8; 2048];
                let _ = sock.read(&mut req).await;
                let frames = RATE * secs;
                let data_len = frames * 2;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    44 + data_len
                );
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let mut hdr = Vec::with_capacity(44);
                hdr.extend_from_slice(b"RIFF");
                hdr.extend_from_slice(&(36 + data_len).to_le_bytes());
                hdr.extend_from_slice(b"WAVEfmt ");
                hdr.extend_from_slice(&16u32.to_le_bytes());
                hdr.extend_from_slice(&1u16.to_le_bytes());
                hdr.extend_from_slice(&1u16.to_le_bytes());
                hdr.extend_from_slice(&RATE.to_le_bytes());
                hdr.extend_from_slice(&(RATE * 2).to_le_bytes());
                hdr.extend_from_slice(&2u16.to_le_bytes());
                hdr.extend_from_slice(&16u16.to_le_bytes());
                hdr.extend_from_slice(b"data");
                hdr.extend_from_slice(&data_len.to_le_bytes());
                if sock.write_all(&hdr).await.is_err() {
                    return;
                }
                let mut piece = Vec::with_capacity(64 * 1024);
                for n in 0..frames {
                    let v =
                        (8000.0 * (2.0 * PI * 440.0 * f64::from(n) / f64::from(RATE)).sin()) as i16;
                    piece.extend_from_slice(&v.to_le_bytes());
                    if piece.len() == piece.capacity() {
                        if sock.write_all(&piece).await.is_err() {
                            return;
                        }
                        piece.clear();
                    }
                }
                let _ = sock.write_all(&piece).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}/api/tts_proxy/long.wav")
}

fn status_kib(field: &str) -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    s.lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

fn fake_pacat(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("pacat");
    std::fs::write(&p, "#!/bin/sh\ncat > /dev/null\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

async fn play(mode: &str, url: &str, cfg: &FetchConfig, sink: &mut PacatSink) {
    match mode {
        "whole" => {
            let f = fetch_with_meta(url, cfg).await.unwrap();
            play_bytes(
                &f.bytes,
                f.content_type.as_deref(),
                sink,
                Instant::now().elapsed(),
            )
            .await
            .unwrap();
        }
        _ => {
            play_url(url, cfg, sink).await.unwrap();
        }
    }
}

/// The child: `REPLY_MEMORY_MODE` is `stream:<secs>` or `whole:<secs>`.
#[test]
#[ignore = "run by long_reply_memory_is_bounded in a child process"]
fn child_measure() {
    let Ok(spec) = std::env::var(MODE_VAR) else {
        return;
    };
    let (mode, secs) = spec.split_once(':').unwrap();
    let secs: u32 = secs.parse().unwrap();
    let dir = std::env::temp_dir().join(format!("webos-reply-mem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let cfg = FetchConfig {
            insecure_allow_loopback: true,
            ..FetchConfig::default()
        };
        let mut sink = PacatSink::new();
        sink.binary = fake_pacat(&dir);
        sink.gain = 0.5;
        // Warm up the same path (threads, decoder, kernel table, process spawn).
        let short = serve_wav(1).await;
        play(mode, &short, &cfg, &mut sink).await;
        let long = serve_wav(secs).await;
        let before = status_kib("VmRSS:");
        let reset = std::fs::write("/proc/self/clear_refs", "5").is_ok();
        let t = Instant::now();
        play(mode, &long, &cfg, &mut sink).await;
        let took = t.elapsed();
        let peak = status_kib("VmHWM:");
        println!(
            "REPLY_MEMORY mode={mode} secs={secs} rss_before_kib={before} hwm_kib={peak} growth_kib={} peak_reset={reset} took_ms={}",
            peak.saturating_sub(before),
            took.as_millis()
        );
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs one measurement in a child process; returns the growth in KiB.
fn measure(spec: &str) -> u64 {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "child_measure",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MODE_VAR, spec)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "child {spec} failed:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = text
        .lines()
        .find_map(|l| l.find("REPLY_MEMORY mode=").map(|i| &l[i..]))
        .unwrap_or_else(|| panic!("no result from {spec}:\n{text}"));
    eprintln!("{line}");
    line.split_whitespace()
        .find_map(|w| w.strip_prefix("growth_kib="))
        .unwrap()
        .parse()
        .unwrap()
}

/// A 120 s announcement must not grow the process by more than a few MiB,
/// and no more than a 30 s one does. The whole-file pipeline is measured as
/// the reference (it shows the method sees the growth the TV saw).
#[test]
fn long_reply_memory_is_bounded() {
    let stream_30 = measure("stream:30");
    let stream_120 = measure("stream:120");
    let whole_120 = measure("whole:120");
    eprintln!(
        "growth: streamed 30 s {stream_30} KiB, streamed 120 s {stream_120} KiB, whole file 120 s {whole_120} KiB"
    );
    assert!(
        stream_120 < 6 * 1024,
        "streamed 120 s reply grew the process by {stream_120} KiB"
    );
    assert!(
        stream_120 < stream_30 + 2 * 1024,
        "growth depends on the length: 30 s {stream_30} KiB, 120 s {stream_120} KiB"
    );
    assert!(
        whole_120 > 4 * stream_120.max(1024),
        "the reference should show the whole-file growth: {whole_120} KiB"
    );
}
