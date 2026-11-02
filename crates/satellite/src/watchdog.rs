//! Internal watchdog thread.
//!
//! - Event loop: the daemon bumps a heartbeat on every kill-switch poll
//!   (250 ms). No progress for `limits.loop_stall_s` → exit code 70.
//! - Capture thread: bumps its heartbeat on every loop turn (at least every
//!   `limits.capture_stall_s`, the ALSA read timeout, and every 50 ms while
//!   idle). No progress for `limits.loop_stall_s` → exit code 71. A stalled
//!   ALSA device on its own is handled inside the capture thread (reopen).
//! - Memory: resident size above `limits.max_rss_mb` → exit code 72.
//!
//! Exiting lets a supervisor (`packaging/webos/supervise.sh`) restart the
//! daemon; the kernel closes the PCM and sockets, and `pacat` sees EOF.
//!
//! When the watchdog itself did not run for a while (the process was stopped
//! with SIGSTOP, or the system was suspended), the baselines are reset so a
//! resume is not mistaken for a hang.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Watchdog check interval.
const TICK: Duration = Duration::from_secs(1);
/// A tick later than this means the process or system was suspended.
const SUSPEND_GAP: Duration = Duration::from_secs(5);
/// Resident size is checked this often.
const RSS_EVERY: Duration = Duration::from_secs(10);
/// The resident size is logged at info each time it grows this much beyond
/// the last logged value (and at debug at every check), so a slow rise shows
/// up in the log without a profiler (see docs/discovery/c3-a/P3-robustness.md).
const RSS_REPORT_STEP: u64 = 2 << 20;

/// Whether `rss` is far enough above the last reported size to log.
fn rss_report_due(last_reported: u64, rss: u64) -> bool {
    rss >= last_reported.saturating_add(RSS_REPORT_STEP)
}

/// Exit code: the event loop stalled.
pub(crate) const EXIT_LOOP_STALL: i32 = 70;
/// Exit code: the capture thread stalled.
pub(crate) const EXIT_CAPTURE_STALL: i32 = 71;
/// Exit code: the resident memory ceiling was exceeded.
pub(crate) const EXIT_RSS: i32 = 72;

/// Heartbeats and switches shared with the watchdog thread.
#[derive(Debug, Default)]
pub(crate) struct Beats {
    /// Bumped by the event loop.
    pub(crate) event_loop: AtomicU64,
    /// Set during shutdown: stop watching.
    pub(crate) disarmed: AtomicBool,
}

/// Why the watchdog fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trip {
    /// No event loop progress.
    Loop,
    /// No capture thread progress.
    Capture,
}

/// Pure stall detector (unit-tested without threads).
#[derive(Debug)]
pub(crate) struct Monitor {
    limit: Duration,
    last_tick: Instant,
    loop_seen: (u64, Instant),
    cap_seen: (u64, Instant),
}

impl Monitor {
    pub(crate) fn new(limit: Duration, now: Instant) -> Self {
        Self {
            limit,
            last_tick: now,
            loop_seen: (0, now),
            cap_seen: (0, now),
        }
    }

    /// Feeds one observation. Returns the trip, if any, and whether a
    /// suspend was detected (baselines reset).
    pub(crate) fn observe(&mut self, now: Instant, lp: u64, cap: u64) -> (Option<Trip>, bool) {
        let gap = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        if gap > SUSPEND_GAP {
            self.loop_seen = (lp, now);
            self.cap_seen = (cap, now);
            return (None, true);
        }
        for (seen, v) in [(&mut self.loop_seen, lp), (&mut self.cap_seen, cap)] {
            if v != seen.0 {
                *seen = (v, now);
            }
        }
        let stale = |s: (u64, Instant)| now.saturating_duration_since(s.1) > self.limit;
        if stale(self.loop_seen) {
            (Some(Trip::Loop), false)
        } else if stale(self.cap_seen) {
            (Some(Trip::Capture), false)
        } else {
            (None, false)
        }
    }
}

/// Starts the watchdog thread. `loop_stall` of zero disables the stall
/// checks, `max_rss` of zero the memory check; with both off no thread runs.
pub(crate) fn spawn(
    loop_stall: Duration,
    max_rss: u64,
    beats: Arc<Beats>,
    capture_beat: impl Fn() -> u64 + Send + 'static,
) -> std::io::Result<()> {
    if loop_stall.is_zero() && max_rss == 0 {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(move || run(loop_stall, max_rss, &beats, &capture_beat))
        .map(|_| ())
}

fn run(loop_stall: Duration, max_rss: u64, beats: &Beats, capture_beat: &dyn Fn() -> u64) {
    let start = Instant::now();
    let mut mon = Monitor::new(loop_stall, start);
    let mut rss_at = start;
    let mut rss_reported = 0_u64;
    loop {
        std::thread::sleep(TICK);
        if beats.disarmed.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        if !loop_stall.is_zero() {
            let (trip, suspended) = mon.observe(
                now,
                beats.event_loop.load(Ordering::Relaxed),
                capture_beat(),
            );
            if suspended {
                log::info!(
                    "watchdog: the process did not run for a while (stopped or suspended); resetting"
                );
            }
            match trip {
                Some(Trip::Loop) => {
                    log::error!(
                        "watchdog: event loop made no progress for {} s; exiting with {EXIT_LOOP_STALL} for a restart",
                        loop_stall.as_secs()
                    );
                    std::process::exit(EXIT_LOOP_STALL);
                }
                Some(Trip::Capture) => {
                    log::error!(
                        "watchdog: capture thread made no progress for {} s; exiting with {EXIT_CAPTURE_STALL} for a restart",
                        loop_stall.as_secs()
                    );
                    std::process::exit(EXIT_CAPTURE_STALL);
                }
                None => {}
            }
        }
        if max_rss > 0 && now.saturating_duration_since(rss_at) >= RSS_EVERY {
            rss_at = now;
            if let Some(rss) = crate::limits::rss_bytes() {
                if rss > max_rss {
                    log::error!(
                        "watchdog: resident memory {} KiB is above the ceiling of {} KiB; exiting with {EXIT_RSS} for a restart",
                        rss / 1024,
                        max_rss / 1024
                    );
                    std::process::exit(EXIT_RSS);
                }
                log::debug!("memory: resident {} KiB", rss / 1024);
                if rss_report_due(rss_reported, rss) {
                    log::info!(
                        "memory: resident size {} KiB (last reported {} KiB)",
                        rss / 1024,
                        rss_reported / 1024
                    );
                    rss_reported = rss;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_is_reported_in_steps() {
        assert!(rss_report_due(0, 3 << 20));
        assert!(!rss_report_due(0, 1 << 20));
        assert!(!rss_report_due(3 << 20, 4 << 20));
        assert!(rss_report_due(3 << 20, 5 << 20));
    }

    #[test]
    fn progress_keeps_it_quiet_and_a_stall_trips() {
        let t0 = Instant::now();
        let s = |n| t0 + Duration::from_secs(n);
        let mut m = Monitor::new(Duration::from_secs(10), t0);
        for i in 1..=20 {
            assert_eq!(m.observe(s(i), i, i), (None, false));
        }
        // The loop stops; the capture thread keeps going.
        for i in 21..=30 {
            assert_eq!(m.observe(s(i), 20, i), (None, false));
        }
        assert_eq!(m.observe(s(31), 20, 31), (Some(Trip::Loop), false));
    }

    #[test]
    fn capture_stall_trips() {
        let t0 = Instant::now();
        let s = |n| t0 + Duration::from_secs(n);
        let mut m = Monitor::new(Duration::from_secs(5), t0);
        assert_eq!(m.observe(s(1), 1, 7), (None, false));
        for i in 2..=6 {
            assert_eq!(m.observe(s(i), i, 7), (None, false));
        }
        assert_eq!(m.observe(s(7), 7, 7), (Some(Trip::Capture), false));
    }

    #[test]
    fn a_suspend_resets_the_baselines() {
        let t0 = Instant::now();
        let s = |n| t0 + Duration::from_secs(n);
        let mut m = Monitor::new(Duration::from_secs(10), t0);
        assert_eq!(m.observe(s(1), 1, 1), (None, false));
        // SIGSTOP for 60 s: the watchdog itself did not run.
        assert_eq!(m.observe(s(61), 1, 1), (None, true));
        // The loop gets 10 s to show progress again.
        assert_eq!(m.observe(s(62), 1, 1), (None, false));
        assert_eq!(m.observe(s(63), 2, 2), (None, false));
        for i in 64..=73 {
            assert_eq!(m.observe(s(i), i, i), (None, false));
        }
    }
}
