//! Capture thread: AudioSource → Frontend → 32 ms chunks to the API server.
//!
//! The thread always drains the ALSA source (so the ring buffer does not
//! overrun) but forwards audio only while [`Shared::streaming`] is set, which
//! the daemon raises on `StartStreaming` and clears on `StopStreaming`, HA
//! disconnect and mute. Chunks go out with `try_send`: when the server is
//! backed up a chunk is dropped and counted rather than stalling capture.
//!
//! Open failures (mic busy because LG hands-free holds the PCM, device not
//! found, I/O errors) are logged and retried with a capped backoff.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use webos_capture::{AudioSource, CaptureError, FileSource};
use webos_dsp::{Frontend, FrontendConfig};
use webos_esphome_api::AppCommand;

use crate::config::{MicSource, Settings};

/// Samples per chunk sent to HA (32 ms at 16 kHz, as ESPHome sends).
pub(crate) const CHUNK_SAMPLES: usize = 512;
/// Frames read from the source per call (one ALSA period on c3-a).
const READ_FRAMES: usize = 256;
/// First retry delay after an open failure.
const RETRY_BASE: Duration = Duration::from_secs(1);
/// Retry delay cap.
const RETRY_MAX: Duration = Duration::from_secs(30);
/// Level statistics are logged at debug this often.
const STATS_EVERY: Duration = Duration::from_secs(60);

/// State shared between the daemon and the capture thread.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Forward audio to HA.
    pub(crate) streaming: AtomicBool,
    /// Stop the thread.
    pub(crate) stop: AtomicBool,
    /// The source is open and delivering audio.
    pub(crate) open: AtomicBool,
    /// Chunks sent to the server.
    pub(crate) chunks_sent: AtomicU64,
    /// Chunks dropped because the server queue was full.
    pub(crate) chunks_dropped: AtomicU64,
    /// Source overruns recovered (ALSA).
    pub(crate) xruns: AtomicU64,
}

/// What the capture thread needs from the configuration.
#[derive(Debug, Clone)]
pub(crate) struct CaptureSettings {
    source: MicSource,
    #[cfg(target_os = "linux")]
    alsa: webos_capture::CaptureConfig,
    path: Option<PathBuf>,
    channels: u16,
    rate: u32,
    realtime: bool,
    looping: bool,
    skip_ms: u32,
    pause_when_idle: bool,
    frontend: FrontendConfig,
}

impl CaptureSettings {
    pub(crate) fn from_settings(s: &Settings) -> Self {
        Self {
            source: s.mic.source,
            #[cfg(target_os = "linux")]
            alsa: s.capture_config(),
            path: s.mic.path.clone(),
            channels: s.mic.channels,
            rate: s.mic.rate,
            realtime: s.mic.realtime,
            looping: s.mic.looping,
            skip_ms: s.mic.skip_ms,
            pause_when_idle: s.mic.pause_when_idle,
            frontend: s.frontend_config(),
        }
    }
}

/// Splits a mono stream into fixed-size chunks.
#[derive(Debug, Default)]
pub(crate) struct Chunker {
    pending: Vec<i16>,
}

impl Chunker {
    /// Appends samples and returns every complete chunk.
    pub(crate) fn push(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.pending.len() >= CHUNK_SAMPLES {
            let rest = self.pending.split_off(CHUNK_SAMPLES);
            out.push(std::mem::replace(&mut self.pending, rest));
        }
        out
    }

    /// Drops buffered samples (when streaming stops or restarts).
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}

/// Sleeps up to `d`, waking early when `stop` is set. Returns false if stopped.
fn sleep_unless_stopped(shared: &Shared, d: Duration) -> bool {
    let end = Instant::now() + d;
    while !shared.stop.load(Ordering::Relaxed) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
    false
}

fn open_source(cfg: &CaptureSettings) -> Result<Box<dyn AudioSource>, CaptureError> {
    match cfg.source {
        MicSource::File => {
            let path = cfg
                .path
                .as_ref()
                .ok_or_else(|| CaptureError::DeviceNotFound("mic.path not set".into()))?;
            // Pacing is done here (with a clock that pauses while idle), not by
            // FileSource, so a paused file does not burst to catch up.
            let mut src = FileSource::open(path, cfg.channels, cfg.rate)?.looping(cfg.looping);
            let skip = u64::from(cfg.skip_ms) * u64::from(cfg.rate) / 1000;
            let mut scratch = vec![0_i32; READ_FRAMES * usize::from(cfg.channels)];
            let mut skipped = 0_u64;
            while skipped < skip {
                let want = usize::try_from(skip - skipped)
                    .unwrap_or(READ_FRAMES)
                    .min(READ_FRAMES);
                let n = src.read(&mut scratch[..want * usize::from(cfg.channels)])?;
                if n == 0 {
                    break;
                }
                skipped += n as u64;
            }
            Ok(Box::new(src))
        }
        #[cfg(target_os = "linux")]
        MicSource::Alsa => Ok(Box::new(webos_capture::AlsaCapture::open(&cfg.alsa)?)),
        #[cfg(not(target_os = "linux"))]
        MicSource::Alsa => Err(CaptureError::Unsupported(
            "ALSA capture is only available on Linux".into(),
        )),
    }
}

/// Starts the capture thread.
pub(crate) fn spawn(
    cfg: CaptureSettings,
    shared: Arc<Shared>,
    tx: mpsc::Sender<AppCommand>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || run(&cfg, &shared, &tx))
}

fn run(cfg: &CaptureSettings, shared: &Shared, tx: &mpsc::Sender<AppCommand>) {
    let mut failures = 0_u32;
    let mut busy_logged = false;
    while !shared.stop.load(Ordering::Relaxed) {
        let src = match open_source(cfg) {
            Ok(src) => src,
            Err(e) => {
                failures = failures.saturating_add(1);
                let delay = RETRY_BASE
                    .saturating_mul(1 << failures.saturating_sub(1).min(5))
                    .min(RETRY_MAX);
                match e {
                    CaptureError::MicBusy if busy_logged => {
                        log::debug!("capture: mic still busy; retry in {} s", delay.as_secs());
                    }
                    CaptureError::MicBusy => {
                        busy_logged = true;
                        log::warn!(
                            "capture: MIC BUSY: another process holds the capture PCM (is LG hands-free voice on?); not streaming, retrying with backoff up to {} s",
                            RETRY_MAX.as_secs()
                        );
                    }
                    other => log::error!(
                        "capture: cannot open the microphone: {other}; retry in {} s",
                        delay.as_secs()
                    ),
                }
                if !sleep_unless_stopped(shared, delay) {
                    break;
                }
                continue;
            }
        };
        if busy_logged {
            log::info!("capture: mic is free again");
        }
        failures = 0;
        busy_logged = false;
        match pump(cfg, src, shared, tx) {
            Ok(()) => break,
            Err(e) => {
                log::error!("capture: stream failed: {e}; reopening");
                if !sleep_unless_stopped(shared, RETRY_BASE) {
                    break;
                }
            }
        }
    }
    shared.open.store(false, Ordering::Relaxed);
    log::debug!("capture: thread stopped");
}

/// Reads until stop (`Ok`) or a stream error (`Err`, caller reopens).
fn pump(
    cfg: &CaptureSettings,
    mut src: Box<dyn AudioSource>,
    shared: &Shared,
    tx: &mpsc::Sender<AppCommand>,
) -> Result<(), CaptureError> {
    let ch = usize::from(src.channels());
    let rate = src.rate();
    let is_file = cfg.source == MicSource::File;
    log::info!(
        "capture: microphone open ({} ch, {rate} Hz, {:?})",
        ch,
        cfg.source
    );
    shared.open.store(true, Ordering::Relaxed);
    let mut frontend = Frontend::new(cfg.frontend, rate);
    let mut buf = vec![0_i32; READ_FRAMES * ch];
    let mut mono = Vec::with_capacity(READ_FRAMES);
    let mut chunker = Chunker::default();
    let mut was_streaming = false;
    // File pacing clock: restarted whenever reading resumes.
    let mut pace_start = Instant::now();
    let mut paced_frames = 0_u64;
    let mut stats_at = Instant::now();
    let mut eof = false;

    while !shared.stop.load(Ordering::Relaxed) {
        let streaming = shared.streaming.load(Ordering::Relaxed);
        if streaming != was_streaming {
            chunker.clear();
            was_streaming = streaming;
            pace_start = Instant::now();
            paced_frames = 0;
        }
        if is_file && (eof || (cfg.pause_when_idle && !streaming)) {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let n = src.read(&mut buf)?;
        if n == 0 {
            log::info!("capture: end of the mic file; no more audio");
            eof = true;
            continue;
        }
        if is_file && cfg.realtime {
            paced_frames += n as u64;
            let due = Duration::from_micros(paced_frames * 1_000_000 / u64::from(rate));
            if let Some(wait) = due.checked_sub(pace_start.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        frontend.process(&buf[..n * ch], src.channels(), &mut mono);
        if streaming {
            for chunk in chunker.push(&mono) {
                match tx.try_send(AppCommand::Audio(chunk)) {
                    Ok(()) => {
                        shared.chunks_sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        shared.chunks_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                }
            }
        }
        shared.xruns.store(src.xruns(), Ordering::Relaxed);
        if stats_at.elapsed() >= STATS_EVERY {
            stats_at = Instant::now();
            let st = frontend.take_stats();
            // Levels only; never audio content.
            log::debug!(
                "capture: last {} s: in rms {:.1} dBFS peak {:.1} dBFS, out rms {:.1} dBFS peak {:.1} dBFS, clipped {}, xruns {}, chunks sent {} dropped {}",
                STATS_EVERY.as_secs(),
                st.in_rms_dbfs,
                st.in_peak_dbfs,
                st.rms_dbfs,
                st.peak_dbfs,
                st.clipped,
                src.xruns(),
                shared.chunks_sent.load(Ordering::Relaxed),
                shared.chunks_dropped.load(Ordering::Relaxed),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn chunker_emits_fixed_chunks() {
        let mut c = Chunker::default();
        assert!(c.push(&[1; 300]).is_empty());
        let out = c.push(&[2; 800]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|ch| ch.len() == CHUNK_SAMPLES));
        assert_eq!(out[0][299], 1);
        assert_eq!(out[0][300], 2);
        c.clear();
        assert!(c.push(&[3; 511]).is_empty());
        assert_eq!(c.push(&[3; 1]).len(), 1);
    }

    fn write_raw(path: &std::path::Path, frames: usize, value: i32) {
        let mut bytes = Vec::new();
        for _ in 0..frames {
            for _ in 0..2 {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn file_source_streams_only_while_streaming() {
        let dir = std::env::temp_dir().join(format!("webos-sat-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mic.raw");
        // 0.5 s of a constant (filtered away by the HPF, but samples flow).
        write_raw(&path, 8000, 1000);
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(path);
        s.mic.realtime = false;
        s.mic.skip_ms = 250;
        let cfg = CaptureSettings::from_settings(&s);
        let shared = Arc::new(Shared::default());
        let (tx, mut rx) = mpsc::channel(1024);
        let h = spawn(cfg, Arc::clone(&shared), tx).unwrap();

        // Paused while not streaming: nothing arrives.
        std::thread::sleep(Duration::from_millis(100));
        assert!(rx.try_recv().is_err());

        shared.streaming.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let mut samples = 0;
        while let Ok(AppCommand::Audio(chunk)) = rx.try_recv() {
            assert_eq!(chunk.len(), CHUNK_SAMPLES);
            samples += chunk.len();
        }
        // 8000 frames - 4000 skipped = 4000 samples = 7 whole chunks.
        assert_eq!(samples, 7 * CHUNK_SAMPLES);
        assert_eq!(shared.chunks_sent.load(Ordering::Relaxed), 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_retried_until_stop() {
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(PathBuf::from("/nonexistent/mic.raw"));
        let shared = Arc::new(Shared::default());
        let (tx, _rx) = mpsc::channel(4);
        let h = spawn(CaptureSettings::from_settings(&s), Arc::clone(&shared), tx).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!shared.open.load(Ordering::Relaxed));
        let t = Instant::now();
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(t.elapsed() < Duration::from_millis(500));
    }
}
