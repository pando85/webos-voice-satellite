//! A small in-place radix-2 complex FFT (f32), enough for the delay
//! estimator. Twiddles are precomputed once per size.

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

/// Complex number as `(re, im)`.
pub(crate) type Complex = (f32, f32);

/// Precomputed plan for one power-of-two size.
#[derive(Debug, Clone)]
pub(crate) struct Fft {
    n: usize,
    /// `exp(-2 pi i k / n)` for `k` in `0..n/2`.
    twiddles: Vec<Complex>,
    /// Bit-reversal permutation.
    rev: Vec<u32>,
}

impl Fft {
    /// Plans an FFT of size `n` (a power of two, at least 2).
    ///
    /// # Panics
    /// If `n` is not a power of two or is below 2 (a programming error).
    pub(crate) fn new(n: usize) -> Self {
        assert!(
            n.is_power_of_two() && n >= 2,
            "FFT size must be a power of two"
        );
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| (i as u32).reverse_bits() >> (32 - bits))
            .collect();
        Self { n, twiddles, rev }
    }

    /// Transform size.
    pub(crate) fn len(&self) -> usize {
        self.n
    }

    /// Forward transform in place (no scaling).
    pub(crate) fn forward(&self, x: &mut [Complex]) {
        self.run(x, false);
    }

    /// Inverse transform in place (no 1/n scaling).
    pub(crate) fn inverse(&self, x: &mut [Complex]) {
        self.run(x, true);
    }

    fn run(&self, x: &mut [Complex], inverse: bool) {
        assert_eq!(x.len(), self.n, "FFT buffer length");
        for i in 0..self.n {
            let j = self.rev[i] as usize;
            if i < j {
                x.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= self.n {
            let half = len / 2;
            let step = self.n / len;
            for start in (0..self.n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * step];
                    let wi = if inverse { -wi } else { wi };
                    let (ar, ai) = x[start + k];
                    let (br, bi) = x[start + k + half];
                    let tr = br * wr - bi * wi;
                    let ti = br * wi + bi * wr;
                    x[start + k] = (ar + tr, ai + ti);
                    x[start + k + half] = (ar - tr, ai - ti);
                }
            }
            len <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::cast_precision_loss)]

    use super::*;

    fn dft(input: &[Complex]) -> Vec<Complex> {
        let size = input.len();
        (0..size)
            .map(|bin| {
                let mut acc = (0.0_f64, 0.0_f64);
                for (t, &(re, im)) in input.iter().enumerate() {
                    let angle = -2.0 * PI * (bin * t) as f64 / size as f64;
                    let (cos, sin) = (angle.cos(), angle.sin());
                    acc.0 += f64::from(re) * cos - f64::from(im) * sin;
                    acc.1 += f64::from(re) * sin + f64::from(im) * cos;
                }
                (acc.0 as f32, acc.1 as f32)
            })
            .collect()
    }

    #[test]
    fn matches_a_direct_dft_and_inverts() {
        let size = 64;
        let input: Vec<Complex> = (0..size)
            .map(|i| (((i * 7) % 13) as f32 - 6.0, ((i * 3) % 5) as f32 - 2.0))
            .collect();
        let mut spec = input.clone();
        let fft = Fft::new(size);
        assert_eq!(fft.len(), size);
        fft.forward(&mut spec);
        for (got, want) in spec.iter().zip(dft(&input)) {
            assert!(
                (got.0 - want.0).abs() < 1e-3 && (got.1 - want.1).abs() < 1e-3,
                "{got:?} {want:?}"
            );
        }
        fft.inverse(&mut spec);
        let scale = size as f32;
        for (got, want) in spec.iter().zip(&input) {
            assert!((got.0 / scale - want.0).abs() < 1e-4 && (got.1 / scale - want.1).abs() < 1e-4);
        }
    }
}
