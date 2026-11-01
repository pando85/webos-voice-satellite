//! Microphone front-end: S24 stereo capture frames to 16 kHz mono S16.
//!
//! The built-in mic of the LG C3 delivers near-raw 24-bit samples (see
//! `docs/discovery/c3-a/D3-mic-identification.md`): DC is already removed, the
//! gain is fixed and speech at about 1 m is quiet. [`Frontend`] therefore does
//! four cheap things, in this order, on every frame:
//!
//! 1. channel selection ([`ChannelMode`]) and S24 sign-extension,
//! 2. a 2nd-order Butterworth high-pass filter (biquad) to remove rumble,
//! 3. a fixed digital gain,
//! 4. conversion to S16 with hard saturation (clipped samples are counted).
//!
//! There is no resampling: the output rate equals the input rate, and the
//! caller must capture at the rate the consumer needs (16 kHz).
//!
//! The crate is pure Rust, contains no `unsafe` code, and does not allocate in
//! the hot path other than growing the caller's output `Vec`.

mod frontend;

pub use frontend::{ChannelMode, Frontend, FrontendConfig, Stats, sign_extend_s24};
