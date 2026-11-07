//! The in-binary parts of the dead-man: the hard cap (a deadline the loop
//! honours, plus `alarm()` as a backstop that kills the process) and the loop
//! watchdog (a thread that exits the process when the loop stops beating).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Longest run any mode accepts, in seconds (EXP-026: 60 s per step).
pub(crate) const MAX_SECS: u64 = 60;

/// Watchdog limit: a loop silent for longer than this is a stall.
pub(crate) const STALL_LIMIT: Duration = Duration::from_millis(1000);

/// Watchdog check period.
pub(crate) const CHECK_EVERY: Duration = Duration::from_millis(250);

/// Validates a requested duration (1 to [`MAX_SECS`]).
pub(crate) fn cap_secs(requested: u64) -> Result<u64, String> {
    if requested == 0 || requested > MAX_SECS {
        Err(format!("--secs must be 1 to {MAX_SECS}, got {requested}"))
    } else {
        Ok(requested)
    }
}

/// The run's deadline.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cap {
    deadline: Instant,
}

impl Cap {
    pub(crate) fn new(start: Instant, secs: u64) -> Self {
        Self {
            deadline: start + Duration::from_secs(secs),
        }
    }

    pub(crate) fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    /// Seconds for the `alarm()` backstop: the cap plus 2 s, so the graceful
    /// end (key releases, ungrab) normally comes first.
    pub(crate) fn alarm_secs(secs: u64) -> u32 {
        u32::try_from(secs + 2).unwrap_or(u32::MAX)
    }
}

/// Last loop turn, in ms since `base`.
#[derive(Debug)]
pub(crate) struct Heartbeat {
    base: Instant,
    last_ms: AtomicU64,
}

impl Heartbeat {
    pub(crate) fn new(base: Instant) -> Arc<Self> {
        Arc::new(Self {
            base,
            last_ms: AtomicU64::new(0),
        })
    }

    fn ms(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.base).as_millis()).unwrap_or(u64::MAX)
    }

    /// Called on every loop turn.
    pub(crate) fn beat(&self) {
        self.last_ms
            .store(self.ms(Instant::now()), Ordering::SeqCst);
    }

    /// True if the last beat is older than `limit` at `now`.
    pub(crate) fn stalled(&self, now: Instant, limit: Duration) -> bool {
        let last = self.last_ms.load(Ordering::SeqCst);
        let limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
        self.ms(now).saturating_sub(last) > limit_ms
    }
}

/// Starts the watchdog thread. `on_stall` runs once when the heartbeat is
/// older than `limit`; in the binary it releases keys and exits the process.
/// An error means no watchdog: the caller must not start the run.
pub(crate) fn spawn_watchdog<F>(
    hb: Arc<Heartbeat>,
    limit: Duration,
    every: Duration,
    on_stall: F,
) -> std::io::Result<()>
where
    F: FnOnce(Duration) + Send + 'static,
{
    std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(move || {
            loop {
                std::thread::sleep(every);
                let now = Instant::now();
                if hb.stalled(now, limit) {
                    let last = hb.last_ms.load(Ordering::SeqCst);
                    let age = Duration::from_millis(hb.ms(now).saturating_sub(last));
                    on_stall(age);
                    return;
                }
            }
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn cap_accepts_1_to_60_seconds_only() {
        assert!(cap_secs(0).is_err());
        assert_eq!(cap_secs(1), Ok(1));
        assert_eq!(cap_secs(60), Ok(60));
        assert!(cap_secs(61).is_err());
        assert!(cap_secs(3600).is_err());
    }

    #[test]
    fn cap_expires_at_the_deadline_and_the_alarm_comes_later() {
        let t0 = Instant::now();
        let cap = Cap::new(t0, 60);
        assert!(!cap.expired(t0 + Duration::from_secs(59)));
        assert!(cap.expired(t0 + Duration::from_secs(60)));
        assert_eq!(Cap::alarm_secs(60), 62);
    }

    #[test]
    fn a_beating_loop_is_not_stalled_a_silent_one_is() {
        let t0 = Instant::now();
        let hb = Heartbeat::new(t0);
        hb.last_ms.store(500, Ordering::SeqCst);
        assert!(!hb.stalled(t0 + Duration::from_millis(1400), STALL_LIMIT));
        assert!(hb.stalled(t0 + Duration::from_millis(1600), STALL_LIMIT));
    }

    #[test]
    fn the_watchdog_fires_on_a_stall_and_not_while_the_loop_beats() {
        let hb = Heartbeat::new(Instant::now());
        hb.beat();
        let fired = Arc::new(AtomicBool::new(false));
        let f2 = Arc::clone(&fired);
        spawn_watchdog(
            Arc::clone(&hb),
            Duration::from_millis(150),
            Duration::from_millis(20),
            move |_| f2.store(true, Ordering::SeqCst),
        )
        .unwrap_or_else(|e| panic!("watchdog: {e}"));
        // Beat for 400 ms: no stall.
        let until = Instant::now() + Duration::from_millis(400);
        while Instant::now() < until {
            hb.beat();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!fired.load(Ordering::SeqCst));
        // Stop beating: it must fire within the limit plus a few checks.
        std::thread::sleep(Duration::from_millis(400));
        assert!(fired.load(Ordering::SeqCst));
    }
}
