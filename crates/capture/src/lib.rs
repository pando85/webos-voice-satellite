//! Microphone capture for the webOS voice satellite.
//!
//! - [`AudioSource`]: the blocking source trait the rest of the program uses.
//!   Samples are interleaved `i32` holding sign-extended signed 24-bit values.
//! - [`AlsaCapture`] (Linux): reads an ALSA capture PCM through raw ioctls on
//!   `/dev/snd/pcmC{card}D{dev}c`. It does **not** use alsa-lib, because the
//!   TV's `asound.conf` routes `default` through PulseAudio and the TV build is
//!   a static musl binary. The card is resolved by **id** and the PCM by
//!   **name** from `/proc/asound`; indexes are never assumed.
//! - [`AlsaDuplex`] (Linux): the mic plus an echo-reference PCM on the same
//!   card (SoundEngineCapture on c3-a), linked so both start on one trigger,
//!   for the echo canceller. A reference that cannot be opened leaves the mic
//!   running alone ([`ReferenceState::Unavailable`]).
//! - [`FileSource`]: raw S24_LE (or S32_LE) interleaved files (what
//!   `arecord -t raw` writes), optionally paced in real time, for tests on a
//!   PC; [`FilePair`] reads a mic file and a reference file in lockstep.
//! - [`list_pcms`]: `/proc/asound` listing for diagnostics.
//!
//! Verified facts for the LG C3 (c3-a), see
//! `docs/discovery/c3-a/D3-mic-identification.md` and
//! `docs/discovery/c3-a/P1-capture-validation.md`: the PCM is exclusive and
//! returns `EBUSY` while LG hands-free voice holds it ([`CaptureError::MicBusy`]);
//! it accepts only period 256 / buffer 8192 at S24_LE, 2 ch, 16 kHz; the first
//! ~5 ms are a full-scale burst, so [`CaptureConfig::discard_ms`] frames are
//! dropped after every start and every overrun recovery. A read waits at most
//! [`CaptureConfig::read_timeout_ms`] for audio and then fails with
//! [`CaptureError::Stalled`], so a driver that stops delivering periods is
//! detected and the caller can reopen the device.
//!
//! `unsafe` is confined to the private `sys` module (ioctl struct layouts and
//! calls); the rest of the crate is safe code.

mod error;
mod file;
mod proc;
mod source;

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "linux")]
mod sys;

#[cfg(target_os = "linux")]
pub use alsa::{AlsaCapture, AlsaDuplex, CaptureConfig, ReferenceConfig};
pub use error::CaptureError;
pub use file::{FilePair, FileSource};
pub use proc::{PcmInfo, list_pcms};
pub use source::{AudioSource, ReferenceState, SampleFormat, StreamFormat};
