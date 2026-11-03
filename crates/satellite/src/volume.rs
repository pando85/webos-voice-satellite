//! Reply volume: an in-process gain on every reply sample (streamed TTS,
//! TTS URLs, announcements, and the timer sound once it exists), applied
//! before `pacat`, independent of the TV's own volume.
//!
//! Why in-process: on c3-a `pacat --volume` is ignored (P1), so scaling the
//! samples is the only level control the satellite owns. The TV volume is
//! never read or changed here.
//!
//! Home Assistant sets the volume on the satellite's media player entity
//! (0 to 100 %). The percentage maps to a gain on a decibel taper:
//! 100 % is unity (0 dB), each percent below it is
//! [`RANGE_DB`]` / 100` dB quieter, and 0 % is digital silence. The value is
//! stored in `sink.volume_file` so restarts, updates and reboots keep it;
//! `sink.volume` is the value used when nothing is stored yet.
//!
//! `sink.gain` stays a separate fixed multiplier (for example 0.0 for silent
//! tests): the effective gain is `sink.gain * gain_for(volume)`.

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Attenuation at 1 %, in dB; 100 % is 0 dB. 50 dB over the slider gives
/// 0.5 dB per percent, so one HA volume step (5 %) is 2.5 dB.
pub(crate) const RANGE_DB: f32 = 50.0;

/// Gain for a volume in percent (values above 100 count as 100).
pub(crate) fn gain_for(percent: u8) -> f32 {
    let p = percent.min(100);
    if p == 0 {
        return 0.0;
    }
    let db = -RANGE_DB * f32::from(100 - p) / 100.0;
    10_f32.powf(db / 20.0)
}

/// The same in dB, for logs (`None` for silence).
pub(crate) fn db_for(percent: u8) -> Option<f32> {
    let p = percent.min(100);
    (p > 0).then(|| -RANGE_DB * f32::from(100 - p) / 100.0)
}

/// Shared, lock-free gain read by the playback tasks for every chunk, so a
/// change from HA also affects a streamed reply that is already playing.
#[derive(Debug, Clone)]
pub(crate) struct SharedGain(Arc<AtomicU32>);

impl SharedGain {
    pub(crate) fn new(gain: f32) -> Self {
        Self(Arc::new(AtomicU32::new(gain.to_bits())))
    }

    pub(crate) fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub(crate) fn set(&self, gain: f32) {
        self.0.store(gain.to_bits(), Ordering::Relaxed);
    }
}

/// Where the starting volume came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Origin {
    /// `sink.volume_file` held a valid value.
    Stored,
    /// No file yet: `sink.volume`.
    Default,
    /// The file exists but is unreadable or invalid (reason); `sink.volume`.
    Invalid(String),
}

/// Parses the file content: a whole number of percent, 0 to 100.
fn parse(text: &str) -> Result<u8, String> {
    let t = text.trim();
    let v: u32 = t
        .parse()
        .map_err(|_| format!("not a whole number of percent: {t:?}"))?;
    u8::try_from(v)
        .ok()
        .filter(|v| *v <= 100)
        .ok_or_else(|| format!("{v} is above 100"))
}

/// Reads the stored volume, falling back to `default`.
pub(crate) fn load(path: &Path, default: u8) -> (u8, Origin) {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(v) => (v, Origin::Stored),
            Err(why) => (default, Origin::Invalid(why)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (default, Origin::Default),
        Err(e) => (default, Origin::Invalid(e.to_string())),
    }
}

/// Stores the volume (see [`write_atomic`]).
pub(crate) fn store(path: &Path, percent: u8) -> std::io::Result<()> {
    write_atomic(path, &format!("{}\n", percent.min(100)))
}

/// Writes a small state file atomically: a temporary file in the same
/// directory (mode 0600), `fsync`, then `rename`, so a power cut leaves the
/// old or the new content, never a torn one.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let name = path
        .file_name()
        .map_or_else(|| "volume".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.map_or_else(
        || Path::new(&format!(".{name}.tmp")).to_path_buf(),
        |d| d.join(format!(".{name}.tmp")),
    );
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    #[test]
    fn taper_is_unity_at_full_silence_at_zero_and_monotonic() {
        assert_eq!(gain_for(100), 1.0);
        assert_eq!(gain_for(0), 0.0);
        assert_eq!(gain_for(200), 1.0, "above 100 counts as 100");
        // 50 % is -25 dB, 40 % is -30 dB, 1 % is -49.5 dB.
        assert!((20.0 * gain_for(50).log10() + 25.0).abs() < 1e-3);
        assert!((20.0 * gain_for(40).log10() + 30.0).abs() < 1e-3);
        assert!((20.0 * gain_for(1).log10() + 49.5).abs() < 1e-3);
        let mut prev = 0.0;
        for p in 0..=100 {
            let g = gain_for(p);
            assert!(g > prev || p == 0, "not increasing at {p}");
            assert!((0.0..=1.0).contains(&g));
            prev = g;
        }
        assert_eq!(db_for(0), None);
        assert_eq!(db_for(40), Some(-30.0));
    }

    #[test]
    fn shared_gain_round_trips() {
        let g = SharedGain::new(0.25);
        let h = g.clone();
        assert_eq!(h.get(), 0.25);
        g.set(0.0316);
        assert_eq!(h.get(), 0.0316);
    }

    #[test]
    fn store_and_load_round_trip_and_fall_back() {
        let dir = std::env::temp_dir().join(format!("webos-sat-volume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("reply_volume");
        assert_eq!(load(&f, 40), (40, Origin::Default));
        store(&f, 65).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "65\n");
        assert_eq!(load(&f, 40), (65, Origin::Stored));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        store(&f, 0).unwrap();
        assert_eq!(load(&f, 40), (0, Origin::Stored));
        for bad in ["", "abc", "101", "-3", "0.5", "999999999999"] {
            std::fs::write(&f, bad).unwrap();
            let (v, o) = load(&f, 40);
            assert_eq!(v, 40, "{bad:?}");
            assert!(matches!(o, Origin::Invalid(_)), "{bad:?}");
        }
        std::fs::write(&f, " 7 \n").unwrap();
        assert_eq!(load(&f, 40), (7, Origin::Stored));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
