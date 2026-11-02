//! Reference conditioning: downmix to mono, scale to `[-1, 1)`, and
//! decimate 48 kHz to 16 kHz with a linear-phase FIR (or pass 16 kHz
//! through).

// Float/int casts are the point of DSP code here: indices, frame counts and
// sample values are bounded (frames of 160, blocks of 4096, delays below
// 2560 samples, stream positions far below 2^53).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::f64::consts::PI;

/// Full scale of the S24 sample scale the capture crate delivers.
const S24_FULL_SCALE: f32 = 8_388_608.0;
/// FIR length for 3:1 decimation. Odd, with `(TAPS - 1) / 2` a multiple of 3
/// so the group delay is a whole number of output samples.
const TAPS: usize = 145;
/// Cutoff (-6 dB point) of the anti-alias filter in Hz at 48 kHz.
const CUTOFF_HZ: f64 = 7_000.0;
/// Kaiser window beta: about 80 dB stop-band attenuation.
const KAISER_BETA: f64 = 8.0;

/// Zeroth-order modified Bessel function of the first kind (power series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        let kf = f64::from(k);
        term *= q / (kf * kf);
        sum += term;
        if term < sum * 1e-12 {
            break;
        }
    }
    sum
}

/// Kaiser-windowed sinc low-pass, normalised to unit DC gain.
fn design_lowpass(taps: usize, cutoff_hz: f64, rate_hz: f64, beta: f64) -> Vec<f32> {
    let m = (taps - 1) as f64 / 2.0;
    let fc = cutoff_hz / rate_hz;
    let i0b = bessel_i0(beta);
    let h: Vec<f64> = (0..taps)
        .map(|n| {
            let t = n as f64 - m;
            let sinc = if t == 0.0 {
                2.0 * fc
            } else {
                (2.0 * PI * fc * t).sin() / (PI * t)
            };
            let r = t / m;
            let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
            sinc * w
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| (v / sum) as f32).collect()
}

/// Turns interleaved reference frames into 16 kHz mono `f32` samples.
#[derive(Debug, Clone)]
pub(crate) struct RefResampler {
    factor: usize,
    taps: Vec<f32>,
    /// Mono input history (the last `TAPS - 1` samples) plus new input.
    buf: Vec<f32>,
    /// Input samples seen modulo `factor`: where the next output falls.
    phase: usize,
    scale: f32,
}

impl RefResampler {
    /// `rate` must be 16000 (pass-through) or 48000 (3:1 decimation).
    /// `gain` is linear and applied with the S24-to-float scaling.
    pub(crate) fn new(rate: u32, gain: f32) -> Option<Self> {
        let factor = match rate {
            16_000 => 1,
            48_000 => 3,
            _ => return None,
        };
        let taps = if factor == 1 {
            Vec::new()
        } else {
            design_lowpass(TAPS, CUTOFF_HZ, 48_000.0, KAISER_BETA)
        };
        let hist = taps.len().saturating_sub(1);
        Some(Self {
            factor,
            taps,
            buf: vec![0.0; hist],
            phase: 0,
            scale: gain / S24_FULL_SCALE,
        })
    }

    /// Group delay the filter adds, in 16 kHz output samples.
    pub(crate) fn delay_samples(&self) -> usize {
        self.taps.len().saturating_sub(1) / 2 / self.factor
    }

    /// Clears the history (after a gap in the stream).
    pub(crate) fn reset(&mut self) {
        let hist = self.taps.len().saturating_sub(1);
        self.buf.clear();
        self.buf.resize(hist, 0.0);
        self.phase = 0;
    }

    /// Downmixes `interleaved` (`ch` channels, S24 scale) and appends the
    /// 16 kHz output to `out`.
    pub(crate) fn push(&mut self, interleaved: &[i32], ch: usize, out: &mut Vec<f32>) {
        if ch == 0 {
            return;
        }
        let k = self.scale / ch as f32;
        let mono = interleaved.chunks_exact(ch).map(|f| {
            let sum = f.iter().map(|&v| v as f32).sum::<f32>();
            sum * k
        });
        if self.factor == 1 {
            out.extend(mono);
            return;
        }
        let hist = self.taps.len() - 1;
        self.buf.extend(mono);
        // `phase` is the input count modulo `factor`: the first new sample
        // that lands on an output position is `pos`.
        let mut pos = (self.factor - self.phase) % self.factor;
        while hist + pos < self.buf.len() {
            let i = hist + pos;
            let window = &self.buf[i - hist..=i];
            // Taps are symmetric, so the convolution order does not matter.
            let y: f32 = window.iter().zip(&self.taps).map(|(a, b)| a * b).sum();
            out.push(y);
            pos += self.factor;
        }
        let consumed = self.buf.len() - hist;
        self.phase = (self.phase + consumed) % self.factor;
        self.buf.drain(..consumed);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

    use super::*;

    fn sine48(freq: f64, amp: f64, n: usize) -> Vec<i32> {
        (0..n)
            .flat_map(|i| {
                let v = (amp * 8_388_607.0 * (2.0 * PI * freq * i as f64 / 48_000.0).sin()) as i32;
                [v, v]
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn passband_is_flat_and_delay_is_whole_samples() {
        let mut r = RefResampler::new(48_000, 1.0).unwrap_or_else(|| panic!("rate"));
        assert_eq!(r.delay_samples(), 24);
        let input = sine48(1000.0, 0.5, 48_000);
        let mut out = Vec::new();
        // Odd-sized pushes: the phase must carry across calls.
        for chunk in input.chunks(2 * 257) {
            r.push(chunk, 2, &mut out);
        }
        assert_eq!(out.len(), 16_000);
        let tail = &out[1000..];
        let db = 20.0 * (rms(tail) / (0.5 / 2f64.sqrt())).log10();
        assert!(db.abs() < 0.05, "1 kHz gain {db} dB");
        // Delay: the output matches the 16 kHz sine shifted by 24 samples.
        let err: f64 = (2000..3000)
            .map(|n| {
                let want = 0.5 * (2.0 * PI * 1000.0 * (n as f64 - 24.0) / 16_000.0).sin();
                (f64::from(out[n]) - want).powi(2)
            })
            .sum::<f64>()
            / 1000.0;
        assert!(err.sqrt() < 1e-3, "delay mismatch {}", err.sqrt());
        // 6 kHz is still in the pass band.
        let mut r = RefResampler::new(48_000, 1.0).unwrap_or_else(|| panic!("rate"));
        let mut out = Vec::new();
        r.push(&sine48(6000.0, 0.5, 48_000), 2, &mut out);
        let db = 20.0 * (rms(&out[1000..]) / (0.5 / 2f64.sqrt())).log10();
        assert!(db > -1.0, "6 kHz at {db} dB");
    }

    #[test]
    fn aliases_are_rejected() {
        // 10 kHz would fold to 6 kHz.
        let mut r = RefResampler::new(48_000, 1.0).unwrap_or_else(|| panic!("rate"));
        let mut out = Vec::new();
        r.push(&sine48(10_000.0, 0.5, 48_000), 2, &mut out);
        let db = 20.0 * (rms(&out[1000..]) / (0.5 / 2f64.sqrt())).log10();
        assert!(db < -60.0, "10 kHz leaks at {db} dB");
    }

    #[test]
    fn sixteen_khz_passes_through_with_gain_and_downmix() {
        let mut r = RefResampler::new(16_000, 2.0).unwrap_or_else(|| panic!("rate"));
        assert_eq!(r.delay_samples(), 0);
        let mut out = Vec::new();
        r.push(&[0x40_0000, 0, -0x40_0000, -0x40_0000], 2, &mut out);
        assert_eq!(out, [0.5, -1.0]);
        assert!(RefResampler::new(44_100, 1.0).is_none());
        r.reset();
    }
}
