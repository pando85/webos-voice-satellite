//! The daemon: wires capture → ESPHome API server and server events → player.
//!
//! Runs on a single-threaded tokio runtime; capture and the watchdog have
//! their own OS threads.
//!
//! Microphone gate ([`MicGate`]): the PCM is open only while HA's mute switch
//! is off, the kill-switch file is absent and the firmware guard passed. The
//! kill switch and the guard force the mute switch on in HA; an unmute from HA
//! is then refused. The API connection stays up while the mic is off, so HA
//! shows the device and its mute state.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use webos_esphome_api::{AppCommand, AppEvent, PipelineMode, Server, TimerTable};

use crate::capture::{
    self, CaptureSettings, KILL_SWITCH_POLL, OFF_FIRMWARE, OFF_KILL, OFF_MUTED, OFF_NONE, Shared,
};
use crate::config::{Hang, MicSource, Mode, Settings, TestTriggers};
use crate::firmware::{self, Verdict};
use crate::player::{Done, Kind, Player, redact_url};
use crate::watchdog::{self, Beats};

/// Delay before `--test-hang` freezes a thread.
const TEST_HANG_AFTER: Duration = Duration::from_secs(5);

/// Delay between HA connecting and a `--ptt-on-connect` run (HA subscribes to
/// voice events right after the hello exchange).
const PTT_ON_CONNECT_DELAY: Duration = Duration::from_secs(2);

/// Per-run timestamps for the latency log line.
#[derive(Debug, Clone)]
struct RunTimes {
    id: u64,
    wake_word: bool,
    start: Instant,
    wake: Option<Instant>,
    stt: Option<Instant>,
    intent: Option<Instant>,
    tts: Option<Instant>,
    play_end: Option<Instant>,
    reply_audio: Option<Duration>,
    expect_tts: bool,
    ended: bool,
    error: Option<String>,
}

/// Tracks pipeline runs and produces one latency line per run.
#[derive(Debug, Default)]
pub(crate) struct RunTracker {
    runs: u64,
    cur: Option<RunTimes>,
}

fn ms(from: Instant, to: Option<Instant>) -> String {
    to.map_or_else(
        || "-".to_owned(),
        |t| t.saturating_duration_since(from).as_millis().to_string(),
    )
}

impl RunTracker {
    fn line(r: &RunTimes) -> String {
        let origin = if r.wake_word {
            r.wake.unwrap_or(r.start)
        } else {
            r.start
        };
        let mut s = format!(
            "run {} latency ms (from {}): stt_end={} intent_end={} tts_start={} playback_end={}",
            r.id,
            if r.wake_word { "wake word" } else { "start" },
            ms(origin, r.stt),
            ms(origin, r.intent),
            ms(origin, r.tts),
            ms(origin, r.play_end),
        );
        if r.wake_word {
            let _ = write!(s, " wake_after_stream_start={}", ms(r.start, r.wake));
        }
        if let Some(a) = r.reply_audio {
            let _ = write!(s, " reply_audio={}", a.as_millis());
        }
        if let Some(e) = &r.error {
            let _ = write!(s, " error={e}");
        }
        s
    }

    fn flush(&mut self) -> Option<String> {
        self.cur.take().map(|r| Self::line(&r))
    }

    /// A run starts streaming. Returns the line of an unfinished previous run.
    pub(crate) fn start(&mut self, now: Instant, wake_word: bool) -> Option<String> {
        let prev = self.flush();
        self.runs += 1;
        self.cur = Some(RunTimes {
            id: self.runs,
            wake_word,
            start: now,
            wake: None,
            stt: None,
            intent: None,
            tts: None,
            play_end: None,
            reply_audio: None,
            expect_tts: false,
            ended: false,
            error: None,
        });
        prev
    }

    fn with(&mut self, f: impl FnOnce(&mut RunTimes)) {
        if let Some(r) = self.cur.as_mut() {
            f(r);
        }
    }

    pub(crate) fn wake(&mut self, now: Instant) {
        self.with(|r| r.wake = Some(now));
    }

    pub(crate) fn stt(&mut self, now: Instant) {
        self.with(|r| r.stt = Some(now));
    }

    pub(crate) fn intent(&mut self, now: Instant, has_speech: bool) {
        self.with(|r| {
            r.intent = Some(now);
            r.expect_tts = has_speech;
        });
    }

    pub(crate) fn tts(&mut self, now: Instant) {
        self.with(|r| {
            r.tts.get_or_insert(now);
            r.expect_tts = true;
        });
    }

    pub(crate) fn error(&mut self, code: &str) {
        self.with(|r| {
            r.error = Some(code.to_owned());
            r.expect_tts = false;
        });
    }

    /// `RUN_END`: the line is ready unless a reply is still to be played.
    pub(crate) fn run_end(&mut self) -> Option<String> {
        let r = self.cur.as_mut()?;
        r.ended = true;
        if r.expect_tts && r.play_end.is_none() {
            None
        } else {
            self.flush()
        }
    }

    /// The reply finished playing.
    pub(crate) fn playback_done(
        &mut self,
        now: Instant,
        audio: Option<Duration>,
    ) -> Option<String> {
        let r = self.cur.as_mut()?;
        r.play_end = Some(now);
        r.reply_audio = audio;
        if r.ended || r.tts.is_some() {
            self.flush()
        } else {
            None
        }
    }
}

fn finished(kind: Kind, success: bool) -> AppCommand {
    match kind {
        Kind::Tts => AppCommand::PlaybackFinished { success },
        Kind::Announce => AppCommand::AnnounceFinished { success },
    }
}

async fn sleep_opt(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

async fn tick_opt(iv: Option<&mut tokio::time::Interval>) {
    match iv {
        Some(iv) => {
            iv.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// Decides whether the microphone may be open and what HA's mute switch must
/// show. Pure logic; the daemon applies the commands it returns.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct MicGate {
    /// HA's mute switch as the user set it.
    ha_muted: bool,
    /// The kill-switch file exists.
    kill: bool,
    /// The firmware guard refused this firmware.
    firmware_block: bool,
}

impl MicGate {
    /// The reason the mute switch is forced on, if any.
    pub(crate) fn forced(&self) -> Option<&'static str> {
        if self.kill {
            Some("kill switch")
        } else if self.firmware_block {
            Some("firmware guard")
        } else {
            None
        }
    }

    /// The microphone must be closed.
    pub(crate) fn mic_off(&self) -> bool {
        self.ha_muted || self.forced().is_some()
    }

    /// Value for [`Shared::mic_off`].
    pub(crate) fn off_code(&self) -> u8 {
        if self.kill {
            OFF_KILL
        } else if self.firmware_block {
            OFF_FIRMWARE
        } else if self.ha_muted {
            OFF_MUTED
        } else {
            OFF_NONE
        }
    }

    /// The kill-switch file appeared or went away. Returns the command for
    /// the API server, if one is needed.
    pub(crate) fn set_kill(&mut self, on: bool) -> Option<AppCommand> {
        if self.kill == on {
            return None;
        }
        self.kill = on;
        if on {
            Some(AppCommand::SetMuted(true))
        } else if self.mic_off() {
            None
        } else {
            Some(AppCommand::SetMuted(false))
        }
    }

    /// HA's switch changed (or the echo of our own `SetMuted`). Returns the
    /// command to send back (re-mute when an unmute is refused) and whether
    /// the user's mute state changed.
    pub(crate) fn on_mute_changed(&mut self, muted: bool) -> (Option<AppCommand>, bool) {
        if self.forced().is_some() {
            return ((!muted).then_some(AppCommand::SetMuted(true)), false);
        }
        let changed = self.ha_muted != muted;
        self.ha_muted = muted;
        (None, changed)
    }
}

/// Remembers HA's mute switch in `path` (exists = muted). Best effort.
fn store_mute(path: &Path, muted: bool) {
    let res = if muted {
        std::fs::write(path, b"")
    } else {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    };
    if let Err(e) = res {
        log::warn!("cannot update the mute file {}: {e}", path.display());
    }
}

struct Daemon {
    cmd_tx: mpsc::Sender<AppCommand>,
    shared: Arc<Shared>,
    player: Player,
    runs: RunTracker,
    connected: bool,
    gate: MicGate,
    kill_switch: PathBuf,
    mute_file: PathBuf,
    /// Timers HA reported (kept across HA reconnects, which do not replay them).
    timers: TimerTable,
}

impl Daemon {
    async fn send(&self, cmd: AppCommand) {
        if self.cmd_tx.send(cmd).await.is_err() {
            log::warn!("daemon: API server is gone");
        }
    }

    fn set_streaming(&self, on: bool) {
        self.shared.streaming.store(on, Ordering::Relaxed);
    }

    fn apply_gate(&self) {
        self.shared
            .mic_off
            .store(self.gate.off_code(), Ordering::Relaxed);
    }

    /// Polls the kill-switch file.
    async fn poll_kill_switch(&mut self) {
        let on = self.kill_switch.exists();
        if on == self.gate.kill {
            return;
        }
        let cmd = self.gate.set_kill(on);
        self.apply_gate();
        if on {
            log::warn!(
                "KILL SWITCH {} present: microphone closed, reported muted to HA",
                self.kill_switch.display()
            );
        } else if let Some(why) = self.gate.forced() {
            log::info!("kill switch removed; the microphone stays off ({why})");
        } else if self.gate.mic_off() {
            log::info!("kill switch removed; the microphone stays muted (HA's switch)");
        } else {
            log::info!("kill switch removed: microphone enabled");
        }
        if let Some(cmd) = cmd {
            self.send(cmd).await;
        }
    }

    async fn ptt(&self, source: &str) {
        if !self.connected {
            log::warn!("push-to-talk ({source}) ignored: Home Assistant is not connected");
            return;
        }
        if !self.shared.open.load(Ordering::Relaxed) {
            log::warn!("push-to-talk ({source}): the microphone is not open; HA will get no audio");
        }
        log::info!("push-to-talk run requested ({source})");
        self.send(AppCommand::StartPipeline {
            mode: PipelineMode::PushToTalk,
        })
        .await;
    }

    async fn report_stopped(&self, stopped: Option<Kind>) {
        if let Some(kind) = stopped {
            self.send(finished(kind, false)).await;
        }
    }

    fn log_line(line: Option<String>) {
        if let Some(l) = line {
            log::info!("{l}");
        }
    }

    /// Logs a timer event and keeps the table. The audible chime is not
    /// implemented (docs/development/esphome-api.md#timers).
    fn on_timer(&mut self, ev: &AppEvent) {
        let AppEvent::Timer {
            event,
            id,
            seconds_left,
            ..
        } = ev
        else {
            return;
        };
        // id and seconds only: the timer name is user speech. For FINISHED
        // this line reads "timer finished id=...".
        log::info!("timer {event} id={id} seconds_left={seconds_left}");
        self.timers.apply(ev, std::time::Instant::now());
        log::debug!("timers known: {}", self.timers.len());
    }

    #[allow(clippy::too_many_lines)] // one arm per event, flat on purpose
    async fn on_event(&mut self, ev: AppEvent) {
        let now = Instant::now();
        if matches!(ev, AppEvent::Timer { .. }) {
            self.on_timer(&ev);
            return;
        }
        match ev {
            AppEvent::HaConnected { client_info, peer } => {
                self.connected = true;
                log::info!("HA connected: {client_info} from {}", peer.ip());
            }
            AppEvent::HaDisconnected => {
                self.connected = false;
                self.set_streaming(false);
                log::warn!("HA disconnected");
                // Playback was already stopped through StopPlayback if it
                // needed HA; a fully received reply keeps playing.
                if !self.player.is_active() {
                    Self::log_line(self.runs.flush());
                }
            }
            AppEvent::StartStreaming { wake_word, .. } => {
                self.set_streaming(true);
                log::info!(
                    "streaming mic audio to HA ({})",
                    if wake_word {
                        "wake word stage"
                    } else {
                        "speech-to-text"
                    }
                );
                Self::log_line(self.runs.start(now, wake_word));
            }
            AppEvent::StopStreaming => {
                self.set_streaming(false);
                log::info!("stopped streaming mic audio");
            }
            AppEvent::WakeWordDetected => {
                log::info!("wake word detected by HA");
                self.runs.wake(now);
            }
            AppEvent::Transcript(text) => {
                log::info!("speech-to-text done ({} chars)", text.chars().count());
                log::debug!("transcript: {text:?}");
                self.runs.stt(now);
            }
            AppEvent::IntentEnd {
                speech,
                continue_conversation,
            } => {
                log::info!(
                    "intent done (reply {} chars, continue_conversation={continue_conversation})",
                    speech.chars().count()
                );
                log::debug!("reply text: {speech:?}");
                self.runs.intent(now, !speech.is_empty());
            }
            AppEvent::TtsStreamStart {
                sample_rate,
                channels,
                sample_width,
            } => {
                log::info!("TTS stream start ({sample_rate} Hz, {channels} ch, {sample_width} B)");
                if sample_width != 2 {
                    log::warn!("unexpected TTS sample width {sample_width}; playing as s16le");
                }
                self.runs.tts(now);
                let stopped = self.player.start_stream(sample_rate, channels);
                self.report_stopped(stopped).await;
            }
            AppEvent::TtsAudio(data) => self.player.push(data),
            AppEvent::TtsStreamEnd => {
                log::debug!("TTS stream end");
                self.player.end_stream();
            }
            AppEvent::TtsUrl(url) => {
                log::info!("TTS URL {}", redact_url(&url));
                self.runs.tts(now);
                let stopped = self.player.play_url(url, None, Kind::Tts);
                self.report_stopped(stopped).await;
            }
            AppEvent::Announce {
                url,
                preannounce_url,
                text,
                start_conversation,
            } => {
                log::info!(
                    "announcement {} ({} chars, start_conversation={start_conversation})",
                    redact_url(&url),
                    text.chars().count()
                );
                log::debug!("announcement text: {text:?}");
                let stopped = self.player.play_url(url, preannounce_url, Kind::Announce);
                self.report_stopped(stopped).await;
            }
            AppEvent::StopPlayback => {
                if let Some(kind) = self.player.stop() {
                    self.send(finished(kind, false)).await;
                }
            }
            AppEvent::RunEnd => {
                log::info!("pipeline run end");
                Self::log_line(self.runs.run_end());
            }
            AppEvent::Error { code, message } => {
                log::warn!("HA pipeline error {code}");
                log::debug!("HA pipeline error message: {message:?}");
                self.runs.error(&code);
            }
            AppEvent::MuteChanged(muted) => {
                if muted {
                    self.set_streaming(false);
                }
                let (cmd, changed) = self.gate.on_mute_changed(muted);
                self.apply_gate();
                if let Some(cmd) = cmd {
                    log::warn!(
                        "unmute refused: {} is active; the microphone stays off",
                        self.gate.forced().unwrap_or("a safety gate")
                    );
                    self.send(cmd).await;
                } else if changed {
                    store_mute(&self.mute_file, muted);
                    log::info!(
                        "microphone {} by Home Assistant",
                        if muted { "MUTED" } else { "unmuted" }
                    );
                } else {
                    log::debug!("mute state {muted} confirmed");
                }
            }
            other => log::debug!("event {other:?}"),
        }
    }

    async fn on_done(&mut self, done: Done) {
        if !self.player.finish(&done) {
            log::debug!("player: late report of playback {} ignored", done.id);
            return;
        }
        let now = Instant::now();
        let success = match &done.result {
            Ok(st) => {
                log::info!(
                    "playback {} done: {} ms of audio, {} bytes, {} non-zero samples{}",
                    done.id,
                    st.audio.as_millis(),
                    st.bytes,
                    st.nonzero_samples,
                    st.file
                        .as_ref()
                        .map(|f| format!(", file {}", f.display()))
                        .unwrap_or_default()
                );
                true
            }
            Err(e) => {
                log::warn!("playback {} failed: {e}", done.id);
                false
            }
        };
        if done.kind == Kind::Tts {
            let audio = done.result.as_ref().ok().map(|s| s.audio);
            Self::log_line(self.runs.playback_done(now, audio));
        }
        self.send(finished(done.kind, success)).await;
    }
}

/// Runs the daemon until SIGTERM/SIGINT.
#[allow(clippy::too_many_lines)] // linear setup, loop and shutdown
pub(crate) async fn run(settings: Settings, triggers: TestTriggers) -> Result<(), String> {
    let esp = settings.esphome_config().map_err(|e| e.to_string())?;
    let server = Server::bind(esp).await.map_err(|e| e.to_string())?;
    log::info!(
        "webos-satellite {} ({}) ESPHome API on {}",
        crate::VERSION,
        crate::TARGET,
        server.local_addr().map_err(|e| e.to_string())?
    );
    log::info!("config: {}", settings.summary());
    if triggers.ptt_on_connect || triggers.ptt_every.is_some() || triggers.hang.is_some() {
        log::warn!("test triggers enabled: {triggers:?}");
    }

    let mut sigterm = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    let mut sigint = signal(SignalKind::interrupt()).map_err(|e| e.to_string())?;
    let mut sigusr1 = signal(SignalKind::user_defined1()).map_err(|e| e.to_string())?;

    // Microphone gate: firmware guard, kill switch and the remembered mute.
    let verdict = firmware::check(&settings);
    let gated = settings.mic.source == MicSource::Alsa;
    match &verdict {
        Verdict::Validated(_) => log::info!("firmware guard: {verdict}"),
        Verdict::Off => log::warn!("{verdict}"),
        _ if !gated => log::info!("firmware guard (not applied to a file mic): {verdict}"),
        _ => log::error!("FIRMWARE GUARD: {verdict}"),
    }
    let gate = MicGate {
        ha_muted: settings.safety.mute_file.exists(),
        kill: settings.safety.kill_switch.exists(),
        firmware_block: gated && !verdict.allows_capture(),
    };
    if gate.kill {
        log::warn!(
            "KILL SWITCH {} present at startup: the microphone stays closed until it is removed",
            settings.safety.kill_switch.display()
        );
    }
    if gate.ha_muted {
        log::info!(
            "microphone muted (remembered in {})",
            settings.safety.mute_file.display()
        );
    }
    // The echo of this SetMuted(true) is not a user change (see MicGate).
    let start_muted = gate.mic_off();

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let shared = Arc::new(Shared::default());
    shared.mic_off.store(gate.off_code(), Ordering::Relaxed);
    let cap = capture::spawn(
        CaptureSettings::from_settings(&settings),
        Arc::clone(&shared),
        cmd_tx.clone(),
    )
    .map_err(|e| format!("cannot start the capture thread: {e}"))?;
    let beats = Arc::new(Beats::default());
    {
        let shared = Arc::clone(&shared);
        watchdog::spawn(
            Duration::from_secs(u64::from(settings.limits.loop_stall_s)),
            u64::from(settings.limits.max_rss_mb) << 20,
            Arc::clone(&beats),
            move || shared.beat.load(Ordering::Relaxed),
        )
        .map_err(|e| format!("cannot start the watchdog thread: {e}"))?;
    }
    if settings.mode == Mode::Wake {
        let _ = cmd_tx
            .send(AppCommand::StartPipeline {
                mode: PipelineMode::WakeWord,
            })
            .await;
    }
    // After the wake word start: a start while muted is ignored by the voice
    // layer, but an armed wake word re-arms on unmute.
    if start_muted {
        let _ = cmd_tx.send(AppCommand::SetMuted(true)).await;
    }
    let server_task = tokio::spawn(server.run(ev_tx, cmd_rx));

    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let mut d = Daemon {
        cmd_tx,
        shared: Arc::clone(&shared),
        player: Player::new(settings.sink.clone(), done_tx),
        runs: RunTracker::default(),
        connected: false,
        gate,
        kill_switch: settings.safety.kill_switch.clone(),
        mute_file: settings.safety.mute_file.clone(),
        timers: TimerTable::new(),
    };
    let mut kill_poll = tokio::time::interval(KILL_SWITCH_POLL);
    kill_poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut ptt_timer = triggers.ptt_every.map(|p| {
        let mut iv = tokio::time::interval_at(Instant::now() + p, p);
        iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
        iv
    });
    let mut hang_at = triggers.hang.map(|_| Instant::now() + TEST_HANG_AFTER);
    let mut ptt_on_connect_at: Option<Instant> = None;
    let mut ptt_on_connect_done = false;

    loop {
        tokio::select! {
            ev = ev_rx.recv() => {
                let Some(ev) = ev else {
                    log::error!("API server stopped");
                    break;
                };
                let connect = matches!(ev, AppEvent::HaConnected { .. });
                d.on_event(ev).await;
                if connect && triggers.ptt_on_connect && !ptt_on_connect_done {
                    ptt_on_connect_at = Some(Instant::now() + PTT_ON_CONNECT_DELAY);
                }
            }
            Some(done) = done_rx.recv() => d.on_done(done).await,
            _ = sigterm.recv() => {
                log::info!("SIGTERM: shutting down");
                break;
            }
            _ = sigint.recv() => {
                log::info!("SIGINT: shutting down");
                break;
            }
            _ = sigusr1.recv() => d.ptt("SIGUSR1").await,
            _ = kill_poll.tick() => {
                beats.event_loop.fetch_add(1, Ordering::Relaxed);
                d.poll_kill_switch().await;
            }
            () = tick_opt(ptt_timer.as_mut()) => d.ptt("--ptt-every").await,
            () = sleep_opt(hang_at) => {
                hang_at = None;
                match triggers.hang {
                    Some(Hang::Capture) => {
                        log::warn!("--test-hang: freezing the capture thread");
                        shared.test_hang.store(true, Ordering::Relaxed);
                    }
                    Some(Hang::Loop) => {
                        log::warn!("--test-hang: freezing the event loop");
                        loop {
                            std::thread::sleep(Duration::from_secs(1));
                        }
                    }
                    None => {}
                }
            }
            () = sleep_opt(ptt_on_connect_at) => {
                ptt_on_connect_at = None;
                ptt_on_connect_done = true;
                d.ptt("--ptt-on-connect").await;
            }
        }
    }

    // Shutdown: stop streaming and playback (kills pacat), release the mic,
    // then close the command channel so the server says goodbye to HA.
    beats.disarmed.store(true, Ordering::Relaxed);
    d.set_streaming(false);
    d.player.shutdown().await;
    Daemon::log_line(d.runs.flush());
    shared.stop.store(true, Ordering::Relaxed);
    let joined = tokio::task::spawn_blocking(move || cap.join()).await;
    if !matches!(joined, Ok(Ok(()))) {
        log::warn!("capture thread did not stop cleanly");
    }
    log::info!(
        "capture released (chunks sent {}, dropped {}, xruns {})",
        shared.chunks_sent.load(Ordering::Relaxed),
        shared.chunks_dropped.load(Ordering::Relaxed),
        shared.xruns.load(Ordering::Relaxed)
    );
    drop(d);
    match tokio::time::timeout(Duration::from_secs(3), server_task).await {
        Ok(_) => log::info!("stopped"),
        Err(_) => log::warn!("API server did not stop within 3 s"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn wake_run_with_streamed_reply() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut r = RunTracker::default();
        assert!(r.start(at(0), true).is_none());
        r.wake(at(5000));
        r.stt(at(6000));
        r.intent(at(7000), true);
        // RUN_END before the stream starts: wait for playback.
        assert!(r.run_end().is_none());
        r.tts(at(7100));
        let line = r
            .playback_done(at(9100), Some(Duration::from_millis(1800)))
            .unwrap();
        assert_eq!(
            line,
            "run 1 latency ms (from wake word): stt_end=1000 intent_end=2000 tts_start=2100 playback_end=4100 wake_after_stream_start=5000 reply_audio=1800"
        );
        assert!(r.flush().is_none());
    }

    #[test]
    fn ptt_run_with_error_and_no_reply() {
        let t0 = Instant::now();
        let mut r = RunTracker::default();
        r.start(t0, false);
        r.error("stt-no-text-recognized");
        let line = r.run_end().unwrap();
        assert!(
            line.starts_with("run 1 latency ms (from start): stt_end=-"),
            "{line}"
        );
        assert!(line.ends_with("error=stt-no-text-recognized"), "{line}");
    }

    #[test]
    fn unfinished_run_is_flushed_by_the_next() {
        let t0 = Instant::now();
        let mut r = RunTracker::default();
        r.start(t0, true);
        let prev = r.start(t0, false).unwrap();
        assert!(prev.starts_with("run 1 "));
        r.intent(t0, false);
        assert!(r.run_end().unwrap().starts_with("run 2 "));
    }

    #[test]
    fn kill_switch_forces_mute_and_restores_the_users_state() {
        let mut g = MicGate::default();
        assert!(!g.mic_off());
        assert_eq!(g.set_kill(true), Some(AppCommand::SetMuted(true)));
        assert!(g.mic_off());
        // The echo of our own SetMuted(true) is not a user change.
        assert_eq!(g.on_mute_changed(true), (None, false));
        // HA tries to unmute: refused, re-muted.
        assert_eq!(
            g.on_mute_changed(false),
            (Some(AppCommand::SetMuted(true)), false)
        );
        assert_eq!(g.set_kill(true), None);
        // Removed: back to unmuted.
        assert_eq!(g.set_kill(false), Some(AppCommand::SetMuted(false)));
        assert_eq!(g.on_mute_changed(false), (None, false));
        assert!(!g.mic_off());
    }

    #[test]
    fn a_user_mute_survives_the_kill_switch() {
        let mut g = MicGate::default();
        assert_eq!(g.on_mute_changed(true), (None, true));
        assert!(g.mic_off());
        assert_eq!(g.set_kill(true), Some(AppCommand::SetMuted(true)));
        assert_eq!(g.set_kill(false), None);
        assert!(g.mic_off());
        assert_eq!(g.on_mute_changed(false), (None, true));
        assert!(!g.mic_off());
    }

    #[test]
    fn firmware_block_cannot_be_unmuted_or_cleared_by_the_kill_switch() {
        let mut g = MicGate {
            firmware_block: true,
            ..MicGate::default()
        };
        assert!(g.mic_off());
        assert_eq!(g.forced(), Some("firmware guard"));
        assert_eq!(
            g.on_mute_changed(false),
            (Some(AppCommand::SetMuted(true)), false)
        );
        assert_eq!(g.set_kill(true), Some(AppCommand::SetMuted(true)));
        assert_eq!(g.set_kill(false), None);
        assert!(g.mic_off());
    }

    #[test]
    fn mute_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("webos-sat-mute-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("muted");
        store_mute(&f, true);
        assert!(f.exists());
        store_mute(&f, false);
        assert!(!f.exists());
        store_mute(&f, false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finished_maps_kinds() {
        assert_eq!(
            finished(Kind::Tts, true),
            AppCommand::PlaybackFinished { success: true }
        );
        assert_eq!(
            finished(Kind::Announce, false),
            AppCommand::AnnounceFinished { success: false }
        );
    }
}
