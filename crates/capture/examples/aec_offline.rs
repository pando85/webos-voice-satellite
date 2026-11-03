//! Runs the satellite's exact capture front-end, echo canceller included, on
//! a recorded mic + reference pair and writes the 16 kHz mono S16 stream the
//! daemon would send to Home Assistant.
//!
//! ```text
//! aec_offline --mic MIC.raw --ref REF.raw --out OUT.wav
//!             [--mic-channels 2] [--ref-channels 2] [--ref-rate 48000]
//!             [--ref-format s32_le|s24_le] [--skip-ms 250]
//!             [--no-aec] [--delay auto|MS] [--apply predelay|hint|none]
//!             [--ns] [--bypass] [--ref-gain-db G] [--render-channels 1|2]
//!             [--output full|linear|nearend] [--aec3 NAME=VALUE ...]
//!             [--agc2-fixed-db G] [--agc2-adaptive] [--rnnoise]
//!             [--gain-db 24] [--hpf-hz 120] [--channel left|right|mix]
//!             [--timeline]
//! ```
//!
//! Experimental stages (evaluation only, see
//! `docs/experiments/P2-aec-tuning.md`): `--aec3` overrides one AEC3
//! parameter ([`webos_dsp::Aec3Param`] names, repeatable); `--agc2-fixed-db`
//! and `--agc2-adaptive` run sonora's AGC2 after the canceller (combine with
//! `--gain-db 0`); `--rnnoise` runs nnnoiseless (RNNoise) on the S16 output
//! stream (upsampled to 48 kHz and back; its latency is removed here, so
//! the output timing does not change).
//!
//! The reading path is the daemon's file mode: [`FilePair`] (mic S24_LE at
//! 16 kHz, reference S32_LE or S24_LE at 16 or 48 kHz, in lockstep), 256-frame
//! reads, the reference pushed before each mic read, then
//! [`Frontend::process`]. The canceller settings default to the daemon's
//! `[aec]` defaults (delay hint, no NS, no bypass). Prints one JSON object
//! (metrics only, no audio):
//! delay events, the canceller metrics over the whole run (or per second with
//! `--timeline`), clipping, and wall time. Inputs and outputs are local files
//! that must never be committed. Exit codes: 0 ok, 2 usage, 1 error.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::process::ExitCode;
use std::time::Instant;

use webos_capture::{AudioSource, FilePair, FileSource, SampleFormat};
use webos_dsp::{
    AecConfig, AecMetrics, AecOutput, Agc2, ChannelMode, DelayApply, DelayEvent, DelayMode,
    EchoCanceller, Frontend, FrontendConfig,
};

mod rnnoise {
    //! nnnoiseless (RNNoise) on a 16 kHz S16 stream: 3x polyphase upsampling
    //! to 48 kHz, 480-sample RNNoise frames, low-pass and 3x decimation.

    use nnnoiseless::DenoiseState;

    /// Low-pass FIR taps at 48 kHz (odd length, linear phase).
    const TAPS: usize = 121;
    /// Samples (16 kHz) by which the chain delays its input: the two FIRs'
    /// group delay (2 x 60 at 48 kHz) plus RNNoise's one frame (480 at 48 kHz).
    pub(crate) const LATENCY_16K: usize = (2 * (TAPS - 1) / 2 + DenoiseState::FRAME_SIZE) / 3;

    #[allow(clippy::cast_precision_loss)]
    fn lowpass() -> Vec<f32> {
        // Windowed sinc (Blackman), cutoff 7.2 kHz at 48 kHz.
        let fc = 7_200.0_f64 / 48_000.0;
        let m = (TAPS - 1) as f64;
        let h: Vec<f64> = (0..TAPS)
            .map(|i| {
                let x = i as f64 - m / 2.0;
                let sinc = if x == 0.0 {
                    2.0 * fc
                } else {
                    (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
                };
                let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / m).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / m).cos();
                sinc * w
            })
            .collect();
        let sum: f64 = h.iter().sum();
        #[allow(clippy::cast_possible_truncation)]
        h.iter().map(|v| (v / sum) as f32).collect()
    }

    fn fir(h: &[f32], x: &[f32]) -> Vec<f32> {
        (0..x.len())
            .map(|n| {
                h.iter()
                    .enumerate()
                    .take(n + 1)
                    .map(|(k, c)| c * x[n - k])
                    .sum()
            })
            .collect()
    }

    /// Denoises the whole stream; returns it time-aligned with the input
    /// (latency removed, tail zero-padded) and RNNoise's own processing time
    /// in us (without the resampling, which this example does naively).
    #[allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]
    pub(crate) fn run(x: &[i16]) -> (Vec<i16>, u128) {
        let h = lowpass();
        let n = x.len() + LATENCY_16K + 2;
        let mut up = vec![0.0_f32; 3 * n];
        for (i, &v) in x.iter().enumerate() {
            up[3 * i] = 3.0 * f32::from(v);
        }
        let up = fir(&h, &up);
        let mut den = vec![0.0_f32; up.len()];
        let mut st = DenoiseState::new();
        let f = DenoiseState::FRAME_SIZE;
        let mut buf = [0.0_f32; DenoiseState::FRAME_SIZE];
        let t = std::time::Instant::now();
        for (i, o) in up.chunks_exact(f).zip(den.chunks_exact_mut(f)) {
            st.process_frame(&mut buf, i);
            o.copy_from_slice(&buf);
        }
        let us = t.elapsed().as_micros();
        let down = fir(&h, &den);
        let y = (0..x.len())
            .map(|i| {
                let v = down.get(3 * (i + LATENCY_16K)).copied().unwrap_or(0.0);
                v.round().clamp(-32_768.0, 32_767.0) as i16
            })
            .collect();
        (y, us)
    }
}

const READ_FRAMES: usize = 256;

struct Args {
    mic: String,
    reference: String,
    out: String,
    mic_channels: u16,
    ref_channels: u16,
    ref_rate: u32,
    ref_format: SampleFormat,
    skip_ms: u32,
    aec: bool,
    aec_cfg: AecConfig,
    fe: FrontendConfig,
    timeline: bool,
    rnnoise: bool,
}

#[allow(clippy::too_many_lines)] // one arm per flag
fn parse() -> Result<Args, String> {
    let mut a = Args {
        mic: String::new(),
        reference: String::new(),
        out: String::new(),
        mic_channels: 2,
        ref_channels: 2,
        ref_rate: 48_000,
        ref_format: SampleFormat::S32Le,
        skip_ms: 250,
        aec: true,
        aec_cfg: AecConfig::default(),
        fe: FrontendConfig::default(),
        timeline: false,
        rnnoise: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        let num = |v: String, f: &str| -> Result<f32, String> {
            v.parse::<f32>().map_err(|e| format!("{f}: {e}"))
        };
        match flag.as_str() {
            "--mic" => a.mic = val()?,
            "--ref" => a.reference = val()?,
            "--out" => a.out = val()?,
            "--mic-channels" => {
                a.mic_channels = val()?.parse().map_err(|e| format!("{flag}: {e}"))?;
            }
            "--ref-channels" => {
                a.ref_channels = val()?.parse().map_err(|e| format!("{flag}: {e}"))?;
            }
            "--ref-rate" => a.ref_rate = val()?.parse().map_err(|e| format!("{flag}: {e}"))?,
            "--ref-format" => {
                a.ref_format = match val()?.as_str() {
                    "s32_le" => SampleFormat::S32Le,
                    "s24_le" => SampleFormat::S24Le,
                    other => return Err(format!("--ref-format: unknown {other:?}")),
                };
            }
            "--skip-ms" => a.skip_ms = val()?.parse().map_err(|e| format!("{flag}: {e}"))?,
            "--no-aec" => a.aec = false,
            "--delay" => {
                let v = val()?;
                a.aec_cfg.delay = if v == "auto" {
                    DelayMode::Auto
                } else {
                    DelayMode::FixedMs(num(v, "--delay")?)
                };
            }
            "--apply" => {
                a.aec_cfg.apply = match val()?.as_str() {
                    "predelay" => DelayApply::PreDelay,
                    "hint" => DelayApply::Hint,
                    "none" => DelayApply::None,
                    other => return Err(format!("--apply: unknown {other:?}")),
                };
            }
            "--ns" => a.aec_cfg.ns = true,
            "--bypass" => a.aec_cfg.bypass_when_silent = true,
            "--ref-gain-db" => a.aec_cfg.reference_gain_db = num(val()?, "--ref-gain-db")?,
            "--output" => {
                a.aec_cfg.output = match val()?.as_str() {
                    "full" => AecOutput::Full,
                    "linear" => AecOutput::Linear,
                    "nearend" => AecOutput::Nearend,
                    other => return Err(format!("--output: unknown {other:?}")),
                };
            }
            "--render-channels" => {
                a.aec_cfg.render_channels = val()?.parse().map_err(|e| format!("{flag}: {e}"))?;
            }
            "--aec3" => a.aec_cfg.aec3.set_assignment(&val()?)?,
            "--agc2-fixed-db" => {
                let g = num(val()?, flag.as_str())?;
                let adaptive = a.aec_cfg.agc2.is_some_and(|x| x.adaptive);
                a.aec_cfg.agc2 = Some(Agc2 {
                    fixed_gain_db: g,
                    adaptive,
                });
            }
            "--agc2-adaptive" => {
                let fixed_gain_db = a.aec_cfg.agc2.map_or(0.0, |x| x.fixed_gain_db);
                a.aec_cfg.agc2 = Some(Agc2 {
                    fixed_gain_db,
                    adaptive: true,
                });
            }
            "--rnnoise" => a.rnnoise = true,
            "--gain-db" => a.fe.gain_db = num(val()?, "--gain-db")?,
            "--hpf-hz" => a.fe.hpf_hz = num(val()?, "--hpf-hz")?,
            "--channel" => {
                a.fe.channel = match val()?.as_str() {
                    "left" => ChannelMode::Left,
                    "right" => ChannelMode::Right,
                    "mix" => ChannelMode::Mix,
                    other => return Err(format!("--channel: unknown {other:?}")),
                };
            }
            "--timeline" => a.timeline = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if a.mic.is_empty() || a.reference.is_empty() || a.out.is_empty() {
        return Err("--mic, --ref and --out are required".into());
    }
    a.aec_cfg.reference_channels = a.ref_channels;
    a.aec_cfg.reference_rate = a.ref_rate;
    Ok(a)
}

fn wav_header(samples: u32) -> [u8; 44] {
    let data = samples * 2;
    let mut h = [0_u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16_u32.to_le_bytes());
    h[20..22].copy_from_slice(&1_u16.to_le_bytes());
    h[22..24].copy_from_slice(&1_u16.to_le_bytes());
    h[24..28].copy_from_slice(&16_000_u32.to_le_bytes());
    h[28..32].copy_from_slice(&32_000_u32.to_le_bytes());
    h[32..34].copy_from_slice(&2_u16.to_le_bytes());
    h[34..36].copy_from_slice(&16_u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

fn opt(v: Option<f32>) -> String {
    v.map_or_else(|| "null".to_owned(), |x| format!("{x:.2}"))
}

fn metrics_json(m: &AecMetrics) -> String {
    format!(
        "{{\"frames\":{},\"processed\":{},\"bypassed\":{},\"ref_underruns\":{},\"active_frames\":{},\"erle_db\":{},\"sonora_erle_db\":{},\"sonora_delay_ms\":{},\"cpu_us_mean\":{:.2},\"cpu_us_max\":{:.1},\"cpu_pct\":{:.3},\"delay_ms\":{},\"delay_confidence\":{},\"predelay_ms\":{:.2},\"delay_updates\":{}}}",
        m.frames,
        m.processed,
        m.bypassed,
        m.ref_underruns,
        m.active_frames,
        opt(m.erle_db),
        opt(m.sonora_erle_db),
        m.sonora_delay_ms
            .map_or_else(|| "null".to_owned(), |v| v.to_string()),
        m.cpu_us_mean,
        m.cpu_us_max,
        m.cpu_pct,
        opt(m.delay_ms),
        opt(m.delay_confidence),
        m.predelay_ms,
        m.delay_updates
    )
}

#[allow(clippy::too_many_lines)]
fn run(a: &Args) -> Result<(), String> {
    let mic =
        FileSource::open(&a.mic, a.mic_channels, 16_000).map_err(|e| format!("{}: {e}", a.mic))?;
    let reference = FileSource::open(&a.reference, a.ref_channels, a.ref_rate)
        .map_err(|e| format!("{}: {e}", a.reference))?
        .format(a.ref_format);
    let mut src = FilePair::new(mic, reference, false);
    let ch = usize::from(a.mic_channels);
    let mut buf = vec![0_i32; READ_FRAMES * ch];
    let mut refbuf = Vec::new();
    // Skip the startup burst on both, as the daemon does.
    let mut skip = u64::from(a.skip_ms) * 16;
    while skip > 0 {
        let want = usize::try_from(skip)
            .unwrap_or(READ_FRAMES)
            .min(READ_FRAMES);
        let n = src.read(&mut buf[..want * ch]).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        skip -= n as u64;
    }
    src.take_reference(&mut refbuf);
    refbuf.clear();

    let mut fe = Frontend::new(a.fe, 16_000);
    if a.aec {
        let aec = EchoCanceller::new(a.aec_cfg).ok_or("unsupported reference rate or channels")?;
        fe = fe.with_echo_canceller(aec);
    }
    let file = File::create(&a.out).map_err(|e| format!("{}: {e}", a.out))?;
    let mut w = BufWriter::new(file);
    w.write_all(&wav_header(0)).map_err(|e| e.to_string())?;
    let mut mono = Vec::new();
    let mut held: Vec<i16> = Vec::new();
    let mut written: u64 = 0;
    let mut frames_in: u64 = 0;
    let mut events = Vec::new();
    let mut timeline = Vec::new();
    let mut epoch = src.reference_epoch();
    let t0 = Instant::now();
    loop {
        let n = src.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if src.reference_epoch() != epoch {
            epoch = src.reference_epoch();
            fe.reset_reference();
        }
        src.take_reference(&mut refbuf);
        fe.push_reference(&refbuf);
        refbuf.clear();
        fe.process(&buf[..n * ch], a.mic_channels, &mut mono);
        frames_in += n as u64;
        if a.rnnoise {
            held.extend_from_slice(&mono);
        } else {
            for s in &mono {
                w.write_all(&s.to_le_bytes()).map_err(|e| e.to_string())?;
            }
        }
        written += mono.len() as u64;
        #[allow(clippy::cast_precision_loss)]
        let t = written as f64 / 16_000.0;
        match fe.take_aec_event() {
            Some(DelayEvent::Locked {
                delay_ms,
                confidence,
            }) => events.push(format!(
                "{{\"t\":{t:.2},\"event\":\"locked\",\"delay_ms\":{delay_ms:.2},\"confidence\":{confidence:.1}}}"
            )),
            Some(DelayEvent::Changed {
                from_ms,
                to_ms,
                confidence,
            }) => events.push(format!(
                "{{\"t\":{t:.2},\"event\":\"changed\",\"from_ms\":{from_ms:.2},\"to_ms\":{to_ms:.2},\"confidence\":{confidence:.1}}}"
            )),
            None => {}
        }
        if a.timeline && frames_in % 16_000 < n as u64 {
            if let Some(m) = fe.take_aec_metrics() {
                timeline.push(format!("{{\"t\":{t:.2},\"m\":{}}}", metrics_json(&m)));
            }
        }
    }
    let mut rnnoise_us = 0;
    if a.rnnoise {
        let (y, us) = rnnoise::run(&held);
        rnnoise_us = us;
        for s in &y {
            w.write_all(&s.to_le_bytes()).map_err(|e| e.to_string())?;
        }
    }
    let wall = t0.elapsed();
    w.flush().map_err(|e| e.to_string())?;
    let mut file = w.into_inner().map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    file.write_all(&wav_header(u32::try_from(written).unwrap_or(u32::MAX)))
        .map_err(|e| e.to_string())?;
    let stats = fe.take_stats();
    let total = fe
        .take_aec_metrics()
        .map_or_else(|| "null".to_owned(), |m| metrics_json(&m));
    println!(
        "{{\"aec\":{},\"in_samples\":{frames_in},\"out_samples\":{written},\"wall_s\":{:.3},\"rnnoise_us\":{rnnoise_us},\"clipped\":{},\"out_peak_dbfs\":{:.1},\"in_rms_dbfs\":{:.1},\"events\":[{}],\"metrics\":{total},\"timeline\":[{}]}}",
        a.aec,
        wall.as_secs_f64(),
        stats.clipped,
        stats.peak_dbfs,
        stats.in_rms_dbfs,
        events.join(","),
        timeline.join(",")
    );
    Ok(())
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("aec_offline: {e}");
            }
            eprintln!(
                "usage: aec_offline --mic MIC.raw --ref REF.raw --out OUT.wav [options]; see the source header"
            );
            return ExitCode::from(2);
        }
    };
    match run(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("aec_offline: {e}");
            ExitCode::FAILURE
        }
    }
}
