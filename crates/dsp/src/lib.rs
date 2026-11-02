//! Microphone front-end: S24 stereo capture frames to 16 kHz mono S16, with
//! optional acoustic echo cancellation.
//!
//! The built-in mic of the LG C3 delivers near-raw 24-bit samples (see
//! `docs/discovery/c3-a/D3-mic-identification.md`): DC is already removed, the
//! gain is fixed and speech at about 1 m is quiet. [`Frontend`] therefore does
//! four cheap things, in this order, on every frame:
//!
//! 1. channel selection ([`ChannelMode`]) and S24 sign-extension,
//! 2. a 2nd-order Butterworth high-pass filter (biquad) to remove rumble,
//! 3. optionally, echo cancellation ([`EchoCanceller`], WebRTC AEC3 through
//!    the pure Rust `sonora` crate) against the TV's output captured as a
//!    reference, on the unamplified signal,
//! 4. a fixed digital gain,
//! 5. conversion to S16 with hard saturation (clipped samples are counted).
//!
//! The mic is not resampled: the output rate equals the input rate, and the
//! caller must capture at the rate the consumer needs (16 kHz). The echo
//! reference may be 16 or 48 kHz; 48 kHz is decimated with a linear-phase
//! FIR. The design is in `docs/design/aec-frontend.md`.
//!
//! The crate contains no `unsafe` code. Without a canceller it does not
//! allocate in the hot path other than growing the caller's output `Vec`.

mod aec;
mod decimate;
mod delay;
mod fft;
mod frontend;

pub use aec::{
    AecConfig, AecMetrics, AecOutput, BYPASS_HOLD, BYPASS_WARMUP, DelayApply, DelayEvent,
    DelayMode, EchoCanceller, FRAME, LINEAR_LATENCY, MAX_DELAY_SAMPLES, MAX_RENDER_CHANNELS,
    MAX_WAIT_SAMPLES, MIN_CONFIDENCE, PREDELAY_MARGIN, RATE as AEC_RATE, RECHECK_SAMPLES,
    SILENT_PEAK, SONORA_LATENCY,
};
pub use delay::DelayEstimate;
pub use frontend::{ChannelMode, Frontend, FrontendConfig, Stats, sign_extend_s24};
