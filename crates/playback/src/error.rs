//! Error type shared by every stage of the playback pipeline.
//!
//! Messages never contain the request URL: HA TTS URLs carry a one-time token
//! in the path and must not end up in logs.

use std::time::Duration;

/// Failure in one stage of fetch, decode, resample or playback.
#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    /// The URL could not be parsed or has an unsupported scheme.
    #[error("invalid URL: {0}")]
    BadUrl(String),
    /// Transport failure (DNS, connect, protocol, I/O).
    #[error("HTTP request failed: {0}")]
    Http(String),
    /// TLS handshake or certificate validation failed.
    #[error("TLS error: {0}")]
    Tls(String),
    /// The server answered with a non-success status.
    #[error("HTTP status {0}")]
    Status(u16),
    /// The request did not finish within the configured timeout.
    #[error("request timed out")]
    Timeout,
    /// The response body is larger than `FetchConfig::max_bytes`.
    #[error("response body exceeds the {limit} byte limit")]
    TooLarge {
        /// The configured limit in bytes.
        limit: u64,
    },
    /// The bytes are not audio in a supported format, or are corrupt.
    #[error("decode error: {0}")]
    Decode(String),
    /// The audio is valid but outside what we support (for example 0 channels).
    #[error("unsupported audio: {0}")]
    Unsupported(String),
    /// The sink failed (spawn failure, broken pipe, non-zero exit).
    #[error("sink error: {0}")]
    Sink(String),
    /// The sink did not finish within the expected duration plus a margin.
    #[error("playback timed out after {0:?}")]
    PlayTimeout(Duration),
    /// A music stream received no byte for this long (socket read timeout).
    #[error("stream stalled: no data for {0:?}")]
    IdleTimeout(Duration),
    /// The body ended before its declared `Content-Length` (or inside a
    /// chunk of a chunked body).
    #[error("stream ended before its declared length")]
    EndedEarly,
    /// The fetch target is not allowed (loopback, link-local or this
    /// device's address, a downgrade or too many redirects, an oversized
    /// URL). The reason never contains the URL or the address.
    #[error("fetch target refused: {0}")]
    TargetRefused(String),
    /// Playback was cancelled through a `StopHandle` or a stream `Canceller`.
    #[error("playback cancelled")]
    Cancelled,
    /// Local I/O error (CA file, WAV output).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
