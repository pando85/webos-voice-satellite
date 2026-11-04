//! The self-check engine: window measurement plus the retry/backoff state
//! machine (design sections 3.1 to 3.3).
//!
//! Fed by the **normal** capture stream: the caller reports PCM discovery
//! ([`SelfCheck::on_pcm`]), the open outcome ([`SelfCheck::on_open`]) and a
//! [`FrameStats`] digest per batch of frames ([`SelfCheck::on_frame`]), plus a
//! periodic [`SelfCheck::on_tick`] for the "no frames" case. It never opens a
//! PCM, reads sysfs or debugfs, or stores samples. Time is a [`Duration`]
//! from any monotonic origin chosen by the caller, so tests inject it.
//!
//! Each call returns a [`Step`] telling the caller what to do (publish a
//! result, close the microphone, when to retry). A failed self-check is a
//! **state**, not an exit: after the configured retries the engine holds
//! (`stopped`) until [`SelfCheck::rerun`].

use std::time::Duration;

use crate::check::{CheckId, CheckResult, Grade, Reason};
use crate::window::{FrameStats, Thresholds, WindowAccumulator, WindowSummary, grade_window};

/// Engine configuration. Defaults are `hypothesis` (see [`Thresholds`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Check thresholds and window lengths.
    pub thresholds: Thresholds,
    /// Capture sample rate in Hz (used for the expected frame count).
    pub sample_rate: u32,
    /// Capture sample width in bits (24 for S24): sets full scale.
    pub sample_bits: u32,
    /// Delays before each retry after consecutive failures: 5, 10, 20, 40, 80
    /// seconds, so one first try and five retries; the sixth consecutive
    /// failure stops the engine (design 3.3: "bounded, never a crash loop").
    pub backoff: Vec<Duration>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            thresholds: Thresholds::default(),
            sample_rate: 16_000,
            sample_bits: 24,
            backoff: [5, 10, 20, 40, 80]
                .iter()
                .map(|s| Duration::from_secs(*s))
                .collect(),
        }
    }
}

/// How the daemon's open of the capture PCM ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    /// Opened.
    Opened,
    /// Busy (for example LG hands-free holds the PCM); not "incompatible".
    Busy,
    /// Failed for another reason.
    Failed,
}

/// What the caller should do about opening the microphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenDecision {
    /// Open now (the first open, a due retry, or no check is pending).
    Open,
    /// Wait this long, then ask again.
    Wait(Duration),
    /// Do not open: the engine gave up after the bounded retries and holds
    /// until [`SelfCheck::rerun`].
    Hold,
}

/// Coarse engine state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No run yet or the microphone closed before a run finished.
    NotRun,
    /// Waiting for the next retry.
    Backoff,
    /// Measuring the window of the current open.
    Measuring,
    /// Last run finished without a failure.
    Done,
    /// Retries used up.
    Stopped,
}

/// Combined verdict of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overall {
    /// No warning and no failure.
    Passed,
    /// At least one warning; the microphone stays open.
    Warning,
    /// A measured failure; the microphone is closed.
    Failed,
}

/// Level numbers of the window, for the report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// Window length.
    pub window: Duration,
    /// Frames received.
    pub frames: u64,
    /// Input RMS in dBFS.
    pub rms_dbfs: f64,
    /// Input peak in dBFS.
    pub peak_dbfs: f64,
    /// Clipped samples.
    pub clipped: u64,
}

/// Result of one run: all eight checks (C1 and C8 stay `Skipped` until the
/// caller fills them with [`SelfCheckResult::set_check`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SelfCheckResult {
    checks: [CheckResult; 8],
    /// Levels over the window, when frames arrived.
    pub levels: Option<Levels>,
    /// Consecutive failed attempts including this one (0 after a pass).
    pub failures: u32,
    /// The retries are used up; only a rerun starts again.
    pub stopped: bool,
}

impl SelfCheckResult {
    fn new(checks: [CheckResult; 8]) -> Self {
        Self {
            checks,
            levels: None,
            failures: 0,
            stopped: false,
        }
    }

    /// All checks in order C1 to C8.
    #[must_use]
    pub fn checks(&self) -> &[CheckResult; 8] {
        &self.checks
    }

    /// One check.
    #[must_use]
    pub fn check(&self, id: CheckId) -> CheckResult {
        self.checks[id as usize]
    }

    /// Fills or replaces one check (the daemon sets C1 and C8).
    pub fn set_check(&mut self, r: CheckResult) {
        self.checks[r.id as usize] = r;
    }

    /// Combined verdict. C1 never counts (an unreadable firmware is the
    /// status `unknown`, not a self-check failure).
    #[must_use]
    pub fn overall(&self) -> Overall {
        let graded = || self.checks.iter().filter(|c| c.id != CheckId::C1);
        if graded().any(|c| c.grade == Grade::Fail) {
            Overall::Failed
        } else if graded().any(|c| c.grade == Grade::Warn) {
            Overall::Warning
        } else {
            Overall::Passed
        }
    }

    /// True when the failure is the PCM being busy, which is reported as busy
    /// and not as an incompatible device.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.check(CheckId::C3).reason == Some(Reason::Busy)
    }

    /// Short text for the `selfcheck` text entity (at most 255 characters),
    /// for example `passed (peak -54 dBFS)`, `warning: very quiet`,
    /// `failed: no audio frames (will retry)`.
    #[must_use]
    pub fn summary_text(&self) -> String {
        match self.overall() {
            Overall::Passed => match self.levels {
                Some(l) => format!("passed (peak {:.0} dBFS)", l.peak_dbfs),
                None => "passed".to_string(),
            },
            Overall::Warning => {
                let phrases: Vec<&str> = self
                    .checks
                    .iter()
                    .filter(|c| c.id != CheckId::C1 && c.grade == Grade::Warn)
                    .filter_map(|c| c.reason.map(Reason::phrase))
                    .collect();
                format!("warning: {}", phrases.join(", "))
            }
            Overall::Failed => {
                let why = self
                    .checks
                    .iter()
                    .find(|c| c.grade == Grade::Fail)
                    .and_then(|c| c.reason)
                    .map_or("check failed", Reason::phrase);
                if self.stopped {
                    format!("failed: {why} (stopped; press Re-run microphone check)")
                } else {
                    format!("failed: {why} (will retry)")
                }
            }
        }
    }
}

/// What the caller should do after an event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    /// A finished run to publish (HA states, `run/selfcheck.json`, log).
    pub result: Option<SelfCheckResult>,
    /// Close the capture PCM (a measured failure of C2 to C5; a warning
    /// leaves it open).
    pub close_mic: bool,
    /// Earliest time of the next open attempt (same clock as the events).
    pub retry_at: Option<Duration>,
    /// The retries are used up.
    pub stopped: bool,
}

#[derive(Clone, Debug)]
enum State {
    Idle,
    Backoff { until: Duration },
    Measuring(Run),
    Done,
    Stopped,
}

#[derive(Clone, Debug)]
struct Run {
    begin: Duration,
    acc: WindowAccumulator,
    any_frames: bool,
    c3: CheckResult,
}

/// The engine.
#[derive(Clone, Debug)]
pub struct SelfCheck {
    cfg: Config,
    state: State,
    c2: CheckResult,
    failures: u32,
    last: Option<SelfCheckResult>,
}

impl SelfCheck {
    /// New engine, nothing run yet.
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: State::Idle,
            c2: CheckResult::skipped(CheckId::C2),
            failures: 0,
            last: None,
        }
    }

    /// Coarse state.
    #[must_use]
    pub fn phase(&self) -> Phase {
        match self.state {
            State::Idle => Phase::NotRun,
            State::Backoff { .. } => Phase::Backoff,
            State::Measuring(_) => Phase::Measuring,
            State::Done => Phase::Done,
            State::Stopped => Phase::Stopped,
        }
    }

    /// The last finished run (sticky until the next run finishes).
    #[must_use]
    pub fn last_result(&self) -> Option<&SelfCheckResult> {
        self.last.as_ref()
    }

    /// Text for the `selfcheck` entity in any state.
    #[must_use]
    pub fn summary_text(&self) -> String {
        match (&self.state, &self.last) {
            (State::Measuring(_), _) => "checking".to_string(),
            (_, Some(r)) => r.summary_text(),
            (_, None) => "not run (microphone closed)".to_string(),
        }
    }

    /// Whether and when to open the microphone.
    #[must_use]
    pub fn next_open(&self, now: Duration) -> OpenDecision {
        match self.state {
            State::Backoff { until } if now < until => {
                OpenDecision::Wait(until.saturating_sub(now))
            }
            State::Stopped => OpenDecision::Hold,
            _ => OpenDecision::Open,
        }
    }

    /// C2 from the caller's discovery (see [`crate::pcm::check_pcm`]). A FAIL
    /// counts as a failed attempt (bounded retries) when a run is pending.
    pub fn on_pcm(&mut self, now: Duration, c2: CheckResult) -> Step {
        self.c2 = c2;
        if c2.grade == Grade::Fail && self.attempt_pending() {
            let mut checks = self.base_checks();
            checks[CheckId::C3 as usize] = CheckResult::not_measured(CheckId::C3);
            return self.fail_attempt(now, checks, None);
        }
        Step::default()
    }

    /// The outcome of the caller's open.
    pub fn on_open(&mut self, now: Duration, outcome: OpenOutcome) -> Step {
        if !self.attempt_pending() {
            return Step::default();
        }
        match outcome {
            OpenOutcome::Opened => {
                let c3 = if self.failures == 0 {
                    CheckResult::pass(CheckId::C3)
                } else {
                    CheckResult::warn(CheckId::C3, Reason::OpenRetried)
                };
                self.state = State::Measuring(Run::new(now, c3));
                Step::default()
            }
            OpenOutcome::Busy | OpenOutcome::Failed => {
                let reason = if outcome == OpenOutcome::Busy {
                    Reason::Busy
                } else {
                    Reason::OpenFailed
                };
                let mut checks = self.base_checks();
                checks[CheckId::C3 as usize] = CheckResult::fail(CheckId::C3, reason);
                self.fail_attempt(now, checks, None)
            }
        }
    }

    /// A digest of the frames just delivered by the normal capture path.
    pub fn on_frame(&mut self, now: Duration, stats: &FrameStats) -> Step {
        let th = &self.cfg.thresholds;
        let (start, end) = match &self.state {
            State::Measuring(run) => (run.begin + th.discard, run.begin + th.discard + th.window),
            _ => return Step::default(),
        };
        if now >= end {
            return self.finish_window(now);
        }
        if let State::Measuring(run) = &mut self.state {
            run.any_frames |= stats.frames > 0;
            if now >= start {
                run.acc.add(stats);
            }
        }
        self.on_tick(now)
    }

    /// Time passing without frames. Call it at least once a second while
    /// measuring (the capture loop already has a periodic wake-up).
    pub fn on_tick(&mut self, now: Duration) -> Step {
        let th = &self.cfg.thresholds;
        let (no_frames, end) = match &self.state {
            State::Measuring(run) => (
                !run.any_frames && now >= run.begin + th.no_frames_fail,
                run.begin + th.discard + th.window,
            ),
            _ => return Step::default(),
        };
        if no_frames || now >= end {
            self.finish_window(now)
        } else {
            Step::default()
        }
    }

    /// The caller closed the microphone (mute, kill switch, not paired). A
    /// run in progress is abandoned: result `not run`, it runs at the next
    /// open.
    pub fn on_mic_closed(&mut self) {
        if matches!(self.state, State::Measuring(_)) {
            self.state = State::Idle;
        }
    }

    /// Restart from scratch (HA button, `ctl.sh selfcheck`, restart, firmware
    /// changed). With `mic_open` the measurement starts at once on the open
    /// stream (no second open); otherwise at the next open. The retry counter
    /// is cleared.
    pub fn rerun(&mut self, now: Duration, mic_open: bool) {
        self.failures = 0;
        self.state = if mic_open {
            State::Measuring(Run::new(now, CheckResult::pass(CheckId::C3)))
        } else {
            State::Idle
        };
    }

    fn attempt_pending(&self) -> bool {
        matches!(self.state, State::Idle | State::Backoff { .. })
    }

    fn base_checks(&self) -> [CheckResult; 8] {
        let mut checks = CheckId::ALL.map(CheckResult::skipped);
        checks[CheckId::C2 as usize] = self.c2;
        checks
    }

    fn finish_window(&mut self, now: Duration) -> Step {
        let State::Measuring(run) = &self.state else {
            return Step::default();
        };
        let th = &self.cfg.thresholds;
        // An early "no frames" verdict covers the time so far; a full window
        // covers exactly the window, however late the caller noticed.
        let elapsed = if run.any_frames || now >= run.begin + th.discard + th.window {
            th.window
        } else {
            now.saturating_sub(run.begin + th.discard)
        };
        let summary = run
            .acc
            .summarize(elapsed, self.cfg.sample_rate, self.cfg.sample_bits);
        let c3 = run.c3;
        let mut checks = self.base_checks();
        checks[CheckId::C3 as usize] = c3;
        for r in grade_window(th, &summary) {
            checks[r.id as usize] = r;
        }
        let levels = (summary.frames > 0).then(|| levels_of(&summary, th.window));
        let mut result = SelfCheckResult::new(checks);
        result.levels = levels;
        if result.overall() == Overall::Failed {
            let fail_checks = result.checks;
            return self.fail_attempt(now, fail_checks, levels);
        }
        self.failures = 0;
        self.state = State::Done;
        self.last = Some(result.clone());
        Step {
            result: Some(result),
            ..Step::default()
        }
    }

    fn fail_attempt(
        &mut self,
        now: Duration,
        checks: [CheckResult; 8],
        levels: Option<Levels>,
    ) -> Step {
        self.failures += 1;
        let retry = usize::try_from(self.failures - 1)
            .ok()
            .and_then(|i| self.cfg.backoff.get(i).copied());
        let stopped = retry.is_none();
        let retry_at = retry.map(|d| now + d);
        self.state = match retry_at {
            Some(until) => State::Backoff { until },
            None => State::Stopped,
        };
        let mut result = SelfCheckResult::new(checks);
        result.levels = levels;
        result.failures = self.failures;
        result.stopped = stopped;
        self.last = Some(result.clone());
        Step {
            result: Some(result),
            close_mic: true,
            retry_at,
            stopped,
        }
    }
}

impl Run {
    fn new(begin: Duration, c3: CheckResult) -> Self {
        Self {
            begin,
            acc: WindowAccumulator::default(),
            any_frames: false,
            c3,
        }
    }
}

fn levels_of(w: &WindowSummary, window: Duration) -> Levels {
    Levels {
        window,
        frames: w.frames,
        rms_dbfs: w.rms_dbfs,
        peak_dbfs: w.peak_dbfs,
        clipped: w.clipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;
    const S: fn(u64) -> Duration = Duration::from_secs;

    /// 10 ms of stereo noise at 16 kHz: 160 frames, amplitude `amp` LSB.
    fn noise(amp: i32, seed: &mut u32) -> FrameStats {
        let mut v = Vec::with_capacity(320);
        for _ in 0..320 {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let r = i32::try_from(*seed >> 8).unwrap_or(0) % (2 * amp + 1) - amp;
            v.push(r);
        }
        FrameStats::from_samples(&v, 2, 24)
    }

    fn constant(value: i32) -> FrameStats {
        FrameStats::from_samples(&vec![value; 320], 2, 24)
    }

    /// Feeds one batch every 10 ms from `from` (inclusive) to `to` (exclusive)
    /// and returns the first step that carries a result.
    fn feed(
        e: &mut SelfCheck,
        from: Duration,
        to: Duration,
        mut make: impl FnMut() -> FrameStats,
    ) -> Option<Step> {
        let mut t = from;
        while t < to {
            let step = e.on_frame(t, &make());
            if step.result.is_some() {
                return Some(step);
            }
            t += MS(10);
        }
        None
    }

    fn started(now: Duration) -> SelfCheck {
        let mut e = SelfCheck::new(Config::default());
        assert_eq!(
            e.on_pcm(now, CheckResult::pass(CheckId::C2)),
            Step::default()
        );
        assert_eq!(e.on_open(now, OpenOutcome::Opened), Step::default());
        assert_eq!(e.phase(), Phase::Measuring);
        e
    }

    #[test]
    fn quiet_room_passes_and_keeps_the_mic_open() {
        let mut e = started(S(100));
        let mut seed = 1;
        // Amplitude 16000 LSB is about -54 dBFS peak at 24 bit (c3-a: -53.6).
        let step = feed(&mut e, S(100), S(112), || noise(16_000, &mut seed))
            .expect("window finishes by 10.25 s");
        let r = step.result.expect("result");
        assert!(!step.close_mic && step.retry_at.is_none() && !step.stopped);
        assert_eq!(r.overall(), Overall::Passed, "{:?}", r.checks());
        assert_eq!(r.check(CheckId::C3).grade, Grade::Pass);
        assert!(
            r.summary_text().starts_with("passed (peak -5"),
            "{}",
            r.summary_text()
        );
        let l = r.levels.expect("levels");
        assert!((l.peak_dbfs + 54.0).abs() < 1.0, "{l:?}");
        assert_eq!(e.phase(), Phase::Done);
        assert_eq!(e.last_result(), Some(&r));
        // Later frames are ignored, a close and reopen is not re-checked.
        assert_eq!(e.on_frame(S(120), &noise(10, &mut seed)), Step::default());
        assert_eq!(e.next_open(S(121)), OpenDecision::Open);
        assert_eq!(e.on_open(S(121), OpenOutcome::Opened), Step::default());
        assert_eq!(e.phase(), Phase::Done);
    }

    #[test]
    fn silent_input_warns_and_stays_open() {
        let mut e = started(S(0));
        let mut seed = 7;
        // Amplitude 2 LSB: moving signal (not stuck) but far below -80 dBFS.
        let step = feed(&mut e, S(0), S(12), || noise(2, &mut seed)).expect("result");
        let r = step.result.expect("result");
        assert_eq!(r.overall(), Overall::Warning);
        assert_eq!(r.check(CheckId::C6).reason, Some(Reason::Quiet));
        assert_eq!(r.check(CheckId::C5).grade, Grade::Pass);
        assert!(!step.close_mic);
        assert_eq!(r.summary_text(), "warning: very quiet");
        assert_eq!(e.phase(), Phase::Done);
    }

    #[test]
    fn constant_input_fails_closes_and_schedules_a_retry() {
        let mut e = started(S(50));
        let step = feed(&mut e, S(50), S(62), || constant(1234)).expect("result");
        let r = step.result.expect("result");
        assert_eq!(r.overall(), Overall::Failed);
        assert_eq!(r.check(CheckId::C5).reason, Some(Reason::Stuck));
        assert!(step.close_mic);
        // Detected at the first batch past 50 s + 250 ms + 10 s.
        assert_eq!(step.retry_at, Some(S(60) + MS(250) + S(5)));
        assert!(!step.stopped);
        assert_eq!(r.failures, 1);
        assert_eq!(r.summary_text(), "failed: signal stuck (will retry)");
        assert_eq!(e.phase(), Phase::Backoff);
        assert!(matches!(e.next_open(S(61)), OpenDecision::Wait(_)));
    }

    #[test]
    fn no_frames_fails_after_three_seconds() {
        let mut e = started(S(10));
        assert_eq!(e.on_tick(S(12)), Step::default());
        let step = e.on_tick(S(13));
        let r = step.result.expect("result");
        assert_eq!(r.check(CheckId::C4).reason, Some(Reason::NoFrames));
        assert_eq!(r.check(CheckId::C5).grade, Grade::Skipped);
        assert!(r.levels.is_none());
        assert!(step.close_mic);
        assert_eq!(r.summary_text(), "failed: no audio frames (will retry)");
    }

    #[test]
    fn a_stall_after_some_frames_still_ends_the_window() {
        let mut e = started(S(0));
        let mut seed = 3;
        assert!(feed(&mut e, S(0), S(2), || noise(16_000, &mut seed)).is_none());
        // No more frames; ticks alone finish the window with a low ratio.
        assert_eq!(e.on_tick(S(10)), Step::default());
        let step = e.on_tick(S(11));
        let r = step.result.expect("result");
        assert_eq!(r.check(CheckId::C4).reason, Some(Reason::FramesLow));
        assert_eq!(r.overall(), Overall::Warning);
        assert!(!step.close_mic);
    }

    #[test]
    fn startup_burst_in_the_discard_period_is_ignored() {
        let mut e = started(S(0));
        let mut seed = 5;
        // Full-scale burst for the first 240 ms, then a quiet room.
        let mut t = S(0);
        let mut result = None;
        while t < S(12) {
            let st = if t < MS(240) {
                constant(8_388_607)
            } else {
                noise(16_000, &mut seed)
            };
            let step = e.on_frame(t, &st);
            if step.result.is_some() {
                result = step.result;
                break;
            }
            t += MS(10);
        }
        let r = result.expect("result");
        assert_eq!(r.overall(), Overall::Passed, "{:?}", r.checks());
        assert_eq!(r.check(CheckId::C7).grade, Grade::Pass);
    }

    #[test]
    fn open_failures_back_off_five_times_then_stop() {
        let mut e = SelfCheck::new(Config::default());
        let mut now = S(0);
        let mut delays = Vec::new();
        for attempt in 1..=6 {
            assert_eq!(e.next_open(now), OpenDecision::Open, "attempt {attempt}");
            let step = e.on_open(now, OpenOutcome::Busy);
            let r = step.result.expect("result");
            assert!(step.close_mic);
            assert_eq!(r.failures, attempt);
            assert_eq!(r.check(CheckId::C3).reason, Some(Reason::Busy));
            assert!(r.busy());
            match step.retry_at {
                Some(at) => {
                    delays.push(at.saturating_sub(now));
                    assert!(!step.stopped);
                    assert_eq!(e.next_open(now), OpenDecision::Wait(at.saturating_sub(now)));
                    now = at;
                }
                None => assert!(step.stopped && attempt == 6),
            }
        }
        assert_eq!(delays, vec![S(5), S(10), S(20), S(40), S(80)]);
        assert_eq!(e.phase(), Phase::Stopped);
        // No crash loop: it holds forever, whatever the time.
        assert_eq!(e.next_open(now + S(100_000)), OpenDecision::Hold);
        assert!(e.on_open(now, OpenOutcome::Failed).result.is_none());
        let last = e.last_result().expect("sticky result");
        assert!(last.stopped);
        assert!(last.summary_text().contains("stopped"));
        assert!(last.summary_text().contains("busy"));
    }

    #[test]
    fn rerun_clears_the_retry_counter() {
        let mut e = SelfCheck::new(Config::default());
        let mut now = S(0);
        for _ in 0..6 {
            if let Some(at) = e.on_open(now, OpenOutcome::Failed).retry_at {
                now = at;
            }
        }
        assert_eq!(e.phase(), Phase::Stopped);
        e.rerun(now, false);
        assert_eq!(e.next_open(now), OpenDecision::Open);
        let step = e.on_open(now, OpenOutcome::Failed);
        assert_eq!(step.retry_at, Some(now + S(5)), "counter restarted");
        assert_eq!(step.result.expect("result").failures, 1);
    }

    #[test]
    fn opened_after_retries_is_a_warning_and_resets_the_counter() {
        let mut e = SelfCheck::new(Config::default());
        let step = e.on_open(S(0), OpenOutcome::Failed);
        let at = step.retry_at.expect("retry");
        assert_eq!(e.on_open(at, OpenOutcome::Opened), Step::default());
        let mut seed = 9;
        let step = feed(&mut e, at, at + S(12), || noise(16_000, &mut seed)).expect("result");
        let r = step.result.expect("result");
        assert_eq!(r.check(CheckId::C3).reason, Some(Reason::OpenRetried));
        assert_eq!(r.overall(), Overall::Warning);
        assert_eq!(r.failures, 0);
        // The next failure starts at the first backoff again.
        e.rerun(at + S(20), false);
        assert_eq!(
            e.on_open(at + S(20), OpenOutcome::Failed).retry_at,
            Some(at + S(25))
        );
    }

    #[test]
    fn missing_capture_device_fails_through_the_same_backoff() {
        use crate::pcm::{PcmDiscovery, WantedPcm, check_pcm};
        let mut e = SelfCheck::new(Config::default());
        let d = PcmDiscovery::parse("", "");
        let w = WantedPcm {
            card_id: "LGSIC".into(),
            device: None,
        };
        let c2 = check_pcm(&d, &w, crate::pcm::CapsFit::Unknown);
        let step = e.on_pcm(S(0), c2);
        let r = step.result.expect("result");
        assert_eq!(r.check(CheckId::C2).reason, Some(Reason::NoCard));
        assert_eq!(r.check(CheckId::C3).grade, Grade::Skipped);
        assert_eq!(step.retry_at, Some(S(5)));
        assert_eq!(r.summary_text(), "failed: no capture device (will retry)");
    }

    #[test]
    fn mic_closed_midway_abandons_the_run_without_a_result() {
        let mut e = started(S(0));
        let mut seed = 2;
        assert!(feed(&mut e, S(0), S(3), || noise(16_000, &mut seed)).is_none());
        e.on_mic_closed();
        assert_eq!(e.phase(), Phase::NotRun);
        assert_eq!(e.summary_text(), "not run (microphone closed)");
        assert_eq!(e.next_open(S(4)), OpenDecision::Open);
        assert_eq!(e.on_frame(S(5), &noise(1, &mut seed)), Step::default());
    }

    #[test]
    fn rerun_on_an_open_mic_measures_in_place() {
        let mut e = started(S(0));
        let mut seed = 4;
        feed(&mut e, S(0), S(12), || noise(16_000, &mut seed)).expect("first run");
        assert_eq!(e.phase(), Phase::Done);
        e.rerun(S(100), true);
        assert_eq!(e.phase(), Phase::Measuring);
        assert_eq!(e.summary_text(), "checking");
        let step = feed(&mut e, S(100), S(112), || constant(5)).expect("second run");
        assert_eq!(step.result.expect("result").overall(), Overall::Failed);
    }

    #[test]
    fn summary_text_fits_an_ha_state() {
        let mut e = SelfCheck::new(Config::default());
        assert_eq!(e.summary_text(), "not run (microphone closed)");
        let step = e.on_open(S(0), OpenOutcome::Failed);
        let text = step.result.expect("result").summary_text();
        assert!(text.len() <= 255);
    }

    #[test]
    fn caller_can_fill_c1_and_c8_without_changing_the_overall_for_c1() {
        let mut r = SelfCheckResult::new(CheckId::ALL.map(CheckResult::skipped));
        assert_eq!(r.overall(), Overall::Passed);
        r.set_check(crate::check::check_c1(false, false));
        assert_eq!(r.overall(), Overall::Passed, "C1 never counts");
        r.set_check(crate::check::check_c8(true, false, true));
        assert_eq!(r.overall(), Overall::Warning);
        assert_eq!(r.check(CheckId::C8).reason, Some(Reason::ReferenceSilent));
    }
}
