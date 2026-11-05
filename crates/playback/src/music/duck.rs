//! Duck controller: from voice-side sources to what the music should do.
//!
//! Pure state machine on an abstract monotonic clock (a [`Duration`] since an
//! arbitrary epoch), so it is tested without audio or a runtime. The engine
//! turns its answer into per-sample gain ramps.
//!
//! Sources (design D4 and D5): the wake word (a pipeline run), a reply,
//! an announcement and the alarm chime. Each source is **reference counted**:
//! two overlapping replies, or a run and its reply, keep the duck until the
//! last of them ends. When the count reaches zero the duck is held for
//! [`DuckConfig::hold`] and then released; a source that begins during the
//! hold cancels the release.

use std::time::Duration;

/// What the music does while a source is active (`[music] on_wake`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckMode {
    /// Lower the level by [`DuckConfig::duck_db`] (default).
    Duck,
    /// Pause the music.
    Pause,
    /// Ignore the sources.
    None,
}

/// Why the music is being lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckSource {
    /// A voice pipeline run (wake word detected until the run ends).
    WakeWord,
    /// An assistant reply (TTS).
    Reply,
    /// An announcement.
    Announcement,
    /// The timer alarm chime.
    Alarm,
}

impl DuckSource {
    const COUNT: usize = 4;

    fn index(self) -> usize {
        match self {
            Self::WakeWord => 0,
            Self::Reply => 1,
            Self::Announcement => 2,
            Self::Alarm => 3,
        }
    }
}

/// Duck settings (`[music] duck_*`, `on_wake`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuckConfig {
    /// Behaviour while a source is active.
    pub mode: DuckMode,
    /// Level while ducked, in dB (0 or negative; positive counts as 0).
    pub duck_db: f32,
    /// Time to reach `duck_db` from unity.
    pub attack: Duration,
    /// Time the duck is kept after the last source ended.
    pub hold: Duration,
    /// Time to return to unity from `duck_db`.
    pub release: Duration,
}

impl Default for DuckConfig {
    fn default() -> Self {
        Self {
            mode: DuckMode::Duck,
            duck_db: -20.0,
            attack: Duration::from_millis(50),
            hold: Duration::from_millis(800),
            release: Duration::from_millis(1000),
        }
    }
}

/// What the controller asks of the music right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckLevel {
    /// Full level (or after the release started).
    Unity,
    /// Lowered by `duck_db` (active or holding).
    Ducked,
    /// Paused (active or holding, mode [`DuckMode::Pause`]).
    Paused,
}

/// Reference-counted duck sources with a hold timer.
#[derive(Debug, Clone)]
pub struct DuckController {
    cfg: DuckConfig,
    counts: [u32; DuckSource::COUNT],
    /// End of the hold after the last source ended.
    hold_until: Option<Duration>,
}

impl DuckController {
    /// A controller with nothing active.
    #[must_use]
    pub fn new(cfg: DuckConfig) -> Self {
        Self {
            cfg,
            counts: [0; DuckSource::COUNT],
            hold_until: None,
        }
    }

    /// The settings.
    #[must_use]
    pub fn config(&self) -> &DuckConfig {
        &self.cfg
    }

    /// A source starts (it may start again while it is active: counted).
    pub fn begin(&mut self, source: DuckSource) {
        let c = &mut self.counts[source.index()];
        *c = c.saturating_add(1);
        self.hold_until = None;
    }

    /// A source ends. An end without a matching begin is ignored. When the
    /// last active source ends, the hold starts at `now`.
    pub fn end(&mut self, source: DuckSource, now: Duration) {
        let c = &mut self.counts[source.index()];
        if *c == 0 {
            return;
        }
        *c -= 1;
        if self.active() == 0 {
            self.hold_until = Some(now + self.cfg.hold);
        }
    }

    /// Ends every source at once (for example a lost connection); the hold
    /// still applies.
    pub fn clear(&mut self, now: Duration) {
        if self.active() > 0 {
            self.counts = [0; DuckSource::COUNT];
            self.hold_until = Some(now + self.cfg.hold);
        }
    }

    /// Number of active sources (all kinds).
    #[must_use]
    pub fn active(&self) -> u32 {
        self.counts.iter().sum()
    }

    /// What the music should do at `now`.
    pub fn level(&mut self, now: Duration) -> DuckLevel {
        let holding = match self.hold_until {
            Some(t) if now < t => true,
            Some(_) => {
                self.hold_until = None;
                false
            }
            None => false,
        };
        if self.cfg.mode == DuckMode::None || (self.active() == 0 && !holding) {
            return DuckLevel::Unity;
        }
        match self.cfg.mode {
            DuckMode::Pause => DuckLevel::Paused,
            _ => DuckLevel::Ducked,
        }
    }

    /// When [`Self::level`] will next change by itself (the end of a hold).
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        self.hold_until
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn idle_is_unity() {
        let mut d = DuckController::new(DuckConfig::default());
        assert_eq!(d.level(MS(0)), DuckLevel::Unity);
        assert_eq!(d.next_deadline(), None);
    }

    #[test]
    fn ducks_while_active_then_holds_then_releases() {
        let mut d = DuckController::new(DuckConfig::default());
        d.begin(DuckSource::WakeWord);
        assert_eq!(d.level(MS(10)), DuckLevel::Ducked);
        assert_eq!(
            d.level(MS(60_000)),
            DuckLevel::Ducked,
            "no timeout while active"
        );
        d.end(DuckSource::WakeWord, MS(60_000));
        assert_eq!(d.next_deadline(), Some(MS(60_800)));
        assert_eq!(d.level(MS(60_000)), DuckLevel::Ducked);
        assert_eq!(d.level(MS(60_799)), DuckLevel::Ducked, "holding");
        assert_eq!(d.level(MS(60_800)), DuckLevel::Unity, "release starts");
        assert_eq!(d.next_deadline(), None);
    }

    #[test]
    fn overlapping_sources_do_not_release_early() {
        let mut d = DuckController::new(DuckConfig::default());
        d.begin(DuckSource::WakeWord);
        d.begin(DuckSource::Reply);
        d.end(DuckSource::WakeWord, MS(1000));
        assert_eq!(d.next_deadline(), None, "the reply still holds the duck");
        assert_eq!(d.level(MS(5000)), DuckLevel::Ducked);
        d.end(DuckSource::Reply, MS(5000));
        assert_eq!(d.level(MS(5799)), DuckLevel::Ducked);
        assert_eq!(d.level(MS(5800)), DuckLevel::Unity);
    }

    #[test]
    fn same_source_twice_needs_two_ends() {
        let mut d = DuckController::new(DuckConfig::default());
        d.begin(DuckSource::Announcement);
        d.begin(DuckSource::Announcement);
        d.end(DuckSource::Announcement, MS(0));
        assert_eq!(d.level(MS(10_000)), DuckLevel::Ducked);
        d.end(DuckSource::Announcement, MS(10_000));
        assert_eq!(d.level(MS(10_800)), DuckLevel::Unity);
    }

    #[test]
    fn unmatched_end_is_ignored() {
        let mut d = DuckController::new(DuckConfig::default());
        d.end(DuckSource::Alarm, MS(0));
        assert_eq!(d.level(MS(1)), DuckLevel::Unity);
        assert_eq!(d.next_deadline(), None);
        d.begin(DuckSource::Alarm);
        d.end(DuckSource::Reply, MS(5));
        assert_eq!(d.level(MS(10)), DuckLevel::Ducked);
    }

    #[test]
    fn a_source_during_the_hold_cancels_the_release() {
        let mut d = DuckController::new(DuckConfig::default());
        d.begin(DuckSource::WakeWord);
        d.end(DuckSource::WakeWord, MS(0));
        d.begin(DuckSource::WakeWord); // continue_conversation: next run
        assert_eq!(d.level(MS(900)), DuckLevel::Ducked);
        d.end(DuckSource::WakeWord, MS(1000));
        assert_eq!(d.next_deadline(), Some(MS(1800)));
    }

    #[test]
    fn pause_mode_pauses_and_none_ignores() {
        let mut d = DuckController::new(DuckConfig {
            mode: DuckMode::Pause,
            ..DuckConfig::default()
        });
        d.begin(DuckSource::Reply);
        assert_eq!(d.level(MS(0)), DuckLevel::Paused);
        d.end(DuckSource::Reply, MS(0));
        assert_eq!(d.level(MS(799)), DuckLevel::Paused);
        assert_eq!(d.level(MS(800)), DuckLevel::Unity);

        let mut d = DuckController::new(DuckConfig {
            mode: DuckMode::None,
            ..DuckConfig::default()
        });
        d.begin(DuckSource::WakeWord);
        assert_eq!(d.level(MS(0)), DuckLevel::Unity);
    }

    #[test]
    fn clear_ends_everything_with_a_hold() {
        let mut d = DuckController::new(DuckConfig::default());
        d.begin(DuckSource::WakeWord);
        d.begin(DuckSource::Alarm);
        d.clear(MS(100));
        assert_eq!(d.active(), 0);
        assert_eq!(d.level(MS(899)), DuckLevel::Ducked);
        assert_eq!(d.level(MS(900)), DuckLevel::Unity);
    }
}
