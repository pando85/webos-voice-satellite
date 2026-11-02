//! Acoustic echo cancellation stage: WebRTC AEC3 through `sonora` (pure
//! Rust port), fed with the TV's own output as the echo reference.
//!
//! - **Input.** Mic samples at 16 kHz after the front-end high-pass filter
//!   and **before** its fixed gain, as `f32` in `[-1, 1)`; reference frames
//!   as captured (interleaved, S24 scale, 16 or 48 kHz), downmixed and
//!   decimated here ([`crate::decimate`]).
//! - **Render channels** ([`AecConfig::render_channels`]). 1 (the default)
//!   gives AEC3 the downmix of all reference channels. 2 gives it the first
//!   two reference channels as a stereo render: AEC3 then detects stereo
//!   content itself and switches to its multichannel configuration while
//!   the channels differ. The delay estimator, the bypass and the metrics
//!   use the downmix in both modes.
//! - **Framing.** sonora takes 10 ms frames (160 samples). Mic and reference
//!   are matched by absolute sample index from the start of the stream (both
//!   streams start on one trigger, see the capture crate). A mic frame waits
//!   until its reference has arrived, at most [`MAX_WAIT_SAMPLES`]; after
//!   that the missing reference is zero-filled and counted.
//! - **Delay.** A GCC-PHAT estimator ([`crate::delay`]) measures the bulk
//!   delay at start and every [`RECHECK_SAMPLES`] while the reference is
//!   active. How a delay is used is [`DelayApply`]: as sonora's stream-delay
//!   hint (the default), not at all (AEC3's own estimator alone), or as a
//!   pre-delay of the reference so AEC3 sees only a small residual delay. On
//!   the c3-a recordings the hint made no measurable difference against AEC3
//!   alone and the pre-delay was harmful (AEC3 output collapsed to its input
//!   seconds after the jump), see `docs/design/aec-frontend.md`.
//! - **Bypass** (off by default). With [`AecConfig::bypass_when_silent`] (and
//!   NS off), sonora is skipped while the reference has been silent for
//!   [`BYPASS_HOLD`] frames, never in the first [`BYPASS_WARMUP`] frames of
//!   a stream; the mic then goes through a delay line of the engine's
//!   latency ([`SONORA_LATENCY`] or [`LINEAR_LATENCY`] samples
//!   line, so the output timing does not change. With AEC3's high-pass
//!   filter off and a zero reference, sonora's output is the input delayed
//!   by exactly that latency (a unit test checks it), and on resume one
//!   frame of history is fed to sonora first so its internal buffer matches
//!   the delay line.
//! - **Output** ([`AecOutput`], experimental). `Full` (the default) is
//!   sonora's `AudioProcessing`: AEC3's linear filter followed by its
//!   nonlinear residual-echo suppressor. `Linear` returns the linear
//!   filter's output alone (sonora-aec3 driven directly, see `direct.rs`),
//!   keeping a quiet near-end talker that the suppressor attenuates, at the
//!   cost of more residual echo. `Nearend` keeps the suppressor but with
//!   AEC3's near-end tuning all the time.
//! - **Metrics** ([`AecMetrics`]): delay, ERLE (mic over output power while
//!   the reference is active) and sonora's own ERLE, processing time per
//!   frame, bypass share, reference underruns. Never audio content.

// Float/int casts are the point of DSP code here: indices, frame counts and
// sample values are bounded (frames of 160, blocks of 4096, delays below
// 2560 samples, stream positions far below 2^53).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::collections::VecDeque;
use std::time::Instant;

use sonora::config::{EchoCanceller as Aec3Config, NoiseSuppression};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::decimate::RefResampler;

mod direct;
use crate::delay::{DelayEstimate, DelayEstimator};

/// Processing rate.
pub const RATE: u32 = 16_000;
/// Samples per sonora frame (10 ms).
pub const FRAME: usize = 160;
/// sonora's capture latency in samples with AEC3 and without NS (8 ms).
pub const SONORA_LATENCY: usize = 128;
/// Latency of [`AecOutput::Linear`] in samples (4 ms): AEC3's 64-sample
/// block framing, without the suppression filter's overlap.
pub const LINEAR_LATENCY: usize = 64;
/// A mic frame waits at most this long for its reference (30 ms).
pub const MAX_WAIT_SAMPLES: usize = 480;
/// Reference frames with a peak below this (-80 dBFS) count as silent.
pub const SILENT_PEAK: f32 = 1e-4;
/// Silent reference frames before the bypass starts (2 s). A 500 ms hold
/// cost up to 3 dB of ERLE on c3-a recordings with short gaps in the
/// reference; 2 s cost nothing on three of four.
pub const BYPASS_HOLD: u32 = 200;
/// No bypass in the first frames of a stream (5 s): bypassing while AEC3
/// was still starting up cost 2 dB of ERLE on a c3-a recording.
pub const BYPASS_WARMUP: u64 = 500;
/// Delay re-check period while the reference is active (30 s).
pub const RECHECK_SAMPLES: u64 = 30 * RATE as u64;
/// Minimum GCC-PHAT confidence (peak over correlation RMS) to accept a delay.
pub const MIN_CONFIDENCE: f32 = 8.0;
/// Pre-delay keeps this much of the measured delay for AEC3 (2 ms), so a
/// small error never makes the echo look non-causal.
pub const PREDELAY_MARGIN: usize = 32;
/// Largest pre-delay (and largest searched delay): 160 ms.
pub const MAX_DELAY_SAMPLES: usize = 2560;
/// Delay changes smaller than this (1 ms) are not applied.
const APPLY_STEP: usize = 16;
/// Reference RMS (full scale 1.0) above which a frame counts as "reference
/// active" for the ERLE metric (-70 dBFS).
const ERLE_REF_RMS: f32 = 3.16e-4;
/// Most render channels the canceller accepts.
pub const MAX_RENDER_CHANNELS: u16 = 2;
const MAX_RENDER: usize = MAX_RENDER_CHANNELS as usize;

/// Where the bulk delay comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DelayMode {
    /// Estimate it (GCC-PHAT) at start and periodically, and apply it.
    Auto,
    /// Use this mic-behind-reference delay in ms (measured on the device,
    /// before decimation). The estimator still runs for the logs.
    FixedMs(f32),
}

/// How a known delay is given to AEC3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayApply {
    /// Pass it to sonora as the stream-delay hint (`set_stream_delay_ms`),
    /// which AEC3 uses to initialise its render buffer alignment after a
    /// reset. AEC3 keeps estimating the delay itself. The default.
    Hint,
    /// Neither: AEC3's own delay estimator alone (the delay is only logged).
    None,
    /// Delay the reference by the delay minus [`PREDELAY_MARGIN`]. Kept for
    /// evaluation only: harmful on the c3-a recordings.
    PreDelay,
}

/// Which AEC3 output the stage returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AecOutput {
    /// AEC3's final output (linear filter, then residual-echo suppression),
    /// through sonora's `AudioProcessing`. The default.
    #[default]
    Full,
    /// The linear filter's output only (no suppression, no comfort noise),
    /// followed by sonora's NS if `ns` is set. Experimental: mono render.
    Linear,
    /// AEC3's final output with the suppressor's near-end tuning used all
    /// the time (AEC3 otherwise uses it only while it detects a dominant
    /// near-end talker), followed by NS if set. Experimental: mono render.
    Nearend,
}

/// Echo canceller settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AecConfig {
    /// Reference sample rate (16000 or 48000).
    pub reference_rate: u32,
    /// Reference channels as captured.
    pub reference_channels: u16,
    /// Channels given to the canceller: 1 = the downmix of all reference
    /// channels (the default), 2 = the first two reference channels as a
    /// stereo render (needs `reference_channels >= 2`).
    pub render_channels: u16,
    /// Digital gain applied to the reference, in dB.
    pub reference_gain_db: f32,
    /// Delay source.
    pub delay: DelayMode,
    /// How the delay is applied.
    pub apply: DelayApply,
    /// sonora's noise suppression (moderate) after the echo canceller.
    pub ns: bool,
    /// Skip sonora while the reference is silent (ignored with `ns`). Off by
    /// default: it saves CPU but cost ERLE on some c3-a recordings.
    pub bypass_when_silent: bool,
    /// Which AEC3 output to return.
    pub output: AecOutput,
}

impl Default for AecConfig {
    fn default() -> Self {
        Self {
            reference_rate: 48_000,
            reference_channels: 2,
            render_channels: 1,
            reference_gain_db: 0.0,
            delay: DelayMode::Auto,
            apply: DelayApply::Hint,
            ns: false,
            bypass_when_silent: false,
            output: AecOutput::Full,
        }
    }
}

/// Metrics since the last [`EchoCanceller::take_metrics`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AecMetrics {
    /// 10 ms frames output.
    pub frames: u64,
    /// Frames that went through sonora.
    pub processed: u64,
    /// Frames passed through the bypass delay line.
    pub bypassed: u64,
    /// Frames whose reference had to be zero-filled.
    pub ref_underruns: u64,
    /// Frames with an active reference (ERLE window).
    pub active_frames: u64,
    /// Mic over output power on active-reference frames, in dB.
    pub erle_db: Option<f32>,
    /// sonora's own ERLE statistic, in dB (instantaneous, if available).
    pub sonora_erle_db: Option<f32>,
    /// sonora's median delay statistic, in ms (if available).
    pub sonora_delay_ms: Option<i32>,
    /// Mean sonora processing time per processed frame, in microseconds.
    pub cpu_us_mean: f32,
    /// Largest sonora processing time of one frame, in microseconds.
    pub cpu_us_max: f32,
    /// sonora time over audio time, in percent of one core.
    pub cpu_pct: f32,
    /// Current delay estimate: mic behind reference at the tap, in ms.
    pub delay_ms: Option<f32>,
    /// Confidence of that estimate.
    pub delay_confidence: Option<f32>,
    /// Pre-delay currently applied to the reference, in ms.
    pub predelay_ms: f32,
    /// Accepted delay estimates since creation.
    pub delay_updates: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Acc {
    frames: u64,
    processed: u64,
    bypassed: u64,
    ref_underruns: u64,
    active_frames: u64,
    mic_energy: f64,
    out_energy: f64,
    cpu_ns: u64,
    cpu_max_ns: u64,
}

/// What changed in the delay after a frame (for logging by the caller).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DelayEvent {
    /// First accepted estimate.
    Locked {
        /// Mic behind reference at the tap, ms.
        delay_ms: f32,
        /// GCC-PHAT confidence.
        confidence: f32,
    },
    /// A later estimate differing from the previous one by 1 ms or more.
    Changed {
        /// Previous delay, ms.
        from_ms: f32,
        /// New delay, ms.
        to_ms: f32,
        /// GCC-PHAT confidence.
        confidence: f32,
    },
}

/// The echo canceller. One instance per capture stream.
#[allow(clippy::struct_excessive_bools)] // independent state flags
pub struct EchoCanceller {
    cfg: AecConfig,
    apm: AudioProcessing,
    /// The direct engine (`AecOutput::Linear` or `Nearend`); `apm` then
    /// runs without an echo canceller and only carries NS (if enabled).
    direct: Option<Box<direct::DirectAec3>>,
    /// One decimator per render channel (with one render channel, it
    /// downmixes all reference channels).
    resamplers: Vec<RefResampler>,
    ref_ch: usize,
    /// Render channels (1 or 2).
    render_ch: usize,
    /// 16 kHz reference samples per render channel, all the same length;
    /// `ref_bufs[c][0]` has absolute index `ref_base`.
    ref_bufs: Vec<VecDeque<f32>>,
    ref_base: u64,
    ref_scratch: Vec<f32>,
    chan_scratch: Vec<i32>,
    /// Pending mic samples; `mic_buf[0]` has absolute index `mic_pos`.
    mic_buf: Vec<f32>,
    mic_pos: u64,
    predelay: usize,
    estimator: DelayEstimator,
    estimating: bool,
    next_check: u64,
    delay: Option<DelayEstimate>,
    delay_updates: u64,
    event: Option<DelayEvent>,
    // Bypass.
    bypass_enabled: bool,
    bypassing: bool,
    silent_frames: u32,
    delay_line: VecDeque<f32>,
    prev_mic: [f32; FRAME],
    prev_ref: [[f32; FRAME]; MAX_RENDER],
    have_prev: bool,
    /// Frames since creation (the bypass warm-up).
    stream_frames: u64,
    acc: Acc,
    errors: u64,
}

impl std::fmt::Debug for EchoCanceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EchoCanceller")
            .field("cfg", &self.cfg)
            .field("predelay", &self.predelay)
            .field("delay", &self.delay)
            .field("bypassing", &self.bypassing)
            .finish_non_exhaustive()
    }
}

fn build_apm(ns: bool, render_channels: u16, aec: bool) -> AudioProcessing {
    let mut c = Config {
        // The front-end already high-passes the mic; AEC3's own filter would
        // make the stage non-transparent, which the bypass relies on.
        echo_canceller: aec.then(|| Aec3Config {
            enforce_high_pass_filtering: false,
            ..Aec3Config::default()
        }),
        high_pass_filter: None,
        ..Config::default()
    };
    if ns {
        c.noise_suppression = Some(NoiseSuppression::default());
    }
    AudioProcessing::builder()
        .config(c)
        .capture_config(StreamConfig::new(RATE, 1))
        .render_config(StreamConfig::new(RATE, render_channels))
        .build()
}

fn ms_to_samples(ms: f32) -> f32 {
    let r = RATE as f32;
    ms * r / 1000.0
}

fn samples_to_ms(s: f32) -> f32 {
    let r = RATE as f32;
    s * 1000.0 / r
}

impl EchoCanceller {
    /// Creates the stage. Returns `None` if the reference rate is not 16000
    /// or 48000, the channel count is zero, `render_channels` is not 1 or 2
    /// or exceeds the reference channels, or a direct output (`Linear`,
    /// `Nearend`) is combined with a stereo render.
    #[must_use]
    pub fn new(cfg: AecConfig) -> Option<Self> {
        let direct = match cfg.output {
            AecOutput::Full => None,
            AecOutput::Linear => Some(direct::DirectOutput::Linear),
            AecOutput::Nearend => Some(direct::DirectOutput::Nearend),
        };
        if cfg.reference_channels == 0
            || !(1..=MAX_RENDER_CHANNELS).contains(&cfg.render_channels)
            || cfg.render_channels > cfg.reference_channels
            || (direct.is_some() && cfg.render_channels != 1)
        {
            return None;
        }
        let gain = if cfg.reference_gain_db.is_finite() {
            10f32.powf(cfg.reference_gain_db / 20.0)
        } else {
            1.0
        };
        let render_ch = usize::from(cfg.render_channels);
        let resamplers = (0..render_ch)
            .map(|_| RefResampler::new(cfg.reference_rate, gain))
            .collect::<Option<Vec<_>>>()?;
        let max = MAX_DELAY_SAMPLES as i32;
        let mut s = Self {
            cfg,
            apm: build_apm(cfg.ns, cfg.render_channels, direct.is_none()),
            direct: direct.map(|d| Box::new(direct::DirectAec3::new(d))),
            resamplers,
            ref_ch: usize::from(cfg.reference_channels),
            render_ch,
            ref_bufs: vec![VecDeque::new(); render_ch],
            ref_base: 0,
            ref_scratch: Vec::new(),
            chan_scratch: Vec::new(),
            mic_buf: Vec::with_capacity(4 * FRAME),
            mic_pos: 0,
            predelay: 0,
            estimator: DelayEstimator::new(RATE, -256, max),
            estimating: true,
            next_check: 0,
            delay: None,
            delay_updates: 0,
            event: None,
            bypass_enabled: cfg.bypass_when_silent && !cfg.ns,
            bypassing: false,
            silent_frames: 0,
            delay_line: VecDeque::with_capacity(SONORA_LATENCY + FRAME),
            prev_mic: [0.0; FRAME],
            prev_ref: [[0.0; FRAME]; MAX_RENDER],
            have_prev: false,
            stream_frames: 0,
            acc: Acc::default(),
            errors: 0,
        };
        if let DelayMode::FixedMs(ms) = cfg.delay {
            s.apply_delay_samples(ms_to_samples(ms));
        }
        Some(s)
    }

    /// Settings in use.
    #[must_use]
    pub fn config(&self) -> &AecConfig {
        &self.cfg
    }

    /// Delay the reference decimator adds, in ms (included when delays are
    /// reported as "at the tap").
    #[must_use]
    pub fn decimator_delay_ms(&self) -> f32 {
        samples_to_ms(self.decimator_delay() as f32)
    }

    fn decimator_delay(&self) -> usize {
        self.resamplers[0].delay_samples()
    }

    /// Output samples held inside the stage (frame buffering plus sonora's
    /// latency); informational.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        self.engine_latency() + if self.cfg.ns { 96 } else { 0 }
    }

    /// Appends reference frames (interleaved, S24 scale, at the configured
    /// rate and channel count).
    pub fn push_reference(&mut self, interleaved: &[i32]) {
        if self.render_ch == 1 {
            self.ref_scratch.clear();
            self.resamplers[0].push(interleaved, self.ref_ch, &mut self.ref_scratch);
            self.ref_bufs[0].extend(self.ref_scratch.iter().copied());
            return;
        }
        for c in 0..self.render_ch {
            self.chan_scratch.clear();
            self.chan_scratch
                .extend(interleaved.chunks_exact(self.ref_ch).map(|f| f[c]));
            self.ref_scratch.clear();
            self.resamplers[c].push(&self.chan_scratch, 1, &mut self.ref_scratch);
            self.ref_bufs[c].extend(self.ref_scratch.iter().copied());
        }
    }

    /// Restarts the alignment after a gap in either stream (the source's
    /// reference epoch changed): both streams start again at index 0. The
    /// adaptive filter is kept; an estimated delay is re-acquired.
    pub fn reset_alignment(&mut self) {
        for r in &mut self.resamplers {
            r.reset();
        }
        for b in &mut self.ref_bufs {
            b.clear();
        }
        self.ref_base = 0;
        self.mic_buf.clear();
        self.mic_pos = 0;
        self.estimator.reset();
        self.estimating = true;
        self.next_check = 0;
        if self.cfg.delay == DelayMode::Auto {
            self.predelay = 0;
            self.delay = None;
        }
        self.silent_frames = 0;
    }

    /// The delay event of the last [`EchoCanceller::process`] call, if any.
    pub fn take_event(&mut self) -> Option<DelayEvent> {
        self.event.take()
    }

    /// Current delay estimate at the tap, in ms.
    #[must_use]
    pub fn delay_ms(&self) -> Option<f32> {
        self.delay.map(|d| self.estimate_to_tap_ms(&d))
    }

    /// The stream-delay hint in force, in ms (0 until a delay is applied).
    #[must_use]
    pub fn delay_hint_ms(&self) -> i32 {
        self.direct
            .as_ref()
            .map_or_else(|| self.apm.stream_delay_ms(), |l| l.delay_hint())
    }

    /// Calls into sonora that returned an error (the mic was passed through).
    #[must_use]
    pub fn errors(&self) -> u64 {
        self.errors
    }

    fn estimate_to_tap_ms(&self, d: &DelayEstimate) -> f32 {
        let total = d.lag_fine + self.predelay as f32 + self.decimator_delay() as f32;
        samples_to_ms(total)
    }

    /// `tap_samples`: mic behind reference at the tap, in samples.
    fn apply_delay_samples(&mut self, tap_samples: f32) {
        let fed = tap_samples - self.decimator_delay() as f32;
        match self.cfg.apply {
            DelayApply::PreDelay => {
                let p = (fed.round().max(0.0) as usize)
                    .saturating_sub(PREDELAY_MARGIN)
                    .min(MAX_DELAY_SAMPLES);
                if p.abs_diff(self.predelay) >= APPLY_STEP || (p == 0) != (self.predelay == 0) {
                    self.predelay = p;
                }
            }
            DelayApply::Hint => {
                let ms = samples_to_ms(fed.max(0.0)).round() as i32;
                match &mut self.direct {
                    Some(l) => l.set_delay_hint(ms),
                    None => {
                        let _ = self.apm.set_stream_delay_ms(ms);
                    }
                }
            }
            DelayApply::None => {}
        }
    }

    fn on_estimate(&mut self, e: DelayEstimate) {
        if e.confidence < MIN_CONFIDENCE {
            return;
        }
        let new_ms = self.estimate_to_tap_ms(&e);
        let old = self.delay.map(|d| self.estimate_to_tap_ms(&d));
        self.delay_updates += 1;
        self.estimating = false;
        self.next_check = self.mic_pos + RECHECK_SAMPLES;
        match old {
            None => {
                self.event = Some(DelayEvent::Locked {
                    delay_ms: new_ms,
                    confidence: e.confidence,
                });
            }
            Some(o) if (o - new_ms).abs() >= 1.0 => {
                self.event = Some(DelayEvent::Changed {
                    from_ms: o,
                    to_ms: new_ms,
                    confidence: e.confidence,
                });
            }
            Some(_) => {}
        }
        if self.cfg.delay == DelayMode::Auto {
            self.apply_delay_samples(ms_to_samples(new_ms));
        }
        // Store the estimate relative to the pre-delay now in force.
        let fed_lag = ms_to_samples(new_ms) - self.predelay as f32 - self.decimator_delay() as f32;
        let lag = fed_lag.round() as i32;
        self.delay = Some(DelayEstimate {
            lag,
            lag_fine: fed_lag,
            confidence: e.confidence,
        });
        self.estimator.reset();
    }

    /// Copies the reference for the mic frame starting at absolute index
    /// `k` (pre-delayed) into `out`, one array per render channel; returns
    /// false if some of it has not arrived yet (that part is zero).
    fn reference_frame(&self, k: u64, out: &mut [[f32; FRAME]]) -> bool {
        let start = k as i64 - self.predelay as i64;
        let mut complete = true;
        for (buf, o) in self.ref_bufs.iter().zip(out.iter_mut()) {
            for (i, o) in o.iter_mut().enumerate() {
                let idx = start + i as i64;
                *o = 0.0;
                if idx < 0 {
                    continue;
                }
                let idx = idx as u64;
                if idx < self.ref_base {
                    continue;
                }
                match buf.get((idx - self.ref_base) as usize) {
                    Some(v) => *o = *v,
                    None => complete = false,
                }
            }
        }
        complete
    }

    /// Processes mic samples (16 kHz, after the high-pass, before the gain,
    /// full scale 1.0) and appends the output to `out`. Output comes in whole
    /// 10 ms frames; up to 159 input samples (plus a frame or two while the
    /// reference catches up) stay buffered.
    pub fn process(&mut self, mic: &[f32], out: &mut Vec<f32>) {
        self.mic_buf.extend_from_slice(mic);
        let mut consumed = 0;
        let mut refr = [[0.0_f32; FRAME]; MAX_RENDER];
        let nr = self.render_ch;
        while self.mic_buf.len() - consumed >= FRAME {
            let k = self.mic_pos;
            let ready = self.reference_frame(k, &mut refr[..nr]);
            if !ready {
                if self.mic_buf.len() - consumed < MAX_WAIT_SAMPLES {
                    break;
                }
                self.acc.ref_underruns += 1;
            }
            let mut m = [0.0_f32; FRAME];
            m.copy_from_slice(&self.mic_buf[consumed..consumed + FRAME]);
            self.frame(&m, &refr[..nr], out);
            consumed += FRAME;
            self.mic_pos += FRAME as u64;
            // Keep enough reference history for the largest pre-delay.
            let keep_from =
                (self.mic_pos as i64 - MAX_DELAY_SAMPLES as i64 - FRAME as i64).max(0) as u64;
            if self.ref_base < keep_from {
                let drop = ((keep_from - self.ref_base) as usize).min(self.ref_bufs[0].len());
                for b in &mut self.ref_bufs {
                    b.drain(..drop);
                }
                self.ref_base += drop as u64;
            }
        }
        self.mic_buf.drain(..consumed);
        // A reference running far ahead (should not happen: one clock) is
        // trimmed so memory stays bounded.
        let ahead_limit = 2 * RATE as usize;
        let len = self.ref_bufs[0].len();
        if len > ahead_limit + MAX_DELAY_SAMPLES {
            let drop = len - ahead_limit - MAX_DELAY_SAMPLES;
            for b in &mut self.ref_bufs {
                b.drain(..drop);
            }
            self.ref_base += drop as u64;
        }
    }

    /// The engine's latency without NS (what the bypass delay line holds).
    fn engine_latency(&self) -> usize {
        if self.cfg.output == AecOutput::Linear {
            LINEAR_LATENCY
        } else {
            SONORA_LATENCY
        }
    }

    fn frame(&mut self, mic: &[f32; FRAME], refr: &[[f32; FRAME]], out: &mut Vec<f32>) {
        self.acc.frames += 1;
        self.stream_frames += 1;
        // The downmix drives the estimator, the bypass and the metrics in
        // both render modes.
        let mono = if refr.len() == 1 {
            refr[0]
        } else {
            let k = 1.0 / refr.len() as f32;
            std::array::from_fn(|i| refr.iter().map(|c| c[i]).sum::<f32>() * k)
        };
        let ref_peak = refr.iter().flatten().fold(0.0_f32, |a, v| a.max(v.abs()));
        let ref_silent = ref_peak < SILENT_PEAK;
        let ref_rms = (mono.iter().map(|v| v * v).sum::<f32>() / FRAME as f32).sqrt();

        // Delay estimation on what AEC3 sees (the pre-delayed reference).
        let due = self.estimating || self.mic_pos >= self.next_check;
        if due && !ref_silent {
            self.estimating = true;
            if let Some(e) = self.estimator.push(mic, &mono) {
                self.on_estimate(e);
            }
        }

        if self.bypass_enabled {
            if ref_silent {
                self.silent_frames = self.silent_frames.saturating_add(1);
            } else {
                self.silent_frames = 0;
            }
            if self.bypassing && !ref_silent {
                // Resume: replay the previous frame so sonora's internal
                // buffer holds the samples the delay line holds now.
                self.bypassing = false;
                self.delay_line.clear();
                if self.have_prev {
                    let (pm, pr) = (self.prev_mic, self.prev_ref);
                    let mut scratch = [0.0_f32; FRAME];
                    let _ = self.run_sonora(&pm, &pr[..self.render_ch], &mut scratch);
                }
            } else if !self.bypassing
                && self.silent_frames >= BYPASS_HOLD
                && self.stream_frames >= BYPASS_WARMUP
            {
                self.bypassing = true;
                self.delay_line.clear();
                let lat = self.engine_latency();
                let tail = &self.prev_mic[FRAME - lat..];
                if self.have_prev {
                    self.delay_line.extend(tail.iter().copied());
                } else {
                    self.delay_line.extend(std::iter::repeat_n(0.0, lat));
                }
            }
        }

        let mut y = [0.0_f32; FRAME];
        if self.bypassing {
            self.acc.bypassed += 1;
            self.delay_line.extend(mic.iter().copied());
            for (o, v) in y.iter_mut().zip(self.delay_line.drain(..FRAME)) {
                *o = v;
            }
        } else if self.run_sonora(mic, refr, &mut y) {
            self.acc.processed += 1;
        } else {
            self.errors += 1;
            y = *mic;
        }
        if ref_rms > ERLE_REF_RMS {
            self.acc.active_frames += 1;
            self.acc.mic_energy += mic.iter().map(|v| f64::from(v * v)).sum::<f64>();
            self.acc.out_energy += y.iter().map(|v| f64::from(v * v)).sum::<f64>();
        }
        self.prev_mic = *mic;
        for (p, r) in self.prev_ref.iter_mut().zip(refr) {
            *p = *r;
        }
        self.have_prev = true;
        out.extend_from_slice(&y);
    }

    fn run_sonora(
        &mut self,
        mic: &[f32; FRAME],
        refr: &[[f32; FRAME]],
        y: &mut [f32; FRAME],
    ) -> bool {
        let t = Instant::now();
        if let Some(l) = &mut self.direct {
            let mut ok = true;
            if self.cfg.ns {
                let mut lin = [0.0_f32; FRAME];
                l.process(mic, &refr[0], &mut lin);
                ok = self
                    .apm
                    .process_capture_f32(&[&lin[..]], &mut [&mut y[..]])
                    .is_ok();
            } else {
                l.process(mic, &refr[0], y);
            }
            let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.acc.cpu_ns = self.acc.cpu_ns.saturating_add(ns);
            self.acc.cpu_max_ns = self.acc.cpu_max_ns.max(ns);
            return ok;
        }
        let mut ro = [[0.0_f32; FRAME]; MAX_RENDER];
        let render_ok = match (refr, &mut ro) {
            ([a], [o, _]) => self
                .apm
                .process_render_f32(&[&a[..]], &mut [&mut o[..]])
                .is_ok(),
            ([a, b], [o0, o1]) => self
                .apm
                .process_render_f32(&[&a[..], &b[..]], &mut [&mut o0[..], &mut o1[..]])
                .is_ok(),
            _ => false,
        };
        let ok = render_ok
            && self
                .apm
                .process_capture_f32(&[&mic[..]], &mut [&mut y[..]])
                .is_ok();
        let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.acc.cpu_ns = self.acc.cpu_ns.saturating_add(ns);
        self.acc.cpu_max_ns = self.acc.cpu_max_ns.max(ns);
        ok
    }

    /// Returns the metrics accumulated since the last call and starts a new
    /// window.
    pub fn take_metrics(&mut self) -> AecMetrics {
        let a = std::mem::take(&mut self.acc);
        let stats = self.apm.statistics();
        let erle_db = (a.active_frames > 0 && a.out_energy > 0.0)
            .then(|| (10.0 * (a.mic_energy / a.out_energy).log10()) as f32);
        let cpu_us_mean = if a.processed > 0 {
            a.cpu_ns as f32 / a.processed as f32 / 1000.0
        } else {
            0.0
        };
        let audio_ns = a.frames as f64 * 10_000_000.0;
        let cpu_pct = if audio_ns > 0.0 {
            (a.cpu_ns as f64 / audio_ns * 100.0) as f32
        } else {
            0.0
        };
        AecMetrics {
            frames: a.frames,
            processed: a.processed,
            bypassed: a.bypassed,
            ref_underruns: a.ref_underruns,
            active_frames: a.active_frames,
            erle_db,
            sonora_erle_db: stats.echo_return_loss_enhancement.map(|v| v as f32),
            sonora_delay_ms: stats.delay_median_ms,
            cpu_us_mean,
            cpu_us_max: a.cpu_max_ns as f32 / 1000.0,
            cpu_pct,
            delay_ms: self.delay_ms(),
            delay_confidence: self.delay.map(|d| d.confidence),
            predelay_ms: samples_to_ms(self.predelay as f32),
            delay_updates: self.delay_updates,
        }
    }
}

#[cfg(test)]
mod tests;
