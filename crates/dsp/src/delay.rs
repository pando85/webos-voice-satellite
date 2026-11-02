//! Bulk delay between the mic and the echo reference, by GCC-PHAT.
//!
//! Mic and reference (as fed to the canceller, both 16 kHz) are cut into
//! blocks of [`BLOCK`] samples. Blocks where the reference is active are
//! transformed (one complex FFT of `mic + i * ref`, zero-padded to twice the
//! block), and their cross-spectra `M * conj(R)` are summed in the 200 to
//! 4000 Hz band. After [`BLOCKS_PER_ESTIMATE`] active blocks the sum is
//! whitened (phase transform), transformed back, and the peak inside the
//! lag window is the delay: a positive lag means the mic lags the reference.
//! The peak-to-RMS ratio of the correlation over the lag window is the
//! confidence.

// Float/int casts are the point of DSP code here: indices, frame counts and
// sample values are bounded (frames of 160, blocks of 4096, delays below
// 2560 samples, stream positions far below 2^53).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use crate::fft::{Complex, Fft};

/// Samples per analysis block (256 ms at 16 kHz).
pub(crate) const BLOCK: usize = 4096;
/// Active blocks summed per estimate (about 1 s of reference activity).
pub(crate) const BLOCKS_PER_ESTIMATE: usize = 4;
/// A block counts as active when the reference RMS is above this
/// (-70 dBFS, full scale 1.0) and the mic is not digital silence.
const REF_ACTIVE_RMS: f32 = 3.16e-4;
const MIC_ACTIVE_RMS: f32 = 1e-6;
/// Band used for the estimate, in Hz.
const BAND_HZ: (f32, f32) = (200.0, 4000.0);
/// Samples around the peak excluded from the confidence RMS.
const PEAK_GUARD: usize = 8;

/// One delay estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DelayEstimate {
    /// Lag in samples (positive: the mic lags the reference), integer.
    pub lag: i32,
    /// Lag with parabolic sub-sample interpolation, in samples.
    pub lag_fine: f32,
    /// Peak over the RMS of the correlation in the lag window.
    pub confidence: f32,
}

/// Streaming GCC-PHAT delay estimator.
#[derive(Debug, Clone)]
pub(crate) struct DelayEstimator {
    fft: Fft,
    mic: Vec<f32>,
    reference: Vec<f32>,
    acc: Vec<Complex>,
    blocks: usize,
    min_lag: i32,
    max_lag: i32,
    lo_bin: usize,
    hi_bin: usize,
    scratch: Vec<Complex>,
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    let n = x.len() as f32;
    (x.iter().map(|v| v * v).sum::<f32>() / n).sqrt()
}

impl DelayEstimator {
    /// Searches lags in `min_lag..=max_lag` samples at `rate` Hz. The window
    /// is clamped to what a block can resolve.
    pub(crate) fn new(rate: u32, min_lag: i32, max_lag: i32) -> Self {
        let n = 2 * BLOCK;
        let limit = (BLOCK - 1) as i32;
        let bin_hz = rate as f32 / n as f32;
        let lo_bin = ((BAND_HZ.0 / bin_hz).ceil() as usize).max(1);
        let hi_bin = ((BAND_HZ.1 / bin_hz).floor() as usize).min(n / 2 - 1);
        Self {
            fft: Fft::new(n),
            mic: Vec::with_capacity(BLOCK),
            reference: Vec::with_capacity(BLOCK),
            acc: vec![(0.0, 0.0); n / 2 + 1],
            blocks: 0,
            min_lag: min_lag.clamp(-limit, limit),
            max_lag: max_lag.clamp(-limit, limit),
            lo_bin,
            hi_bin,
            scratch: vec![(0.0, 0.0); n],
        }
    }

    /// Drops the partial block and the accumulated spectrum.
    pub(crate) fn reset(&mut self) {
        self.mic.clear();
        self.reference.clear();
        self.acc.fill((0.0, 0.0));
        self.blocks = 0;
    }

    /// Active blocks accumulated toward the next estimate.
    #[cfg(test)]
    pub(crate) fn blocks(&self) -> usize {
        self.blocks
    }

    /// Feeds aligned mic and reference samples (same length). Returns an
    /// estimate when enough active blocks have been summed; the accumulator
    /// then starts over.
    pub(crate) fn push(&mut self, mic: &[f32], reference: &[f32]) -> Option<DelayEstimate> {
        let mut result = None;
        let mut i = 0;
        let n = mic.len().min(reference.len());
        while i < n {
            let take = (BLOCK - self.mic.len()).min(n - i);
            self.mic.extend_from_slice(&mic[i..i + take]);
            self.reference.extend_from_slice(&reference[i..i + take]);
            i += take;
            if self.mic.len() == BLOCK {
                if rms(&self.reference) > REF_ACTIVE_RMS && rms(&self.mic) > MIC_ACTIVE_RMS {
                    self.accumulate();
                    if self.blocks >= BLOCKS_PER_ESTIMATE {
                        result = Some(self.estimate());
                        self.acc.fill((0.0, 0.0));
                        self.blocks = 0;
                    }
                }
                self.mic.clear();
                self.reference.clear();
            }
        }
        result
    }

    fn accumulate(&mut self) {
        let n = self.fft.len();
        for (k, z) in self.scratch.iter_mut().enumerate() {
            *z = if k < BLOCK {
                (self.mic[k], self.reference[k])
            } else {
                (0.0, 0.0)
            };
        }
        self.fft.forward(&mut self.scratch);
        // Split the packed transform: M = (Z[k] + conj Z[n-k]) / 2,
        // R = (Z[k] - conj Z[n-k]) / 2i; accumulate M * conj(R).
        for k in self.lo_bin..=self.hi_bin {
            let (zr, zi) = self.scratch[k];
            let (yr, yi) = self.scratch[n - k];
            let (mr, mi) = (f32::midpoint(zr, yr), 0.5 * (zi - yi));
            let (rr, ri) = (f32::midpoint(zi, yi), -0.5 * (zr - yr));
            let a = &mut self.acc[k];
            a.0 += mr * rr + mi * ri;
            a.1 += mi * rr - mr * ri;
        }
        self.blocks += 1;
    }

    fn estimate(&mut self) -> DelayEstimate {
        let n = self.fft.len();
        self.scratch.fill((0.0, 0.0));
        for k in self.lo_bin..=self.hi_bin {
            let (re, im) = self.acc[k];
            let mag = (re * re + im * im).sqrt();
            if mag > 0.0 {
                self.scratch[k] = (re / mag, im / mag);
                self.scratch[n - k] = (re / mag, -im / mag);
            }
        }
        self.fft.inverse(&mut self.scratch);
        let at = |lag: i32| -> f32 { self.scratch[(lag.rem_euclid(n as i32)) as usize].0 };
        let mut best = self.min_lag;
        for lag in self.min_lag..=self.max_lag {
            if at(lag) > at(best) {
                best = lag;
            }
        }
        let peak = at(best);
        let (mut sum, mut cnt) = (0.0_f32, 0_u32);
        for lag in self.min_lag..=self.max_lag {
            if lag.abs_diff(best) as usize > PEAK_GUARD {
                let v = at(lag);
                sum += v * v;
                cnt += 1;
            }
        }
        let floor = if cnt > 0 {
            (sum / cnt as f32).sqrt()
        } else {
            0.0
        };
        let confidence = if floor > 0.0 { peak / floor } else { 0.0 };
        let (l, c, r) = (at(best - 1), peak, at(best + 1));
        let den = l - 2.0 * c + r;
        let frac = if den.abs() > f32::EPSILON {
            (0.5 * (l - r) / den).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let lag_fine = best as f32 + frac;
        DelayEstimate {
            lag: best,
            lag_fine,
            confidence,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]

    use super::*;

    /// Deterministic white-ish noise in [-a, a].
    pub(crate) fn noise(n: usize, a: f32, seed: u32) -> Vec<f32> {
        let mut s = seed.max(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * a
            })
            .collect()
    }

    fn echo(reference: &[f32], delay: usize, gain: f32, noise_amp: f32) -> Vec<f32> {
        let nz = noise(reference.len(), noise_amp, 99);
        (0..reference.len())
            .map(|i| {
                let e = if i >= delay {
                    reference[i - delay]
                } else {
                    0.0
                };
                // A little colouring: direct path plus one reflection.
                let r2 = if i >= delay + 37 {
                    reference[i - delay - 37]
                } else {
                    0.0
                };
                gain * (e + 0.3 * r2) + nz[i]
            })
            .collect()
    }

    #[test]
    fn finds_a_known_delay_in_noise() {
        let reference = noise(16_000 * 3, 0.05, 7);
        for delay in [0_usize, 1, 230, 800, 2400] {
            let mic = echo(&reference, delay, 0.2, 0.002);
            let mut est = DelayEstimator::new(16_000, -256, 2560);
            let mut got = None;
            for (m, r) in mic.chunks(160).zip(reference.chunks(160)) {
                if let Some(e) = est.push(m, r) {
                    got = Some(e);
                    break;
                }
            }
            let e = got.unwrap_or_else(|| panic!("no estimate for {delay}"));
            assert_eq!(e.lag, delay as i32, "{e:?}");
            assert!(e.confidence > 8.0, "{e:?}");
            assert!((e.lag_fine - delay as f32).abs() < 0.5, "{e:?}");
        }
    }

    #[test]
    fn negative_lag_is_reported() {
        // The reference lags the mic (non-causal as fed): lag -100.
        let mic = noise(16_000 * 2, 0.05, 3);
        let reference = echo(&mic, 100, 1.0, 0.0);
        let mut est = DelayEstimator::new(16_000, -256, 2560);
        let e = est
            .push(&mic, &reference)
            .unwrap_or_else(|| panic!("no estimate"));
        assert_eq!(e.lag, -100);
    }

    #[test]
    fn silent_reference_gives_no_estimate_and_uncorrelated_is_low_confidence() {
        let mut est = DelayEstimator::new(16_000, -256, 2560);
        let mic = noise(16_000 * 4, 0.05, 5);
        assert!(est.push(&mic, &vec![0.0; mic.len()]).is_none());
        assert_eq!(est.blocks(), 0);
        let other = noise(16_000 * 4, 0.05, 6);
        let e = est
            .push(&mic, &other)
            .unwrap_or_else(|| panic!("no estimate"));
        assert!(e.confidence < 6.0, "{e:?}");
        est.reset();
        assert_eq!(est.blocks(), 0);
    }
}
