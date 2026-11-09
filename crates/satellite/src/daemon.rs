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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};
use webos_esphome_api::{
    AppCommand, AppEvent, MusicCommand, PairingEvent, PipelineMode, Server, TimerEvent, TimerTable,
};

use crate::alarm::{self, Alarm, Phase, Step};
use crate::capture::{
    self, CaptureSettings, KILL_SWITCH_POLL, OFF_FIRMWARE, OFF_KILL, OFF_MUTED, OFF_NONE,
    OFF_UNPAIRED, Shared,
};
use crate::compat::{self, Compat, ModelState};
use crate::config::{Hang, MicSource, Mode, RemoteOff, Settings, TestTriggers};
use crate::config_switch::{self, ConfigSwitches};
use crate::darken::{self, Action, Darkener};
use crate::firmware::{self, Model, Verdict};
use crate::lg_voice::{self, KeyOwner, LunaSend, Takeover};
use crate::music::{End, Music};
use crate::overlay::{self, Overlay};
use crate::player::{Done, Kind, Player, redact_url};
use crate::power::{self, PowerUpdate};
use crate::remote_mic::StreamEvent;
use crate::remote_ptt::{self, Act, Button, KeySource};
use crate::selfcheck::Note;
use crate::tv_control::{self, Tv, TvMsg};
use crate::voice_status::{Busy, Ha, Mic, MicButtonStatus, VoiceFile, VoiceStatus};
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
        Kind::MediaAnnounce | Kind::Alarm => None,
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

/// Seconds since the Unix epoch (0 if the clock is before it).
fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
    /// The reply volume in force, in percent (what HA's media player shows).
    reply_volume: u8,
    /// When the pairing window closes, while it is open (`run/voice.json`).
    pairing_until: Option<SystemTime>,
    /// The Home Assistant state last published and when it began (epoch s).
    ha_state: Ha,
    ha_since: u64,
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
    /// `run/voice.json`, written on change.
    voice_file: VoiceFile,
    /// Music on the media player (`[music] enabled`).
    music: Option<Music>,
    /// HA's `aec_enabled` and `music_enabled` switches (ADR 0012), when
    /// `ctl.sh` is found.
    config_switches: Option<ConfigSwitches>,
    /// The Magic Remote's mic button (`[remote] mic_button = "ptt"`).
    remote: Option<Button>,
    /// Speech-to-text audio streams now: a run without the wake word, or a
    /// wake word run after the wake word. Set by `StartStreaming` (no wake
    /// word) and `WakeWordDetected`, cleared by `StopStreaming`, `RunEnd`
    /// and a disconnect. Unlike `in_run` it cannot stay set when the voice
    /// layer aborted a run and swallowed HA's `RUN_END`; the mic button
    /// joins only a run for which it is set.
    stt_streaming: bool,
    /// LG's voice key, taken over while the mic button runs (ADR 0013).
    lg_takeover: Option<Takeover<LunaSend>>,
    /// The mic button as `run/voice.json` reports it.
    mic_status: Option<MicButtonStatus>,
    /// The input grab's live status (ADR 0016), while it runs.
    grab_status: Option<Arc<std::sync::Mutex<crate::key_grab::GrabStatus>>>,
    /// The last text sent to HA's "LG voice key" sensor.
    lg_text: Option<String>,
    /// The on-screen assistant overlay (`[overlay] enabled`) and the task
    /// that starts its app (`None` without `luna-send` or when it is off).
    overlay: Overlay,
    overlay_tx: Option<overlay::Sender>,
    /// Open while the TV shows a picture ([`overlay::screen_on`]); closed
    /// until the first power update.
    screen_tx: watch::Sender<bool>,
    /// HA's "LG voice key" sensor is listed.
    lg_sensor_listed: bool,
    /// The watchdog's heartbeats (kept going during a long takeover step).
    beats: Arc<Beats>,
    /// When the persistent databases are looked at again after the takeover.
    lg_recheck_at: Option<Instant>,
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
        let reload = consume_trigger(&self.report_dir.join(darken::RELOAD_TRIGGER), &mut warned);
        let lg_refresh = consume_trigger(
            &self.report_dir.join(tv_control::LG_REFRESH_TRIGGER),
            &mut warned,
        );
        let volume_reload =
            consume_trigger(&self.report_dir.join(volume::RELOAD_TRIGGER), &mut warned);
        self.trigger_warned = warned;
        if reload {
            self.reload_remote_off("ctl.sh").await;
        }
        if volume_reload {
            self.reload_reply_volume("ctl.sh").await;
        }
        if lg_refresh {
            // Read-only: the sensors show what the TV holds now. Without TV
            // control the file is only removed.
            let out = self.tv.as_mut().map(Tv::refresh_lg_settings);
            if let Some(out) = out {
                self.apply_tv(out).await;
            }
        }
        self.publish_voice_status();
    }

    /// The microphone as `run/voice.json` reports it.
    fn mic_state(&self) -> Mic {
        if self.shared.open.load(Ordering::Relaxed) {
            Mic::Listening
        } else if self.gate.mic_off() {
            Mic::Muted
        } else if self.shared.mic_busy.load(Ordering::Relaxed) {
            Mic::Busy
        } else {
            Mic::Closed
        }
    }

    /// What keeps the daemon from a restart now, in this order: a voice run
    /// (a conversation, or a reply or announcement playing), music loaded,
    /// the timer alarm queued or ringing. The one definition for
    /// `run/voice.json` (`busy`, read by `ctl.sh config`) and for HA's config
    /// switches.
    fn busy(&self) -> Option<Busy> {
        // The alarm chime plays through the player: it is the alarm, not a
        // voice run (a reply taking the speaker from it counts as voice).
        let voice_playback =
            self.player.is_active() && self.player.current_kind() != Some(Kind::Alarm);
        if self.in_run || voice_playback {
            Some(Busy::Voice)
        } else if self.music.as_ref().is_some_and(Music::is_active) {
            Some(Busy::Music)
        } else if self.alarm.phase() != Phase::Idle {
            Some(Busy::Alarm)
        } else {
            None
        }
    }

    /// Writes `run/voice.json` if its content changed (or the keep-alive is
    /// due).
    fn publish_voice_status(&mut self) {
        let ha = if self.gate.unpaired {
            Ha::Unpaired
        } else if self.connected {
            Ha::Connected
        } else {
            Ha::Disconnected
        };
        if ha != self.ha_state {
            self.ha_state = ha;
            self.ha_since = epoch_now();
        }
        let status = VoiceStatus {
            mic: self.mic_state(),
            remote_off: self.darkener.mode(),
            remote_off_active: self.luna.is_some(),
            power_state: self.last_power.as_ref().and_then(|p| p.state.clone()),
            pairing_until: self.pairing_until_epoch(),
            ha,
            ha_since: self.ha_since,
            busy: self.busy(),
            mic_button: self.live_mic_status(),
            music: Some(self.music.as_ref().map_or_else(
                || crate::music::DISABLED_JSON.to_owned(),
                |m| m.status_json(Instant::now()),
            )),
        };
        self.voice_file.publish(Instant::now().into_std(), &status);
    }

    /// The mic button's status with the input grab's live state: "ours (grab +
    /// configd)" (no LG toast) only while the helper actually grabs; starting,
    /// restarting, failed or a dead supervisor read as "ours (configd)" (LG's
    /// toast shows), ADR 0016.
    fn live_mic_status(&self) -> Option<MicButtonStatus> {
        let mut m = self.mic_status.clone()?;
        if let Some(g) = self.grab_status.as_ref().and_then(|g| g.lock().ok()) {
            m.key_grab = Some(crate::key_grab::describe(&g));
            if m.lg_voice_key == KeyOwner::Ours.word() && g.state == "grabbing" {
                m.lg_voice_key = KeyOwner::OursGrab.word();
            }
        }
        Some(m)
    }

    /// Follows the input grab's live state: HA's "LG voice key" sensor when
    /// its text changes, and `voice.json`. Called every poll tick.
    async fn sync_grab_status(&mut self) {
        if self.grab_status.is_none() {
            return;
        }
        let Some(m) = self.live_mic_status() else {
            return;
        };
        let text = sensor_text_for(&m);
        if self.lg_sensor_listed && self.lg_text.as_deref() != Some(text.as_str()) {
            self.send(AppCommand::SetTextSensor {
                object_id: LG_VOICE_KEY_ID.to_owned(),
                state: Some(text.clone()),
            })
            .await;
        }
        self.lg_text = Some(text);
        self.publish_voice_status();
    }

    /// The end of the open pairing window as seconds since the Unix epoch, or
    /// `None` when it is closed (an end in the past counts as closed, in case
    /// the close event was missed). The wall clock is used on purpose: a
    /// reader has no access to this process's monotonic clock.
    fn pairing_until_epoch(&mut self) -> Option<u64> {
        let until = self.pairing_until?;
        if until <= SystemTime::now() {
            self.pairing_until = None;
            return None;
        }
        until
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() + u64::from(d.subsec_nanos() > 0))
    }

    /// `ctl.sh set reply-volume` wrote `etc/reply_volume` and created the
    /// trigger: read it again and apply a change (gain, and HA's media player
    /// follows). An unusable or missing file keeps the current value. The
    /// file already holds the value, so it is not written back.
    async fn reload_reply_volume(&mut self, source: &str) {
        let path = self.volume_file.clone();
        let (percent, origin) = volume::load(&path, self.reply_volume);
        match origin {
            Origin::Stored if percent != self.reply_volume => {
                self.reply_volume = percent;
                self.volume_gain.set(volume::gain_for(percent));
                log::info!(
                    "reply volume {percent} % ({}) set by {source}",
                    volume::db_for(percent)
                        .map_or_else(|| "silent".to_owned(), |db| format!("{db:.1} dB"))
                );
                // HA's media player shows the new value (not echoed back).
                self.send(AppCommand::SetVolume(percent)).await;
            }
            Origin::Stored => log::debug!(
                "reply volume: {} reloaded ({source}); the value is unchanged ({percent} %)",
                path.display()
            ),
            Origin::Default => log::warn!(
                "reply volume: reload requested ({source}) but {} does not exist; keeping {} %",
                path.display(),
                self.reply_volume
            ),
            Origin::Invalid(why) => log::warn!(
                "reply volume: reload requested ({source}) but {} is not usable ({why}); keeping {} %",
                path.display(),
                self.reply_volume
            ),
        }
    }

    /// `ctl.sh set remote-off` wrote the mode file and created the trigger:
    /// read it again and apply a change. An unusable file keeps the current
    /// mode. The file already holds the value, so it is not written back.
    async fn reload_remote_off(&mut self, source: &str) {
        let path = self.remote_off_file.clone();
        match darken::load_mode(&path) {
            Ok(Some(mode)) if mode != self.darkener.mode() => {
                self.apply_remote_off(mode, source);
                // HA's select follows (there is one only with the power watch).
                if self.luna.is_some() {
                    self.send(AppCommand::SetSelect {
                        object_id: darken::SELECT_OBJECT_ID.to_owned(),
                        option: darken::option_for(mode).to_owned(),
                    })
                    .await;
                }
            }
            Ok(Some(_)) => log::debug!(
                "remote power-off: {} reloaded ({source}); the mode is unchanged",
                path.display()
            ),
            Ok(None) => log::warn!(
                "remote power-off: reload requested ({source}) but {} does not exist; keeping {:?}",
                path.display(),
                self.darkener.mode()
            ),
            Err(why) => log::warn!(
                "remote power-off: reload requested ({source}) but {} is not usable ({why}); keeping {:?}",
                path.display(),
                self.darkener.mode()
            ),
        }
    }

    /// Puts a new remote power-off mode in force and logs who asked. Used by
    /// HA's select ([`Daemon::on_select`], which also stores the word) and by
    /// the reload of the mode file; it does not write the file.
    fn apply_remote_off(&mut self, mode: RemoteOff, source: &str) {
        self.darkener.set_mode(mode);
        match mode {
            RemoteOff::KeepListening => {
                log::warn!(
                    "remote power-off set to KEEP LISTENING by {source}: after the remote turns the TV off the satellite wakes it and turns the screen off (the TV stays on with a dark screen; more power than standby, OLED panel care may not run)"
                );
                if self.luna.is_none() {
                    log::warn!("power: keep listening needs the power watch, which is not running");
                }
            }
            RemoteOff::Normal => {
                log::info!("remote power-off set to turn off normally by {source}");
            }
        }
    }

    /// Polls the kill-switch file.
    async fn poll_kill_switch(&mut self) {
        let on = self.kill_switch.exists();
        if self.gate.mic_off() || on {
            // The remote's microphone obeys the same gates.
            let acts = self
                .remote
                .as_mut()
                .map(|b| b.halt("the microphone is off"))
                .unwrap_or_default();
            self.run_remote_acts(acts).await;
        }
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
                self.pairing_until = None;
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
                self.pairing_until = None;
                self.set_streaming(false);
                log::warn!(
                    "pairing CLEARED by {client_info:?} from {} (the device was removed in Home Assistant): microphone closed; to pair again, open the pairing window locally (--pair or the pair file)",
                    peer.ip()
                );
                self.set_unpaired(true).await;
            }
            PairingEvent::WindowOpened { seconds } => {
                log::info!("pairing window open for {seconds} s");
                self.pairing_until = SystemTime::now().checked_add(Duration::from_secs(seconds));
            }
            PairingEvent::WindowClosed => {
                log::info!("pairing window closed");
                self.pairing_until = None;
            }
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

    /// What the mic button's machine needs to know at a press.
    fn remote_ctx(&self) -> remote_ptt::Context {
        remote_ptt::Context {
            connected: self.connected,
            mic_off: self.gate.mic_off(),
            mic_open: self.shared.open.load(Ordering::Relaxed),
            announcing: self.player.is_active()
                && self.player.current_kind() == Some(Kind::Announce),
            run_streaming: self.stt_streaming && self.shared.streaming.load(Ordering::Relaxed),
        }
    }

    /// Carries out the mic button's actions, in order.
    async fn run_remote_acts(&mut self, acts: Vec<Act>) {
        for a in acts {
            match a {
                Act::StartRun => {
                    self.send(AppCommand::StartPipeline {
                        mode: PipelineMode::PushToTalk,
                    })
                    .await;
                }
                Act::EndAudio => self.send(AppCommand::EndAudio).await,
                Act::Audio(chunk) => {
                    self.send(AppCommand::Audio(chunk)).await;
                    self.shared.chunks_sent.fetch_add(1, Ordering::Relaxed);
                }
                Act::RemoteFeeds(on) => self.shared.remote_feeds.store(on, Ordering::Relaxed),
                // Run by the button itself.
                Act::OpenStream | Act::CloseStream => {}
            }
        }
    }

    async fn on_remote_key(&mut self, key: crate::evdev::Key) {
        let ctx = self.remote_ctx();
        let acts = match self.remote.as_mut() {
            Some(b) => b.on_key(key, Instant::now(), ctx),
            None => return,
        };
        self.run_remote_acts(acts).await;
    }

    async fn on_remote_stream(&mut self, generation: u64, ev: StreamEvent) {
        let acts = match self.remote.as_mut() {
            Some(b) => b.on_stream(generation, ev, Instant::now()),
            None => return,
        };
        self.run_remote_acts(acts).await;
    }

    async fn remote_tick(&mut self) {
        let acts = match self.remote.as_mut() {
            Some(b) => b.on_tick(Instant::now()),
            None => return,
        };
        self.run_remote_acts(acts).await;
    }

    /// Follows whether speech-to-text audio streams (see `stt_streaming`).
    /// Hands a changed overlay frame to its sender, which launches it only
    /// while the screen gate is open at the moment of the call
    /// ([`overlay::screen_on`]).
    fn publish_overlay(&mut self) {
        let Some(frame) = self.overlay.take() else {
            return;
        };
        if let Some(tx) = &self.overlay_tx {
            tx.send(frame);
        }
    }

    /// The screen gate follows the last power update.
    fn update_screen_gate(&self) {
        self.screen_tx
            .send_replace(overlay::screen_on(self.last_power.as_ref()));
    }

    fn track_stt(&mut self, ev: &AppEvent) {
        match ev {
            AppEvent::StartStreaming { wake_word, .. } => self.stt_streaming = !wake_word,
            AppEvent::WakeWordDetected => self.stt_streaming = true,
            AppEvent::StopStreaming | AppEvent::RunEnd | AppEvent::HaDisconnected => {
                self.stt_streaming = false;
            }
            _ => {}
        }
    }

    /// A few seconds after the takeover: configd's persistent databases must
    /// be as before it. If not, the override is removed at once and the mic
    /// button stops, loudly (log, `voice.json`, HA's switch and sensor).
    async fn lg_recheck(&mut self) {
        let Some(t) = self.lg_takeover.as_ref() else {
            return;
        };
        let Err(why) = t.check_persistence() else {
            log::info!(
                "remote mic button: configd's persistent databases unchanged since the takeover"
            );
            return;
        };
        log::error!(
            "remote mic button: {why}; giving LG's voice key back and stopping the mic button"
        );
        self.stop_mic_button(why).await;
    }

    /// Stops a running mic button with a reason: the override goes, the
    /// reader and the machine stop, HA's switch goes off and the reports
    /// say why.
    async fn stop_mic_button(&mut self, why: String) {
        let _beat = Heartbeat::start(&self.beats);
        // The grab (ADR 0016) ends first and its helper has exited before LG's
        // key is given back: a press in between shows LG's toast, never LG's
        // assistant through a still-grabbed remote.
        if let Some(mut b) = self.remote.take() {
            let acts = b.shutdown().await;
            self.run_remote_acts(acts).await;
        }
        self.grab_status = None;
        let mut key = KeyOwner::Lg;
        if let Some(mut t) = self.lg_takeover.take() {
            if let Err(e) = t.release().await {
                log::error!("remote mic button: {e}");
                key = KeyOwner::Unknown;
            }
        }
        let status = MicButtonStatus {
            state: "failed",
            lg_voice_key: key.word(),
            error: Some(why),
            key_grab: None,
        };
        let text = sensor_text_for(&status);
        self.lg_text = Some(text.clone());
        self.mic_status = Some(status);
        if let Some(cs) = self.config_switches.as_mut() {
            cs.set_mic_button_active(false);
            self.cmd_tx
                .send(AppCommand::SetSwitch {
                    object_id: config_switch::MIC_BUTTON_ID.to_owned(),
                    state: Some(false),
                })
                .await
                .ok();
        }
        if self.lg_sensor_listed {
            self.send(AppCommand::SetTextSensor {
                object_id: LG_VOICE_KEY_ID.to_owned(),
                state: Some(text),
            })
            .await;
        }
        self.publish_voice_status();
    }

    /// The mic button follows the voice layer's events.
    async fn remote_on_event(&mut self, ev: &AppEvent) {
        let Some(b) = self.remote.as_mut() else {
            return;
        };
        let acts = match ev {
            // Only the run its own push-to-talk request started.
            AppEvent::StartStreaming {
                push_to_talk: true, ..
            } => b.on_run_started(),
            AppEvent::PushToTalkFailed => b.on_ptt_failed(),
            AppEvent::StopStreaming | AppEvent::RunEnd => b.on_audio_over(),
            AppEvent::HaDisconnected => b.halt("Home Assistant disconnected"),
            AppEvent::MuteChanged(true) => b.halt("the microphone was muted"),
            _ => return,
        };
        self.run_remote_acts(acts).await;
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
    fn on_volume(&mut self, percent: u8) {
        self.reply_volume = percent.min(100);
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

    /// HA flipped `aec_enabled` or `music_enabled` (ADR 0012): `ctl.sh
    /// config` runs detached unless a guard refuses; a refusal or failure
    /// sends the real state again.
    async fn on_config_switch(&mut self, setting: config_switch::Setting, want: bool) {
        let busy = self.busy();
        let cmd_tx = self.cmd_tx.clone();
        let Some(cs) = self.config_switches.as_mut() else {
            return;
        };
        if let Some(cmd) = cs.on_request(setting, want, busy, &cmd_tx) {
            self.send(cmd).await;
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
        self.apply_remote_off(mode, "Home Assistant");
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
        if let (Some(m), Some(_)) = (self.music.as_mut(), shown.state.as_ref()) {
            m.on_power(&shown.state_key());
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
        self.update_screen_gate();
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

    /// What the power key of TV control (the media player's turn off and
    /// `tv_power`) needs from the daemon (design section 4.2): the select,
    /// the kill switch and the darkener's verdict (used only in the "Keep
    /// listening" mode; in "Turn off normally" TV control checks Always
    /// Ready itself).
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
        if let Some(m) = self.music.as_mut() {
            m.on_resume();
        }
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
        self.track_stt(&ev);
        self.overlay.on_event(&ev, now);
        self.publish_overlay();
        self.remote_on_event(&ev).await;
        // A command for a TV control entity.
        let tv_out = self.tv.as_mut().and_then(|tv| tv.on_event(&ev));
        if let Some(out) = tv_out {
            self.apply_tv(out).await;
            return;
        }
        match ev {
            AppEvent::HaConnected { client_info, peer } => {
                self.connected = true;
                if let Some(m) = self.music.as_mut() {
                    m.on_ha(true, now);
                }
                log::info!("HA connected: {client_info} from {}", peer.ip());
            }
            AppEvent::HaDisconnected => {
                self.connected = false;
                if let Some(m) = self.music.as_mut() {
                    m.on_ha(false, now);
                }
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
                let stopped = self.player.play_url(url, preannounce_url, Kind::Announce);
                self.report_stopped(stopped).await;
            }
            AppEvent::StopPlayback => {
                // Replies and announcements only; the alarm has its own
                // stops (media player stop, the button, the wake word).
                if let Some(kind) = self.player.stop_reply() {
                    self.send_finished(kind, false).await;
                }
                self.overlay.on_playback_end(now);
                self.publish_overlay();
            }
            AppEvent::MediaStop => self.on_media_stop().await,
            AppEvent::Music(cmd) => self.on_music(cmd).await,
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
                log::debug!(
                    "HA pipeline error message ({} chars, not logged)",
                    message.chars().count()
                );
                self.runs.error(now, &code);
            }
            AppEvent::Pairing(ev) => self.on_pairing(ev).await,
            AppEvent::ButtonPressed { object_id } => match object_id.as_str() {
                compat::SELFCHECK_RUN => self.request_selfcheck("Home Assistant"),
                compat::FIRMWARE_ACK => self.acknowledge_firmware("Home Assistant").await,
                other => log::debug!("button {other} pressed (no action)"),
            },
            AppEvent::SwitchChanged { object_id, state } => {
                match config_switch::Setting::from_object_id(&object_id) {
                    Some(setting) => self.on_config_switch(setting, state).await,
                    None => log::debug!("switch {object_id} changed (no action)"),
                }
            }
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

    /// HA's media player stop (design D5): a ringing timer alarm first, then
    /// a `play_media` announcement, then the music.
    async fn on_media_stop(&mut self) {
        if self.alarm.phase() != Phase::Idle || self.music.is_none() {
            self.stop_alarm("media player stop").await;
        } else if self.player.current_kind() == Some(Kind::MediaAnnounce) {
            log::info!("media player stop: the announcement stops, the music goes on");
            self.player.stop();
        } else if let Some(m) = self.music.as_mut() {
            if !m.stop(End::Stopped) {
                log::info!("media player stop: nothing is playing");
            }
        }
    }

    /// A music command from HA (only with `[music] enabled`).
    async fn on_music(&mut self, cmd: MusicCommand) {
        let Some(m) = self.music.as_mut() else {
            return;
        };
        match cmd {
            MusicCommand::Play {
                url,
                announce: false,
            } => m.play(&url),
            MusicCommand::Play {
                url,
                announce: true,
            } => {
                let shown = webos_playback::guard::redact_url(&url);
                if let Some(why) = m.refusal() {
                    log::info!("music: announcement {shown} refused: {why}; the URL is released");
                    m.release(&url);
                    return;
                }
                log::info!("music: announcement {shown} over the music (play_media, announce)");
                let stopped = self.player.play_url(url, None, Kind::MediaAnnounce);
                self.report_stopped(stopped).await;
            }
            MusicCommand::Pause => m.pause(),
            MusicCommand::Resume => m.resume(),
            MusicCommand::EnqueueRefused { url: Some(url) } => {
                log::info!("music: enqueue refused (no queue here); releasing the URL");
                m.release(&url);
            }
            MusicCommand::EnqueueRefused { url: None } => {
                log::info!("music: enqueue refused (no queue here)");
            }
            other => log::debug!("music: command {other:?} ignored"),
        }
    }

    /// After every event: the music's duck sources, shared volume,
    /// disconnect deadline and state.
    async fn sync_music(&mut self) {
        let playing = self.player.current_kind();
        if let Some(m) = self.music.as_mut() {
            m.sync(self.in_run, playing, self.reply_volume, Instant::now())
                .await;
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
            self.overlay.on_playback_end(now);
            self.publish_overlay();
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

/// The overlay as this start can run it: its app id and whether it can
/// show at all (`[overlay] enabled` aside): the power watch's `luna-send`
/// is there and the overlay app is installed.
pub(crate) struct OverlayPlan {
    pub(crate) app_id: Option<String>,
    pub(crate) available: bool,
}

impl OverlayPlan {
    pub(crate) fn new(settings: &Settings, luna: Option<&PathBuf>) -> Self {
        let prefix = settings.install_prefix.as_deref();
        let app_id = overlay::app_id(settings.overlay.app_id.as_deref(), prefix);
        let present = app_id
            .as_deref()
            .is_some_and(|id| overlay::app_present(prefix, id));
        Self {
            available: luna.is_some() && present,
            app_id,
        }
    }

    /// Starts the sender with `[overlay] enabled` when the overlay can show.
    fn start(
        &self,
        settings: &Settings,
        luna: Option<&PathBuf>,
        screen: watch::Receiver<bool>,
    ) -> Option<overlay::Sender> {
        if !settings.overlay.enabled {
            log::info!("overlay: off (overlay.enabled = false)");
            return None;
        }
        let (Some(luna), Some(app_id)) = (luna, self.app_id.as_ref()) else {
            log::info!(
                "overlay: unavailable: needs the power watch's luna-send and the app directory (normal off the TV)"
            );
            return None;
        };
        if !self.available {
            log::warn!(
                "overlay: unavailable: the overlay app {app_id} is not installed (ctl.sh install)"
            );
            return None;
        }
        log::info!("overlay: on (app {app_id})");
        Some(overlay::Sender::spawn(luna.clone(), app_id.clone(), screen))
    }
}

/// Object id of the diagnostic text sensor "LG voice key".
pub(crate) const LG_VOICE_KEY_ID: &str = "lg_voice_key";

/// The "LG voice key" diagnostic text sensor (listed where `luna-send` is).
fn lg_voice_key_entity() -> webos_esphome_api::config::TextSensorEntity {
    webos_esphome_api::config::TextSensorEntity {
        object_id: LG_VOICE_KEY_ID.to_owned(),
        name: "LG voice key".to_owned(),
        icon: "mdi:account-voice".to_owned(),
        device_class: String::new(),
        entity_category: webos_esphome_api::proto::EntityCategory::Diagnostic,
    }
}

/// The mic button feature after the start: what runs and what is reported.
pub(crate) struct RemoteSetup {
    pub(crate) started: Option<remote_ptt::Started>,
    pub(crate) takeover: Option<Takeover<LunaSend>>,
    pub(crate) status: MicButtonStatus,
}

impl RemoteSetup {
    /// The text of HA's "LG voice key" sensor.
    pub(crate) fn sensor_text(&self) -> String {
        sensor_text_for(&self.status)
    }
}

/// The text of HA's "LG voice key" sensor for a status.
pub(crate) fn sensor_text_for(s: &MicButtonStatus) -> String {
    let owner = match s.lg_voice_key {
        "ours (configd)" => "Satellite (LG ignores the key; LG's toast shows)",
        "ours (grab + configd)" => "Satellite (input grab; no LG toast)",
        "unknown" => "Unknown (restore pending)",
        _ => "LG assistant",
    };
    match &s.error {
        Some(e) => format!("{owner}; mic button off: {e}")
            .chars()
            .take(250)
            .collect(),
        None => owner.to_owned(),
    }
}

/// Where the state file and the lock of LG's voice key live: `<install>/run`
/// (the fixed place `ctl.sh`, the supervisor and `status` read), whatever
/// `[report] dir` says; off the TV (no install) the report directory.
pub(crate) fn lg_state_dir(settings: &Settings) -> PathBuf {
    settings
        .install_prefix
        .as_ref()
        .map_or_else(|| settings.report.dir_or_default(), |p| p.join("run"))
}

/// Keeps the watchdog's event-loop heartbeat going while a long awaited step
/// runs on the loop: the takeover of LG's voice key may wait up to 30 s for
/// its lock and then for several luna calls, against `limits.loop_stall_s`
/// (30 s by default). The runtime keeps running other tasks while it waits,
/// so the ticker is real progress; a truly stuck loop stops it too (it runs
/// on the same thread). Stops when dropped.
pub(crate) struct Heartbeat(tokio::task::JoinHandle<()>);

impl Heartbeat {
    pub(crate) fn start(beats: &Arc<Beats>) -> Self {
        let b = Arc::clone(beats);
        Self(tokio::spawn(async move {
            loop {
                b.event_loop.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(KILL_SWITCH_POLL).await;
            }
        }))
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// How long after the takeover the persistent databases are looked at again.
const LG_RECHECK_AFTER: Duration = Duration::from_secs(5);

/// The mic button (ADR 0013, docs/design/remote-mic-button.md). With
/// `mic_button = "ptt"`: the key device must be there, LG's voice key is
/// taken over (`crate::lg_voice`), then the reader, the machine and the
/// remote microphone start. If any step fails the feature does not run and
/// LG's key stays as it was; the reason is logged and reported. With
/// `"off"` nothing starts, but an override left by an earlier run is given
/// back.
#[allow(clippy::too_many_lines)] // linear: off, checks, takeover, key source, start
pub(crate) async fn setup_remote(
    settings: &Settings,
    paths: crate::evdev::InputPaths,
) -> RemoteSetup {
    let r = &settings.remote;
    let luna = power::find_binary(&settings.power.luna_send);
    let state_dir = lg_state_dir(settings);
    let off = |key: KeyOwner, error: Option<String>| MicButtonStatus {
        state: if error.is_some() { "failed" } else { "off" },
        lg_voice_key: key.word(),
        error,
        key_grab: None,
    };
    if !r.ptt() {
        log::info!("remote mic button: off (mic_button = \"off\")");
        let mut key = KeyOwner::Lg;
        if let Some(l) = luna.clone() {
            let mut t = Takeover::new(LunaSend(l), &state_dir);
            if let Err(e) = t.release().await {
                log::warn!("remote mic button: {e}");
                key = KeyOwner::Unknown;
            }
        } else if state_dir.join(lg_voice::STATE_FILE).exists() {
            log::warn!(
                "remote mic button: {} is left from an earlier run but luna-send is not found: LG's voice key is unknown (ctl.sh restore-lg-voice or a TV restart)",
                state_dir.join(lg_voice::STATE_FILE).display()
            );
            key = KeyOwner::Unknown;
        }
        return RemoteSetup {
            started: None,
            takeover: None,
            status: off(key, None),
        };
    }
    let fail = |key: KeyOwner, why: String| {
        log::warn!("remote mic button: NOT running: {why}");
        RemoteSetup {
            started: None,
            takeover: None,
            status: off(key, Some(why)),
        }
    };
    if crate::evdev::find_device(&paths, &r.device_name).is_none() {
        return fail(
            KeyOwner::Lg,
            format!("no input device named {:?}", r.device_name),
        );
    }
    let Some(luna) = luna else {
        return fail(
            KeyOwner::Lg,
            format!(
                "{} not found, so LG's voice key cannot be taken over",
                settings.power.luna_send.display()
            ),
        );
    };
    let mut takeover = Takeover::new(LunaSend(luna.clone()), &state_dir);
    if let Err(why) = takeover.acquire().await {
        let key = if state_dir.join(lg_voice::STATE_FILE).exists() {
            KeyOwner::Unknown
        } else {
            KeyOwner::Lg
        };
        return fail(key, why);
    }
    log::info!(
        "remote mic button: LG's voice key taken over (configd {} = ignore, in memory only; a reboot or ctl.sh restore-lg-voice gives it back)",
        lg_voice::KEY
    );
    let mic_luna = r.remote_mic.then_some(luna);
    let (source, grab_off) = choose_key_source(settings, paths).await;
    // "ours (grab + configd)" only once the helper actually grabs: the daemon
    // derives it from the live state (`Daemon::live_mic_status`).
    let owner = KeyOwner::Ours;
    match Button::start(r, mic_luna, settings.dsp.hpf_hz, source) {
        Ok(st) => RemoteSetup {
            started: Some(st),
            takeover: Some(takeover),
            status: MicButtonStatus {
                state: "ptt",
                lg_voice_key: owner.word(),
                error: None,
                key_grab: Some(
                    grab_off.map_or_else(|| "starting".to_owned(), |w| format!("off: {w}")),
                ),
            },
        },
        Err(e) => {
            let why = format!("cannot start the reader thread: {e}");
            let key = match takeover.release().await {
                Ok(()) => KeyOwner::Lg,
                Err(r) => {
                    log::warn!("remote mic button: {r}");
                    KeyOwner::Unknown
                }
            };
            fail(key, why)
        }
    }
}

/// Where the mic key comes from (ADR 0016): the input grab where the
/// firmware passed EXP-026, the helper is installed and its check passes;
/// otherwise the passive reader with ADR 0013's override alone (LG shows its
/// toast on a press). The second value says why the grab is not used.
async fn choose_key_source(
    settings: &Settings,
    paths: crate::evdev::InputPaths,
) -> (KeySource, Option<String>) {
    let fw = firmware::read(&settings.safety.firmware_info)
        .ok()
        .map(|i| i.version);
    let plan = crate::key_grab::plan(
        &settings.remote,
        fw.as_deref(),
        settings.install_prefix.as_deref(),
    );
    let why = match plan {
        Ok(p) => match crate::key_grab::preflight(&p).await {
            Ok(line) => {
                log::info!(
                    "remote grab: check passed ({line}); the mic key is taken at the input layer (ADR 0016)"
                );
                return (KeySource::Grab(p), None);
            }
            Err(why) => why,
        },
        Err(why) => why,
    };
    log::info!(
        "remote grab: not used ({why}); LG's toast may show on a press (ADR 0013 override only)"
    );
    (KeySource::Passive(paths), Some(why))
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
    // HA's two config switches, after the TV control and compatibility
    // entities so the built-in mute switch stays first and the others keep
    // their order (ADR 0012). Absent when ctl.sh is not found.
    let mut config_switches = ConfigSwitches::new(&settings);
    if let Some(cs) = &config_switches {
        esp.switches.extend(config_switch::entities());
        cs.spawn_last_change_log();
    }
    let overlay_plan = OverlayPlan::new(&settings, luna.as_ref());
    if let Some(cs) = config_switches.as_mut() {
        cs.set_overlay_available(overlay_plan.available);
    }
    // HA's "LG voice key" sensor, where luna-send is (the TV).
    let lg_sensor_listed = power::find_binary(&settings.power.luna_send).is_some();
    if lg_sensor_listed {
        esp.text_sensors.push(lg_voice_key_entity());
    }
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
    if triggers.ptt_on_connect
        || triggers.ptt_every.is_some()
        || triggers.hang.is_some()
        || triggers.fetch_loopback
    {
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
    // Nothing captures yet: no wake word session in HA until the capture
    // thread reports audio (queued before it can send `true`).
    let _ = cmd_tx.try_send(AppCommand::CaptureLive(false));
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
    // The Magic Remote's mic button: runs only with mic_button = "ptt" and
    // LG's voice key taken over (ADR 0013); HA's switch shows what runs.
    // After every step that can fail, so no early return leaves the key
    // taken over (the supervisor would give it back, but not at once).
    let remote_setup = {
        let _beat = Heartbeat::start(&beats);
        setup_remote(&settings, crate::evdev::InputPaths::default()).await
    };
    if let Some(cs) = config_switches.as_mut() {
        cs.set_mic_button_active(remote_setup.started.is_some());
    }
    let lg_sensor = lg_sensor_listed.then(|| remote_setup.sensor_text());
    let lg_sensor_text = lg_sensor.clone();
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
    if let Some(cs) = &config_switches {
        for cmd in cs.initial_commands() {
            let _ = cmd_tx.send(cmd).await;
        }
    }
    if let Some(text) = lg_sensor {
        let _ = cmd_tx
            .send(AppCommand::SetTextSensor {
                object_id: LG_VOICE_KEY_ID.to_owned(),
                state: Some(text),
            })
            .await;
    }
    let server_task = tokio::spawn(server.run(ev_tx, cmd_rx));

    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let (power_tx, mut power_rx) = mpsc::unbounded_channel();
    let power_watch = luna.clone().map(|l| power::spawn(l, power_tx));
    let volume_gain = SharedGain::new(volume::gain_for(reply_volume));
    let (music, mut music_rx) = match Music::start(
        &settings.music,
        &settings.sink,
        triggers.fetch_loopback,
        cmd_tx.clone(),
    ) {
        Some((m, rx)) => (Some(m), Some(rx)),
        None => (None, None),
    };
    let RemoteSetup {
        started,
        takeover: lg_takeover,
        status: mic_status,
    } = remote_setup;
    let (remote, mut remote_keys, mut remote_stream, grab_status) = match started {
        Some(st) => (
            Some(st.button),
            Some(st.keys),
            Some(st.stream),
            st.grab_status,
        ),
        None => (None, None, None, None),
    };
    let mut player = Player::new(settings.sink.clone(), volume_gain.clone(), done_tx);
    player.set_fetch_loopback(triggers.fetch_loopback);
    let (screen_tx, screen_rx) = watch::channel(false);
    let overlay_tx = overlay_plan.start(&settings, luna.as_ref(), screen_rx);
    let overlay_inst = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
    let mut d = Daemon {
        cmd_tx,
        shared: Arc::clone(&shared),
        player,
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
        reply_volume,
        pairing_until: None,
        ha_state: if unpaired {
            Ha::Unpaired
        } else {
            Ha::Disconnected
        },
        ha_since: epoch_now(),
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
        voice_file: VoiceFile::new(&settings.report.dir_or_default()),
        music,
        config_switches,
        remote,
        stt_streaming: false,
        lg_recheck_at: lg_takeover
            .as_ref()
            .map(|_| Instant::now() + LG_RECHECK_AFTER),
        lg_takeover,
        mic_status: Some(mic_status),
        grab_status,
        lg_text: lg_sensor_text,
        overlay: Overlay::new(overlay_tx.is_some(), overlay_inst, Instant::now()),
        overlay_tx,
        screen_tx,
        lg_sensor_listed,
        beats: Arc::clone(&beats),
    };
    d.publish_voice_status();
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
            Some(ev) = recv_opt(music_rx.as_mut()) => {
                if let Some(m) = d.music.as_mut() {
                    m.on_event(ev);
                }
            }
            Some(u) = power_rx.recv() => d.on_power(u).await,
            Some(m) = recv_opt(tv_rx.as_mut()) => d.on_tv_msg(m).await,
            Some(n) = notes_rx.recv() => d.on_note(n).await,
            Some(k) = recv_opt(remote_keys.as_mut()) => d.on_remote_key(k).await,
            Some((g, ev)) = recv_opt(remote_stream.as_mut()) => d.on_remote_stream(g, ev).await,
            () = sleep_opt(d.remote.as_ref().and_then(Button::deadline)) => d.remote_tick().await,
            () = sleep_opt(d.lg_recheck_at) => {
                d.lg_recheck_at = None;
                d.lg_recheck().await;
            }
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
                d.sync_grab_status().await;
                d.poll_power().await;
                d.alarm_poll().await;
                d.publish_timers().await;
                d.poll_compat_triggers().await;
                d.overlay.on_tick(Instant::now());
                d.publish_overlay();
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
        d.sync_music().await;
    }

    // Shutdown: stop streaming and playback (kills pacat), release the mic,
    // then close the command channel so the server says goodbye to HA.
    beats.disarmed.store(true, Ordering::Relaxed);
    d.voice_file.remove();
    if let Some(w) = power_watch {
        w.shutdown().await;
    }
    if let Some(tv) = d.tv.take() {
        tv.shutdown().await;
    }
    d.set_streaming(false);
    if let Some(mut b) = d.remote.take() {
        let acts = b.shutdown().await;
        d.run_remote_acts(acts).await;
    }
    // LG's voice key back (the supervisor and ctl.sh restore-lg-voice do it
    // too, from the state file, when this process cannot).
    if let Some(mut t) = d.lg_takeover.take() {
        match t.release().await {
            Ok(()) => log::info!("remote mic button: LG's voice key given back"),
            Err(e) => log::warn!("remote mic button: {e}"),
        }
    }
    if let Some(m) = d.music.take() {
        m.shutdown().await;
    }
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

    /// A daemon wired to nothing: the command channel is returned so a test
    /// sees what HA would be sent.
    fn test_daemon(
        dir: &Path,
        mode: RemoteOff,
        luna: bool,
    ) -> (Daemon, mpsc::Receiver<AppCommand>) {
        let mut s = Settings::default();
        s.report.dir = Some(dir.join("run"));
        s.safety.firmware_seen_file = Some(dir.join("firmware_seen"));
        let verdict = firmware::evaluate(&s, Err("no file".to_owned()), Model::NotChecked);
        let compat = Compat::new(&s, Err("no file".to_owned()), ModelState::NotRead, verdict);
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let (done_tx, _done_rx) = mpsc::unbounded_channel();
        let gain = SharedGain::new(1.0);
        let d = Daemon {
            cmd_tx,
            shared: Arc::new(Shared::default()),
            player: Player::new(s.sink.clone(), gain.clone(), done_tx),
            runs: RunTracker::default(),
            connected: false,
            gate: MicGate::default(),
            kill_switch: dir.join("kill"),
            mute_file: dir.join("muted"),
            timers: TimerTable::new(),
            alarm: Alarm::new(Duration::ZERO),
            timers_on: false,
            last_timers_text: None,
            last_ringing: None,
            pairing_triggers: None,
            trigger_warned: false,
            volume_gain: gain,
            volume_file: dir.join("volume"),
            reply_volume: 100,
            pairing_until: None,
            ha_state: Ha::Disconnected,
            ha_since: 1_762_000_000,
            luna: luna.then(|| PathBuf::from("luna-send")),
            last_power: None,
            darkener: Darkener::new(mode, 3, Duration::from_secs(60)),
            remote_off_file: dir.join("remote_off"),
            resume: resume::Detector::default(),
            last_resume: None,
            tv: None,
            in_run: false,
            compat,
            report_dir: dir.join("run"),
            compat_poll: 0,
            voice_file: VoiceFile::new(&dir.join("run")),
            music: None,
            config_switches: None,
            remote: None,
            stt_streaming: false,
            lg_takeover: None,
            mic_status: None,
            grab_status: None,
            lg_text: None,
            overlay: Overlay::new(true, 7, Instant::now()),
            overlay_tx: None,
            screen_tx: watch::channel(false).0,
            lg_sensor_listed: false,
            lg_recheck_at: None,
            beats: Arc::new(Beats::default()),
        };
        (d, cmd_rx)
    }

    fn test_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("webos-sat-daemon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).unwrap();
        d
    }

    fn select_option(cmd: AppCommand) -> String {
        match cmd {
            AppCommand::SetSelect { object_id, option } => {
                assert_eq!(object_id, darken::SELECT_OBJECT_ID);
                option
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn config_switch_requests_follow_the_guards() {
        use webos_esphome_api::AppEvent;
        let dir = test_dir("cfgsw");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, false);
        let ev = |id: &str, state: bool| AppEvent::SwitchChanged {
            object_id: id.to_owned(),
            state,
        };
        // Without ctl.sh the switches do not exist: nothing happens.
        d.on_event(ev(config_switch::AEC_ID, false)).await;
        assert!(rx.try_recv().is_err());
        d.config_switches = Some(ConfigSwitches::fixture(
            true,
            false,
            Path::new("/nonexistent"),
            &dir.join("run"),
        ));
        // Another switch id is not a config switch.
        d.on_event(ev("tv_screen", true)).await;
        assert!(rx.try_recv().is_err());
        // The state in force: nothing to do, nothing sent.
        d.on_event(ev(config_switch::AEC_ID, true)).await;
        assert!(rx.try_recv().is_err());
        // A conversation is running: refused, the stored state is sent again.
        d.in_run = true;
        d.on_event(ev(config_switch::AEC_ID, false)).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            AppCommand::ResendSwitch {
                object_id: config_switch::AEC_ID.to_owned()
            }
        );
        d.in_run = false;
    }

    #[tokio::test]
    async fn reload_applies_a_mode_written_outside_ha() {
        let dir = test_dir("reload");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, true);
        let file = dir.join("remote_off");
        // What `ctl.sh set remote-off keep` does: the file, then the trigger.
        std::fs::write(&file, "keep_listening\n").unwrap();
        std::fs::write(dir.join("run").join(darken::RELOAD_TRIGGER), b"").unwrap();
        // The poll runs on every fourth call (once a second).
        for _ in 0..4 {
            d.poll_compat_triggers().await;
        }
        assert_eq!(d.darkener.mode(), RemoteOff::KeepListening);
        assert_eq!(
            select_option(rx.try_recv().unwrap()),
            "Keep listening (screen off)"
        );
        assert!(rx.try_recv().is_err());
        // The trigger is consumed and the file is not rewritten by the daemon.
        assert!(!dir.join("run").join(darken::RELOAD_TRIGGER).exists());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep_listening\n");
        // The status file follows within the same poll.
        let v = std::fs::read_to_string(dir.join("run").join(crate::voice_status::FILE)).unwrap();
        assert!(v.contains("\"remote_off\":\"keep_listening\""), "{v}");
        assert!(v.contains("\"remote_off_active\":true"), "{v}");
        // The same word again: nothing to apply, nothing sent.
        std::fs::write(dir.join("run").join(darken::RELOAD_TRIGGER), b"").unwrap();
        d.reload_remote_off("test").await;
        d.poll_compat_triggers().await;
        assert!(rx.try_recv().is_err());
        // Back to normal.
        std::fs::write(&file, "normal\n").unwrap();
        d.reload_remote_off("test").await;
        assert_eq!(d.darkener.mode(), RemoteOff::Normal);
        assert_eq!(select_option(rx.try_recv().unwrap()), "Turn off normally");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ctl.sh set reply-volume N`: the file, then the trigger. The poll
    /// consumes it, the shared gain changes (so a reply that is playing
    /// follows), HA's media player is told, and the file is not rewritten.
    #[tokio::test]
    async fn reply_volume_reload_applies_a_value_written_outside_ha() {
        let dir = test_dir("volume-reload");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, true);
        let file = dir.join("volume");
        let trigger = dir.join("run").join(volume::RELOAD_TRIGGER);
        std::fs::write(&file, "30\n").unwrap();
        std::fs::write(&trigger, b"").unwrap();
        // The poll runs on every fourth call (once a second).
        for _ in 0..4 {
            d.poll_compat_triggers().await;
        }
        assert_eq!(d.reply_volume, 30);
        assert!((d.volume_gain.get() - volume::gain_for(30)).abs() < f32::EPSILON);
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::SetVolume(30)));
        assert!(rx.try_recv().is_err());
        assert!(!trigger.exists(), "the trigger is consumed");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "30\n");
        // The same value again: nothing applied, nothing sent.
        std::fs::write(&trigger, b"").unwrap();
        for _ in 0..4 {
            d.poll_compat_triggers().await;
        }
        assert!(rx.try_recv().is_err());
        assert!(!trigger.exists());
        // 0 is silent, 100 is unity.
        std::fs::write(&file, "0\n").unwrap();
        d.reload_reply_volume("test").await;
        assert_eq!(d.volume_gain.get().to_bits(), 0.0_f32.to_bits());
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::SetVolume(0)));
        std::fs::write(&file, "100\n").unwrap();
        d.reload_reply_volume("test").await;
        assert!((d.volume_gain.get() - 1.0).abs() < f32::EPSILON);
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::SetVolume(100)));
        // HA's own change keeps the value in force in step.
        d.on_volume(55);
        assert_eq!(d.reply_volume, 55);
        std::fs::write(&file, "55\n").unwrap();
        d.reload_reply_volume("test").await;
        assert!(rx.try_recv().is_err(), "unchanged after HA's own change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reply_volume_reload_keeps_the_value_when_the_file_is_unusable() {
        let dir = test_dir("volume-invalid");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, true);
        d.on_volume(40);
        let file = dir.join("volume");
        // Missing file (on_volume stored one: remove it).
        std::fs::remove_file(&file).unwrap();
        d.reload_reply_volume("test").await;
        assert_eq!(d.reply_volume, 40);
        // Garbage, above 100, negative, empty, and a directory.
        for bad in ["loud\n", "101\n", "-5\n", "", "50 %\n"] {
            std::fs::write(&file, bad).unwrap();
            d.reload_reply_volume("test").await;
            assert_eq!(d.reply_volume, 40, "{bad:?}");
        }
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        d.reload_reply_volume("test").await;
        assert_eq!(d.reply_volume, 40);
        assert!((d.volume_gain.get() - volume::gain_for(40)).abs() < f32::EPSILON);
        assert!(rx.try_recv().is_err(), "HA is told nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The pairing window is published as its end time (written on open and
    /// close only), so a reader can show a countdown.
    #[tokio::test]
    async fn voice_json_reports_the_pairing_window() {
        let dir = test_dir("voice-pairing");
        let (mut d, _rx) = test_daemon(&dir, RemoteOff::Normal, true);
        let read = |dir: &Path| {
            std::fs::read_to_string(dir.join("run").join(crate::voice_status::FILE)).unwrap()
        };
        d.publish_voice_status();
        assert!(read(&dir).contains("\"pairing_until\":null"));
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        d.on_pairing(PairingEvent::WindowOpened { seconds: 600 })
            .await;
        d.publish_voice_status();
        let v = read(&dir);
        let until: u64 = v
            .split("\"pairing_until\":")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .parse()
            .unwrap_or_else(|_| panic!("{v}"));
        assert!(
            (before + 600..=before + 602).contains(&until),
            "{until} {before}"
        );
        // Closing without a key.
        d.on_pairing(PairingEvent::WindowClosed).await;
        d.publish_voice_status();
        assert!(read(&dir).contains("\"pairing_until\":null"));
        // A window that ends in the past counts as closed (a missed event).
        d.pairing_until = Some(SystemTime::now() - Duration::from_secs(5));
        d.publish_voice_status();
        assert!(read(&dir).contains("\"pairing_until\":null"));
        assert!(d.pairing_until.is_none());
        // Pairing ends the window.
        d.on_pairing(PairingEvent::WindowOpened { seconds: 60 })
            .await;
        assert!(d.pairing_until.is_some());
        d.on_pairing(PairingEvent::Paired {
            client_info: "Home Assistant".to_owned(),
            peer: "127.0.0.1:1".parse().unwrap(),
        })
        .await;
        assert!(d.pairing_until.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Home Assistant connection in `run/voice.json`: a state word and
    /// the time it began, nothing else (no address, name or version).
    #[tokio::test]
    async fn voice_json_reports_the_home_assistant_connection() {
        let dir = test_dir("voice-ha");
        let (mut d, _rx) = test_daemon(&dir, RemoteOff::Normal, true);
        let read = |dir: &Path| {
            std::fs::read_to_string(dir.join("run").join(crate::voice_status::FILE)).unwrap()
        };
        let since = |v: &str| -> u64 {
            v.split("\"ha_since\":")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .trim_end_matches(['}', '\n'])
                .parse()
                .unwrap()
        };
        d.publish_voice_status();
        let v = read(&dir);
        assert!(v.contains("\"ha\":\"disconnected\""), "{v}");
        let t0 = since(&v);
        assert!(t0 > 1_700_000_000, "{v}");
        // The session opens (the event carries an address; it never reaches the file).
        d.on_event(AppEvent::HaConnected {
            client_info: "Home Assistant 2026.10".to_owned(),
            peer: "192.168.1.9:50000".parse().unwrap(),
        })
        .await;
        d.publish_voice_status();
        let v = read(&dir);
        assert!(v.contains("\"ha\":\"connected\""), "{v}");
        assert!(since(&v) >= t0);
        assert!(!v.contains("192.168") && !v.contains("2026.10"), "{v}");
        // An unchanged state keeps its start time.
        let t1 = since(&v);
        d.ha_since = t1 - 100;
        d.publish_voice_status();
        assert_eq!(since(&read(&dir)), t1 - 100);
        // Unpaired wins over a session; pairing brings the state back.
        let _ = d.gate.set_unpaired(true);
        d.publish_voice_status();
        assert!(read(&dir).contains("\"ha\":\"unpaired\""));
        assert!(since(&read(&dir)) >= t1);
        let _ = d.gate.set_unpaired(false);
        d.on_event(AppEvent::HaDisconnected).await;
        d.publish_voice_status();
        assert!(read(&dir).contains("\"ha\":\"disconnected\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refresh trigger of `ctl.sh set`: consumed once a second, both LG
    /// sensors are read at once and HA gets the new values. Read-only.
    #[tokio::test]
    async fn the_lg_refresh_trigger_reads_both_sensors_and_is_consumed() {
        let dir = test_dir("lg-refresh");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, true);
        let calls = dir.join("calls");
        let luna = dir.join("luna-send");
        crate::testutil::write_script(
            &luna,
            &format!(
                r#"echo "$5 $6" >> '{calls}'
case "$5" in
  */getPowerState) echo '{{"returnValue":true,"state":"Active"}}' ;;
  */getSystemSettings) cat '{d}/ar' 2>/dev/null || echo '{{"returnValue":true,"settings":{{"lifeOnScreenMode":"off"}}}}' ;;
  */getActivation) cat '{d}/hl' 2>/dev/null || echo '{{"returnValue":true,"settingStatus":false}}' ;;
  *) echo '{{"returnValue":true}}' ;;
esac
"#,
                calls = calls.display(),
                d = dir.display()
            ),
        );
        let mut settings = Settings::default();
        let t = &mut settings.tv_control;
        (t.screen, t.power_on, t.volume, t.mute, t.input, t.apps) =
            (false, false, false, false, false, false);
        let mut st = tv_control::start(&settings, luna, None, &dir.join("no-declarations")).await;
        let startup = st.tv.exec(std::mem::take(&mut st.effects));
        let mut tv_rx = st.rx;
        d.tv = Some(st.tv);
        d.apply_tv(startup).await;
        let text = |cmd: AppCommand| match cmd {
            AppCommand::SetTextSensor { object_id, state } => (object_id, state),
            other => panic!("unexpected {other:?}"),
        };
        let mut first = Vec::new();
        while let Ok(c) = rx.try_recv() {
            first.push(text(c));
        }
        assert!(first.contains(&("lg_always_ready".to_owned(), Some("Off".to_owned()))));
        assert!(first.contains(&("lg_hands_free".to_owned(), Some("Off".to_owned()))));
        // What `ctl.sh set` does after a change: the TV holds new values and
        // the trigger file is created.
        std::fs::write(
            dir.join("ar"),
            r#"{"returnValue":true,"settings":{"lifeOnScreenMode":"alwaysReady"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("hl"),
            r#"{"returnValue":true,"settingStatus":true}"#,
        )
        .unwrap();
        let trigger = dir.join("run").join(tv_control::LG_REFRESH_TRIGGER);
        std::fs::write(&trigger, b"").unwrap();
        for _ in 0..4 {
            d.poll_compat_triggers().await;
        }
        assert!(!trigger.exists(), "consumed");
        let mut got = Vec::new();
        for _ in 0..2 {
            let msg = tokio::time::timeout(Duration::from_secs(10), tv_rx.recv())
                .await
                .unwrap()
                .unwrap();
            d.on_tv_msg(msg).await;
        }
        while let Ok(c) = rx.try_recv() {
            got.push(text(c));
        }
        assert!(got.contains(&("lg_always_ready".to_owned(), Some("On".to_owned()))));
        assert!(got.contains(&("lg_hands_free".to_owned(), Some("On".to_owned()))));
        let log = std::fs::read_to_string(&calls).unwrap();
        assert!(!log.contains("setSystemSettings") && !log.contains("setActivation"));
        // Without TV control the trigger file is only removed.
        d.tv = None;
        std::fs::write(&trigger, b"").unwrap();
        for _ in 0..4 {
            d.poll_compat_triggers().await;
        }
        assert!(!trigger.exists());
        assert!(rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reload_keeps_the_mode_when_the_file_is_unusable() {
        let dir = test_dir("invalid");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::KeepListening, true);
        let file = dir.join("remote_off");
        // Missing file.
        d.reload_remote_off("test").await;
        assert_eq!(d.darkener.mode(), RemoteOff::KeepListening);
        // Garbage, an empty file and a directory.
        for bad in ["darken\n", "", "keep\n"] {
            std::fs::write(&file, bad).unwrap();
            d.reload_remote_off("test").await;
            assert_eq!(d.darkener.mode(), RemoteOff::KeepListening, "{bad:?}");
        }
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        d.reload_remote_off("test").await;
        assert_eq!(d.darkener.mode(), RemoteOff::KeepListening);
        assert!(rx.try_recv().is_err(), "HA is told nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reload_without_the_power_watch_sends_no_select_state() {
        let dir = test_dir("nowatch");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, false);
        std::fs::write(dir.join("remote_off"), "keep_listening\n").unwrap();
        d.reload_remote_off("test").await;
        // No select entity exists without the power watch.
        assert!(rx.try_recv().is_err());
        d.publish_voice_status();
        let v = std::fs::read_to_string(dir.join("run").join(crate::voice_status::FILE)).unwrap();
        assert!(v.contains("\"remote_off\":\"keep_listening\""), "{v}");
        assert!(v.contains("\"remote_off_active\":false"), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ha_select_applies_and_stores_the_word() {
        let dir = test_dir("select");
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, true);
        d.on_select(darken::SELECT_OBJECT_ID, "Keep listening (screen off)");
        assert_eq!(d.darkener.mode(), RemoteOff::KeepListening);
        assert_eq!(
            std::fs::read_to_string(dir.join("remote_off")).unwrap(),
            "keep_listening\n"
        );
        // HA already shows its own choice: no echo.
        assert!(rx.try_recv().is_err());
        // Another object id and an unknown option change nothing.
        d.on_select("other", "Turn off normally");
        d.on_select(darken::SELECT_OBJECT_ID, "nonsense");
        assert_eq!(d.darkener.mode(), RemoteOff::KeepListening);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn voice_json_follows_the_microphone_and_power_state() {
        let dir = test_dir("voice");
        let (mut d, _rx) = test_daemon(&dir, RemoteOff::Normal, true);
        let read = |dir: &Path| {
            std::fs::read_to_string(dir.join("run").join(crate::voice_status::FILE)).unwrap()
        };
        // Nothing open, nothing wanted off: closed.
        d.publish_voice_status();
        assert!(read(&dir).contains("\"mic\":\"closed\""));
        assert!(read(&dir).contains("\"power_state\":null"));
        // Another process holds the PCM.
        d.shared.mic_busy.store(true, Ordering::Relaxed);
        assert_eq!(d.mic_state(), Mic::Busy);
        // Open and delivering.
        d.shared.open.store(true, Ordering::Relaxed);
        assert_eq!(d.mic_state(), Mic::Listening);
        d.shared.open.store(false, Ordering::Relaxed);
        d.shared.mic_busy.store(false, Ordering::Relaxed);
        // Each gate reads as muted, and a gate wins over busy.
        d.shared.mic_busy.store(true, Ordering::Relaxed);
        let _ = d.gate.on_mute_changed(true);
        assert_eq!(d.mic_state(), Mic::Muted);
        let _ = d.gate.on_mute_changed(false);
        let _ = d.gate.set_kill(true);
        assert_eq!(d.mic_state(), Mic::Muted);
        let _ = d.gate.set_kill(false);
        let _ = d.gate.set_unpaired(true);
        assert_eq!(d.mic_state(), Mic::Muted);
        let _ = d.gate.set_unpaired(false);
        d.shared.mic_busy.store(false, Ordering::Relaxed);
        // The last settled power state is reported.
        d.last_power = Some(PowerUpdate {
            state: Some("Screen Off".to_owned()),
            ..PowerUpdate::default()
        });
        d.shared.open.store(true, Ordering::Relaxed);
        d.publish_voice_status();
        let v = read(&dir);
        assert!(v.contains("\"mic\":\"listening\""), "{v}");
        assert!(v.contains("\"power_state\":\"Screen Off\""), "{v}");
        // No identifier of any kind: only the documented keys (the music
        // object is `{"enabled":false}` with music off).
        let parsed: Vec<&str> = v
            .trim_matches(|c| c == '{' || c == '}' || c == '\n')
            .split(',')
            .map(|kv| kv.split(':').next().unwrap())
            .collect();
        assert_eq!(
            parsed,
            [
                "\"schema\"",
                "\"mic\"",
                "\"remote_off\"",
                "\"remote_off_active\"",
                "\"power_state\"",
                "\"pairing_until\"",
                "\"ha\"",
                "\"ha_since\"",
                "\"busy\"",
                "\"music\""
            ]
        );
        assert!(v.contains(",\"music\":{\"enabled\":false}}"), "{v}");
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

    // ---------------------------------------------------------------- music

    type MusicRx = mpsc::UnboundedReceiver<webos_playback::music::MusicEvent>;

    /// A test daemon with `[music] enabled`, the WAV sink for replies, the
    /// alarm and music in `dir/wav`, and the player's reports kept.
    fn music_daemon(
        dir: &Path,
    ) -> (
        Daemon,
        mpsc::Receiver<AppCommand>,
        MusicRx,
        mpsc::UnboundedReceiver<Done>,
    ) {
        let (mut d, cmd_rx) = test_daemon(dir, RemoteOff::Normal, false);
        let wav = dir.join("wav");
        std::fs::create_dir_all(&wav).unwrap();
        let sink = crate::config::SinkSection {
            kind: crate::config::SinkKind::Wav,
            dir: wav,
            ..crate::config::SinkSection::default()
        };
        let m = crate::config::MusicSection {
            enabled: true,
            disconnect_stop_s: 1,
            ..crate::config::MusicSection::default()
        };
        let (music, music_rx) = Music::start(&m, &sink, false, d.cmd_tx.clone()).unwrap();
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        d.player = Player::new(sink, d.volume_gain.clone(), done_tx);
        d.music = Some(music);
        d.alarm = Alarm::new(Duration::from_secs(30));
        (d, cmd_rx, music_rx, done_rx)
    }

    /// Runs the parts of the daemon loop that music needs for `ms`.
    async fn drive(
        d: &mut Daemon,
        music_rx: &mut MusicRx,
        done_rx: &mut mpsc::UnboundedReceiver<Done>,
        ms: u64,
    ) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            tokio::select! {
                Some(ev) = music_rx.recv() => d.music.as_mut().unwrap().on_event(ev),
                Some(done) = done_rx.recv() => d.on_done(done).await,
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            d.sync_music().await;
        }
    }

    fn music_states(rx: &mut mpsc::Receiver<AppCommand>) -> Vec<AppCommand> {
        let mut v = Vec::new();
        while let Ok(c) = rx.try_recv() {
            if matches!(
                c,
                AppCommand::SetMusicState(_)
                    | AppCommand::AnnounceFinished { .. }
                    | AppCommand::PlaybackFinished { .. }
            ) {
                v.push(c);
            }
        }
        v
    }

    /// A playing tone in the music slot (no network).
    fn tone(d: &mut Daemon) {
        let src = webos_playback::music::ToneSource::new(
            48_000,
            2,
            webos_playback::music::Wave::Dc,
            8_000,
            Some(Duration::from_secs(20)),
        );
        d.music.as_mut().unwrap().play_test_source(Box::new(src));
    }

    /// Every command in `rx` now.
    fn drain(rx: &mut mpsc::Receiver<AppCommand>) -> Vec<AppCommand> {
        let mut v = Vec::new();
        while let Ok(c) = rx.try_recv() {
            v.push(c);
        }
        v
    }

    #[tokio::test]
    async fn config_switch_is_refused_for_each_kind_of_activity() {
        use webos_esphome_api::AppEvent;
        let resend = AppCommand::ResendSwitch {
            object_id: config_switch::AEC_ID.to_owned(),
        };
        let flip = AppEvent::SwitchChanged {
            object_id: config_switch::AEC_ID.to_owned(),
            state: false,
        };
        // 0: music, 1: the timer alarm, 2: the player (a reply streaming in).
        for (n, what) in ["music", "alarm", "player"].into_iter().enumerate() {
            let dir = test_dir(&format!("cfgsw-busy-{what}"));
            let (mut d, mut rx, mut mrx, mut drx) = music_daemon(&dir);
            d.config_switches = Some(ConfigSwitches::fixture(
                true,
                false,
                Path::new("/nonexistent"),
                &dir.join("run"),
            ));
            match n {
                0 => tone(&mut d),
                1 => assert!(d.alarm.on_finished("t1", std::time::Instant::now())),
                _ => {
                    d.player.start_stream(16_000, 1);
                    assert!(d.player.is_active());
                }
            }
            drive(&mut d, &mut mrx, &mut drx, 50).await;
            let _ = drain(&mut rx);
            let want = [Busy::Music, Busy::Alarm, Busy::Voice][n];
            assert_eq!(d.busy(), Some(want), "{what}: the reason produced");
            assert_eq!(
                config_switch::decide(true, false, d.busy(), false),
                config_switch::Decision::Refuse(want.reason()),
                "{what}: the refusal text"
            );
            d.on_event(flip.clone()).await;
            assert!(
                drain(&mut rx).contains(&resend),
                "{what}: refused with the stored state sent again"
            );
        }
        // Control: the same request with nothing active is accepted: it
        // starts bin/ctl.sh of the install, here a script that records its
        // arguments.
        let dir = test_dir("cfgsw-busy-none");
        let (mut d, mut rx, _mrx, _drx) = music_daemon(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(
            dir.join("bin").join("ctl.sh"),
            "#!/bin/sh\necho \"$@\" > \"$WEBOS_SAT_PREFIX/marker\"\n",
        )
        .unwrap();
        // The lib.sh it would source: checked before ctl.sh runs.
        std::fs::write(dir.join("bin").join("lib.sh"), "").unwrap();
        d.config_switches = Some(ConfigSwitches::fixture(true, false, &dir, &dir.join("run")));
        let _ = drain(&mut rx);
        d.on_event(flip).await;
        let marker = dir.join("marker");
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().trim(),
            "config aec off"
        );
    }

    #[tokio::test]
    async fn a_ringing_alarm_is_busy_alarm_not_voice() {
        let dir = test_dir("busy-ringing");
        let (mut d, _rx, _mrx, _drx) = music_daemon(&dir);
        let file = dir.join("run").join(crate::voice_status::FILE);
        assert!(d.alarm.on_finished("t1", std::time::Instant::now()));
        d.alarm_poll().await;
        assert_eq!(d.alarm.phase(), Phase::Ringing, "the chime started");
        assert_eq!(d.player.current_kind(), Some(Kind::Alarm));
        assert!(d.player.is_active());
        assert_eq!(d.busy(), Some(Busy::Alarm));
        d.publish_voice_status();
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("\"busy\":\"alarm\"")
        );
        assert_eq!(
            config_switch::decide(true, false, d.busy(), false),
            config_switch::Decision::Refuse("a timer alarm is active")
        );
        // A reply that takes the speaker is a voice run again.
        d.player.start_stream(16_000, 1);
        assert_eq!(d.busy(), Some(Busy::Voice));
        d.halt_chime();
    }

    #[tokio::test]
    async fn the_mic_button_joins_only_while_speech_to_text_streams() {
        let dir = test_dir("stt-join");
        let (mut d, _rx) = test_daemon(&dir, RemoteOff::Normal, false);
        let start = |wake_word: bool| AppEvent::StartStreaming {
            wake_word,
            conversation_id: None,
            push_to_talk: false,
        };
        let joinable = |d: &Daemon| d.remote_ctx().run_streaming;
        // A continued conversation streams: joinable.
        d.on_event(start(false)).await;
        assert!(joinable(&d));
        // The voice layer aborts it (mute, capture loss, an announcement)
        // and swallows HA's RUN_END: only StopStreaming arrives.
        d.on_event(AppEvent::StopStreaming).await;
        assert!(!joinable(&d));
        // The wake word session that follows is not joinable, although
        // in_run is still set from the aborted run.
        d.on_event(start(true)).await;
        assert!(d.in_run, "the stale flag this test is about");
        assert!(!joinable(&d));
        // After the wake word it is speech-to-text: joinable.
        d.on_event(AppEvent::WakeWordDetected).await;
        assert!(joinable(&d));
        d.on_event(AppEvent::RunEnd).await;
        assert!(!joinable(&d));
        d.on_event(start(false)).await;
        d.on_event(AppEvent::HaDisconnected).await;
        assert!(!joinable(&d));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn no_luna(dir: &Path) -> Settings {
        let mut s = Settings::default();
        s.power.luna_send = dir.join("no-luna-send");
        s.report.dir = Some(dir.join("run"));
        s
    }

    #[tokio::test]
    async fn mic_button_off_starts_nothing() {
        // Off is inert: no reader thread, no device opened, no machine.
        let dir = test_dir("micoff");
        let (_root, paths) =
            crate::evdev::tests::fake_inputs("micoff", &[(2, "LGE M-RCU - Builtin [0]")]);
        let r = setup_remote(&no_luna(&dir), paths).await;
        assert!(r.started.is_none() && r.takeover.is_none());
        assert_eq!(r.status.state, "off");
        assert_eq!(r.status.lg_voice_key, "lg");
        assert_eq!(r.sensor_text(), "LG assistant");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake `luna-send` that models configd's in-memory value in a file
    /// next to it (`val`: absent = LG's `launch`); `refuse`: setConfigs
    /// answers an error.
    fn fake_configd(dir: &Path, refuse: bool) -> PathBuf {
        let luna = dir.join("luna-send");
        crate::testutil::write_script(
            &luna,
            &format!(
                r#"D='{d}'
echo "$5 $6" >> "$D/calls"
case "$5" in
*/setConfigs)
  if [ {refuse} = 1 ]; then echo '{{"returnValue":false,"errorText":"denied"}}'; exit 0; fi
  case "$6" in *'"action":"ignore"'*) echo ignore > "$D/val" ;; *null*) rm -f "$D/val" ;; esac
  echo '{{"returnValue":true}}' ;;
*/getConfigs)
  v=$(cat "$D/val" 2>/dev/null || echo launch)
  printf '{{"configs":{{"com.webos.keyaction.voice":{{"action":"%s"}}}},"returnValue":true}}\n' "$v" ;;
*) exit 3 ;;
esac"#,
                d = dir.display(),
                refuse = u8::from(refuse)
            ),
        );
        luna
    }

    #[tokio::test]
    async fn mic_button_on_takes_lgs_key_over_and_gives_it_back() {
        let dir = test_dir("micon");
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = no_luna(&dir);
        s.power.luna_send = fake_configd(&dir, false);
        s.remote.mic_button = crate::config::MicButton::Ptt;
        s.remote.remote_mic = false;
        let (_root, paths) =
            crate::evdev::tests::fake_inputs("micon", &[(2, "LGE M-RCU - Builtin [0]")]);
        let r = setup_remote(&s, paths).await;
        assert_eq!(r.status.state, "ptt", "{:?}", r.status.error);
        assert_eq!(r.status.lg_voice_key, "ours (configd)");
        assert_eq!(
            r.sensor_text(),
            "Satellite (LG ignores the key; LG's toast shows)"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("val")).unwrap().trim(),
            "ignore"
        );
        assert!(dir.join("run").join(lg_voice::STATE_FILE).exists());
        // Every set was volatile and only the one key was read.
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        for l in calls.lines() {
            assert!(l.contains("com.webos.keyaction.voice"), "{l}");
            if l.contains("setConfigs") {
                assert!(l.contains("\"volatile\":true"), "{l}");
            }
        }
        let RemoteSetup {
            started, takeover, ..
        } = r;
        let mut b = started.unwrap().button;
        b.shutdown().await;
        takeover.unwrap().release().await.unwrap();
        assert!(!dir.join("val").exists(), "LG's value is back");
        assert!(!dir.join("run").join(lg_voice::STATE_FILE).exists());
        // A later start with the feature off finds nothing to give back.
        s.remote.mic_button = crate::config::MicButton::Off;
        let (_r2, p2) = crate::evdev::tests::fake_inputs("micon2", &[]);
        assert_eq!(setup_remote(&s, p2).await.status.lg_voice_key, "lg");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_refused_takeover_leaves_lg_as_it_was_and_off_gives_back_a_leftover() {
        let dir = test_dir("micref");
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = no_luna(&dir);
        s.power.luna_send = fake_configd(&dir, true);
        s.remote.mic_button = crate::config::MicButton::Ptt;
        let (_root, paths) =
            crate::evdev::tests::fake_inputs("micref", &[(2, "LGE M-RCU - Builtin [0]")]);
        let r = setup_remote(&s, paths).await;
        assert!(r.started.is_none());
        assert_eq!(r.status.state, "failed");
        assert_eq!(r.status.lg_voice_key, "lg");
        assert!(
            r.status.error.as_deref().unwrap().contains("denied"),
            "{:?}",
            r.status.error
        );
        assert!(!dir.join("val").exists());
        assert!(!dir.join("run").join(lg_voice::STATE_FILE).exists());
        // A state file left by a crashed run, with the feature now off: the
        // start gives LG's key back.
        s.power.luna_send = fake_configd(&dir, false);
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::write(dir.join("run").join(lg_voice::STATE_FILE), "configd 1\n").unwrap();
        std::fs::write(dir.join("val"), "ignore\n").unwrap();
        s.remote.mic_button = crate::config::MicButton::Off;
        let (_r2, p2) = crate::evdev::tests::fake_inputs("micref2", &[]);
        let r = setup_remote(&s, p2).await;
        assert_eq!(r.status.lg_voice_key, "lg");
        assert!(!dir.join("val").exists());
        assert!(!dir.join("run").join(lg_voice::STATE_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_heartbeat_keeps_the_loop_watchdog_fed_during_a_long_takeover() {
        let beats = Arc::new(Beats::default());
        let hb = Heartbeat::start(&beats);
        // A takeover that waits (a held lock, a slow configd) on the loop.
        tokio::time::sleep(Duration::from_millis(700)).await;
        let n = beats.event_loop.load(Ordering::Relaxed);
        assert!(n >= 2, "{n} beats in 700 ms");
        drop(hb);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let m = beats.event_loop.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            beats.event_loop.load(Ordering::Relaxed),
            m,
            "stops when dropped"
        );
    }

    #[test]
    fn the_state_file_lives_under_the_install_not_the_report_dir() {
        let mut s = Settings::default();
        s.report.dir = Some(PathBuf::from("/elsewhere/reports"));
        assert_eq!(lg_state_dir(&s), PathBuf::from("/elsewhere/reports"));
        s.install_prefix = Some(PathBuf::from("/var/lib/webos-satellite"));
        assert_eq!(
            lg_state_dir(&s),
            PathBuf::from("/var/lib/webos-satellite/run")
        );
    }

    #[tokio::test]
    async fn off_without_luna_send_reports_a_leftover_as_unknown() {
        let dir = test_dir("micoffleft");
        let s = no_luna(&dir);
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::write(dir.join("run").join(lg_voice::STATE_FILE), "configd 1 t\n").unwrap();
        let (_root, paths) = crate::evdev::tests::fake_inputs("micoffleft", &[]);
        let r = setup_remote(&s, paths).await;
        assert_eq!(r.status.state, "off");
        assert_eq!(r.status.lg_voice_key, "unknown");
        assert!(dir.join("run").join(lg_voice::STATE_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_persistent_change_after_the_takeover_stops_the_mic_button_loudly() {
        let dir = test_dir("micrecheck");
        std::fs::create_dir_all(dir.join("run")).unwrap();
        let luna = fake_configd(&dir, false);
        let db = dir.join("configd_db.json");
        std::fs::write(&db, "{}").unwrap();
        let mut t =
            Takeover::with_persistent(LunaSend(luna), &dir.join("run"), std::slice::from_ref(&db));
        t.acquire().await.unwrap();
        let (mut d, mut rx) = test_daemon(&dir, RemoteOff::Normal, false);
        d.config_switches = Some(ConfigSwitches::for_test(true, false));
        d.lg_sensor_listed = true;
        d.lg_takeover = Some(t);
        // Unchanged: nothing happens.
        d.lg_recheck().await;
        assert!(d.lg_takeover.is_some());
        // configd wrote its database meanwhile.
        std::fs::write(&db, "{\"changed\":1}").unwrap();
        d.lg_recheck().await;
        assert!(d.lg_takeover.is_none());
        assert!(!dir.join("val").exists(), "LG's value is back");
        assert!(!dir.join("run").join(lg_voice::STATE_FILE).exists());
        let st = d.mic_status.clone().unwrap();
        assert_eq!((st.state, st.lg_voice_key), ("failed", "lg"));
        assert!(st.error.unwrap().contains("configd_db.json changed"));
        let mut cmds = Vec::new();
        while let Ok(c) = rx.try_recv() {
            cmds.push(c);
        }
        assert!(cmds.contains(&AppCommand::SetSwitch {
            object_id: config_switch::MIC_BUTTON_ID.to_owned(),
            state: Some(false)
        }));
        assert!(cmds.iter().any(|c| matches!(c, AppCommand::SetTextSensor { object_id, state: Some(t) }
            if object_id == LG_VOICE_KEY_ID && t.contains("mic button off: configd's persistent database"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_stop_ends_the_grab_helper_before_lgs_key_is_given_back() {
        let dir = test_dir("micorder");
        std::fs::create_dir_all(dir.join("run")).unwrap();
        // configd: on the restore (null) it records whether the helper had
        // already exited.
        let luna = dir.join("luna-send");
        crate::testutil::write_script(
            &luna,
            &format!(
                r#"D='{d}'
case "$5" in
*/setConfigs)
  case "$6" in
  *'"action":"ignore"'*) echo ignore > "$D/val" ;;
  *null*) if [ -e "$D/helper-gone" ]; then echo gone; else echo running; fi > "$D/order"; rm -f "$D/val" ;;
  esac
  echo '{{"returnValue":true}}' ;;
*/getConfigs)
  v=$(cat "$D/val" 2>/dev/null || echo launch)
  printf '{{"configs":{{"com.webos.keyaction.voice":{{"action":"%s"}}}},"returnValue":true}}\n' "$v" ;;
*) exit 3 ;;
esac"#,
                d = dir.display()
            ),
        );
        // The helper: grabs, beats, and on its stdin's end takes 300 ms to
        // release before it marks itself gone.
        let helper = dir.join("remote-proxy");
        crate::testutil::write_script(
            &helper,
            &format!(
                "echo 'ready event2 event4'; ( while :; do echo 'hb 0 0 0'; sleep 0.2; done ) & b=$!; \
                 cat >/dev/null; kill $b; sleep 0.3; touch '{d}/helper-gone'; echo 'end stdin end of file'",
                d = dir.display()
            ),
        );
        let db = dir.join("configd_db.json");
        std::fs::write(&db, "{}").unwrap();
        let mut t =
            Takeover::with_persistent(LunaSend(luna), &dir.join("run"), std::slice::from_ref(&db));
        t.acquire().await.unwrap();
        let section = crate::config::RemoteSection {
            mic_button: crate::config::MicButton::Ptt,
            remote_mic: false,
            ..crate::config::RemoteSection::default()
        };
        let plan = crate::key_grab::Plan {
            helper,
            target: "LGE M-RCU - Builtin [2]".into(),
            require: "LGE M-RCU - Builtin [0]".into(),
            run_dir: dir.join("run"),
        };
        let st = Button::start(&section, None, 120.0, KeySource::Grab(plan)).unwrap();
        let (mut d, _rx) = test_daemon(&dir, RemoteOff::Normal, false);
        d.grab_status = st.grab_status.clone();
        d.remote = Some(st.button);
        d.mic_status = Some(MicButtonStatus {
            state: "ptt",
            lg_voice_key: KeyOwner::Ours.word(),
            error: None,
            key_grab: Some("starting".into()),
        });
        d.lg_takeover = Some(t);
        // Wait until the helper grabs: the live status then says "no toast".
        for _ in 0..40 {
            if d.live_mic_status().unwrap().lg_voice_key == "ours (grab + configd)" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            d.live_mic_status().unwrap().lg_voice_key,
            "ours (grab + configd)"
        );
        std::fs::write(&db, "{\"changed\":1}").unwrap();
        d.lg_recheck().await;
        assert_eq!(
            std::fs::read_to_string(dir.join("order")).unwrap().trim(),
            "gone",
            "LG's key was given back while the helper still ran"
        );
        assert!(d.remote.is_none() && d.lg_takeover.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn mic_button_on_without_a_takeover_does_not_run_and_says_why() {
        let dir = test_dir("micfail");
        let mut s = no_luna(&dir);
        s.remote.mic_button = crate::config::MicButton::Ptt;
        // No key device.
        let (_r1, empty) = crate::evdev::tests::fake_inputs("micfail-none", &[]);
        let r = setup_remote(&s, empty).await;
        assert!(r.started.is_none());
        assert_eq!(r.status.state, "failed");
        assert!(
            r.status
                .error
                .as_deref()
                .unwrap()
                .contains("no input device")
        );
        // The device is there but luna-send is not: LG's key cannot be
        // taken over, so the feature does not run.
        let (_r2, paths) =
            crate::evdev::tests::fake_inputs("micfail-dev", &[(2, "LGE M-RCU - Builtin [0]")]);
        let r = setup_remote(&s, paths).await;
        assert!(r.started.is_none() && r.takeover.is_none());
        assert_eq!(r.status.state, "failed");
        assert_eq!(r.status.lg_voice_key, "lg");
        assert!(
            r.sensor_text()
                .starts_with("LG assistant; mic button off: "),
            "{}",
            r.sensor_text()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn busy_follows_run_music_and_alarm() {
        let dir = test_dir("busy-word");
        let (mut d, _rx, mut mrx, mut drx) = music_daemon(&dir);
        let file = dir.join("run").join(crate::voice_status::FILE);
        let read = |d: &mut Daemon| {
            d.publish_voice_status();
            std::fs::read_to_string(&file).unwrap()
        };
        assert_eq!(d.busy(), None);
        assert!(read(&mut d).contains("\"ha_since\":1762000000,\"busy\":null,\"music\":"));
        // Alarm alone.
        assert!(d.alarm.on_finished("t1", std::time::Instant::now()));
        assert_eq!(d.busy(), Some(Busy::Alarm));
        assert!(read(&mut d).contains("\"busy\":\"alarm\""));
        // Music is listed before the alarm.
        tone(&mut d);
        drive(&mut d, &mut mrx, &mut drx, 50).await;
        assert_eq!(d.busy(), Some(Busy::Music));
        assert!(read(&mut d).contains("\"busy\":\"music\""));
        // A voice run wins over both.
        d.in_run = true;
        assert_eq!(d.busy(), Some(Busy::Voice));
        assert!(read(&mut d).contains("\"busy\":\"voice\""));
        d.in_run = false;
        assert_eq!(d.busy(), Some(Busy::Music));
        // The player (a reply streaming in) is a voice run too.
        d.player.start_stream(16_000, 1);
        assert_eq!(d.busy(), Some(Busy::Voice));
    }

    #[tokio::test]
    async fn media_stop_stops_the_alarm_first_then_the_music() {
        use webos_esphome_api::MusicState as S;
        let dir = test_dir("music-stop");
        let (mut d, mut rx, mut mrx, mut drx) = music_daemon(&dir);
        tone(&mut d);
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        assert_eq!(
            music_states(&mut rx),
            vec![AppCommand::SetMusicState(S::Playing)]
        );
        d.on_event(AppEvent::Timer {
            event: TimerEvent::Finished,
            id: "t1".to_owned(),
            name: String::new(),
            total_seconds: 5,
            seconds_left: 0,
            is_active: false,
        })
        .await;
        assert_ne!(d.alarm.phase(), Phase::Idle);
        drive(&mut d, &mut mrx, &mut drx, 50).await;
        // The chime ducks the music.
        assert!(
            d.music
                .as_ref()
                .unwrap()
                .status_json(Instant::now())
                .contains("\"ducked\":true")
        );
        // The first stop is for the alarm; the music goes on.
        d.on_event(AppEvent::MediaStop).await;
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        assert_eq!(d.alarm.phase(), Phase::Idle);
        assert!(music_states(&mut rx).is_empty(), "music still playing");
        // The second stops the music.
        d.on_event(AppEvent::MediaStop).await;
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        assert_eq!(
            music_states(&mut rx),
            vec![AppCommand::SetMusicState(S::Idle)]
        );
        // A third finds nothing to stop.
        d.on_event(AppEvent::MediaStop).await;
        d.music.take().unwrap().shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn active_standby_stops_the_music_and_refuses_play_media() {
        use webos_esphome_api::MusicState as S;
        let dir = test_dir("music-standby");
        let (mut d, mut rx, mut mrx, mut drx) = music_daemon(&dir);
        tone(&mut d);
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        d.on_power(PowerUpdate {
            state: Some("Active Standby".to_owned()),
            processing: Some("Request Power Off".to_owned()),
            ..PowerUpdate::default()
        })
        .await;
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        assert_eq!(
            music_states(&mut rx),
            vec![
                AppCommand::SetMusicState(S::Playing),
                AppCommand::SetMusicState(S::Idle)
            ]
        );
        // Refused there: play_media and an announcement (both only released).
        for announce in [false, true] {
            d.on_event(AppEvent::Music(MusicCommand::Play {
                url: "http://192.0.2.7:9/api/esphome/ffmpeg_proxy/d/t.flac".to_owned(),
                announce,
            }))
            .await;
        }
        assert!(d.player.current_kind().is_none());
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        assert!(music_states(&mut rx).is_empty());
        let v = d.music.as_ref().unwrap().status_json(Instant::now());
        assert!(
            v.contains("\"last_end\":\"standby\"") && v.contains("\"standby\":true"),
            "{v}"
        );
        // Back to Active: accepted again (the mic mute does not matter).
        d.on_power(PowerUpdate {
            state: Some("Active".to_owned()),
            ..PowerUpdate::default()
        })
        .await;
        d.on_event(AppEvent::MuteChanged(true)).await;
        tone(&mut d);
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        assert_eq!(
            music_states(&mut rx),
            vec![AppCommand::SetMusicState(S::Playing)]
        );
        // A resume from suspend leaves the music stopped.
        d.on_resume("test").await;
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        assert_eq!(
            music_states(&mut rx),
            vec![AppCommand::SetMusicState(S::Idle)]
        );
        d.music.take().unwrap().shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn media_announce_ducks_and_tells_ha_no_announce_finished() {
        let dir = test_dir("music-announce");
        let (mut d, mut rx, mut mrx, mut drx) = music_daemon(&dir);
        tone(&mut d);
        drive(&mut d, &mut mrx, &mut drx, 100).await;
        let _ = music_states(&mut rx);
        // A loopback URL: the fetch guard refuses it, the announcement fails.
        d.on_event(AppEvent::Music(MusicCommand::Play {
            url: "http://127.0.0.1:9/api/tts_proxy/secret.flac".to_owned(),
            announce: true,
        }))
        .await;
        assert_eq!(d.player.current_kind(), Some(Kind::MediaAnnounce));
        d.sync_music().await;
        assert!(
            d.music
                .as_ref()
                .unwrap()
                .status_json(Instant::now())
                .contains("\"ducked\":true")
        );
        drive(&mut d, &mut mrx, &mut drx, 300).await;
        assert!(d.player.current_kind().is_none());
        assert!(
            music_states(&mut rx).is_empty(),
            "no AnnounceFinished, the music state stays playing"
        );
        // A voice run ducks; its end releases after the hold.
        d.on_event(AppEvent::WakeWordDetected).await;
        d.sync_music().await;
        assert!(
            d.music
                .as_ref()
                .unwrap()
                .status_json(Instant::now())
                .contains("\"ducked\":true")
        );
        d.on_event(AppEvent::RunEnd).await;
        drive(&mut d, &mut mrx, &mut drx, 900).await;
        assert!(
            d.music
                .as_ref()
                .unwrap()
                .status_json(Instant::now())
                .contains("\"ducked\":false")
        );
        // HA disconnect: the music plays on for disconnect_stop_s (1 s), then stops.
        d.on_event(AppEvent::HaDisconnected).await;
        drive(&mut d, &mut mrx, &mut drx, 500).await;
        assert!(music_states(&mut rx).is_empty());
        drive(&mut d, &mut mrx, &mut drx, 800).await;
        assert_eq!(
            music_states(&mut rx),
            vec![AppCommand::SetMusicState(
                webos_esphome_api::MusicState::Idle
            )]
        );
        let v = d.music.as_ref().unwrap().status_json(Instant::now());
        assert!(
            v.contains("\"last_end\":\"ha_gone\"") && !v.contains("secret"),
            "{v}"
        );
        d.music.take().unwrap().shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The overlay end to end in the daemon: the frames of a wake word run
    /// reach `luna-send` as SAM launches of the overlay app, with the texts
    /// in the parameters, in order, only while the screen gate is open:
    /// nothing before the power state is known, nothing once a power-off is
    /// in progress ("apaga la tele": the answer and the re-send land during
    /// the power-off), and no re-send through a closed gate.
    #[tokio::test]
    async fn overlay_frames_reach_the_app_only_while_the_screen_is_on() {
        let dir = test_dir("overlay");
        let (mut d, _rx) = test_daemon(&dir, RemoteOff::Normal, false);
        let fake = dir.join("luna-send");
        let out = dir.join("launches");
        crate::testutil::write_script(
            &fake,
            &format!(
                "printf '%s %s %s %s\\n' \"$3\" \"$4\" \"$5\" \"$6\" >> '{}'\necho '{{\"returnValue\":true}}'",
                out.display()
            ),
        );
        let (screen_tx, screen_rx) = watch::channel(false);
        d.screen_tx = screen_tx;
        d.overlay_tx = Some(overlay::Sender::spawn(
            fake,
            "io.example.overlay".into(),
            screen_rx,
        ));
        let read = || std::fs::read_to_string(&out).unwrap_or_default();
        let lines = |n: usize| {
            let out = out.clone();
            async move {
                for _ in 0..300 {
                    let t = std::fs::read_to_string(&out).unwrap_or_default();
                    if t.lines().count() >= n {
                        return t;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                std::fs::read_to_string(&out).unwrap_or_default()
            }
        };
        let power = |state: &str, processing: Option<&str>| PowerUpdate {
            state: Some(state.into()),
            processing: processing.map(Into::into),
            reason: None,
            on_off: None,
        };
        // The power state is not known yet: the gate is closed.
        d.on_event(AppEvent::WakeWordDetected).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(read(), "", "launched before the power state was known");
        d.on_event(AppEvent::RunEnd).await;
        // The TV shows a picture: run 2 is shown.
        d.last_power = Some(power("Active", None));
        d.update_screen_gate();
        d.on_event(AppEvent::StartStreaming {
            wake_word: true,
            conversation_id: None,
            push_to_talk: false,
        })
        .await;
        d.on_event(AppEvent::WakeWordDetected).await;
        let t = lines(1).await;
        assert!(
            t.starts_with(r#"-w 1500 luna://com.webos.applicationManager/launch {"id":"io.example.overlay","params":{"overlay":1,"inst":7,"seq":2,"stage":"listening","heard":"","answer":"","error":null}}"#),
            "{t}"
        );
        d.on_event(AppEvent::Transcript("apaga la tele".into()))
            .await;
        let t = lines(2).await;
        assert!(
            t.lines()
                .nth(1)
                .unwrap()
                .contains(r#""seq":2,"stage":"thinking","heard":"apaga la tele""#),
            "{t}"
        );
        // HA turns the TV off: the state still says Active, with the
        // power-off in progress. The reply and the end of the run must not
        // launch the app, and neither may the re-send of the run's start.
        d.last_power = Some(power("Active", Some("Request Power Off")));
        d.update_screen_gate();
        // The run's re-send (2.5 s after its first frame) finds the gate closed.
        tokio::time::sleep(overlay::COLD_SETTLE + Duration::from_millis(400)).await;
        assert_eq!(read().lines().count(), 2, "{}", read());
        d.on_event(AppEvent::IntentEnd {
            speech: "Apagando.".into(),
            continue_conversation: false,
        })
        .await;
        d.on_event(AppEvent::RunEnd).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let t = read();
        assert_eq!(t.lines().count(), 2, "{t}");
        assert!(!t.contains("Apagando"), "{t}");
        // The screen is off; the next run is not shown either.
        d.last_power = Some(power("Screen Off", None));
        d.update_screen_gate();
        d.on_event(AppEvent::WakeWordDetected).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(read().lines().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
