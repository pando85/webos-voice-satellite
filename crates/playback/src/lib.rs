//! TTS reply playback: fetch, decode, resample, play.
//!
//! Pipeline used by the satellite when Home Assistant sends a TTS URL or an
//! announcement: [`play_url`] **streams** it, so its memory does not grow
//! with the length of the audio: the guarded fetch and an incremental
//! decoder on two threads ([`stream`], `reply-fetch` and `reply-decode`,
//! with bounded buffers), a [`StreamResampler`] to the sink's format, and a
//! [`SinkWriter`] ([`Sink::open`]; [`PacatSink`] on the TV) that takes each
//! chunk as it is decoded. The whole-buffer stages are still here for local
//! files and tests: [`fetch`] (HTTP(S), rustls), [`decode`] (symphonia),
//! [`resample`] and [`Sink::play`]; [`play_bytes`] runs them on a file.
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
pub mod guard;
mod pcm;
mod resample;
mod sink;
pub mod stream;

use std::time::{Duration, Instant};

pub use decode::{MAX_DECODED_SAMPLES, decode};
pub use error::PlaybackError;
pub use fetch::{
    DEFAULT_MAX_BYTES, FetchConfig, Fetched, REPLY_DECODE_THREAD_NAME, REPLY_FETCH_THREAD_NAME,
    REPLY_RING_BYTES, fetch, fetch_with_meta,
};
pub use pcm::Pcm;
pub use resample::{StreamResampler, mix_channels, resample};
pub use sink::{
    NullSink, PacatSink, PlayFuture, Sink, SinkFormat, SinkWriter, StopHandle, WavFileSink,
    wav_bytes,
};

/// Wall-clock cost of each stage of [`play_url`] (or [`play_bytes`]), for
/// latency logging.
///
/// For the streamed [`play_url`] the stages overlap: `fetch` is the time to
/// the first decoded audio (connect, headers, first packet), `decode` the
/// further time spent waiting for decoded audio, `resample` the conversion
/// time and `play` the time in the sink (writing, which waits for `pacat`,
/// and draining).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// Download time (streamed: time to the first decoded audio).
    pub fetch: Duration,
    /// Decode time (streamed: waiting for decoded audio after the first).
    pub decode: Duration,
    /// Resample and channel-mix time.
    pub resample: Duration,
    /// Time inside the sink (about the audio duration for a real sink).
    pub play: Duration,
    /// Size of the downloaded body.
    pub bytes: usize,
    /// Duration of the audio that was played.
    pub audio: Duration,
}

/// Streams `url` to `sink`: fetch, decode, convert to the sink's format and
/// play, chunk by chunk, with bounded buffers (see the [crate
/// documentation](crate)). Resolves when the sink has played everything
/// (for `pacat`: when it has drained and exited).
///
/// The fetch follows [`FetchConfig::reply_stream`]: the target rules of
/// [`guard`], the body cap `max_bytes` (a larger `Content-Length` is
/// refused before any audio) and at most [`MAX_DECODED_SAMPLES`] decoded
/// samples, as in the whole-file pipeline. The decoder format hint comes
/// from the response `Content-Type` or the URL's extension. The sink is
/// opened only once the first audio is decoded, so an unreachable or
/// undecodable URL plays nothing. A [`StopHandle::stop`] on the sink's
/// handle ends it at once with [`PlaybackError::Cancelled`]; so does
/// dropping the future (the `pacat` child and both threads stop either way).
///
/// # Errors
/// Any [`PlaybackError`] from the fetch, the decoder or the sink. An error
/// after audio has started stops the output there.
pub async fn play_url(
    url: &str,
    cfg: &FetchConfig,
    sink: &mut dyn Sink,
) -> Result<Timings, PlaybackError> {
    let t0 = Instant::now();
    let mut stop = sink.stop_handle().subscribe();
    let mut source = stream::open(url, &cfg.reply_stream())?;
    let fmt = sink.format();

    // The first chunk sets up the converter; nothing is opened before it.
    let first = tokio::select! {
        biased;
        _ = stop.changed() => return Err(PlaybackError::Cancelled),
        item = source.recv() => item,
    };
    let first = match first {
        Some(Ok(pcm)) => pcm,
        Some(Err(e)) => return Err(e),
        None => return Err(PlaybackError::Decode("no audio frames decoded".into())),
    };
    let mut timings = Timings {
        fetch: t0.elapsed(),
        decode: Duration::ZERO,
        resample: Duration::ZERO,
        play: Duration::ZERO,
        bytes: 0,
        audio: Duration::ZERO,
    };
    let mut conv = StreamResampler::new(first.rate, first.channels, fmt.rate_hz, fmt.channels);
    let mut decoded_samples = 0usize;
    let mut frames_out = 0usize;
    let mut writer = sink.open()?;
    let mut next = Some(first);
    loop {
        let pcm = if let Some(p) = next.take() {
            p
        } else {
            let t = Instant::now();
            let item = tokio::select! {
                biased;
                _ = stop.changed() => return Err(PlaybackError::Cancelled),
                item = source.recv() => item,
            };
            timings.decode += t.elapsed();
            match item {
                Some(Ok(p)) => p,
                Some(Err(e)) => return Err(e),
                None => break,
            }
        };
        decoded_samples += pcm.samples.len();
        if decoded_samples > MAX_DECODED_SAMPLES {
            return Err(PlaybackError::Unsupported("decoded audio too long".into()));
        }
        let t = Instant::now();
        let out = conv.push(&pcm);
        drop(pcm);
        timings.resample += t.elapsed();
        write_chunk(writer.as_mut(), &out, &mut stop, &mut timings).await?;
        frames_out += out.frames();
    }
    timings.bytes = usize::try_from(source.stats().bytes_received).unwrap_or(usize::MAX);
    // Both threads have ended; free their buffers before the drain.
    drop(source);
    let t = Instant::now();
    let tail = conv.flush();
    timings.resample += t.elapsed();
    write_chunk(writer.as_mut(), &tail, &mut stop, &mut timings).await?;
    frames_out += tail.frames();
    let t = Instant::now();
    tokio::select! {
        biased;
        _ = stop.changed() => return Err(PlaybackError::Cancelled),
        r = writer.finish() => r?,
    }
    timings.play += t.elapsed();
    timings.audio = frames_duration(frames_out, fmt.rate_hz);
    Ok(timings)
}

/// Writes one converted chunk, unless a stop comes first.
async fn write_chunk(
    writer: &mut dyn SinkWriter,
    pcm: &Pcm,
    stop: &mut tokio::sync::watch::Receiver<u64>,
    timings: &mut Timings,
) -> Result<(), PlaybackError> {
    if pcm.samples.is_empty() {
        return Ok(());
    }
    let t = Instant::now();
    tokio::select! {
        biased;
        _ = stop.changed() => return Err(PlaybackError::Cancelled),
        r = writer.write(pcm) => r?,
    }
    timings.play += t.elapsed();
    Ok(())
}

fn frames_duration(frames: usize, rate: u32) -> Duration {
    if rate == 0 {
        return Duration::ZERO;
    }
    let frames = frames as u64;
    let rate = u64::from(rate);
    Duration::new(
        frames / rate,
        u32::try_from((frames % rate) * 1_000_000_000 / rate).unwrap_or(0),
    )
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
