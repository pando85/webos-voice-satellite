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
//! **Acquire** (daemon start with `mic_button = "ptt"`), in this order:
//!
//! 1. A state file left by an earlier run (a crash) is restored first.
//! 2. The current value must read `launch` (LG's base value); anything else
//!    is left alone and the feature does not run.
//! 3. The state file (`run/lg_voice_override`) is written **before** the
//!    change, so every restore path (`ctl.sh restore-lg-voice`, the
//!    supervisor after any daemon exit, `ctl.sh stop`, `uninstall`, `purge`)
//!    knows there is something to undo.
//! 4. `setConfigs {"configs":{"com.webos.keyaction.voice":{"action":"ignore"}},"volatile":true}`.
//! 5. Read back: the action must be `ignore`.
//! 6. configd's persistent database (`/var/preferences/configd_factory_db.json`)
//!    must not have been created by it (only checked for existence, never
//!    opened or written).
//!
//! Any failure after step 3 restores (`null`, volatile) and reads back, so
//! LG's key is as it was; the feature then stays off and the reason goes to
//! the log, `run/voice.json`, `ctl.sh status` and Home Assistant.
//!
//! **Release** (feature off, daemon shutdown): `setConfigs` with `null`
//! (volatile), read back that the action is no longer `ignore`, remove the
//! state file. A restore that cannot be confirmed keeps the state file, so
//! `ctl.sh restore-lg-voice` (and the supervisor) try again.
//!
//! Only `getConfigs` with the one config name is read; never a whole
//! category (`voiceframework` returns LG's security keys).

use std::future::Future;
use std::path::{Path, PathBuf};

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
/// The state file, in the report directory (`run/`).
pub(crate) const STATE_FILE: &str = "lg_voice_override";
/// configd's persistent database: written by `setConfigs` without
/// `volatile`. Only its existence is checked.
pub(crate) const FACTORY_DB: &str = "/var/preferences/configd_factory_db.json";
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
    /// A restore could not be confirmed (the state file is kept).
    Unknown,
}

impl KeyOwner {
    /// The word in `voice.json` and `ctl.sh status --json`.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Lg => "lg",
            Self::Ours => "ours (configd)",
            Self::Unknown => "unknown",
        }
    }
}

/// The `action` of a `getConfigs` reply for the one key: `Ok(None)` when
/// configd has no value for it.
pub(crate) fn parse_action(reply: &str) -> Result<Option<String>, String> {
    let line = reply.trim();
    if crate::power::json_bool(line, "returnValue") != Some(true) {
        let why = json_str(line, "errorText").unwrap_or_else(|| "no answer".to_owned());
        return Err(format!("getConfigs failed: {}", short(&why)));
    }
    if !line.contains(KEY) {
        return Ok(None);
    }
    Ok(json_str(line, "action"))
}

/// A reply of `setConfigs` that says it worked.
fn set_ok(reply: &str) -> Result<(), String> {
    let line = reply.trim();
    if crate::power::json_bool(line, "returnValue") == Some(true) {
        Ok(())
    } else {
        let why = json_str(line, "errorText").unwrap_or_else(|| "no answer".to_owned());
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

/// The override: its state file and whether it is held.
#[derive(Debug)]
pub(crate) struct Takeover<L> {
    luna: L,
    state_file: PathBuf,
    factory_db: PathBuf,
    held: bool,
}

impl<L: Luna> Takeover<L> {
    pub(crate) fn new(luna: L, state_dir: &Path) -> Self {
        Self::with_factory_db(luna, state_dir, Path::new(FACTORY_DB))
    }

    pub(crate) fn with_factory_db(luna: L, state_dir: &Path, factory_db: &Path) -> Self {
        Self {
            luna,
            state_file: state_dir.join(STATE_FILE),
            factory_db: factory_db.to_path_buf(),
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
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Some(dir) = self.state_file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("state directory: {e}"))?;
        }
        crate::volume::write_atomic(&self.state_file, &format!("configd {epoch}\n"))
            .map_err(|e| format!("cannot write {}: {e}", self.state_file.display()))
    }

    /// Removes the override and confirms it; the state file goes only then.
    async fn restore(&mut self) -> Result<(), String> {
        let set = self
            .luna
            .call(SET_CONFIGS, SET_NULL)
            .await
            .and_then(|r| set_ok(&r));
        let back = self.read_action().await;
        match (set, back) {
            (_, Ok(a)) if a.as_deref() != Some("ignore") => {
                let _ = std::fs::remove_file(&self.state_file);
                self.held = false;
                Ok(())
            }
            (Err(e), _) | (_, Err(e)) => Err(format!(
                "LG's voice key could not be restored ({e}); run ctl.sh restore-lg-voice or reboot the TV"
            )),
            (Ok(()), Ok(_)) => Err(
                "LG's voice key still reads ignore after the restore; run ctl.sh restore-lg-voice or reboot the TV"
                    .to_owned(),
            ),
        }
    }

    /// Takes the key from LG (see the module documentation). `Err` carries
    /// the reason, for the log and HA; LG's key is then as it was, unless
    /// the restore itself failed, which the reason says.
    pub(crate) async fn acquire(&mut self) -> Result<(), String> {
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
                before.map_or_else(|| "not set".to_owned(), |a| format!("{:?}", short(&a)))
            ));
        }
        let db_before = self.factory_db.exists();
        // First the state file, so every restore path knows.
        self.write_state()?;
        let failure = match self.luna.call(SET_CONFIGS, SET_IGNORE).await.and_then(|r| set_ok(&r)) {
            Err(e) => Some(e),
            Ok(()) => match self.read_action().await {
                Ok(Some(a)) if a == "ignore" => (!db_before && self.factory_db.exists()).then(|| {
                    format!(
                        "configd created its persistent database {} (the in-memory override was not kept in memory)",
                        self.factory_db.display()
                    )
                }),
                Ok(a) => Some(format!(
                    "the read-back says {}, not ignore",
                    a.map_or_else(|| "not set".to_owned(), |a| format!("{:?}", short(&a)))
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
    /// state file is there.
    pub(crate) async fn release(&mut self) -> Result<(), String> {
        if !self.held && !self.state_file.exists() {
            return Ok(());
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
    }

    impl Luna for FakeConfigd {
        #[allow(clippy::unused_async_trait_impl)] // the trait's method is async
        async fn call(&mut self, uri: &str, payload: &str) -> Result<String, String> {
            self.calls.push((uri.to_owned(), payload.to_owned()));
            if let Some(r) = self.script.pop_front() {
                return r;
            }
            if uri == SET_CONFIGS {
                if self.refuse_set {
                    return Ok(r#"{"returnValue":false,"errorText":"denied"}"#.to_owned());
                }
                if let Some(f) = &self.creates {
                    std::fs::write(f, "{}").unwrap();
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
        Takeover::with_factory_db(fake, dir, &dir.join("configd_factory_db.json"))
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
