//! Error type shared by all capture sources.

use std::io;

/// Errors from opening or reading an audio source.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The capture PCM is held by another process (`EBUSY`). On the LG C3 this
    /// means LG hands-free voice is on and `voiceinput` owns the mic. Not
    /// retried: the caller decides what to do.
    #[error("capture device is busy (another process holds it; is LG hands-free voice on?)")]
    MicBusy,
    /// No card or PCM matched the configuration.
    #[error("capture device not found: {0}")]
    DeviceNotFound(String),
    /// The device or the configuration is not supported (format, rate,
    /// channels, period or buffer size rejected by the driver).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The device went away (`ENODEV`), for example card removal.
    #[error("capture device disconnected")]
    Disconnected,
    /// Overruns kept happening and re-preparing the stream did not help.
    #[error("xrun recovery failed after {attempts} consecutive attempts: {source}")]
    XrunRecoveryFailed {
        /// Consecutive recoveries tried without a successful read.
        attempts: u32,
        /// The last error.
        source: io::Error,
    },
    /// Any other I/O error.
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
}
