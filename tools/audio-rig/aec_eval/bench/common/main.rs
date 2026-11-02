//! Throwaway AEC evaluation harness (host + armv7 benchmark). One binary per candidate;
//! the candidate lives in `src/engine.rs` of each crate and is pulled in with include!.
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub const SR: u32 = 16_000;
pub const FRAME: usize = 160; // 10 ms

/// A candidate: one 10 ms reference frame and one 10 ms mic frame in, processed samples appended.
pub trait Engine {
    fn process(&mut self, reference: &[f32], mic: &[f32], out: &mut Vec<f32>);
    fn flush(&mut self, _out: &mut Vec<f32>) {}
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/engine.rs"));

fn read_wav(path: &str) -> Vec<f32> {
    let mut b = Vec::new();
    std::fs::File::open(path).expect("open wav").read_to_end(&mut b).expect("read wav");
    assert!(&b[0..4] == b"RIFF" && &b[8..12] == b"WAVE", "not a wav");
    let mut p = 12;
    let (mut fmt, mut bits) = (0u16, 0u16);
    while p + 8 <= b.len() {
        let id = &b[p..p + 4];
        let sz = u32::from_le_bytes([b[p + 4], b[p + 5], b[p + 6], b[p + 7]]) as usize;
        if id == b"fmt " {
            fmt = u16::from_le_bytes([b[p + 8], b[p + 9]]);
            let ch = u16::from_le_bytes([b[p + 10], b[p + 11]]);
            let sr = u32::from_le_bytes([b[p + 12], b[p + 13], b[p + 14], b[p + 15]]);
            bits = u16::from_le_bytes([b[p + 22], b[p + 23]]);
            assert!(ch == 1 && sr == SR, "need 16 kHz mono");
        } else if id == b"data" {
            let d = &b[p + 8..p + 8 + sz.min(b.len() - p - 8)];
            return match (fmt, bits) {
                (1, 16) => d.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
                (3, 32) => d.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
                _ => panic!("unsupported wav format"),
            };
        }
        p += 8 + sz + (sz & 1);
    }
    panic!("no data chunk");
}

fn write_wav_f32(path: &str, s: &[f32]) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).expect("create wav"));
    let n = (s.len() * 4) as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + n).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&3u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&SR.to_le_bytes()).unwrap();
    f.write_all(&(SR * 4).to_le_bytes()).unwrap();
    f.write_all(&4u16.to_le_bytes()).unwrap();
    f.write_all(&32u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&n.to_le_bytes()).unwrap();
    for v in s {
        f.write_all(&v.to_le_bytes()).unwrap();
    }
}

fn cpu_ticks() -> u64 {
    // utime + stime in clock ticks (USER_HZ = 100 on Linux) from /proc/self/stat
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = s.rsplit(')').next().unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    // after ')' : state(0) ppid(1) ... utime is field 14 overall => index 11 here, stime index 12
    let g = |i: usize| f.get(i).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
    g(11) + g(12)
}

fn rss_kb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for l in s.lines() {
        if let Some(v) = l.strip_prefix("VmHWM:") {
            return v.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
        }
    }
    0
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / 8_388_608.0) - 1.0
    }
}

/// Deterministic synthetic signals for CPU benchmarking on a device without any audio files:
/// band-limited, amplitude-modulated noise as "far-end", a 40 ms delayed decaying FIR echo of it,
/// plus low noise as the mic. No speech, no recordings.
fn synthetic(seconds: usize) -> (Vec<f32>, Vec<f32>) {
    let n = seconds * SR as usize;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut lp = 0.0f32;
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        lp = 0.6 * lp + 0.4 * rng.next();
        let env = 0.5 + 0.5 * ((i as f32) * 2.0 * std::f32::consts::PI * 3.0 / SR as f32).sin().abs();
        r.push(0.25 * lp * env);
    }
    let taps = 1600usize;
    let delay = 640usize;
    let h: Vec<f32> = (0..taps).map(|k| rng.next() * 0.3 * (-(k as f32) / 400.0).exp()).collect();
    let mut m = vec![0.0f32; n];
    for i in 0..n {
        let mut acc = 0.0f32;
        for k in 0..taps {
            if i >= delay + k {
                acc += h[k] * r[i - delay - k];
            }
        }
        m[i] = acc + 0.0005 * rng.next();
    }
    (r, m)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mode = a.iter().position(|x| x == "--mode").map(|i| a[i + 1].clone()).unwrap_or_else(|| "aec".into());
    let get = |k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone());
    let (mic, reference, out_path) = if let Some(s) = get("--synthetic") {
        let secs: usize = s.parse().expect("seconds");
        let (r, m) = synthetic(secs);
        (m, r, get("--out"))
    } else {
        (read_wav(&a[1]), read_wav(&a[2]), a.get(3).filter(|x| !x.starts_with("--")).cloned())
    };
    let repeat: usize = get("--repeat").and_then(|x| x.parse().ok()).unwrap_or(1);
    let realtime = a.iter().any(|x| x == "--realtime");
    let n = mic.len().min(reference.len()) / FRAME * FRAME;

    let mut best_rtf = f64::MAX;
    let mut last_out = Vec::new();
    let mut cpu_total = 0u64;
    let mut wall_total = 0.0f64;
    for _ in 0..repeat {
        let mut eng = make(&mode);
        let mut out: Vec<f32> = Vec::with_capacity(n + FRAME);
        let c0 = cpu_ticks();
        let t0 = Instant::now();
        let mut next = t0;
        for f in (0..n).step_by(FRAME) {
            eng.process(&reference[f..f + FRAME], &mic[f..f + FRAME], &mut out);
            if realtime {
                next += Duration::from_millis(10);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
            }
        }
        eng.flush(&mut out);
        let wall = t0.elapsed().as_secs_f64();
        cpu_total += cpu_ticks() - c0;
        wall_total += wall;
        if !realtime {
            best_rtf = best_rtf.min(wall / (n as f64 / SR as f64));
        }
        last_out = out;
    }
    if let Some(p) = out_path {
        let mut o = last_out.clone();
        o.truncate(n);
        write_wav_f32(&p, &o);
    }
    let audio_s = (n as f64 / SR as f64) * repeat as f64;
    eprintln!(
        "RESULT engine={} mode={} audio_s={:.1} wall_s={:.2} rtf_best={:.4} cpu_s={:.2} cpu_pct_of_realtime={:.1} peak_rss_kb={} out_samples={}",
        ENGINE_NAME.replace(" ", "_"),
        mode,
        audio_s,
        wall_total,
        if best_rtf == f64::MAX { f64::NAN } else { best_rtf },
        cpu_total as f64 / 100.0,
        100.0 * (cpu_total as f64 / 100.0) / audio_s,
        rss_kb(),
        last_out.len()
    );
}
