//! Process resource limits applied at startup: niceness and the soft
//! `RLIMIT_NOFILE`. Both are inherited by threads created afterwards and by
//! the `pacat` child. `RLIMIT_AS` is deliberately not set: `pacat` inherits
//! it and PulseAudio clients map large shared-memory pools, so an address
//! space cap could break playback. Memory is bounded by the resident-size
//! check of the watchdog instead (`limits.max_rss_mb`).

use rustix::process::{Resource, Rlimit, getrlimit, setpriority_process, setrlimit};

use crate::config::LimitsSection;

/// Applies the limits and returns one log line describing the result.
/// Failures are reported in the line, never fatal.
///
/// Must run before the runtime and the threads start: on Linux the nice
/// value is per thread and inherited at thread creation.
pub(crate) fn apply(l: &LimitsSection) -> String {
    let nice = match setpriority_process(None, l.nice) {
        Ok(()) => format!("nice {}", l.nice),
        Err(e) => format!("nice {} NOT set ({e})", l.nice),
    };
    let nofile = if l.max_open_files == 0 {
        "open files inherited".to_owned()
    } else {
        let cur = getrlimit(Resource::Nofile);
        let want = u64::from(l.max_open_files);
        let soft = cur.maximum.map_or(want, |hard| want.min(hard));
        match setrlimit(
            Resource::Nofile,
            Rlimit {
                current: Some(soft),
                maximum: cur.maximum,
            },
        ) {
            Ok(()) => format!("open files {soft}"),
            Err(e) => format!("open files NOT limited ({e})"),
        }
    };
    let rss = if l.max_rss_mb == 0 {
        "rss ceiling off".to_owned()
    } else {
        format!("rss ceiling {} MiB", l.max_rss_mb)
    };
    format!(
        "limits: {nice}, {nofile}, {rss}, capture stall {} s, loop stall {} s",
        l.capture_stall_s, l.loop_stall_s
    )
}

/// Resident set size of this process in bytes, from `/proc/self/statm`.
pub(crate) fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * rustix::param::page_size() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_own_rss() {
        let rss = rss_bytes().unwrap_or(0);
        assert!(rss > 0 && rss < 4 << 30, "{rss}");
    }

    #[test]
    fn disabled_limits_are_reported() {
        let l = LimitsSection {
            nice: 0,
            max_open_files: 0,
            max_rss_mb: 0,
            capture_stall_s: 0,
            loop_stall_s: 0,
        };
        let line = apply(&l);
        assert!(line.contains("open files inherited"), "{line}");
        assert!(line.contains("rss ceiling off"), "{line}");
    }
}
