//! Voice-assistant state machine (pure logic, no I/O).
//!
//! Inputs are HA messages, application commands and time; outputs are messages
//! to HA ([`ToHa`]) and events to the application ([`AppEvent`]). The server
//! module performs the I/O. See the crate documentation for the diagram.

use tokio::time::{Duration, Instant};

use crate::app::{AppEvent, PipelineMode};
use crate::proto::VoiceAssistantEvent as Ev;

/// `VoiceAssistantRequest.flags` bit: start at the wake word stage.
pub(crate) const FLAG_USE_WAKE_WORD: u32 = 2;
/// How long HA may take to answer a start request.
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(10);
/// A conversation id is reused for this long (ESPHome default 300 s).
pub(crate) const CONVERSATION_TIMEOUT: Duration = Duration::from_secs(300);
/// First retry delay after a failed run.
pub(crate) const BACKOFF_BASE: Duration = Duration::from_millis(250);
/// First retry delay when HA reports a configuration error (missing engine).
pub(crate) const BACKOFF_CONFIG_BASE: Duration = Duration::from_secs(2);
/// Runs that HA ended without an error and without progress (HA cancelled the
/// idle wake word run: an announcement, a timer alarm, a reconfiguration) in a
/// row before the re-arm delay starts to grow. They are not failures.
pub(crate) const SOFT_ENDS_FREE: u32 = 3;
/// Retry delay cap.
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// SPEAKER mode: give up waiting for `TTS_STREAM_START` after `TTS_END` after
/// this long (ESPHome's `tts_playback_start_timeout` default is also 2 s).
pub(crate) const TTS_STREAM_START_TIMEOUT: Duration = Duration::from_secs(2);
/// Format of TTS audio streamed by HA in SPEAKER mode (HA 2026.10.4 forces
/// WAV 16 kHz mono 16-bit and sends the PCM frames without the WAV header).
pub(crate) const TTS_STREAM_RATE: u32 = 16_000;
/// After aborting a run, wait this long at most for HA's `RUN_END` of the
/// aborted run before sending the next start request.
pub(crate) const ABORT_DRAIN_TIMEOUT: Duration = Duration::from_millis(1500);

/// Media player state reported to HA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaState {
    Idle,
    Announcing,
}

/// Message the state machine wants sent to HA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToHa {
    /// `VoiceAssistantRequest{start: true}`.
    Start {
        wake_word: bool,
        conversation_id: String,
    },
    /// `VoiceAssistantRequest{start: false}`: abort the run.
    Abort,
    /// `VoiceAssistantAudio{end: true}`: graceful end of the audio stream.
    AudioEnd,
    /// `VoiceAssistantAnnounceFinished{success}` (after TTS or announcement).
    AnnounceFinished(bool),
    /// `MediaPlayerStateResponse`.
    MediaState(MediaState),
    /// `SwitchStateResponse` of the mute switch.
    MuteState(bool),
}

/// Output of the state machine.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Out {
    Ha(ToHa),
    App(AppEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nothing to do (not subscribed, not armed, or muted).
    Idle,
    /// Re-arm at `until` (backoff after a failure).
    Waiting { until: Instant },
    /// Start request sent; waiting for `VoiceAssistantResponse`.
    Starting { since: Instant },
    /// Pipeline run in progress (see [`Run`]).
    Running,
    /// Announcement handed to the app; waiting for `AnnounceFinished`.
    Announcing { start_conversation: bool },
    /// A run was aborted and HA has not sent its `RUN_END` yet. The next
    /// start waits for it: HA 2026.10.4 ends the aborted run asynchronously,
    /// and its trailing events (`ERROR`, `RUN_END`) would otherwise be taken
    /// for the new run's, ending it at once (seen on c3-a with the real
    /// integration: an endless abort/restart cascade).
    Draining { since: Instant, wake_word: bool },
}

/// SPEAKER-mode TTS stream of the current run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum TtsStream {
    #[default]
    None,
    /// `TTS_END` seen; HA should send `TTS_STREAM_START` shortly.
    Expected { since: Instant },
    /// Between `TTS_STREAM_START` and `TTS_STREAM_END`.
    Active,
}

#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
struct Run {
    wake_word: bool,
    /// Audio is being forwarded to HA.
    streaming: bool,
    /// Wake word detected or STT produced text.
    progress: bool,
    error: Option<String>,
    /// TTS (URL or stream) handed to the app; waiting for `PlaybackFinished`.
    playback_pending: bool,
    tts_stream: TtsStream,
    ended: bool,
    continue_conversation: bool,
}

/// Voice-assistant state of one device (survives reconnects: `armed`, `muted`).
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Voice {
    phase: Phase,
    run: Run,
    subscribed: bool,
    armed: bool,
    muted: bool,
    /// The application's capture delivers audio (`AppCommand::CaptureLive`;
    /// true until told otherwise). No run is open while it is false.
    capture_live: bool,
    /// SPEAKER feature advertised: TTS arrives as audio over the API.
    speaker: bool,
    failures: u32,
    /// Runs in a row that HA ended without an error or any progress (see
    /// [`SOFT_ENDS_FREE`]); reset by progress and by every announcement.
    soft_ends: u32,
    conversation: Option<(String, Instant)>,
    /// An abort was sent and the aborted run's `RUN_END` has not arrived.
    stale_run: bool,
    out: Vec<Out>,
}

/// Error codes that mean the HA pipeline is misconfigured; retry slowly.
fn is_config_error(code: &str) -> bool {
    code.ends_with("-missing") || code.contains("not-found") || code.ends_with("-not-supported")
}

pub(crate) fn backoff_delay(failures: u32, config_error: bool) -> Duration {
    let base = if config_error {
        BACKOFF_CONFIG_BASE
    } else {
        BACKOFF_BASE
    };
    let shift = failures.saturating_sub(1).min(16);
    base.saturating_mul(1 << shift).min(BACKOFF_MAX)
}

impl Voice {
    pub(crate) fn new(speaker: bool) -> Self {
        Self {
            phase: Phase::Idle,
            run: Run::default(),
            subscribed: false,
            armed: false,
            muted: false,
            capture_live: true,
            speaker,
            failures: 0,
            soft_ends: 0,
            conversation: None,
            stale_run: false,
            out: Vec::new(),
        }
    }

    /// Drains the pending outputs.
    pub(crate) fn take(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    pub(crate) fn is_streaming(&self) -> bool {
        matches!(self.phase, Phase::Running) && self.run.streaming
    }

    pub(crate) fn is_muted(&self) -> bool {
        self.muted
    }

    pub(crate) fn media_state(&self) -> MediaState {
        match self.phase {
            Phase::Announcing { .. } => MediaState::Announcing,
            Phase::Running
                if self.run.playback_pending || self.run.tts_stream != TtsStream::None =>
            {
                MediaState::Announcing
            }
            _ => MediaState::Idle,
        }
    }

    /// Next time [`Voice::on_tick`] has work to do.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Waiting { until } => Some(until),
            Phase::Starting { since } => Some(since + START_TIMEOUT),
            Phase::Draining { since, .. } => Some(since + ABORT_DRAIN_TIMEOUT),
            Phase::Running => match self.run.tts_stream {
                TtsStream::Expected { since } => Some(since + TTS_STREAM_START_TIMEOUT),
                _ => None,
            },
            _ => None,
        }
    }

    fn ha(&mut self, msg: ToHa) {
        self.out.push(Out::Ha(msg));
    }

    fn app(&mut self, ev: AppEvent) {
        self.out.push(Out::App(ev));
    }

    fn conversation_id(&self, now: Instant) -> Option<String> {
        self.conversation
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < CONVERSATION_TIMEOUT)
            .map(|(id, _)| id.clone())
    }

    /// A run may be open: subscribed, not muted, and the capture delivers audio.
    fn can_listen(&self) -> bool {
        self.subscribed && !self.muted && self.capture_live
    }

    fn start(&mut self, now: Instant, wake_word: bool) {
        if !self.can_listen() {
            // Not an error: `on_capture(true)` or an unmute re-arms an armed
            // wake word. A wake word session opened without audio would block
            // HA's cancel before an announcement (see `AppCommand::CaptureLive`).
            if self.subscribed && !self.muted {
                log::debug!("voice: no run while the microphone delivers no audio");
            }
            self.phase = Phase::Idle;
            return;
        }
        if self.stale_run {
            self.phase = Phase::Draining {
                since: now,
                wake_word,
            };
            return;
        }
        let conversation_id = self.conversation_id(now).unwrap_or_default();
        self.ha(ToHa::Start {
            wake_word,
            conversation_id,
        });
        self.run = Run {
            wake_word,
            ..Run::default()
        };
        self.phase = Phase::Starting { since: now };
    }

    /// Re-arms the wake word if armed, otherwise idles.
    fn rearm(&mut self, now: Instant) {
        if self.armed {
            self.start(now, true);
        } else {
            self.phase = Phase::Idle;
        }
    }

    fn stop_streaming(&mut self, graceful: bool) {
        if self.run.streaming {
            self.run.streaming = false;
            if graceful {
                self.ha(ToHa::AudioEnd);
            }
            self.app(AppEvent::StopStreaming);
        }
    }

    /// Aborts a run that is starting or in progress (HA is told to abort).
    fn abort_run(&mut self) {
        match self.phase {
            Phase::Starting { .. } => {
                self.ha(ToHa::Abort);
                self.stale_run = true;
            }
            Phase::Running => {
                if !self.run.ended {
                    self.ha(ToHa::Abort);
                    self.stale_run = true;
                }
                self.stop_streaming(false);
                if self.run.playback_pending || self.run.tts_stream != TtsStream::None {
                    self.app(AppEvent::StopPlayback);
                    self.ha(ToHa::MediaState(MediaState::Idle));
                }
            }
            _ => {}
        }
        self.run = Run::default();
    }

    fn fail(&mut self, now: Instant, config_error: bool) {
        self.failures = self.failures.saturating_add(1);
        let delay = backoff_delay(self.failures, config_error);
        log::info!(
            "voice: run failed ({} in a row); retry in {} ms",
            self.failures,
            delay.as_millis()
        );
        if self.armed && self.can_listen() {
            self.phase = Phase::Waiting { until: now + delay };
        } else {
            self.phase = Phase::Idle;
        }
    }

    /// A wake word or a transcript: the run did something, so both backoffs reset.
    fn made_progress(&mut self) {
        self.run.progress = true;
        self.failures = 0;
        self.soft_ends = 0;
    }

    /// HA ended a run without an error and nothing happened in it: HA
    /// cancelled it (its announcement, a timer alarm and other services first
    /// cancel the idle wake word pipeline, which ends with `RUN_END`). That is
    /// no failure: the failure backoff is left alone and the wake word is
    /// requested again after the first delay (an announcement re-arms it
    /// itself after its playback). Only a stream of such ends with no
    /// announcement between them is slowed down, so a misbehaving HA cannot
    /// make the device re-request in a hot loop.
    fn soft_end(&mut self, now: Instant) {
        self.soft_ends = self.soft_ends.saturating_add(1);
        let delay = if self.soft_ends <= SOFT_ENDS_FREE {
            BACKOFF_BASE
        } else {
            backoff_delay(self.soft_ends - SOFT_ENDS_FREE, false)
        };
        log::info!(
            "voice: HA ended the run without an error ({} in a row); asking again in {} ms",
            self.soft_ends,
            delay.as_millis()
        );
        if self.armed && self.can_listen() {
            self.phase = Phase::Waiting { until: now + delay };
        } else {
            self.phase = Phase::Idle;
        }
    }

    /// Both the run and its playback are over: decide what comes next.
    fn finish_run(&mut self, now: Instant) {
        let run = std::mem::take(&mut self.run);
        if run.error.is_some() {
            let config_error = run.error.as_deref().is_some_and(is_config_error);
            self.fail(now, config_error);
        } else if !run.progress {
            self.soft_end(now);
        } else if run.continue_conversation {
            // Follow-up turn: no wake word, same conversation id.
            self.start(now, false);
        } else {
            self.rearm(now);
        }
    }

    // ---- HA inputs ----

    pub(crate) fn on_subscribe(&mut self, now: Instant, subscribe: bool) {
        if subscribe {
            let was = self.subscribed;
            self.subscribed = true;
            if !was && matches!(self.phase, Phase::Idle) {
                self.rearm(now);
            }
        } else {
            self.abort_run();
            self.subscribed = false;
            self.phase = Phase::Idle;
        }
    }

    pub(crate) fn on_disconnect(&mut self) {
        self.stop_streaming(false);
        // SPEAKER mode: a reply whose stream already ended is complete on the
        // device and finishes playing; HA needs no report for it. A reply
        // still streaming (or a URL/announcement that may need HA) stops.
        let received_reply =
            self.speaker && self.run.playback_pending && self.run.tts_stream == TtsStream::None;
        if (self.run.playback_pending && !received_reply)
            || self.run.tts_stream != TtsStream::None
            || matches!(self.phase, Phase::Announcing { .. })
        {
            self.app(AppEvent::StopPlayback);
        }
        self.run = Run::default();
        self.subscribed = false;
        self.stale_run = false;
        self.phase = Phase::Idle;
        self.out.retain(|o| matches!(o, Out::App(_)));
    }

    pub(crate) fn on_response(&mut self, now: Instant, port: u32, error: bool) {
        if !matches!(self.phase, Phase::Starting { .. }) {
            log::debug!("voice: ignoring VoiceAssistantResponse outside a start");
            return;
        }
        if error {
            log::warn!("voice: HA could not start the pipeline");
            self.run = Run::default();
            self.fail(now, false);
            return;
        }
        if port != 0 {
            log::warn!("voice: HA offered UDP port {port}; this device only streams over the API");
        }
        self.begin_streaming(now);
    }

    fn begin_streaming(&mut self, now: Instant) {
        self.phase = Phase::Running;
        self.run.streaming = true;
        let conversation_id = self.conversation_id(now);
        self.app(AppEvent::StartStreaming {
            wake_word: self.run.wake_word,
            conversation_id,
        });
    }

    pub(crate) fn on_event(&mut self, now: Instant, event_type: i32, data: &[(String, String)]) {
        let get = |key: &str| data.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let Ok(ev) = Ev::try_from(event_type) else {
            log::debug!("voice: unknown event type {event_type}");
            return;
        };
        if self.stale_run {
            // No start was sent since the abort, so this is the old run.
            if ev == Ev::VoiceAssistantRunEnd {
                self.stale_run = false;
                if let Phase::Draining { wake_word, .. } = self.phase {
                    self.start(now, wake_word);
                }
            } else {
                log::debug!("voice: ignoring {} of the aborted run", ev.as_str_name());
            }
            return;
        }
        match self.phase {
            Phase::Running => {}
            // Events can overtake the response; treat them as an implicit start.
            Phase::Starting { .. } => self.begin_streaming(now),
            _ => {
                log::debug!("voice: ignoring {} outside a run", ev.as_str_name());
                return;
            }
        }
        match ev {
            Ev::VoiceAssistantWakeWordEnd => {
                self.made_progress();
                self.app(AppEvent::WakeWordDetected);
            }
            Ev::VoiceAssistantSttVadEnd => self.stop_streaming(true),
            Ev::VoiceAssistantSttEnd => {
                self.stop_streaming(true);
                self.made_progress();
                self.app(AppEvent::Transcript(get("text").unwrap_or_default()));
            }
            Ev::VoiceAssistantIntentEnd => {
                if let Some(id) = get("conversation_id").filter(|id| !id.is_empty()) {
                    self.conversation = Some((id, now));
                }
                let cont = get("continue_conversation").is_some_and(|v| v == "1");
                self.run.continue_conversation = cont;
                self.app(AppEvent::IntentEnd {
                    speech: get("speech").unwrap_or_default(),
                    continue_conversation: cont,
                });
            }
            Ev::VoiceAssistantTtsEnd => {
                if let Some(url) = get("url").filter(|u| !u.is_empty()) {
                    if self.speaker {
                        // HA also sends the URL in SPEAKER mode, but streams the
                        // audio over the API; wait for TTS_STREAM_START.
                        log::debug!("voice: TTS_END in SPEAKER mode; awaiting stream");
                        if self.run.tts_stream == TtsStream::None {
                            self.run.tts_stream = TtsStream::Expected { since: now };
                        }
                    } else {
                        if self.run.playback_pending {
                            self.app(AppEvent::StopPlayback);
                        }
                        self.run.playback_pending = true;
                        self.ha(ToHa::MediaState(MediaState::Announcing));
                        self.app(AppEvent::TtsUrl(url));
                    }
                }
            }
            Ev::VoiceAssistantTtsStreamStart => {
                self.run.tts_stream = TtsStream::Active;
                self.run.playback_pending = true;
                self.ha(ToHa::MediaState(MediaState::Announcing));
                self.app(AppEvent::TtsStreamStart {
                    sample_rate: TTS_STREAM_RATE,
                    channels: 1,
                    sample_width: 2,
                });
            }
            Ev::VoiceAssistantTtsStreamEnd => {
                if self.run.tts_stream == TtsStream::Active {
                    self.app(AppEvent::TtsStreamEnd);
                }
                self.run.tts_stream = TtsStream::None;
                self.maybe_finish(now);
            }
            Ev::VoiceAssistantError => {
                let code = get("code").unwrap_or_default();
                let message = get("message").unwrap_or_default();
                log::info!("voice: HA error {code}");
                log::debug!("voice: HA error message: {message:?}");
                self.stop_streaming(false);
                self.run.error = Some(code.clone());
                self.app(AppEvent::Error { code, message });
            }
            Ev::VoiceAssistantRunEnd => {
                self.stop_streaming(false);
                self.run.ended = true;
                self.app(AppEvent::RunEnd);
                self.maybe_finish(now);
            }
            // RUN_START may carry a streaming TTS URL; this device waits for
            // TTS_END instead. The remaining events need no action.
            _ => log::debug!("voice: {}", ev.as_str_name()),
        }
    }

    /// Finishes the run once it ended and nothing is playing or streaming.
    fn maybe_finish(&mut self, now: Instant) {
        if self.run.ended && !self.run.playback_pending && self.run.tts_stream == TtsStream::None {
            self.finish_run(now);
        }
    }

    /// SPEAKER mode: TTS audio from HA (`VoiceAssistantAudio`, HA -> device).
    pub(crate) fn on_tts_audio(&mut self, data: Vec<u8>) {
        if matches!(self.phase, Phase::Running) && self.run.tts_stream == TtsStream::Active {
            if !data.is_empty() {
                self.app(AppEvent::TtsAudio(data));
            }
        } else {
            log::debug!(
                "voice: dropping {} bytes of TTS audio outside a stream",
                data.len()
            );
        }
    }

    pub(crate) fn on_announce(
        &mut self,
        url: String,
        preannounce: String,
        text: String,
        start_conversation: bool,
    ) {
        self.abort_run();
        // HA's cancel of the idle run that came with this announcement is
        // not a failure and does not add to any backoff.
        self.soft_ends = 0;
        self.phase = Phase::Announcing { start_conversation };
        self.ha(ToHa::MediaState(MediaState::Announcing));
        self.app(AppEvent::Announce {
            url,
            preannounce_url: Some(preannounce).filter(|p| !p.is_empty()),
            text,
            start_conversation,
        });
    }

    pub(crate) fn on_media_stop(&mut self) {
        if self.run.playback_pending
            || self.run.tts_stream != TtsStream::None
            || matches!(self.phase, Phase::Announcing { .. })
        {
            self.app(AppEvent::StopPlayback);
        }
    }

    // ---- application inputs ----

    pub(crate) fn on_start_pipeline(&mut self, now: Instant, mode: PipelineMode) {
        if self.muted {
            log::warn!("voice: start ignored while muted");
            return;
        }
        match mode {
            PipelineMode::WakeWord => {
                self.armed = true;
                if matches!(self.phase, Phase::Idle) {
                    self.rearm(now);
                }
            }
            PipelineMode::PushToTalk => {
                if !self.capture_live {
                    log::warn!("voice: push-to-talk ignored; the microphone delivers no audio");
                    return;
                }
                if matches!(self.phase, Phase::Announcing { .. }) {
                    log::warn!("voice: push-to-talk ignored during an announcement");
                    return;
                }
                if !self.subscribed {
                    log::warn!("voice: push-to-talk ignored; HA has not subscribed");
                    return;
                }
                self.abort_run();
                self.start(now, false);
            }
        }
    }

    pub(crate) fn on_stop_pipeline(&mut self) {
        self.armed = false;
        self.abort_run();
        if !matches!(self.phase, Phase::Announcing { .. }) {
            self.phase = Phase::Idle;
        }
    }

    pub(crate) fn on_playback_finished(&mut self, now: Instant, success: bool) {
        if !(matches!(self.phase, Phase::Running) && self.run.playback_pending) {
            log::debug!("voice: PlaybackFinished without pending TTS");
            return;
        }
        self.run.playback_pending = false;
        if !self.speaker {
            // URL mode: HA waits for this to mark the response finished. In
            // SPEAKER mode HA calls tts_response_finished itself after streaming.
            self.ha(ToHa::AnnounceFinished(success));
        }
        self.ha(ToHa::MediaState(MediaState::Idle));
        self.maybe_finish(now);
    }

    pub(crate) fn on_announce_finished(&mut self, now: Instant, success: bool) {
        let Phase::Announcing { start_conversation } = self.phase else {
            log::debug!("voice: AnnounceFinished without an announcement");
            return;
        };
        self.ha(ToHa::AnnounceFinished(success));
        self.ha(ToHa::MediaState(MediaState::Idle));
        if start_conversation {
            self.start(now, false);
        } else {
            self.rearm(now);
        }
    }

    /// Ends whatever uses the microphone (the mute switch, or a capture that
    /// stopped delivering): a run is aborted (HA is told, which also ends its
    /// audio stream), a reply that is playing finishes, and nothing re-arms
    /// until listening is possible again. An announcement is not touched.
    fn halt_listening(&mut self) {
        match self.phase {
            Phase::Running
                if self.run.playback_pending || self.run.tts_stream != TtsStream::None =>
            {
                // Let the response finish playing; the run ends without re-arming.
                self.stop_streaming(false);
            }
            Phase::Starting { .. }
            | Phase::Waiting { .. }
            | Phase::Running
            | Phase::Draining { .. } => {
                self.abort_run();
                self.phase = Phase::Idle;
            }
            Phase::Idle | Phase::Announcing { .. } => {}
        }
    }

    pub(crate) fn on_mute(&mut self, now: Instant, muted: bool) {
        if self.muted == muted {
            return;
        }
        self.muted = muted;
        self.ha(ToHa::MuteState(muted));
        self.app(AppEvent::MuteChanged(muted));
        if muted {
            self.halt_listening();
        } else if matches!(self.phase, Phase::Idle) {
            self.rearm(now);
        }
    }

    /// The capture started or stopped delivering audio
    /// (`AppCommand::CaptureLive`).
    pub(crate) fn on_capture(&mut self, now: Instant, live: bool) {
        if self.capture_live == live {
            return;
        }
        self.capture_live = live;
        if live {
            log::debug!("voice: the microphone delivers audio");
            if matches!(self.phase, Phase::Idle) {
                self.rearm(now);
            }
        } else {
            log::info!("voice: the microphone stopped delivering audio; no run stays open");
            self.halt_listening();
        }
    }

    /// The system resumed from a suspend: a wake word run waiting out a retry
    /// backoff is re-armed now (the suspend is no reason to keep waiting).
    pub(crate) fn on_resume(&mut self, now: Instant) {
        if let Phase::Waiting { .. } = self.phase {
            log::info!("voice: resumed; re-arming the wake word without waiting for the backoff");
            self.failures = 0;
            self.rearm(now);
        }
    }

    pub(crate) fn on_tick(&mut self, now: Instant) {
        match self.phase {
            Phase::Waiting { until } if now >= until => self.rearm(now),
            Phase::Draining { since, wake_word } if now >= since + ABORT_DRAIN_TIMEOUT => {
                log::debug!("voice: no RUN_END for the aborted run; starting anyway");
                self.stale_run = false;
                self.start(now, wake_word);
            }
            Phase::Starting { since } if now >= since + START_TIMEOUT => {
                log::warn!("voice: no VoiceAssistantResponse within {START_TIMEOUT:?}");
                self.ha(ToHa::Abort);
                self.run = Run::default();
                self.fail(now, false);
            }
            Phase::Running => {
                if let TtsStream::Expected { since } = self.run.tts_stream {
                    if now >= since + TTS_STREAM_START_TIMEOUT {
                        log::warn!("voice: SPEAKER mode but HA sent no TTS stream");
                        self.run.tts_stream = TtsStream::None;
                        self.ha(ToHa::MediaState(MediaState::Idle));
                        self.maybe_finish(now);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(v: &mut Voice, now: Instant, e: Ev, data: &[(&str, &str)]) {
        let data: Vec<(String, String)> = data
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        v.on_event(now, e as i32, &data);
    }

    fn armed_and_streaming(now: Instant) -> Voice {
        let mut v = Voice::new(false);
        v.on_start_pipeline(now, PipelineMode::WakeWord);
        assert!(v.take().is_empty(), "nothing before subscribe");
        v.on_subscribe(now, true);
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Start {
                wake_word: true,
                conversation_id: String::new()
            })]
        );
        v.on_response(now, 0, false);
        assert_eq!(
            v.take(),
            [Out::App(AppEvent::StartStreaming {
                wake_word: true,
                conversation_id: None
            })]
        );
        assert!(v.is_streaming());
        v
    }

    #[test]
    fn full_wake_word_run_then_rearm() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantRunStart, &[]);
        ev(&mut v, t0, Ev::VoiceAssistantWakeWordStart, &[]);
        ev(&mut v, t0, Ev::VoiceAssistantWakeWordEnd, &[]);
        assert_eq!(v.take(), [Out::App(AppEvent::WakeWordDetected)]);
        assert!(v.is_streaming());
        ev(&mut v, t0, Ev::VoiceAssistantSttStart, &[]);
        ev(&mut v, t0, Ev::VoiceAssistantSttVadEnd, &[]);
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::AudioEnd), Out::App(AppEvent::StopStreaming)]
        );
        assert!(!v.is_streaming());
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantSttEnd,
            &[("text", "turn on the light")],
        );
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantIntentEnd,
            &[
                ("conversation_id", "c1"),
                ("continue_conversation", "0"),
                ("speech", "Done"),
            ],
        );
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantTtsEnd,
            &[("url", "http://ha/tts.flac")],
        );
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        assert_eq!(
            v.take(),
            [
                Out::App(AppEvent::Transcript("turn on the light".into())),
                Out::App(AppEvent::IntentEnd {
                    speech: "Done".into(),
                    continue_conversation: false
                }),
                Out::Ha(ToHa::MediaState(MediaState::Announcing)),
                Out::App(AppEvent::TtsUrl("http://ha/tts.flac".into())),
                Out::App(AppEvent::RunEnd),
            ]
        );
        v.on_playback_finished(t0, true);
        assert_eq!(
            v.take(),
            [
                Out::Ha(ToHa::AnnounceFinished(true)),
                Out::Ha(ToHa::MediaState(MediaState::Idle)),
                Out::Ha(ToHa::Start {
                    wake_word: true,
                    conversation_id: "c1".into()
                }),
            ]
        );
    }

    #[test]
    fn continue_conversation_starts_without_wake_word() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantSttEnd, &[("text", "x")]);
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantIntentEnd,
            &[("conversation_id", "c9"), ("continue_conversation", "1")],
        );
        ev(&mut v, t0, Ev::VoiceAssistantTtsEnd, &[("url", "u")]);
        v.on_playback_finished(t0, true); // before RUN_END
        v.take();
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        let out = v.take();
        assert_eq!(
            out.last(),
            Some(&Out::Ha(ToHa::Start {
                wake_word: false,
                conversation_id: "c9".into()
            }))
        );
    }

    #[test]
    fn errors_back_off_exponentially() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantError,
            &[("code", "wake-engine-missing"), ("message", "m")],
        );
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        let out = v.take();
        assert!(out.contains(&Out::App(AppEvent::StopStreaming)));
        assert!(!out.iter().any(|o| matches!(o, Out::Ha(ToHa::Start { .. }))));
        assert_eq!(v.next_deadline(), Some(t0 + BACKOFF_CONFIG_BASE));
        v.on_tick(t0 + Duration::from_millis(1999));
        assert!(v.take().is_empty());
        let t1 = t0 + BACKOFF_CONFIG_BASE;
        v.on_tick(t1);
        assert!(matches!(
            v.take()[..],
            [Out::Ha(ToHa::Start {
                wake_word: true,
                ..
            })]
        ));
        // Second failure doubles the delay.
        v.on_response(t1, 0, false);
        ev(
            &mut v,
            t1,
            Ev::VoiceAssistantError,
            &[("code", "wake-provider-missing")],
        );
        ev(&mut v, t1, Ev::VoiceAssistantRunEnd, &[]);
        assert_eq!(v.next_deadline(), Some(t1 + 2 * BACKOFF_CONFIG_BASE));
    }

    #[test]
    fn resume_rearms_a_waiting_wake_word_at_once() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantError,
            &[("code", "wake-engine-missing")],
        );
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        assert_eq!(v.next_deadline(), Some(t0 + BACKOFF_CONFIG_BASE));
        let t1 = t0 + Duration::from_millis(100);
        v.on_resume(t1);
        assert!(matches!(
            v.take()[..],
            [Out::Ha(ToHa::Start {
                wake_word: true,
                ..
            })]
        ));
        // Not waiting: a resume changes nothing.
        v.on_resume(t1);
        assert!(v.take().is_empty());
    }

    #[test]
    fn run_with_an_error_and_no_progress_counts_as_failure() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantError,
            &[("code", "stt-stream-failed")],
        );
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        assert_eq!(v.next_deadline(), Some(t0 + BACKOFF_BASE));
        assert_eq!(v.failures, 1);
    }

    /// Re-arms a waiting run at `t` and answers the start.
    fn rearm_now(v: &mut Voice, t: Instant) {
        v.on_tick(t);
        assert!(v.take().iter().any(|o| is_start(o, true)), "start request");
        v.on_response(t, 0, false);
        v.take();
    }

    #[test]
    fn a_run_ended_by_ha_without_error_is_no_failure_and_rearms_after_the_base_delay() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        // HA cancelled the idle wake word run (its announcement does that).
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        assert_eq!(v.failures, 0);
        assert_eq!(v.next_deadline(), Some(t0 + BACKOFF_BASE));
        rearm_now(&mut v, t0 + BACKOFF_BASE);
    }

    #[test]
    fn many_announcements_in_a_row_re_arm_promptly_every_time() {
        let mut t = Instant::now();
        let mut v = armed_and_streaming(t);
        for n in 0..30 {
            // HA cancels the idle run (RUN_END, no error), then announces.
            ev(&mut v, t, Ev::VoiceAssistantRunEnd, &[]);
            v.take();
            assert_eq!(v.failures, 0, "announcement {n}");
            t += Duration::from_millis(400);
            v.on_announce("u".into(), String::new(), String::new(), false);
            v.take();
            // The announcement plays; afterwards the wake word returns at
            // once (never a delay that grew with the count).
            v.on_announce_finished(t, true);
            let out = v.take();
            assert!(
                out.iter().any(|o| is_start(o, true)),
                "announcement {n}: {out:?}"
            );
            assert_eq!(v.next_deadline(), Some(t + START_TIMEOUT));
            v.on_response(t, 0, false);
            v.take();
            assert!(v.is_streaming());
        }
        assert_eq!(v.failures, 0);
        assert_eq!(v.soft_ends, 0);
    }

    #[test]
    fn many_announcements_with_the_cancel_ending_the_run_late_still_rearm_promptly() {
        // The announcement arrives first, HA's RUN_END of the cancelled run
        // after it (as the stale-run logic expects).
        let mut t = Instant::now();
        let mut v = armed_and_streaming(t);
        for n in 0..30 {
            v.on_announce("u".into(), String::new(), String::new(), false);
            v.take();
            ev(&mut v, t, Ev::VoiceAssistantRunEnd, &[]);
            t += Duration::from_millis(300);
            v.on_announce_finished(t, true);
            assert!(
                v.take().iter().any(|o| is_start(o, true)),
                "announcement {n}"
            );
            v.on_response(t, 0, false);
            v.take();
        }
        assert_eq!(v.failures, 0);
    }

    #[test]
    fn a_timer_alarm_cancel_with_no_announcement_does_not_grow_the_error_backoff() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        let mut t = t0;
        for _ in 0..SOFT_ENDS_FREE {
            ev(&mut v, t, Ev::VoiceAssistantRunEnd, &[]);
            v.take();
            assert_eq!(v.next_deadline(), Some(t + BACKOFF_BASE));
            t += BACKOFF_BASE;
            rearm_now(&mut v, t);
        }
        assert_eq!(v.failures, 0);
        // A real error afterwards starts the error backoff from its base.
        ev(
            &mut v,
            t,
            Ev::VoiceAssistantError,
            &[("code", "stt-stream-failed")],
        );
        ev(&mut v, t, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        assert_eq!(v.next_deadline(), Some(t + BACKOFF_BASE));
    }

    #[test]
    fn runs_ended_by_ha_again_and_again_slow_down_but_never_count_as_failures() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        let mut t = t0;
        let mut last = Duration::ZERO;
        for n in 1..=40_u32 {
            ev(&mut v, t, Ev::VoiceAssistantRunEnd, &[]);
            v.take();
            let d = v.next_deadline().unwrap() - t;
            assert!(d >= last && d <= BACKOFF_MAX, "{n}: {d:?}");
            last = d;
            t += d;
            rearm_now(&mut v, t);
        }
        assert_eq!(v.failures, 0);
        assert_eq!(last, BACKOFF_MAX);
        // A wake word resets it.
        ev(&mut v, t, Ev::VoiceAssistantWakeWordEnd, &[]);
        assert_eq!(v.soft_ends, 0);
    }

    #[test]
    fn backoff_caps() {
        assert_eq!(backoff_delay(1, false), BACKOFF_BASE);
        assert_eq!(backoff_delay(3, false), BACKOFF_BASE * 4);
        assert_eq!(backoff_delay(100, true), BACKOFF_MAX);
    }

    #[test]
    fn start_timeout_and_error_response_retry() {
        let t0 = Instant::now();
        let mut v = Voice::new(false);
        v.on_subscribe(t0, true);
        v.on_start_pipeline(t0, PipelineMode::WakeWord);
        v.take();
        v.on_tick(t0 + START_TIMEOUT);
        assert_eq!(v.take(), [Out::Ha(ToHa::Abort)]);
        let t1 = t0 + START_TIMEOUT + BACKOFF_BASE;
        v.on_tick(t1);
        v.take();
        v.on_response(t1, 0, true);
        assert!(v.take().is_empty());
        assert_eq!(v.next_deadline(), Some(t1 + 2 * BACKOFF_BASE));
    }

    #[test]
    fn push_to_talk_is_one_shot() {
        let t0 = Instant::now();
        let mut v = Voice::new(false);
        v.on_subscribe(t0, true);
        assert!(v.take().is_empty());
        v.on_start_pipeline(t0, PipelineMode::PushToTalk);
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Start {
                wake_word: false,
                conversation_id: String::new()
            })]
        );
        v.on_response(t0, 0, false);
        ev(&mut v, t0, Ev::VoiceAssistantSttEnd, &[("text", "hi")]);
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        assert_eq!(v.next_deadline(), None);
        assert!(!v.is_streaming());
    }

    #[test]
    fn announce_aborts_run_and_start_conversation_follows() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_announce("http://a".into(), String::new(), "hello".into(), true);
        assert_eq!(
            v.take(),
            [
                Out::Ha(ToHa::Abort),
                Out::App(AppEvent::StopStreaming),
                Out::Ha(ToHa::MediaState(MediaState::Announcing)),
                Out::App(AppEvent::Announce {
                    url: "http://a".into(),
                    preannounce_url: None,
                    text: "hello".into(),
                    start_conversation: true
                }),
            ]
        );
        // Stale events of the aborted run are ignored.
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        assert!(v.take().is_empty());
        v.on_announce_finished(t0, true);
        assert_eq!(
            v.take(),
            [
                Out::Ha(ToHa::AnnounceFinished(true)),
                Out::Ha(ToHa::MediaState(MediaState::Idle)),
                Out::Ha(ToHa::Start {
                    wake_word: false,
                    conversation_id: String::new()
                }),
            ]
        );
    }

    #[test]
    fn mute_aborts_and_unmute_rearms() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_mute(t0, true);
        assert_eq!(
            v.take(),
            [
                Out::Ha(ToHa::MuteState(true)),
                Out::App(AppEvent::MuteChanged(true)),
                Out::Ha(ToHa::Abort),
                Out::App(AppEvent::StopStreaming),
            ]
        );
        v.on_start_pipeline(t0, PipelineMode::PushToTalk);
        assert!(v.take().is_empty());
        // HA ends the aborted run while the mic is muted.
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        assert!(v.take().is_empty());
        v.on_mute(t0, false);
        let out = v.take();
        assert!(matches!(
            out.last(),
            Some(Out::Ha(ToHa::Start {
                wake_word: true,
                ..
            }))
        ));
    }

    fn is_start(o: &Out, wake_word: bool) -> bool {
        matches!(o, Out::Ha(ToHa::Start { wake_word: w, .. }) if *w == wake_word)
    }

    /// A device whose capture does not deliver audio yet: armed, subscribed.
    fn armed_without_capture(now: Instant) -> Voice {
        let mut v = Voice::new(true);
        v.on_capture(now, false);
        v.on_start_pipeline(now, PipelineMode::WakeWord);
        v.on_subscribe(now, true);
        v
    }

    #[test]
    fn no_session_is_opened_while_the_capture_delivers_no_audio() {
        let t0 = Instant::now();
        let mut v = armed_without_capture(t0);
        // The case of c3-b: mic busy. Nothing is sent to HA, so HA has no
        // wake word session whose cancel would wait for an audio chunk.
        assert!(v.take().is_empty());
        assert!(!v.is_streaming());
        assert_eq!(v.next_deadline(), None);
        v.on_tick(t0 + Duration::from_secs(120));
        assert!(v.take().is_empty());
        // Audio flows: the wake word is armed at once.
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn capture_stopping_ends_a_streaming_wake_word_run_and_it_returns_with_the_audio() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_capture(t0, false);
        // Abort (HA also ends its audio stream on it) and no more audio.
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Abort), Out::App(AppEvent::StopStreaming)]
        );
        assert!(!v.is_streaming());
        // HA's trailing RUN_END of the aborted run does not restart anything.
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        assert!(v.take().is_empty());
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn capture_stopping_aborts_a_start_in_flight() {
        let t0 = Instant::now();
        let mut v = Voice::new(true);
        v.on_start_pipeline(t0, PipelineMode::WakeWord);
        v.on_subscribe(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
        v.on_capture(t0, false);
        assert_eq!(v.take(), [Out::Ha(ToHa::Abort)]);
        // HA's answer to the aborted start is ignored.
        v.on_response(t0, 0, false);
        assert!(v.take().is_empty());
        assert!(!v.is_streaming());
        assert_eq!(v.next_deadline(), None);
    }

    #[test]
    fn capture_stopping_cancels_a_retry_backoff() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantError,
            &[("code", "stt-no-text")],
        );
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        assert!(v.next_deadline().is_some(), "waiting out a backoff");
        v.on_capture(t0, false);
        assert!(v.take().is_empty());
        assert_eq!(v.next_deadline(), None);
        v.on_tick(t0 + Duration::from_secs(120));
        assert!(v.take().is_empty());
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn capture_stopping_during_a_reply_lets_it_play_and_does_not_rearm() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantSttEnd, &[("text", "x")]);
        ev(&mut v, t0, Ev::VoiceAssistantTtsEnd, &[("url", "u")]);
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        v.on_capture(t0, false);
        // Nothing to stop: the audio ended with the speech, the reply plays.
        assert!(v.take().is_empty());
        v.on_playback_finished(t0, true);
        assert!(
            !v.take()
                .iter()
                .any(|o| matches!(o, Out::Ha(ToHa::Start { .. })))
        );
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn capture_stopping_during_stt_aborts_and_stops_streaming() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantWakeWordEnd, &[]);
        v.take();
        v.on_capture(t0, false);
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Abort), Out::App(AppEvent::StopStreaming)]
        );
    }

    #[test]
    fn mute_and_unmute_without_audio_open_nothing() {
        let t0 = Instant::now();
        let mut v = armed_without_capture(t0);
        v.on_mute(t0, true);
        v.take();
        v.on_mute(t0, false);
        assert!(
            !v.take()
                .iter()
                .any(|o| matches!(o, Out::Ha(ToHa::Start { .. })))
        );
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn push_to_talk_needs_audio() {
        let t0 = Instant::now();
        let mut v = armed_without_capture(t0);
        v.on_start_pipeline(t0, PipelineMode::PushToTalk);
        assert!(v.take().is_empty());
    }

    #[test]
    fn an_announcement_without_audio_plays_and_nothing_opens_after_it() {
        let t0 = Instant::now();
        let mut v = armed_without_capture(t0);
        assert!(v.take().is_empty());
        v.on_announce("u".into(), String::new(), "hi".into(), false);
        let out = v.take();
        // No session to abort; the announcement is handed to the app.
        assert!(!out.contains(&Out::Ha(ToHa::Abort)), "{out:?}");
        assert!(
            out.iter()
                .any(|o| matches!(o, Out::App(AppEvent::Announce { .. })))
        );
        assert_eq!(v.media_state(), MediaState::Announcing);
        v.on_announce_finished(t0, true);
        assert_eq!(
            v.take(),
            [
                Out::Ha(ToHa::AnnounceFinished(true)),
                Out::Ha(ToHa::MediaState(MediaState::Idle)),
            ]
        );
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn an_announcement_while_capturing_aborts_the_run_and_rearms() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_announce("u".into(), String::new(), String::new(), false);
        let out = v.take();
        assert!(out.contains(&Out::Ha(ToHa::Abort)));
        assert!(out.contains(&Out::App(AppEvent::StopStreaming)));
        // HA ends the aborted run (it also cancelled it before announcing).
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.on_announce_finished(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn an_announcement_during_the_audio_loss_does_not_reopen_a_session() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_announce("u".into(), String::new(), String::new(), false);
        v.take();
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        // The mic goes away while the announcement plays.
        v.on_capture(t0, false);
        assert!(v.take().is_empty());
        v.on_announce_finished(t0, true);
        assert!(
            !v.take()
                .iter()
                .any(|o| matches!(o, Out::Ha(ToHa::Start { .. })))
        );
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn start_conversation_without_audio_does_not_open_a_session() {
        let t0 = Instant::now();
        let mut v = armed_without_capture(t0);
        v.on_announce("u".into(), String::new(), "q".into(), true);
        v.take();
        v.on_announce_finished(t0, true);
        assert!(
            !v.take()
                .iter()
                .any(|o| matches!(o, Out::Ha(ToHa::Start { .. })))
        );
        // The wake word comes back with the audio.
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn repeated_capture_reports_change_nothing() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_capture(t0, true);
        assert!(v.take().is_empty());
        assert!(v.is_streaming());
        v.on_capture(t0, false);
        v.take();
        v.on_capture(t0, false);
        assert!(v.take().is_empty());
    }

    #[test]
    fn the_capture_state_survives_a_reconnect() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_capture(t0, false);
        v.take();
        v.on_disconnect();
        v.take();
        v.on_subscribe(t0, true);
        assert!(v.take().is_empty(), "no session for a mic without audio");
        v.on_capture(t0, true);
        assert!(v.take().iter().any(|o| is_start(o, true)));
    }

    #[test]
    fn stop_pipeline_disarms() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_stop_pipeline();
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Abort), Out::App(AppEvent::StopStreaming)]
        );
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        assert!(v.take().is_empty());
        assert_eq!(v.next_deadline(), None);
    }

    #[test]
    fn disconnect_keeps_arming_for_next_connection() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_disconnect();
        assert_eq!(v.take(), [Out::App(AppEvent::StopStreaming)]);
        v.on_subscribe(t0, true);
        assert!(matches!(
            v.take()[..],
            [Out::Ha(ToHa::Start {
                wake_word: true,
                ..
            })]
        ));
    }

    #[test]
    fn events_before_response_imply_start() {
        let t0 = Instant::now();
        let mut v = Voice::new(false);
        v.on_subscribe(t0, true);
        v.on_start_pipeline(t0, PipelineMode::WakeWord);
        v.take();
        ev(&mut v, t0, Ev::VoiceAssistantRunStart, &[]);
        assert!(v.is_streaming());
        v.on_response(t0, 0, false); // late response is ignored
        assert_eq!(
            v.take(),
            [Out::App(AppEvent::StartStreaming {
                wake_word: true,
                conversation_id: None
            })]
        );
    }

    fn speaker_streaming(t0: Instant) -> Voice {
        let mut v = Voice::new(true);
        v.on_start_pipeline(t0, PipelineMode::WakeWord);
        v.on_subscribe(t0, true);
        v.on_response(t0, 0, false);
        ev(&mut v, t0, Ev::VoiceAssistantWakeWordEnd, &[]);
        ev(&mut v, t0, Ev::VoiceAssistantSttEnd, &[("text", "hola")]);
        v.take();
        v
    }

    #[test]
    fn speaker_mode_streams_tts_and_rearms_after_playback() {
        let t0 = Instant::now();
        let mut v = speaker_streaming(t0);
        // HA sends the URL too, but the device must not fetch it.
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantTtsEnd,
            &[("url", "http://10.0.0.1:8123/api/tts_proxy/x.wav")],
        );
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]); // RUN_END can precede the stream
        assert_eq!(v.take(), [Out::App(AppEvent::RunEnd)]);
        ev(&mut v, t0, Ev::VoiceAssistantTtsStreamStart, &[]);
        v.on_tts_audio(vec![1, 2, 3, 4]);
        v.on_tts_audio(vec![]);
        ev(&mut v, t0, Ev::VoiceAssistantTtsStreamEnd, &[]);
        assert_eq!(
            v.take(),
            [
                Out::Ha(ToHa::MediaState(MediaState::Announcing)),
                Out::App(AppEvent::TtsStreamStart {
                    sample_rate: 16_000,
                    channels: 1,
                    sample_width: 2
                }),
                Out::App(AppEvent::TtsAudio(vec![1, 2, 3, 4])),
                Out::App(AppEvent::TtsStreamEnd),
            ]
        );
        // Waits for the app to finish playing; no AnnounceFinished in SPEAKER mode.
        v.on_playback_finished(t0, true);
        assert_eq!(
            v.take(),
            [
                Out::Ha(ToHa::MediaState(MediaState::Idle)),
                Out::Ha(ToHa::Start {
                    wake_word: true,
                    conversation_id: String::new()
                }),
            ]
        );
        // Audio outside a stream is dropped.
        v.on_tts_audio(vec![9]);
        assert!(v.take().is_empty());
    }

    #[test]
    fn push_to_talk_waits_for_the_aborted_runs_end() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantRunStart, &[]);
        ev(&mut v, t0, Ev::VoiceAssistantWakeWordStart, &[]);
        v.take();
        v.on_start_pipeline(t0, PipelineMode::PushToTalk);
        // Abort now, start only after HA ended the aborted run.
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Abort), Out::App(AppEvent::StopStreaming)]
        );
        assert_eq!(v.next_deadline(), Some(t0 + ABORT_DRAIN_TIMEOUT));
        // Trailing events of the aborted run, as HA 2026.10.4 sends them.
        ev(
            &mut v,
            t0,
            Ev::VoiceAssistantError,
            &[("code", "no_wake_word")],
        );
        assert!(v.take().is_empty(), "old run's error must be ignored");
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Start {
                wake_word: false,
                conversation_id: String::new()
            })]
        );
        // The new run proceeds normally.
        v.on_response(t0, 0, false);
        ev(&mut v, t0, Ev::VoiceAssistantRunStart, &[]);
        ev(&mut v, t0, Ev::VoiceAssistantSttStart, &[]);
        assert_eq!(
            v.take(),
            [Out::App(AppEvent::StartStreaming {
                wake_word: false,
                conversation_id: None
            })]
        );
        assert!(v.is_streaming());
    }

    #[test]
    fn aborted_run_without_run_end_starts_after_the_drain_timeout() {
        let t0 = Instant::now();
        let mut v = armed_and_streaming(t0);
        v.on_start_pipeline(t0, PipelineMode::PushToTalk);
        v.take();
        v.on_tick(t0 + ABORT_DRAIN_TIMEOUT - Duration::from_millis(1));
        assert!(v.take().is_empty());
        v.on_tick(t0 + ABORT_DRAIN_TIMEOUT);
        assert_eq!(
            v.take(),
            [Out::Ha(ToHa::Start {
                wake_word: false,
                conversation_id: String::new()
            })]
        );
    }

    #[test]
    fn speaker_mode_disconnect_keeps_a_received_reply_but_stops_a_partial_one() {
        let t0 = Instant::now();
        // Stream complete, then HA disconnects: no StopPlayback.
        let mut v = speaker_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantTtsStreamStart, &[]);
        ev(&mut v, t0, Ev::VoiceAssistantTtsStreamEnd, &[]);
        v.take();
        v.on_disconnect();
        assert!(!v.take().contains(&Out::App(AppEvent::StopPlayback)));
        // Stream still arriving: StopPlayback.
        let mut v = speaker_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantTtsStreamStart, &[]);
        v.take();
        v.on_disconnect();
        assert!(v.take().contains(&Out::App(AppEvent::StopPlayback)));
    }

    #[test]
    fn speaker_mode_missing_stream_times_out() {
        let t0 = Instant::now();
        let mut v = speaker_streaming(t0);
        ev(&mut v, t0, Ev::VoiceAssistantTtsEnd, &[("url", "u")]);
        ev(&mut v, t0, Ev::VoiceAssistantRunEnd, &[]);
        v.take();
        assert_eq!(v.next_deadline(), Some(t0 + TTS_STREAM_START_TIMEOUT));
        v.on_tick(t0 + TTS_STREAM_START_TIMEOUT);
        let out = v.take();
        assert!(matches!(
            out.last(),
            Some(Out::Ha(ToHa::Start {
                wake_word: true,
                ..
            }))
        ));
    }
}
