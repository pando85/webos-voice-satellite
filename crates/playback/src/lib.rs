//! TTS playback (stub).
//!
//! Phase 1.6 downloads, decodes and resamples reply audio and hands it to the
//! sink chosen by configuration. Nothing is played by this stub.

/// Output sample format requested from the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkFormat {
    /// Sample rate in Hz.
    pub rate_hz: u32,
    /// Number of interleaved channels.
    pub channels: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_is_copy() {
        let f = SinkFormat {
            rate_hz: 44_100,
            channels: 2,
        };
        let g = f;
        assert_eq!(f, g);
    }
}
