//! The source abstraction consumed by the rest of the satellite.

use crate::CaptureError;

/// Sample layout of raw capture data. Both formats use a 4-byte
/// little-endian container per sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SampleFormat {
    /// 24-bit samples in the low three bytes (`S24_LE`; the c3-a mic).
    #[default]
    S24Le,
    /// Full 32-bit samples (`S32_LE`; the c3-a SoundEngineCapture reference).
    S32Le,
}

impl SampleFormat {
    /// Converts one raw container to the crate's sample scale: a signed
    /// 24-bit value sign-extended to `i32`. `S32_LE` keeps its 24 most
    /// significant bits (an arithmetic shift; the low byte is dropped).
    #[must_use]
    #[inline]
    pub fn to_s24(self, raw: i32) -> i32 {
        match self {
            Self::S24Le => crate::file::sign_extend_s24(raw),
            Self::S32Le => raw >> 8,
        }
    }

    /// Lower-case name as used in configuration files (`s24_le`, `s32_le`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::S24Le => "s24_le",
            Self::S32Le => "s32_le",
        }
    }
}

/// Interleaved layout of a stream: channel count and rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFormat {
    /// Interleaved channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub rate: u32,
}

/// Status of the echo-reference stream of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceState {
    /// No reference was requested.
    Off,
    /// Open and delivering frames.
    Active {
        /// The mic and the reference start on one trigger
        /// (`SNDRV_PCM_IOCTL_LINK`). `false`: started back to back,
        /// reference first.
        linked: bool,
        /// Why the start is not linked, when it is not.
        unlinked_reason: Option<String>,
    },
    /// Requested but not available (the reason). The mic runs alone.
    Unavailable(String),
}

/// A blocking source of interleaved audio frames.
///
/// Samples are `i32` values holding a signed 24-bit sample sign-extended to 32
/// bits (the S24_LE format with the data in the low 24 bits).
///
/// A source may also capture an **echo reference** (what the TV plays) next
/// to the microphone. Reference frames read alongside the mic are collected
/// with [`AudioSource::take_reference`]. The alignment contract: after every
/// change of [`AudioSource::reference_epoch`] (and at the start), the first
/// mic frame returned and the first reference frame collected were captured
/// at the same instant, up to the start skew of the two streams. Sources
/// without a reference keep the defaults (no frames, epoch 0, state `Off`).
pub trait AudioSource: Send {
    /// Reads whole frames into `out` and returns the number of **frames**
    /// stored (so `out[..n * channels]` is valid). Blocks until data is
    /// available. `Ok(0)` means end of stream (files only) or that `out` holds
    /// less than one frame. A trailing partial frame of `out` is left untouched.
    ///
    /// # Errors
    /// See [`CaptureError`].
    fn read(&mut self, out: &mut [i32]) -> Result<usize, CaptureError>;

    /// Interleaved channel count.
    fn channels(&self) -> u16;

    /// Sample rate in Hz.
    fn rate(&self) -> u32;

    /// Number of overruns recovered so far. Sources without a ring buffer
    /// report 0.
    fn xruns(&self) -> u64 {
        0
    }

    /// Layout of the reference stream, if one is active.
    fn reference_format(&self) -> Option<StreamFormat> {
        None
    }

    /// Appends the reference frames captured since the last call
    /// (interleaved, same sample scale as the mic) to `out`.
    fn take_reference(&mut self, out: &mut Vec<i32>) {
        let _ = out;
    }

    /// Bumped whenever mic and reference restart together (overrun recovery,
    /// file loop): reference frames before and after are not contiguous.
    fn reference_epoch(&self) -> u64 {
        0
    }

    /// Status of the reference stream.
    fn reference_state(&self) -> ReferenceState {
        ReferenceState::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s32_keeps_the_top_24_bits() {
        assert_eq!(SampleFormat::S32Le.to_s24(0x7FFF_FFFF), 0x7F_FFFF);
        assert_eq!(SampleFormat::S32Le.to_s24(i32::MIN), -0x80_0000);
        assert_eq!(SampleFormat::S32Le.to_s24(0x0000_0100), 1);
        assert_eq!(SampleFormat::S32Le.to_s24(-256), -1);
        assert_eq!(SampleFormat::S24Le.to_s24(0x00FF_FFFF), -1);
        assert_eq!(SampleFormat::S24Le.name(), "s24_le");
    }
}
