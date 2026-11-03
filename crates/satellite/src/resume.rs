//! Detects a system suspend and resume (always-on gap G2).
//!
//! On Linux `CLOCK_MONOTONIC` (Rust's `Instant`, tokio's timers) stops while
//! the kernel is suspended and `CLOCK_BOOTTIME` does not, so their difference
//! grows by the time spent suspended. A jump of more than [`MIN_GAP`] between
//! two checks is a resume. This is independent of the TV's power-state
//! service and also sees a suspend the subscription missed (the process is
//! frozen before it could read the last lines).
//!
//! Because the monotonic clock stops, a suspend never looks like a hang to
//! the watchdog or to the API keepalive: those count monotonic time only.

use std::time::Duration;

/// A smaller jump is scheduling noise, not a suspend.
pub(crate) const MIN_GAP: Duration = Duration::from_secs(2);

/// `CLOCK_BOOTTIME - CLOCK_MONOTONIC`: the total time spent suspended since
/// boot. `None` where the clocks are not available.
pub(crate) fn suspended_total() -> Option<Duration> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use rustix::time::{ClockId, clock_gettime};
        fn read(id: ClockId) -> Option<Duration> {
            let t = clock_gettime(id);
            Some(Duration::new(
                u64::try_from(t.tv_sec).ok()?,
                u32::try_from(t.tv_nsec).ok()?,
            ))
        }
        Some(read(ClockId::Boottime)?.saturating_sub(read(ClockId::Monotonic)?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        None
    }
}

/// Turns successive [`suspended_total`] readings into resume events.
#[derive(Debug, Default)]
pub(crate) struct Detector {
    last: Option<Duration>,
}

impl Detector {
    /// Feeds one reading; returns the suspended time if a resume happened
    /// since the previous one.
    pub(crate) fn observe(&mut self, total: Option<Duration>) -> Option<Duration> {
        let total = total?;
        let prev = self.last.replace(total)?;
        let gap = total.saturating_sub(prev);
        (gap >= MIN_GAP).then_some(gap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jump_is_a_resume_and_noise_is_not() {
        let s = Duration::from_secs;
        let mut d = Detector::default();
        assert_eq!(d.observe(Some(s(100))), None, "first reading");
        assert_eq!(d.observe(Some(s(100) + Duration::from_millis(5))), None);
        assert_eq!(d.observe(Some(s(147))), Some(Duration::from_millis(46_995)));
        assert_eq!(d.observe(Some(s(147))), None);
        assert_eq!(d.observe(None), None);
        assert_eq!(d.observe(Some(s(150))), Some(s(3)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clocks_are_readable_and_consistent() {
        let a = suspended_total().unwrap_or_default();
        let b = suspended_total().unwrap_or_default();
        assert!(suspended_total().is_some());
        // Without a suspend between the two reads the difference stays tiny.
        assert!(b.saturating_sub(a) < MIN_GAP);
    }
}
