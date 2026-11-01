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

fn convert_rate(pcm: &Pcm, out_rate: u32) -> Pcm {
    let ch = usize::from(pcm.channels);
    let in_frames = pcm.frames();
    let ratio = f64::from(pcm.rate) / f64::from(out_rate);
    let out_frames = ((in_frames as f64) / ratio).round() as usize;
    // Cut-off at the lower Nyquist frequency; widen the kernel when downsampling.
    let fc = (f64::from(out_rate) / f64::from(pcm.rate)).min(1.0);
    let half = ZERO_CROSSINGS as f64 / fc;
    let table = kernel_table();

    let mut out = vec![0i16; out_frames * ch];
    let mut acc = vec![0f32; ch];
    for n in 0..out_frames {
        let pos = n as f64 * ratio;
        let lo = (pos - half).ceil().max(0.0) as usize;
        let hi = ((pos + half).floor() as usize).min(in_frames.saturating_sub(1));
        acc.fill(0.0);
        let mut wsum = 0f32;
        for i in lo..=hi {
            let w = kernel(table, (i as f64 - pos) * fc);
            wsum += w;
            let base = i * ch;
            for (c, a) in acc.iter_mut().enumerate() {
                *a += w * f32::from(pcm.samples[base + c]);
            }
        }
        let norm = if wsum.abs() > 1e-6 { 1.0 / wsum } else { 0.0 };
        for (c, a) in acc.iter().enumerate() {
            out[n * ch + c] = (a * norm).round().clamp(-32768.0, 32767.0) as i16;
        }
    }
    Pcm {
        rate: out_rate,
        channels: pcm.channels,
        samples: out,
    }
}
