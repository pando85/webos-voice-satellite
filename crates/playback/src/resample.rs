//! Channel mixing and windowed-sinc sample-rate conversion.
//!
//! A small purpose-built resampler instead of a general crate: TTS replies are
//! short mono speech at 16 to 48 kHz. The kernel is a Blackman-Harris windowed
//! sinc (16 zero crossings, low-pass scaled when downsampling) read from a
//! linearly interpolated table, and each output sample is normalised by its
//! tap sum so the DC gain is exactly 1.

// Float/int casts are the point of DSP code here: indices and sample values are
// bounded (buffers are capped by `MAX_DECODED_SAMPLES`, outputs are clamped).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::f64::consts::PI;
use std::sync::OnceLock;

use crate::pcm::Pcm;

const ZERO_CROSSINGS: usize = 16;
const TABLE_RES: usize = 256;

fn kernel_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=ZERO_CROSSINGS * TABLE_RES)
            .map(|i| {
                let x = i as f64 / TABLE_RES as f64;
                let sinc = if i == 0 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                };
                let t = x / ZERO_CROSSINGS as f64;
                let win = 0.358_75
                    + 0.488_29 * (PI * t).cos()
                    + 0.141_28 * (2.0 * PI * t).cos()
                    + 0.011_68 * (3.0 * PI * t).cos();
                (sinc * win) as f32
            })
            .collect()
    })
}

fn kernel(table: &[f32], x: f64) -> f32 {
    let p = x.abs() * TABLE_RES as f64;
    let idx = p as usize;
    if idx >= table.len() - 1 {
        return 0.0;
    }
    let frac = (p - idx as f64) as f32;
    table[idx] + (table[idx + 1] - table[idx]) * frac
}

/// Converts `pcm` to the requested rate and channel count.
///
/// Channel mapping: same count is a copy; any to mono averages all channels;
/// mono to N duplicates; otherwise the first `channels` input channels are kept
/// (more outputs than inputs repeat input channels cyclically). A `rate` or
/// `channels` of 0 leaves that property unchanged.
#[must_use]
pub fn resample(pcm: &Pcm, rate: u32, channels: u16) -> Pcm {
    let rate = if rate == 0 { pcm.rate } else { rate };
    let channels = if channels == 0 {
        pcm.channels
    } else {
        channels
    };
    if pcm.channels == 0 || pcm.rate == 0 {
        return pcm.clone();
    }
    // Downmix before and upmix after the rate conversion to do the least work.
    let (first, second) = if channels < pcm.channels {
        (Some(channels), None)
    } else {
        (None, Some(channels))
    };
    let mut cur = pcm.clone();
    if let Some(c) = first {
        cur = mix_channels(&cur, c);
    }
    if cur.rate != rate {
        cur = convert_rate(&cur, rate);
    }
    if let Some(c) = second {
        cur = mix_channels(&cur, c);
    }
    cur
}

/// Channel up/down-mix only; see [`resample`] for the mapping rules.
#[must_use]
pub fn mix_channels(pcm: &Pcm, channels: u16) -> Pcm {
    let inc = usize::from(pcm.channels);
    let outc = usize::from(channels);
    if inc == 0 || outc == 0 || inc == outc {
        return pcm.clone();
    }
    let frames = pcm.frames();
    let mut out = Vec::with_capacity(frames * outc);
    for frame in pcm.samples.chunks_exact(inc) {
        if outc == 1 {
            let sum: i32 = frame.iter().map(|&s| i32::from(s)).sum();
            out.push(i16::try_from(sum / inc as i32).unwrap_or(0));
        } else {
            for c in 0..outc {
                out.push(frame[c % inc]);
            }
        }
    }
    Pcm {
        rate: pcm.rate,
        channels,
        samples: out,
    }
}

/// Kernel parameters of a rate conversion: the ratio of input to output
/// rate, the cut-off (relative to the input Nyquist) and the half width of
/// the kernel in input frames.
fn rate_params(in_rate: u32, out_rate: u32) -> (f64, f64, f64) {
    let ratio = f64::from(in_rate) / f64::from(out_rate);
    // Cut-off at the lower Nyquist frequency; widen the kernel when downsampling.
    let fc = (f64::from(out_rate) / f64::from(in_rate)).min(1.0);
    (ratio, fc, ZERO_CROSSINGS as f64 / fc)
}

/// Appends the output frame at input position `pos`, from input frames
/// `lo..=hi`, to `out`. `samples` holds interleaved frames starting at input
/// frame `base`. Shared by [`convert_rate`] and [`StreamResampler`], so both
/// compute bit-identical output.
fn render_frame(
    (samples, base, ch): (&[i16], usize, usize),
    (pos, fc): (f64, f64),
    (lo, hi): (usize, usize),
    acc: &mut [f32],
    out: &mut Vec<i16>,
) {
    let table = kernel_table();
    acc.fill(0.0);
    let mut wsum = 0f32;
    for i in lo..=hi {
        let w = kernel(table, (i as f64 - pos) * fc);
        wsum += w;
        let at = (i - base) * ch;
        for (c, a) in acc.iter_mut().enumerate() {
            *a += w * f32::from(samples[at + c]);
        }
    }
    let norm = if wsum.abs() > 1e-6 { 1.0 / wsum } else { 0.0 };
    for a in acc.iter() {
        out.push((a * norm).round().clamp(-32768.0, 32767.0) as i16);
    }
}

fn convert_rate(pcm: &Pcm, out_rate: u32) -> Pcm {
    let ch = usize::from(pcm.channels);
    let in_frames = pcm.frames();
    let (ratio, fc, half) = rate_params(pcm.rate, out_rate);
    let out_frames = ((in_frames as f64) / ratio).round() as usize;

    let mut out = Vec::with_capacity(out_frames * ch);
    let mut acc = vec![0f32; ch];
    for n in 0..out_frames {
        let pos = n as f64 * ratio;
        let lo = (pos - half).ceil().max(0.0) as usize;
        let hi = ((pos + half).floor() as usize).min(in_frames.saturating_sub(1));
        render_frame(
            (&pcm.samples, 0, ch),
            (pos, fc),
            (lo, hi),
            &mut acc,
            &mut out,
        );
    }
    Pcm {
        rate: out_rate,
        channels: pcm.channels,
        samples: out,
    }
}

/// [`resample`] for audio that arrives in chunks, with bounded memory.
///
/// Feed chunks with [`StreamResampler::push`] and end with
/// [`StreamResampler::flush`]. The concatenated output is **sample for
/// sample the same** as [`resample`] of the whole input: the same kernel, the
/// same positions (`n * ratio` from the first frame), the same edge handling
/// at the start and, at [`flush`](Self::flush), at the end. Only the input
/// frames that later outputs still reach are kept (about 33 frames when
/// upsampling, more when downsampling) plus the newest chunk, so memory does
/// not grow with the length of the stream.
#[derive(Debug, Clone)]
pub struct StreamResampler {
    in_rate: u32,
    out_rate: u32,
    out_channels: u16,
    /// Channels during the rate conversion (after a downmix).
    work_channels: u16,
    ratio: f64,
    fc: f64,
    half: f64,
    /// Kept input frames (interleaved, `work_channels`), from frame `base`.
    pending: Vec<i16>,
    base: usize,
    /// Input frames pushed so far.
    total_in: usize,
    /// Next output frame index.
    next_out: usize,
    acc: Vec<f32>,
}

impl StreamResampler {
    /// A converter from `in_rate`/`in_channels` to `out_rate`/`out_channels`.
    /// An `out_rate` or `out_channels` of 0 keeps that property, as in
    /// [`resample`].
    #[must_use]
    pub fn new(in_rate: u32, in_channels: u16, out_rate: u32, out_channels: u16) -> Self {
        let out_rate = if out_rate == 0 { in_rate } else { out_rate };
        let out_channels = if out_channels == 0 {
            in_channels
        } else {
            out_channels
        };
        let work_channels = out_channels.min(in_channels);
        let (ratio, fc, half) = if in_rate == 0 {
            (1.0, 1.0, 0.0)
        } else {
            rate_params(in_rate, out_rate)
        };
        Self {
            in_rate,
            out_rate,
            out_channels,
            work_channels,
            ratio,
            fc,
            half,
            pending: Vec::new(),
            base: 0,
            total_in: 0,
            next_out: 0,
            acc: vec![0.0; usize::from(work_channels)],
        }
    }

    fn passthrough(&self) -> bool {
        self.in_rate == self.out_rate || self.in_rate == 0 || self.work_channels == 0
    }

    /// Converts one chunk (in the input format given to [`new`](Self::new));
    /// returns the output frames that are complete so far (possibly none).
    #[must_use]
    pub fn push(&mut self, pcm: &Pcm) -> Pcm {
        let mixed;
        let chunk = if self.work_channels < pcm.channels {
            mixed = mix_channels(pcm, self.work_channels);
            &mixed
        } else {
            pcm
        };
        if self.passthrough() {
            return self.finish_channels(chunk.samples.clone());
        }
        self.total_in += chunk.frames();
        self.pending.extend_from_slice(&chunk.samples);
        let out = self.render(false);
        self.finish_channels(out)
    }

    /// Ends the stream: the last output frames, computed with the end of the
    /// input as the edge (as [`resample`] does).
    #[must_use]
    pub fn flush(&mut self) -> Pcm {
        if self.passthrough() {
            return self.finish_channels(Vec::new());
        }
        let out = self.render(true);
        self.pending = Vec::new();
        self.finish_channels(out)
    }

    fn render(&mut self, last: bool) -> Vec<i16> {
        let ch = usize::from(self.work_channels);
        let total_out = if last {
            ((self.total_in as f64) / self.ratio).round() as usize
        } else {
            usize::MAX
        };
        let mut out = Vec::new();
        while self.next_out < total_out && self.total_in > 0 {
            let pos = self.next_out as f64 * self.ratio;
            let lo = (pos - self.half).ceil().max(0.0) as usize;
            let reach = (pos + self.half).floor() as usize;
            let hi = if last {
                reach.min(self.total_in - 1)
            } else if reach < self.total_in {
                reach
            } else {
                // Needs input that has not arrived yet.
                break;
            };
            render_frame(
                (&self.pending, self.base, ch),
                (pos, self.fc),
                (lo, hi),
                &mut self.acc,
                &mut out,
            );
            self.next_out += 1;
        }
        // Drop the input frames that no later output reaches.
        let pos = self.next_out as f64 * self.ratio;
        let keep_from = ((pos - self.half).ceil().max(0.0) as usize)
            .clamp(self.base, self.base + self.pending.len() / ch);
        let drop = (keep_from - self.base) * ch;
        if drop > 0 {
            self.pending.drain(..drop);
            self.base = keep_from;
        }
        out
    }

    fn finish_channels(&self, samples: Vec<i16>) -> Pcm {
        let pcm = Pcm {
            rate: self.out_rate,
            channels: self.work_channels,
            samples,
        };
        if self.out_channels == self.work_channels {
            pcm
        } else {
            mix_channels(&pcm, self.out_channels)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn noise(rate: u32, channels: u16, frames: usize) -> Pcm {
        // A deterministic pseudo-random signal with all frequencies.
        let mut x: u32 = 0x1234_5678;
        let samples = (0..frames * usize::from(channels))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 16) as i16 / 2
            })
            .collect();
        Pcm {
            rate,
            channels,
            samples,
        }
    }

    fn chunked(pcm: &Pcm, rate: u32, channels: u16, chunk_frames: usize) -> Pcm {
        let mut r = StreamResampler::new(pcm.rate, pcm.channels, rate, channels);
        let ch = usize::from(pcm.channels);
        let mut out = Vec::new();
        for part in pcm.samples.chunks(chunk_frames * ch) {
            let o = r.push(&Pcm {
                rate: pcm.rate,
                channels: pcm.channels,
                samples: part.to_vec(),
            });
            assert_eq!((o.rate, o.channels), (rate, channels));
            out.extend_from_slice(&o.samples);
            // Bounded: one chunk plus the kernel's reach, never the stream.
            assert!(
                r.pending.len() <= (chunk_frames + 100) * ch,
                "{} pending",
                r.pending.len()
            );
        }
        let o = r.flush();
        out.extend_from_slice(&o.samples);
        Pcm {
            rate,
            channels,
            samples: out,
        }
    }

    #[test]
    fn chunked_output_equals_whole_buffer_output() {
        let cases: &[(u32, u16, u32, u16)] = &[
            (22_050, 1, 44_100, 2),
            (16_000, 1, 44_100, 2),
            (48_000, 2, 44_100, 2),
            (44_100, 2, 22_050, 1),
            (24_000, 1, 48_000, 1),
            (44_100, 2, 44_100, 2),
            (44_100, 1, 44_100, 2),
            (8_000, 2, 44_100, 1),
        ];
        for &(ir, ic, or, oc) in cases {
            let src = noise(ir, ic, 9_001);
            let whole = resample(&src, or, oc);
            for chunk in [1, 7, 441, 2_205, 9_001] {
                let got = chunked(&src, or, oc, chunk);
                assert_eq!(
                    (got.rate, got.channels),
                    (whole.rate, whole.channels),
                    "{ir}/{ic} -> {or}/{oc} chunk {chunk}"
                );
                assert!(
                    got.samples == whole.samples,
                    "{ir}/{ic} -> {or}/{oc} chunk {chunk}: {} vs {} samples",
                    got.samples.len(),
                    whole.samples.len()
                );
            }
        }
    }

    #[test]
    fn empty_and_tiny_streams() {
        let mut r = StreamResampler::new(22_050, 1, 44_100, 2);
        assert_eq!(r.flush().samples.len(), 0);
        for frames in [1, 2, 31, 40] {
            let src = noise(22_050, 1, frames);
            assert_eq!(
                chunked(&src, 44_100, 2, 1).samples,
                resample(&src, 44_100, 2).samples,
                "{frames} frames"
            );
        }
    }
}
