//! Level statistics over the check window and the verdict function for C4 to
//! C7 (design section 3.2).
//!
//! Only numbers leave the capture thread: [`FrameStats`] is a per-frame digest
//! (count, sum of squares, peak, min, max, clipped). No sample is stored.
//! [`grade_window`] is the single place where thresholds meet statistics.

use std::time::Duration;

use crate::check::{CheckId, CheckResult, Reason};

/// Floor for dBFS values (digital silence), so nothing is infinite or NaN.
pub const DBFS_FLOOR: f64 = -140.0;

/// Thresholds of the checks. **All defaults are `hypothesis`** until run on
/// c3-a and c3-b (design section 10); the references are the c3-a numbers
/// quoted next to each field.
#[derive(Clone, Debug, PartialEq)]
pub struct Thresholds {
    /// Length of the measurement window (design: about 10 s).
    pub window: Duration,
    /// Audio discarded after the open because of the startup burst
    /// (verified-on-target c3-a: 250 ms).
    pub discard: Duration,
    /// No frame within this time after the open fails C4 (design: 3 s; the
    /// existing stall watchdog is 5 s).
    pub no_frames_fail: Duration,
    /// C4 passes at or above this fraction of the expected frames (0.90).
    pub frames_pass_ratio: f64,
    /// C5 fails when the input peak-to-peak (in LSB of the capture format)
    /// over the window is at most this (1 LSB).
    pub stuck_max_peak_to_peak: i64,
    /// C6 warns when the input peak is at or below this (-80 dBFS). c3-a in a
    /// quiet room: -53.6 dBFS peak; a wrong 32-bit decode: -92.6 dBFS.
    pub silent_peak_dbfs: f64,
    /// C7 warns at or above this clipped fraction (0.5 %).
    pub clipped_fraction_warn: f64,
    /// C7 warns at or above this RMS level (-10 dBFS). c3-a quiet room:
    /// -67.6 dBFS RMS.
    pub loud_rms_dbfs: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(10),
            discard: Duration::from_millis(250),
            no_frames_fail: Duration::from_secs(3),
            frames_pass_ratio: 0.90,
            stuck_max_peak_to_peak: 1,
            silent_peak_dbfs: -80.0,
            clipped_fraction_warn: 0.005,
            loud_rms_dbfs: -10.0,
        }
    }
}

/// Digest of one capture frame batch (any size). Built by the capture thread
/// from the samples it already has; the samples are not kept.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Frames (samples per channel).
    pub frames: u32,
    /// Interleaved samples (frames times channels).
    pub samples: u32,
    /// Sum of squared sample values, in LSB squared.
    pub sum_sq: f64,
    /// Largest absolute sample value, in LSB.
    pub peak: u32,
    /// Smallest sample value, in LSB.
    pub min: i32,
    /// Largest sample value, in LSB.
    pub max: i32,
    /// Clipped samples (counted at full scale by [`FrameStats::from_samples`];
    /// the caller may replace it with the front-end's own count).
    pub clipped: u32,
}

impl FrameStats {
    /// Digest of interleaved, sign-extended samples. `bits` is the capture
    /// format's width (24 for S24); a sample at full scale counts as clipped.
    #[must_use]
    pub fn from_samples(samples: &[i32], channels: u32, bits: u32) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let full = full_scale(bits);
        let clip_at = i64::from(full) - 1;
        let (mut min, mut max) = (i32::MAX, i32::MIN);
        let (mut peak, mut clipped) = (0u32, 0u32);
        let mut sum_sq = 0.0_f64;
        for &s in samples {
            min = min.min(s);
            max = max.max(s);
            let a = s.unsigned_abs();
            peak = peak.max(a);
            if i64::from(a) >= clip_at {
                clipped += 1;
            }
            sum_sq += f64::from(s) * f64::from(s);
        }
        let n = u32::try_from(samples.len()).unwrap_or(u32::MAX);
        Self {
            frames: n / channels.max(1),
            samples: n,
            sum_sq,
            peak,
            min,
            max,
            clipped,
        }
    }
}

/// Full-scale magnitude of a signed `bits`-bit sample (2^(bits-1)).
#[must_use]
pub fn full_scale(bits: u32) -> u32 {
    1u32 << (bits.clamp(2, 32) - 1)
}

/// Accumulator over the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowAccumulator {
    frames: u64,
    samples: u64,
    sum_sq: f64,
    peak: u32,
    min: i32,
    max: i32,
    clipped: u64,
}

impl WindowAccumulator {
    /// Adds a batch.
    pub fn add(&mut self, s: &FrameStats) {
        if s.samples == 0 {
            return;
        }
        if self.samples == 0 {
            self.min = s.min;
            self.max = s.max;
        } else {
            self.min = self.min.min(s.min);
            self.max = self.max.max(s.max);
        }
        self.frames += u64::from(s.frames);
        self.samples += u64::from(s.samples);
        self.sum_sq += s.sum_sq;
        self.peak = self.peak.max(s.peak);
        self.clipped += u64::from(s.clipped);
    }

    /// Frames added so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Summary over a window of `elapsed` audio time at `sample_rate`.
    #[must_use]
    pub fn summarize(&self, elapsed: Duration, sample_rate: u32, bits: u32) -> WindowSummary {
        let expected = f64::from(sample_rate) * elapsed.as_secs_f64();
        let fs = f64::from(full_scale(bits));
        let (rms_dbfs, peak_dbfs, clipped_fraction) = if self.samples == 0 {
            (DBFS_FLOOR, DBFS_FLOOR, 0.0)
        } else {
            let ms = self.sum_sq / count_f64(self.samples);
            (
                to_dbfs(ms.sqrt() / fs),
                to_dbfs(f64::from(self.peak) / fs),
                count_f64(self.clipped) / count_f64(self.samples),
            )
        };
        WindowSummary {
            frames: self.frames,
            frames_ratio: if expected > 0.0 {
                count_f64(self.frames) / expected
            } else {
                0.0
            },
            peak_to_peak: if self.samples == 0 {
                0
            } else {
                i64::from(self.max) - i64::from(self.min)
            },
            rms_dbfs,
            peak_dbfs,
            clipped: self.clipped,
            clipped_fraction,
        }
    }
}

#[allow(clippy::cast_precision_loss)] // counts far below 2^52
fn count_f64(n: u64) -> f64 {
    n as f64
}

/// Linear amplitude (1.0 = full scale) to dBFS, clamped to [`DBFS_FLOOR`, 0].
#[must_use]
pub fn to_dbfs(linear: f64) -> f64 {
    if linear.is_nan() || linear <= 0.0 {
        return DBFS_FLOOR;
    }
    (20.0 * linear.log10()).clamp(DBFS_FLOOR, 0.0)
}

/// What the checks see: window statistics reduced to the numbers of the
/// table in design 3.2. Build it with [`WindowAccumulator::summarize`] or
/// directly (the tests do).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSummary {
    /// Frames received in the window.
    pub frames: u64,
    /// Frames received divided by frames expected for the window length.
    pub frames_ratio: f64,
    /// Input peak-to-peak over the window, in LSB.
    pub peak_to_peak: i64,
    /// Input RMS in dBFS.
    pub rms_dbfs: f64,
    /// Input peak in dBFS.
    pub peak_dbfs: f64,
    /// Clipped samples.
    pub clipped: u64,
    /// Clipped samples divided by all samples.
    pub clipped_fraction: f64,
}

/// Verdicts of C4 to C7 for one window. **The single place to change the
/// rules.**
///
/// * C4: pass at >= `frames_pass_ratio`; any lower ratio with some frames is a
///   warn (the design table gives 50 to 90 % as WARN and defines FAIL only as
///   "none at all"); fail without any frame.
/// * C5: fail when peak-to-peak <= `stuck_max_peak_to_peak`.
/// * C6: warn when the peak is at or below `silent_peak_dbfs`.
/// * C7: warn when the clipped fraction or the RMS reaches its limit.
///
/// Without frames C5 to C7 are skipped as not measured.
#[must_use]
pub fn grade_window(th: &Thresholds, w: &WindowSummary) -> [CheckResult; 4] {
    if w.frames == 0 {
        return [
            CheckResult::fail(CheckId::C4, Reason::NoFrames),
            CheckResult::not_measured(CheckId::C5),
            CheckResult::not_measured(CheckId::C6),
            CheckResult::not_measured(CheckId::C7),
        ];
    }
    let c4 = if w.frames_ratio >= th.frames_pass_ratio {
        CheckResult::pass(CheckId::C4)
    } else {
        // The microphone stays open (design 3.3 "a lower C4").
        CheckResult::warn(CheckId::C4, Reason::FramesLow)
    };
    let c5 = if w.peak_to_peak <= th.stuck_max_peak_to_peak {
        CheckResult::fail(CheckId::C5, Reason::Stuck)
    } else {
        CheckResult::pass(CheckId::C5)
    };
    let c6 = if w.peak_dbfs <= th.silent_peak_dbfs {
        CheckResult::warn(CheckId::C6, Reason::Quiet)
    } else {
        CheckResult::pass(CheckId::C6)
    };
    let c7 = if w.clipped_fraction >= th.clipped_fraction_warn {
        CheckResult::warn(CheckId::C7, Reason::Clipping)
    } else if w.rms_dbfs >= th.loud_rms_dbfs {
        CheckResult::warn(CheckId::C7, Reason::Loud)
    } else {
        CheckResult::pass(CheckId::C7)
    };
    [c4, c5, c6, c7]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Grade;

    /// c3-a quiet room: -67.6 dBFS RMS, -53.6 dBFS peak (verified-on-target).
    fn c3a() -> WindowSummary {
        WindowSummary {
            frames: 160_000,
            frames_ratio: 1.0,
            peak_to_peak: 2_000,
            rms_dbfs: -67.6,
            peak_dbfs: -53.6,
            clipped: 0,
            clipped_fraction: 0.0,
        }
    }

    fn grades(w: &WindowSummary) -> Vec<(Grade, Option<Reason>)> {
        grade_window(&Thresholds::default(), w)
            .iter()
            .map(|c| (c.grade, c.reason))
            .collect()
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one table, one place
    fn table_driven_verdicts() {
        use Grade::{Fail, Pass, Skipped, Warn};
        type Case = (&'static str, WindowSummary, Vec<(Grade, Option<Reason>)>);
        let p = (Pass, None);
        let cases: Vec<Case> = vec![
            ("c3-a quiet room passes", c3a(), vec![p; 4]),
            (
                "wrong 32-bit decode: peak -92.6 dBFS is a quiet warning",
                WindowSummary {
                    peak_dbfs: -92.6,
                    rms_dbfs: -110.0,
                    ..c3a()
                },
                vec![p, p, (Warn, Some(Reason::Quiet)), p],
            ),
            (
                "peak exactly at -80 dBFS warns (at or below)",
                WindowSummary {
                    peak_dbfs: -80.0,
                    ..c3a()
                },
                vec![p, p, (Warn, Some(Reason::Quiet)), p],
            ),
            (
                "peak just above -80 dBFS passes",
                WindowSummary {
                    peak_dbfs: -79.9,
                    ..c3a()
                },
                vec![p; 4],
            ),
            (
                "all samples equal is stuck (and silent)",
                WindowSummary {
                    peak_to_peak: 0,
                    peak_dbfs: DBFS_FLOOR,
                    rms_dbfs: DBFS_FLOOR,
                    ..c3a()
                },
                vec![
                    p,
                    (Fail, Some(Reason::Stuck)),
                    (Warn, Some(Reason::Quiet)),
                    p,
                ],
            ),
            (
                "1 LSB peak-to-peak is still stuck, 2 LSB is not",
                WindowSummary {
                    peak_to_peak: 1,
                    ..c3a()
                },
                vec![p, (Fail, Some(Reason::Stuck)), p, p],
            ),
            (
                "2 LSB peak-to-peak passes C5",
                WindowSummary {
                    peak_to_peak: 2,
                    ..c3a()
                },
                vec![p; 4],
            ),
            (
                "clipped 0.5 % warns",
                WindowSummary {
                    clipped: 800,
                    clipped_fraction: 0.005,
                    ..c3a()
                },
                vec![p, p, p, (Warn, Some(Reason::Clipping))],
            ),
            (
                "clipped just below 0.5 % passes",
                WindowSummary {
                    clipped_fraction: 0.0049,
                    ..c3a()
                },
                vec![p; 4],
            ),
            (
                "very loud RMS warns",
                WindowSummary {
                    rms_dbfs: -9.0,
                    peak_dbfs: -1.0,
                    ..c3a()
                },
                vec![p, p, p, (Warn, Some(Reason::Loud))],
            ),
            (
                "90 % of the frames passes",
                WindowSummary {
                    frames_ratio: 0.90,
                    ..c3a()
                },
                vec![p; 4],
            ),
            (
                "89 % of the frames warns",
                WindowSummary {
                    frames_ratio: 0.89,
                    ..c3a()
                },
                vec![(Warn, Some(Reason::FramesLow)), p, p, p],
            ),
            (
                "30 % of the frames is still only a warning",
                WindowSummary {
                    frames_ratio: 0.30,
                    ..c3a()
                },
                vec![(Warn, Some(Reason::FramesLow)), p, p, p],
            ),
            (
                "no frames fails C4 and skips the level checks",
                WindowSummary {
                    frames: 0,
                    frames_ratio: 0.0,
                    ..c3a()
                },
                vec![
                    (Fail, Some(Reason::NoFrames)),
                    (Skipped, Some(Reason::NotMeasured)),
                    (Skipped, Some(Reason::NotMeasured)),
                    (Skipped, Some(Reason::NotMeasured)),
                ],
            ),
        ];
        for (name, w, want) in cases {
            assert_eq!(grades(&w), want, "{name}");
        }
    }

    #[test]
    fn accumulator_matches_known_levels() {
        // A constant 2^15 LSB at 24 bit is -48.2 dBFS RMS and peak.
        let mut acc = WindowAccumulator::default();
        let s = vec![1 << 15; 3200];
        acc.add(&FrameStats::from_samples(&s, 2, 24));
        let w = acc.summarize(Duration::from_millis(100), 16_000, 24);
        assert_eq!(w.frames, 1600);
        assert!((w.frames_ratio - 1.0).abs() < 1e-9);
        assert!((w.rms_dbfs + 48.16).abs() < 0.05, "{}", w.rms_dbfs);
        assert!((w.peak_dbfs + 48.16).abs() < 0.05);
        assert_eq!(w.peak_to_peak, 0);
        assert_eq!(w.clipped, 0);
    }

    #[test]
    fn accumulator_tracks_extremes_across_batches() {
        let mut acc = WindowAccumulator::default();
        acc.add(&FrameStats::from_samples(&[5, 6, 5, 6], 2, 24));
        acc.add(&FrameStats::from_samples(&[-100, 100, 0, 0], 2, 24));
        let w = acc.summarize(Duration::from_secs(1), 16_000, 24);
        assert_eq!(w.peak_to_peak, 200);
        assert_eq!(acc.frames(), 4);
    }

    #[test]
    fn full_scale_samples_count_as_clipped() {
        let fs = i32::try_from(full_scale(24)).unwrap_or(i32::MAX);
        let s = [fs - 1, -fs, 0, 0];
        let st = FrameStats::from_samples(&s, 2, 24);
        assert_eq!(st.clipped, 2);
        let mut acc = WindowAccumulator::default();
        acc.add(&st);
        let w = acc.summarize(Duration::from_millis(1), 16_000, 24);
        assert!((w.clipped_fraction - 0.5).abs() < 1e-9);
        assert!(w.peak_dbfs > -0.1);
    }

    #[test]
    fn empty_input_is_digital_silence_not_nan() {
        let w = WindowAccumulator::default().summarize(Duration::from_secs(10), 16_000, 24);
        assert_eq!(w.frames, 0);
        assert!((w.rms_dbfs - DBFS_FLOOR).abs() < f64::EPSILON);
        assert_eq!(FrameStats::from_samples(&[], 2, 24).samples, 0);
        assert!((to_dbfs(f64::NAN) - DBFS_FLOOR).abs() < f64::EPSILON);
    }

    #[test]
    fn dbfs_of_full_scale_is_zero_and_half_is_minus_six() {
        assert!(to_dbfs(1.0).abs() < 1e-9);
        assert!((to_dbfs(0.5) + 6.0206).abs() < 1e-3);
    }
}
