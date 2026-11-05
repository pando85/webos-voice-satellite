//! The setup log (section 6.6): events and the peer address only. Callers
//! build every line from fixed words, validated values and IP literals; no
//! request bytes, token, cookie or CSRF token reach this module.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Bytes written per run at most (a flood cannot fill the flash).
const CAP: u64 = 256 * 1024;

pub(crate) struct Log {
    file: Option<File>,
    written: u64,
}

impl Log {
    /// Opens `path` for appending (created with mode 0600). A log that cannot
    /// be opened is not an error: the server runs without it.
    pub(crate) fn open(path: &Path) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .ok();
        Log { file, written: 0 }
    }

    pub(crate) fn line(&mut self, msg: &str) {
        let Some(f) = self.file.as_mut() else { return };
        if self.written >= CAP {
            return;
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut text = format!("{} {msg}\n", utc(secs));
        if self.written + text.len() as u64 >= CAP {
            text = format!("{} log limit reached; further lines dropped\n", utc(secs));
        }
        if f.write_all(text.as_bytes()).is_ok() {
            self.written += text.len() as u64;
        } else {
            self.written = CAP;
        }
        if self.written + 64 >= CAP {
            self.written = CAP;
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` (the civil-from-days algorithm of H. Hinnant).
pub(crate) fn utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::assert_is_empty,
        clippy::format_push_string,
        clippy::cast_possible_truncation
    )]

    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_793_836_799), "2026-11-04T23:59:59Z");
    }
}
