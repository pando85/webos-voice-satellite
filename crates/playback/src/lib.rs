//! TTS reply playback: fetch, decode, resample, play.
//!
//! Pipeline used by the satellite when Home Assistant sends a TTS URL:
//! [`fetch`] (HTTP(S), rustls) then [`decode`] (symphonia) then [`resample`]
//! to the sink's format then [`Sink::play`] ([`PacatSink`] on the TV).
//! [`play_url`] runs all four and reports per-stage timings.
//!
//! The [`music`] module is the music engine (second `pacat`, gain, ducking, pacer).
//! [`stream`] is the music source: a streaming fetch and an incremental
//! decoder on two named threads, for bodies of unknown or endless length.
//!
//! Everything is pure Rust except the external `pacat` process, so the crate
//! builds as a static musl binary for the TV.

pub mod music;

mod decode;
mod error;
mod fetch;
mod pcm;
mod resample;
mod sink;
pub mod stream;

use std::time::{Duration, Instant};

pub use decode::{MAX_DECODED_SAMPLES, decode};
pub use error::PlaybackError;
pub use fetch::{DEFAULT_MAX_BYTES, FetchConfig, Fetched, fetch, fetch_with_meta};
pub use pcm::Pcm;
pub use resample::{mix_channels, resample};
pub use sink::{
    NullSink, PacatSink, PlayFuture, Sink, SinkFormat, StopHandle, WavFileSink, wav_bytes,
};

/// Wall-clock cost of each stage of [`play_url`], for latency logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// Download time.
    pub fetch: Duration,
    /// Decode time.
    pub decode: Duration,
    /// Resample and channel-mix time.
    pub resample: Duration,
    /// Time inside [`Sink::play`] (about the audio duration for a real sink).
    pub play: Duration,
    /// Size of the downloaded body.
    pub bytes: usize,
    /// Duration of the audio that was played.
    pub audio: Duration,
}

/// Fetches `url`, decodes it, converts it to the sink's format and plays it.
///
/// The decoder format hint comes from the response `Content-Type`.
///
/// # Errors
/// Any [`PlaybackError`] from the four stages.
pub async fn play_url(
    url: &str,
    cfg: &FetchConfig,
    sink: &mut dyn Sink,
) -> Result<Timings, PlaybackError> {
    let t0 = Instant::now();
    let fetched = fetch_with_meta(url, cfg).await?;
    let fetch_t = t0.elapsed();
    play_bytes(
        &fetched.bytes,
        fetched.content_type.as_deref(),
        sink,
        fetch_t,
    )
    .await
}

/// Decodes and plays an in-memory file (used for local test files).
///
/// `fetch_time` is only copied into the returned [`Timings`].
///
/// # Errors
/// Any [`PlaybackError`] from decode, resample or the sink.
pub async fn play_bytes(
    bytes: &[u8],
    hint: Option<&str>,
    sink: &mut dyn Sink,
    fetch_time: Duration,
) -> Result<Timings, PlaybackError> {
    let t = Instant::now();
    let pcm = decode(bytes, hint)?;
    let decode_t = t.elapsed();

    let fmt = sink.format();
    let t = Instant::now();
    let out = resample(&pcm, fmt.rate_hz, fmt.channels);
    let resample_t = t.elapsed();

    let t = Instant::now();
    sink.play(&out).await?;
    Ok(Timings {
        fetch: fetch_time,
        decode: decode_t,
        resample: resample_t,
        play: t.elapsed(),
        bytes: bytes.len(),
        audio: out.duration(),
    })
}
