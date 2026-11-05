//! Integer gain for the music path: Q15 gains and per-sample linear ramps.
//!
//! The hot path ([`GainStage::process`]) uses only integer arithmetic, so it
//! is cheap on the soft-float armv7 TV build. Floating point appears only in
//! the control-side conversions ([`q15_from_f32`], [`q15_from_db`]), which run
//! when a setting changes, not per sample.
//!
//! A gain is a [`u32`] in Q15: [`ONE`] (32768) is unity. Gains above unity are
//! allowed up to [`MAX_GAIN`] (four times) for a positive `gain_db` trim; the
//! result saturates at the 16-bit range.
//!
//! Three independent ramps are multiplied per frame (see [`GainStage`]):
//! `vol` (volume and trim), `duck` and `fade` (pause and resume). Each ramp
//! moves its gain **every frame**, never in per-chunk steps, so a change has
//! no zipper noise however the audio is split into chunks.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Unity gain in Q15.
pub const ONE: u32 = 1 << 15;
/// Largest gain accepted (4.0, +12 dB).
pub const MAX_GAIN: u32 = 4 * ONE;

/// Q15 gain for a linear factor (clamped to 0 to [`MAX_GAIN`]; NaN is 0).
#[must_use]
#[allow(clippy::cast_precision_loss)] // Q15 values are at most 2^17: exact in f32
pub fn q15_from_f32(gain: f32) -> u32 {
    if gain.is_nan() || gain <= 0.0 {
        return 0;
    }
    let v = (gain * ONE as f32).round();
    if v >= MAX_GAIN as f32 {
        MAX_GAIN
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            v as u32
        }
    }
}

/// Q15 gain for a level in dB (0 dB is [`ONE`]; below -120 dB is 0).
#[must_use]
pub fn q15_from_db(db: f32) -> u32 {
    if db.is_nan() || db <= -120.0 {
        return 0;
    }
    q15_from_f32(10_f32.powf(db / 20.0))
}

/// Linear factor of a Q15 gain (for logs and tests).
#[must_use]
#[allow(clippy::cast_precision_loss)] // Q15 values are at most 2^17: exact in f32
pub fn q15_to_f32(q: u32) -> f32 {
    q as f32 / ONE as f32
}

/// Q15 product `a * b`, rounded.
#[must_use]
pub fn q15_mul(a: u32, b: u32) -> u32 {
    let p = (u64::from(a) * u64::from(b) + u64::from(ONE >> 1)) >> 15;
    u32::try_from(p).unwrap_or(u32::MAX)
}

/// One sample times a Q15 gain, rounded and saturated to 16 bits.
#[must_use]
#[inline]
pub fn scale(sample: i16, gain_q15: u32) -> i16 {
    let v = (i64::from(sample) * i64::from(gain_q15) + i64::from(ONE >> 1)) >> 15;
    #[allow(clippy::cast_possible_truncation)]
    {
        v.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
    }
}

/// Fixed-point position: Q15 shifted left by this many bits (Q31 overall),
/// so a ramp of tens of thousands of frames still has a nonzero step.
const FRAC: u32 = 16;

/// A gain that moves linearly to a target, one step per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GainRamp {
    /// Current gain, Q15 with [`FRAC`] extra fractional bits.
    cur: i64,
    /// Target gain, same scale.
    target: i64,
    /// Added to `cur` per frame.
    step: i64,
    /// Frames until the target is reached (0 means idle at the target).
    remaining: u32,
}

impl GainRamp {
    /// A ramp idle at `q15`.
    #[must_use]
    pub fn new(q15: u32) -> Self {
        let v = i64::from(q15) << FRAC;
        Self {
            cur: v,
            target: v,
            step: 0,
            remaining: 0,
        }
    }

    /// Jumps to `q15` without a ramp (a new track, never mid-stream).
    pub fn set(&mut self, q15: u32) {
        *self = Self::new(q15);
    }

    /// Ramps from the current gain to `q15` over `frames` frames (0 jumps).
    pub fn to(&mut self, q15: u32, frames: u32) {
        let target = i64::from(q15) << FRAC;
        if frames == 0 || target == self.cur {
            self.set(q15);
            return;
        }
        self.target = target;
        self.step = (target - self.cur) / i64::from(frames);
        self.remaining = frames;
        if self.step == 0 {
            // Closer than one step per frame: take one frame.
            self.step = target - self.cur;
            self.remaining = 1;
        }
    }

    /// Ramps to `q15` at the slope of a full swing of `full_dist_q15` over
    /// `full_frames`, so a partial swing takes proportionally less time
    /// (a duck interrupted halfway does not start over at the slow rate).
    pub fn to_slope(&mut self, q15: u32, full_dist_q15: u32, full_frames: u32) {
        let dist = (i64::from(q15) << FRAC).abs_diff(self.cur) >> FRAC;
        if full_dist_q15 == 0 || full_frames == 0 || dist == 0 {
            self.set(q15);
            return;
        }
        let frames = (dist * u64::from(full_frames)).div_ceil(u64::from(full_dist_q15));
        self.to(q15, u32::try_from(frames.max(1)).unwrap_or(u32::MAX));
    }

    /// The gain the ramp is heading to (or sits at).
    #[must_use]
    pub fn target(&self) -> u32 {
        u32::try_from(self.target >> FRAC).unwrap_or(u32::MAX)
    }

    /// The gain right now, rounded to Q15.
    #[must_use]
    pub fn current(&self) -> u32 {
        u32::try_from((self.cur + (1 << (FRAC - 1))) >> FRAC).unwrap_or(u32::MAX)
    }

    /// True when the ramp has reached its target.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.remaining == 0
    }

    /// Gain for this frame, then advances one frame.
    #[inline]
    fn next(&mut self) -> u32 {
        let g = self.current();
        if self.remaining > 0 {
            self.remaining -= 1;
            self.cur = if self.remaining == 0 {
                self.target
            } else {
                self.cur + self.step
            };
        }
        g
    }
}

/// The three gains applied to music, multiplied for every frame.
#[derive(Debug, Clone, Copy)]
pub struct GainStage {
    /// Volume times trim (`sink.gain`, reply volume, `gain_db`).
    pub vol: GainRamp,
    /// Ducking (attack and release).
    pub duck: GainRamp,
    /// Pause fade-out and resume fade-in.
    pub fade: GainRamp,
}

impl GainStage {
    /// A stage at `vol`, no duck, no fade.
    #[must_use]
    pub fn new(vol_q15: u32) -> Self {
        Self {
            vol: GainRamp::new(vol_q15),
            duck: GainRamp::new(ONE),
            fade: GainRamp::new(ONE),
        }
    }

    fn all_idle(&self) -> bool {
        self.vol.is_idle() && self.duck.is_idle() && self.fade.is_idle()
    }

    /// The combined gain right now.
    #[must_use]
    pub fn combined(&self) -> u32 {
        q15_mul(
            q15_mul(self.vol.current(), self.duck.current()),
            self.fade.current(),
        )
    }

    /// Scales interleaved samples in place, advancing the ramps one step per
    /// frame. `channels` samples make a frame and share one gain; a trailing
    /// partial frame is scaled with the last gain and does not advance.
    pub fn process(&mut self, samples: &mut [i16], channels: usize) {
        let channels = channels.max(1);
        if self.all_idle() {
            let g = self.combined();
            if g != ONE {
                for s in samples {
                    *s = scale(*s, g);
                }
            }
            return;
        }
        let mut chunks = samples.chunks_exact_mut(channels);
        for frame in &mut chunks {
            let g = q15_mul(q15_mul(self.vol.next(), self.duck.next()), self.fade.next());
            for s in frame {
                *s = scale(*s, g);
            }
        }
        let g = self.combined();
        for s in chunks.into_remainder() {
            *s = scale(*s, g);
        }
    }
}

/// Shared volume for the music path: `sink.gain` times the reply volume, set
/// by the daemon whenever Home Assistant changes the volume (design D2: one
/// shared volume). Lock-free; the engine reads it before every slice and
/// ramps to a change over [`crate::music::MusicConfig::volume_ramp`].
#[derive(Debug, Clone)]
pub struct MusicVolume(Arc<AtomicU32>);

impl MusicVolume {
    /// A volume of the given linear factor (1.0 is unity).
    #[must_use]
    pub fn new(gain: f32) -> Self {
        Self(Arc::new(AtomicU32::new(q15_from_f32(gain))))
    }

    /// Sets the linear factor (clamped to 0 to 4.0).
    pub fn set(&self, gain: f32) {
        self.0.store(q15_from_f32(gain), Ordering::Relaxed);
    }

    /// The current gain in Q15.
    #[must_use]
    pub fn get_q15(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
    use super::*;

    fn dc(n_frames: usize, ch: usize, v: i16) -> Vec<i16> {
        vec![v; n_frames * ch]
    }

    #[test]
    fn q15_conversions() {
        assert_eq!(q15_from_f32(1.0), ONE);
        assert_eq!(q15_from_f32(0.0), 0);
        assert_eq!(q15_from_f32(-3.0), 0);
        assert_eq!(q15_from_f32(f32::NAN), 0);
        assert_eq!(q15_from_f32(100.0), MAX_GAIN);
        assert_eq!(q15_from_f32(0.5), ONE / 2);
        assert_eq!(q15_from_db(0.0), ONE);
        // -20 dB is 0.1 (3276.8 in Q15).
        assert_eq!(q15_from_db(-20.0), 3277);
        assert_eq!(q15_from_db(-200.0), 0);
        // +12 dB is 3.98, inside the headroom.
        assert!((130_000..MAX_GAIN).contains(&q15_from_db(12.0)));
        assert!((q15_to_f32(q15_from_db(-6.0)) - 0.5012).abs() < 0.001);
    }

    #[test]
    fn unity_is_a_no_op_and_zero_is_silence() {
        let mut g = GainStage::new(ONE);
        let mut v = vec![i16::MIN, -1, 0, 1, i16::MAX, 12345];
        let orig = v.clone();
        g.process(&mut v, 2);
        assert_eq!(v, orig, "unity must be bit exact");
        let mut g = GainStage::new(0);
        g.process(&mut v, 2);
        assert!(v.iter().all(|&s| s == 0));
    }

    #[test]
    fn constant_gain_matches_the_q15_formula() {
        let mut g = GainStage::new(q15_from_db(-20.0));
        let mut v = dc(8, 2, 10_000);
        g.process(&mut v, 2);
        // 10000 * 3277 / 32768 = 1000.0 (rounded).
        assert!(v.iter().all(|&s| s == 1000), "{v:?}");
    }

    #[test]
    fn gain_above_unity_saturates() {
        let mut g = GainStage::new(q15_from_f32(4.0));
        let mut v = vec![20_000, -20_000, 1000];
        g.process(&mut v, 1);
        assert_eq!(v, vec![i16::MAX, i16::MIN, 4000]);
    }

    #[test]
    fn ramp_reaches_its_target_exactly_and_is_monotonic() {
        let mut r = GainRamp::new(ONE);
        r.to(3277, 2400);
        let mut last = ONE;
        for i in 0..2400 {
            let g = r.next();
            assert!(g <= last, "non increasing at {i}");
            last = g;
        }
        assert!(r.is_idle());
        assert_eq!(r.current(), 3277);
        assert_eq!(r.next(), 3277);
    }

    #[test]
    fn ramp_has_no_steps_at_chunk_boundaries() {
        // A -20 dB duck over 50 ms at 48 kHz stereo on a DC signal, cut into
        // chunks of awkward sizes: identical to one call, and every
        // frame-to-frame step is bounded by the slope.
        let frames = 4800;
        let attack = 2400;
        let whole = {
            let mut g = GainStage::new(ONE);
            g.duck.to(q15_from_db(-20.0), attack);
            let mut v = dc(frames, 2, 20_000);
            g.process(&mut v, 2);
            v
        };
        for sizes in [[1usize, 7, 480, 3, 960], [333, 1, 1, 2000, 5]] {
            let mut g = GainStage::new(ONE);
            g.duck.to(q15_from_db(-20.0), attack);
            let mut v = dc(frames, 2, 20_000);
            let mut pos = 0;
            let mut i = 0;
            while pos < frames {
                let n = sizes[i % sizes.len()].min(frames - pos);
                g.process(&mut v[pos * 2..(pos + n) * 2], 2);
                pos += n;
                i += 1;
            }
            assert_eq!(v, whole, "chunking must not change the output");
        }
        // Bound: the gain falls (ONE - 3277) / 2400 = 12.3 Q15 per frame,
        // the signal 20000 * 12.3 / 32768 = 7.5 per frame, plus rounding.
        let left: Vec<i16> = whole.chunks(2).map(|f| f[0]).collect();
        let max_step = left
            .windows(2)
            .map(|w| (i32::from(w[1]) - i32::from(w[0])).abs())
            .max()
            .unwrap_or(0);
        assert!(max_step <= 9, "max per-sample step {max_step}");
        assert_eq!(left[0], 20_000);
        assert_eq!(left[attack as usize + 1], 2000, "settled at -20 dB");
        assert_eq!(left[frames - 1], 2000);
        // Both channels of a frame carry the same gain.
        assert!(whole.chunks(2).all(|f| f[0] == f[1]));
    }

    #[test]
    fn slope_ramps_take_proportional_time() {
        let duck = q15_from_db(-20.0);
        let mut r = GainRamp::new(ONE);
        r.to_slope(duck, ONE - duck, 2400);
        let mut full = 0;
        while !r.is_idle() {
            r.next();
            full += 1;
        }
        assert!((2399..=2401).contains(&full), "{full}");
        // Half way down, going back up: about half the frames.
        let mut r = GainRamp::new(ONE);
        r.to_slope(duck, ONE - duck, 2400);
        for _ in 0..1200 {
            r.next();
        }
        r.to_slope(ONE, ONE - duck, 2400);
        let mut n = 0;
        while !r.is_idle() {
            r.next();
            n += 1;
        }
        assert!((1199..=1202).contains(&n), "{n}");
        assert_eq!(r.current(), ONE);
    }

    #[test]
    fn retargeting_mid_ramp_is_continuous() {
        let mut r = GainRamp::new(ONE);
        r.to(0, 1000);
        for _ in 0..400 {
            r.next();
        }
        let before = r.current();
        r.to(ONE, 1000);
        let after = r.next();
        assert!(before.abs_diff(after) <= 40, "{before} -> {after}");
    }

    #[test]
    fn three_ramps_multiply() {
        let mut g = GainStage::new(q15_from_f32(0.5));
        g.duck.set(q15_from_f32(0.5));
        g.fade.set(q15_from_f32(0.5));
        let mut v = dc(4, 1, 8000);
        g.process(&mut v, 1);
        assert!(v.iter().all(|&s| s == 1000));
    }

    #[test]
    fn volume_handle_round_trips() {
        let v = MusicVolume::new(0.25);
        assert_eq!(v.get_q15(), ONE / 4);
        v.set(2.0);
        assert_eq!(v.get_q15(), 2 * ONE);
        v.set(f32::NAN);
        assert_eq!(v.get_q15(), 0);
    }
}
