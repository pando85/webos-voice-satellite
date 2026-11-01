//! ALSA capture (stub).
//!
//! Phase 1.2 implements capture from the sound card selected by name (never by
//! index) with the parameters recorded in the discovery logs. This stub only
//! fixes the shape of the configuration so other crates can depend on it.

/// Configuration for opening a capture device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    /// ALSA card name, resolved through `/proc/asound/cards` at runtime.
    pub card_name: String,
    /// PCM device number on the card.
    pub device: u32,
    /// Sample rate in Hz.
    pub rate_hz: u32,
    /// Number of interleaved channels the device delivers.
    pub channels: u32,
}

impl CaptureConfig {
    /// Creates a configuration for the given card name with 16 kHz, 2 channels.
    #[must_use]
    pub fn new(card_name: impl Into<String>) -> Self {
        Self {
            card_name: card_name.into(),
            device: 0,
            rate_hz: 16_000,
            channels: 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shape() {
        let c = CaptureConfig::new("example");
        assert_eq!(c.card_name, "example");
        assert_eq!((c.device, c.rate_hz, c.channels), (0, 16_000, 2));
    }
}
