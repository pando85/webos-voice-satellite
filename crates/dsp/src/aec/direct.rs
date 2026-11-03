//! AEC3 driven directly through sonora-aec3 (experimental outputs and
//! tuning).
//!
//! sonora's `AudioProcessing` returns AEC3's final output: the adaptive
//! linear filter's residual **after** the nonlinear residual-echo
//! suppressor, with AEC3's default tuning, and exposes neither the linear
//! output nor the configuration. On c3-a recordings with a quiet near-end
//! talker under TV content, that suppressor also removed most of the talker
//! (`docs/experiments/P2-aec-offline-ab.md`). This engine drives
//! sonora-aec3's `BlockProcessor` the way sonora's `EchoCanceller3` does at
//! 16 kHz (one band, mono capture, mono or stereo render) and returns either
//!
//! - the **linear** output (`filter.export_linear_aec_output`): mic minus
//!   the filter's echo estimate, no suppression, no comfort noise; or
//! - the **final** output with a given configuration: AEC3's defaults with
//!   the overrides of [`Aec3Tuning`] (and, for the near-end variant, the
//!   suppressor's `nearend_tuning` all the time), optionally blended with the
//!   aligned linear output (`linear_mix`).
//!
//! With a stereo render it repeats `EchoCanceller3`'s multichannel handling:
//! AEC3's stereo-content detector decides when the render is proper stereo,
//! the block processor is then rebuilt with two render channels and AEC3's
//! default multichannel configuration (plus the same overrides), and while
//! stereo content is only "temporary" the render is downmixed and the echo
//! path is flagged as changed.
//!
//! Samples are scaled to the S16 range AEC3 works in, as `AudioProcessing`
//! does. Latency: 64 samples for the linear output (block framing), 128 for
//! the final output (plus the suppression filter's overlap), as with
//! `AudioProcessing`.

use std::collections::VecDeque;

use sonora_aec3::block::Block;
use sonora_aec3::block_framer::BlockFramer;
use sonora_aec3::block_processor::BlockProcessor;
use sonora_aec3::common::SUB_FRAME_LENGTH;
use sonora_aec3::config::EchoCanceller3Config;
use sonora_aec3::frame_blocker::FrameBlocker;
use sonora_aec3::multi_channel_content_detector::MultiChannelContentDetector;

use super::tuning::Aec3Tuning;
use super::{FRAME, LINEAR_LATENCY, RATE, SONORA_LATENCY};

/// Float full scale to AEC3's internal S16 scale.
const SCALE: f32 = 32_768.0;
/// AEC3's capture saturation threshold (sonora `detect_saturation`).
const SATURATION: f32 = 32_700.0;
/// Extra delay of the final output over the linear one (samples).
const FINAL_OVER_LINEAR: usize = SONORA_LATENCY - LINEAR_LATENCY;

/// Which output the direct engine returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectOutput {
    /// The linear filter's output.
    Linear,
    /// AEC3's final output (with the tuning overrides, if any).
    Final,
    /// AEC3's final output with the near-end suppressor tuning always.
    Nearend,
}

/// The direct engine. One per stream.
pub(crate) struct DirectAec3 {
    output: DirectOutput,
    mono_cfg: EchoCanceller3Config,
    multi_cfg: EchoCanceller3Config,
    /// Render channels delivered (1 or 2).
    render_in: usize,
    /// Render channels given to AEC3 (1 until proper stereo is detected).
    render_to_aec: usize,
    detector: MultiChannelContentDetector,
    bp: BlockProcessor,
    render_blocker: FrameBlocker,
    capture_blocker: FrameBlocker,
    framer: BlockFramer,
    lin_framer: BlockFramer,
    render_block: Block,
    capture_block: Block,
    linear_block: Block,
    render_frame: Vec<Vec<Vec<f32>>>,
    render_sub: Vec<Vec<Vec<f32>>>,
    sub_out: Vec<Vec<Vec<f32>>>,
    lin_out: Vec<Vec<Vec<f32>>>,
    /// Share of the linear output in the final output (0 = none).
    mix: f32,
    /// Aligns the linear output to the final output for the blend.
    lin_delay: VecDeque<f32>,
    delay_ms: i32,
    delay_set: bool,
}

impl std::fmt::Debug for DirectAec3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectAec3")
            .field("output", &self.output)
            .field("render_in", &self.render_in)
            .field("render_to_aec", &self.render_to_aec)
            .field("mix", &self.mix)
            .field("delay_ms", &self.delay_ms)
            .finish_non_exhaustive()
    }
}

fn configure(
    mut cfg: EchoCanceller3Config,
    output: DirectOutput,
    tuning: &Aec3Tuning,
    linear_wanted: bool,
) -> EchoCanceller3Config {
    if output == DirectOutput::Nearend {
        cfg.suppressor.normal_tuning = cfg.suppressor.nearend_tuning.clone();
    }
    tuning.apply(&mut cfg);
    cfg.filter.export_linear_aec_output = linear_wanted;
    cfg
}

impl DirectAec3 {
    /// `render_channels`: 1 or 2.
    pub(crate) fn new(output: DirectOutput, tuning: &Aec3Tuning, render_channels: usize) -> Self {
        let render_in = render_channels.clamp(1, 2);
        let mix = if output == DirectOutput::Linear {
            0.0
        } else {
            tuning.linear_mix()
        };
        let linear_wanted = output == DirectOutput::Linear || mix > 0.0;
        let mono_cfg = configure(
            EchoCanceller3Config::default(),
            output,
            tuning,
            linear_wanted,
        );
        let multi_cfg = configure(
            EchoCanceller3Config::create_default_multichannel_config(),
            output,
            tuning,
            linear_wanted,
        );
        let mc = &mono_cfg.multi_channel;
        let detector = MultiChannelContentDetector::new(
            mc.detect_stereo_content,
            render_in,
            mc.stereo_detection_threshold,
            mc.stereo_detection_timeout_threshold_seconds,
            mc.stereo_detection_hysteresis_seconds,
        );
        Self {
            output,
            bp: BlockProcessor::new(&mono_cfg, RATE as usize, 1, 1),
            mono_cfg,
            multi_cfg,
            render_in,
            render_to_aec: 1,
            detector,
            render_blocker: FrameBlocker::new(1, 1),
            capture_blocker: FrameBlocker::new(1, 1),
            framer: BlockFramer::new(1, 1),
            lin_framer: BlockFramer::new(1, 1),
            render_block: Block::new(1, 1),
            capture_block: Block::new(1, 1),
            linear_block: Block::new(1, 1),
            render_frame: vec![vec![vec![0.0; FRAME]; render_in]; 1],
            render_sub: vec![vec![vec![0.0; SUB_FRAME_LENGTH]; 1]; 1],
            sub_out: vec![vec![vec![0.0; SUB_FRAME_LENGTH]; 1]; 1],
            lin_out: vec![vec![vec![0.0; SUB_FRAME_LENGTH]; 1]; 1],
            mix,
            lin_delay: std::iter::repeat_n(0.0, FINAL_OVER_LINEAR).collect(),
            delay_ms: 0,
            delay_set: false,
        }
    }

    /// The stream-delay hint (as `AudioProcessing::set_stream_delay_ms`).
    pub(crate) fn set_delay_hint(&mut self, ms: i32) {
        self.delay_ms = ms.clamp(0, 500);
        self.delay_set = true;
    }

    pub(crate) fn delay_hint(&self) -> i32 {
        self.delay_ms
    }

    /// True while AEC3 runs with a stereo render.
    #[cfg(test)]
    pub(crate) fn stereo_active(&self) -> bool {
        self.render_to_aec == 2
    }

    /// Rebuilds the block processor after a change of the stereo detection
    /// (`EchoCanceller3::initialize`).
    fn reinitialize(&mut self) {
        let proper = self.detector.is_proper_multi_channel_content_detected();
        self.render_to_aec = if proper { self.render_in } else { 1 };
        let cfg = if proper && self.render_in > 1 {
            &self.multi_cfg
        } else {
            &self.mono_cfg
        };
        self.render_block.set_num_channels(self.render_to_aec);
        self.render_blocker = FrameBlocker::new(1, self.render_to_aec);
        self.bp = BlockProcessor::new(cfg, RATE as usize, self.render_to_aec, 1);
        self.render_sub = vec![vec![vec![0.0; SUB_FRAME_LENGTH]; self.render_to_aec]; 1];
    }

    fn buffer_render(&mut self) {
        if self.detector.update_detection(&self.render_frame) {
            self.reinitialize();
        }
        let downmix = self.detector.is_temporary_multi_channel_content_detected();
        let n_in = self.render_in;
        for half in 0..2 {
            let off = half * SUB_FRAME_LENGTH;
            let band = &mut self.render_frame[0];
            if n_in > self.render_to_aec {
                if downmix {
                    let k = 1.0 / n_in as f32;
                    let (first, rest) = band.split_at_mut(1);
                    for (i, v) in first[0][off..off + SUB_FRAME_LENGTH].iter_mut().enumerate() {
                        let s: f32 = rest.iter().map(|c| c[off + i]).sum();
                        *v = (*v + s) * k;
                    }
                }
                self.render_sub[0][0].copy_from_slice(&band[0][off..off + SUB_FRAME_LENGTH]);
            } else {
                for (dst, src) in self.render_sub[0].iter_mut().zip(band.iter()) {
                    dst.copy_from_slice(&src[off..off + SUB_FRAME_LENGTH]);
                }
            }
            let refs: Vec<Vec<&[f32]>> = self
                .render_sub
                .iter()
                .map(|b| b.iter().map(Vec::as_slice).collect())
                .collect();
            self.render_blocker
                .insert_sub_frame_and_extract_block(&refs, &mut self.render_block);
            self.bp.buffer_render(&self.render_block);
        }
        if self.render_blocker.is_block_available() {
            self.render_blocker.extract_block(&mut self.render_block);
            self.bp.buffer_render(&self.render_block);
        }
    }

    fn capture(&mut self, path_change: bool, saturated: bool) {
        let linear = self.output == DirectOutput::Linear || self.mix > 0.0;
        self.bp.process_capture(
            path_change,
            saturated,
            linear.then_some(&mut self.linear_block),
            &mut self.capture_block,
        );
    }

    /// One 10 ms frame: reference channels (one or two arrays, as many as
    /// `render_channels`) and mic in, output out (full scale 1.0).
    pub(crate) fn process(
        &mut self,
        mic: &[f32; FRAME],
        refr: &[[f32; FRAME]],
        y: &mut [f32; FRAME],
    ) {
        for c in 0..self.render_in {
            let src = refr.get(c).or_else(|| refr.first());
            for (o, v) in self.render_frame[0][c]
                .iter_mut()
                .zip(src.into_iter().flatten())
            {
                *o = v * SCALE;
            }
        }
        let mut m = [0.0_f32; FRAME];
        let mut saturated = false;
        for (o, v) in m.iter_mut().zip(mic) {
            *o = v * SCALE;
            saturated |= o.abs() >= SATURATION;
        }
        if self.delay_set {
            self.bp.set_audio_buffer_delay(self.delay_ms);
        }
        self.buffer_render();
        let path_change = self.detector.is_temporary_multi_channel_content_detected();
        let linear_out = self.output == DirectOutput::Linear;
        for (sf, out) in m
            .as_chunks::<SUB_FRAME_LENGTH>()
            .0
            .iter()
            .zip(y.as_chunks_mut::<SUB_FRAME_LENGTH>().0)
        {
            self.capture_blocker
                .insert_sub_frame_and_extract_block(&[vec![&sf[..]]], &mut self.capture_block);
            self.capture(path_change, saturated);
            let block = if linear_out {
                &self.linear_block
            } else {
                &self.capture_block
            };
            self.framer
                .insert_block_and_extract_sub_frame(block, &mut self.sub_out);
            if self.mix > 0.0 {
                self.lin_framer
                    .insert_block_and_extract_sub_frame(&self.linear_block, &mut self.lin_out);
                let m = self.mix;
                for (o, (f, l)) in out
                    .iter_mut()
                    .zip(self.sub_out[0][0].iter().zip(&self.lin_out[0][0]))
                {
                    self.lin_delay.push_back(*l);
                    let la = self.lin_delay.pop_front().unwrap_or(0.0);
                    *o = ((1.0 - m) * f + m * la) / SCALE;
                }
            } else {
                for (o, v) in out.iter_mut().zip(&self.sub_out[0][0]) {
                    *o = v / SCALE;
                }
            }
        }
        if self.capture_blocker.is_block_available() {
            self.capture_blocker.extract_block(&mut self.capture_block);
            self.capture(path_change, saturated);
            let block = if linear_out {
                &self.linear_block
            } else {
                &self.capture_block
            };
            self.framer.insert_block(block);
            if self.mix > 0.0 {
                self.lin_framer.insert_block(&self.linear_block);
            }
        }
    }
}
