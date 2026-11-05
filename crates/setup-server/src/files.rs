//! The session directory (section 4.1): `state.json` (written here, atomic,
//! 0600), `heartbeat` (touched by `ctl.sh setup status`), `answer.json`
//! (written by `ctl.sh setup answer`, consumed here) and `stop` (created by
//! `ctl.sh setup stop`). The directory is volatile (tmpfs), root's, 0700.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::secret::is_id;

pub(crate) const STATE: &str = "state.json";
pub(crate) const ANSWER: &str = "answer.json";
pub(crate) const HEARTBEAT: &str = "heartbeat";
pub(crate) const STOP: &str = "stop";
const STATE_TMP: &str = ".state.json.tmp";
const MAX_ANSWER: u64 = 256;

pub(crate) struct Dir {
    path: PathBuf,
}

/// Why the directory is refused (startup fails closed).
#[derive(Debug)]
pub(crate) enum DirError {
    Io(io::Error),
    NotADirectory,
    WrongOwner,
}

impl std::fmt::Display for DirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirError::Io(e) => write!(f, "{e}"),
            DirError::NotADirectory => f.write_str("not a directory (or a symbolic link)"),
            DirError::WrongOwner => f.write_str("owned by another user"),
        }
    }
}

impl Dir {
    /// Creates the directory with mode 0700, or accepts an existing one only
    /// when it is a real directory owned by this user (then forced to 0700);
    /// removes stale `answer.json`, `stop` and temporary files.
    pub(crate) fn prepare(path: &Path) -> Result<Self, DirError> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                DirBuilder::new()
                    .mode(0o700)
                    .create(path)
                    .map_err(DirError::Io)?;
            }
            Err(e) => return Err(DirError::Io(e)),
            Ok(_) => {}
        }
        let md = fs::symlink_metadata(path).map_err(DirError::Io)?;
        if !md.file_type().is_dir() {
            return Err(DirError::NotADirectory);
        }
        if md.uid() != rustix::process::geteuid().as_raw() {
            return Err(DirError::WrongOwner);
        }
        if md.mode() & 0o777 != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(DirError::Io)?;
        }
        let dir = Dir {
            path: path.to_owned(),
        };
        dir.remove(ANSWER);
        dir.remove(STOP);
        dir.remove(STATE_TMP);
        Ok(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    pub(crate) fn remove(&self, name: &str) {
        let _ = fs::remove_file(self.file(name));
    }

    /// Writes `state.json` through a temporary file and `rename` (mode 0600).
    pub(crate) fn write_state(&self, bytes: &[u8]) -> io::Result<()> {
        let tmp = self.file(STATE_TMP);
        let _ = fs::remove_file(&tmp);
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.write_all(b"\n")?;
        drop(f);
        fs::rename(&tmp, self.file(STATE))
    }

    /// The heartbeat file's modification time, if it exists.
    pub(crate) fn heartbeat(&self) -> Option<SystemTime> {
        fs::symlink_metadata(self.file(HEARTBEAT))
            .ok()
            .filter(|m| m.file_type().is_file())
            .and_then(|m| m.modified().ok())
    }

    /// True when `ctl.sh setup stop` asked to end, or the directory is gone.
    pub(crate) fn stop_requested(&self) -> bool {
        self.file(STOP).exists() || !self.path.is_dir()
    }

    /// Reads and deletes `answer.json` (an invalid one is deleted too).
    pub(crate) fn take_answer(&self) -> Taken {
        let path = self.file(ANSWER);
        let Ok(md) = fs::symlink_metadata(&path) else {
            return Taken::Nothing;
        };
        let answer = if md.file_type().is_file() && md.len() <= MAX_ANSWER {
            let mut buf = Vec::new();
            fs::File::open(&path)
                .and_then(|f| f.take(MAX_ANSWER).read_to_end(&mut buf))
                .ok()
                .and_then(|_| parse_answer(&buf))
        } else {
            None
        };
        let _ = fs::remove_file(&path);
        answer.map_or(Taken::Invalid, Taken::Answer)
    }
}

/// What [`Dir::take_answer`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Taken {
    Nothing,
    Invalid,
    Answer(Answer),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) id: String,
    pub(crate) allow: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnswer {
    id: String,
    verdict: String,
}

/// `{"id":"a1b2c3d4","verdict":"allow"|"refuse"}`, nothing else.
pub(crate) fn parse_answer(bytes: &[u8]) -> Option<Answer> {
    let raw: RawAnswer = serde_json::from_slice(bytes).ok()?;
    if !is_id(&raw.id) {
        return None;
    }
    let allow = match raw.verdict.as_str() {
        "allow" => true,
        "refuse" => false,
        _ => return None,
    };
    Some(Answer { id: raw.id, allow })
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::assert_is_empty,
        clippy::format_push_string,
        clippy::cast_possible_truncation
    )]

    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh directory under the system temp dir (removed by the caller or
    /// left in /tmp; names are unique per process and call).
    pub(crate) fn scratch(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "setup-server-test-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn answers() {
        assert_eq!(
            parse_answer(br#"{"id":"a1b2c3d4","verdict":"allow"}"#),
            Some(Answer {
                id: "a1b2c3d4".into(),
                allow: true
            })
        );
        assert_eq!(
            parse_answer(br#"{"id":"a1b2c3d4","verdict":"refuse"}"#),
            Some(Answer {
                id: "a1b2c3d4".into(),
                allow: false
            })
        );
        for bad in [
            &br#"{"id":"A1B2C3D4","verdict":"allow"}"#[..],
            br#"{"id":"a1b2c3d4","verdict":"yes"}"#,
            br#"{"id":"a1b2c3d4","verdict":"allow","x":1}"#,
            br#"{"id":"a1b2c3d4","id":"a1b2c3d4","verdict":"allow"}"#,
            br#"{"id":"a1b2c3d4"}"#,
            b"allow",
            b"",
        ] {
            assert_eq!(parse_answer(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn directory_is_created_0700_and_files_0600() {
        let p = scratch("dir");
        let d = Dir::prepare(&p).expect("prepare");
        assert_eq!(fs::metadata(&p).expect("md").mode() & 0o777, 0o700);
        d.write_state(b"{}").expect("write");
        assert_eq!(
            fs::metadata(p.join(STATE)).expect("md").mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read(p.join(STATE)).expect("read"), b"{}\n");
        // An existing directory with a loose mode is tightened.
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::write(p.join(ANSWER), b"x").expect("w");
        fs::write(p.join(STOP), b"").expect("w");
        let d = Dir::prepare(&p).expect("prepare again");
        assert_eq!(fs::metadata(&p).expect("md").mode() & 0o777, 0o700);
        assert!(!p.join(ANSWER).exists() && !p.join(STOP).exists());
        assert!(!d.stop_requested());
        fs::write(p.join(STOP), b"").expect("w");
        assert!(d.stop_requested());
        let _ = fs::remove_dir_all(&p);
        assert!(d.stop_requested());
    }

    #[test]
    fn a_symlink_or_file_is_refused() {
        let p = scratch("link");
        let target = scratch("target");
        fs::create_dir(&target).expect("mkdir");
        std::os::unix::fs::symlink(&target, &p).expect("symlink");
        assert!(matches!(Dir::prepare(&p), Err(DirError::NotADirectory)));
        let _ = fs::remove_file(&p);
        fs::write(&p, b"").expect("file");
        assert!(matches!(Dir::prepare(&p), Err(DirError::NotADirectory)));
        let _ = fs::remove_file(&p);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn answer_file_is_consumed() {
        let p = scratch("answer");
        let d = Dir::prepare(&p).expect("prepare");
        assert_eq!(d.take_answer(), Taken::Nothing);
        fs::write(p.join(ANSWER), br#"{"id":"0000abcd","verdict":"allow"}"#).expect("w");
        assert_eq!(
            d.take_answer(),
            Taken::Answer(Answer {
                id: "0000abcd".into(),
                allow: true
            })
        );
        assert!(!p.join(ANSWER).exists());
        fs::write(p.join(ANSWER), vec![b' '; 1000]).expect("w");
        assert_eq!(d.take_answer(), Taken::Invalid);
        assert!(!p.join(ANSWER).exists());
        assert!(d.heartbeat().is_none());
        fs::write(p.join(HEARTBEAT), b"").expect("w");
        assert!(d.heartbeat().is_some());
        let _ = fs::remove_dir_all(&p);
    }
}
