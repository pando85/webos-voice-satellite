//! `run/voice.json`: what the daemon tells `ctl.sh` and the TV app about the
//! microphone and the remote power-off setting (docs/design/voice-and-power-
//! screen.md, 6.4).
//!
//! One small file, written atomically, only when its content changes (plus a
//! keep-alive rewrite so a reader can tell a live daemon from a stale file by
//! the file's age). The daemon checks it once a second, never per audio
//! frame. It carries no identifier, address, name or key: only the fields
//! below.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::compat::json_str;
use crate::config::RemoteOff;
use crate::darken;
use crate::volume::write_atomic;

/// File name inside the report directory (`report.dir`, default
/// `/tmp/webos-satellite`; the packaged config uses `run/`).
pub(crate) const FILE: &str = "voice.json";

/// Rewrite an unchanged file this often: readers treat a file older than 60 s
/// as stale (`unknown`).
const KEEP_ALIVE: Duration = Duration::from_secs(30);

/// The microphone, as the app shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mic {
    /// The capture PCM is open and delivering audio.
    Listening,
    /// Wanted but another process holds the PCM (LG hands-free).
    Busy,
    /// A gate keeps it closed: HA's mute switch, the kill switch, the
    /// firmware guard or an unpaired satellite.
    Muted,
    /// Closed for another reason (not open yet, open failed, stalled).
    Closed,
}

impl Mic {
    fn word(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Busy => "busy",
            Self::Muted => "muted",
            Self::Closed => "closed",
        }
    }
}

/// The Home Assistant API connection, as the app shows it. Only the state:
/// no address, name or version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ha {
    /// Home Assistant has an open API session.
    Connected,
    /// Paired (or no pairing needed) but no session now.
    Disconnected,
    /// Pairing is on and no key is stored yet: nobody has paired.
    Unpaired,
}

impl Ha {
    fn word(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Unpaired => "unpaired",
        }
    }
}

/// The content of `voice.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VoiceStatus {
    pub(crate) mic: Mic,
    /// The mode the daemon holds now.
    pub(crate) remote_off: RemoteOff,
    /// The power watch runs, so "keep listening" can act. False without the
    /// watch (`power.watch = false` or no `luna-send`).
    pub(crate) remote_off_active: bool,
    /// The last settled TV power state (`Active`, `Screen Off`, ...), if known.
    pub(crate) power_state: Option<String>,
    /// When the open pairing window closes, in seconds since the Unix epoch;
    /// `None` while it is closed. Written on open and close only, so the
    /// reader (ctl.sh, the TV app, the phone page) computes the countdown.
    pub(crate) pairing_until: Option<u64>,
    /// The Home Assistant connection.
    pub(crate) ha: Ha,
    /// When `ha` last changed, in seconds since the Unix epoch (the daemon's
    /// start for the first state). Written on a change only.
    pub(crate) ha_since: u64,
    /// The `music` object (`crate::music`, design music-playback section K):
    /// `{"enabled":false}` with music off. `None` leaves the key out.
    pub(crate) music: Option<String>,
}

impl VoiceStatus {
    /// One JSON line (schema 1).
    pub(crate) fn to_json(&self) -> String {
        format!(
            "{{\"schema\":1,\"mic\":{},\"remote_off\":{},\"remote_off_active\":{},\"power_state\":{},\"pairing_until\":{},\"ha\":{},\"ha_since\":{}{}}}\n",
            json_str(self.mic.word()),
            json_str(darken::file_word(self.remote_off)),
            self.remote_off_active,
            self.power_state
                .as_deref()
                .map_or_else(|| "null".to_owned(), json_str),
            self.pairing_until
                .map_or_else(|| "null".to_owned(), |t| t.to_string()),
            json_str(self.ha.word()),
            self.ha_since,
            self.music
                .as_deref()
                .map_or_else(String::new, |m| format!(",\"music\":{m}")),
        )
    }
}

/// Writes the file when needed.
#[derive(Debug)]
pub(crate) struct VoiceFile {
    path: PathBuf,
    last: Option<String>,
    written: Option<Instant>,
    warned: bool,
}

impl VoiceFile {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE),
            last: None,
            written: None,
            warned: false,
        }
    }

    /// Writes `status` if it differs from the last write or the keep-alive is
    /// due. Returns true if the file was written.
    pub(crate) fn publish(&mut self, now: Instant, status: &VoiceStatus) -> bool {
        let json = status.to_json();
        let due = self
            .written
            .is_none_or(|w| now.saturating_duration_since(w) >= KEEP_ALIVE);
        if self.last.as_deref() == Some(json.as_str()) && !due {
            return false;
        }
        let result = self
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| write_atomic(&self.path, &json));
        match result {
            Ok(()) => {
                self.last = Some(json);
                self.written = Some(now);
                self.warned = false;
                true
            }
            Err(e) => {
                // Retried at the next check; logged once until it works again.
                if !self.warned {
                    self.warned = true;
                    log::warn!("cannot write {}: {e}", self.path.display());
                }
                false
            }
        }
    }

    /// Removes the file (clean shutdown: the daemon no longer answers).
    pub(crate) fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn status(mic: Mic) -> VoiceStatus {
        VoiceStatus {
            mic,
            remote_off: RemoteOff::KeepListening,
            remote_off_active: true,
            power_state: Some("Active".to_owned()),
            pairing_until: None,
            ha: Ha::Connected,
            ha_since: 1_762_000_000,
            music: None,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webos-sat-voice-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn json_has_exactly_the_documented_fields() {
        assert_eq!(
            status(Mic::Listening).to_json(),
            "{\"schema\":1,\"mic\":\"listening\",\"remote_off\":\"keep_listening\",\"remote_off_active\":true,\"power_state\":\"Active\",\"pairing_until\":null,\"ha\":\"connected\",\"ha_since\":1762000000}\n"
        );
        let s = VoiceStatus {
            mic: Mic::Busy,
            remote_off: RemoteOff::Normal,
            remote_off_active: false,
            power_state: None,
            pairing_until: Some(1_762_345_678),
            ha: Ha::Unpaired,
            ha_since: 5,
            music: None,
        };
        assert_eq!(
            s.to_json(),
            "{\"schema\":1,\"mic\":\"busy\",\"remote_off\":\"normal\",\"remote_off_active\":false,\"power_state\":null,\"pairing_until\":1762345678,\"ha\":\"unpaired\",\"ha_since\":5}\n"
        );
        // The music block (design music-playback section K) goes last.
        let mut m = status(Mic::Listening);
        m.music = Some("{\"enabled\":false}".to_owned());
        assert!(
            m.to_json()
                .ends_with(",\"ha_since\":1762000000,\"music\":{\"enabled\":false}}\n")
        );
        for (m, w) in [(Mic::Muted, "muted"), (Mic::Closed, "closed")] {
            assert!(status(m).to_json().contains(&format!("\"mic\":\"{w}\"")));
        }
    }

    #[test]
    fn power_state_is_escaped() {
        let mut s = status(Mic::Closed);
        s.power_state = Some("a\"b\\c".to_owned());
        assert!(s.to_json().contains("\"power_state\":\"a\\\"b\\\\c\""));
    }

    #[test]
    fn written_on_change_only_with_a_keep_alive() {
        let dir = tmp("change");
        let mut f = VoiceFile::new(&dir);
        let t0 = Instant::now();
        let file = dir.join(FILE);
        // First call creates the directory and the file.
        assert!(f.publish(t0, &status(Mic::Closed)));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            status(Mic::Closed).to_json()
        );
        // Unchanged: no write (the file is removed to prove it).
        std::fs::remove_file(&file).unwrap();
        assert!(!f.publish(t0 + Duration::from_secs(1), &status(Mic::Closed)));
        assert!(!file.exists());
        // A change writes, and leaves no temporary file behind.
        assert!(f.publish(t0 + Duration::from_secs(2), &status(Mic::Listening)));
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("\"mic\":\"listening\"")
        );
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        // Unchanged for a while: the keep-alive rewrites it.
        assert!(!f.publish(t0 + Duration::from_secs(20), &status(Mic::Listening)));
        assert!(f.publish(t0 + Duration::from_secs(33), &status(Mic::Listening)));
        // Clean shutdown removes it.
        f.remove();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_is_retried() {
        let dir = tmp("fail");
        std::fs::create_dir_all(&dir).unwrap();
        // The directory is a file: the write fails.
        let blocker = dir.join("run");
        std::fs::write(&blocker, b"").unwrap();
        let mut f = VoiceFile::new(&blocker);
        let t0 = Instant::now();
        assert!(!f.publish(t0, &status(Mic::Closed)));
        assert!(f.warned);
        std::fs::remove_file(&blocker).unwrap();
        assert!(f.publish(t0 + Duration::from_secs(1), &status(Mic::Closed)));
        assert!(blocker.join(FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
