//! Shared helpers for the music stream tests: synthetic signals, a minimal
//! FLAC encoder (verbatim subframes, bit-exact), streaming WAV headers and a
//! std-thread HTTP server. Only generated tones and noise, no recordings.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    unreachable_pub,
    missing_docs
)]

use std::f64::consts::PI;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use webos_playback::Pcm;

// ------------------------------------------------------------------ signals

pub fn sine(rate: u32, channels: u16, hz: f64, secs: f64) -> Pcm {
    let frames = (f64::from(rate) * secs) as usize;
    let mut samples = Vec::with_capacity(frames * usize::from(channels));
    for n in 0..frames {
        for c in 0..channels {
            // A different frequency per channel catches channel swaps.
            let f = hz * f64::from(c + 1);
            let v = (0.5 * (2.0 * PI * f * n as f64 / f64::from(rate)).sin() * 32767.0) as i16;
            samples.push(v);
        }
    }
    Pcm {
        rate,
        channels,
        samples,
    }
}

/// Deterministic white noise (xorshift), full scale.
pub fn noise(rate: u32, channels: u16, frames: usize, seed: u64) -> Pcm {
    let mut x = seed | 1;
    let samples = (0..frames * usize::from(channels))
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 48) as i16
        })
        .collect();
    Pcm {
        rate,
        channels,
        samples,
    }
}

pub fn zero_crossings(s: &[i16]) -> usize {
    s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count()
}

pub fn rms(s: &[i16]) -> f64 {
    (s.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / s.len().max(1) as f64).sqrt()
}

/// Every `channels`-th sample starting at `ch`.
pub fn channel(s: &[i16], channels: u16, ch: usize) -> Vec<i16> {
    s.iter()
        .skip(ch)
        .step_by(usize::from(channels))
        .copied()
        .collect()
}

// --------------------------------------------------------------------- FLAC

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// FLAC's UTF-8-like coding of the frame number (up to 36 bits).
fn utf8_number(n: u64, out: &mut Vec<u8>) {
    if n < 0x80 {
        out.push(n as u8);
        return;
    }
    let mut len = 2;
    while len < 7 && n >= 1u64 << (5 * len + 1) {
        len += 1;
    }
    let first_bits = 7 - len;
    out.push(
        ((0xFFu16 << (8 - len)) as u8) | ((n >> (6 * (len - 1))) as u8 & ((1 << first_bits) - 1)),
    );
    for i in (0..len - 1).rev() {
        out.push(0x80 | ((n >> (6 * i)) as u8 & 0x3F));
    }
}

/// `fLaC` plus a STREAMINFO block with unknown length and no MD5, as a
/// non-seekable encoder (HA's ffmpeg proxy writing to a pipe) produces.
pub fn flac_header(rate: u32, channels: u16, block: u16) -> Vec<u8> {
    let mut v = b"fLaC".to_vec();
    v.extend_from_slice(&[0x80, 0, 0, 34]); // last block, STREAMINFO, 34 bytes
    v.extend_from_slice(&block.to_be_bytes());
    v.extend_from_slice(&block.to_be_bytes());
    v.extend_from_slice(&[0; 6]); // min and max frame size unknown
    let packed: u64 = (u64::from(rate) << 44) | (u64::from(channels - 1) << 41) | (15u64 << 36);
    v.extend_from_slice(&packed.to_be_bytes());
    v.extend_from_slice(&[0; 16]);
    v
}

/// One FLAC frame with verbatim 16-bit subframes. `rate_code` 0 means "as
/// in STREAMINFO"; other codes set the rate in the frame header.
pub fn flac_frame(frame_no: u64, interleaved: &[i16], channels: u16, rate_code: u8) -> Vec<u8> {
    let ch = usize::from(channels);
    let frames = interleaved.len() / ch;
    assert!((1..=65536).contains(&frames));
    let mut v = vec![
        0xFF,
        0xF8,
        0x70 | (rate_code & 0x0F),
        (((channels - 1) as u8) << 4) | 0x08,
    ];
    utf8_number(frame_no, &mut v);
    v.extend_from_slice(&((frames - 1) as u16).to_be_bytes());
    v.push(crc8(&v));
    for c in 0..ch {
        v.push(0x02); // verbatim, no wasted bits
        for f in 0..frames {
            v.extend_from_slice(&interleaved[f * ch + c].to_be_bytes());
        }
    }
    let crc = crc16(&v);
    v.extend_from_slice(&crc.to_be_bytes());
    v
}

/// A whole FLAC stream of `pcm` in blocks of `block` frames.
pub fn flac_bytes(pcm: &Pcm, block: u16) -> Vec<u8> {
    let mut v = flac_header(pcm.rate, pcm.channels, block);
    let per = usize::from(block) * usize::from(pcm.channels);
    for (i, c) in pcm.samples.chunks(per).enumerate() {
        v.extend_from_slice(&flac_frame(i as u64, c, pcm.channels, 0));
    }
    v
}

// ---------------------------------------------------------------------- WAV

/// A WAV header for a stream of unknown length (sizes 0xFFFFFFFF), as a
/// pipe-writing encoder emits.
pub fn wav_stream_header(rate: u32, channels: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(44);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&u32::MAX.to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&u32::MAX.to_le_bytes());
    v
}

pub fn s16le(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

// ------------------------------------------------------------------- server

/// How a canned body is framed.
#[derive(Clone, Copy, Debug)]
pub enum Framing {
    /// `Content-Length`.
    Length,
    /// `Transfer-Encoding: chunked`, 2048-byte chunks (HA's proxy).
    Chunked,
    /// No length, the body ends when the server closes (MA flow mode).
    Close,
}

/// A local HTTP server running `handler` on a thread per connection.
pub struct Server {
    pub addr: SocketAddr,
    /// Connections accepted.
    pub hits: Arc<AtomicUsize>,
    /// Body bytes written by handlers that report them.
    pub written: Arc<AtomicU64>,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

/// Reads the request head; returns it as text.
pub fn read_head(sock: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 16 * 1024 {
        match sock.read(&mut b) {
            Ok(1) => head.push(b[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

pub fn serve<F>(handler: F) -> Server
where
    F: Fn(TcpStream, &AtomicU64) + Send + Sync + 'static,
{
    serve_on("127.0.0.1:0", handler)
}

pub fn serve_on<F>(bind: &str, handler: F) -> Server
where
    F: Fn(TcpStream, &AtomicU64) + Send + Sync + 'static,
{
    let listener = TcpListener::bind(bind).unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let written = Arc::new(AtomicU64::new(0));
    let handler = Arc::new(handler);
    let (h, w) = (Arc::clone(&hits), Arc::clone(&written));
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(sock) = sock else { return };
            h.fetch_add(1, Ordering::SeqCst);
            let (handler, w) = (Arc::clone(&handler), Arc::clone(&w));
            std::thread::spawn(move || handler(sock, &w));
        }
    });
    Server {
        addr,
        hits,
        written,
    }
}

/// Writes a complete response with the given framing.
pub fn respond(sock: &mut TcpStream, ctype: &str, body: &[u8], framing: Framing) {
    let _ = read_head(sock);
    let head = match framing {
        Framing::Length => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ),
        Framing::Chunked => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nTransfer-Encoding: chunked\r\n\r\n"
        ),
        Framing::Close => {
            format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nConnection: close\r\n\r\n")
        }
    };
    if sock.write_all(head.as_bytes()).is_err() {
        return;
    }
    match framing {
        Framing::Chunked => {
            for c in body.chunks(2048) {
                if write!(sock, "{:x}\r\n", c.len()).is_err()
                    || sock.write_all(c).is_err()
                    || sock.write_all(b"\r\n").is_err()
                {
                    return;
                }
            }
            let _ = sock.write_all(b"0\r\n\r\n");
        }
        _ => {
            let _ = sock.write_all(body);
        }
    }
    let _ = sock.flush();
    let _ = sock.shutdown(std::net::Shutdown::Write);
    // Drain until the client closes, so the close is not a reset.
    let mut sink = [0u8; 256];
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    while matches!(sock.read(&mut sink), Ok(n) if n > 0) {}
}

/// Serves `body` with `framing` to every connection.
pub fn serve_body(ctype: &'static str, body: Vec<u8>, framing: Framing) -> Server {
    let body = Arc::new(body);
    serve(move |mut s, _| respond(&mut s, ctype, &body, framing))
}

/// Resident set size of this process in KiB.
pub fn rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

/// Names of this process's threads.
pub fn thread_names() -> Vec<String> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("comm")).ok())
        .map(|s| s.trim().to_owned())
        .collect()
}
