//! The on-screen assistant overlay: what a voice run shows over the app in
//! front (docs/design/assistant-overlay.md, ADR 0014).
//!
//! Pure state machine, no I/O. The daemon feeds it the voice events it
//! already receives ([`AppEvent`]), the end of the reply's playback and
//! time; it returns [`Frame`]s, each the whole content of the overlay, that
//! the daemon hands to the overlay app (a `topui` window of our own package,
//! started through `com.webos.applicationManager/launch`, the frame in its
//! launch parameters). The app draws the frame and hides itself a few
//! seconds after [`Stage::Done`] or [`Stage::Error`]; [`Stage::Hide`] hides
//! it at once.
//!
//! Which runs are shown: a run that starts without the wake word (the
//! remote's mic button, a `continue_conversation` follow-up, the
//! conversation HA starts after an announcement) from its first audio, and
//! a wake word run from `WAKE_WORD_END` on. The idle wake word stage, which
//! streams all the time, is never shown, and neither are its errors.
//!
//! The speech-to-text result and the reply text exist here only to be shown:
//! nothing in this module logs them (the frame's `Debug` prints lengths).

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;
use webos_esphome_api::AppEvent;

use crate::compat::json_str;
use crate::power;

/// Longest text of a frame, in characters (the overlay shows about three
/// lines; a longer reply is cut and ends with an ellipsis).
pub(crate) const MAX_TEXT: usize = 280;
/// A shown run with no new event for this long is closed with
/// [`Stage::Done`] (a lost `RUN_END`, a reply that never ends). The app has
/// its own, longer limit for a daemon that stops sending.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(90);
/// The first frame of a run may start the app cold (about 2 s until the
/// page runs on c3-a, EXP-024): a relaunch sent meanwhile could reach a page
/// that does not listen yet. The latest frame is sent once more this long
/// after the first frame of a run.
pub(crate) const COLD_SETTLE: Duration = Duration::from_millis(2500);
/// SAM's launch method (documented for webOS TV; launch and relaunch with
/// `params` verified on c3-a, EXP-024).
const LAUNCH_URI: &str = "luna://com.webos.applicationManager/launch";

/// What the overlay shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The run's audio goes to HA (shown only while it does).
    Listening,
    /// The audio ended; speech-to-text and the conversation agent work.
    Thinking,
    /// The reply plays (or, without a spoken reply, its text is shown).
    Answer,
    /// The run failed; the app shows the error's text.
    Error(ErrorKind),
    /// The run and its reply are over: the app keeps the last content for a
    /// few seconds, then hides.
    Done,
    /// Hide now (the microphone was muted, the overlay was switched off).
    Hide,
}

impl Stage {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Answer => "answer",
            Self::Error(_) => "error",
            Self::Done => "done",
            Self::Hide => "hide",
        }
    }
}

/// Why a shown run failed (the app has a text for each).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// Speech-to-text found no words (`stt-no-text-recognized`).
    NoSpeech,
    /// The Home Assistant connection ended during the run.
    Disconnected,
    /// Any other pipeline error.
    Failed,
}

impl ErrorKind {
    fn word(self) -> &'static str {
        match self {
            Self::NoSpeech => "no_speech",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        }
    }

    fn of_code(code: &str) -> Self {
        if code == "stt-no-text-recognized" {
            Self::NoSpeech
        } else {
            Self::Failed
        }
    }
}

/// One overlay content: the stage, the run's sequence number and the two
/// texts. `seq` grows by one per shown run, so the app can tell a new run
/// from an update of the one it shows.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) seq: u32,
    pub(crate) stage: Stage,
    /// What speech-to-text heard (empty until `STT_END`).
    pub(crate) heard: String,
    /// The reply text (empty until the reply starts).
    pub(crate) answer: String,
}

impl fmt::Debug for Frame {
    // Never the texts: a `{frame:?}` in a log line must not leak speech.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("seq", &self.seq)
            .field("stage", &self.stage)
            .field("heard_chars", &self.heard.chars().count())
            .field("answer_chars", &self.answer.chars().count())
            .finish()
    }
}

impl Frame {
    /// The launch parameters of the overlay app, one JSON object
    /// (`"overlay":1` is the schema version).
    pub(crate) fn params_json(&self) -> String {
        let error = match self.stage {
            Stage::Error(k) => json_str(k.word()),
            _ => "null".to_owned(),
        };
        format!(
            "{{\"overlay\":1,\"seq\":{},\"stage\":{},\"heard\":{},\"answer\":{},\"error\":{}}}",
            self.seq,
            json_str(self.stage.word()),
            json_str(&self.heard),
            json_str(&self.answer),
            error
        )
    }
}

/// A text as the overlay shows it: control characters become spaces, runs
/// of white space one space, and more than [`MAX_TEXT`] characters are cut
/// at a word boundary when there is one near the end, with an ellipsis.
pub(crate) fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT * 4));
    let mut space = false;
    let mut n = 0_usize;
    let mut cut = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
            continue;
        }
        if n >= MAX_TEXT {
            cut = true;
            break;
        }
        if space {
            out.push(' ');
            n += 1;
            space = false;
        }
        out.push(c);
        n += 1;
    }
    if cut {
        // Back to the last space if it is in the last fifth of the text.
        let keep = MAX_TEXT - 1;
        let mut s: String = out.chars().take(keep).collect();
        if let Some(i) = s.rfind(' ') {
            if s[..i].chars().count() >= keep * 4 / 5 {
                s.truncate(i);
            }
        }
        out = s;
        out.push('…');
    }
    out
}

/// The overlay's view of the voice runs.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Overlay {
    /// `[overlay] enabled`.
    enabled: bool,
    /// A run is on screen (from its first shown event to `Done`/`Hide`).
    shown: bool,
    seq: u32,
    stage: Stage,
    heard: String,
    answer: String,
    /// `RUN_END` of the shown run was seen.
    ended: bool,
    /// The reply plays.
    playing: bool,
    /// The last time the shown run changed.
    since: Instant,
    /// The frame to publish, when the content changed.
    pending: Option<Frame>,
}

impl Overlay {
    pub(crate) fn new(enabled: bool, now: Instant) -> Self {
        Self {
            enabled,
            shown: false,
            seq: 0,
            stage: Stage::Done,
            heard: String::new(),
            answer: String::new(),
            ended: false,
            playing: false,
            since: now,
            pending: None,
        }
    }

    /// Whether a run is on screen now.
    #[cfg(test)]
    pub(crate) fn shown(&self) -> bool {
        self.shown
    }

    /// The frame to publish, if the content changed since the last call.
    pub(crate) fn take(&mut self) -> Option<Frame> {
        self.pending.take()
    }

    fn set(&mut self, stage: Stage, now: Instant) {
        if !self.enabled {
            return;
        }
        self.stage = stage;
        self.since = now;
        if matches!(stage, Stage::Done | Stage::Hide) {
            self.shown = false;
        }
        self.pending = Some(Frame {
            seq: self.seq,
            stage,
            heard: self.heard.clone(),
            answer: self.answer.clone(),
        });
    }

    /// A new shown run starts listening.
    fn begin(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        self.shown = true;
        self.heard.clear();
        self.answer.clear();
        self.ended = false;
        self.playing = false;
        self.set(Stage::Listening, now);
    }

    /// Closes the shown run once it ended and its reply finished.
    fn maybe_done(&mut self, now: Instant) {
        if !self.shown || !self.ended || self.playing {
            return;
        }
        match self.stage {
            // The error stays on screen until the app hides it.
            Stage::Error(_) => self.shown = false,
            // `Done` carries the texts: a reply text without a spoken reply
            // (no TTS in the pipeline) is shown with it.
            _ => self.set(Stage::Done, now),
        }
    }

    /// One voice event (the daemon passes every [`AppEvent`]).
    pub(crate) fn on_event(&mut self, ev: &AppEvent, now: Instant) {
        if !self.enabled {
            return;
        }
        match ev {
            AppEvent::StartStreaming {
                wake_word: false, ..
            }
            | AppEvent::WakeWordDetected => self.begin(now),
            _ if !self.shown => {}
            AppEvent::StopStreaming if self.stage == Stage::Listening => {
                self.set(Stage::Thinking, now);
            }
            AppEvent::Transcript(text) => {
                self.heard = clean(text);
                if matches!(self.stage, Stage::Listening | Stage::Thinking) {
                    self.set(Stage::Thinking, now);
                }
            }
            AppEvent::IntentEnd { speech, .. } => {
                // Shown when the reply starts to play (or at the run's end
                // without a spoken reply).
                self.answer = clean(speech);
                self.since = now;
            }
            AppEvent::TtsStreamStart { .. } | AppEvent::TtsUrl(_) => {
                self.playing = true;
                if self.answer.is_empty() {
                    self.since = now;
                } else {
                    self.set(Stage::Answer, now);
                }
            }
            AppEvent::Error { code, .. } => {
                if self.answer.is_empty() {
                    self.set(Stage::Error(ErrorKind::of_code(code)), now);
                }
                // With a reply text (a TTS error after the intent) the text
                // is still worth showing: the run ends as without a reply.
            }
            AppEvent::RunEnd => {
                self.ended = true;
                self.maybe_done(now);
            }
            AppEvent::HaDisconnected => {
                self.playing = false;
                self.ended = true;
                if matches!(self.stage, Stage::Listening | Stage::Thinking) {
                    self.set(Stage::Error(ErrorKind::Disconnected), now);
                }
                self.maybe_done(now);
            }
            AppEvent::MuteChanged(true) => self.set(Stage::Hide, now),
            _ => {}
        }
    }

    /// The reply stopped playing (finished, failed or stopped).
    pub(crate) fn on_playback_end(&mut self, now: Instant) {
        if self.shown && self.playing {
            self.playing = false;
            self.maybe_done(now);
        }
    }

    /// Closes a shown run that has not changed for [`STALE_AFTER`].
    pub(crate) fn on_tick(&mut self, now: Instant) {
        if self.shown && now.saturating_duration_since(self.since) >= STALE_AFTER {
            log::debug!("overlay: no news of the shown run; closing it");
            self.playing = false;
            self.ended = true;
            if matches!(self.stage, Stage::Error(_)) {
                self.shown = false;
            } else {
                self.set(Stage::Done, now);
            }
        }
    }
}

/// The `luna-send` payload that shows `frame` in app `app_id` (an id
/// checked by the config: no character that needs escaping).
pub(crate) fn launch_payload(app_id: &str, frame: &Frame) -> String {
    format!(
        "{{\"id\":{},\"params\":{}}}",
        json_str(app_id),
        frame.params_json()
    )
}

/// Hands frames to the overlay app: one `luna-send` at a time, always the
/// latest frame (frames that were overtaken while a call ran are skipped:
/// the app shows a state, not a history). Dropping it ends the task.
#[derive(Debug)]
pub(crate) struct Sender {
    tx: watch::Sender<Option<Frame>>,
}

impl Sender {
    /// Starts the task (needs the tokio runtime).
    pub(crate) fn spawn(luna: PathBuf, app_id: String) -> Self {
        let (tx, rx) = watch::channel(None);
        tokio::spawn(send_loop(luna, app_id, rx));
        Self { tx }
    }

    pub(crate) fn send(&self, frame: Frame) {
        // The receiver lives as long as the task, which ends only with us.
        let _ = self.tx.send(Some(frame));
    }
}

async fn send_loop(luna: PathBuf, app_id: String, mut rx: watch::Receiver<Option<Frame>>) {
    let mut last_seq: Option<u32> = None;
    let mut resend_at: Option<Instant> = None;
    let mut failures = 0_u32;
    loop {
        let resend = tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    return;
                }
                false
            }
            () = crate::daemon::sleep_opt(resend_at) => true,
        };
        let Some(frame) = rx.borrow_and_update().clone() else {
            continue;
        };
        if resend {
            resend_at = None;
            // A hide sent again could start the app only to close it.
            if frame.stage == Stage::Hide {
                continue;
            }
        } else if last_seq != Some(frame.seq) {
            last_seq = Some(frame.seq);
            resend_at = Some(Instant::now() + COLD_SETTLE);
        }
        let started = Instant::now();
        let r = power::call(&luna, LAUNCH_URI, &launch_payload(&app_id, &frame)).await;
        let ms = started.elapsed().as_millis();
        match r {
            Ok(text)
                if text.contains("\"returnValue\":true")
                    || text.contains("\"returnValue\": true") =>
            {
                failures = 0;
                log::debug!(
                    "overlay: {} of run {} shown ({ms} ms{})",
                    frame.stage.word(),
                    frame.seq,
                    if resend { ", again" } else { "" }
                );
            }
            other => {
                failures = failures.saturating_add(1);
                // The reply of SAM never holds the parameters; cut anyway.
                let why = match other {
                    Ok(t) => t.chars().take(160).collect::<String>(),
                    Err(e) => e,
                };
                if failures <= 3 {
                    log::warn!("overlay: the overlay app did not start ({ms} ms): {why}");
                } else {
                    log::debug!("overlay: the overlay app did not start ({ms} ms): {why}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn start(wake_word: bool) -> AppEvent {
        AppEvent::StartStreaming {
            wake_word,
            conversation_id: None,
            push_to_talk: !wake_word,
        }
    }

    fn intent(speech: &str) -> AppEvent {
        AppEvent::IntentEnd {
            speech: speech.into(),
            continue_conversation: false,
        }
    }

    fn tts() -> AppEvent {
        AppEvent::TtsStreamStart {
            sample_rate: 16_000,
            channels: 1,
            sample_width: 2,
        }
    }

    fn err(code: &str) -> AppEvent {
        AppEvent::Error {
            code: code.into(),
            message: "m".into(),
        }
    }

    /// Feeds events and returns the stages published after each.
    fn drive(o: &mut Overlay, now: Instant, evs: &[AppEvent]) -> Vec<Option<Stage>> {
        evs.iter()
            .map(|ev| {
                o.on_event(ev, now);
                o.take().map(|f| f.stage)
            })
            .collect()
    }

    #[test]
    fn a_wake_word_run_is_shown_from_the_wake_word_to_the_end_of_the_reply() {
        let t = Instant::now();
        let mut o = Overlay::new(true, t);
        // The idle wake word stage is not shown, nor its error and end.
        let quiet = drive(
            &mut o,
            t,
            &[start(true), err("wake-word-timeout"), AppEvent::RunEnd],
        );
        assert_eq!(quiet, vec![None, None, None]);
        assert!(!o.shown());
        let got = drive(
            &mut o,
            t,
            &[
                start(true),
                AppEvent::WakeWordDetected,
                AppEvent::StopStreaming,
                AppEvent::Transcript("  enciende\n la luz ".into()),
                intent("Vale, encendida."),
                tts(),
                AppEvent::TtsStreamEnd,
                AppEvent::RunEnd,
            ],
        );
        assert_eq!(
            got,
            vec![
                None,
                Some(Stage::Listening),
                Some(Stage::Thinking),
                Some(Stage::Thinking),
                None,
                Some(Stage::Answer),
                None,
                // The reply still plays: not done yet.
                None,
            ]
        );
        assert!(o.shown());
        o.on_playback_end(t);
        let f = o.take().unwrap();
        assert_eq!(f.stage, Stage::Done);
        assert_eq!(f.seq, 1);
        assert_eq!(f.heard, "enciende la luz");
        assert_eq!(f.answer, "Vale, encendida.");
        assert!(!o.shown());
    }

    #[test]
    fn a_run_without_the_wake_word_is_shown_from_its_start() {
        let t = Instant::now();
        let mut o = Overlay::new(true, t);
        assert_eq!(
            drive(&mut o, t, &[start(false)]),
            vec![Some(Stage::Listening)]
        );
        // A follow-up turn (continue_conversation) is a new shown run.
        drive(&mut o, t, &[AppEvent::Transcript("hola".into())]);
        drive(&mut o, t, &[intent("¿Y qué más?"), tts(), AppEvent::RunEnd]);
        o.on_playback_end(t);
        assert_eq!(o.take().unwrap().stage, Stage::Done);
        o.on_event(&start(false), t);
        let f = o.take().unwrap();
        assert_eq!((f.seq, f.stage), (2, Stage::Listening));
        assert!(f.heard.is_empty() && f.answer.is_empty());
    }

    #[test]
    fn errors_of_a_shown_run_are_shown_and_stay_until_the_app_hides_them() {
        let t = Instant::now();
        let mut o = Overlay::new(true, t);
        let got = drive(
            &mut o,
            t,
            &[
                AppEvent::WakeWordDetected,
                AppEvent::StopStreaming,
                err("stt-no-text-recognized"),
                AppEvent::RunEnd,
            ],
        );
        assert_eq!(
            got,
            vec![
                Some(Stage::Listening),
                Some(Stage::Thinking),
                Some(Stage::Error(ErrorKind::NoSpeech)),
                None
            ]
        );
        assert!(!o.shown());
        let got = drive(&mut o, t, &[start(false), err("intent-failed")]);
        assert_eq!(got[1], Some(Stage::Error(ErrorKind::Failed)));
    }

    #[test]
    fn a_reply_text_without_speech_is_shown_at_the_end_of_the_run() {
        let t = Instant::now();
        let mut o = Overlay::new(true, t);
        drive(
            &mut o,
            t,
            &[
                start(false),
                AppEvent::Transcript("apaga".into()),
                intent("Apagada."),
            ],
        );
        o.take();
        // A TTS error after the intent: the text is still shown, then done.
        o.on_event(&err("tts-failed"), t);
        assert_eq!(o.take(), None);
        o.on_event(&AppEvent::RunEnd, t);
        let f = o.take().unwrap();
        assert_eq!((f.stage, f.answer.as_str()), (Stage::Done, "Apagada."));
    }

    #[test]
    fn a_disconnect_mute_and_silence_end_the_shown_run() {
        let t = Instant::now();
        let mut o = Overlay::new(true, t);
        drive(&mut o, t, &[start(false)]);
        o.take();
        o.on_event(&AppEvent::HaDisconnected, t);
        assert_eq!(
            o.take().unwrap().stage,
            Stage::Error(ErrorKind::Disconnected)
        );
        assert!(!o.shown());

        // A disconnect while the reply plays: it plays on, then done.
        drive(&mut o, t, &[start(false), intent("Hola"), tts()]);
        o.take();
        o.on_event(&AppEvent::HaDisconnected, t);
        assert_eq!(o.take().unwrap().stage, Stage::Done);

        drive(&mut o, t, &[start(false)]);
        o.take();
        o.on_event(&AppEvent::MuteChanged(true), t);
        assert_eq!(o.take().unwrap().stage, Stage::Hide);
        assert!(!o.shown());

        drive(&mut o, t, &[start(false)]);
        o.take();
        o.on_tick(t + STALE_AFTER - Duration::from_millis(1));
        assert_eq!(o.take(), None);
        o.on_tick(t + STALE_AFTER);
        assert_eq!(o.take().unwrap().stage, Stage::Done);
        assert!(!o.shown());
    }

    #[test]
    fn disabled_publishes_nothing() {
        let t = Instant::now();
        let mut o = Overlay::new(false, t);
        drive(
            &mut o,
            t,
            &[
                AppEvent::WakeWordDetected,
                intent("x"),
                tts(),
                AppEvent::RunEnd,
            ],
        );
        o.on_playback_end(t);
        o.on_tick(t + STALE_AFTER);
        assert_eq!(o.take(), None);
        assert!(!o.shown());
    }

    #[test]
    fn params_are_one_json_object_with_escaped_texts() {
        let f = Frame {
            seq: 7,
            stage: Stage::Error(ErrorKind::NoSpeech),
            heard: "di \"hola\" \\ ya".into(),
            answer: String::new(),
        };
        assert_eq!(
            f.params_json(),
            r#"{"overlay":1,"seq":7,"stage":"error","heard":"di \"hola\" \\ ya","answer":"","error":"no_speech"}"#
        );
        let g = Frame {
            seq: 1,
            stage: Stage::Answer,
            heard: String::new(),
            answer: "ok".into(),
        };
        assert!(g.params_json().ends_with(r#""answer":"ok","error":null}"#));
        // Debug never shows the texts.
        let d = format!("{f:?}");
        assert!(!d.contains("hola") && d.contains("heard_chars: 14"));
    }

    #[test]
    fn clean_folds_white_space_and_cuts_long_texts() {
        assert_eq!(clean("\n a\t\tb \u{7}c  "), "a b c");
        assert_eq!(clean(""), "");
        let long = "palabra ".repeat(100);
        let c = clean(&long);
        assert!(c.chars().count() <= MAX_TEXT);
        assert!(c.ends_with("palabra…"));
        let one = "x".repeat(MAX_TEXT + 10);
        let c = clean(&one);
        assert_eq!(c.chars().count(), MAX_TEXT);
        assert!(c.ends_with('…'));
        // Exactly MAX_TEXT characters is not cut.
        assert_eq!(clean(&"y".repeat(MAX_TEXT)).chars().count(), MAX_TEXT);
        // Multi-byte characters are counted, not bytes.
        assert_eq!(clean(&"ñ".repeat(MAX_TEXT + 1)).chars().count(), MAX_TEXT);
    }
}
