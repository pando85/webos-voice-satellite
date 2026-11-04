//! Capture thread: AudioSource → Frontend → 32 ms chunks to the API server.
//!
//! With `[aec] enabled`, the source also captures the echo reference (ALSA:
//! [`webos_capture::AlsaDuplex`], the reference PCM on the mic's card; file
//! mode: [`FilePair`] with `aec.reference_path`), and the front-end runs the
//! echo canceller between its high-pass filter and its gain. A reference
//! that cannot be opened or fails is logged and the mic runs without echo
//! cancellation; it is retried at the next open of the microphone.
//!
//! The thread always drains the ALSA source (so the ring buffer does not
//! overrun) but forwards audio only while [`Shared::streaming`] is set, which
//! the daemon raises on `StartStreaming` and clears on `StopStreaming`, HA
//! disconnect and mute. Chunks go out with `try_send`: when the server is
//! backed up a chunk is dropped and counted rather than stalling capture.
//!
//! The PCM is open only while the microphone is wanted: [`Shared::mic_off`]
//! (HA's mute switch, the kill switch, the firmware guard or "not paired",
//! set by the daemon) or the kill-switch file itself (checked here every 250 ms, so the
//! hardware is released even if the daemon's event loop is stuck) closes it
//! within one read (16 ms at the c3-a settings), and it is reopened when the
//! microphone is wanted again.
//!
//! Listening indicator (privacy gate F7): the thread itself publishes two HA
//! binary sensors, [`Indicators`], from what it actually does with the PCM, in
//! the same command stream as the audio. "Listening" goes ON right after the
//! PCM is opened and before any audio is read or forwarded, and OFF only after
//! the PCM is closed again; so no audio message can precede its ON, and OFF
//! never claims a closed mic while the PCM is open. "Microphone muted" is ON
//! only while the PCM is closed and a gate (mute switch, kill switch,
//! firmware guard, not paired) is what keeps it closed.
//!
//! Open failures (mic busy because LG hands-free holds the PCM, device not
//! found, I/O errors) are logged and retried with a capped backoff. A device
//! that stops delivering audio for `limits.capture_stall_s` is closed and
//! reopened.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use webos_capture::{AudioSource, CaptureError, FilePair, FileSource, ReferenceState};
use webos_compat::pcm::WantedPcm;
use webos_compat::window::FrameStats;
use webos_dsp::{AecConfig, AecMetrics, DelayEvent, EchoCanceller, Frontend, FrontendConfig};
use webos_esphome_api::AppCommand;
use webos_esphome_api::config::BinarySensorEntity;
use webos_esphome_api::proto::EntityCategory;

use crate::config::{MicSource, Settings};
use crate::selfcheck::{Glue, Note, SAMPLE_BITS, SelfCheckSettings};

/// Samples per chunk sent to HA (32 ms at 16 kHz, as ESPHome sends).
pub(crate) const CHUNK_SAMPLES: usize = 512;
/// Frames read from the source per call (one ALSA period on c3-a).
const READ_FRAMES: usize = 256;
/// First retry delay after an open failure.
const RETRY_BASE: Duration = Duration::from_secs(1);
/// Retry delay cap.
const RETRY_MAX: Duration = Duration::from_secs(30);
/// The kill-switch file is checked this often.
pub(crate) const KILL_SWITCH_POLL: Duration = Duration::from_millis(250);
/// A stream that ran this long resets the stream-error backoff.
const STREAM_OK_AFTER: Duration = Duration::from_secs(60);
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
    /// Keep the microphone closed: one of the `OFF_*` reasons, or 0.
    pub(crate) mic_off: AtomicU8,
    /// Bumped on every turn of the capture loop (watchdog heartbeat).
    pub(crate) beat: AtomicU64,
    /// Times the PCM was closed because the microphone was not wanted.
    pub(crate) releases: AtomicU64,
    /// Test aid (`--test-hang capture`): freeze the thread.
    pub(crate) test_hang: AtomicBool,
    /// Re-run the microphone self-check (HA button, `ctl.sh selfcheck`, a
    /// firmware change): set by the daemon, taken by the capture thread.
    pub(crate) selfcheck_rerun: AtomicBool,
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
    kill_switch: Option<PathBuf>,
    aec: Option<AecSettings>,
    /// The microphone self-check (`[safety] self_check`); `None`: off.
    pub(crate) selfcheck: Option<SelfCheckSettings>,
}

/// Echo canceller part of [`CaptureSettings`] (`[aec] enabled`).
#[derive(Debug, Clone)]
struct AecSettings {
    dsp: AecConfig,
    #[cfg(target_os = "linux")]
    reference: webos_capture::ReferenceConfig,
    pcm_id: String,
    path: Option<PathBuf>,
    format: webos_capture::SampleFormat,
}

impl CaptureSettings {
    pub(crate) fn from_settings(s: &Settings) -> Self {
        let aec = s.aec.enabled.then(|| AecSettings {
            dsp: s.aec_config(),
            #[cfg(target_os = "linux")]
            reference: s.reference_config(),
            pcm_id: s.aec.reference_pcm_id.clone(),
            path: s.aec.reference_path.clone(),
            format: s.reference_format(),
        });
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
            kill_switch: Some(s.safety.kill_switch.clone()),
            aec,
            // A file mic that pauses while idle delivers no frames until HA
            // streams, which the check would read as "no audio": not checked.
            selfcheck: (s.safety.self_check
                && (s.mic.source == MicSource::Alsa || !s.mic.pause_when_idle))
                .then(|| SelfCheckSettings {
                    window: Duration::from_secs(u64::from(s.safety.self_check_window_s)),
                    rate: s.mic.rate,
                    wanted: (s.mic.source == MicSource::Alsa).then(|| WantedPcm {
                        card_id: s.mic.card_id.clone(),
                        device: s.mic.device,
                    }),
                    pcm_name: s.mic.pcm_name.clone(),
                    asound: PathBuf::from("/proc/asound"),
                }),
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

/// [`Shared::mic_off`]: the microphone may be open.
pub(crate) const OFF_NONE: u8 = 0;
/// [`Shared::mic_off`]: muted by HA's switch.
pub(crate) const OFF_MUTED: u8 = 1;
/// [`Shared::mic_off`]: the kill switch.
pub(crate) const OFF_KILL: u8 = 2;
/// [`Shared::mic_off`]: the firmware guard.
pub(crate) const OFF_FIRMWARE: u8 = 3;
/// [`Shared::mic_off`]: pairing enabled and no key yet (no HA may listen).
pub(crate) const OFF_UNPAIRED: u8 = 4;

/// Object id of the "Listening" binary sensor.
pub(crate) const LISTENING_ID: &str = "listening";
/// Object id of the "Microphone muted" binary sensor.
pub(crate) const MUTED_ID: &str = "microphone_muted";

/// The two indicator entities, always listed (they are a privacy feature,
/// not an option).
pub(crate) fn indicator_entities() -> Vec<BinarySensorEntity> {
    vec![
        BinarySensorEntity {
            object_id: LISTENING_ID.to_owned(),
            name: "Listening".to_owned(),
            icon: "mdi:ear-hearing".to_owned(),
            device_class: String::new(),
            entity_category: EntityCategory::None,
        },
        BinarySensorEntity {
            object_id: MUTED_ID.to_owned(),
            name: "Microphone muted".to_owned(),
            icon: "mdi:microphone-off".to_owned(),
            device_class: String::new(),
            entity_category: EntityCategory::None,
        },
    ]
}

/// Publishes the indicator sensors, sending only changes. Commands go in
/// order with the audio chunks on the same channel and are never dropped
/// while the server is alive (unlike audio, which is dropped when the
/// server is backed up).
struct Indicators<'a> {
    tx: &'a mpsc::Sender<AppCommand>,
    shared: &'a Shared,
    listening: Option<bool>,
    muted: Option<bool>,
}

impl<'a> Indicators<'a> {
    fn new(tx: &'a mpsc::Sender<AppCommand>, shared: &'a Shared) -> Self {
        Self {
            tx,
            shared,
            listening: None,
            muted: None,
        }
    }

    /// "Listening": the capture PCM is open. Call after opening it and after
    /// closing it, nowhere else.
    fn listening(&mut self, on: bool) {
        if self.listening != Some(on) {
            self.listening = Some(on);
            self.publish(LISTENING_ID, on);
        }
    }

    /// "Microphone muted": a gate keeps the (closed) PCM closed. Call only
    /// while the PCM is closed.
    fn muted(&mut self, on: bool) {
        if self.muted != Some(on) {
            self.muted = Some(on);
            self.publish(MUTED_ID, on);
        }
    }

    fn publish(&self, object_id: &str, on: bool) {
        let mut cmd = AppCommand::SetBinarySensor {
            object_id: object_id.to_owned(),
            state: Some(on),
        };
        loop {
            match self.tx.try_send(cmd) {
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(back)) => {
                    // The server is backed up: wait (the heartbeat keeps
                    // beating) rather than lose a state change.
                    cmd = back;
                    if self.shared.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    self.shared.beat.fetch_add(1, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}

/// Cached check of the kill-switch file (one `stat` per 250 ms at most).
#[derive(Debug)]
struct KillSwitch<'a> {
    path: Option<&'a Path>,
    checked: Option<Instant>,
    active: bool,
}

impl<'a> KillSwitch<'a> {
    fn new(path: Option<&'a Path>) -> Self {
        Self {
            path,
            checked: None,
            active: false,
        }
    }

    fn active(&mut self) -> bool {
        let Some(path) = self.path else {
            return false;
        };
        if self.checked.is_none_or(|t| t.elapsed() >= KILL_SWITCH_POLL) {
            self.checked = Some(Instant::now());
            self.active = path.exists();
        }
        self.active
    }
}

/// Why the microphone is not wanted, or `None` if it is.
fn unwanted(shared: &Shared, kill: &mut KillSwitch<'_>) -> Option<&'static str> {
    if kill.active() {
        return Some("kill switch");
    }
    match shared.mic_off.load(Ordering::Relaxed) {
        OFF_NONE => None,
        OFF_MUTED => Some("muted"),
        OFF_KILL => Some("kill switch"),
        OFF_FIRMWARE => Some("firmware guard"),
        OFF_UNPAIRED => Some("not paired"),
        _ => Some("disabled"),
    }
}

/// Sleeps up to `d`, waking early when `stop` is set. Returns false if stopped.
fn sleep_unless_stopped(shared: &Shared, d: Duration) -> bool {
    let end = Instant::now() + d;
    while !shared.stop.load(Ordering::Relaxed) {
        shared.beat.fetch_add(1, Ordering::Relaxed);
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
            let mic = FileSource::open(path, cfg.channels, cfg.rate)?;
            let mut src: Box<dyn AudioSource> = match cfg
                .aec
                .as_ref()
                .and_then(|a| a.path.as_ref().map(|p| (a, p)))
            {
                Some((a, ref_path)) => {
                    match FileSource::open(ref_path, a.dsp.reference_channels, a.dsp.reference_rate)
                    {
                        Ok(reference) => {
                            Box::new(FilePair::new(mic, reference.format(a.format), cfg.looping))
                        }
                        // Like an ALSA reference that cannot be opened: the
                        // mic runs alone.
                        Err(e) => {
                            log::warn!("aec: reference file {}: {e}", ref_path.display());
                            Box::new(mic.looping(cfg.looping))
                        }
                    }
                }
                None => Box::new(mic.looping(cfg.looping)),
            };
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
            // The reference skipped along with the mic is dropped too.
            src.take_reference(&mut Vec::new());
            Ok(src)
        }
        #[cfg(target_os = "linux")]
        MicSource::Alsa => match &cfg.aec {
            Some(a) => Ok(Box::new(webos_capture::AlsaDuplex::open(
                &cfg.alsa,
                &a.reference,
            )?)),
            None => Ok(Box::new(webos_capture::AlsaCapture::open(&cfg.alsa)?)),
        },
        #[cfg(not(target_os = "linux"))]
        MicSource::Alsa => Err(CaptureError::Unsupported(
            "ALSA capture is only available on Linux".into(),
        )),
    }
}

/// Builds the front-end for a freshly opened source: with the echo canceller
/// when `[aec]` is enabled and the source delivers a reference, otherwise
/// without (and says why).
fn build_frontend(cfg: &CaptureSettings, src: &dyn AudioSource, rate: u32) -> Frontend {
    let fe = Frontend::new(cfg.frontend, rate);
    let Some(a) = &cfg.aec else {
        return fe;
    };
    match src.reference_state() {
        ReferenceState::Active {
            linked,
            unlinked_reason,
        } => {
            let Some(fmt) = src.reference_format() else {
                log::warn!(
                    "aec: the source reports no reference format; running WITHOUT echo cancellation"
                );
                return fe;
            };
            // A stereo render needs two reference channels; with fewer the
            // canceller falls back to the downmix.
            let dsp = AecConfig {
                reference_channels: fmt.channels,
                reference_rate: fmt.rate,
                render_channels: a.dsp.render_channels.min(fmt.channels).max(1),
                ..a.dsp
            };
            let Some(aec) = EchoCanceller::new(dsp) else {
                log::warn!(
                    "aec: unsupported reference ({} ch, {} Hz; needs 16000 or 48000 Hz); running WITHOUT echo cancellation",
                    fmt.channels,
                    fmt.rate
                );
                return fe;
            };
            let start = if linked {
                "linked start".to_owned()
            } else {
                format!(
                    "NOT linked ({}), started reference first",
                    unlinked_reason.as_deref().unwrap_or("unknown")
                )
            };
            let what = match (&cfg.source, &a.path) {
                (MicSource::File, Some(p)) => format!("file {}", p.display()),
                _ => format!("PCM {:?}", a.pcm_id),
            };
            log::info!(
                "aec: echo cancellation on: reference {what} ({} ch, {} Hz, {}), render {} ch, output {:?}, {start}, delay {}, ns={}, bypass_when_silent={}, aec3=[{}]",
                fmt.channels,
                fmt.rate,
                a.format.name(),
                aec.config().render_channels,
                aec.config().output,
                match a.dsp.delay {
                    webos_dsp::DelayMode::Auto => "auto".to_owned(),
                    webos_dsp::DelayMode::FixedMs(ms) => format!("{ms} ms"),
                },
                a.dsp.ns,
                a.dsp.bypass_when_silent,
                a.dsp.aec3
            );
            fe.with_echo_canceller(aec)
        }
        ReferenceState::Unavailable(why) => {
            log::warn!(
                "aec: echo reference {:?} unavailable: {why}; running WITHOUT echo cancellation (retried at the next microphone open)",
                a.pcm_id
            );
            fe
        }
        ReferenceState::Off => {
            log::warn!("aec: no echo reference on this source; running WITHOUT echo cancellation");
            fe
        }
    }
}

/// Moves the reference frames captured with each mic read into the
/// front-end, restarts the alignment when the source restarted both
/// streams, and drops the canceller if the reference is lost.
#[derive(Debug)]
struct ReferenceFeed {
    on: bool,
    epoch: u64,
    buf: Vec<i32>,
}

impl ReferenceFeed {
    fn new(src: &dyn AudioSource, frontend: &Frontend) -> Self {
        Self {
            on: frontend.has_echo_canceller(),
            epoch: src.reference_epoch(),
            buf: Vec::new(),
        }
    }

    /// Call after each mic read, before processing it.
    fn feed(
        &mut self,
        src: &mut dyn AudioSource,
        frontend: &mut Frontend,
        fe_cfg: FrontendConfig,
        rate: u32,
    ) {
        if !self.on {
            return;
        }
        if let ReferenceState::Unavailable(why) = src.reference_state() {
            log::warn!(
                "aec: echo reference lost: {why}; continuing WITHOUT echo cancellation until the microphone reopens"
            );
            *frontend = Frontend::new(fe_cfg, rate);
            self.on = false;
            return;
        }
        let e = src.reference_epoch();
        if e != self.epoch {
            self.epoch = e;
            frontend.reset_reference();
        }
        src.take_reference(&mut self.buf);
        frontend.push_reference(&self.buf);
        self.buf.clear();
    }
}

/// Logs a delay lock or change of the canceller at info.
fn log_aec_event(frontend: &mut Frontend) {
    match frontend.take_aec_event() {
        Some(DelayEvent::Locked {
            delay_ms,
            confidence,
        }) => log::info!(
            "aec: echo delay {delay_ms:.1} ms (mic behind reference, confidence {confidence:.0})"
        ),
        Some(DelayEvent::Changed {
            from_ms,
            to_ms,
            confidence,
        }) => log::info!(
            "aec: echo delay changed {from_ms:.1} -> {to_ms:.1} ms (confidence {confidence:.0})"
        ),
        None => {}
    }
}

/// One log line of canceller metrics (levels and timing only, never audio).
fn log_aec_metrics(m: &AecMetrics, window_s: u64) {
    let f = |v: Option<f32>| v.map_or_else(|| "n/a".to_owned(), |x| format!("{x:.1}"));
    log::debug!(
        "aec: last {window_s} s: delay {} ms (confidence {}), ERLE {} dB (sonora {} dB) over {} active-reference frames, sonora {:.0} us/frame mean {:.0} max ({:.2} % of a core), bypassed {}/{} frames, reference underruns {}",
        f(m.delay_ms),
        f(m.delay_confidence),
        f(m.erle_db),
        f(m.sonora_erle_db),
        m.active_frames,
        m.cpu_us_mean,
        m.cpu_us_max,
        m.cpu_pct,
        m.bypassed,
        m.frames,
        m.ref_underruns
    );
}

/// Starts the capture thread. `notes` receives the self-check results
/// (without a receiver the check still runs and closes the mic on a failure).
pub(crate) fn spawn(
    cfg: CaptureSettings,
    shared: Arc<Shared>,
    tx: mpsc::Sender<AppCommand>,
    notes: Option<mpsc::UnboundedSender<Note>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            let glue = cfg
                .selfcheck
                .clone()
                .map(|c| Glue::new(c, notes.unwrap_or_else(|| mpsc::unbounded_channel().0)));
            run(&cfg, &shared, &tx, glue);
        })
}

/// How [`pump`] ended without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PumpEnd {
    /// The thread is stopping.
    Stop,
    /// The microphone is not wanted any more (the reason).
    Released(&'static str),
    /// The self-check measured a failure and closed the microphone.
    CheckClosed,
}

fn backoff(n: u32) -> Duration {
    RETRY_BASE
        .saturating_mul(1 << n.saturating_sub(1).min(5))
        .min(RETRY_MAX)
}

#[allow(clippy::too_many_lines)] // one loop: gates, self-check, open, pump, errors
fn run(
    cfg: &CaptureSettings,
    shared: &Shared,
    tx: &mpsc::Sender<AppCommand>,
    mut glue: Option<Glue>,
) {
    let mut failures = 0_u32;
    let mut stream_errors = 0_u32;
    let mut busy_logged = false;
    let mut kill = KillSwitch::new(cfg.kill_switch.as_deref());
    let mut idle_reason: Option<&'static str> = None;
    let mut ind = Indicators::new(tx, shared);
    while !shared.stop.load(Ordering::Relaxed) {
        let rerun = glue.as_mut().is_some_and(|g| g.poll_rerun(shared, false));
        if let Some(why) = unwanted(shared, &mut kill) {
            if let Some(g) = glue.as_mut() {
                g.mic_closed();
                if rerun {
                    g.not_run();
                }
            }
            // Not inside `pump`, so the PCM is closed here.
            ind.listening(false);
            ind.muted(true);
            if idle_reason != Some(why) {
                log::info!("capture: microphone off ({why})");
                idle_reason = Some(why);
            }
            if !sleep_unless_stopped(shared, Duration::from_millis(20)) {
                break;
            }
            continue;
        }
        idle_reason = None;
        if let Some(g) = glue.as_mut() {
            // The self-check holds the open while it waits for a retry or
            // after the retries are used up: the microphone is closed because
            // of a capture problem, not because a gate wants it off.
            if !g.may_open() {
                ind.listening(false);
                ind.muted(true);
                if !sleep_unless_stopped(shared, Duration::from_millis(20)) {
                    break;
                }
                continue;
            }
            if cfg.source == MicSource::Alsa && g.check_pcm(shared) {
                continue;
            }
        }
        // Wanted, so no gate holds the mic closed (it is closed right now).
        ind.listening(false);
        ind.muted(false);
        let src = match open_source(cfg) {
            Ok(src) => src,
            Err(e) => {
                if let Some(g) = glue.as_mut() {
                    g.open_failed(matches!(e, CaptureError::MicBusy), shared);
                }
                failures = failures.saturating_add(1);
                let delay = backoff(failures);
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
                if !sleep_unless_wanted_or_stopped(shared, &mut kill, delay) {
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
        let opened = Instant::now();
        let end = pump(cfg, src, shared, tx, &mut kill, &mut ind, &mut glue);
        // `pump` consumed and dropped the source: the PCM is closed now.
        shared.open.store(false, Ordering::Relaxed);
        ind.listening(false);
        if opened.elapsed() >= STREAM_OK_AFTER {
            stream_errors = 0;
        }
        if let Some(g) = glue.as_mut() {
            // A stall is the "no frames" case of the check; anything but a
            // measured failure leaves a run in progress abandoned.
            if matches!(end, Err(CaptureError::Stalled(_))) {
                let _ = g.on_tick(shared);
            }
            if !matches!(end, Ok(PumpEnd::CheckClosed)) {
                g.mic_closed();
            }
        }
        match end {
            Ok(PumpEnd::Stop) => break,
            Ok(PumpEnd::CheckClosed) => {
                // The self-check logged why; the loop holds the open.
            }
            Ok(PumpEnd::Released(why)) => {
                shared.releases.fetch_add(1, Ordering::Relaxed);
                log::info!("capture: microphone closed, PCM released ({why})");
                idle_reason = Some(why);
            }
            Err(e) => {
                stream_errors = stream_errors.saturating_add(1);
                let delay = backoff(stream_errors);
                match e {
                    CaptureError::Stalled(ms) => log::warn!(
                        "capture: no audio from the device for {ms} ms; closed it, reopening in {} s",
                        delay.as_secs()
                    ),
                    other => log::error!(
                        "capture: stream failed: {other}; reopening in {} s",
                        delay.as_secs()
                    ),
                }
                if !sleep_unless_wanted_or_stopped(shared, &mut kill, delay) {
                    break;
                }
            }
        }
    }
    shared.open.store(false, Ordering::Relaxed);
    ind.listening(false);
    log::debug!("capture: thread stopped");
}

/// Sleeps up to `d` (a retry delay); returns early (true) when the
/// microphone stops being wanted, so a mute during a retry wait is logged at
/// once. Returns false if stopped.
fn sleep_unless_wanted_or_stopped(shared: &Shared, kill: &mut KillSwitch<'_>, d: Duration) -> bool {
    let end = Instant::now() + d;
    loop {
        if unwanted(shared, kill).is_some() {
            return !shared.stop.load(Ordering::Relaxed);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return !shared.stop.load(Ordering::Relaxed);
        }
        if !sleep_unless_stopped(shared, left.min(Duration::from_millis(100))) {
            return false;
        }
    }
}

#[allow(clippy::too_many_lines)] // one read loop with the self-check and the pacing
/// Reads until stop or release (`Ok`) or a stream error (`Err`, caller
/// reopens). The source is dropped (PCM closed) when this returns.
fn pump(
    cfg: &CaptureSettings,
    mut src: Box<dyn AudioSource>,
    shared: &Shared,
    tx: &mpsc::Sender<AppCommand>,
    kill: &mut KillSwitch<'_>,
    ind: &mut Indicators<'_>,
    glue: &mut Option<Glue>,
) -> Result<PumpEnd, CaptureError> {
    let ch = usize::from(src.channels());
    let rate = src.rate();
    let is_file = cfg.source == MicSource::File;
    log::info!(
        "capture: microphone open ({} ch, {rate} Hz, {:?})",
        ch,
        cfg.source
    );
    shared.open.store(true, Ordering::Relaxed);
    // Before the first read: no audio can reach HA ahead of this ON.
    ind.listening(true);
    let mut frontend = build_frontend(cfg, src.as_ref(), rate);
    let mut feed = ReferenceFeed::new(src.as_ref(), &frontend);
    if let Some(g) = glue.as_mut() {
        let reference = cfg
            .aec
            .is_some()
            .then(|| matches!(src.reference_state(), ReferenceState::Active { .. }));
        g.opened(src.channels(), rate, reference);
    }
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
        shared.beat.fetch_add(1, Ordering::Relaxed);
        while shared.test_hang.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
        }
        if let Some(why) = unwanted(shared, kill) {
            return Ok(PumpEnd::Released(why));
        }
        if let Some(g) = glue.as_mut() {
            // A rerun on the open stream measures from now, no second open.
            g.poll_rerun(shared, true);
        }
        let streaming = shared.streaming.load(Ordering::Relaxed);
        if streaming != was_streaming {
            chunker.clear();
            was_streaming = streaming;
            pace_start = Instant::now();
            paced_frames = 0;
        }
        if is_file && (eof || (cfg.pause_when_idle && !streaming)) {
            if glue.as_mut().is_some_and(|g| g.on_tick(shared)) {
                return Ok(PumpEnd::CheckClosed);
            }
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
        if let Some(g) = glue.as_mut() {
            // Levels of what was read, before the front-end; the samples are
            // not kept.
            let stats = FrameStats::from_samples(
                &buf[..n * ch],
                u32::try_from(ch).unwrap_or(1),
                SAMPLE_BITS,
            );
            if g.on_frame(&stats, shared) {
                return Ok(PumpEnd::CheckClosed);
            }
        }
        feed.feed(src.as_mut(), &mut frontend, cfg.frontend, rate);
        frontend.process(&buf[..n * ch], src.channels(), &mut mono);
        log_aec_event(&mut frontend);
        if streaming {
            for chunk in chunker.push(&mono) {
                match tx.try_send(AppCommand::Audio(chunk)) {
                    Ok(()) => {
                        shared.chunks_sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        shared.chunks_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return Ok(PumpEnd::Stop),
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
            if let Some(m) = frontend.take_aec_metrics() {
                log_aec_metrics(&m, STATS_EVERY.as_secs());
            }
        }
    }
    Ok(PumpEnd::Stop)
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
        let h = spawn(cfg, Arc::clone(&shared), tx, None).unwrap();

        // Paused while not streaming: no audio arrives (the indicators do).
        std::thread::sleep(Duration::from_millis(100));
        while let Ok(cmd) = rx.try_recv() {
            assert!(matches!(cmd, AppCommand::SetBinarySensor { .. }), "{cmd:?}");
        }

        shared.streaming.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let mut samples = 0;
        while let Ok(cmd) = rx.try_recv() {
            if let AppCommand::Audio(chunk) = cmd {
                assert_eq!(chunk.len(), CHUNK_SAMPLES);
                samples += chunk.len();
            }
        }
        // 8000 frames - 4000 skipped = 4000 samples = 7 whole chunks.
        assert_eq!(samples, 7 * CHUNK_SAMPLES);
        assert_eq!(shared.chunks_sent.load(Ordering::Relaxed), 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn wait_for(f: impl Fn() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn mute_and_kill_switch_close_and_reopen_the_source() {
        let dir = std::env::temp_dir().join(format!("webos-sat-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mic.raw");
        write_raw(&path, 16_000, 1000);
        let kill = dir.join("disable");
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(path);
        s.mic.looping = true;
        s.safety.kill_switch.clone_from(&kill);
        let shared = Arc::new(Shared::default());
        let (tx, _rx) = mpsc::channel(1024);
        let h = spawn(
            CaptureSettings::from_settings(&s),
            Arc::clone(&shared),
            tx,
            None,
        )
        .unwrap();
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));

        // Mute (daemon flag): closed within a read or two.
        let t = Instant::now();
        shared.mic_off.store(OFF_MUTED, Ordering::Relaxed);
        assert!(wait_for(|| !shared.open.load(Ordering::Relaxed)));
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(shared.releases.load(Ordering::Relaxed), 1);
        shared.mic_off.store(OFF_NONE, Ordering::Relaxed);
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));

        // Kill-switch file (checked by the thread itself): closed within one
        // poll interval plus a read.
        let t = Instant::now();
        std::fs::write(&kill, b"").unwrap();
        assert!(wait_for(|| !shared.open.load(Ordering::Relaxed)));
        assert!(
            t.elapsed() < KILL_SWITCH_POLL + Duration::from_millis(250),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(shared.releases.load(Ordering::Relaxed), 2);
        // Stays closed while the file exists.
        std::thread::sleep(Duration::from_millis(400));
        assert!(!shared.open.load(Ordering::Relaxed));
        std::fs::remove_file(&kill).unwrap();
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));
        assert!(shared.beat.load(Ordering::Relaxed) > 0);

        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the receiving side (the API server) sees, in channel order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Seen {
        Audio,
        Listening(bool),
        Muted(bool),
    }

    /// Privacy gate F7: the indicators follow the real PCM state, and the
    /// command stream (audio and indicators share one channel) never shows
    /// audio before "Listening" ON, "Listening" OFF while audio follows,
    /// "muted" ON while "Listening" is ON, or "Listening" ON while "muted"
    /// is ON. Also: whenever the PCM has been open (or closed) for a while,
    /// what the server last heard says so.
    #[test]
    #[allow(clippy::too_many_lines, clippy::type_complexity)]
    fn indicators_follow_the_real_pcm_state() {
        use std::sync::Mutex;
        let dir = std::env::temp_dir().join(format!("webos-sat-ind-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mic.raw");
        write_raw(&path, 16_000, 1000);
        let kill = dir.join("disable");
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(path);
        s.mic.looping = true;
        s.mic.pause_when_idle = false;
        s.safety.kill_switch.clone_from(&kill);
        let shared = Arc::new(Shared::default());
        // Audio flows for the whole test (wake word armed).
        shared.streaming.store(true, Ordering::Relaxed);
        let (tx, mut rx) = mpsc::channel(4096);
        let log = Arc::new(Mutex::new(Vec::<Seen>::new()));
        let collector = {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                while let Some(cmd) = rx.blocking_recv() {
                    let seen = match cmd {
                        AppCommand::Audio(_) => Seen::Audio,
                        AppCommand::SetBinarySensor { object_id, state } => {
                            let on = state.unwrap();
                            match object_id.as_str() {
                                LISTENING_ID => Seen::Listening(on),
                                MUTED_ID => Seen::Muted(on),
                                other => panic!("unexpected sensor {other}"),
                            }
                        }
                        other => panic!("unexpected command {other:?}"),
                    };
                    log.lock().unwrap().push(seen);
                }
            })
        };
        let shown =
            |what: fn(&Seen) -> Option<bool>| log.lock().unwrap().iter().rev().find_map(what);
        let listening = |e: &Seen| match e {
            Seen::Listening(b) => Some(*b),
            _ => None,
        };
        let muted = |e: &Seen| match e {
            Seen::Muted(b) => Some(*b),
            _ => None,
        };
        let h = spawn(
            CaptureSettings::from_settings(&s),
            Arc::clone(&shared),
            tx,
            None,
        )
        .unwrap();

        // Settled-state check while the gate flips through all its reasons:
        // the PCM state and what the server last heard agree within 300 ms.
        let steps: [(&str, Box<dyn Fn()>); 8] = [
            (
                "mute",
                Box::new(|| shared.mic_off.store(OFF_MUTED, Ordering::Relaxed)),
            ),
            (
                "unmute",
                Box::new(|| shared.mic_off.store(OFF_NONE, Ordering::Relaxed)),
            ),
            (
                "firmware",
                Box::new(|| shared.mic_off.store(OFF_FIRMWARE, Ordering::Relaxed)),
            ),
            (
                "unmute",
                Box::new(|| shared.mic_off.store(OFF_NONE, Ordering::Relaxed)),
            ),
            ("kill", Box::new(|| std::fs::write(&kill, b"").unwrap())),
            ("unkill", Box::new(|| std::fs::remove_file(&kill).unwrap())),
            (
                "unpaired",
                Box::new(|| shared.mic_off.store(OFF_UNPAIRED, Ordering::Relaxed)),
            ),
            (
                "paired",
                Box::new(|| shared.mic_off.store(OFF_NONE, Ordering::Relaxed)),
            ),
        ];
        assert!(
            wait_for(|| shown(listening) == Some(true)),
            "listening ON after the first open"
        );
        assert_eq!(shown(muted), Some(false));
        for (name, step) in &steps {
            step();
            let want_open = matches!(*name, "unmute" | "unkill" | "paired");
            assert!(
                wait_for(|| shared.open.load(Ordering::Relaxed) == want_open),
                "{name}: PCM open should be {want_open}"
            );
            // Sample for 300 ms: never contradicting the settled PCM state.
            let t = Instant::now();
            let mut settled_since = Instant::now();
            let mut last_open = shared.open.load(Ordering::Relaxed);
            while t.elapsed() < Duration::from_millis(300) {
                let open = shared.open.load(Ordering::Relaxed);
                if open != last_open {
                    last_open = open;
                    settled_since = Instant::now();
                }
                if settled_since.elapsed() > Duration::from_millis(100) {
                    assert_eq!(shown(listening), Some(open), "{name}: listening vs PCM");
                    assert_eq!(shown(muted), Some(!open), "{name}: muted vs PCM");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(shown(listening), Some(want_open), "{name}");
            assert_eq!(shown(muted), Some(!want_open), "{name}");
        }

        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        collector.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        // Wire-order invariants over everything the server received.
        let log = log.lock().unwrap();
        let (mut l, mut m): (Option<bool>, Option<bool>) = (None, None);
        let mut opens = 0;
        let mut audio = 0;
        for e in log.iter() {
            match e {
                Seen::Audio => {
                    audio += 1;
                    assert_eq!(l, Some(true), "audio while not shown as listening");
                }
                Seen::Listening(on) => {
                    assert_ne!(l, Some(*on), "repeated listening state");
                    if *on {
                        opens += 1;
                        assert_ne!(m, Some(true), "listening ON while shown muted");
                    }
                    l = Some(*on);
                }
                Seen::Muted(on) => {
                    assert_ne!(m, Some(*on), "repeated muted state");
                    assert!(!*on || l != Some(true), "muted ON while shown listening");
                    m = Some(*on);
                }
            }
        }
        assert!(audio > 0);
        // One ON per PCM open: the initial one and one per unmute (twice),
        // unkill and pair.
        assert_eq!(opens, 5, "{log:?}");
        // The thread's end closed the PCM and said so.
        assert_eq!(l, Some(false));
    }

    /// Deterministic noise in [-a, a].
    fn noise(n: usize, a: f32, seed: u32) -> Vec<f32> {
        let mut s = seed.max(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                #[allow(clippy::cast_precision_loss)]
                let v = (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * a;
                v
            })
            .collect()
    }

    /// mic.raw: S24 stereo 16 kHz, the echo of `base` (15 ms, -10 dB) plus
    /// faint near-end noise; ref.raw: S32 stereo 48 kHz, `base` held for
    /// three samples (what the TV played).
    #[allow(clippy::cast_possible_truncation)]
    fn write_echo_pair(dir: &Path, seconds: usize) -> (PathBuf, PathBuf) {
        let n = 16_000 * seconds;
        // Band-limited (4-tap average: nulls at 4 and 8 kHz) so the echo has
        // no energy the decimated 48 kHz reference cannot carry.
        let white = noise(n + 3, 0.1, 3);
        let base: Vec<f32> = white
            .windows(4)
            .map(|w| w.iter().sum::<f32>() / 4.0)
            .collect();
        let near = noise(n, 0.0003, 4);
        let mut mic = Vec::new();
        for i in 0..n {
            let e = if i >= 240 { 0.3 * base[i - 240] } else { 0.0 };
            let v = ((e + near[i]) * 8_388_607.0) as i32 & 0x00FF_FFFF;
            for _ in 0..2 {
                mic.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut reference = Vec::new();
        for &b in &base {
            let v = (b * 2_147_483_647.0) as i32;
            for _ in 0..6 {
                reference.extend_from_slice(&v.to_le_bytes());
            }
        }
        let (m, r) = (dir.join("mic.raw"), dir.join("ref.raw"));
        std::fs::write(&m, mic).unwrap();
        std::fs::write(&r, reference).unwrap();
        (m, r)
    }

    /// Streams the whole file pair and returns the RMS (dBFS) of the last
    /// 2 s sent to HA.
    fn stream_rms_db(s: &Settings) -> f64 {
        let shared = Arc::new(Shared::default());
        let (tx, mut rx) = mpsc::channel(4096);
        shared.streaming.store(true, Ordering::Relaxed);
        let h = spawn(
            CaptureSettings::from_settings(s),
            Arc::clone(&shared),
            tx,
            None,
        )
        .unwrap();
        let mut samples: Vec<i16> = Vec::new();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            match rx.try_recv() {
                Ok(AppCommand::Audio(chunk)) => samples.extend_from_slice(&chunk),
                Ok(_) => {}
                Err(_) => {
                    if samples.len() >= 16_000 * 7 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(samples.len() >= 16_000 * 7, "{} samples", samples.len());
        let tail = &samples[samples.len() - 32_000..];
        #[allow(clippy::cast_precision_loss)]
        let ms = tail.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / tail.len() as f64;
        10.0 * (ms.sqrt() / 32_768.0).log10()
    }

    #[test]
    fn file_pair_with_aec_cancels_the_echo() {
        let dir = std::env::temp_dir().join(format!("webos-sat-aec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (m, r) = write_echo_pair(&dir, 8);
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(m);
        s.mic.realtime = false;
        s.dsp.gain_db = 0.0;
        let plain = stream_rms_db(&s);
        s.aec.enabled = true;
        s.aec.reference_path = Some(r);
        let cancelled = stream_rms_db(&s);
        let _ = std::fs::remove_dir_all(&dir);
        // Echo at about -21 dBFS. This checks the wiring (the canceller is in
        // the streamed path and sees the reference in step); cancellation
        // depth on real recordings is measured with chain_eval.py. The
        // synthetic 48 kHz reference (sample-and-hold) limits it to about
        // 14 dB here.
        assert!(plain > -30.0, "plain {plain:.1}");
        assert!(
            plain - cancelled > 10.0,
            "plain {plain:.1} dBFS, AEC {cancelled:.1} dBFS"
        );
    }

    #[test]
    fn missing_reference_file_leaves_the_mic_running() {
        let dir = std::env::temp_dir().join(format!("webos-sat-aec-miss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (m, _r) = write_echo_pair(&dir, 8);
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(m);
        s.mic.realtime = false;
        s.dsp.gain_db = 0.0;
        s.aec.enabled = true;
        s.aec.reference_path = Some(dir.join("missing.raw"));
        // Audio still flows (uncancelled).
        let db = stream_rms_db(&s);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(db > -30.0, "{db:.1}");
    }

    #[test]
    fn missing_file_is_retried_until_stop() {
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(PathBuf::from("/nonexistent/mic.raw"));
        let shared = Arc::new(Shared::default());
        let (tx, mut rx) = mpsc::channel(4);
        let h = spawn(
            CaptureSettings::from_settings(&s),
            Arc::clone(&shared),
            tx,
            None,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!shared.open.load(Ordering::Relaxed));
        // A failed open is never reported as listening.
        while let Ok(cmd) = rx.try_recv() {
            assert!(
                !matches!(
                    cmd,
                    AppCommand::SetBinarySensor { ref object_id, state: Some(true) }
                        if object_id == LISTENING_ID
                ),
                "{cmd:?}"
            );
        }
        let t = Instant::now();
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(t.elapsed() < Duration::from_millis(500));
    }

    /// 2-channel S24 raw file of pseudo-random noise (`amp` LSB), or a
    /// constant when `amp` is 0 (the value 1000).
    fn write_noise(path: &std::path::Path, frames: usize, amp: i32) {
        let mut seed = 12_345_u32;
        let mut bytes = Vec::new();
        for _ in 0..frames * 2 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let v = if amp == 0 {
                1000
            } else {
                i32::try_from(seed >> 8).unwrap() % (2 * amp + 1) - amp
            };
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// Settings for a live-paced file mic with a 1 s self-check window.
    fn selfcheck_settings(dir: &std::path::Path, amp: i32) -> Settings {
        let path = dir.join("mic.raw");
        // 2 s of audio, looped: the check sees a steady stream.
        write_noise(&path, 16_000 * 2, amp);
        let mut s = Settings::default();
        s.mic.source = MicSource::File;
        s.mic.path = Some(path);
        s.mic.looping = true;
        s.mic.pause_when_idle = false;
        s.safety.self_check_window_s = 1;
        s.safety.kill_switch = dir.join("kill");
        s
    }

    fn notes_of(rx: &mut mpsc::UnboundedReceiver<Note>) -> Vec<Note> {
        let mut out = Vec::new();
        while let Ok(n) = rx.try_recv() {
            out.push(n);
        }
        out
    }

    fn last_result(notes: &[Note]) -> Option<&crate::selfcheck::Finished> {
        notes.iter().rev().find_map(|n| match n {
            Note::Result(f) => Some(&**f),
            _ => None,
        })
    }

    /// A normal stream passes the check and the microphone stays open.
    #[test]
    fn selfcheck_passes_on_a_normal_stream_and_keeps_the_mic_open() {
        use webos_compat::engine::Overall;
        let dir = std::env::temp_dir().join(format!("webos-sat-sc-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = selfcheck_settings(&dir, 20_000);
        let shared = Arc::new(Shared::default());
        let (tx, _rx) = mpsc::channel(4096);
        let (ntx, mut nrx) = mpsc::unbounded_channel();
        let h = spawn(
            CaptureSettings::from_settings(&s),
            Arc::clone(&shared),
            tx,
            Some(ntx),
        )
        .unwrap();
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));
        std::thread::sleep(Duration::from_millis(1700));
        let notes = notes_of(&mut nrx);
        assert!(matches!(notes.first(), Some(Note::Checking)), "{notes:?}");
        let f = last_result(&notes).unwrap();
        assert_eq!(
            f.result.overall(),
            Overall::Passed,
            "{}",
            f.result.summary_text()
        );
        assert!(shared.open.load(Ordering::Relaxed));
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A very quiet stream is a warning: the microphone stays open.
    #[test]
    fn selfcheck_warns_on_a_silent_stream_and_does_not_close() {
        use webos_compat::engine::Overall;
        let dir = std::env::temp_dir().join(format!("webos-sat-sc-warn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = selfcheck_settings(&dir, 1);
        let shared = Arc::new(Shared::default());
        let (tx, _rx) = mpsc::channel(4096);
        let (ntx, mut nrx) = mpsc::unbounded_channel();
        let h = spawn(
            CaptureSettings::from_settings(&s),
            Arc::clone(&shared),
            tx,
            Some(ntx),
        )
        .unwrap();
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));
        std::thread::sleep(Duration::from_millis(1700));
        let notes = notes_of(&mut nrx);
        let f = last_result(&notes).unwrap();
        assert_eq!(
            f.result.overall(),
            Overall::Warning,
            "{}",
            f.result.summary_text()
        );
        assert!(shared.open.load(Ordering::Relaxed));
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stuck (constant) stream fails: the thread closes the PCM, shows the
    /// microphone as muted, keeps the retry bounded and retries only on a
    /// rerun request.
    #[test]
    fn selfcheck_fails_on_a_stuck_stream_closes_the_mic_and_reruns_on_request() {
        use webos_compat::engine::Overall;
        let dir = std::env::temp_dir().join(format!("webos-sat-sc-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = selfcheck_settings(&dir, 0);
        let shared = Arc::new(Shared::default());
        let (tx, mut rx) = mpsc::channel(4096);
        let (ntx, mut nrx) = mpsc::unbounded_channel();
        let h = spawn(
            CaptureSettings::from_settings(&s),
            Arc::clone(&shared),
            tx,
            Some(ntx),
        )
        .unwrap();
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));
        assert!(wait_for(|| !shared.open.load(Ordering::Relaxed)));
        std::thread::sleep(Duration::from_millis(100));
        let notes = notes_of(&mut nrx);
        let f = last_result(&notes).unwrap();
        assert_eq!(f.result.overall(), Overall::Failed);
        assert!(f.result.summary_text().starts_with("failed:"));
        // The first retry is 5 s away: the mic stays closed meanwhile and the
        // indicators say so (muted on, listening off).
        let mut muted = None;
        let mut listening = None;
        while let Ok(cmd) = rx.try_recv() {
            if let AppCommand::SetBinarySensor { object_id, state } = cmd {
                match object_id.as_str() {
                    MUTED_ID => muted = state,
                    LISTENING_ID => listening = state,
                    _ => {}
                }
            }
        }
        assert_eq!((muted, listening), (Some(true), Some(false)));
        std::thread::sleep(Duration::from_millis(500));
        assert!(!shared.open.load(Ordering::Relaxed));
        // A rerun request opens again at once and measures again.
        shared.selfcheck_rerun.store(true, Ordering::Relaxed);
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));
        assert!(wait_for(|| !shared.open.load(Ordering::Relaxed)));
        let notes = notes_of(&mut nrx);
        assert!(notes.iter().any(|n| matches!(n, Note::Checking)));
        assert_eq!(
            last_result(&notes).map(|f| f.result.overall()),
            Some(Overall::Failed)
        );
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A card that is not in `/proc/asound` fails C2 before any open.
    #[test]
    fn selfcheck_fails_c2_without_opening_when_the_card_is_missing() {
        use webos_compat::check::{CheckId, Grade, Reason};
        let dir = std::env::temp_dir().join(format!("webos-sat-sc-c2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let asound = dir.join("asound");
        std::fs::create_dir_all(&asound).unwrap();
        std::fs::write(
            asound.join("cards"),
            " 0 [lg115x         ]: lg115x - lg115x\n",
        )
        .unwrap();
        std::fs::write(asound.join("pcm"), "00-10: MixerCapture : : capture 1\n").unwrap();
        let mut s = Settings::default();
        s.mic.source = MicSource::Alsa;
        s.mic.card_id = "NOSUCHCARD".to_owned();
        s.safety.kill_switch = dir.join("kill");
        let mut cfg = CaptureSettings::from_settings(&s);
        cfg.selfcheck.as_mut().unwrap().asound = asound;
        let shared = Arc::new(Shared::default());
        let (tx, _rx) = mpsc::channel(64);
        let (ntx, mut nrx) = mpsc::unbounded_channel();
        let h = spawn(cfg, Arc::clone(&shared), tx, Some(ntx)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!shared.open.load(Ordering::Relaxed), "never opened");
        let notes = notes_of(&mut nrx);
        let f = last_result(&notes).unwrap();
        let c2 = f.result.check(CheckId::C2);
        assert_eq!((c2.grade, c2.reason), (Grade::Fail, Some(Reason::NoCard)));
        // The cards are listed for the report (ids only).
        assert!(notes.iter().any(|n| matches!(
            n,
            Note::Audio(a, _) if a.cards.iter().any(|c| c.id == "lg115x")
        )));
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `safety.self_check = false`: no check, no notes, nothing changes.
    #[test]
    fn selfcheck_off_sends_no_notes() {
        let dir = std::env::temp_dir().join(format!("webos-sat-sc-off-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = selfcheck_settings(&dir, 0);
        s.safety.self_check = false;
        let cfg = CaptureSettings::from_settings(&s);
        assert!(cfg.selfcheck.is_none());
        let shared = Arc::new(Shared::default());
        let (tx, _rx) = mpsc::channel(4096);
        let (ntx, mut nrx) = mpsc::unbounded_channel();
        let h = spawn(cfg, Arc::clone(&shared), tx, Some(ntx)).unwrap();
        assert!(wait_for(|| shared.open.load(Ordering::Relaxed)));
        std::thread::sleep(Duration::from_millis(1700));
        // A constant stream would have failed the check; the mic stays open.
        assert!(shared.open.load(Ordering::Relaxed));
        assert!(notes_of(&mut nrx).is_empty());
        shared.stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A paused file mic (frames only while HA streams) is not checked.
    #[test]
    fn selfcheck_skips_a_file_mic_that_pauses_when_idle() {
        let dir = std::env::temp_dir().join(format!("webos-sat-sc-pause-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = selfcheck_settings(&dir, 0);
        s.mic.pause_when_idle = true;
        assert!(CaptureSettings::from_settings(&s).selfcheck.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
