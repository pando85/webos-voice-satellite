//! Taking the Magic Remote's mic button from LG's assistant while the
//! satellite's hold-to-talk runs ([ADR 0013](../../../docs/decisions/0013-remote-mic-button-coexistence.md),
//! accepted 2026-11-07 for this path; design: docs/design/remote-mic-button.md).
//!
//! The mechanism is LG's own per-key action in configd: the compositor's key
//! filter forwards the mic key to LG's assistant only while
//! `com.webos.keyaction.voice` has `"action":"launch"` (LG ships `"ignore"`
//! for hotel mode). The satellite sets `{"action":"ignore"}` **in memory
//! only** (`setConfigs` with `"volatile":true`; a reboot clears it) and
//! removes it again with `null`.
//!
//! **Acquire** (daemon start with `mic_button = "ptt"`), under the lock
//! `run/lg_voice.lock` (`flock`, shared with `ctl.sh restore-lg-voice`, so a
//! restore by the supervisor or by hand never interleaves with it):
//!
//! 1. A state file left by an earlier run (a crash) is restored first.
//! 2. The current value must read `launch` (LG's base value); anything else
//!    is left alone and the feature does not run.
//! 3. configd's persistent databases are looked at ([`PERSISTENT`]: whether
//!    each exists, its size and modification time).
//! 4. The state file (`run/lg_voice_override`: `configd <epoch> <token>`) is
//!    written **before** the change, so every restore path knows.
//! 5. `setConfigs {"configs":{"com.webos.keyaction.voice":{"action":"ignore"}},"volatile":true}`.
//! 6. Read back: the action must be `ignore`.
//! 7. The persistent databases must be as in step 3 (checked again a few
//!    seconds later and at the release, [`Takeover::check_persistence`]).
//!
//! Any failure after step 4 restores and the feature does not run.
//!
//! **Restore:** `setConfigs` with `null` (volatile), then a read-back that
//! must say **`launch`** (anything else, a missing value or a reply that
//! cannot be read, is no success). Only then the state file goes, and only
//! if it still holds the token read at the start (a newer acquire keeps its
//! file). A restore that is not confirmed keeps the state file for the next
//! path (`ctl.sh restore-lg-voice`, the supervisor) and reports `unknown`.
//!
//! Only `getConfigs` with the one config name is read; never a whole
//! category (`voiceframework` returns LG's security keys). The persistent
//! databases are only looked at (metadata), never opened or written.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::power::json_str;

/// configd's luna service.
pub(crate) const SET_CONFIGS: &str = "luna://com.webos.service.config/setConfigs";
pub(crate) const GET_CONFIGS: &str = "luna://com.webos.service.config/getConfigs";
/// The voice key's action config.
pub(crate) const KEY: &str = "com.webos.keyaction.voice";
/// In memory only.
pub(crate) const SET_IGNORE: &str =
    r#"{"configs":{"com.webos.keyaction.voice":{"action":"ignore"}},"volatile":true}"#;
/// Removes the in-memory value (LG's layered value applies again).
pub(crate) const SET_NULL: &str =
    r#"{"configs":{"com.webos.keyaction.voice":null},"volatile":true}"#;
/// Only this one config name, never a category.
pub(crate) const GET_KEY: &str = r#"{"configNames":["com.webos.keyaction.voice"]}"#;
/// The state file and the lock, in `<install>/run/`.
pub(crate) const STATE_FILE: &str = "lg_voice_override";
pub(crate) const LOCK_FILE: &str = "lg_voice.lock";
/// configd's persistent databases: written by `setConfigs` without
/// `volatile` (upstream `webosose/configd`); only their metadata is read.
pub(crate) const PERSISTENT: [&str; 2] = [
    "/var/preferences/configd_factory_db.json",
    "/var/preferences/configd_db.json",
];
/// How long the lock is waited for.
const LOCK_WAIT: Duration = Duration::from_secs(30);
/// Largest reply read from a call.
const REPLY_MAX: usize = 8192;

/// One luna call (the daemon's is `luna-send -n 1` on a pseudo-terminal).
pub(crate) trait Luna {
    fn call(&mut self, uri: &str, payload: &str) -> impl Future<Output = Result<String, String>>;
}

/// `luna-send` on a pseudo-terminal (it prints nothing over a pipe on this
/// firmware, EXP-011).
#[derive(Debug, Clone)]
pub(crate) struct LunaSend(pub(crate) PathBuf);

impl Luna for LunaSend {
    async fn call(&mut self, uri: &str, payload: &str) -> Result<String, String> {
        crate::power::call_pty(&self.0, uri, payload, REPLY_MAX).await
    }
}

/// Who handles the mic button, as `run/voice.json` and HA report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyOwner {
    /// LG's assistant (`launch`), nothing of ours in place.
    Lg,
    /// The in-memory override is in place (`ignore`): ours.
    Ours,
    /// The override is in place and the input grab of ADR 0016 runs: the key
    /// never reaches LG's compositor (the override is the safety net).
    OursGrab,
    /// A restore could not be confirmed (the state file is kept).
    Unknown,
}

impl KeyOwner {
    /// The word in `voice.json` and `ctl.sh status --json`.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Lg => "lg",
            Self::Ours => "ours (configd)",
            Self::OursGrab => "ours (grab + configd)",
            Self::Unknown => "unknown",
        }
    }
}

/// The `action` of a `getConfigs` reply for the one key: `Ok(None)` when
/// configd reports it missing (`missingConfigs`) or has no value; `Err` for
/// a refusal or a reply that cannot be read.
pub(crate) fn parse_action(reply: &str) -> Result<Option<String>, String> {
    let line = reply.trim();
    match crate::power::json_bool(line, "returnValue") {
        Some(true) => {}
        Some(false) => {
            let why = json_str(line, "errorText").unwrap_or_else(|| "returnValue false".to_owned());
            return Err(format!("getConfigs failed: {}", short(&why)));
        }
        None => return Err("getConfigs gave no readable answer".to_owned()),
    }
    // The value object of the key: `"com.webos.keyaction.voice": {...}`.
    let pat = format!("\"{KEY}\"");
    let Some(at) = line.find(&pat) else {
        return Ok(None);
    };
    let rest = line[at + pat.len()..].trim_start();
    let Some(rest) = rest.strip_prefix(':') else {
        return Ok(None);
    };
    let rest = rest.trim_start();
    if !rest.starts_with('{') {
        // A name in a `missingConfigs` list, or null.
        return Ok(None);
    }
    let body = &rest[..rest.find('}').map_or(rest.len(), |e| e + 1)];
    Ok(json_str(body, "action"))
}

/// A reply of `setConfigs` that says it worked.
fn set_ok(reply: &str) -> Result<(), String> {
    let line = reply.trim();
    if crate::power::json_bool(line, "returnValue") == Some(true) {
        Ok(())
    } else {
        let why = json_str(line, "errorText").unwrap_or_else(|| "no readable answer".to_owned());
        Err(format!("setConfigs failed: {}", short(&why)))
    }
}

/// Printable, at most 120 characters (for logs and HA).
fn short(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(120)
        .collect()
}

/// What a value reads as, for messages.
fn shown(a: Option<&str>) -> String {
    a.map_or_else(|| "not set".to_owned(), |a| format!("{:?}", short(a)))
}

/// Existence, size and modification time of a file (`None`: absent).
type Meta = Option<(u64, Option<SystemTime>)>;

fn meta(p: &Path) -> Meta {
    std::fs::metadata(p)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

/// The lock shared with `ctl.sh restore-lg-voice` (`flock` on
/// `run/lg_voice.lock`); released when dropped (the file closes).
pub(crate) struct Lock(#[allow(dead_code)] std::fs::File);

impl Lock {
    pub(crate) async fn take(path: &Path) -> Result<Self, String> {
        use rustix::fs::{FlockOperation, flock};
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("lock directory: {e}"))?;
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let until = tokio::time::Instant::now() + LOCK_WAIT;
        loop {
            match flock(&f, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => return Ok(Self(f)),
                Err(e) if e == rustix::io::Errno::WOULDBLOCK || e == rustix::io::Errno::INTR => {}
                Err(e) => return Err(format!("cannot lock {}: {e}", path.display())),
            }
            if tokio::time::Instant::now() >= until {
                return Err(format!(
                    "{} is held by another restore for more than {} s",
                    path.display(),
                    LOCK_WAIT.as_secs()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// A token for the state file (time and pid; only compared for equality).
fn token() -> String {
    let n = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}{:x}", n, std::process::id())
}

/// The override: its state file, lock and persistence baseline.
#[derive(Debug)]
pub(crate) struct Takeover<L> {
    luna: L,
    state_file: PathBuf,
    lock_file: PathBuf,
    persistent: Vec<PathBuf>,
    /// Metadata of [`Takeover::persistent`] before the change.
    baseline: Vec<Meta>,
    held: bool,
}

impl<L: Luna> Takeover<L> {
    /// `state_dir`: `<install>/run` (the fixed place `ctl.sh` and the
    /// supervisor read).
    pub(crate) fn new(luna: L, state_dir: &Path) -> Self {
        let dbs: Vec<PathBuf> = PERSISTENT.iter().map(PathBuf::from).collect();
        Self::with_persistent(luna, state_dir, &dbs)
    }

    pub(crate) fn with_persistent(luna: L, state_dir: &Path, persistent: &[PathBuf]) -> Self {
        Self {
            luna,
            state_file: state_dir.join(STATE_FILE),
            lock_file: state_dir.join(LOCK_FILE),
            persistent: persistent.to_vec(),
            baseline: Vec::new(),
            held: false,
        }
    }

    /// The override is in place (acquired and not released).
    #[cfg(test)]
    pub(crate) fn held(&self) -> bool {
        self.held
    }

    async fn read_action(&mut self) -> Result<Option<String>, String> {
        let reply = self.luna.call(GET_CONFIGS, GET_KEY).await?;
        parse_action(&reply)
    }

    fn write_state(&self) -> Result<(), String> {
        let epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Some(dir) = self.state_file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("state directory: {e}"))?;
        }
        crate::volume::write_atomic(&self.state_file, &format!("configd {epoch} {}\n", token()))
            .map_err(|e| format!("cannot write {}: {e}", self.state_file.display()))
    }

    /// The persistent databases are as before the change. `Err` names what
    /// changed.
    pub(crate) fn check_persistence(&self) -> Result<(), String> {
        for (p, before) in self.persistent.iter().zip(&self.baseline) {
            let now = meta(p);
            if now != *before {
                let what = match (before, now) {
                    (None, Some(_)) => "was created",
                    (Some(_), None) => "was removed",
                    _ => "changed (size or modification time)",
                };
                return Err(format!(
                    "configd's persistent database {} {what} while LG's voice key was overridden in memory",
                    p.display()
                ));
            }
        }
        Ok(())
    }

    /// Removes the override and confirms `launch`; the state file goes
    /// only then, and only if it still holds `token` (`None`: no file was
    /// there when the restore began).
    async fn restore(&mut self) -> Result<(), String> {
        let token = std::fs::read_to_string(&self.state_file).ok();
        let set = self
            .luna
            .call(SET_CONFIGS, SET_NULL)
            .await
            .and_then(|r| set_ok(&r));
        let back = self.read_action().await;
        match back {
            Ok(Some(a)) if a == "launch" => {
                let now = std::fs::read_to_string(&self.state_file).ok();
                if now == token {
                    let _ = std::fs::remove_file(&self.state_file);
                } else {
                    log::warn!(
                        "remote mic button: {} changed during the restore; kept",
                        self.state_file.display()
                    );
                }
                self.held = false;
                Ok(())
            }
            Ok(a) => Err(format!(
                "LG's voice key reads {} after the restore, not launch{}; kept {} (run ctl.sh restore-lg-voice or reboot the TV)",
                shown(a.as_deref()),
                set.err().map_or_else(String::new, |e| format!(" ({e})")),
                self.state_file.display()
            )),
            Err(e) => Err(format!(
                "LG's voice key could not be read back after the restore ({e}); kept {} (run ctl.sh restore-lg-voice or reboot the TV)",
                self.state_file.display()
            )),
        }
    }

    /// Takes the key from LG (see the module documentation). `Err` carries
    /// the reason, for the log and HA; LG's key is then as it was, unless
    /// the restore itself failed, which the reason says.
    pub(crate) async fn acquire(&mut self) -> Result<(), String> {
        let _lock = Lock::take(&self.lock_file).await?;
        if self.state_file.exists() {
            log::warn!(
                "remote mic button: {} is left from an earlier run; restoring LG's voice key first",
                self.state_file.display()
            );
            self.restore().await?;
        }
        let before = self.read_action().await?;
        if before.as_deref() != Some("launch") {
            return Err(format!(
                "LG's voice key action is {}, not launch; left as it is",
                shown(before.as_deref())
            ));
        }
        self.baseline = self.persistent.iter().map(|p| meta(p)).collect();
        // First the state file, so every restore path knows.
        self.write_state()?;
        let failure = match self
            .luna
            .call(SET_CONFIGS, SET_IGNORE)
            .await
            .and_then(|r| set_ok(&r))
        {
            Err(e) => Some(e),
            Ok(()) => match self.read_action().await {
                Ok(Some(a)) if a == "ignore" => self.check_persistence().err(),
                Ok(a) => Some(format!(
                    "the read-back says {}, not ignore",
                    shown(a.as_deref())
                )),
                Err(e) => Some(e),
            },
        };
        if let Some(why) = failure {
            return match self.restore().await {
                Ok(()) => Err(format!("{why}; LG's voice key was restored")),
                Err(r) => Err(format!("{why}; {r}")),
            };
        }
        self.held = true;
        Ok(())
    }

    /// Gives the key back to LG. Does nothing when nothing is held and no
    /// state file is there. A change of the persistent databases since the
    /// acquire is logged as an error (the restore still runs).
    pub(crate) async fn release(&mut self) -> Result<(), String> {
        if !self.held && !self.state_file.exists() {
            return Ok(());
        }
        let _lock = Lock::take(&self.lock_file).await?;
        if self.held {
            if let Err(e) = self.check_persistence() {
                log::error!("remote mic button: {e}");
            }
        }
        self.restore().await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use std::collections::VecDeque;

    use super::*;

    /// A fake configd: records the calls and answers from a script, or
    /// from its own in-memory value when the script is empty.
    #[allow(clippy::struct_excessive_bools)] // one bool per fake behaviour
    #[derive(Debug, Default)]
    pub(crate) struct FakeConfigd {
        pub(crate) calls: Vec<(String, String)>,
        /// Scripted answers, used before the model.
        pub(crate) script: VecDeque<Result<String, String>>,
        /// The volatile value: `None` = LG's base `launch`.
        pub(crate) volatile: Option<String>,
        /// `setConfigs` answers with an error.
        pub(crate) refuse_set: bool,
        /// The value read back ignores the set (the override is not applied).
        pub(crate) deaf: bool,
        /// A file `setConfigs` creates (to model a persistent write).
        pub(crate) creates: Option<PathBuf>,
        /// LG's base value is missing (`missingConfigs`) instead of launch.
        pub(crate) base_missing: bool,
        /// A file a `getConfigs` after a `null` rewrites (a newer acquire
        /// writing its state file during a restore).
        pub(crate) rewrite_on_restore: Option<PathBuf>,
        last_null: bool,
    }

    impl Luna for FakeConfigd {
        #[allow(clippy::unused_async_trait_impl)] // the trait's method is async
        async fn call(&mut self, uri: &str, payload: &str) -> Result<String, String> {
            self.calls.push((uri.to_owned(), payload.to_owned()));
            if let Some(r) = self.script.pop_front() {
                return r;
            }
            if uri == SET_CONFIGS {
                self.last_null = payload == SET_NULL;
                if self.refuse_set {
                    return Ok(r#"{"returnValue":false,"errorText":"denied"}"#.to_owned());
                }
                if let Some(f) = &self.creates {
                    let mut t = std::fs::read_to_string(f).unwrap_or_default();
                    t.push('x');
                    std::fs::write(f, t).unwrap();
                }
                if !self.deaf {
                    self.volatile = if payload == SET_IGNORE {
                        Some("ignore".to_owned())
                    } else {
                        None
                    };
                }
                return Ok(r#"{"returnValue":true}"#.to_owned());
            }
            if self.last_null {
                if let Some(f) = self.rewrite_on_restore.take() {
                    std::fs::write(f, "configd 2 newer\n").unwrap();
                }
            }
            if self.volatile.is_none() && self.base_missing {
                return Ok(r#"{"configs":{},"missingConfigs":["com.webos.keyaction.voice"],"returnValue":true}"#.to_owned());
            }
            let action = self.volatile.as_deref().unwrap_or("launch");
            Ok(format!(
                r#"{{"configs":{{"com.webos.keyaction.voice":{{"action":"{action}","id":"com.webos.app.voice"}}}},"returnValue":true}}"#
            ))
        }
    }

    impl FakeConfigd {
        pub(crate) fn sets(&self) -> Vec<&str> {
            self.calls
                .iter()
                .filter(|(u, _)| u == SET_CONFIGS)
                .map(|(_, p)| p.as_str())
                .collect()
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webos-sat-lgv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn takeover(dir: &Path, fake: FakeConfigd) -> Takeover<FakeConfigd> {
        Takeover::with_persistent(
            fake,
            dir,
            &[
                dir.join("configd_factory_db.json"),
                dir.join("configd_db.json"),
            ],
        )
    }

    #[test]
    fn replies_are_read_for_the_one_key_only() {
        assert_eq!(
            parse_action(
                r#"{"configs":{"com.webos.keyaction.voice":{"action":"launch","id":"com.webos.app.voice","params":{"activateType":"mrcu"}}},"returnValue":true}"#
            ),
            Ok(Some("launch".to_owned()))
        );
        assert_eq!(
            parse_action(r#"{"configs":{},"returnValue":true}"#),
            Ok(None)
        );
        assert!(parse_action(r#"{"returnValue":false,"errorText":"x"}"#).is_err());
        assert!(parse_action("").is_err());
        assert!(parse_action("garbage").is_err());
        // missingConfigs: the name is listed, no value.
        assert_eq!(
            parse_action(
                r#"{"configs":{},"missingConfigs":["com.webos.keyaction.voice"],"returnValue":true}"#
            ),
            Ok(None)
        );
        // Another key's action is not taken for this one.
        assert_eq!(
            parse_action(
                r#"{"configs":{"com.webos.keyaction.other":{"action":"launch"},"com.webos.keyaction.voice":null},"returnValue":true}"#
            ),
            Ok(None)
        );
        // Every payload names the one key, and every set is volatile.
        for p in [SET_IGNORE, SET_NULL] {
            assert!(p.contains("\"volatile\":true") && p.contains(KEY), "{p}");
        }
        assert_eq!(GET_KEY, r#"{"configNames":["com.webos.keyaction.voice"]}"#);
        assert!(!GET_KEY.contains("voiceframework"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn acquire_writes_the_state_file_first_sets_ignore_and_reads_it_back() {
        let dir = tmp("ok");
        let mut t = takeover(&dir, FakeConfigd::default());
        t.acquire().await.unwrap();
        assert!(t.held());
        assert!(
            std::fs::read_to_string(dir.join(STATE_FILE))
                .unwrap()
                .starts_with("configd ")
        );
        let urls: Vec<&str> = t.luna.calls.iter().map(|(u, _)| u.as_str()).collect();
        assert_eq!(urls, [GET_CONFIGS, SET_CONFIGS, GET_CONFIGS]);
        assert_eq!(t.luna.sets(), [SET_IGNORE]);
        // Release: null, read back, state file gone.
        t.release().await.unwrap();
        assert!(!t.held() && !dir.join(STATE_FILE).exists());
        assert_eq!(t.luna.sets(), [SET_IGNORE, SET_NULL]);
        assert_eq!(t.luna.volatile, None);
        // A second release does nothing.
        let n = t.luna.calls.len();
        t.release().await.unwrap();
        assert_eq!(t.luna.calls.len(), n);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_set_failure_restores_and_leaves_lg_as_it_was() {
        let dir = tmp("setfail");
        let mut t = takeover(
            &dir,
            FakeConfigd {
                refuse_set: true,
                ..FakeConfigd::default()
            },
        );
        let e = t.acquire().await.unwrap_err();
        assert!(e.contains("setConfigs failed: denied"), "{e}");
        assert!(!t.held());
        // The restore ran (null) and was confirmed: state file removed.
        assert_eq!(t.luna.sets(), [SET_IGNORE, SET_NULL]);
        assert!(!dir.join(STATE_FILE).exists());
        assert!(e.contains("restored"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_read_back_mismatch_restores_and_the_feature_does_not_run() {
        let dir = tmp("mismatch");
        let mut t = takeover(
            &dir,
            FakeConfigd {
                deaf: true,
                ..FakeConfigd::default()
            },
        );
        let e = t.acquire().await.unwrap_err();
        assert!(e.contains("read-back says \"launch\""), "{e}");
        assert!(!t.held() && !dir.join(STATE_FILE).exists());
        assert_eq!(t.luna.sets(), [SET_IGNORE, SET_NULL]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_change_of_configd_db_json_fails_the_takeover_and_is_seen_later() {
        let dir = tmp("configddb");
        let db = dir.join("configd_db.json");
        std::fs::write(&db, "{}").unwrap();
        let mut t = takeover(
            &dir,
            FakeConfigd {
                creates: Some(db.clone()),
                ..FakeConfigd::default()
            },
        );
        let e = t.acquire().await.unwrap_err();
        assert!(e.contains("configd_db.json changed"), "{e}");
        assert!(!t.held() && !dir.join(STATE_FILE).exists());
        // A change after a good acquire is found by the later check.
        let mut t = takeover(&dir, FakeConfigd::default());
        t.acquire().await.unwrap();
        t.check_persistence().unwrap();
        std::fs::write(&db, "{}{}").unwrap();
        let e = t.check_persistence().unwrap_err();
        assert!(e.contains("changed"), "{e}");
        // The release still restores.
        t.release().await.unwrap();
        assert!(!dir.join(STATE_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn success_needs_launch_back_otherwise_the_state_file_stays() {
        let dir = tmp("needlaunch");
        let mut t = takeover(&dir, FakeConfigd::default());
        t.acquire().await.unwrap();
        // After the null, configd reports the key missing: no success.
        t.luna.base_missing = true;
        let e = t.release().await.unwrap_err();
        assert!(
            e.contains("reads not set after the restore, not launch"),
            "{e}"
        );
        assert!(dir.join(STATE_FILE).exists(), "kept for the next path");
        // An unreadable read-back: no success either.
        t.luna.base_missing = false;
        t.luna
            .script
            .push_back(Ok(r#"{"returnValue":true}"#.to_owned()));
        t.luna.script.push_back(Ok("garbage".to_owned()));
        assert!(t.release().await.is_err());
        assert!(dir.join(STATE_FILE).exists());
        // launch again: done.
        t.release().await.unwrap();
        assert!(!dir.join(STATE_FILE).exists());
        // A missing base value is no start either.
        let mut t = takeover(
            &dir,
            FakeConfigd {
                base_missing: true,
                ..FakeConfigd::default()
            },
        );
        let e = t.acquire().await.unwrap_err();
        assert!(e.contains("not set, not launch"), "{e}");
        assert!(t.luna.sets().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_state_file_rewritten_during_the_restore_is_kept() {
        let dir = tmp("token");
        let mut t = takeover(&dir, FakeConfigd::default());
        t.acquire().await.unwrap();
        // A newer acquire's file appears while this restore runs.
        t.luna.rewrite_on_restore = Some(dir.join(STATE_FILE));
        t.release().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(STATE_FILE)).unwrap(),
            "configd 2 newer\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn acquire_waits_for_the_lock_a_restore_holds() {
        use rustix::fs::{FlockOperation, flock};
        let dir = tmp("lock");
        let held = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOCK_FILE))
            .unwrap();
        flock(&held, FlockOperation::LockExclusive).unwrap();
        let mut t = takeover(&dir, FakeConfigd::default());
        let start = tokio::time::Instant::now();
        let unlock = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        t.acquire().await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(250), "it waited");
        unlock.join().unwrap();
        // The state file has a token.
        let a = std::fs::read_to_string(dir.join(STATE_FILE)).unwrap();
        assert_eq!(a.split_whitespace().count(), 3, "{a}");
        t.release().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_persistent_database_written_by_configd_fails_the_takeover() {
        let dir = tmp("factory");
        let db = dir.join("configd_factory_db.json");
        let mut t = takeover(
            &dir,
            FakeConfigd {
                creates: Some(db.clone()),
                ..FakeConfigd::default()
            },
        );
        let e = t.acquire().await.unwrap_err();
        assert!(e.contains("persistent database"), "{e}");
        assert!(!t.held());
        assert_eq!(t.luna.sets(), [SET_IGNORE, SET_NULL]);
        // The file is never removed or opened by us.
        assert!(db.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_value_other_than_launch_is_left_alone() {
        let dir = tmp("notlaunch");
        let mut t = takeover(
            &dir,
            FakeConfigd {
                volatile: Some("ignore".to_owned()),
                ..FakeConfigd::default()
            },
        );
        let e = t.acquire().await.unwrap_err();
        assert!(e.contains("not launch; left as it is"), "{e}");
        assert!(t.luna.sets().is_empty(), "nothing written");
        assert!(!dir.join(STATE_FILE).exists());
        // A getConfigs error: nothing written either.
        let mut t = takeover(&dir, FakeConfigd::default());
        t.luna.script.push_back(Err("timeout".to_owned()));
        assert!(t.acquire().await.is_err());
        assert!(t.luna.sets().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_state_file_from_a_crash_is_restored_before_acquiring_again() {
        let dir = tmp("crash");
        std::fs::write(dir.join(STATE_FILE), "configd 1\n").unwrap();
        let mut t = takeover(
            &dir,
            FakeConfigd {
                volatile: Some("ignore".to_owned()),
                ..FakeConfigd::default()
            },
        );
        t.acquire().await.unwrap();
        assert_eq!(t.luna.sets(), [SET_NULL, SET_IGNORE]);
        assert!(t.held());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_unconfirmed_restore_keeps_the_state_file_for_the_other_paths() {
        let dir = tmp("keep");
        let mut t = takeover(&dir, FakeConfigd::default());
        t.acquire().await.unwrap();
        // configd does not answer the restore.
        t.luna.script.push_back(Err("timeout".to_owned()));
        t.luna.script.push_back(Err("timeout".to_owned()));
        let e = t.release().await.unwrap_err();
        assert!(e.contains("restore-lg-voice"), "{e}");
        assert!(dir.join(STATE_FILE).exists());
        // Next try works.
        t.release().await.unwrap();
        assert!(!dir.join(STATE_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_owner_words_are_fixed() {
        assert_eq!(KeyOwner::Lg.word(), "lg");
        assert_eq!(KeyOwner::Ours.word(), "ours (configd)");
        assert_eq!(KeyOwner::Unknown.word(), "unknown");
    }
}
