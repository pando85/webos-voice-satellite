//! AEC3 driven directly through sonora-aec3 (experimental outputs).
//!
//! sonora's `AudioProcessing` returns AEC3's final output: the adaptive
//! linear filter's residual **after** the nonlinear residual-echo
//! suppressor, with AEC3's default tuning, and exposes neither the linear
//! output nor the suppressor tuning. On c3-a recordings with a quiet
//! near-end talker under TV content, that suppressor also removed most of
//! the talker (`docs/experiments/P2-aec-offline-ab.md`). This engine drives
//! sonora-aec3's `BlockProcessor` the way sonora's `EchoCanceller3` does at
//! 16 kHz (one band, mono render and capture) and returns either
//!
//! - the **linear** output (`filter.export_linear_aec_output`): mic minus
//!   the filter's echo estimate, no suppression, no comfort noise; or
//! - the **final** output with a given configuration (used for the
//!   near-end tuning: the suppressor's `nearend_tuning` all the time).
//!
//! Samples are scaled to the S16 range AEC3 works in, as `AudioProcessing`
//! does. Latency: 64 samples for the linear output (block framing), 128 for
//! the final output (plus the suppression filter's overlap), as with
//! `AudioProcessing`.

use sonora_aec3::block::Block;
use sonora_aec3::block_framer::BlockFramer;
use sonora_aec3::block_processor::BlockProcessor;
use sonora_aec3::common::SUB_FRAME_LENGTH;
use sonora_aec3::config::EchoCanceller3Config;
use sonora_aec3::frame_blocker::FrameBlocker;

use super::{FRAME, RATE};

/// Float full scale to AEC3's internal S16 scale.
const SCALE: f32 = 32_768.0;
/// AEC3's capture saturation threshold (sonora `detect_saturation`).
const SATURATION: f32 = 32_700.0;

/// Which output the direct engine returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectOutput {
    /// The linear filter's output.
    Linear,
    /// AEC3's final output, default configuration (for validation).
    #[cfg_attr(not(test), allow(dead_code))]
    Final,
    /// AEC3's final output with the near-end suppressor tuning always.
    Nearend,
}

/// The direct engine. One per stream.
pub(crate) struct DirectAec3 {
    output: DirectOutput,
    bp: BlockProcessor,
    render_blocker: FrameBlocker,
    capture_blocker: FrameBlocker,
    framer: BlockFramer,
    render_block: Block,
    capture_block: Block,
    linear_block: Block,
    sub_out: Vec<Vec<Vec<f32>>>,
    delay_ms: i32,
}

impl std::fmt::Debug for DirectAec3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectAec3")
            .field("output", &self.output)
            .field("delay_ms", &self.delay_ms)
            .finish_non_exhaustive()
    }
}

impl DirectAec3 {
    pub(crate) fn new(output: DirectOutput) -> Self {
        let mut cfg = EchoCanceller3Config::default();
        match output {
            DirectOutput::Linear => cfg.filter.export_linear_aec_output = true,
            DirectOutput::Final => {}
            DirectOutput::Nearend => {
                cfg.suppressor.normal_tuning = cfg.suppressor.nearend_tuning.clone();
            }
        }
        Self {
            output,
            bp: BlockProcessor::new(&cfg, RATE as usize, 1, 1),
            render_blocker: FrameBlocker::new(1, 1),
            capture_blocker: FrameBlocker::new(1, 1),
            framer: BlockFramer::new(1, 1),
            render_block: Block::new(1, 1),
            capture_block: Block::new(1, 1),
            linear_block: Block::new(1, 1),
            sub_out: vec![vec![vec![0.0; SUB_FRAME_LENGTH]; 1]; 1],
            delay_ms: 0,
        }
    }

    /// The stream-delay hint (as `AudioProcessing::set_stream_delay_ms`).
    pub(crate) fn set_delay_hint(&mut self, ms: i32) {
        self.delay_ms = ms.clamp(0, 500);
        self.bp.set_audio_buffer_delay(self.delay_ms);
    }

    pub(crate) fn delay_hint(&self) -> i32 {
        self.delay_ms
    }

    fn capture(&mut self, saturated: bool) {
        let linear = self.output == DirectOutput::Linear;
        self.bp.process_capture(
            false,
            saturated,
            linear.then_some(&mut self.linear_block),
            &mut self.capture_block,
        );
    }

    /// One 10 ms frame: mono reference and mic in, output out (full scale
    /// 1.0).
    pub(crate) fn process(
        &mut self,
        mic: &[f32; FRAME],
        refr: &[f32; FRAME],
        y: &mut [f32; FRAME],
    ) {
        let mut r = [0.0_f32; FRAME];
        let mut m = [0.0_f32; FRAME];
        for (o, v) in r.iter_mut().zip(refr) {
            *o = v * SCALE;
        }
        let mut saturated = false;
        for (o, v) in m.iter_mut().zip(mic) {
            *o = v * SCALE;
            saturated |= o.abs() >= SATURATION;
        }
        for sf in r.as_chunks::<SUB_FRAME_LENGTH>().0 {
            self.render_blocker
                .insert_sub_frame_and_extract_block(&[vec![&sf[..]]], &mut self.render_block);
            self.bp.buffer_render(&self.render_block);
        }
        if self.render_blocker.is_block_available() {
            self.render_blocker.extract_block(&mut self.render_block);
            self.bp.buffer_render(&self.render_block);
        }
        for (sf, out) in m
            .as_chunks::<SUB_FRAME_LENGTH>()
            .0
            .iter()
            .zip(y.as_chunks_mut::<SUB_FRAME_LENGTH>().0)
        {
            self.capture_blocker
                .insert_sub_frame_and_extract_block(&[vec![&sf[..]]], &mut self.capture_block);
            self.capture(saturated);
            let block = if self.output == DirectOutput::Linear {
                &self.linear_block
            } else {
                &self.capture_block
            };
            self.framer
                .insert_block_and_extract_sub_frame(block, &mut self.sub_out);
            for (o, v) in out.iter_mut().zip(&self.sub_out[0][0]) {
                *o = v / SCALE;
            }
        }
        if self.capture_blocker.is_block_available() {
            self.capture_blocker.extract_block(&mut self.capture_block);
            self.capture(saturated);
            let block = if self.output == DirectOutput::Linear {
                &self.linear_block
            } else {
                &self.capture_block
            };
            self.framer.insert_block(block);
        }
    }
}
