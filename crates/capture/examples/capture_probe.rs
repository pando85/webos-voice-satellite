//! Records a few seconds from the mic (or a raw file) through the front-end,
//! printing level statistics once per second.
//!
//! ```text
//! capture_probe [--seconds N] [--out FILE.s16] [--raw-out FILE.s24] [--file IN.raw]
//!               [--card ID] [--pcm NAME] [--device N] [--discard-ms N]
//!               [--gain-db G] [--hpf-hz F] [--channel left|right|mix]
//! ```
//!
//! `--out` writes 16 kHz mono S16_LE; `--raw-out` writes the captured S24_LE
//! interleaved frames (after the startup discard) in the layout `arecord`
//! produces, for comparison. Output files are never meant to be committed.
//! Exit codes: 0 ok, 2 usage, 3 mic busy, 1 other error.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::ExitCode;

use webos_capture::{AlsaCapture, AudioSource, CaptureConfig, CaptureError, FileSource};
use webos_dsp::{ChannelMode, Frontend, FrontendConfig};

struct Args {
    seconds: u32,
    out: Option<String>,
    raw_out: Option<String>,
    file: Option<String>,
    cfg: CaptureConfig,
    fe: FrontendConfig,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        seconds: 10,
        out: None,
        raw_out: None,
        file: None,
        cfg: CaptureConfig::default(),
        fe: FrontendConfig::default(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--seconds" => a.seconds = val()?.parse().map_err(|e| format!("--seconds: {e}"))?,
            "--out" => a.out = Some(val()?),
            "--raw-out" => a.raw_out = Some(val()?),
            "--file" => a.file = Some(val()?),
            "--card" => a.cfg.card_id = val()?,
            "--pcm" => a.cfg.pcm_name = Some(val()?),
            "--device" => {
                a.cfg.device = Some(val()?.parse().map_err(|e| format!("--device: {e}"))?);
            }
            "--discard-ms" => {
                a.cfg.discard_ms = val()?.parse().map_err(|e| format!("--discard-ms: {e}"))?;
            }
            "--gain-db" => a.fe.gain_db = val()?.parse().map_err(|e| format!("--gain-db: {e}"))?,
            "--hpf-hz" => a.fe.hpf_hz = val()?.parse().map_err(|e| format!("--hpf-hz: {e}"))?,
            "--channel" => {
                a.fe.channel = match val()?.as_str() {
                    "left" => ChannelMode::Left,
                    "right" => ChannelMode::Right,
                    "mix" => ChannelMode::Mix,
                    other => return Err(format!("--channel: unknown mode {other}")),
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn run(a: &Args) -> Result<(), CaptureError> {
    let mut src: Box<dyn AudioSource> = match &a.file {
        Some(path) => Box::new(FileSource::open(path, a.cfg.channels, a.cfg.rate)?.realtime(true)),
        None => Box::new(AlsaCapture::open(&a.cfg)?),
    };
    let (ch, rate) = (src.channels(), src.rate());
    eprintln!(
        "capture_probe: open ok, {ch} ch, {rate} Hz, {} s, gain {} dB, hpf {} Hz, {:?}",
        a.seconds, a.fe.gain_db, a.fe.hpf_hz, a.fe.channel
    );
    let mut fe = Frontend::new(a.fe, rate);
    let mut out = match &a.out {
        Some(p) => Some(BufWriter::new(File::create(p)?)),
        None => None,
    };
    let mut raw_out = match &a.raw_out {
        Some(p) => Some(BufWriter::new(File::create(p)?)),
        None => None,
    };

    let mut buf = vec![0_i32; 512 * usize::from(ch)];
    let mut s16 = Vec::new();
    let total_frames = u64::from(a.seconds) * u64::from(rate);
    let (mut frames, mut next_report, mut sec) = (0_u64, u64::from(rate), 1_u32);
    let mut last_xruns = 0;
    while frames < total_frames {
        let n = src.read(&mut buf)?;
        if n == 0 {
            eprintln!("capture_probe: end of stream");
            break;
        }
        let samples = &buf[..n * usize::from(ch)];
        fe.process(samples, ch, &mut s16);
        if let Some(w) = &mut out {
            for s in &s16 {
                w.write_all(&s.to_le_bytes())?;
            }
        }
        if let Some(w) = &mut raw_out {
            for s in samples {
                w.write_all(&s.to_le_bytes())?;
            }
        }
        frames += n as u64;
        if frames >= next_report {
            let st = fe.take_stats();
            let xr = src.xruns();
            println!(
                "t={sec:>3}s in_peak={:7.1} in_rms={:7.1} out_peak={:7.1} out_rms={:7.1} dBFS clipped={} xruns={xr}{}",
                st.in_peak_dbfs,
                st.in_rms_dbfs,
                st.peak_dbfs,
                st.rms_dbfs,
                st.clipped,
                if xr > last_xruns {
                    " (xrun recovered)"
                } else {
                    ""
                },
            );
            last_xruns = xr;
            sec += 1;
            next_report += u64::from(rate);
        }
    }
    if let Some(w) = &mut out {
        w.flush()?;
    }
    if let Some(w) = &mut raw_out {
        w.flush()?;
    }
    println!("done: frames={frames} xruns={}", src.xruns());
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("capture_probe: {e}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CaptureError::MicBusy) => {
            eprintln!("capture_probe: MicBusy: {}", CaptureError::MicBusy);
            ExitCode::from(3)
        }
        Err(e) => {
            eprintln!("capture_probe: error: {e}");
            ExitCode::FAILURE
        }
    }
}
