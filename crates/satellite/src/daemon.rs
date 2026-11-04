//! The daemon: wires capture → ESPHome API server and server events → player.
//!
//! Runs on a single-threaded tokio runtime; capture and the watchdog have
//! their own OS threads.
//!
//! Microphone gate ([`MicGate`]): the PCM is open only while HA's mute switch
//! is off, the kill-switch file is absent, the strict firmware gate (opt-in,
//! `safety.firmware_policy = "strict"`) passed and, with `[pairing]`, a key is
//! active. The kill switch and the gate force the mute switch on in HA; an
//! unmute from HA is then refused. The API connection stays up while the mic
//! is off, so HA shows the device and its mute state. By default (`warn`) a
//! firmware that is not verified never closes the microphone: the status
//! entities and the self-check say so instead ([`crate::compat`], ADR 0006).
//!
//! Pairing (`[pairing]`): the API server owns the key state; the daemon
//! prepares the state directory, applies `--reset-pairing`, polls the `pair`
//! and `reset-pairing` trigger files and keeps the microphone closed until
//! the server reports [`PairingEvent::Paired`].

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use webos_esphome_api::{
    AppCommand, AppEvent, PairingEvent, PipelineMode, Server, TimerEvent, TimerTable,
};

use crate::alarm::{self, Alarm, Phase, Step};
use crate::capture::{
    self, CaptureSettings, KILL_SWITCH_POLL, OFF_FIRMWARE, OFF_KILL, OFF_MUTED, OFF_NONE,
    OFF_UNPAIRED, Shared,
};
use crate::compat::{self, Compat, ModelState};
use crate::config::{Hang, MicSource, Mode, RemoteOff, Settings, TestTriggers};
use crate::darken::{self, Action, Darkener};
use crate::firmware::{self, Model, Verdict};
use crate::player::{Done, Kind, Player, redact_url};
use crate::power::{self, PowerUpdate};
use crate::selfcheck::Note;
use crate::tv_control::{self, Tv, TvMsg};
use crate::volume::{self, SharedGain};
use crate::watchdog::{self, Beats};
use crate::{resume, volume::Origin};

/// Two resume signals (the clock jump, the power state's `Prepare Resume`)
/// this close together are one resume.
const RESUME_DEDUP: Duration = Duration::from_secs(30);
/// tvpower's luna methods used by wake-and-darken.
const TVPOWER: &str = "luna://com.webos.service.tvpower/power";
/// The remote's power key as a toggle (fallback wake, D6 A3).
const POWER_KEY_URI: &str = "luna://com.webos.service.networkinput/controls/power";
/// `powerOn` attempts while tvpowerd answers `Keyblock1.5second`.
const POWER_ON_TRIES: u32 = 3;
/// Pause before a `powerOn` retry (the key block is 1.5 s).
const POWER_ON_RETRY: Duration = Duration::from_millis(1600);

/// The pairing trigger files are checked this often.
const PAIRING_POLL: Duration = Duration::from_secs(1);

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
    /// When the error event arrived (an intent failure has no `intent_end`).
    error_at: Option<Instant>,
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
            let _ = write!(s, " error={e} error_at={}", ms(origin, r.error_at));
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
            error_at: None,
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

    pub(crate) fn error(&mut self, now: Instant, code: &str) {
        self.with(|r| {
            r.error = Some(code.to_owned());
            r.error_at = Some(now);
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

/// A luna reply for the log.
fn reply(r: &Result<String, String>) -> String {
    match r {
        Ok(t) if t.is_empty() => "no output".to_owned(),
        Ok(t) => t.chars().take(160).collect(),
        Err(e) => format!("failed: {e}"),
    }
}

/// What HA is told when a playback ends; the timer alarm is local and HA
/// is told nothing.
fn finished(kind: Kind, success: bool) -> Option<AppCommand> {
    match kind {
        Kind::Tts => Some(AppCommand::PlaybackFinished { success }),
        Kind::Announce => Some(AppCommand::AnnounceFinished { success }),
        Kind::Alarm => None,
    }
}

pub(crate) async fn sleep_opt(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

/// Receives from an optional channel; never completes without one.
async fn recv_opt<T>(rx: Option<&mut mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Starts TV control when `[tv_control]` has anything on and `luna-send` is
/// there (the power watch runs): the read-only capability check, then the
/// entities are added to the server configuration. TV control is not gated by
/// the firmware status (owner decision 2026-11-04): on an unverified firmware it
/// runs with a warning, unless `tv_control.follow_firmware_guard` is set.
async fn start_tv_control(
    settings: &Settings,
    luna: Option<&PathBuf>,
    verdict: &Verdict,
    esp: &mut webos_esphome_api::Config,
) -> Option<tv_control::Started> {
    if !settings.tv_control.any() {
        log::info!("tv_control: off ([tv_control] enabled = false or nothing switched on)");
        return None;
    }
    let Some(luna) = luna else {
        log::info!(
            "tv_control: not started: luna-send was not found or power.watch is off (normal off the TV)"
        );
        return None;
    };
    let unverified = !matches!(verdict, Verdict::Verified(..) | Verdict::Off);
    let blocked = if settings.tv_control.follow_firmware_guard && unverified {
        let why = match verdict {
            Verdict::NotVerified { info, .. } | Verdict::SeriesOnly(info, _) => {
                format!("firmware {} is not verified", info.version)
            }
            _ => "the firmware version cannot be read".to_owned(),
        };
        log::warn!(
            "tv_control: TV control SETTERS ARE OFF ({why}; tv_control.follow_firmware_guard = true); sensors and states still report"
        );
        Some(why)
    } else {
        match verdict {
            Verdict::NotVerified { info, .. } | Verdict::SeriesOnly(info, _) => log::warn!(
                "tv_control: firmware {} is not verified; TV control runs anyway (owner decision 2026-11-04: not gated by the firmware status; each control is checked at startup and a failing one is switched off)",
                info.version
            ),
            Verdict::Unreadable { .. } => log::warn!(
                "tv_control: the firmware version cannot be read; TV control runs anyway (not gated by the firmware status)"
            ),
            _ => {}
        }
        None
    };
    let mut started = tv_control::start(
        settings,
        luna.clone(),
        blocked,
        Path::new(tv_control::DECLARATION_DIR),
    )
    .await;
    std::mem::take(&mut started.entities).add_to(esp);
    Some(started)
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
#[allow(clippy::struct_excessive_bools)] // one bool per independent gate
pub(crate) struct MicGate {
    /// HA's mute switch as the user set it.
    ha_muted: bool,
    /// The kill-switch file exists.
    kill: bool,
    /// The firmware guard refused this firmware.
    firmware_block: bool,
    /// Pairing is enabled and no key is active: nobody may listen yet.
    unpaired: bool,
}

impl MicGate {
    /// The reason the mute switch is forced on, if any.
    pub(crate) fn forced(&self) -> Option<&'static str> {
        if self.kill {
            Some("kill switch")
        } else if self.firmware_block {
            Some("firmware guard")
        } else if self.unpaired {
            Some("not paired")
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
        } else if self.unpaired {
            OFF_UNPAIRED
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

    /// Pairing state changed (`true`: no key active). Returns the command for
    /// the API server, if one is needed (as [`MicGate::set_kill`]).
    pub(crate) fn set_unpaired(&mut self, on: bool) -> Option<AppCommand> {
        if self.unpaired == on {
            return None;
        }
        self.unpaired = on;
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

/// Deletes a trigger file. True if it existed and is gone (act on it once).
fn consume_trigger(path: &Path, warned: &mut bool) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            if !*warned {
                *warned = true;
                log::warn!(
                    "pairing trigger {} cannot be removed ({e}); ignored",
                    path.display()
                );
            }
            false
        }
    }
}

/// Creates the pairing state directory (mode 0700) and applies the startup
/// actions: `--reset-pairing` or a `reset-pairing` file deletes the stored
/// key; a `pair` file opens the window (as `--pair`).
fn prepare_pairing(settings: &mut Settings) -> Result<(), String> {
    let p = &settings.pairing;
    if !p.state_dir.is_dir() {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            b.mode(0o700);
        }
        b.create(&p.state_dir).map_err(|e| {
            format!(
                "cannot create pairing.state_dir {}: {e}",
                p.state_dir.display()
            )
        })?;
    }
    let mut warned = false;
    let reset = consume_trigger(&p.reset_trigger(), &mut warned) || p.reset_now;
    let pair = consume_trigger(&p.pair_trigger(), &mut warned);
    if reset {
        let key = p.key_file();
        match webos_esphome_api::pairing::remove_key(&key) {
            Ok(true) => log::warn!("PAIRING RESET: stored key {} deleted", key.display()),
            Ok(false) => log::info!("pairing reset: no stored key"),
            Err(e) => return Err(format!("cannot delete {}: {e}", key.display())),
        }
        // A reset always reopens the window.
        settings.pairing.open_now = true;
    }
    if pair {
        settings.pairing.open_now = true;
    }
    if settings.pairing.open_now {
        // A local open (flag, file or reset) ends the closed state that a
        // remote clear by the key holder leaves behind.
        let key = settings.pairing.key_file();
        webos_esphome_api::pairing::forget_cleared(&key).map_err(|e| {
            format!(
                "cannot remove {}: {e}",
                webos_esphome_api::pairing::cleared_marker(&key).display()
            )
        })?;
    }
    Ok(())
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

#[allow(clippy::struct_excessive_bools)] // independent flags of the daemon's state
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
    /// The alarm that sounds when a timer finishes.
    alarm: Alarm,
    /// `[device] timers`: the timer entities exist.
    timers_on: bool,
    /// Last states sent for the timer sensors (only changes are sent).
    last_timers_text: Option<String>,
    last_ringing: Option<bool>,
    /// `pair` and `reset-pairing` trigger files, with pairing enabled.
    pairing_triggers: Option<(PathBuf, PathBuf)>,
    trigger_warned: bool,
    /// Reply volume gain shared with the player.
    volume_gain: SharedGain,
    volume_file: PathBuf,
    /// `luna-send`, when the power watch runs.
    luna: Option<PathBuf>,
    /// Last power state update (for change-only logging and the sensor).
    last_power: Option<PowerUpdate>,
    darkener: Darkener,
    remote_off_file: PathBuf,
    resume: resume::Detector,
    last_resume: Option<Instant>,
    /// TV control (screen, power, volume, input, apps), when `luna-send` is
    /// there and `[tv_control]` is on.
    tv: Option<Tv>,
    /// A conversation is in progress (wake word heard or push-to-talk until
    /// the run ends): list changes of TV control wait.
    in_run: bool,
    /// Compatibility status, report and status files (ADR 0006).
    compat: Compat,
    /// Directory of the trigger files `selfcheck-run` and `firmware-ack`.
    report_dir: PathBuf,
    /// Counts kill-switch polls: the compat triggers are checked once a second.
    compat_poll: u32,
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

    /// Sends every compatibility entity state and rewrites the status files
    /// when something changed.
    async fn publish_compat(&mut self) {
        let _ = self.compat.refresh_files();
        for cmd in self.compat.commands() {
            self.send(cmd).await;
        }
    }

    /// A note from the capture thread (self-check results).
    async fn on_note(&mut self, note: Note) {
        self.compat.on_note(note);
        self.publish_compat().await;
    }

    /// The HA button or `ctl.sh selfcheck`: the capture thread runs the check
    /// again, on the open stream when the microphone is open.
    fn request_selfcheck(&self, source: &str) {
        if self.compat.self_check_on() {
            log::info!("self-check: re-run requested ({source})");
            self.shared.selfcheck_rerun.store(true, Ordering::Relaxed);
        } else {
            log::info!("self-check: re-run requested ({source}) but safety.self_check = false");
        }
    }

    /// The HA button or `ctl.sh ack-firmware`.
    async fn acknowledge_firmware(&mut self, source: &str) {
        let line = self.compat.acknowledge();
        log::info!("{line} ({source})");
        self.publish_compat().await;
    }

    /// The trigger files of `ctl.sh`, once a second.
    async fn poll_compat_triggers(&mut self) {
        self.compat_poll = self.compat_poll.wrapping_add(1);
        if self.compat_poll % 4 != 0 {
            return;
        }
        let mut warned = self.trigger_warned;
        if consume_trigger(
            &self.report_dir.join(compat::SELFCHECK_TRIGGER),
            &mut warned,
        ) {
            self.request_selfcheck("ctl.sh");
        }
        if consume_trigger(&self.report_dir.join(compat::ACK_TRIGGER), &mut warned) {
            self.acknowledge_firmware("ctl.sh").await;
        }
        self.trigger_warned = warned;
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

    /// Acts on the `reset-pairing` and `pair` trigger files.
    async fn poll_pairing(&mut self) {
        let Some((pair, reset)) = self.pairing_triggers.clone() else {
            return;
        };
        if consume_trigger(&reset, &mut self.trigger_warned) {
            log::warn!("{} found: resetting the pairing", reset.display());
            self.send(AppCommand::ResetPairing).await;
        } else if consume_trigger(&pair, &mut self.trigger_warned) {
            log::info!("{} found: opening the pairing window", pair.display());
            self.send(AppCommand::OpenPairingWindow).await;
        }
    }

    async fn set_unpaired(&mut self, on: bool) {
        let cmd = self.gate.set_unpaired(on);
        self.apply_gate();
        if let Some(cmd) = cmd {
            self.send(cmd).await;
        }
    }

    async fn on_pairing(&mut self, ev: PairingEvent) {
        match ev {
            PairingEvent::Paired { client_info, peer } => {
                log::warn!(
                    "PAIRED by {client_info:?} from {}; if that is not your Home Assistant, reset the pairing",
                    peer.ip()
                );
                self.set_unpaired(false).await;
                if self.gate.mic_off() {
                    log::info!(
                        "the microphone stays off ({})",
                        self.gate.forced().unwrap_or("muted")
                    );
                }
            }
            PairingEvent::KeyRotated => log::info!("pairing key replaced by the key holder"),
            PairingEvent::Reset => {
                self.set_streaming(false);
                log::warn!("pairing reset: microphone closed until paired again");
                self.set_unpaired(true).await;
            }
            PairingEvent::Cleared { client_info, peer } => {
                self.set_streaming(false);
                log::warn!(
                    "pairing CLEARED by {client_info:?} from {} (the device was removed in Home Assistant): microphone closed; to pair again, open the pairing window locally (--pair or the pair file)",
                    peer.ip()
                );
                self.set_unpaired(true).await;
            }
            PairingEvent::WindowOpened { seconds } => {
                log::info!("pairing window open for {seconds} s");
            }
            PairingEvent::WindowClosed => log::info!("pairing window closed"),
            other => log::debug!("pairing event {other:?}"),
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

    async fn send_finished(&self, kind: Kind, success: bool) {
        if let Some(cmd) = finished(kind, success) {
            self.send(cmd).await;
        }
    }

    /// A playback was stopped to make room for a new one. A ringing timer
    /// alarm is not failed to HA: it waits and resumes afterwards.
    async fn report_stopped(&mut self, stopped: Option<Kind>) {
        match stopped {
            Some(Kind::Alarm) => {
                self.alarm.preempted();
                log::info!("timer alarm paused: a reply or announcement is playing");
                self.publish_timers().await;
            }
            Some(kind) => self.send_finished(kind, false).await,
            None => {}
        }
    }

    fn log_line(line: Option<String>) {
        if let Some(l) = line {
            log::info!("{l}");
        }
    }

    /// HA set the reply volume: apply it to the next samples and store it.
    fn on_volume(&self, percent: u8) {
        self.volume_gain.set(volume::gain_for(percent));
        log::info!(
            "reply volume {percent} % ({}) set by Home Assistant",
            volume::db_for(percent).map_or_else(|| "silent".to_owned(), |db| format!("{db:.1} dB"))
        );
        if let Err(e) = volume::store(&self.volume_file, percent) {
            log::warn!(
                "cannot store the reply volume in {}: {e} (it is lost at the next restart)",
                self.volume_file.display()
            );
        }
    }

    /// HA chose a select option.
    fn on_select(&mut self, object_id: &str, option: &str) {
        if object_id != darken::SELECT_OBJECT_ID {
            return;
        }
        let Some(mode) = darken::mode_for(option) else {
            return;
        };
        self.darkener.set_mode(mode);
        match mode {
            RemoteOff::KeepListening => log::warn!(
                "remote power-off set to KEEP LISTENING by Home Assistant: after the remote turns the TV off the satellite wakes it and turns the screen off (the TV stays on with a dark screen; more power than standby, OLED panel care may not run)"
            ),
            RemoteOff::Normal => {
                log::info!("remote power-off set to turn off normally by Home Assistant");
            }
        }
        let word = darken::file_word(mode);
        if let Err(e) = volume::write_atomic(&self.remote_off_file, &format!("{word}\n")) {
            log::warn!(
                "cannot store the remote power-off setting in {}: {e}",
                self.remote_off_file.display()
            );
        }
    }

    /// A power state update: log, report, resume and wake-and-darken.
    async fn on_power(&mut self, u: PowerUpdate) {
        let now = Instant::now();
        let changed = self.last_power.as_ref() != Some(&u);
        if changed {
            log::info!("power: {}", u.describe());
        }
        let text_changed = self
            .last_power
            .as_ref()
            .is_none_or(|l| l.text() != u.text());
        // A processing-only update keeps the last known state in the text.
        let mut shown = u.clone();
        if shown.state.is_none() {
            shown.state = self.last_power.as_ref().and_then(|l| l.state.clone());
        }
        if text_changed {
            self.send(AppCommand::SetPowerState(shown.text())).await;
        }
        if let Some(state) = shown.state.as_deref() {
            self.compat
                .on_power_state(state, self.shared.open.load(Ordering::Relaxed));
            if changed {
                self.publish_compat().await;
            }
        }
        if u.processing_key() == "PrepareResume" {
            self.on_resume("the TV reported Prepare Resume").await;
        }
        let inhibit = self.gate.kill.then_some("the kill switch is on");
        let action = self.darkener.on_update(now, &shown, inhibit);
        let tv_out = self
            .tv
            .as_mut()
            .map(|tv| tv.on_power(&shown.state_key(), !shown.processing_key().is_empty()));
        self.last_power = Some(shown);
        self.run_action(action);
        if let Some(out) = tv_out {
            self.apply_tv(out).await;
        }
    }

    /// Sends what TV control produced: states and options for HA, and the
    /// darkener's note that the satellite woke the TV itself.
    async fn apply_tv(&mut self, out: tv_control::Outcome) {
        if out.own_wake {
            self.darkener.note_own_wake();
        }
        for cmd in out.commands {
            self.send(cmd).await;
        }
    }

    async fn on_tv_msg(&mut self, msg: TvMsg) {
        let out = self.tv.as_mut().map(|tv| tv.on_msg(msg));
        if let Some(out) = out {
            self.apply_tv(out).await;
        }
    }

    /// What the power-off of `tv_power` needs from the daemon (design
    /// section 4.2): the select, the kill switch and the darkener's verdict.
    fn tv_env(&self) -> webos_tv_control::PowerOffEnv {
        webos_tv_control::PowerOffEnv {
            keep_listening: self.darkener.mode() == RemoteOff::KeepListening,
            kill_switch: self.gate.kill,
            darkener: self.darkener.would_act(Instant::now(), None),
        }
    }

    /// Drives TV control's clock and applies new select options when no
    /// conversation or reply is running (HA reconnects to list them).
    async fn poll_tv(&mut self) {
        let env = self.tv_env();
        let Some(tv) = self.tv.as_mut() else {
            return;
        };
        let out = tv.tick(env);
        let pending = tv.pending_options();
        let results = tv.control_results();
        if self.compat.set_tv_control(results) {
            self.publish_compat().await;
        }
        self.apply_tv(out).await;
        if pending.is_empty() || self.in_run || self.player.is_active() {
            return;
        }
        for (id, options) in pending {
            log::info!("tv_control: new options for {id}; Home Assistant lists the entities again");
            self.send(AppCommand::SetSelectOptions {
                object_id: id.to_owned(),
                options,
            })
            .await;
            let out = self.tv.as_mut().map(|tv| tv.options_applied(id));
            if let Some(out) = out {
                self.apply_tv(out).await;
            }
        }
    }

    /// Runs a wake-and-darken call in the background (logged).
    fn run_action(&self, action: Option<Action>) {
        let (Some(action), Some(luna)) = (action, self.luna.clone()) else {
            return;
        };
        tokio::spawn(async move {
            match action {
                Action::Wake => {
                    let uri = format!("{TVPOWER}/powerOn");
                    for n in 1..=POWER_ON_TRIES {
                        let r = power::call(&luna, &uri, r#"{"reason":"remoteKey"}"#).await;
                        log::info!("remote power-off: powerOn try {n}: {}", reply(&r));
                        match r {
                            Ok(t) if t.contains("Keyblock") => {
                                tokio::time::sleep(POWER_ON_RETRY).await;
                            }
                            _ => break,
                        }
                    }
                }
                Action::Key => {
                    let r = power::call(&luna, POWER_KEY_URI, "{}").await;
                    log::info!("remote power-off: power key: {}", reply(&r));
                }
                Action::Darken => {
                    let r = power::call(&luna, &format!("{TVPOWER}/turnOffScreen"), "{}").await;
                    log::info!("remote power-off: turnOffScreen: {}", reply(&r));
                }
            }
        });
    }

    /// The system resumed (clock jump or power state). Deduplicated.
    async fn on_resume(&mut self, why: &str) {
        let now = Instant::now();
        if self
            .last_resume
            .is_some_and(|t| now.saturating_duration_since(t) < RESUME_DEDUP)
        {
            log::debug!("resume signal ignored (already handled): {why}");
            return;
        }
        self.last_resume = Some(now);
        log::info!(
            "RESUMED from a system suspend ({why}); capture recovers by itself (xruns so far {}); checking the HA connection",
            self.shared.xruns.load(Ordering::Relaxed)
        );
        self.send(AppCommand::Resumed).await;
        let out = self.tv.as_mut().map(Tv::on_resume);
        if let Some(out) = out {
            self.apply_tv(out).await;
        }
    }

    /// Called every kill-switch poll: resume detection and the darkener's
    /// timers.
    async fn poll_power(&mut self) {
        if let Some(gap) = self.resume.observe(resume::suspended_total()) {
            self.on_resume(&format!("suspended for {} s", gap.as_secs()))
                .await;
        }
        let action = self.darkener.on_tick(Instant::now());
        self.run_action(action);
    }

    /// A timer event: logged, kept in the table, and a finished timer rings
    /// the alarm (docs/development/esphome-api.md#timers).
    async fn on_timer(&mut self, ev: &AppEvent) {
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
        match event {
            TimerEvent::Finished => {
                if self.alarm.on_finished(id, std::time::Instant::now()) {
                    log::info!("timer alarm: timer {id} finished");
                } else {
                    log::info!("timer alarm: sound is off (device.timer_alarm_max_s = 0)");
                }
            }
            TimerEvent::Cancelled | TimerEvent::Updated => {
                if self.alarm.timer_gone(id) {
                    self.halt_chime();
                    log::info!("timer alarm stopped: Home Assistant changed timer {id}");
                }
            }
            TimerEvent::Started => {}
        }
        self.alarm_poll().await;
        self.publish_timers().await;
    }

    /// Stops the chime's playback when it is the one playing.
    fn halt_chime(&mut self) {
        if self.player.current_kind() == Some(Kind::Alarm) {
            self.player.stop();
        }
    }

    /// Stops the alarm for `why` (wake word, button, media player stop).
    async fn stop_alarm(&mut self, why: &str) {
        if let Some(was) = self.alarm.stop() {
            self.halt_chime();
            log::info!("timer alarm stopped ({why}; was {was:?})");
            self.publish_timers().await;
        }
    }

    /// Starts a queued alarm when the speaker and the conversation are free,
    /// and ends one that reached its cap.
    async fn alarm_poll(&mut self) {
        let busy = self.in_run || self.player.is_active();
        match self.alarm.poll(std::time::Instant::now(), busy) {
            Some(Step::Start(max)) => {
                if self.volume_gain.get() == 0.0 {
                    log::warn!("timer alarm: the reply volume is 0 %, the alarm is silent");
                }
                let stopped = self.player.start_alarm(max);
                self.alarm.started();
                log::info!(
                    "timer alarm: ringing for at most {} s",
                    (max + Duration::from_millis(500)).as_secs()
                );
                self.report_stopped(stopped).await;
                self.publish_timers().await;
            }
            Some(Step::Expire) => {
                self.alarm.stop();
                self.halt_chime();
                log::info!("timer alarm: stopped at the time cap (nobody stopped it)");
                self.publish_timers().await;
            }
            None => {}
        }
    }

    /// Sends the "Timers" and "Timer alarm" sensor states when they changed.
    async fn publish_timers(&mut self) {
        if !self.timers_on {
            return;
        }
        let ringing = self.alarm.phase() != Phase::Idle;
        let text = alarm::timers_text(&self.timers, ringing, std::time::Instant::now());
        if self.last_timers_text.as_deref() != Some(&text) {
            self.last_timers_text = Some(text.clone());
            self.send(AppCommand::SetTextSensor {
                object_id: alarm::TIMERS_SENSOR_ID.to_owned(),
                state: Some(text),
            })
            .await;
        }
        if self.alarm.enabled() && self.last_ringing != Some(ringing) {
            self.last_ringing = Some(ringing);
            self.send(AppCommand::SetBinarySensor {
                object_id: alarm::RINGING_SENSOR_ID.to_owned(),
                state: Some(ringing),
            })
            .await;
        }
    }

    #[allow(clippy::too_many_lines)] // one arm per event, flat on purpose
    async fn on_event(&mut self, ev: AppEvent) {
        let now = Instant::now();
        if matches!(ev, AppEvent::Timer { .. }) {
            self.on_timer(&ev).await;
            return;
        }
        // A command for a TV control entity.
        let tv_out = self.tv.as_mut().and_then(|tv| tv.on_event(&ev));
        if let Some(out) = tv_out {
            self.apply_tv(out).await;
            return;
        }
        match ev {
            AppEvent::HaConnected { client_info, peer } => {
                self.connected = true;
                log::info!("HA connected: {client_info} from {}", peer.ip());
            }
            AppEvent::HaDisconnected => {
                self.connected = false;
                self.in_run = false;
                self.set_streaming(false);
                log::warn!("HA disconnected");
                // Playback was already stopped through StopPlayback if it
                // needed HA; a fully received reply keeps playing.
                if !self.player.is_active() {
                    Self::log_line(self.runs.flush());
                }
            }
            AppEvent::StartStreaming { wake_word, .. } => {
                if !wake_word {
                    self.stop_alarm("a conversation starts").await;
                }
                self.in_run |= !wake_word;
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
                self.stop_alarm("wake word").await;
                self.in_run = true;
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
                // Replies and announcements only; the alarm has its own
                // stops (media player stop, the button, the wake word).
                if let Some(kind) = self.player.stop_reply() {
                    self.send_finished(kind, false).await;
                }
            }
            AppEvent::MediaStop => self.stop_alarm("media player stop").await,
            AppEvent::ButtonPressed { object_id } if object_id == alarm::STOP_BUTTON_ID => {
                if self.alarm.phase() == Phase::Idle {
                    log::info!("stop timer alarm pressed; no alarm is ringing");
                }
                self.stop_alarm("button in Home Assistant").await;
            }
            AppEvent::RunEnd => {
                self.in_run = false;
                log::info!("pipeline run end");
                Self::log_line(self.runs.run_end());
            }
            AppEvent::Error { code, message } => {
                log::warn!("HA pipeline error {code}");
                log::debug!("HA pipeline error message: {message:?}");
                self.runs.error(now, &code);
            }
            AppEvent::Pairing(ev) => self.on_pairing(ev).await,
            AppEvent::ButtonPressed { object_id } => match object_id.as_str() {
                compat::SELFCHECK_RUN => self.request_selfcheck("Home Assistant"),
                compat::FIRMWARE_ACK => self.acknowledge_firmware("Home Assistant").await,
                other => log::debug!("button {other} pressed (no action)"),
            },
            AppEvent::VolumeChanged(percent) => self.on_volume(percent),
            AppEvent::SelectChanged { object_id, option } => self.on_select(&object_id, &option),
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
        if done.kind == Kind::Alarm {
            match &done.result {
                Ok(st) => log::info!(
                    "timer alarm: chime ended ({} ms of audio, {} non-zero samples{})",
                    st.audio.as_millis(),
                    st.nonzero_samples.unwrap_or(0),
                    st.file
                        .as_ref()
                        .map(|f| format!(", file {}", f.display()))
                        .unwrap_or_default()
                ),
                Err(e) => log::warn!("timer alarm: playback failed: {e}"),
            }
            self.alarm
                .ended(done.result.is_ok(), std::time::Instant::now());
            self.alarm_poll().await;
            self.publish_timers().await;
            return;
        }
        let success = match &done.result {
            Ok(st) => {
                log::info!(
                    "playback {} done: {} ms of audio, {} bytes, {}{}",
                    done.id,
                    st.audio.as_millis(),
                    st.bytes,
                    st.nonzero_samples.map_or_else(
                        || "non-zero samples not counted (decoded URL media)".to_owned(),
                        |n| format!("{n} non-zero samples")
                    ),
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
        self.send_finished(done.kind, success).await;
        self.alarm_poll().await;
    }
}

/// The firmware version of a verdict, for log lines.
fn status_version(v: &Verdict) -> &str {
    match v {
        Verdict::Verified(i, _)
        | Verdict::SeriesOnly(i, _)
        | Verdict::NotVerified { info: i, .. } => i.version.as_str(),
        _ => "?",
    }
}

/// Runs the daemon until SIGTERM/SIGINT.
#[allow(clippy::too_many_lines)] // linear setup, loop and shutdown
pub(crate) async fn run(mut settings: Settings, triggers: TestTriggers) -> Result<(), String> {
    if settings.pairing_active() {
        prepare_pairing(&mut settings)?;
    }
    let mut esp = settings.esphome_config().map_err(|e| e.to_string())?;
    // Power watch: only where luna-send exists (the TV), never on a PC.
    let luna = if settings.power.watch {
        let found = power::find_binary(&settings.power.luna_send);
        if found.is_none() {
            log::info!(
                "power: {} not found; the TV power state is not watched (normal off the TV)",
                settings.power.luna_send.display()
            );
        }
        found
    } else {
        log::info!("power: the TV power state watch is off (power.watch = false)");
        None
    };
    let remote_off = match darken::load_mode(&settings.power.remote_off_file) {
        Ok(Some(m)) => m,
        Ok(None) => settings.power.remote_off,
        Err(why) => {
            log::warn!(
                "power: {} is not usable ({why}); using {:?}",
                settings.power.remote_off_file.display(),
                settings.power.remote_off
            );
            settings.power.remote_off
        }
    };
    // The listening indicator (privacy gate F7): always on, driven by the
    // capture thread from the real PCM state.
    esp.binary_sensors.extend(capture::indicator_entities());
    if luna.is_some() {
        esp.power_sensor = true;
        esp.selects.push(darken::select_entity());
        if remote_off == RemoteOff::KeepListening {
            log::warn!(
                "power: remote power-off is KEEP LISTENING: the satellite wakes the TV after a remote power-off and turns its screen off"
            );
        }
    } else if remote_off == RemoteOff::KeepListening {
        log::warn!("power: remote_off = keep_listening needs the power watch; ignored");
    }
    // Timers: the sensors, and with an alarm sound the stop button.
    if settings.device.timers {
        let (buttons, texts, binaries) = alarm::entities(settings.device.timer_alarm_max_s > 0);
        esp.buttons.extend(buttons);
        esp.text_sensors.extend(texts);
        esp.binary_sensors.extend(binaries);
    }
    // The model name (one `luna-send` call for `modelName` only) runs while
    // TV control does its capability check.
    let model_task = luna
        .clone()
        .map(|l| tokio::spawn(async move { compat::read_model(&l).await }));
    // TV control: the capability check (read-only calls) before the entities
    // are listed. `firmware::check` is the firmware alone (no model): it is
    // also the strict gate, which does not use the model.
    let verdict = firmware::check(&settings);
    let mut tv_started = start_tv_control(&settings, luna.as_ref(), &verdict, &mut esp).await;
    let model = match model_task {
        Some(t) => t.await.ok().flatten(),
        None => None,
    };
    let model_state = model.map_or(ModelState::NotRead, ModelState::Known);
    let info = firmware::read(&settings.safety.firmware_info);
    let model_arg = match &model_state {
        ModelState::Known(m) => Model::Known(m),
        ModelState::NotRead => Model::NotRead,
    };
    let status = firmware::evaluate(&settings, info.clone(), model_arg);
    let mut compat = Compat::new(&settings, info, model_state, status.clone());
    compat::add_entities(&mut esp, settings.safety.self_check);
    // Reply volume.
    let (reply_volume, origin) = volume::load(&settings.sink.volume_file, settings.sink.volume);
    match origin {
        Origin::Stored => log::info!(
            "reply volume {reply_volume} % (stored in {})",
            settings.sink.volume_file.display()
        ),
        Origin::Default => {
            log::info!("reply volume {reply_volume} % (sink.volume; none stored yet)");
        }
        Origin::Invalid(why) => log::warn!(
            "reply volume: {} is not usable ({why}); using sink.volume {reply_volume} %",
            settings.sink.volume_file.display()
        ),
    }
    let server = Server::bind(esp).await.map_err(|e| e.to_string())?;
    let unpaired = server.needs_pairing();
    if unpaired {
        log::warn!("NOT PAIRED: the microphone stays closed until Home Assistant sets the key");
    } else if settings.pairing_active() && settings.pairing.open_now {
        log::warn!("pairing window not opened: already paired (use --reset-pairing to pair again)");
    }
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

    // Microphone gate: the strict firmware gate (opt-in), kill switch and the
    // remembered mute. The default policy only reports the status.
    let gated = settings.mic.source == MicSource::Alsa;
    match &status {
        Verdict::Verified(..) => log::info!("firmware: {status}"),
        Verdict::Off => log::warn!("{status}"),
        _ if verdict.blocks() && !gated => {
            log::info!("firmware gate (not applied to a file mic): {verdict}");
        }
        _ if verdict.blocks() => log::error!("FIRMWARE GUARD: {verdict}"),
        _ => log::warn!("firmware: {status}"),
    }
    if let Some(line) = compat.remember_first_start() {
        log::info!("{line}");
    }
    if let Some(old) = compat.firmware_changed() {
        log::warn!(
            "FIRMWARE CHANGED: {old} -> {}; the microphone keeps running and the self-check runs at the first open; press Acknowledge firmware change in Home Assistant or run ctl.sh ack-firmware when you have checked that it works",
            status_version(&status)
        );
    }
    let gate = MicGate {
        ha_muted: settings.safety.mute_file.exists(),
        kill: settings.safety.kill_switch.exists(),
        firmware_block: gated && verdict.blocks(),
        unpaired,
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
    let (notes_tx, mut notes_rx) = mpsc::unbounded_channel();
    let cap = capture::spawn(
        CaptureSettings::from_settings(&settings),
        Arc::clone(&shared),
        cmd_tx.clone(),
        Some(notes_tx),
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
    let _ = cmd_tx.send(AppCommand::SetVolume(reply_volume)).await;
    if luna.is_some() {
        let _ = cmd_tx
            .send(AppCommand::SetSelect {
                object_id: darken::SELECT_OBJECT_ID.to_owned(),
                option: darken::option_for(remote_off).to_owned(),
            })
            .await;
    }
    let server_task = tokio::spawn(server.run(ev_tx, cmd_rx));

    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let (power_tx, mut power_rx) = mpsc::unbounded_channel();
    let power_watch = luna.clone().map(|l| power::spawn(l, power_tx));
    let volume_gain = SharedGain::new(volume::gain_for(reply_volume));
    let mut d = Daemon {
        cmd_tx,
        shared: Arc::clone(&shared),
        player: Player::new(settings.sink.clone(), volume_gain.clone(), done_tx),
        runs: RunTracker::default(),
        connected: false,
        gate,
        kill_switch: settings.safety.kill_switch.clone(),
        mute_file: settings.safety.mute_file.clone(),
        timers: TimerTable::new(),
        alarm: Alarm::new(Duration::from_secs(u64::from(
            settings.device.timer_alarm_max_s,
        ))),
        timers_on: settings.device.timers,
        last_timers_text: None,
        last_ringing: None,
        pairing_triggers: settings.pairing_active().then(|| {
            (
                settings.pairing.pair_trigger(),
                settings.pairing.reset_trigger(),
            )
        }),
        trigger_warned: false,
        volume_gain,
        volume_file: settings.sink.volume_file.clone(),
        luna,
        last_power: None,
        darkener: Darkener::new(
            remote_off,
            settings.power.max_actions_per_hour,
            Duration::from_secs(u64::from(settings.power.cooldown_s)),
        ),
        remote_off_file: settings.power.remote_off_file.clone(),
        resume: resume::Detector::default(),
        last_resume: None,
        tv: None,
        in_run: false,
        compat,
        report_dir: settings.report.dir_or_default(),
        compat_poll: 0,
    };
    d.publish_timers().await;
    let mut tv_rx = None;
    if let Some(started) = tv_started.take() {
        let tv_control::Started {
            tv,
            effects,
            rx,
            entities: _,
        } = started;
        d.tv = Some(tv);
        tv_rx = Some(rx);
        // The first states and the subscriptions, now that the server runs.
        let out = d.tv.as_mut().map(|tv| tv.exec(effects));
        if let Some(out) = out {
            d.apply_tv(out).await;
        }
        if let Some(results) = d.tv.as_ref().map(tv_control::Tv::control_results) {
            d.compat.set_tv_control(results);
        }
    }
    // The first compatibility states (kept by the server until HA connects).
    d.publish_compat().await;
    let mut tv_tick = d.tv.as_ref().map(|_| {
        let mut iv = tokio::time::interval(tv_control::TICK);
        iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
        iv
    });
    // The first reading is the baseline.
    d.resume.observe(resume::suspended_total());
    let mut pairing_poll = settings.pairing_active().then(|| {
        let mut iv = tokio::time::interval_at(Instant::now() + PAIRING_POLL, PAIRING_POLL);
        iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
        iv
    });
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
            Some(u) = power_rx.recv() => d.on_power(u).await,
            Some(m) = recv_opt(tv_rx.as_mut()) => d.on_tv_msg(m).await,
            Some(n) = notes_rx.recv() => d.on_note(n).await,
            () = tick_opt(tv_tick.as_mut()) => d.poll_tv().await,
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
                d.poll_power().await;
                d.alarm_poll().await;
                d.publish_timers().await;
                d.poll_compat_triggers().await;
            }
            () = tick_opt(ptt_timer.as_mut()) => d.ptt("--ptt-every").await,
            () = tick_opt(pairing_poll.as_mut()) => d.poll_pairing().await,
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
    if let Some(w) = power_watch {
        w.shutdown().await;
    }
    if let Some(tv) = d.tv.take() {
        tv.shutdown().await;
    }
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
        r.error(t0 + Duration::from_millis(1500), "stt-no-text-recognized");
        let line = r.run_end().unwrap();
        assert!(
            line.starts_with("run 1 latency ms (from start): stt_end=-"),
            "{line}"
        );
        assert!(
            line.ends_with("error=stt-no-text-recognized error_at=1500"),
            "{line}"
        );
    }

    /// An intent failure (conversation agent down) has no `intent_end`; the
    /// line says when the error arrived so the gap is not a logging hole.
    #[test]
    fn intent_failure_records_when_the_error_arrived() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut r = RunTracker::default();
        r.start(at(0), true);
        r.wake(at(5000));
        r.stt(at(7000));
        r.error(at(7200), "intent-failed");
        let line = r.run_end().unwrap();
        assert_eq!(
            line,
            "run 1 latency ms (from wake word): stt_end=2000 intent_end=- tts_start=- playback_end=- wake_after_stream_start=5000 error=intent-failed error_at=2200"
        );
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

    /// ADR 0006: an unverified or unreadable firmware no longer keeps the
    /// microphone off; only the opt-in strict policy does.
    #[test]
    fn unverified_firmware_does_not_gate_the_mic_unless_strict() {
        use crate::config::FirmwarePolicy;
        use crate::firmware::{FirmwareInfo, evaluate};
        let info = || FirmwareInfo {
            version: "99.99.99".to_owned(),
            release: Some("9.9.9".to_owned()),
            build: None,
        };
        let mut s = Settings::default();
        for model in [
            Model::Known("SOME-OTHER-TV"),
            Model::NotRead,
            Model::NotChecked,
        ] {
            let v = evaluate(&s, Ok(info()), model);
            let g = MicGate {
                firmware_block: v.blocks(),
                ..MicGate::default()
            };
            assert!(!g.mic_off() && g.forced().is_none(), "{v}");
            assert_eq!(g.off_code(), OFF_NONE);
        }
        // The firmware cannot be read at all: still not gated.
        let v = evaluate(&s, Err("no file".to_owned()), Model::NotChecked);
        let g = MicGate {
            firmware_block: v.blocks(),
            ..MicGate::default()
        };
        assert!(!g.mic_off());
        // Strict with a list that does not hold this firmware: the old gate.
        s.safety.firmware_policy = FirmwarePolicy::Strict;
        s.safety.validated_firmware = vec!["03.30.74".to_owned()];
        let v = evaluate(&s, Ok(info()), Model::NotChecked);
        let g = MicGate {
            firmware_block: v.blocks(),
            ..MicGate::default()
        };
        assert!(g.mic_off());
        assert_eq!(g.off_code(), OFF_FIRMWARE);
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
    fn unpaired_keeps_the_mic_closed_until_paired() {
        let mut g = MicGate {
            unpaired: true,
            ..MicGate::default()
        };
        assert!(g.mic_off());
        assert_eq!(g.forced(), Some("not paired"));
        assert_eq!(g.off_code(), OFF_UNPAIRED);
        // No HA can unmute while unpaired.
        assert_eq!(
            g.on_mute_changed(false),
            (Some(AppCommand::SetMuted(true)), false)
        );
        // Paired: unmuted unless something else holds the mic.
        assert_eq!(g.set_unpaired(false), Some(AppCommand::SetMuted(false)));
        assert!(!g.mic_off());
        assert_eq!(g.set_unpaired(false), None);
        // Reset: closed again; the kill switch outranks it.
        assert_eq!(g.set_unpaired(true), Some(AppCommand::SetMuted(true)));
        assert_eq!(g.set_kill(true), Some(AppCommand::SetMuted(true)));
        assert_eq!(g.off_code(), OFF_KILL);
        assert_eq!(g.set_unpaired(false), None, "still killed");
        assert!(g.mic_off());
        // A user mute from before pairing survives it.
        let mut g = MicGate {
            unpaired: true,
            ha_muted: true,
            ..MicGate::default()
        };
        assert_eq!(g.set_unpaired(false), None);
        assert!(g.mic_off());
    }

    #[test]
    fn pairing_startup_actions_and_triggers() {
        let dir = std::env::temp_dir().join(format!("webos-sat-pairing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Settings::default();
        s.pairing.enabled = true;
        s.pairing.state_dir = dir.join("state");
        // Creates the directory with mode 0700; nothing else happens.
        prepare_pairing(&mut s).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&s.pairing.state_dir)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        assert!(!s.pairing.open_now);
        // Without a local action the marker of a remote clear stays.
        let key = s.pairing.key_file();
        webos_esphome_api::pairing::mark_cleared(&key).unwrap();
        prepare_pairing(&mut s).unwrap();
        assert!(webos_esphome_api::pairing::was_cleared(&key));
        // A pair file opens the window, is consumed and ends the cleared state.
        std::fs::write(s.pairing.pair_trigger(), b"").unwrap();
        prepare_pairing(&mut s).unwrap();
        assert!(s.pairing.open_now && !s.pairing.pair_trigger().exists());
        assert!(!webos_esphome_api::pairing::was_cleared(&key));
        // A reset file deletes the stored key and reopens the window.
        s.pairing.open_now = false;
        webos_esphome_api::pairing::store_key(&s.pairing.key_file(), &[3; 32]).unwrap();
        std::fs::write(s.pairing.reset_trigger(), b"").unwrap();
        prepare_pairing(&mut s).unwrap();
        assert!(!s.pairing.key_file().exists() && !s.pairing.reset_trigger().exists());
        assert!(s.pairing.open_now);
        // --reset-pairing does the same without a file.
        s.pairing.open_now = false;
        webos_esphome_api::pairing::store_key(&s.pairing.key_file(), &[3; 32]).unwrap();
        s.pairing.reset_now = true;
        prepare_pairing(&mut s).unwrap();
        assert!(!s.pairing.key_file().exists() && s.pairing.open_now);
        let mut warned = false;
        assert!(!consume_trigger(&dir.join("absent"), &mut warned) && !warned);
        let _ = std::fs::remove_dir_all(&dir);
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
            Some(AppCommand::PlaybackFinished { success: true })
        );
        assert_eq!(
            finished(Kind::Announce, false),
            Some(AppCommand::AnnounceFinished { success: false })
        );
        // The timer alarm is local: HA is told nothing.
        assert_eq!(finished(Kind::Alarm, true), None);
    }
}
