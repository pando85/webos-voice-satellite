//! ESPHome native API server (stub).
//!
//! Phase 1.3 implements frame handling (plaintext and Noise), hello/connect,
//! device info, entity listing and the voice-assistant messages.

/// Plaintext frame preamble byte of the ESPHome native API.
pub const PLAINTEXT_PREAMBLE: u8 = 0x00;
/// Noise-encrypted frame preamble byte of the ESPHome native API.
pub const NOISE_PREAMBLE: u8 = 0x01;
/// Default TCP port of the ESPHome native API.
pub const DEFAULT_PORT: u16 = 6053;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants() {
        assert_ne!(PLAINTEXT_PREAMBLE, NOISE_PREAMBLE);
        assert_eq!(DEFAULT_PORT, 6053);
    }
}
