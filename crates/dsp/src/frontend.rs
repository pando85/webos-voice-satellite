//! Implementation of the front-end; see the crate docs.

use std::f64::consts::{PI, SQRT_2};

/// Full scale of a signed 24-bit sample, as a float.
const S24_FULL_SCALE: f64 = 8_388_608.0;
/// Full scale of a signed 16-bit sample, as a float.
const S16_FULL_SCALE: f64 = 32_768.0;
/// Level reported for digital silence.
const FLOOR_DBFS: f32 = -140.0;

/// Sign-extends a 24-bit sample held in the low bits of an `i32`.
///
/// Bits 24..31 are ignored, so both properly sign-extended values and
/// zero-padded 24-bit values give the same result.
#[must_use]
pub fn sign_extend_s24(raw: i32) -> i32 {
    (raw << 8) >> 8
}

/// Which channel(s) of the interleaved input feed the mono output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelMode {
    /// First channel (channel 0). ADR 0004 selects this for the MVP.
    #[default]
    Left,
    /// Second channel (channel 1). Falls back to channel 0 on mono input.
    Right,
    /// Mean of all input channels.
    Mix,
}

/// Front-end settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrontendConfig {
    /// Channel selection.
    pub channel: ChannelMode,
    /// Fixed digital gain in dB, applied after the high-pass filter.
    pub gain_db: f32,
    /// High-pass cutoff in Hz. A value of 0 (or below), or one at or above the
    /// Nyquist frequency, disables the filter.
    pub hpf_hz: f32,
}

impl Default for FrontendConfig {
    fn default() -> Self {
        Self {
            channel: ChannelMode::Left,
            gain_db: 24.0,
            hpf_hz: 120.0,
        }
    }
}

/// Level statistics over the samples processed since the last
/// [`Frontend::take_stats`]. Levels are in dBFS (full scale = 0 dB), floored at
/// -140 dB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Number of output samples (equals the number of input frames).
    pub samples: u64,
    /// Peak of the S16 output (after high-pass, gain and saturation).
    pub peak_dbfs: f32,
    /// RMS of the S16 output.
    pub rms_dbfs: f32,
    /// Output samples that hit the S16 rails (counted before saturation).
    pub clipped: u64,
    /// Peak of the selected input channel before any processing.
    pub in_peak_dbfs: f32,
    /// RMS of the selected input channel before any processing.
    pub in_rms_dbfs: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Accum {
    samples: u64,
    clipped: u64,
    out_peak: u32,
    out_sumsq: f64,
    in_peak: u32,
    in_sumsq: f64,
}

/// Direct form II transposed biquad, coefficients normalised by `a0`.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// 2nd-order Butterworth (Q = 1/sqrt 2) high-pass, RBJ cookbook form.
    fn butterworth_highpass(cutoff_hz: f64, rate_hz: f64) -> Self {
        let w0 = 2.0 * PI * cutoff_hz / rate_hz;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * (1.0 / SQRT_2));
        let a0 = 1.0 + alpha;
        Self {
            b0: f64::midpoint(1.0, cos) / a0,
            b1: -(1.0 + cos) / a0,
            b2: f64::midpoint(1.0, cos) / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The mic front-end. One instance per stream: it keeps filter state.
#[derive(Debug, Clone)]
pub struct Frontend {
    channel: ChannelMode,
    rate: u32,
    gain_db: f32,
    /// Linear gain applied to S24-normalised samples, pre-multiplied by the S16
    /// full scale so the output stage is one multiply.
    scale: f64,
    hpf: Option<Biquad>,
    acc: Accum,
}

fn linear_gain(gain_db: f32) -> f64 {
    10f64.powf(f64::from(gain_db) / 20.0)
}

fn dbfs(peak_or_rms: f64) -> f32 {
    if peak_or_rms <= 0.0 {
        return FLOOR_DBFS;
    }
    // Intentional narrowing: a level in dB needs no f64 precision.
    #[allow(clippy::cast_possible_truncation)]
    let db = (20.0 * peak_or_rms.log10()) as f32;
    db.max(FLOOR_DBFS)
}

impl Frontend {
    /// Creates a front-end for input sampled at `in_rate` Hz.
    ///
    /// A non-finite `gain_db` is replaced by 0 dB. The output rate equals
    /// `in_rate`.
    #[must_use]
    pub fn new(cfg: FrontendConfig, in_rate: u32) -> Self {
        let gain_db = if cfg.gain_db.is_finite() {
            cfg.gain_db
        } else {
            0.0
        };
        let nyquist = f64::from(in_rate) / 2.0;
        let cutoff = f64::from(cfg.hpf_hz);
        let hpf = (in_rate > 0 && cutoff.is_finite() && cutoff > 0.0 && cutoff < nyquist)
            .then(|| Biquad::butterworth_highpass(cutoff, f64::from(in_rate)));
        Self {
            channel: cfg.channel,
            rate: in_rate,
            gain_db,
            scale: linear_gain(gain_db) * S16_FULL_SCALE / S24_FULL_SCALE,
            hpf,
            acc: Accum::default(),
        }
    }

    /// Output sample rate in Hz (equal to the input rate).
    #[must_use]
    pub fn output_rate(&self) -> u32 {
        self.rate
    }

    /// Current gain in dB.
    #[must_use]
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Changes the gain without resetting filter state. Non-finite values are
    /// ignored.
    pub fn set_gain_db(&mut self, gain_db: f32) {
        if gain_db.is_finite() {
            self.gain_db = gain_db;
            self.scale = linear_gain(gain_db) * S16_FULL_SCALE / S24_FULL_SCALE;
        }
    }

    /// Clears the filter state (use after a gap in the stream). Statistics are
    /// kept.
    pub fn reset(&mut self) {
        if let Some(f) = &mut self.hpf {
            f.z1 = 0.0;
            f.z2 = 0.0;
        }
    }

    /// Converts interleaved S24-in-i32 frames to mono S16.
    ///
    /// `out` is cleared first, then one sample per input frame is appended. A
    /// trailing partial frame in `interleaved` is ignored. `channels == 0`
    /// produces no output.
    pub fn process(&mut self, interleaved: &[i32], channels: u16, out: &mut Vec<i16>) {
        out.clear();
        let ch = usize::from(channels);
        if ch == 0 {
            return;
        }
        let frames = interleaved.len() / ch;
        out.reserve(frames);
        let mut acc = self.acc;
        for frame in interleaved.chunks_exact(ch) {
            let x = match self.channel {
                ChannelMode::Left => f64::from(sign_extend_s24(frame[0])),
                ChannelMode::Right => f64::from(sign_extend_s24(frame[ch.min(2) - 1])),
                ChannelMode::Mix => {
                    let sum: i64 = frame.iter().map(|&v| i64::from(sign_extend_s24(v))).sum();
                    #[allow(clippy::cast_precision_loss)]
                    let mean = sum as f64 / ch as f64;
                    mean
                }
            };
            let in_abs = x.abs();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                acc.in_peak = acc.in_peak.max(in_abs as u32);
            }
            acc.in_sumsq += x * x;

            let filtered = self.hpf.as_mut().map_or(x, |f| f.run(x));
            let y = (filtered * self.scale).round();
            // Hard saturation to the S16 range.
            let clamped = y.clamp(-S16_FULL_SCALE, S16_FULL_SCALE - 1.0);
            if (y - clamped).abs() > 0.0 {
                acc.clipped += 1;
            }
            #[allow(clippy::cast_possible_truncation)]
            let s = clamped as i16;
            acc.out_peak = acc.out_peak.max(u32::from(s.unsigned_abs()));
            acc.out_sumsq += f64::from(s) * f64::from(s);
            out.push(s);
        }
        acc.samples += frames as u64;
        self.acc = acc;
    }

    fn stats_from(acc: &Accum) -> Stats {
        let (out_rms, in_rms) = if acc.samples == 0 {
            (0.0, 0.0)
        } else {
            #[allow(clippy::cast_precision_loss)]
            let n = acc.samples as f64;
            ((acc.out_sumsq / n).sqrt(), (acc.in_sumsq / n).sqrt())
        };
        Stats {
            samples: acc.samples,
            peak_dbfs: dbfs(f64::from(acc.out_peak) / S16_FULL_SCALE),
            rms_dbfs: dbfs(out_rms / S16_FULL_SCALE),
            clipped: acc.clipped,
            in_peak_dbfs: dbfs(f64::from(acc.in_peak) / S24_FULL_SCALE),
            in_rms_dbfs: dbfs(in_rms / S24_FULL_SCALE),
        }
    }

    /// Statistics accumulated since the last [`Frontend::take_stats`] (or
    /// since creation), without resetting them.
    #[must_use]
    pub fn stats(&self) -> Stats {
        Self::stats_from(&self.acc)
    }

    /// Returns the accumulated statistics and starts a new window.
    pub fn take_stats(&mut self) -> Stats {
        let s = Self::stats_from(&self.acc);
        self.acc = Accum::default();
        s
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

    use super::*;

    const RATE: u32 = 16_000;

    /// Interleaved stereo sine (same signal on both channels) at `amp` of S24
    /// full scale.
    fn sine(freq: f64, amp: f64, frames: usize) -> Vec<i32> {
        let mut v = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let t = n as f64 / f64::from(RATE);
            let s = (amp * S24_FULL_SCALE * (2.0 * PI * freq * t).sin()).round() as i32;
            v.push(s);
            v.push(s);
        }
        v
    }

    fn rms(samples: &[i16]) -> f64 {
        let sum: f64 = samples.iter().map(|&s| f64::from(s).powi(2)).sum();
        (sum / samples.len() as f64).sqrt()
    }

    fn cfg(gain_db: f32, hpf_hz: f32) -> FrontendConfig {
        FrontendConfig {
            channel: ChannelMode::Left,
            gain_db,
            hpf_hz,
        }
    }

    #[test]
    fn default_config_matches_contract() {
        let c = FrontendConfig::default();
        assert_eq!(c.channel, ChannelMode::Left);
        assert!((c.gain_db - 24.0).abs() < f32::EPSILON);
        assert!((c.hpf_hz - 120.0).abs() < f32::EPSILON);
    }

    #[test]
    fn sine_1khz_amplitude_after_gain() {
        // 0.01 FS (-40 dBFS) at 1 kHz, +20 dB gain -> 0.1 FS = 3276.8 peak.
        let mut fe = Frontend::new(cfg(20.0, 120.0), RATE);
        let input = sine(1000.0, 0.01, 8000);
        let mut out = Vec::new();
        fe.process(&input, 2, &mut out);
        assert_eq!(out.len(), 8000);
        // Skip the filter transient.
        let tail = &out[2000..];
        let peak = tail.iter().map(|s| i32::from(*s).abs()).max().unwrap_or(0);
        assert!((peak - 3277).abs() < 80, "peak {peak}");
        // RMS of a sine is peak / sqrt 2.
        let expect_rms = 3276.8 / SQRT_2;
        assert!((rms(tail) - expect_rms).abs() < 40.0, "rms {}", rms(tail));
    }

    #[test]
    fn zero_db_gain_is_s24_to_s16_shift() {
        let mut fe = Frontend::new(cfg(0.0, 0.0), RATE);
        let input = [0x0012_3400, 0, -0x0012_3400, 0, 0x7F_FFFF, 0, -0x80_0000, 0];
        let mut out = Vec::new();
        fe.process(&input, 2, &mut out);
        assert_eq!(out, [0x1234, -0x1234, 0x7FFF, i16::MIN]);
    }

    #[test]
    fn hpf_removes_dc() {
        let mut fe = Frontend::new(cfg(0.0, 120.0), RATE);
        let input = vec![0x20_0000; 2 * 16_000];
        let mut out = Vec::new();
        fe.process(&input, 2, &mut out);
        let tail = &out[8000..];
        let dc = f64::from(tail.iter().map(|&s| i32::from(s)).sum::<i32>()) / tail.len() as f64;
        assert!(dc.abs() < 1.0, "residual dc {dc}");
        // Without the filter the DC passes through.
        let mut plain = Frontend::new(cfg(0.0, 0.0), RATE);
        plain.process(&input, 2, &mut out);
        assert_eq!(out[100], 0x2000);
    }

    #[test]
    fn hpf_attenuates_50hz_and_passes_1khz() {
        let mut out = Vec::new();
        let amp = 0.2;
        let mut fe = Frontend::new(cfg(0.0, 120.0), RATE);
        fe.process(&sine(50.0, amp, 16_000), 2, &mut out);
        let r50 = rms(&out[8000..]);
        let mut fe = Frontend::new(cfg(0.0, 120.0), RATE);
        fe.process(&sine(1000.0, amp, 16_000), 2, &mut out);
        let r1k = rms(&out[8000..]);
        let atten_db = 20.0 * (r50 / r1k).log10();
        // 2nd-order Butterworth at 120 Hz: 50 Hz is -(40 * log10(120/50)) ~ -15.2 dB.
        assert!((-17.0..-13.5).contains(&atten_db), "50 Hz at {atten_db} dB");
        assert!(
            r1k > 0.99 * amp * S16_FULL_SCALE / SQRT_2 * 0.99,
            "1 kHz {r1k}"
        );
    }

    #[test]
    fn hpf_cutoff_is_minus_3db() {
        let mut out = Vec::new();
        let mut fe = Frontend::new(cfg(0.0, 120.0), RATE);
        fe.process(&sine(120.0, 0.2, 32_000), 2, &mut out);
        let r = rms(&out[16_000..]);
        let reference = 0.2 * S16_FULL_SCALE / SQRT_2;
        let db = 20.0 * (r / reference).log10();
        assert!((db + 3.0).abs() < 0.3, "cutoff gain {db} dB");
    }

    #[test]
    fn saturation_is_hard_and_counted() {
        // 0.5 FS at +12 dB (x3.98) = ~2 FS: must clip.
        let mut fe = Frontend::new(cfg(12.0, 0.0), RATE);
        let input = vec![0x40_0000, 0, -0x40_0000, 0];
        let mut out = Vec::new();
        fe.process(&input, 2, &mut out);
        assert_eq!(out, [i16::MAX, i16::MIN]);
        let s = fe.take_stats();
        assert_eq!(s.clipped, 2);
        assert!(s.peak_dbfs > -0.01);
        // A clean signal clips nothing.
        let mut fe = Frontend::new(cfg(0.0, 0.0), RATE);
        fe.process(&[0x7F_0000, 0], 2, &mut out);
        assert_eq!(fe.stats().clipped, 0);
    }

    #[test]
    fn channel_select_and_mix() {
        let input = [0x10_0000, 0x30_0000, -0x10_0000, -0x30_0000];
        let mut out = Vec::new();
        for (mode, expect) in [
            (ChannelMode::Left, [0x1000, -0x1000]),
            (ChannelMode::Right, [0x3000, -0x3000]),
            (ChannelMode::Mix, [0x2000, -0x2000]),
        ] {
            let mut fe = Frontend::new(
                FrontendConfig {
                    channel: mode,
                    gain_db: 0.0,
                    hpf_hz: 0.0,
                },
                RATE,
            );
            fe.process(&input, 2, &mut out);
            assert_eq!(out, expect, "{mode:?}");
        }
    }

    #[test]
    fn right_on_mono_falls_back_to_channel_zero() {
        let mut fe = Frontend::new(
            FrontendConfig {
                channel: ChannelMode::Right,
                gain_db: 0.0,
                hpf_hz: 0.0,
            },
            RATE,
        );
        let mut out = Vec::new();
        fe.process(&[0x10_0000, 0x20_0000], 1, &mut out);
        assert_eq!(out, [0x1000, 0x2000]);
    }

    #[test]
    fn three_channels_mix_uses_all() {
        let mut fe = Frontend::new(
            FrontendConfig {
                channel: ChannelMode::Mix,
                gain_db: 0.0,
                hpf_hz: 0.0,
            },
            RATE,
        );
        let mut out = Vec::new();
        fe.process(&[0x10_0000, 0x20_0000, 0x30_0000], 3, &mut out);
        assert_eq!(out, [0x2000]);
    }

    #[test]
    fn s24_sign_extension_edges() {
        assert_eq!(sign_extend_s24(0), 0);
        assert_eq!(sign_extend_s24(0x7F_FFFF), 0x7F_FFFF);
        assert_eq!(sign_extend_s24(0x80_0000), -0x80_0000);
        assert_eq!(sign_extend_s24(0xFF_FFFF), -1);
        // Already sign-extended values are unchanged.
        assert_eq!(sign_extend_s24(-1), -1);
        assert_eq!(sign_extend_s24(-0x80_0000), -0x80_0000);
        // Garbage in the top byte is ignored.
        assert_eq!(sign_extend_s24(0x5A00_0001), 1);
        assert_eq!(sign_extend_s24(i32::MIN), 0);
    }

    #[test]
    fn unextended_s24_input_is_handled() {
        // 0xFF8000 is -32768 in 24 bit, zero-padded in the container.
        let mut fe = Frontend::new(cfg(0.0, 0.0), RATE);
        let mut out = Vec::new();
        fe.process(&[0x00FF_8000, 0], 2, &mut out);
        assert_eq!(out, [-128]);
    }

    #[test]
    fn partial_frame_and_zero_channels() {
        let mut fe = Frontend::new(cfg(0.0, 0.0), RATE);
        let mut out = vec![7; 3];
        fe.process(&[0x10_0000, 0, 0x10_0000], 2, &mut out);
        assert_eq!(out.len(), 1);
        fe.process(&[1, 2, 3], 0, &mut out);
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn stats_report_levels_and_reset() {
        let mut fe = Frontend::new(cfg(20.0, 0.0), RATE);
        let mut out = Vec::new();
        // Constant -40 dBFS input (0.01 FS): output constant -20 dBFS.
        let v = (0.01 * S24_FULL_SCALE) as i32;
        fe.process(&vec![v; 2 * 1000], 2, &mut out);
        let s = fe.take_stats();
        assert_eq!(s.samples, 1000);
        assert!((s.in_peak_dbfs + 40.0).abs() < 0.05, "{s:?}");
        assert!((s.in_rms_dbfs + 40.0).abs() < 0.05, "{s:?}");
        assert!((s.peak_dbfs + 20.0).abs() < 0.05, "{s:?}");
        assert!((s.rms_dbfs + 20.0).abs() < 0.05, "{s:?}");
        let empty = fe.take_stats();
        assert_eq!(empty.samples, 0);
        assert!((empty.rms_dbfs - FLOOR_DBFS).abs() < f32::EPSILON);
    }

    #[test]
    fn filter_state_is_continuous_across_calls() {
        let input = sine(300.0, 0.1, 4000);
        let mut whole = Frontend::new(cfg(6.0, 120.0), RATE);
        let mut split = Frontend::new(cfg(6.0, 120.0), RATE);
        let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
        whole.process(&input, 2, &mut a);
        split.process(&input[..2 * 1234], 2, &mut b);
        split.process(&input[2 * 1234..], 2, &mut c);
        b.extend_from_slice(&c);
        assert_eq!(a, b);
    }

    #[test]
    fn invalid_settings_are_sanitised() {
        // NaN gain -> 0 dB; cutoff above Nyquist -> filter disabled.
        let mut fe = Frontend::new(cfg(f32::NAN, 9000.0), RATE);
        let mut out = Vec::new();
        fe.process(&[0x10_0000, 0], 2, &mut out);
        assert_eq!(out, [0x1000]);
        assert_eq!(fe.output_rate(), RATE);
        fe.set_gain_db(f32::INFINITY);
        assert!(fe.gain_db().abs() < f32::EPSILON);
        fe.set_gain_db(6.0);
        assert!((fe.gain_db() - 6.0).abs() < f32::EPSILON);
    }
}
