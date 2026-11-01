//! Interleaved 16-bit PCM buffer used between the pipeline stages.

use std::time::Duration;

/// Decoded audio: interleaved signed 16-bit samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    /// Sample rate in Hz.
    pub rate: u32,
    /// Number of interleaved channels (at least 1).
    pub channels: u16,
    /// Interleaved samples, `frames * channels` long.
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Number of sample frames (one sample per channel).
    #[must_use]
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / usize::from(self.channels)
        }
    }

    /// Playing time of the buffer.
    #[must_use]
    pub fn duration(&self) -> Duration {
        if self.rate == 0 {
            return Duration::ZERO;
        }
        // frames / rate, computed without float rounding surprises.
        let frames = self.frames() as u64;
        let rate = u64::from(self.rate);
        Duration::new(
            frames / rate,
            u32::try_from((frames % rate) * 1_000_000_000 / rate).unwrap_or(0),
        )
    }

    /// Little-endian byte representation (`s16le`), as written to a sink.
    #[must_use]
    pub fn to_s16le_bytes(&self) -> Vec<u8> {
        self.samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    /// Multiplies every sample by `gain` (clamped to the 16-bit range).
    /// A gain of 1.0 is a no-op; negative or NaN gains are treated as 0.
    pub fn apply_gain(&mut self, gain: f32) {
        if (gain - 1.0).abs() <= f32::EPSILON {
            return;
        }
        let g = if gain.is_nan() { 0.0 } else { gain.max(0.0) };
        for s in &mut self.samples {
            #[allow(clippy::cast_possible_truncation)]
            {
                *s = (f32::from(*s) * g).round().clamp(-32768.0, 32767.0) as i16;
            }
        }
    }

    /// Digital silence of the given length, useful for tests and silent trials.
    #[must_use]
    pub fn silence(rate: u32, channels: u16, frames: usize) -> Self {
        Self {
            rate,
            channels,
            samples: vec![0; frames * usize::from(channels)],
        }
    }

    /// True when every sample is zero.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.samples.iter().all(|&s| s == 0)
    }
}
