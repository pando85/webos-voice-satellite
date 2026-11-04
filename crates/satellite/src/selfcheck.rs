//! The capture thread's side of the microphone self-check (design sections 3
//! and 9 S5; ADR 0006).
//!
//! [`Glue`] drives the pure engine of `webos-compat` from what the capture
//! thread already does: it reads the two `/proc/asound` text files (read-only)
//! before an open, reports the open outcome, and feeds a level digest of the
//! frames the thread reads anyway. **No extra PCM open, no sysfs or debugfs,
//! no recording**: only numbers (`FrameStats`) leave the loop. Results go to
//! the daemon as [`Note`]s over a channel; the daemon owns the entities, the
//! report and the files.
//!
//! A PCM that is **busy** (LG hands-free holds it) is reported once as a
//! capture problem but is never counted toward the bounded retries: the
//! thread keeps retrying with its own slow backoff, so turning hands-free off
//! later lets the microphone start without a restart. Every other open failure
//! uses the engine's 5, 10, 20, 40 and 80 s retries and then holds until a
//! rerun (HA button, `ctl.sh selfcheck`, restart, firmware change).

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;
use webos_compat::check::Grade;
use webos_compat::engine::{
    Config, OpenDecision, OpenOutcome, Overall, Phase, SelfCheck, SelfCheckResult, Step,
};
use webos_compat::pcm::{CapsFit, PcmDiscovery, WantedPcm, check_pcm};
use webos_compat::report::{AudioFacts, CaptureCounters, ChosenPcm};
use webos_compat::window::{FrameStats, Thresholds};

use crate::capture::Shared;

/// Sample width of the capture format (S24_LE; sets full scale).
pub(crate) const SAMPLE_BITS: u32 = 24;

/// What the capture thread tells the daemon.
#[derive(Debug)]
pub(crate) enum Note {
    /// A measurement window is running (or a rerun was requested).
    Checking,
    /// The microphone was closed before a run finished (mute, kill switch,
    /// not paired); it runs at the next open.
    NotRun,
    /// What the thread found about the audio device. The second field is
    /// whether the echo reference is present (`None`: not configured).
    Audio(Box<AudioFacts>, Option<bool>),
    /// A run finished or an attempt failed.
    Result(Box<Finished>),
}

/// A finished run or failed attempt.
#[derive(Debug, Clone)]
pub(crate) struct Finished {
    /// The engine's result (C1 and C8 are filled by the daemon).
    pub(crate) result: SelfCheckResult,
    /// Capture counters at that moment.
    pub(crate) counters: CaptureCounters,
}

/// Settings of the self-check for the capture thread.
#[derive(Debug, Clone)]
pub(crate) struct SelfCheckSettings {
    /// Measurement window.
    pub(crate) window: Duration,
    /// Configured sample rate.
    pub(crate) rate: u32,
    /// Card (and device) to look for, ALSA source only.
    pub(crate) wanted: Option<WantedPcm>,
    /// Substring of the PCM name, to name the chosen device in the report.
    pub(crate) pcm_name: Option<String>,
    /// `/proc/asound` (a directory with `cards` and `pcm`).
    pub(crate) asound: PathBuf,
}

/// The self-check as the capture thread drives it.
#[derive(Debug)]
pub(crate) struct Glue {
    engine: SelfCheck,
    t0: Instant,
    notes: UnboundedSender<Note>,
    cfg: SelfCheckSettings,
    /// The last open failure was "busy": retried forever, outside the engine.
    busy: bool,
    discovery: Option<PcmDiscovery>,
}

impl Glue {
    pub(crate) fn new(cfg: SelfCheckSettings, notes: UnboundedSender<Note>) -> Self {
        let engine = SelfCheck::new(Config {
            thresholds: Thresholds {
                window: cfg.window,
                ..Thresholds::default()
            },
            sample_rate: cfg.rate,
            sample_bits: SAMPLE_BITS,
            ..Config::default()
        });
        Self {
            engine,
            t0: Instant::now(),
            notes,
            cfg,
            busy: false,
            discovery: None,
        }
    }

    fn now(&self) -> Duration {
        self.t0.elapsed()
    }

    fn note(&self, n: Note) {
        let _ = self.notes.send(n);
    }

    /// Applies a rerun request (HA button, `ctl.sh selfcheck`, a firmware
    /// change): the retry counter is cleared and, with the microphone open,
    /// the measurement starts at once on the open stream. Returns true when a
    /// request was applied.
    pub(crate) fn poll_rerun(&mut self, shared: &Shared, mic_open: bool) -> bool {
        if !shared.selfcheck_rerun.swap(false, Ordering::Relaxed) {
            return false;
        }
        log::info!("self-check: re-run requested");
        self.busy = false;
        self.engine.rerun(self.now(), mic_open);
        self.note(Note::Checking);
        true
    }

    /// Whether the thread may try to open the microphone now. The engine
    /// holds the open while it waits for a retry or after the retries are
    /// used up; a busy PCM is tried again on the thread's own backoff.
    pub(crate) fn may_open(&self) -> bool {
        self.busy || matches!(self.engine.next_open(self.now()), OpenDecision::Open)
    }

    /// C2 before an open (ALSA source): reads `/proc/asound/cards` and `pcm`
    /// and checks the card by id. Returns true when it failed and the open
    /// must be skipped (the failure was counted and published). A listing
    /// that cannot be read is not judged.
    pub(crate) fn check_pcm(&mut self, shared: &Shared) -> bool {
        let Some(wanted) = self.cfg.wanted.clone() else {
            return false;
        };
        let (Ok(cards), Ok(pcm)) = (
            std::fs::read_to_string(self.cfg.asound.join("cards")),
            std::fs::read_to_string(self.cfg.asound.join("pcm")),
        ) else {
            return false;
        };
        let disc = PcmDiscovery::parse(&cards, &pcm);
        let c2 = check_pcm(&disc, &wanted, CapsFit::Unknown);
        self.note(Note::Audio(
            Box::new(AudioFacts::from_discovery(&disc, None)),
            None,
        ));
        self.discovery = Some(disc);
        let step = self.engine.on_pcm(self.now(), c2);
        let counted = step.result.is_some();
        self.apply(step, shared);
        c2.grade == Grade::Fail && counted
    }

    /// The device that will be (or was) opened, for the report.
    fn chosen(&self, channels: u16, rate: u32) -> Option<ChosenPcm> {
        let wanted = self.cfg.wanted.as_ref()?;
        let disc = self.discovery.as_ref()?;
        let name = self.cfg.pcm_name.as_deref();
        let device = wanted.device.or_else(|| {
            let caps = disc.capture_devices(&wanted.card_id);
            let index = disc.card_index(&wanted.card_id)?;
            name.and_then(|n| {
                disc.pcms
                    .iter()
                    .find(|p| {
                        p.card == index && p.capture && (p.id.contains(n) || p.name.contains(n))
                    })
                    .map(|p| p.device)
            })
            .or_else(|| caps.first().copied())
        })?;
        Some(ChosenPcm {
            card_id: wanted.card_id.clone(),
            device,
            channels: u32::from(channels),
            format: "S24_LE".to_owned(),
            rate,
        })
    }

    /// The open succeeded: the measurement starts (first open, after a
    /// rerun or after a failed attempt).
    pub(crate) fn opened(&mut self, channels: u16, rate: u32, reference: Option<bool>) {
        if self.busy {
            // Hands-free let go: start from scratch, outside the retry count.
            self.busy = false;
            self.engine.rerun(self.now(), false);
        }
        let _ = self.engine.on_open(self.now(), OpenOutcome::Opened);
        if let (Some(d), true) = (&self.discovery, self.cfg.wanted.is_some()) {
            self.note(Note::Audio(
                Box::new(AudioFacts::from_discovery(d, self.chosen(channels, rate))),
                reference,
            ));
        } else if reference.is_some() {
            self.note(Note::Audio(Box::default(), reference));
        }
        if self.engine.phase() == Phase::Measuring {
            self.note(Note::Checking);
        }
    }

    /// The open failed. `busy`: the PCM is held by another process.
    pub(crate) fn open_failed(&mut self, busy: bool, shared: &Shared) {
        if busy && self.busy {
            return;
        }
        if self.engine.phase() == Phase::Done {
            // A reopen after a good run failed: report it as a new attempt.
            self.engine.rerun(self.now(), false);
        }
        self.busy = busy;
        let outcome = if busy {
            OpenOutcome::Busy
        } else {
            OpenOutcome::Failed
        };
        let step = self.engine.on_open(self.now(), outcome);
        self.apply(step, shared);
    }

    /// Frames from the normal capture read. Returns true when the microphone
    /// must be closed (a measured failure).
    pub(crate) fn on_frame(&mut self, stats: &FrameStats, shared: &Shared) -> bool {
        let step = self.engine.on_frame(self.now(), stats);
        self.apply(step, shared)
    }

    /// Time passing without frames. Returns true when the microphone must be
    /// closed.
    pub(crate) fn on_tick(&mut self, shared: &Shared) -> bool {
        let step = self.engine.on_tick(self.now());
        self.apply(step, shared)
    }

    /// The microphone is closed (mute, kill switch, not paired, an error). A
    /// measurement in progress is abandoned.
    pub(crate) fn mic_closed(&mut self) {
        if self.engine.phase() == Phase::Measuring {
            self.engine.on_mic_closed();
            self.note(Note::NotRun);
        }
    }

    /// A rerun was asked for while the microphone is held closed by a gate.
    pub(crate) fn not_run(&self) {
        self.note(Note::NotRun);
    }

    /// Publishes a finished run. Returns whether the mic is to be closed.
    fn apply(&mut self, step: Step, shared: &Shared) -> bool {
        let Some(result) = step.result else {
            return step.close_mic;
        };
        let counters = CaptureCounters {
            xruns: shared.xruns.load(Ordering::Relaxed),
            chunks_sent: shared.chunks_sent.load(Ordering::Relaxed),
            chunks_dropped: shared.chunks_dropped.load(Ordering::Relaxed),
        };
        let retry_in = step.retry_at.map(|t| t.saturating_sub(self.now()));
        let summary = result.summary_text();
        match result.overall() {
            Overall::Passed => log::info!("self-check: {summary}"),
            Overall::Warning => log::warn!("self-check: {summary}; the microphone stays open"),
            Overall::Failed => {
                let next = if self.busy {
                    "retrying while the microphone is busy (turn LG hands-free voice off)"
                        .to_owned()
                } else if let Some(d) = retry_in {
                    format!("retry in {} s", d.as_secs())
                } else {
                    "stopped retrying; use Re-run microphone check or ctl.sh selfcheck".to_owned()
                };
                log::warn!(
                    "self-check: {summary}; capture problem{}, {next}; press Report compatibility in the TV app if this TV should work",
                    if step.close_mic {
                        ", microphone closed"
                    } else {
                        ""
                    }
                );
            }
        }
        self.note(Note::Result(Box::new(Finished { result, counters })));
        step.close_mic
    }
}
