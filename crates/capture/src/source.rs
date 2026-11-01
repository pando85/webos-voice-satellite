//! The source abstraction consumed by the rest of the satellite.

use crate::CaptureError;

/// A blocking source of interleaved audio frames.
///
/// Samples are `i32` values holding a signed 24-bit sample sign-extended to 32
/// bits (the S24_LE format with the data in the low 24 bits).
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
}
