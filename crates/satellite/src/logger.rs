//! Minimal stderr logger with UTC timestamps.
//!
//! Policy (enforced by the call sites, not here): never log audio content,
//! Noise keys or URLs with tokens; transcripts and reply text only at debug.

use std::io::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

struct StderrLogger {
    level: log::LevelFilter,
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian
/// (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (y, m as u32, d as u32)
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for a duration since the Unix epoch.
pub(crate) fn format_utc(since_epoch: std::time::Duration) -> String {
    let secs = i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let tod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        tod / 3600,
        (tod / 60) % 60,
        tod % 60,
        since_epoch.subsec_millis()
    )
}

impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= self.level
    }

    fn log(&self, r: &log::Record<'_>) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{} {:<5} {}", format_utc(now), r.level(), r.args());
    }

    fn flush(&self) {}
}

/// Installs the logger. Calling it twice keeps the first logger.
pub(crate) fn init(level: log::LevelFilter) {
    if log::set_logger(Box::leak(Box::new(StderrLogger { level }))).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc(Duration::ZERO), "1970-01-01T00:00:00.000Z");
        // 2026-11-02T10:20:30.456Z
        assert_eq!(
            format_utc(Duration::from_millis(1_793_614_830_456)),
            "2026-11-02T10:20:30.456Z"
        );
        // Leap day.
        assert_eq!(
            format_utc(Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00.000Z"
        );
    }
}
