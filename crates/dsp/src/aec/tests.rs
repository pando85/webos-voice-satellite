#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use super::*;

const S24: f32 = 8_388_608.0;

/// Deterministic noise in [-a, a].
fn noise(n: usize, a: f32, seed: u32) -> Vec<f32> {
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * a
        })
        .collect()
}

/// Mono float reference to interleaved stereo S24 frames.
fn to_s24_stereo(x: &[f32]) -> Vec<i32> {
    x.iter()
        .flat_map(|&v| {
            let s = (v * S24) as i32;
            [s, s]
        })
        .collect()
}

/// A simple echo path: direct sound after `delay` samples plus two
/// reflections, scaled by `gain`.
fn echo(reference: &[f32], delay: usize, gain: f32) -> Vec<f32> {
    let taps = [(0_usize, 1.0_f32), (23, -0.4), (160, 0.2)];
    (0..reference.len())
        .map(|i| {
            gain * taps
                .iter()
                .filter(|(d, _)| i >= delay + d)
                .map(|(d, g)| g * reference[i - delay - d])
                .sum::<f32>()
        })
        .collect()
}

fn cfg16() -> AecConfig {
    AecConfig {
        reference_rate: 16_000,
        reference_channels: 2,
        ..AecConfig::default()
    }
}

#[test]
fn defaults_follow_the_measurements() {
    let c = AecConfig::default();
    assert_eq!(
        (c.apply, c.delay, c.ns, c.bypass_when_silent),
        (DelayApply::Hint, DelayMode::Auto, false, false)
    );
    assert_eq!((c.reference_rate, c.reference_channels), (48_000, 2));
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| f64::from(v) * f64::from(v)).sum()
}

/// Runs mic + reference through the stage in daemon-like chunks: 256 mic
/// samples per read, with the matching reference pushed first.
fn run(aec: &mut EchoCanceller, mic: &[f32], reference_16k: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    for (m, r) in mic.chunks(256).zip(reference_16k.chunks(256)) {
        aec.push_reference(&to_s24_stereo(r));
        aec.process(m, &mut out);
    }
    out
}

#[test]
fn sonora_is_transparent_with_a_silent_reference_after_its_latency() {
    // The bypass relies on this: AEC3 without its high-pass filter passes
    // the mic through, delayed by exactly SONORA_LATENCY samples.
    let mut aec = EchoCanceller::new(AecConfig {
        bypass_when_silent: false,
        ..cfg16()
    })
    .unwrap_or_else(|| panic!("config"));
    let mic = noise(16_000, 0.05, 11);
    let out = run(&mut aec, &mic, &vec![0.0; mic.len()]);
    assert_eq!(out.len(), 16_000);
    let max_err = (SONORA_LATENCY..out.len())
        .map(|i| (out[i] - mic[i - SONORA_LATENCY]).abs())
        .fold(0.0_f32, f32::max);
    assert!(max_err < 1e-5, "max error {max_err}");
}

#[test]
fn cancels_a_synthetic_echo_and_finds_its_delay() {
    let n = 16_000 * 8;
    let reference = noise(n, 0.05, 3);
    let delay = 240; // 15 ms
    let near = noise(n, 0.0005, 4); // room noise, -66 dBFS-ish
    let mic: Vec<f32> = echo(&reference, delay, 0.3)
        .iter()
        .zip(&near)
        .map(|(e, v)| e + v)
        .collect();
    let mut aec = EchoCanceller::new(cfg16()).unwrap_or_else(|| panic!("config"));
    let mut events = Vec::new();
    let mut out = Vec::new();
    for (m, r) in mic.chunks(256).zip(reference.chunks(256)) {
        aec.push_reference(&to_s24_stereo(r));
        aec.process(m, &mut out);
        if let Some(e) = aec.take_event() {
            events.push(e);
        }
    }
    let Some(DelayEvent::Locked {
        delay_ms,
        confidence,
    }) = events.first().copied()
    else {
        panic!("no lock: {events:?}");
    };
    assert!((delay_ms - 15.0).abs() < 0.2, "delay {delay_ms}");
    assert!(confidence >= MIN_CONFIDENCE);
    // The default passes the delay as sonora's hint, no pre-delay.
    assert_eq!(aec.predelay, 0);
    assert_eq!(aec.apm.stream_delay_ms(), 15);
    // ERLE over the last 3 s (the output is 128 samples late).
    let tail = 16_000 * 5;
    let e_mic = energy(&mic[tail..n - 256]);
    let e_out = energy(&out[tail + SONORA_LATENCY..n - 256 + SONORA_LATENCY]);
    let erle = 10.0 * (e_mic / e_out).log10();
    assert!(erle > 20.0, "ERLE {erle} dB");
    let m = aec.take_metrics();
    assert!(m.erle_db.is_some_and(|e| e > 10.0), "{m:?}");
    assert_eq!(m.ref_underruns, 0);
    assert!(m.delay_ms.is_some_and(|d| (d - 15.0).abs() < 0.2), "{m:?}");
    assert!(m.cpu_us_mean > 0.0 && m.cpu_pct > 0.0);
}

#[test]
fn reference_at_48k_is_decimated_and_the_delay_reported_at_the_tap() {
    // Build the 48 kHz reference and derive the mic's echo from the same
    // signal at 16 kHz (every third sample of a band-limited source).
    let n16 = 16_000 * 6;
    let base = noise(n16, 0.05, 21);
    // Upsample by sample-and-hold of a smooth version: fine for the
    // delay test (the band 200-4000 Hz dominates).
    let mut ref48 = Vec::with_capacity(n16 * 3 * 2);
    for &v in &base {
        let s = (v * S24) as i32;
        for _ in 0..3 {
            ref48.extend_from_slice(&[s, s]);
        }
    }
    let delay = 256; // 16 ms at the tap
    let mic = echo(&base, delay, 0.3);
    let mut aec = EchoCanceller::new(AecConfig::default()).unwrap_or_else(|| panic!("config"));
    assert!((aec.decimator_delay_ms() - 1.5).abs() < 1e-6);
    let mut out = Vec::new();
    let mut lock = None;
    for (i, m) in mic.chunks(256).enumerate() {
        let r0 = i * 256 * 3 * 2;
        let r1 = (r0 + 256 * 3 * 2).min(ref48.len());
        aec.push_reference(&ref48[r0..r1]);
        aec.process(m, &mut out);
        if let Some(DelayEvent::Locked { delay_ms, .. }) = aec.take_event() {
            lock = Some(delay_ms);
        }
    }
    // Sample-and-hold adds 1/3 sample (about 0.02 ms) of delay.
    let d = lock.unwrap_or_else(|| panic!("no lock"));
    assert!((d - 16.0).abs() < 0.3, "delay {d}");
    assert!(out.len() >= mic.len() - 2 * FRAME);
}

#[test]
fn pre_delay_mode_shifts_the_reference() {
    let n = 16_000 * 6;
    let reference = noise(n, 0.05, 31);
    let mic = echo(&reference, 240, 0.3);
    let mut aec = EchoCanceller::new(AecConfig {
        apply: DelayApply::PreDelay,
        ..cfg16()
    })
    .unwrap_or_else(|| panic!("config"));
    let out = run(&mut aec, &mic, &reference);
    // 240 - 32 margin; the reported delay stays the delay at the tap.
    assert_eq!(aec.predelay, 208);
    assert!(aec.delay_ms().is_some_and(|d| (d - 15.0).abs() < 0.2));
    let tail = 16_000 * 4;
    let erle = 10.0
        * (energy(&mic[tail..n - 256])
            / energy(&out[tail + SONORA_LATENCY..n - 256 + SONORA_LATENCY]))
        .log10();
    assert!(erle > 20.0, "ERLE {erle} dB");
}

#[test]
fn output_does_not_depend_on_chunking() {
    let n = 16_000 * 2;
    let reference = noise(n, 0.05, 5);
    let mic = echo(&reference, 200, 0.3);
    let mut a = EchoCanceller::new(cfg16()).unwrap_or_else(|| panic!("config"));
    let mut b = EchoCanceller::new(cfg16()).unwrap_or_else(|| panic!("config"));
    let out_a = run(&mut a, &mic, &reference);
    // Odd chunks, the reference pushed in other odd chunks ahead of the mic.
    let mut out_b = Vec::new();
    let mut ri = 0;
    for m in mic.chunks(97) {
        let want = (ri + 131).min(reference.len());
        if ri < want {
            b.push_reference(&to_s24_stereo(&reference[ri..want]));
            ri = want;
        }
        b.process(m, &mut out_b);
    }
    let len = out_a.len().min(out_b.len());
    assert!(len > n - 4 * FRAME);
    assert_eq!(out_a[..len], out_b[..len]);
}

#[test]
fn missing_reference_never_blocks_the_mic() {
    let mut aec = EchoCanceller::new(cfg16()).unwrap_or_else(|| panic!("config"));
    let mic = noise(16_000, 0.01, 8);
    let mut out = Vec::new();
    for m in mic.chunks(256) {
        aec.process(m, &mut out);
    }
    // Everything but the wait window comes out, with the reference zero-filled.
    assert!(out.len() >= mic.len() - MAX_WAIT_SAMPLES, "{}", out.len());
    let m = aec.take_metrics();
    assert!(m.ref_underruns > 90, "{m:?}");
}

#[test]
fn bypass_saves_work_and_keeps_the_output_continuous() {
    // Reference: loud for 6 s, silent for 4 s, then very quiet (about -75
    // dBFS: "active" for the bypass, below AEC3's render activity level, so
    // AEC3 stays transparent). Mic: near-end noise only. Then the output
    // must equal the mic delayed by the latency through the bypass and right
    // from the resume: a stale or missing warm-up would show at the switch.
    let n = 16_000 * 12;
    let mut reference = noise(n, 0.05, 9);
    reference[96_000..160_000].fill(0.0);
    for v in &mut reference[160_000..] {
        *v *= 0.005;
    }
    let mic = noise(n, 0.02, 10);
    let bypass = AecConfig {
        bypass_when_silent: true,
        ..cfg16()
    };
    let mut on = EchoCanceller::new(bypass).unwrap_or_else(|| panic!("config"));
    let mut off = EchoCanceller::new(cfg16()).unwrap_or_else(|| panic!("config"));
    let out_on = run(&mut on, &mic, &reference);
    let out_off = run(&mut off, &mic, &reference);
    let m_on = on.take_metrics();
    let m_off = off.take_metrics();
    // 4 s silent minus the 2 s hold: about 200 frames bypassed.
    assert!((190..=202).contains(&m_on.bypassed), "{m_on:?}");
    assert_eq!(m_off.bypassed, 0);
    assert!(m_on.processed < m_off.processed);
    let resid_db = |o: &[f32], lo: usize, hi: usize| {
        let d: Vec<f32> = (lo..hi).map(|i| o[i] - mic[i - SONORA_LATENCY]).collect();
        10.0 * (energy(&d) / energy(&mic[lo - SONORA_LATENCY..hi - SONORA_LATENCY])).log10()
    };
    // The run without bypass is transparent in both quiet stretches, so the
    // comparison is meaningful.
    assert!(resid_db(&out_off, 130_000, 159_000) < -100.0);
    assert!(resid_db(&out_off, 160_400, 180_000) < -60.0);
    // Through the bypass, and from the resume on, the output is the delayed
    // mic (frame by frame around the switch at 160 000).
    assert!(resid_db(&out_on, 130_000, 159_000) < -100.0);
    for f in (159_040..162_000).step_by(FRAME) {
        let r = resid_db(&out_on, f, f + FRAME);
        assert!(r < -60.0, "frame at {f}: {r:.1} dB");
    }
}

#[test]
fn no_bypass_during_the_warm_up() {
    // Silent reference from the start: the bypass waits for the warm-up.
    let mut aec = EchoCanceller::new(AecConfig {
        bypass_when_silent: true,
        ..cfg16()
    })
    .unwrap_or_else(|| panic!("config"));
    let mic = noise(16_000 * 6, 0.01, 16);
    let _ = run(&mut aec, &mic, &vec![0.0; mic.len()]);
    let m = aec.take_metrics();
    // Bypass from frame 500 (5 s) on: about 100 frames of 600.
    assert!((95..=101).contains(&m.bypassed), "{m:?}");
}

#[test]
fn hint_mode_keeps_the_reference_undelayed() {
    let n = 16_000 * 4;
    let reference = noise(n, 0.05, 12);
    let mic = echo(&reference, 300, 0.3);
    let mut aec = EchoCanceller::new(AecConfig {
        apply: DelayApply::Hint,
        ..cfg16()
    })
    .unwrap_or_else(|| panic!("config"));
    let _ = run(&mut aec, &mic, &reference);
    assert_eq!(aec.predelay, 0);
    assert!(aec.delay_ms().is_some_and(|d| (d - 18.75).abs() < 0.2));
    assert_eq!(aec.apm.stream_delay_ms(), 19);
}

#[test]
fn fixed_delay_is_applied_at_once_and_not_replaced() {
    let mut aec = EchoCanceller::new(AecConfig {
        delay: DelayMode::FixedMs(15.0),
        ..cfg16()
    })
    .unwrap_or_else(|| panic!("config"));
    assert_eq!(aec.apm.stream_delay_ms(), 15);
    let n = 16_000 * 3;
    let reference = noise(n, 0.05, 13);
    let mic = echo(&reference, 400, 0.3);
    let _ = run(&mut aec, &mic, &reference);
    // The estimate (25 ms) is reported but the fixed hint stays.
    assert!(aec.delay_ms().is_some_and(|d| (d - 25.0).abs() < 0.2));
    assert_eq!(aec.apm.stream_delay_ms(), 15);
    // Same with a pre-delay.
    let aec = EchoCanceller::new(AecConfig {
        delay: DelayMode::FixedMs(15.0),
        apply: DelayApply::PreDelay,
        ..cfg16()
    })
    .unwrap_or_else(|| panic!("config"));
    assert_eq!(aec.predelay, 240 - PREDELAY_MARGIN);
}

#[test]
fn reset_alignment_reacquires_the_delay() {
    let n = 16_000 * 3;
    let reference = noise(n, 0.05, 14);
    let mic = echo(&reference, 240, 0.3);
    let mut aec = EchoCanceller::new(cfg16()).unwrap_or_else(|| panic!("config"));
    let _ = run(&mut aec, &mic, &reference);
    assert!(aec.delay_ms().is_some_and(|d| (d - 15.0).abs() < 0.2));
    aec.reset_alignment();
    assert!(aec.delay_ms().is_none());
    let mic2 = echo(&reference, 300, 0.3);
    let _ = run(&mut aec, &mic2, &reference);
    assert!(aec.delay_ms().is_some_and(|d| (d - 18.75).abs() < 0.2));
    assert_eq!(aec.apm.stream_delay_ms(), 19);
}

#[test]
fn noise_suppression_disables_the_bypass_and_rejects_bad_configs() {
    let mut aec = EchoCanceller::new(AecConfig {
        ns: true,
        ..cfg16()
    })
    .unwrap_or_else(|| panic!("config"));
    let mic = noise(16_000 * 2, 0.01, 15);
    let _ = run(&mut aec, &mic, &vec![0.0; mic.len()]);
    let m = aec.take_metrics();
    assert_eq!(m.bypassed, 0);
    assert_eq!(aec.latency_samples(), SONORA_LATENCY + 96);
    assert!(
        EchoCanceller::new(AecConfig {
            reference_rate: 44_100,
            ..cfg16()
        })
        .is_none()
    );
    assert!(
        EchoCanceller::new(AecConfig {
            reference_channels: 0,
            ..cfg16()
        })
        .is_none()
    );
}
