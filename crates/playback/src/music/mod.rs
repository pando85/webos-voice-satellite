//! Music playback engine (design milestone M4): one music output slot with
//! gain, ducking, pause and a write-ahead pacer.
//!
//! ```text
//!  decoder thread (M3a)         engine task (this module)                 output
//!  -------------------   ->     ----------------------------------   ->   -------
//!  MusicSource (PCM chunks      10 ms slices, Q15 gain ramps             pacat #2 "music"
//!  with rate and channels)      (volume x duck x fade), pacer:           opened from the
//!                               written at most `write_ahead` (60 ms)    DECODED rate and
//!                               ahead of the clock                       channels
//!
//!  daemon (M5) -> play / pause / resume / stop, duck_begin / duck_end(source), MusicVolume
//! ```
//!
//! The engine is a library: it does not know the daemon, HA or the config
//! file. The daemon builds a [`MusicConfig`] from `[music]`, a
//! [`PacatFactory`] from `[sink]`, shares the reply volume through
//! [`MusicVolume`], and feeds the duck sources from its own events.
//!
//! # Behaviour (design D2, D4, D5, D11, section D)
//!
//! * **Output slot.** The output is opened when the first chunk arrives, from
//!   that chunk's rate and channels. A track in another format (a new
//!   [`MusicEngine::play`]) closes the old output and opens a new one. A format
//!   change inside one stream ends the track with an error.
//! * **Gain.** The samples are scaled in integer Q15 ([`gain`]): the product
//!   of the volume (shared [`MusicVolume`] times the `gain_db` trim), the duck
//!   and the pause fade. Every ramp moves per sample; nothing steps at chunk
//!   boundaries. `sink.gain = 0` writes only zeros.
//! * **Duck.** [`DuckController`]: reference-counted sources, attack, hold,
//!   release; modes duck, pause and none.
//! * **Pacer.** Audio is written in 10 ms slices and never more than
//!   `write_ahead` ahead of the clock, which bounds how late a duck, a pause
//!   or a stop is heard to `write_ahead` plus the output's own latency. A
//!   source that cannot keep up gets up to `max_silence` of silence per
//!   underrun (counted), and after any stall, blocked write or pause the
//!   clock is **reset** rather than caught up, so there is never a burst.
//! * **Pause.** The fade-out is written, then nothing is read or written
//!   until resume; the output stays open, no sample is lost or repeated. A
//!   pause longer than `pause_timeout` ends the track (the stream is dropped).
//! * **Stop** kills the output and drops the source, which cancels the
//!   decoder.
//!
//! Timing is on `tokio::time`, so tests run it with a paused clock.

mod duck;
mod gain;
mod output;
mod source;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Context;
use std::time::Duration;

use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub use duck::{DuckConfig, DuckController, DuckLevel, DuckMode, DuckSource};
pub use gain::{
    GainRamp, GainStage, MAX_GAIN, MusicVolume, ONE, q15_from_db, q15_from_f32, q15_mul,
    q15_to_f32, scale,
};
pub use output::{
    MusicOutput, OutFuture, OutputFactory, PacatConfig, PacatFactory, WavStreamFactory, read_wav,
};
pub use source::{ChannelSource, MusicSource, SourceItem, ToneSource, Wave};

use crate::sink::SinkFormat;

/// The engine works in slices of this length: it is the granularity of the
/// pacer, of gain changes and of command handling.
pub const SLICE: Duration = Duration::from_millis(10);

/// Engine settings (`[music]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicConfig {
    /// Ducking (`on_wake`, `duck_db`, `duck_attack_ms`, `duck_hold_ms`,
    /// `duck_release_ms`).
    pub duck: DuckConfig,
    /// Trim on top of the shared volume, in dB (`gain_db`, -60 to +12).
    pub gain_db: f32,
    /// How far ahead of the clock audio is written (`write_ahead_ms`, 60).
    pub write_ahead: Duration,
    /// A paused track is dropped after this (`pause_timeout_s`, 600).
    pub pause_timeout: Duration,
    /// Fade at pause and resume (10 ms); zero cuts without a fade.
    pub fade: Duration,
    /// Time a volume change takes to move the level (30 ms).
    pub volume_ramp: Duration,
    /// Most silence written for one underrun (200 ms).
    pub max_silence: Duration,
    /// How long a finished track may take to drain its output (3 s).
    pub drain_timeout: Duration,
}

impl Default for MusicConfig {
    fn default() -> Self {
        Self {
            duck: DuckConfig::default(),
            gain_db: 0.0,
            write_ahead: Duration::from_millis(60),
            pause_timeout: Duration::from_secs(600),
            fade: Duration::from_millis(10),
            volume_ramp: Duration::from_millis(30),
            max_silence: Duration::from_millis(200),
            drain_timeout: Duration::from_secs(3),
        }
    }
}

/// What the music slot is doing, for the entity state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicState {
    /// No track.
    Idle,
    /// A track is loading or playing (also while paused by a duck source).
    Playing,
    /// Paused by [`MusicEngine::pause`].
    Paused,
}

/// Identifies one [`MusicEngine::play`], so late events can be ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackId(pub u64);

/// Why a track ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The source ended and the output drained.
    Finished,
    /// [`MusicEngine::stop`] or a shutdown.
    Stopped,
    /// Another track started.
    Replaced,
    /// Paused for longer than `pause_timeout`.
    PauseTimeout,
    /// The source failed, the stream changed format, or the output failed
    /// (the text never contains a URL).
    Error(String),
}

/// Events from the engine task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MusicEvent {
    /// The first chunk arrived and the output was opened in this format.
    Started {
        /// The track.
        id: TrackId,
        /// Decoded sample rate.
        rate: u32,
        /// Decoded channel count.
        channels: u16,
    },
    /// The track is over and its output is closed.
    Ended {
        /// The track.
        id: TrackId,
        /// Why.
        reason: EndReason,
    },
}

/// Counters since the engine started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MusicStats {
    /// Frames of music (and underrun silence) written to outputs.
    pub frames_written: u64,
    /// Of those, frames of silence written because the source was late.
    pub silence_frames: u64,
    /// Number of underruns (source had nothing when the clock needed it).
    pub underruns: u64,
    /// Times the pacer clock was reset after a stall or a blocked write.
    pub clock_resets: u64,
    /// Tracks started (outputs opened).
    pub tracks: u64,
}

#[derive(Debug, Default)]
struct Counters {
    frames_written: AtomicU64,
    silence_frames: AtomicU64,
    underruns: AtomicU64,
    clock_resets: AtomicU64,
    tracks: AtomicU64,
}

struct Shared {
    duck: Mutex<DuckController>,
    notify: Notify,
    counters: Counters,
    volume: MusicVolume,
    epoch: Instant,
    state: watch::Sender<MusicState>,
}

impl Shared {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn duck(&self) -> MutexGuard<'_, DuckController> {
        self.duck.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_state(&self, s: MusicState) {
        self.state.send_replace(s);
    }
}

enum Cmd {
    Play(TrackId, Box<dyn MusicSource>),
    Pause,
    Resume,
    Stop,
    Shutdown(oneshot::Sender<()>),
}

/// Control handle of the music engine (the engine itself is a task).
pub struct MusicEngine {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Shared>,
    state_rx: watch::Receiver<MusicState>,
    next_id: AtomicU64,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for MusicEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MusicEngine")
            .field("state", &*self.state_rx.borrow())
            .finish_non_exhaustive()
    }
}

impl MusicEngine {
    /// Starts the engine task on the current tokio runtime.
    ///
    /// `volume` is shared with the daemon (design D2). The receiver carries
    /// [`MusicEvent`]s; dropping it is fine.
    #[must_use]
    pub fn spawn(
        mut cfg: MusicConfig,
        factory: Box<dyn OutputFactory>,
        volume: MusicVolume,
    ) -> (Self, mpsc::UnboundedReceiver<MusicEvent>) {
        cfg.write_ahead = cfg.write_ahead.max(SLICE * 2);
        cfg.duck.duck_db = cfg.duck.duck_db.min(0.0);
        cfg.gain_db = cfg.gain_db.clamp(-60.0, 12.0);
        let (tx, rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(MusicState::Idle);
        let shared = Arc::new(Shared {
            duck: Mutex::new(DuckController::new(cfg.duck)),
            notify: Notify::new(),
            counters: Counters::default(),
            volume,
            epoch: Instant::now(),
            state: state_tx,
        });
        let core = Core {
            cfg,
            trim_q15: q15_from_db(cfg.gain_db),
            duck_q15: q15_from_db(cfg.duck.duck_db),
            factory,
            shared: Arc::clone(&shared),
            rx,
            events: ev_tx,
        };
        let task = tokio::spawn(core.run());
        (
            Self {
                tx,
                shared,
                state_rx,
                next_id: AtomicU64::new(1),
                task,
            },
            ev_rx,
        )
    }

    /// Starts a track, replacing the current one (which ends with
    /// [`EndReason::Replaced`]). A pause is cleared.
    pub fn play(&self, source: Box<dyn MusicSource>) -> TrackId {
        let id = TrackId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let _ = self.tx.send(Cmd::Play(id, source));
        id
    }

    /// Pauses the current track (ignored when idle).
    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }

    /// Resumes a paused track.
    pub fn resume(&self) {
        let _ = self.tx.send(Cmd::Resume);
    }

    /// Stops the current track (ignored when idle).
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    /// A duck source starts (wake word detected, reply or announcement
    /// playing, alarm ringing). Counted: every begin needs an end.
    pub fn duck_begin(&self, source: DuckSource) {
        self.shared.duck().begin(source);
        self.shared.notify.notify_one();
    }

    /// A duck source ends; the duck is released after the hold once the last
    /// source has ended.
    pub fn duck_end(&self, source: DuckSource) {
        let now = self.shared.now();
        self.shared.duck().end(source, now);
        self.shared.notify.notify_one();
    }

    /// Ends all duck sources (for example after a lost connection).
    pub fn duck_clear(&self) {
        let now = self.shared.now();
        self.shared.duck().clear(now);
        self.shared.notify.notify_one();
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> MusicState {
        *self.state_rx.borrow()
    }

    /// A receiver of state changes.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<MusicState> {
        self.state_rx.clone()
    }

    /// Counters (underruns, silence, clock resets).
    #[must_use]
    pub fn stats(&self) -> MusicStats {
        let c = &self.shared.counters;
        MusicStats {
            frames_written: c.frames_written.load(Ordering::Relaxed),
            silence_frames: c.silence_frames.load(Ordering::Relaxed),
            underruns: c.underruns.load(Ordering::Relaxed),
            clock_resets: c.clock_resets.load(Ordering::Relaxed),
            tracks: c.tracks.load(Ordering::Relaxed),
        }
    }

    /// Stops playback, kills the output and ends the task (bounded wait).
    pub async fn shutdown(self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Cmd::Shutdown(tx)).is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
        }
        if tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .is_err()
        {
            // The task is gone with the handle; nothing else to do.
        }
    }
}

// ------------------------------------------------------------------- engine

/// Pause state of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Running,
    /// Pause asked: the fade-out is being written.
    FadingOut,
    Paused,
}

/// Why `drive` returned.
enum Exit {
    /// The track is over; `drain` lets the output play out what it has.
    End { reason: EndReason, drain: bool },
    /// A command ends it.
    Abort(Abort),
}

enum Abort {
    Stop,
    Replace(TrackId, Box<dyn MusicSource>),
    Shutdown(oneshot::Sender<()>),
    /// Every handle was dropped.
    Closed,
}

impl Abort {
    fn reason(&self) -> EndReason {
        match self {
            Self::Replace(..) => EndReason::Replaced,
            _ => EndReason::Stopped,
        }
    }
}

enum Pull {
    Item(SourceItem),
    /// The source had nothing before the clock ran out.
    Starved,
    /// A wake-up (duck change, command): re-evaluate.
    Woken,
    Abort(Abort),
}

/// Per-track state (the source and the output live next to it so a command
/// handler can borrow it while a write or a poll is in flight).
struct Track {
    fmt: Option<SinkFormat>,
    buf: Vec<i16>,
    pos: usize,
    /// Frames written (music and silence).
    written: u64,
    /// The time frame 0 would have started playing; `None` before the first write.
    origin: Option<Instant>,
    gain: GainStage,
    run: Run,
    user_paused: bool,
    paused_since: Option<Instant>,
    /// Frames of silence written in the current underrun.
    silence_run: u64,
    scratch: Vec<i16>,
    bytes: Vec<u8>,
}

impl Track {
    fn new() -> Self {
        Self {
            fmt: None,
            buf: Vec::new(),
            pos: 0,
            written: 0,
            origin: None,
            gain: GainStage::new(ONE),
            run: Run::Running,
            user_paused: false,
            paused_since: None,
            silence_run: 0,
            scratch: Vec::new(),
            bytes: Vec::new(),
        }
    }

    fn rate(&self) -> u32 {
        self.fmt.map_or(48_000, |f| f.rate_hz)
    }

    /// Time of the audio written so far.
    fn written_dur(&self) -> Duration {
        frames_to_dur(self.written, self.rate())
    }

    /// The clock restarts as if everything written had just played.
    fn reset_clock(&mut self, now: Instant) {
        if self.origin.is_some() {
            self.origin = Some(now.checked_sub(self.written_dur()).unwrap_or(now));
        }
    }
}

fn frames_to_dur(frames: u64, rate: u32) -> Duration {
    if rate == 0 {
        return Duration::ZERO;
    }
    let ns = u128::from(frames) * 1_000_000_000 / u128::from(rate);
    Duration::from_nanos(u64::try_from(ns).unwrap_or(u64::MAX))
}

fn dur_to_frames(d: Duration, rate: u32) -> u32 {
    let f = d.as_nanos() * u128::from(rate) / 1_000_000_000;
    u32::try_from(f).unwrap_or(u32::MAX)
}

/// Handles a command received while a track runs; `Some` ends the track.
fn on_cmd(t: &mut Track, shared: &Shared, cmd: Option<Cmd>) -> Option<Abort> {
    match cmd {
        None => Some(Abort::Closed),
        Some(Cmd::Play(id, src)) => Some(Abort::Replace(id, src)),
        Some(Cmd::Stop) => Some(Abort::Stop),
        Some(Cmd::Shutdown(tx)) => Some(Abort::Shutdown(tx)),
        Some(Cmd::Pause) => {
            if !t.user_paused {
                t.user_paused = true;
                shared.set_state(MusicState::Paused);
            }
            None
        }
        Some(Cmd::Resume) => {
            if t.user_paused {
                t.user_paused = false;
                shared.set_state(MusicState::Playing);
            }
            None
        }
    }
}

struct Core {
    cfg: MusicConfig,
    trim_q15: u32,
    duck_q15: u32,
    factory: Box<dyn OutputFactory>,
    shared: Arc<Shared>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    events: mpsc::UnboundedSender<MusicEvent>,
}

enum Next {
    Idle,
    Play(TrackId, Box<dyn MusicSource>),
    Shutdown(oneshot::Sender<()>),
    Closed,
}

impl Core {
    async fn run(mut self) {
        let mut queued: VecDeque<(TrackId, Box<dyn MusicSource>)> = VecDeque::new();
        loop {
            let (id, src) = if let Some(x) = queued.pop_front() {
                x
            } else {
                match self.rx.recv().await {
                    Some(Cmd::Play(id, src)) => (id, src),
                    Some(Cmd::Shutdown(tx)) => {
                        let _ = tx.send(());
                        return;
                    }
                    Some(_) => continue,
                    None => return,
                }
            };
            match self.run_track(id, src).await {
                Next::Idle => {}
                Next::Play(id, src) => queued.push_back((id, src)),
                Next::Shutdown(tx) => {
                    let _ = tx.send(());
                    return;
                }
                Next::Closed => return,
            }
        }
    }

    async fn run_track(&mut self, id: TrackId, mut src: Box<dyn MusicSource>) -> Next {
        let mut t = Track::new();
        let mut out: Option<Box<dyn MusicOutput>> = None;
        self.shared.set_state(MusicState::Playing);

        let exit = self.drive(id, &mut t, src.as_mut(), &mut out).await;

        let (reason, abort) = match exit {
            Exit::End { reason, drain } => {
                let mut abort = None;
                if let Some(o) = out.as_mut() {
                    let drained = if drain {
                        match self.finish(&mut t, o.as_mut()).await {
                            Ok(drained) => drained,
                            Err(a) => {
                                abort = Some(a);
                                false
                            }
                        }
                    } else {
                        false
                    };
                    if !drained {
                        let _ = o.abort().await;
                    }
                }
                let reason = abort.as_ref().map_or(reason, Abort::reason);
                (reason, abort)
            }
            Exit::Abort(a) => {
                if let Some(o) = out.as_mut() {
                    let _ = o.abort().await;
                }
                (a.reason(), Some(a))
            }
        };
        drop(out);
        drop(src); // cancels the decoder behind it

        let next = match abort {
            Some(Abort::Replace(id, src)) => Next::Play(id, src),
            Some(Abort::Shutdown(tx)) => Next::Shutdown(tx),
            Some(Abort::Closed) => Next::Closed,
            Some(Abort::Stop) | None => Next::Idle,
        };
        if !matches!(next, Next::Play(..)) {
            self.shared.set_state(MusicState::Idle);
        }
        let _ = self.events.send(MusicEvent::Ended { id, reason });
        next
    }

    /// Lets the output play out what it holds. `Ok(true)` when it drained,
    /// `Ok(false)` on timeout or failure, `Err` when a command ended it.
    async fn finish(&mut self, t: &mut Track, out: &mut dyn MusicOutput) -> Result<bool, Abort> {
        let fut = out.finish();
        tokio::pin!(fut);
        let deadline = tokio::time::sleep(self.cfg.drain_timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                r = &mut fut => return Ok(r.is_ok()),
                cmd = self.rx.recv() => {
                    if let Some(a) = on_cmd(t, &self.shared, cmd) {
                        return Err(a);
                    }
                }
                () = &mut deadline => return Ok(false),
            }
        }
    }

    /// Duck and volume targets for the next slice.
    fn update_gain(&self, t: &mut Track, level: DuckLevel) {
        let rate = t.rate();
        let base = q15_mul(self.shared.volume.get_q15(), self.trim_q15).min(MAX_GAIN);
        if base != t.gain.vol.target() {
            t.gain
                .vol
                .to(base, dur_to_frames(self.cfg.volume_ramp, rate));
        }
        let want = match level {
            DuckLevel::Ducked => self.duck_q15,
            DuckLevel::Unity => ONE,
            DuckLevel::Paused => t.gain.duck.target(),
        };
        if want != t.gain.duck.target() {
            let full = if want < t.gain.duck.target() {
                self.cfg.duck.attack
            } else {
                self.cfg.duck.release
            };
            t.gain
                .duck
                .to_slope(want, ONE - self.duck_q15, dur_to_frames(full, rate));
        }
    }

    /// Waits up to `d` (forever for `None`), woken by a command or a duck
    /// change.
    async fn idle_wait(&mut self, t: &mut Track, d: Option<Duration>) -> Option<Abort> {
        tokio::select! {
            biased;
            cmd = self.rx.recv() => on_cmd(t, &self.shared, cmd),
            () = tokio::time::sleep(d.unwrap_or_default()), if d.is_some() => None,
            () = self.shared.notify.notified() => None,
        }
    }

    async fn pull(
        &mut self,
        t: &mut Track,
        src: &mut dyn MusicSource,
        cap: Option<Duration>,
    ) -> Pull {
        tokio::select! {
            biased;
            cmd = self.rx.recv() => match on_cmd(t, &self.shared, cmd) {
                Some(a) => Pull::Abort(a),
                None => Pull::Woken,
            },
            item = std::future::poll_fn(|cx: &mut Context<'_>| src.poll_chunk(cx)) => Pull::Item(item),
            () = tokio::time::sleep(cap.unwrap_or_default()), if cap.is_some() => Pull::Starved,
            () = self.shared.notify.notified() => Pull::Woken,
        }
    }

    /// Writes `t.bytes` (one slice), noticing commands while it is blocked.
    async fn write(&mut self, t: &mut Track, out: &mut dyn MusicOutput) -> Result<(), Exit> {
        let bytes = std::mem::take(&mut t.bytes);
        let res = {
            let fut = out.write(&bytes);
            tokio::pin!(fut);
            loop {
                tokio::select! {
                    biased;
                    r = &mut fut => break Ok(r),
                    cmd = self.rx.recv() => {
                        if let Some(a) = on_cmd(t, &self.shared, cmd) {
                            break Err(a);
                        }
                    }
                }
            }
        };
        t.bytes = bytes;
        match res {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(Exit::End {
                reason: EndReason::Error(format!("writing to the output failed: {e}")),
                drain: false,
            }),
            Err(a) => Err(Exit::Abort(a)),
        }
    }

    /// Validates the first chunk and opens the output from it.
    fn open(
        &mut self,
        id: TrackId,
        t: &mut Track,
        out: &mut Option<Box<dyn MusicOutput>>,
        fmt: SinkFormat,
        level: DuckLevel,
    ) -> Result<(), Exit> {
        match self.factory.open(fmt) {
            Ok(o) => *out = Some(o),
            Err(e) => {
                return Err(Exit::End {
                    reason: EndReason::Error(format!("cannot open the music output: {e}")),
                    drain: false,
                });
            }
        }
        t.fmt = Some(fmt);
        let base = q15_mul(self.shared.volume.get_q15(), self.trim_q15).min(MAX_GAIN);
        t.gain = GainStage::new(base);
        if level == DuckLevel::Ducked {
            t.gain.duck.set(self.duck_q15);
        }
        self.shared.counters.tracks.fetch_add(1, Ordering::Relaxed);
        let _ = self.events.send(MusicEvent::Started {
            id,
            rate: fmt.rate_hz,
            channels: fmt.channels,
        });
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn drive(
        &mut self,
        id: TrackId,
        t: &mut Track,
        src: &mut dyn MusicSource,
        out: &mut Option<Box<dyn MusicOutput>>,
    ) -> Exit {
        let max_silence_dur = self.cfg.max_silence;
        let max_silence = move |rate: u32| u64::from(dur_to_frames(max_silence_dur, rate));
        loop {
            let now = Instant::now();
            let level = {
                let rel = self.shared.now();
                self.shared.duck().level(rel)
            };

            // Pause state machine: user pause or a duck source in pause mode.
            let want_pause = t.user_paused || level == DuckLevel::Paused;
            let fade_frames = dur_to_frames(self.cfg.fade, t.rate());
            match t.run {
                Run::Running if want_pause => {
                    t.gain.fade.to(0, fade_frames);
                    t.run = Run::FadingOut;
                }
                Run::FadingOut | Run::Paused if !want_pause => {
                    t.gain.fade.to(ONE, fade_frames);
                    if t.run == Run::Paused {
                        t.reset_clock(now);
                    }
                    t.run = Run::Running;
                    t.paused_since = None;
                }
                _ => {}
            }
            if t.run == Run::FadingOut
                && (t.gain.fade.is_idle() || out.is_none() || t.pos >= t.buf.len())
            {
                t.run = Run::Paused;
                t.paused_since = Some(now);
            }
            if t.run == Run::Paused {
                let mut wait: Option<Duration> = None;
                if t.user_paused {
                    let since = t.paused_since.unwrap_or(now);
                    let left = self.cfg.pause_timeout.saturating_sub(now - since);
                    if left.is_zero() {
                        return Exit::End {
                            reason: EndReason::PauseTimeout,
                            drain: false,
                        };
                    }
                    wait = Some(left);
                }
                if let Some(dl) = self.shared.duck().next_deadline() {
                    let left = dl.saturating_sub(self.shared.now());
                    wait = Some(wait.map_or(left, |w| w.min(left)));
                }
                if let Some(a) = self.idle_wait(t, wait).await {
                    return Exit::Abort(a);
                }
                continue;
            }

            // Pacer: never write more than `write_ahead` ahead of the clock.
            if let (Some(_), Some(origin)) = (t.fmt, t.origin) {
                let wd = t.written_dur();
                let played = now - origin;
                if played > wd + SLICE {
                    // The output starved or a write blocked: do not catch up.
                    t.reset_clock(now);
                    self.shared
                        .counters
                        .clock_resets
                        .fetch_add(1, Ordering::Relaxed);
                }
                let ahead = wd.saturating_sub(now - t.origin.unwrap_or(origin));
                let limit = self.cfg.write_ahead.saturating_sub(SLICE);
                if ahead > limit {
                    if let Some(a) = self.idle_wait(t, Some(ahead.saturating_sub(limit))).await {
                        return Exit::Abort(a);
                    }
                    continue;
                }
            }

            // Data: refill from the source when the buffer is used up.
            if t.pos >= t.buf.len() {
                let cap = match (t.fmt, t.origin) {
                    (Some(fmt), Some(origin)) if t.silence_run < max_silence(fmt.rate_hz) => {
                        Some(t.written_dur().saturating_sub(now - origin))
                    }
                    _ => None,
                };
                match self.pull(t, src, cap).await {
                    Pull::Abort(a) => return Exit::Abort(a),
                    Pull::Woken => continue,
                    Pull::Starved => {
                        let Some(fmt) = t.fmt else { continue };
                        let Some(o) = out.as_mut() else { continue };
                        if t.silence_run == 0 {
                            self.shared
                                .counters
                                .underruns
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        let frames = u64::from(dur_to_frames(SLICE, fmt.rate_hz));
                        t.bytes.clear();
                        t.bytes.resize(
                            usize::try_from(frames).unwrap_or(0) * usize::from(fmt.channels) * 2,
                            0,
                        );
                        if let Err(e) = self.write(t, o.as_mut()).await {
                            return e;
                        }
                        t.written += frames;
                        t.silence_run += frames;
                        let c = &self.shared.counters;
                        c.frames_written.fetch_add(frames, Ordering::Relaxed);
                        c.silence_frames.fetch_add(frames, Ordering::Relaxed);
                        continue;
                    }
                    Pull::Item(SourceItem::End) => {
                        return Exit::End {
                            reason: EndReason::Finished,
                            drain: true,
                        };
                    }
                    Pull::Item(SourceItem::Error(m)) => {
                        return Exit::End {
                            reason: EndReason::Error(m),
                            drain: true,
                        };
                    }
                    Pull::Item(SourceItem::Pcm(p)) => {
                        let fmt = SinkFormat {
                            rate_hz: p.rate,
                            channels: p.channels,
                        };
                        if !(1..=8).contains(&fmt.channels)
                            || !(1000..=384_000).contains(&fmt.rate_hz)
                        {
                            return Exit::End {
                                reason: EndReason::Error(format!(
                                    "unsupported stream format: {} Hz, {} channels",
                                    fmt.rate_hz, fmt.channels
                                )),
                                drain: false,
                            };
                        }
                        match t.fmt {
                            None => {
                                if let Err(e) = self.open(id, t, out, fmt, level) {
                                    return e;
                                }
                            }
                            Some(cur) if cur != fmt => {
                                return Exit::End {
                                    reason: EndReason::Error(
                                        "the stream changed format".to_owned(),
                                    ),
                                    drain: true,
                                };
                            }
                            Some(_) => {}
                        }
                        let ch = usize::from(fmt.channels);
                        let mut samples = p.samples;
                        samples.truncate(samples.len() / ch * ch);
                        t.buf = samples;
                        t.pos = 0;
                        continue;
                    }
                }
            }

            // One slice: gain, write.
            let (Some(fmt), Some(o)) = (t.fmt, out.as_mut()) else {
                continue;
            };
            let ch = usize::from(fmt.channels);
            let slice = usize::try_from(dur_to_frames(SLICE, fmt.rate_hz))
                .unwrap_or(1)
                .max(1);
            let n = slice.min((t.buf.len() - t.pos) / ch);
            self.update_gain(t, level);
            let end = t.pos + n * ch;
            t.scratch.clear();
            t.scratch.extend_from_slice(&t.buf[t.pos..end]);
            t.gain.process(&mut t.scratch, ch);
            t.bytes.clear();
            t.bytes
                .extend(t.scratch.iter().flat_map(|s| s.to_le_bytes()));
            if t.origin.is_none() {
                t.origin = Some(Instant::now());
            }
            if let Err(e) = self.write(t, o.as_mut()).await {
                return e;
            }
            t.pos = end;
            t.written += n as u64;
            t.silence_run = 0;
            let c = &self.shared.counters;
            c.frames_written.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
}
