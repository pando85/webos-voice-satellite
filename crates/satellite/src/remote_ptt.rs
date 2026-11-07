//! Hold-to-talk with the Magic Remote's mic button (`[remote] mic_button =
//! "ptt"`, off by default; design: docs/design/remote-mic-button.md; the
//! coexistence with LG's assistant: ADR 0013, proposed, see [`LgVoiceKey`]).
//!
//! Three parts:
//!
//! - [`crate::evdev`]: a passive reader of the button's input device, found
//!   by name (never by index, never grabbed); it reports press and release.
//! - [`crate::remote_mic`]: the remote's own microphone through LG's
//!   `voiceinput` service (`startStreaming` with `deviceType remote`, a Unix
//!   socket of 16 kHz mono S16LE, EXP-022 B), opened per run and closed by us
//!   (the stream does not end at the release on webOS 8.3).
//! - [`Hold`] (this module): the pure state machine that turns presses,
//!   releases, run events, stream events and time into actions for the
//!   daemon. No I/O, so every rule is a unit test.
//!
//! Rules (the design note gives the reasons):
//!
//! - **Press:** refused (and logged) when Home Assistant is not connected,
//!   the microphone is muted or gated, an announcement plays, or the TV
//!   microphone delivers no audio (the voice layer needs a live capture). A
//!   run without the wake word that is already streaming (a continued
//!   conversation, a push-to-talk run, speech after the wake word) is
//!   **joined**: no new run, the audio switches to the remote. Otherwise a
//!   push-to-talk run starts (an idle wake word session is aborted by the
//!   voice layer; a reply that plays is interrupted, as with `SIGUSR1`).
//! - **Release** after at least `tap_ms`: the audio ends (`EndAudio`, HA
//!   finalizes speech-to-text) and the remote stream closes.
//! - **Tap** (release before `tap_ms`): tap-to-talk. The run goes on with
//!   the **TV** microphone and Home Assistant's end-of-speech detection
//!   ends it; the remote stream closes at once, so the remote's microphone
//!   is live only while the button is held.
//! - **Cap:** `max_hold_s` after the press the audio ends and the stream
//!   closes, held or not.
//! - **Audio source:** the remote stream from its first chunk on (its chunks
//!   go to HA through the daemon and the TV microphone's chunks are held
//!   back), the TV microphone until then and whenever the remote has
//!   nothing. Audio of the remote that arrives before the run streams is
//!   kept (at most [`PREROLL_MAX`] samples) and sent first, so the first
//!   words are not lost while HA starts the run. A stream that fails, ends,
//!   sends nothing within [`FIRST_DATA_WAIT`] or delivers the wrong rate
//!   falls back to the TV microphone, also in the middle of a run.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::config::RemoteSection;
use crate::evdev::{self, InputPaths, Key, Timing};
use crate::remote_mic::{MicConfig, RemoteMic, StreamEvent};

/// A press this soon after the last release is a bounce and ignored (with
/// its release).
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(50);
/// Remote audio kept while the run has not started streaming yet (2 s).
pub(crate) const PREROLL_MAX: usize = 32_000;
/// The remote stream must deliver its first chunk this soon after it opened.
pub(crate) const FIRST_DATA_WAIT: Duration = Duration::from_millis(1000);
/// The stream's rate is checked over this long after its first chunk.
pub(crate) const RATE_WINDOW: Duration = Duration::from_millis(1000);
/// Expected samples per second of the remote stream (16 kHz mono S16LE,
/// 31.6 kB/s measured on c3-a, EXP-022 B).
pub(crate) const EXPECTED_RATE: f64 = 16_000.0;
/// Accepted measured rate, as a share of [`EXPECTED_RATE`]: 8 kHz (0.5) and
/// 32 kHz or stereo (2.0) fall outside.
const RATE_MIN: f64 = 0.6;
const RATE_MAX: f64 = 1.5;

/// The settings the machine uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HoldConfig {
    /// Use the remote's microphone (false: the TV microphone only).
    pub(crate) remote_mic: bool,
    /// Longest hold.
    pub(crate) max_hold: Duration,
    /// A release before this is a tap (zero: no taps).
    pub(crate) tap: Duration,
}

impl HoldConfig {
    pub(crate) fn from_section(s: &RemoteSection) -> Self {
        Self {
            remote_mic: s.remote_mic,
            max_hold: Duration::from_secs(u64::from(s.max_hold_s)),
            tap: Duration::from_millis(u64::from(s.tap_ms)),
        }
    }
}

/// What the daemon knows at a press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // one bool per independent condition
pub(crate) struct Context {
    /// Home Assistant has an API session.
    pub(crate) connected: bool,
    /// The microphone is muted or closed by a gate (mute switch, kill
    /// switch, firmware guard, not paired). The remote's microphone is a
    /// microphone too: the same gates apply.
    pub(crate) mic_off: bool,
    /// The TV microphone is open and delivering (the voice layer refuses a
    /// run without it).
    pub(crate) mic_open: bool,
    /// An announcement plays (the voice layer refuses a run then).
    pub(crate) announcing: bool,
    /// A run past the wake word streams audio now (a continued
    /// conversation, a push-to-talk run, or speech after the wake word).
    pub(crate) run_streaming: bool,
}

/// What a press led to (for the daemon's log line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PressOutcome {
    /// A push-to-talk run was requested.
    Started,
    /// The run that streams now takes the remote's audio.
    Joined,
    /// Not now; the reason is for the log.
    Refused(&'static str),
    /// A second press without a release, or a bounce.
    Ignored,
}

/// An action for the daemon, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    /// `AppCommand::StartPipeline { mode: PushToTalk }`.
    StartRun,
    /// `AppCommand::EndAudio`.
    EndAudio,
    /// Open the remote's stream (a new session).
    OpenStream,
    /// Close the remote's stream (end the subscription, close the socket).
    CloseStream,
    /// Send this chunk of remote audio to HA (`AppCommand::Audio`).
    Audio(Vec<i16>),
    /// The remote feeds the run: hold the TV microphone's chunks back
    /// (`true`) or let them flow again (`false`).
    RemoteFeeds(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// The button is held. `started`: the run streams (its `StartStreaming`
    /// arrived, or it was joined); before that a `StopStreaming` belongs to
    /// the run the press replaced.
    Held {
        since: Instant,
        started: bool,
    },
    /// Released after a hold before the run started: the audio kept so far
    /// is sent and ended as soon as the run streams.
    EndPending {
        since: Instant,
    },
    /// Released as a tap: the run goes on with the TV microphone until HA
    /// ends its audio (or the cap).
    Tapped {
        since: Instant,
        started: bool,
    },
    /// Waiting for the release of a press that was refused or ignored, or
    /// whose run is already over (the release then does nothing).
    WaitRelease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Closed,
    /// Asked for; no answer yet.
    Opening,
    /// The socket is connected. `first`: arrival of the first chunk;
    /// `samples`: received after the first chunk; `checked`: the rate check
    /// passed.
    Open {
        opened: Instant,
        first: Option<Instant>,
        samples: u64,
        checked: bool,
    },
}

/// The hold-to-talk state machine (pure).
#[derive(Debug)]
pub(crate) struct Hold {
    cfg: HoldConfig,
    phase: Phase,
    stream: Stream,
    /// The button is down (as far as the reader reported).
    key_down: bool,
    last_release: Option<Instant>,
    /// Remote audio received before the run streamed.
    preroll: VecDeque<Vec<i16>>,
    preroll_samples: usize,
    /// `Act::RemoteFeeds(true)` is in force.
    feeding: bool,
    out: Vec<Act>,
}

impl Hold {
    pub(crate) fn new(cfg: HoldConfig) -> Self {
        Self {
            cfg,
            phase: Phase::Idle,
            stream: Stream::Closed,
            key_down: false,
            last_release: None,
            preroll: VecDeque::new(),
            preroll_samples: 0,
            feeding: false,
            out: Vec::new(),
        }
    }

    fn take(&mut self) -> Vec<Act> {
        std::mem::take(&mut self.out)
    }

    /// The remote's audio feeds the run now.
    #[cfg(test)]
    pub(crate) fn feeding(&self) -> bool {
        self.feeding
    }

    /// A press is being served (held, released waiting for its run, or a
    /// tapped run).
    pub(crate) fn active(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::WaitRelease)
    }

    /// The remote stream is open or being opened.
    pub(crate) fn stream_wanted(&self) -> bool {
        self.stream != Stream::Closed
    }

    /// Next time [`Hold::on_tick`] has work to do.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let cap = match self.phase {
            Phase::Held { since, .. }
            | Phase::EndPending { since }
            | Phase::Tapped { since, .. } => Some(since + self.cfg.max_hold),
            Phase::Idle | Phase::WaitRelease => None,
        };
        let stream = match self.stream {
            Stream::Open {
                opened,
                first: None,
                ..
            } => Some(opened + FIRST_DATA_WAIT),
            _ => None,
        };
        match (cap, stream) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    // ---- the stream ----

    fn open_stream(&mut self) {
        if self.cfg.remote_mic && self.stream == Stream::Closed {
            self.stream = Stream::Opening;
            self.out.push(Act::OpenStream);
        }
    }

    /// Closes the stream (if open or opening) and gives the audio back to
    /// the TV microphone. The audio kept for the run is dropped unless
    /// `keep_preroll`.
    fn close_stream(&mut self, keep_preroll: bool) {
        if self.stream != Stream::Closed {
            self.stream = Stream::Closed;
            self.out.push(Act::CloseStream);
        }
        self.stop_feeding();
        if !keep_preroll {
            self.clear_preroll();
        }
    }

    fn stop_feeding(&mut self) {
        if self.feeding {
            self.feeding = false;
            self.out.push(Act::RemoteFeeds(false));
        }
    }

    fn clear_preroll(&mut self) {
        self.preroll.clear();
        self.preroll_samples = 0;
    }

    /// The remote feeds the run from now on; with `flush`, the audio kept
    /// so far goes first.
    fn start_feeding(&mut self, flush: bool) {
        if !self.feeding {
            self.feeding = true;
            self.out.push(Act::RemoteFeeds(true));
        }
        if flush {
            let kept: Vec<Vec<i16>> = self.preroll.drain(..).collect();
            self.out.extend(kept.into_iter().map(Act::Audio));
        }
        self.clear_preroll();
    }

    fn push_preroll(&mut self, chunk: Vec<i16>) {
        self.preroll_samples += chunk.len();
        self.preroll.push_back(chunk);
        while self.preroll_samples > PREROLL_MAX {
            match self.preroll.pop_front() {
                Some(c) => self.preroll_samples -= c.len(),
                None => break,
            }
        }
    }

    /// The stream's socket is connected.
    pub(crate) fn on_stream_opened(&mut self, now: Instant) -> Vec<Act> {
        if self.stream != Stream::Opening {
            // Closed meanwhile (a tap, the cap, the run's end): a late open.
            self.out.push(Act::CloseStream);
            return self.take();
        }
        // The remote takes over at its first chunk (`on_chunk`), so a socket
        // that stays silent never costs the TV microphone's audio.
        self.stream = Stream::Open {
            opened: now,
            first: None,
            samples: 0,
            checked: false,
        };
        self.take()
    }

    /// The stream could not be opened, or it ended or failed: the TV
    /// microphone takes over (also in the middle of a run).
    pub(crate) fn on_stream_lost(&mut self) -> Vec<Act> {
        if self.stream != Stream::Closed {
            self.stream = Stream::Closed;
            // Closing a session that already ended releases what is left of
            // it (the subscription child).
            self.out.push(Act::CloseStream);
        }
        self.stop_feeding();
        self.clear_preroll();
        self.take()
    }

    /// A chunk of remote audio (after the front-end, [`CHUNK_SAMPLES`]).
    /// Returns the actions and, when the rate check failed, the measured
    /// rate for the log (the stream is then closed).
    pub(crate) fn on_chunk(&mut self, now: Instant, chunk: Vec<i16>) -> (Vec<Act>, Option<f64>) {
        let Stream::Open {
            opened,
            first,
            samples,
            checked,
        } = self.stream
        else {
            return (Vec::new(), None);
        };
        let (first_at, samples) = match first {
            None => (now, 0),
            Some(f) => (f, samples + chunk.len() as u64),
        };
        let mut checked = checked;
        let elapsed = now.saturating_duration_since(first_at);
        if !checked && elapsed >= RATE_WINDOW {
            #[allow(clippy::cast_precision_loss)]
            let rate = samples as f64 / elapsed.as_secs_f64();
            if !(EXPECTED_RATE * RATE_MIN..=EXPECTED_RATE * RATE_MAX).contains(&rate) {
                let acts = self.on_stream_lost();
                return (acts, Some(rate));
            }
            checked = true;
        }
        self.stream = Stream::Open {
            opened,
            first: Some(first_at),
            samples,
            checked,
        };
        match self.phase {
            _ if self.feeding => self.out.push(Act::Audio(chunk)),
            Phase::Held { started: true, .. } => {
                // The run streams the TV microphone already: switch now;
                // nothing kept to send (it would repeat what HA got).
                self.start_feeding(false);
                self.out.push(Act::Audio(chunk));
            }
            Phase::Held { started: false, .. } => self.push_preroll(chunk),
            _ => {}
        }
        (self.take(), None)
    }

    // ---- the button ----

    /// The button went down.
    pub(crate) fn on_press(&mut self, now: Instant, ctx: Context) -> (PressOutcome, Vec<Act>) {
        if self.key_down {
            return (PressOutcome::Ignored, Vec::new());
        }
        self.key_down = true;
        if self
            .last_release
            .is_some_and(|r| now.saturating_duration_since(r) < DEBOUNCE)
        {
            // A bounce: the press and its release are ignored; a run of the
            // previous press goes on.
            if matches!(self.phase, Phase::Idle) {
                self.phase = Phase::WaitRelease;
            }
            return (PressOutcome::Ignored, Vec::new());
        }
        // A new press replaces what is left of the previous one (a tap's
        // run, a release waiting for its run).
        self.close_stream(false);
        self.phase = Phase::WaitRelease;
        let refusal = if !ctx.connected {
            Some("Home Assistant is not connected")
        } else if ctx.mic_off {
            Some("the microphone is muted")
        } else if ctx.announcing {
            Some("an announcement is playing")
        } else if !ctx.mic_open {
            Some("the TV microphone delivers no audio")
        } else {
            None
        };
        if let Some(why) = refusal {
            return (PressOutcome::Refused(why), self.take());
        }
        let outcome = if ctx.run_streaming {
            self.phase = Phase::Held {
                since: now,
                started: true,
            };
            PressOutcome::Joined
        } else {
            self.phase = Phase::Held {
                since: now,
                started: false,
            };
            self.out.push(Act::StartRun);
            PressOutcome::Started
        };
        self.open_stream();
        (outcome, self.take())
    }

    /// The button went up (or its device went away while it was down).
    pub(crate) fn on_release(&mut self, now: Instant) -> Vec<Act> {
        if !self.key_down {
            return Vec::new();
        }
        self.key_down = false;
        self.last_release = Some(now);
        match self.phase {
            Phase::Held { since, started } => {
                let held = now.saturating_duration_since(since);
                if held < self.cfg.tap {
                    // Tap-to-talk on the TV microphone; the remote's
                    // microphone is live only while the button is held.
                    self.close_stream(false);
                    self.phase = Phase::Tapped { since, started };
                } else if started {
                    self.end_audio_now();
                } else {
                    // Keep what the remote sent: it is the whole utterance.
                    self.close_stream(true);
                    self.phase = Phase::EndPending { since };
                }
            }
            Phase::WaitRelease => self.phase = Phase::Idle,
            Phase::Idle | Phase::EndPending { .. } | Phase::Tapped { .. } => {}
        }
        self.take()
    }

    /// Ends the run's audio now: `EndAudio` after the last remote chunk,
    /// then the stream closes and the TV microphone's chunks flow again
    /// (HA no longer takes them for this run).
    fn end_audio_now(&mut self) {
        self.out.push(Act::EndAudio);
        self.close_stream(false);
        self.phase = if self.key_down {
            Phase::WaitRelease
        } else {
            Phase::Idle
        };
    }

    // ---- the run ----

    /// The voice layer started streaming a run without the wake word
    /// (`StartStreaming { wake_word: false }`).
    pub(crate) fn on_run_started(&mut self) -> Vec<Act> {
        match self.phase {
            Phase::Held {
                since,
                started: false,
            } => {
                self.phase = Phase::Held {
                    since,
                    started: true,
                };
                if matches!(self.stream, Stream::Open { first: Some(_), .. }) {
                    // The remote was there first: it feeds the whole run,
                    // starting with what it kept.
                    self.start_feeding(true);
                }
            }
            Phase::EndPending { since } => {
                if self.preroll.is_empty() {
                    // Nothing was kept (the stream never opened): HA hears the
                    // TV microphone and its end-of-speech ends the run.
                    self.phase = Phase::Tapped {
                        since,
                        started: true,
                    };
                } else {
                    self.start_feeding(true);
                    self.end_audio_now();
                }
            }
            Phase::Tapped {
                since,
                started: false,
            } => {
                self.phase = Phase::Tapped {
                    since,
                    started: true,
                };
            }
            _ => {}
        }
        self.take()
    }

    /// The run's audio ended without us (HA's end of speech, an error, an
    /// abort, the end of the run): `StopStreaming` or `RunEnd`.
    pub(crate) fn on_audio_over(&mut self) -> Vec<Act> {
        match self.phase {
            Phase::Held { started: true, .. } | Phase::Tapped { started: true, .. } => {
                self.close_stream(false);
                self.phase = if self.key_down {
                    Phase::WaitRelease
                } else {
                    Phase::Idle
                };
            }
            // Before our run streams, a stop belongs to the run it replaced.
            _ => {}
        }
        self.take()
    }

    /// Everything stops now (mute, a gate, HA gone, shutdown): the stream
    /// closes and no audio is ended (the voice layer already stopped the
    /// run). Returns nothing when there was nothing to stop.
    pub(crate) fn halt(&mut self) -> Vec<Act> {
        self.close_stream(false);
        if !matches!(self.phase, Phase::Idle) {
            self.phase = if self.key_down {
                Phase::WaitRelease
            } else {
                Phase::Idle
            };
        }
        self.take()
    }

    /// Time passed: the cap of the hold and the first-data deadline of the
    /// stream. Returns the actions and what happened, for the log.
    pub(crate) fn on_tick(&mut self, now: Instant) -> (Vec<Act>, Option<TickNote>) {
        let mut note = None;
        if let Stream::Open {
            opened,
            first: None,
            ..
        } = self.stream
        {
            if now >= opened + FIRST_DATA_WAIT {
                let lost = self.on_stream_lost();
                self.out.extend(lost);
                note = Some(TickNote::NoData);
            }
        }
        match self.phase {
            Phase::Held { since, started } if now >= since + self.cfg.max_hold => {
                if started {
                    self.end_audio_now();
                    note = Some(TickNote::Cap);
                } else {
                    self.close_stream(false);
                    self.phase = Phase::WaitRelease;
                    note = Some(TickNote::NotStarted);
                }
            }
            Phase::Tapped { since, started } if now >= since + self.cfg.max_hold => {
                if started {
                    self.out.push(Act::EndAudio);
                    note = Some(TickNote::Cap);
                } else {
                    note = Some(TickNote::NotStarted);
                }
                self.phase = Phase::Idle;
            }
            Phase::EndPending { since } if now >= since + self.cfg.max_hold => {
                self.clear_preroll();
                self.phase = Phase::Idle;
                note = Some(TickNote::NotStarted);
            }
            _ => {}
        }
        (self.take(), note)
    }
}

/// Hand-over of the mic button from LG's assistant: the seam for
/// [ADR 0013](../../../docs/decisions/0013-remote-mic-button-coexistence.md)
/// (**proposed**, not accepted; EXP-023 has not run). Its mechanisms (a
/// volatile configd override of `com.webos.keyaction.voice`, else a SIGSTOP
/// of `voiceconductor`), the state file and `ctl.sh restore-lg-voice` are
/// **not built**: the only implementation is [`LeaveToLg`], which changes
/// nothing, so LG's assistant still reacts to every press.
///
/// The daemon calls, in this order: [`LgVoiceKey::acquire`] once when hold-to-
/// talk starts (after the key device was found by name, the self-check of
/// ADR 0013 decision 3), [`LgVoiceKey::run_started`] and
/// [`LgVoiceKey::run_ended`] around each button run (for a pause limited to
/// the run, ADR 0013 decision 2(b) refinement), and [`LgVoiceKey::release`]
/// when the feature stops (shutdown). An implementation must be idempotent
/// and must not block the event loop (spawn its luna calls).
pub(crate) trait LgVoiceKey: Send {
    /// Takes the key from LG; returns what is in force afterwards.
    fn acquire(&mut self) -> KeyOwner;
    /// Gives the key back to LG.
    fn release(&mut self);
    /// A run from the button starts (the press was accepted).
    fn run_started(&mut self) {}
    /// That run's audio is over (released, capped, ended or halted).
    fn run_ended(&mut self) {}
}

/// Who handles the mic button besides the satellite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyOwner {
    /// LG's assistant still opens on every press (both react).
    Lg,
    /// LG's assistant was kept away by the named mechanism.
    #[cfg_attr(not(test), allow(dead_code))] // the ADR 0013 mechanisms are not built
    Ours(&'static str),
}

/// The hand-over that is built today: none (ADR 0013 is proposed).
#[derive(Debug, Default)]
pub(crate) struct LeaveToLg;

impl LgVoiceKey for LeaveToLg {
    fn acquire(&mut self) -> KeyOwner {
        KeyOwner::Lg
    }

    fn release(&mut self) {}
}

/// What [`Hold::on_tick`] did, for the daemon's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TickNote {
    /// The longest hold was reached: the audio ended.
    Cap,
    /// Home Assistant did not start the run before the cap.
    NotStarted,
    /// The remote stream sent nothing after it opened.
    NoData,
}

/// The feature in the daemon: the button reader's events, the machine, the
/// remote microphone and the hand-over seam. It runs the stream actions
/// itself and returns the others ([`Act::StartRun`], [`Act::EndAudio`],
/// [`Act::Audio`], [`Act::RemoteFeeds`]) for the daemon. With `mic_button =
/// "off"` the key is only observed: presses are logged at debug and nothing
/// else happens.
pub(crate) struct Button {
    ptt: bool,
    hold: Hold,
    mic: Option<RemoteMic>,
    lg: Box<dyn LgVoiceKey>,
    acquired: bool,
    /// [`Hold::active`] after the last event (for the run hooks).
    active: bool,
    pressed_at: Option<Instant>,
    stop: Arc<AtomicBool>,
}

/// What [`Button::start`] returns: the feature and its two event streams.
pub(crate) struct Started {
    pub(crate) button: Button,
    pub(crate) keys: mpsc::UnboundedReceiver<Key>,
    pub(crate) stream: mpsc::UnboundedReceiver<(u64, StreamEvent)>,
}

impl Button {
    /// Starts the reader thread (always passive) and, with hold-to-talk,
    /// prepares the remote microphone (`luna`: `None` keeps every run on
    /// the TV microphone).
    pub(crate) fn start(
        section: &RemoteSection,
        luna: Option<PathBuf>,
        hpf_hz: f32,
        paths: InputPaths,
        lg: Box<dyn LgVoiceKey>,
    ) -> std::io::Result<Started> {
        let (key_tx, keys) = mpsc::unbounded_channel();
        let (stream_tx, stream) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        evdev::spawn(
            paths,
            section.device_name.clone(),
            section.key_code,
            Timing::default(),
            key_tx,
            Arc::clone(&stop),
        )?;
        let ptt = section.ptt();
        let mic = (ptt && section.remote_mic)
            .then(|| RemoteMic::new(MicConfig::new(luna, hpf_hz, section.gain_db), stream_tx));
        if ptt {
            log::info!(
                "remote mic button: hold to talk (audio from the {}); LG's assistant hand-over is not built (ADR 0013 proposed): LG's assistant still opens on each press",
                if mic.is_some() {
                    "remote's microphone, TV microphone as fallback"
                } else {
                    "TV microphone"
                }
            );
        } else {
            log::info!("remote mic button: off (mic_button = \"off\"): the key is only observed");
        }
        Ok(Started {
            button: Self {
                ptt,
                hold: Hold::new(HoldConfig::from_section(section)),
                mic,
                lg,
                acquired: false,
                active: false,
                pressed_at: None,
                stop,
            },
            keys,
            stream,
        })
    }

    /// Next time [`Button::on_tick`] has work.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.hold.next_deadline()
    }

    /// Runs the stream actions, calls the run hooks, returns the rest.
    fn apply(&mut self, acts: Vec<Act>) -> Vec<Act> {
        let mut rest = Vec::with_capacity(acts.len());
        for a in acts {
            match a {
                Act::OpenStream => {
                    if let Some(m) = self.mic.as_mut() {
                        m.open();
                    }
                }
                Act::CloseStream => {
                    if let Some(m) = self.mic.as_mut() {
                        m.close();
                    }
                }
                other => rest.push(other),
            }
        }
        let active = self.hold.active();
        if self.active && !active {
            self.lg.run_ended();
        }
        self.active = active;
        rest
    }

    /// An event of the button reader.
    pub(crate) fn on_key(&mut self, key: Key, now: Instant, ctx: Context) -> Vec<Act> {
        match key {
            Key::Found => {
                if self.ptt && !self.acquired {
                    self.acquired = true;
                    match self.lg.acquire() {
                        KeyOwner::Lg => log::info!(
                            "remote mic button: key device found; LG's assistant keeps reacting to the button (no hand-over built)"
                        ),
                        KeyOwner::Ours(how) => {
                            log::info!("remote mic button: LG's assistant kept away ({how})");
                        }
                    }
                }
                Vec::new()
            }
            Key::Pressed if !self.ptt => {
                log::debug!("remote mic button pressed (mic_button = \"off\": observed only)");
                Vec::new()
            }
            Key::Released if !self.ptt => Vec::new(),
            Key::Pressed => {
                let (outcome, acts) = self.hold.on_press(now, ctx);
                match outcome {
                    PressOutcome::Started => {
                        self.pressed_at = Some(now);
                        log::info!("remote mic button: pressed; push-to-talk run requested");
                        self.lg.run_started();
                    }
                    PressOutcome::Joined => {
                        self.pressed_at = Some(now);
                        log::info!(
                            "remote mic button: pressed during a run; it takes the remote's audio"
                        );
                        self.lg.run_started();
                    }
                    PressOutcome::Refused(why) => {
                        log::warn!("remote mic button: pressed, refused: {why}");
                    }
                    PressOutcome::Ignored => log::debug!("remote mic button: press ignored"),
                }
                self.apply(acts)
            }
            Key::Released => {
                if let Some(at) = self.pressed_at.take() {
                    log::info!(
                        "remote mic button: released after {} ms",
                        now.saturating_duration_since(at).as_millis()
                    );
                }
                let acts = self.hold.on_release(now);
                self.apply(acts)
            }
        }
    }

    /// An event of the remote microphone (`generation`: its session).
    pub(crate) fn on_stream(&mut self, generation: u64, ev: StreamEvent, now: Instant) -> Vec<Act> {
        if self
            .mic
            .as_ref()
            .is_none_or(|m| m.generation() != generation)
        {
            return Vec::new();
        }
        let acts = match ev {
            StreamEvent::Opened => {
                log::info!("remote mic: stream open");
                self.hold.on_stream_opened(now)
            }
            StreamEvent::Chunk(c) => {
                let (acts, bad) = self.hold.on_chunk(now, c);
                if let Some(rate) = bad {
                    log::warn!(
                        "remote mic: the stream delivers {rate:.0} samples/s, not 16000 (another format?); using the TV microphone"
                    );
                }
                acts
            }
            StreamEvent::Lost(why) => {
                if self.hold.stream_wanted() {
                    log::warn!("remote mic: {why}; using the TV microphone");
                }
                self.hold.on_stream_lost()
            }
        };
        self.apply(acts)
    }

    /// Time passed (the cap, the stream's first data).
    pub(crate) fn on_tick(&mut self, now: Instant) -> Vec<Act> {
        let (acts, note) = self.hold.on_tick(now);
        match note {
            Some(TickNote::Cap) => {
                log::info!("remote mic button: longest hold reached; audio ended");
            }
            Some(TickNote::NotStarted) => {
                log::warn!("remote mic button: Home Assistant did not start the run in time");
            }
            Some(TickNote::NoData) => {
                log::warn!("remote mic: the stream sent nothing; using the TV microphone");
            }
            None => {}
        }
        self.apply(acts)
    }

    /// `StartStreaming { wake_word: false }`.
    pub(crate) fn on_run_started(&mut self) -> Vec<Act> {
        let acts = self.hold.on_run_started();
        self.apply(acts)
    }

    /// `StopStreaming` or `RunEnd`.
    pub(crate) fn on_audio_over(&mut self) -> Vec<Act> {
        let acts = self.hold.on_audio_over();
        self.apply(acts)
    }

    /// Mute, a gate, HA gone.
    pub(crate) fn halt(&mut self, why: &str) -> Vec<Act> {
        let acts = self.hold.halt();
        if !acts.is_empty() {
            log::info!("remote mic button: stopped ({why})");
        }
        self.apply(acts)
    }

    /// Daemon shutdown: the stream closes (its child is reaped), the key
    /// goes back to LG, the reader thread is told to stop.
    pub(crate) async fn shutdown(&mut self) -> Vec<Act> {
        let acts = self.halt("shutdown");
        if let Some(m) = self.mic.as_mut() {
            m.shutdown().await;
        }
        if self.acquired {
            self.lg.release();
            self.acquired = false;
        }
        self.stop.store(true, Ordering::Relaxed);
        acts
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::many_single_char_names
    )]
    use super::*;
    use crate::capture::CHUNK_SAMPLES as CHUNK;

    fn cfg() -> HoldConfig {
        HoldConfig {
            remote_mic: true,
            max_hold: Duration::from_secs(15),
            tap: Duration::from_millis(300),
        }
    }

    fn ok_ctx() -> Context {
        Context {
            connected: true,
            mic_off: false,
            mic_open: true,
            announcing: false,
            run_streaming: false,
        }
    }

    fn ms(t: Instant, n: u64) -> Instant {
        t + Duration::from_millis(n)
    }

    fn chunk(v: i16) -> Vec<i16> {
        vec![v; CHUNK]
    }

    /// Feeds `n` chunks at the real rate from `from`; returns the time after.
    fn feed(h: &mut Hold, from: Instant, n: u64, v: i16) -> (Instant, Vec<Act>) {
        let mut out = Vec::new();
        let mut t = from;
        for _ in 0..n {
            let (a, bad) = h.on_chunk(t, chunk(v));
            assert!(bad.is_none());
            out.extend(a);
            t += Duration::from_millis(32);
        }
        (t, out)
    }

    fn audio_count(acts: &[Act]) -> usize {
        acts.iter().filter(|a| matches!(a, Act::Audio(_))).count()
    }

    #[test]
    fn hold_streams_the_remote_with_its_preroll_and_release_ends_the_audio() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        let (o, a) = h.on_press(t0, ok_ctx());
        assert_eq!(o, PressOutcome::Started);
        assert_eq!(a, [Act::StartRun, Act::OpenStream]);
        assert!(h.on_stream_opened(ms(t0, 30)).is_empty());
        // Audio before the run streams is kept.
        let (t, a) = feed(&mut h, ms(t0, 40), 5, 7);
        assert!(a.is_empty());
        // The run starts: the remote feeds it, the kept audio goes first.
        let a = h.on_run_started();
        assert_eq!(a[0], Act::RemoteFeeds(true));
        assert_eq!(audio_count(&a), 5);
        assert!(h.feeding());
        let (_, a) = feed(&mut h, t, 10, 9);
        assert_eq!(audio_count(&a), 10);
        // Release after 2 s: end of audio, then the stream closes and the TV
        // microphone flows again.
        let a = h.on_release(ms(t0, 2000));
        assert_eq!(
            a,
            [Act::EndAudio, Act::CloseStream, Act::RemoteFeeds(false)]
        );
        assert!(!h.stream_wanted());
        assert_eq!(h.next_deadline(), None);
        // The run's own stop afterwards changes nothing.
        assert!(h.on_audio_over().is_empty());
        assert!(h.on_run_started().is_empty());
    }

    #[test]
    fn a_tap_continues_on_the_tv_microphone_and_closes_the_remote_at_once() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        feed(&mut h, ms(t0, 40), 3, 1);
        let a = h.on_release(ms(t0, 250));
        assert_eq!(a, [Act::CloseStream]);
        // The run starts later on the TV microphone: no remote audio, no end.
        assert!(h.on_run_started().is_empty());
        // HA's end of speech ends it; nothing for us to do but forget it.
        assert!(h.on_audio_over().is_empty());
        assert_eq!(h.next_deadline(), None);
    }

    #[test]
    fn a_tap_whose_run_hears_no_end_is_capped() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_run_started();
        h.on_release(ms(t0, 100));
        assert_eq!(h.next_deadline(), Some(ms(t0, 15_000)));
        let (a, n) = h.on_tick(ms(t0, 14_999));
        assert!(a.is_empty() && n.is_none());
        let (a, n) = h.on_tick(ms(t0, 15_000));
        assert_eq!(a, [Act::EndAudio]);
        assert_eq!(n, Some(TickNote::Cap));
        assert_eq!(h.next_deadline(), None);
    }

    #[test]
    fn tap_ms_zero_makes_every_release_end_the_audio() {
        let t0 = Instant::now();
        let mut h = Hold::new(HoldConfig {
            tap: Duration::ZERO,
            ..cfg()
        });
        h.on_press(t0, ok_ctx());
        h.on_run_started();
        let a = h.on_release(ms(t0, 10));
        assert_eq!(a, [Act::EndAudio, Act::CloseStream]);
    }

    #[test]
    fn the_cap_ends_the_audio_while_held_and_the_release_does_nothing() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        h.on_run_started();
        let (t, _) = feed(&mut h, ms(t0, 40), 3, 1);
        assert_eq!(h.next_deadline(), Some(ms(t0, 15_000)));
        let (a, n) = h.on_tick(ms(t0, 15_000));
        assert_eq!(
            a,
            [Act::EndAudio, Act::CloseStream, Act::RemoteFeeds(false)]
        );
        assert_eq!(n, Some(TickNote::Cap));
        // Late chunks of the closed stream are dropped.
        let (a, _) = h.on_chunk(t, chunk(1));
        assert!(a.is_empty());
        assert!(h.on_release(ms(t0, 20_000)).is_empty());
        assert_eq!(h.next_deadline(), None);
    }

    #[test]
    fn a_run_that_never_starts_is_given_up_at_the_cap() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        feed(&mut h, ms(t0, 40), 3, 1);
        let (a, n) = h.on_tick(ms(t0, 15_000));
        assert_eq!(a, [Act::CloseStream]);
        assert_eq!(n, Some(TickNote::NotStarted));
        // A run that starts after that is not ours to end.
        assert!(h.on_run_started().is_empty());
        assert!(h.on_release(ms(t0, 16_000)).is_empty());
    }

    #[test]
    fn refusals_open_nothing_and_swallow_the_release() {
        let t0 = Instant::now();
        for (ctx, why) in [
            (
                Context {
                    connected: false,
                    ..ok_ctx()
                },
                "Home Assistant is not connected",
            ),
            (
                Context {
                    mic_off: true,
                    ..ok_ctx()
                },
                "the microphone is muted",
            ),
            (
                Context {
                    announcing: true,
                    ..ok_ctx()
                },
                "an announcement is playing",
            ),
            (
                Context {
                    mic_open: false,
                    ..ok_ctx()
                },
                "the TV microphone delivers no audio",
            ),
            // Muted wins over a joinable run.
            (
                Context {
                    mic_off: true,
                    run_streaming: true,
                    ..ok_ctx()
                },
                "the microphone is muted",
            ),
        ] {
            let mut h = Hold::new(cfg());
            let (o, a) = h.on_press(t0, ctx);
            assert_eq!(o, PressOutcome::Refused(why));
            assert!(a.is_empty(), "{a:?}");
            assert!(h.on_release(ms(t0, 2000)).is_empty());
            assert_eq!(h.next_deadline(), None);
            // The next press works.
            let (o, _) = h.on_press(ms(t0, 3000), ok_ctx());
            assert_eq!(o, PressOutcome::Started);
        }
    }

    #[test]
    fn a_streaming_run_is_joined_and_switches_to_the_remote_without_preroll() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        let (o, a) = h.on_press(
            t0,
            Context {
                run_streaming: true,
                ..ok_ctx()
            },
        );
        assert_eq!(o, PressOutcome::Joined);
        assert_eq!(a, [Act::OpenStream]);
        // The TV microphone already feeds the run: switch at the remote's
        // first chunk, nothing kept to repeat.
        assert!(h.on_stream_opened(ms(t0, 40)).is_empty());
        let (_, a) = feed(&mut h, ms(t0, 50), 4, 3);
        assert_eq!(a[0], Act::RemoteFeeds(true));
        assert_eq!(audio_count(&a), 4);
        let a = h.on_release(ms(t0, 1500));
        assert_eq!(
            a,
            [Act::EndAudio, Act::CloseStream, Act::RemoteFeeds(false)]
        );
    }

    #[test]
    fn the_stop_of_the_replaced_run_does_not_end_the_new_one() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        // The idle wake word session is aborted: its StopStreaming and
        // RunEnd come before our run's StartStreaming.
        assert!(h.on_audio_over().is_empty());
        assert!(h.stream_wanted());
        feed(&mut h, ms(t0, 40), 1, 1);
        let a = h.on_run_started();
        assert_eq!(a[0], Act::RemoteFeeds(true));
        assert_eq!(audio_count(&a), 1);
        // Our run's own stop (HA's VAD) ends it while held: stream closed,
        // the release then does nothing.
        let a = h.on_audio_over();
        assert_eq!(a, [Act::CloseStream, Act::RemoteFeeds(false)]);
        assert!(h.on_release(ms(t0, 3000)).is_empty());
    }

    #[test]
    fn a_release_before_the_run_streams_sends_the_kept_audio_and_ends_it() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        let (t, _) = feed(&mut h, ms(t0, 40), 20, 5);
        // Held for about 700 ms; HA has not started the run yet (slow).
        let a = h.on_release(t);
        assert_eq!(a, [Act::CloseStream]);
        let a = h.on_run_started();
        assert_eq!(a.first(), Some(&Act::RemoteFeeds(true)));
        assert_eq!(audio_count(&a), 20);
        assert_eq!(&a[a.len() - 2..], [Act::EndAudio, Act::RemoteFeeds(false)]);
        assert_eq!(h.next_deadline(), None);
    }

    #[test]
    fn a_release_before_the_run_streams_without_remote_audio_leaves_it_to_ha() {
        let t0 = Instant::now();
        let mut h = Hold::new(HoldConfig {
            remote_mic: false,
            ..cfg()
        });
        let (_, a) = h.on_press(t0, ok_ctx());
        assert_eq!(a, [Act::StartRun]);
        assert!(h.on_release(ms(t0, 800)).is_empty());
        // Nothing was kept: HA's end of speech ends the run, capped.
        assert!(h.on_run_started().is_empty());
        assert_eq!(h.next_deadline(), Some(ms(t0, 15_000)));
        assert!(h.on_audio_over().is_empty());
        assert_eq!(h.next_deadline(), None);
    }

    #[test]
    fn tv_microphone_only_hold_ends_the_audio_at_the_release() {
        let t0 = Instant::now();
        let mut h = Hold::new(HoldConfig {
            remote_mic: false,
            ..cfg()
        });
        h.on_press(t0, ok_ctx());
        assert!(h.on_run_started().is_empty());
        assert_eq!(h.on_release(ms(t0, 900)), [Act::EndAudio]);
    }

    #[test]
    fn a_stream_that_fails_falls_back_to_the_tv_microphone() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        // The luna call failed (2003 three times, luna-send missing, ...):
        // the ended session is closed (its subscription child released).
        assert_eq!(h.on_stream_lost(), [Act::CloseStream]);
        assert!(!h.stream_wanted());
        assert!(h.on_run_started().is_empty());
        // The release still ends the audio (of the TV microphone).
        assert_eq!(h.on_release(ms(t0, 2000)), [Act::EndAudio]);
    }

    #[test]
    fn a_stream_that_ends_mid_run_gives_the_audio_back_to_the_tv() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        h.on_run_started();
        feed(&mut h, ms(t0, 40), 3, 1);
        assert!(h.feeding());
        assert_eq!(
            h.on_stream_lost(),
            [Act::CloseStream, Act::RemoteFeeds(false)]
        );
        assert!(!h.feeding());
        assert_eq!(h.on_release(ms(t0, 2000)), [Act::EndAudio]);
    }

    #[test]
    fn a_stream_without_data_is_dropped_after_the_first_data_wait() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        h.on_run_started();
        // The TV microphone feeds the run meanwhile.
        assert!(!h.feeding());
        assert_eq!(h.next_deadline(), Some(ms(t0, 1030)));
        let (a, n) = h.on_tick(ms(t0, 1030));
        assert_eq!(a, [Act::CloseStream]);
        assert_eq!(n, Some(TickNote::NoData));
        assert_eq!(h.next_deadline(), Some(ms(t0, 15_000)));
    }

    #[test]
    fn a_stream_at_the_wrong_rate_is_dropped() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        h.on_run_started();
        // Half the expected rate (8 kHz): one chunk per 64 ms.
        let mut t = ms(t0, 40);
        let mut bad = None;
        for _ in 0..40 {
            let (_, b) = h.on_chunk(t, chunk(1));
            if b.is_some() {
                bad = b;
                break;
            }
            t += Duration::from_millis(64);
        }
        let rate = bad.expect("the rate check fails");
        assert!((7000.0..9000.0).contains(&rate), "{rate}");
        assert!(!h.feeding() && !h.stream_wanted());
        // A stream at the right rate passes the same check.
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        h.on_run_started();
        let (_, a) = feed(&mut h, ms(t0, 40), 60, 1);
        assert_eq!(audio_count(&a), 60);
        assert!(h.feeding());
    }

    #[test]
    fn the_preroll_keeps_the_last_two_seconds() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        let (_, _) = feed(&mut h, ms(t0, 40), 100, 1);
        let a = h.on_run_started();
        assert_eq!(audio_count(&a), PREROLL_MAX / CHUNK);
    }

    #[test]
    fn mute_or_a_lost_connection_halts_everything() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_stream_opened(ms(t0, 30));
        h.on_run_started();
        feed(&mut h, ms(t0, 40), 2, 1);
        assert_eq!(h.halt(), [Act::CloseStream, Act::RemoteFeeds(false)]);
        assert!(h.halt().is_empty());
        assert!(h.on_release(ms(t0, 2000)).is_empty());
        assert_eq!(h.next_deadline(), None);
    }

    #[test]
    fn a_late_open_after_a_tap_is_closed_again() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_release(ms(t0, 100));
        assert_eq!(h.on_stream_opened(ms(t0, 150)), [Act::CloseStream]);
        assert!(!h.stream_wanted());
    }

    #[test]
    fn duplicate_presses_and_bounces_are_ignored() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        let (o, a) = h.on_press(ms(t0, 10), ok_ctx());
        assert_eq!(o, PressOutcome::Ignored);
        assert!(a.is_empty());
        h.on_run_started();
        h.on_release(ms(t0, 1000));
        // A press 20 ms after the release is a bounce; so is its release.
        let (o, a) = h.on_press(ms(t0, 1020), ok_ctx());
        assert_eq!(o, PressOutcome::Ignored);
        assert!(a.is_empty());
        assert!(h.on_release(ms(t0, 1040)).is_empty());
        // A release without a press does nothing.
        assert!(h.on_release(ms(t0, 1100)).is_empty());
        let (o, _) = h.on_press(ms(t0, 2000), ok_ctx());
        assert_eq!(o, PressOutcome::Started);
    }

    /// A hand-over that records its calls.
    struct FakeLg(Arc<std::sync::Mutex<Vec<&'static str>>>);

    impl LgVoiceKey for FakeLg {
        fn acquire(&mut self) -> KeyOwner {
            self.0.lock().unwrap().push("acquire");
            KeyOwner::Ours("fake")
        }
        fn release(&mut self) {
            self.0.lock().unwrap().push("release");
        }
        fn run_started(&mut self) {
            self.0.lock().unwrap().push("run_started");
        }
        fn run_ended(&mut self) {
            self.0.lock().unwrap().push("run_ended");
        }
    }

    fn button(ptt: bool, tag: &str) -> (Button, Arc<std::sync::Mutex<Vec<&'static str>>>, PathBuf) {
        let (root, paths) = crate::evdev::tests::fake_inputs(tag, &[]);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let section = RemoteSection {
            mic_button: if ptt {
                crate::config::MicButton::Ptt
            } else {
                crate::config::MicButton::Off
            },
            ..RemoteSection::default()
        };
        let st = Button::start(
            &section,
            None,
            120.0,
            paths,
            Box::new(FakeLg(Arc::clone(&calls))),
        )
        .unwrap();
        (st.button, calls, root)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_hand_over_seam_is_called_around_the_feature_and_each_run() {
        let (mut b, calls, root) = button(true, "seam");
        let t0 = Instant::now();
        assert!(b.on_key(Key::Found, t0, ok_ctx()).is_empty());
        b.on_key(Key::Found, t0, ok_ctx());
        let a = b.on_key(Key::Pressed, t0, ok_ctx());
        // The stream actions are the button's own; the run is the daemon's.
        assert_eq!(a, [Act::StartRun]);
        assert!(b.on_run_started().is_empty());
        let a = b.on_key(Key::Released, ms(t0, 1500), ok_ctx());
        assert_eq!(a, [Act::EndAudio]);
        // A refused press is no run.
        b.on_key(
            Key::Pressed,
            ms(t0, 3000),
            Context {
                mic_off: true,
                ..ok_ctx()
            },
        );
        b.on_key(Key::Released, ms(t0, 3500), ok_ctx());
        b.shutdown().await;
        assert_eq!(
            *calls.lock().unwrap(),
            ["acquire", "run_started", "run_ended", "release"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_mic_button_off_the_key_is_only_observed() {
        let (mut b, calls, root) = button(false, "off");
        let t0 = Instant::now();
        b.on_key(Key::Found, t0, ok_ctx());
        assert!(b.on_key(Key::Pressed, t0, ok_ctx()).is_empty());
        assert!(b.on_key(Key::Released, ms(t0, 2000), ok_ctx()).is_empty());
        assert_eq!(b.deadline(), None);
        assert!(b.shutdown().await.is_empty());
        assert!(calls.lock().unwrap().is_empty(), "LG's side untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn without_luna_send_a_held_run_falls_back_to_the_tv_microphone() {
        let (mut b, _calls, root) = button(true, "noluna");
        let t0 = Instant::now();
        assert_eq!(b.on_key(Key::Pressed, t0, ok_ctx()), [Act::StartRun]);
        // The session reports itself lost at once (no luna-send); the
        // daemon hands that event over.
        let g = b.mic.as_ref().unwrap().generation();
        assert_eq!(g, 1);
        let lost = StreamEvent::Lost("luna-send not found".into());
        assert!(b.on_stream(g, lost, ms(t0, 5)).is_empty());
        assert!(!b.hold.stream_wanted());
        // A stale event of an older session is ignored.
        assert!(b.on_stream(0, StreamEvent::Opened, ms(t0, 6)).is_empty());
        assert!(b.on_run_started().is_empty());
        assert_eq!(
            b.on_key(Key::Released, ms(t0, 1000), ok_ctx()),
            [Act::EndAudio]
        );
        b.shutdown().await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_press_during_a_tapped_run_takes_it_over() {
        let t0 = Instant::now();
        let mut h = Hold::new(cfg());
        h.on_press(t0, ok_ctx());
        h.on_run_started();
        h.on_release(ms(t0, 100));
        // The tapped run streams the TV microphone; a hold joins it.
        let (o, a) = h.on_press(
            ms(t0, 2000),
            Context {
                run_streaming: true,
                ..ok_ctx()
            },
        );
        assert_eq!(o, PressOutcome::Joined);
        assert_eq!(a, [Act::OpenStream]);
        assert_eq!(h.next_deadline(), Some(ms(t0, 17_000)));
    }
}
