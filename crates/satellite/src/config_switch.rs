//! HA's config switches, `aec_enabled`, `music_enabled`
//! ([ADR 0012](../../../docs/decisions/0012-config-switches-in-ha.md)) and
//! `remote_mic_button` (the Magic Remote's mic button, `[remote] mic_button`
//! "ptt" or "off", added the same way: docs/design/remote-mic-button.md).
//!
//! Flipping one in Home Assistant runs `ctl.sh config aec|music|mic-button on|off`
//! detached: that command rewrites the one config key, checks the config,
//! restarts this daemon (which ends this process) and puts the previous
//! config back if the new one does not start. The daemon never edits the
//! config itself.
//!
//! This module is a thin wrapper. The guards that every entry point must
//! share (the 30 s cooldown and the refusal while a voice request, music or
//! the timer alarm is active) live in `ctl.sh config`, so the TV app and HA
//! behave the same. What stays here, in pure functions
//! ([`Setting::from_object_id`], [`ctl_args`], [`decide`]) tested without a
//! process:
//!
//! - only the fixed object ids map to a setting, and the arguments of
//!   `ctl.sh` come from a fixed table, never from received text;
//! - a request for the state already in force does nothing (an optimization,
//!   not a separate rule: `ctl.sh` answers `already` for it too);
//! - a busy pre-check from the daemon's own state ([`Busy`], the same
//!   definition that writes `busy` into `run/voice.json`), so a refusal does
//!   not even start a process;
//! - one detached `ctl.sh` at a time.
//!
//! `ctl.sh` is started with an empty environment plus a fixed `PATH`,
//! `WEBOS_SAT_PREFIX` (this install) and `WEBOS_SAT_SOURCE=ha`: nothing this
//! process inherited, in particular not `WEBOS_SAT_LOCK_HELD`, reaches it, so
//! it takes the install lock like any other caller.
//!
//! A refused or failed request (ours, or reported by `ctl.sh`: cooldown,
//! busy) is answered with the stored state again
//! (`AppCommand::ResendSwitch`); whether HA's UI then reverts is not verified.
//! Logs name the object id, the result words of `ctl.sh` and, for a failure,
//! its short error text; never speech, audio or a URL.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::mpsc;
use webos_esphome_api::proto::EntityCategory;
use webos_esphome_api::{AppCommand, SwitchEntity};

use crate::config::Settings;
use crate::voice_status::Busy;

/// Object id of the echo cancellation switch.
pub(crate) const AEC_ID: &str = "aec_enabled";
/// Object id of the music switch.
pub(crate) const MUSIC_ID: &str = "music_enabled";
/// Object id of the remote mic button switch (on: `mic_button = "ptt"`).
pub(crate) const MIC_BUTTON_ID: &str = "remote_mic_button";
/// After this long a still running `ctl.sh` is logged (a stop, a start and a
/// rollback with another start take about 40 s in the worst case). It is not
/// killed (it may be between two config files) and stays in flight until it
/// exits, so two never overlap.
const SLOW_AFTER: Duration = Duration::from_secs(120);
/// The `PATH` of `ctl.sh`, as the setup server gets it from `ctl.sh`.
const CTL_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
/// File in the state directory that takes `ctl.sh`'s one JSON line.
const OUT_FILE: &str = "config_switch.out";
/// At most this much of `ctl.sh`'s output is read.
const OUT_MAX: u64 = 4096;

/// The `sh -c` text that starts `ctl.sh` in a session of its own, like
/// `start.sh` does (`setsid` when present): `ctl.sh` stops the daemon that
/// started it. Fixed text; the script path and the arguments follow as
/// positional parameters and are never part of it.
const DETACH: &str = r#"if command -v setsid >/dev/null 2>&1; then exec setsid nohup sh "$@"; else exec nohup sh "$@"; fi"#;

/// A setting HA can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Setting {
    /// `[aec] enabled`.
    Aec,
    /// `[music] enabled`.
    Music,
    /// `[remote] mic_button` (on: `"ptt"`, off: `"off"`).
    MicButton,
}

/// Every setting, in the order of the entities.
const ALL: [Setting; 3] = [Setting::Aec, Setting::Music, Setting::MicButton];

impl Setting {
    /// The setting of an object id, for exactly the known ids.
    pub(crate) fn from_object_id(id: &str) -> Option<Self> {
        match id {
            AEC_ID => Some(Self::Aec),
            MUSIC_ID => Some(Self::Music),
            MIC_BUTTON_ID => Some(Self::MicButton),
            _ => None,
        }
    }

    pub(crate) fn object_id(self) -> &'static str {
        match self {
            Self::Aec => AEC_ID,
            Self::Music => MUSIC_ID,
            Self::MicButton => MIC_BUTTON_ID,
        }
    }

    /// The `ctl.sh config` word.
    fn word(self) -> &'static str {
        match self {
            Self::Aec => "aec",
            Self::Music => "music",
            Self::MicButton => "mic-button",
        }
    }
}

/// The arguments of `ctl.sh` for a change: only these literals.
pub(crate) fn ctl_args(setting: Setting, on: bool) -> [&'static str; 3] {
    ["config", setting.word(), if on { "on" } else { "off" }]
}

/// The switch entities (entity category `config`).
pub(crate) fn entities() -> Vec<SwitchEntity> {
    let sw = |id: &str, name: &str, icon: &str| SwitchEntity {
        object_id: id.to_owned(),
        name: name.to_owned(),
        icon: icon.to_owned(),
        device_class: String::new(),
        entity_category: EntityCategory::Config,
    };
    vec![
        sw(AEC_ID, "Echo cancellation", "mdi:waveform"),
        sw(MUSIC_ID, "Music playback", "mdi:music"),
        sw(MIC_BUTTON_ID, "Remote mic button", "mdi:remote"),
    ]
}

/// What to do with a request from HA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Run `ctl.sh`.
    Run,
    /// The state asked for is the one in force.
    NoChange,
    /// Not now; the reason is for the log.
    Refuse(&'static str),
}

/// The daemon's own guards, in order: no change needed, one in flight, busy.
/// The cooldown is not here: `ctl.sh config` owns it.
pub(crate) fn decide(current: bool, want: bool, busy: Option<Busy>, in_flight: bool) -> Decision {
    if current == want {
        return Decision::NoChange;
    }
    if in_flight {
        return Decision::Refuse("a change is still running");
    }
    match busy {
        Some(b) => Decision::Refuse(b.reason()),
        None => Decision::Run,
    }
}

/// The string value of `"key":"..."` in one flat JSON line as `ctl.sh`
/// writes it, cut to 160 printable ASCII characters. Nothing else of the
/// text is used.
fn json_field(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let rest = &text[text.find(&pat)? + pat.len()..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => {
                // An escape: keep a space for it, skip its character.
                chars.next();
                out.push(' ');
            }
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            _ => out.push('?'),
        }
        if out.len() >= 160 {
            break;
        }
    }
    Some(out)
}

/// What `ctl.sh` reported, from its JSON line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) result: String,
    pub(crate) daemon: String,
    pub(crate) error: String,
    /// `cooldown` or `busy` for a refusal, else empty (an older `ctl.sh`
    /// has no such field).
    pub(crate) reason: String,
}

/// Reads the `result`, `daemon`, `error` and `reason` fields; `None` without `result`.
pub(crate) fn parse_outcome(text: &str) -> Option<Outcome> {
    Some(Outcome {
        result: json_field(text, "result")?,
        daemon: json_field(text, "daemon").unwrap_or_default(),
        error: json_field(text, "error").unwrap_or_default(),
        reason: json_field(text, "reason").unwrap_or_default(),
    })
}

impl Outcome {
    /// One log phrase that says why this process was not restarted.
    pub(crate) fn describe(&self) -> String {
        let why = match (self.result.as_str(), self.daemon.as_str()) {
            ("refused", _) if self.reason == "cooldown" => {
                "ctl.sh refused the change: the cooldown of the last one is still running"
            }
            ("refused", _) if self.reason == "busy" => {
                "ctl.sh refused the change: the satellite is busy"
            }
            ("already", _) => {
                "the config already held that value but this process runs with the other: restart the satellite to apply it"
            }
            ("applied", "not running") => {
                "the config was changed but ctl.sh found no running satellite to restart (not started by start.sh?): restart it to apply"
            }
            ("applied", _) => "the config was changed but the satellite was not restarted",
            ("failed", _) => "the change failed",
            ("refused", _) => "ctl.sh refused the change",
            _ => "unexpected result",
        };
        if self.error.is_empty() {
            format!("result={}, daemon={}: {why}", self.result, self.daemon)
        } else {
            format!(
                "result={}, daemon={}, error={:?}: {why}",
                self.result, self.daemon, self.error
            )
        }
    }
}

/// Reads (at most [`OUT_MAX`] bytes) and removes the output file of a
/// `ctl.sh`. `finished`: that script has exited. Otherwise (`false`) the
/// script may still be running (it outlives the daemon it restarted), so an
/// empty file is left alone for it to fill.
fn take_outcome(path: &Path, finished: bool) -> Option<Outcome> {
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path)
        .and_then(|f| f.take(OUT_MAX).read_to_end(&mut bytes))
        .is_ok();
    if finished || !bytes.is_empty() {
        let _ = std::fs::remove_file(path);
    }
    if !read {
        return None;
    }
    parse_outcome(&String::from_utf8_lossy(&bytes))
}

/// How often the output file of a `ctl.sh` that restarted this process is
/// looked at, and how many times (about a minute in all: a rollback with a
/// second start takes up to about 40 s).
const LAST_CHANGE_EVERY: Duration = Duration::from_secs(5);
const LAST_CHANGE_TRIES: u32 = 12;

/// Waits `every`, then reads the output file; repeats while the file exists
/// but holds no result yet (the script has not printed it), at most `tries`
/// times. The file is removed once it holds something.
async fn log_last_change(path: &Path, every: Duration, tries: u32) -> Option<Outcome> {
    for _ in 0..tries {
        tokio::time::sleep(every).await;
        if !path.exists() {
            return None;
        }
        if let Some(o) = take_outcome(path, false) {
            return Some(o);
        }
    }
    None
}

/// The switches' state in this process and the guard state.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // one bool per switch state
pub(crate) struct ConfigSwitches {
    /// The install directory (`WEBOS_SAT_PREFIX` of `ctl.sh`).
    prefix: PathBuf,
    /// `bin/ctl.sh` in it.
    ctl: PathBuf,
    /// Where the output file of `ctl.sh` lives.
    state_dir: PathBuf,
    aec: bool,
    music: bool,
    /// `mic_button = "ptt"` in the config this process started with.
    mic_button: bool,
    /// The feature runs (LG's voice key taken over): what HA's switch shows.
    mic_button_active: bool,
    in_flight: Arc<AtomicBool>,
    slow_after: Duration,
}

impl ConfigSwitches {
    /// `Some` when `bin/ctl.sh` of the install exists (the settings know the
    /// install only for a config in an `etc/` directory). Without it the
    /// entities are not listed. The result of an earlier `ctl.sh` is logged
    /// by [`ConfigSwitches::spawn_last_change_log`], not here: `ctl.sh`
    /// writes its line after it has started this process.
    pub(crate) fn new(settings: &Settings) -> Option<Self> {
        let prefix = settings.install_prefix.as_ref()?;
        let ctl = prefix.join("bin").join("ctl.sh");
        if !ctl.is_file() {
            log::info!(
                "config switches: {} not found; the aec_enabled, music_enabled and remote_mic_button switches are not offered",
                ctl.display()
            );
            return None;
        }
        let state_dir = settings.report.dir_or_default();
        Some(Self {
            prefix: prefix.clone(),
            ctl,
            state_dir,
            aec: settings.aec.enabled,
            music: settings.music.enabled,
            mic_button: settings.remote.ptt(),
            mic_button_active: false,
            in_flight: Arc::new(AtomicBool::new(false)),
            slow_after: SLOW_AFTER,
        })
    }

    /// A fixture for the daemon's tests: a script that does not exist, so a
    /// request that passes the guards cannot start anything useful.
    #[cfg(test)]
    pub(crate) fn for_test(aec: bool, music: bool) -> Self {
        Self::fixture(
            aec,
            music,
            Path::new("/nonexistent"),
            &std::env::temp_dir().join(format!("webos-sat-cfgsw-fixture-{}", std::process::id())),
        )
    }

    /// A fixture for tests: `prefix/bin/ctl.sh` need not exist; `state_dir`
    /// is where the output file goes.
    #[cfg(test)]
    pub(crate) fn fixture(aec: bool, music: bool, prefix: &Path, state_dir: &Path) -> Self {
        Self {
            prefix: prefix.to_path_buf(),
            ctl: prefix.join("bin").join("ctl.sh"),
            state_dir: state_dir.to_path_buf(),
            aec,
            music,
            mic_button: false,
            mic_button_active: false,
            in_flight: Arc::new(AtomicBool::new(false)),
            slow_after: SLOW_AFTER,
        }
    }

    /// The mic button as it runs: `active` only when the feature started
    /// with LG's voice key taken over. Call before the first states go out.
    pub(crate) fn set_mic_button_active(&mut self, active: bool) {
        self.mic_button_active = active;
    }

    /// When this process is the restarted one, logs what the `ctl.sh` that
    /// restarted it reported. That script prints its line only after
    /// `start.sh` has returned, so the file is read after a delay and polled
    /// (every [`LAST_CHANGE_EVERY`], at most [`LAST_CHANGE_TRIES`] times)
    /// until a line is there. Needs the tokio runtime.
    pub(crate) fn spawn_last_change_log(&self) {
        let path = self.state_dir.join(OUT_FILE);
        tokio::spawn(async move {
            if let Some(o) = log_last_change(&path, LAST_CHANGE_EVERY, LAST_CHANGE_TRIES).await {
                log::info!(
                    "config switches: the change that restarted the satellite, by ctl.sh: result={}, daemon={}",
                    o.result,
                    o.daemon
                );
            }
        });
    }

    /// The state HA's switch shows: the config value, except for the mic
    /// button, which is on only while it runs.
    pub(crate) fn current(&self, setting: Setting) -> bool {
        match setting {
            Setting::Aec => self.aec,
            Setting::Music => self.music,
            Setting::MicButton => self.mic_button_active,
        }
    }

    /// The state a request is compared with: for the mic button, an "on"
    /// is compared with what runs (a takeover that failed can be retried:
    /// `ctl.sh config` restarts the daemon), an "off" with the config (a
    /// failed takeover still has `ptt` in the config, which an "off" removes).
    fn compared(&self, setting: Setting, want: bool) -> bool {
        match setting {
            Setting::MicButton if !want => self.mic_button,
            _ => self.current(setting),
        }
    }

    /// The states from the config this process started with.
    pub(crate) fn initial_commands(&self) -> Vec<AppCommand> {
        ALL.into_iter()
            .map(|s| AppCommand::SetSwitch {
                object_id: s.object_id().to_owned(),
                state: Some(self.current(s)),
            })
            .collect()
    }

    /// Handles HA's request. Returns a command for the server when HA's
    /// switch must be sent the real state again. The cooldown and the final
    /// busy check are `ctl.sh`'s; a refusal it reports comes back through the
    /// reaper of [`spawn_ctl`].
    pub(crate) fn on_request(
        &mut self,
        setting: Setting,
        want: bool,
        busy: Option<Busy>,
        cmd_tx: &mpsc::Sender<AppCommand>,
    ) -> Option<AppCommand> {
        let id = setting.object_id();
        let resend = || AppCommand::ResendSwitch {
            object_id: id.to_owned(),
        };
        let decision = decide(
            self.compared(setting, want),
            want,
            busy,
            self.in_flight.load(Ordering::Relaxed),
        );
        match decision {
            Decision::NoChange => {
                log::info!("config switch {id}: already in that state; nothing to do");
                None
            }
            Decision::Refuse(why) => {
                log::warn!("config switch {id}: refused ({why})");
                Some(resend())
            }
            Decision::Run => match spawn_ctl(self, setting, want, cmd_tx) {
                Ok(()) => {
                    log::info!(
                        "config switch {id}: changing to {}; the satellite restarts",
                        if want { "on" } else { "off" }
                    );
                    None
                }
                Err(e) => {
                    log::warn!("config switch {id}: cannot start ctl.sh ({e})");
                    Some(resend())
                }
            },
        }
    }
}

/// The command that runs `ctl.sh config ...` in a session of its own, with
/// an environment of three variables: nothing is inherited (not
/// `WEBOS_SAT_LOCK_HELD`, which would make `ctl.sh` skip the install lock).
fn ctl_command(c: &ConfigSwitches, setting: Setting, on: bool) -> Command {
    let mut cmd = Command::new("sh");
    cmd.env_clear()
        .env("PATH", CTL_PATH)
        .env("WEBOS_SAT_PREFIX", &c.prefix)
        .env("WEBOS_SAT_SOURCE", "ha")
        .arg("-c")
        .arg(DETACH)
        .arg("webos-satellite-config")
        .arg(&c.ctl)
        .args(ctl_args(setting, on))
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// Starts `ctl.sh config ...`. A task reaps it and, if this process is still
/// alive afterwards (the change did not restart it), logs what `ctl.sh`
/// reported and asks the server to send the stored state again. The change
/// stays in flight until the script exits.
fn spawn_ctl(
    c: &ConfigSwitches,
    setting: Setting,
    on: bool,
    cmd_tx: &mpsc::Sender<AppCommand>,
) -> std::io::Result<()> {
    // The JSON line goes to a file, not a pipe: the restart ends this process
    // and a closed pipe would kill the script at its last line.
    let out_path = c.state_dir.join(OUT_FILE);
    let out = open_out(&out_path);
    let mut cmd = ctl_command(c, setting, on);
    cmd.stdout(out.map_or_else(
        |e| {
            log::debug!("config switch: no output file ({e}); ctl.sh's result is not logged");
            Stdio::null()
        },
        Stdio::from,
    ));
    let mut child = cmd.spawn()?;
    c.in_flight.store(true, Ordering::Relaxed);
    let flag = Arc::clone(&c.in_flight);
    let tx = cmd_tx.clone();
    let id = setting.object_id();
    let slow = c.slow_after;
    tokio::spawn(async move {
        let status = loop {
            match tokio::time::timeout(slow, child.wait()).await {
                Ok(r) => break r,
                Err(_) => log::warn!(
                    "config switch {id}: ctl.sh still running after {} s; waiting for it",
                    slow.as_secs()
                ),
            }
        };
        let word = match status {
            Ok(st) if st.success() => "ctl.sh finished",
            Ok(_) => "ctl.sh failed",
            Err(_) => "ctl.sh not waited for",
        };
        // Reached only if the change did not restart this process.
        match take_outcome(&out_path, true) {
            Some(o) => log::warn!(
                "config switch {id}: {word}; the satellite was not restarted ({})",
                o.describe()
            ),
            None => log::warn!(
                "config switch {id}: {word}; the satellite was not restarted (no result line)"
            ),
        }
        flag.store(false, Ordering::Relaxed);
        let _ = tx
            .send(AppCommand::ResendSwitch {
                object_id: id.to_owned(),
            })
            .await;
    });
    Ok(())
}

/// Creates the (empty) output file, mode 0600.
fn open_out(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn only_the_known_object_ids_map_to_a_setting() {
        assert_eq!(Setting::from_object_id("aec_enabled"), Some(Setting::Aec));
        assert_eq!(
            Setting::from_object_id("music_enabled"),
            Some(Setting::Music)
        );
        assert_eq!(
            Setting::from_object_id("remote_mic_button"),
            Some(Setting::MicButton)
        );
        for id in [
            "",
            "aec",
            "AEC_ENABLED",
            "aec_enabled ",
            "mute_microphone",
            "tv_screen",
            "aec_enabled; reboot",
            "../aec_enabled",
            "remote_ptt_enabled",
            "mic-button",
        ] {
            assert_eq!(Setting::from_object_id(id), None, "{id:?}");
        }
    }

    #[test]
    fn ctl_arguments_are_fixed_literals() {
        assert_eq!(ctl_args(Setting::Aec, true), ["config", "aec", "on"]);
        assert_eq!(ctl_args(Setting::Aec, false), ["config", "aec", "off"]);
        assert_eq!(ctl_args(Setting::Music, true), ["config", "music", "on"]);
        assert_eq!(ctl_args(Setting::Music, false), ["config", "music", "off"]);
        assert_eq!(
            ctl_args(Setting::MicButton, true),
            ["config", "mic-button", "on"]
        );
        assert_eq!(
            ctl_args(Setting::MicButton, false),
            ["config", "mic-button", "off"]
        );
        // The object ids map back to their own words.
        for s in ALL {
            assert_eq!(Setting::from_object_id(s.object_id()), Some(s));
        }
    }

    #[test]
    fn the_detach_text_holds_no_data() {
        // Everything variable arrives as "$@"; the text has no other
        // expansion and no command substitution.
        assert_eq!(DETACH.matches('$').count(), 2);
        assert_eq!(DETACH.matches("\"$@\"").count(), 2);
        assert!(!DETACH.contains('`'));
        assert!(!DETACH.contains("$("));
    }

    #[test]
    fn entities_are_config_switches_with_the_known_ids() {
        let e = entities();
        let ids: Vec<_> = e.iter().map(|s| s.object_id.as_str()).collect();
        assert_eq!(ids, [AEC_ID, MUSIC_ID, MIC_BUTTON_ID]);
        assert_eq!(e[2].name, "Remote mic button");
        assert!(e.iter().all(|s| s.entity_category == EntityCategory::Config
            && !s.name.is_empty()
            && s.icon.starts_with("mdi:")));
        // They may not clash with the built-in mute switch.
        assert!(!ids.contains(&webos_esphome_api::entities::MUTE_OBJECT_ID));
    }

    #[test]
    fn no_change_needed_does_nothing_even_when_busy() {
        assert_eq!(
            decide(true, true, Some(Busy::Voice), true),
            Decision::NoChange
        );
        assert_eq!(decide(false, false, None, false), Decision::NoChange);
    }

    #[test]
    fn an_idle_change_runs() {
        assert_eq!(decide(true, false, None, false), Decision::Run);
        assert_eq!(decide(false, true, None, false), Decision::Run);
    }

    #[test]
    fn busy_refuses_each_kind_alone_with_its_reason() {
        for b in [Busy::Voice, Busy::Music, Busy::Alarm] {
            assert_eq!(
                decide(true, false, Some(b), false),
                Decision::Refuse(b.reason()),
                "{b:?}"
            );
        }
    }

    #[test]
    fn one_in_flight_and_no_cooldown_of_its_own() {
        assert!(matches!(
            decide(true, false, None, true),
            Decision::Refuse(_)
        ));
        // In flight comes before busy.
        assert_eq!(
            decide(true, false, Some(Busy::Music), true),
            Decision::Refuse("a change is still running")
        );
        // Back to back requests are not limited here: the cooldown is in
        // ctl.sh, which reports it.
        assert_eq!(decide(true, false, None, false), Decision::Run);
        assert_eq!(decide(true, false, None, false), Decision::Run);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webos-sat-cfgsw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_result_line_is_read_field_by_field() {
        let line = r#"{"schema":1,"setting":"aec","requested":"on","before":"off","after":"on","result":"applied","daemon":"not running","error":""}"#;
        let o = parse_outcome(line).unwrap();
        assert_eq!(o.result, "applied");
        assert_eq!(o.daemon, "not running");
        assert_eq!(o.error, "");
        assert!(o.describe().contains("no running satellite"));
        assert_eq!(o.reason, "");
        let o = parse_outcome(r#"{"result":"already","daemon":"none"}"#).unwrap();
        assert!(o.describe().contains("restart the satellite"));
        // An error text is cut, stripped of control characters and escapes.
        let long = format!(
            "{{\"result\":\"failed\",\"daemon\":\"none\",\"error\":\"a\\\"b\\nc {}\"}}",
            "x".repeat(500)
        );
        let o = parse_outcome(&long).unwrap();
        assert!(
            o.error.len() <= 161 && o.error.starts_with("a b c x"),
            "{o:?}"
        );
        let o = parse_outcome(
            r#"{"result":"refused","daemon":"none","error":"the last change was 5 s ago; try again in 25 s","reason":"cooldown","retry_in_s":25}"#,
        )
        .unwrap();
        assert_eq!(o.reason, "cooldown");
        assert!(o.describe().contains("cooldown"), "{}", o.describe());
        let o = parse_outcome(r#"{"result":"refused","daemon":"none","reason":"busy"}"#).unwrap();
        assert!(o.describe().contains("busy"), "{}", o.describe());
        assert!(parse_outcome("no json here").is_none());
        assert!(parse_outcome("").is_none());
    }

    fn switches(aec: bool, music: bool) -> ConfigSwitches {
        ConfigSwitches::for_test(aec, music)
    }

    #[test]
    fn initial_states_come_from_the_settings() {
        let cmds = switches(true, false).initial_commands();
        assert_eq!(
            cmds,
            [
                AppCommand::SetSwitch {
                    object_id: AEC_ID.into(),
                    state: Some(true)
                },
                AppCommand::SetSwitch {
                    object_id: MUSIC_ID.into(),
                    state: Some(false)
                },
                AppCommand::SetSwitch {
                    object_id: MIC_BUTTON_ID.into(),
                    state: Some(false)
                },
            ]
        );
    }

    #[test]
    fn new_needs_the_script_and_a_known_place() {
        let mut s = Settings::default();
        assert!(ConfigSwitches::new(&s).is_none(), "no place known");
        s.install_prefix = Some(PathBuf::from("/nonexistent"));
        assert!(ConfigSwitches::new(&s).is_none(), "file missing");
        let dir = tmp("new");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("ctl.sh"), "#!/bin/sh\n").unwrap();
        s.install_prefix = Some(dir.clone());
        s.report.dir = Some(dir.join("run"));
        s.aec.enabled = false;
        s.music.enabled = true;
        s.remote.mic_button = crate::config::MicButton::Ptt;
        let c = ConfigSwitches::new(&s).unwrap();
        assert!(!c.current(Setting::Aec) && c.current(Setting::Music));
        // Configured, but HA sees it on only once it runs.
        assert!(!c.current(Setting::MicButton));
        let mut c = c;
        c.set_mic_button_active(true);
        assert!(c.current(Setting::MicButton));
        assert_eq!(c.prefix, dir);
        assert_eq!(c.ctl, dir.join("bin").join("ctl.sh"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_failed_mic_button_takeover_can_be_retried_and_turned_off() {
        let d = tmp("micbtn");
        let (tx, _rx) = mpsc::channel(4);
        // Configured ptt, but the takeover failed: HA shows off.
        let mut c = ConfigSwitches::fixture(true, false, &d, &d.join("run"));
        c.mic_button = true;
        c.set_mic_button_active(false);
        assert_eq!(
            c.initial_commands()[2],
            AppCommand::SetSwitch {
                object_id: MIC_BUTTON_ID.into(),
                state: Some(false)
            }
        );
        // "on" is compared with what runs: a retry is a change.
        assert!(!c.compared(Setting::MicButton, true));
        // "off" is compared with the config: ptt there, so a change too.
        assert!(c.compared(Setting::MicButton, false));
        // Not configured and not running: "off" changes nothing.
        c.mic_button = false;
        assert!(c.on_request(Setting::MicButton, false, None, &tx).is_none());
        assert!(!c.in_flight.load(Ordering::Relaxed));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn refusals_resend_the_state_and_start_nothing() {
        let d = tmp("refuse");
        let (tx, _rx) = mpsc::channel(4);
        let mut c = ConfigSwitches::fixture(true, false, &d, &d.join("run"));
        let resend = Some(AppCommand::ResendSwitch {
            object_id: AEC_ID.into(),
        });
        assert_eq!(
            c.on_request(Setting::Aec, false, Some(Busy::Music), &tx),
            resend
        );
        assert!(!c.in_flight.load(Ordering::Relaxed));
        // Same state: no answer needed.
        assert_eq!(c.on_request(Setting::Aec, true, None, &tx), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An install directory with a `bin/ctl.sh` of the given text.
    fn install(name: &str, script: &str) -> PathBuf {
        let d = tmp(name);
        std::fs::create_dir_all(d.join("bin")).unwrap();
        std::fs::write(d.join("bin").join("ctl.sh"), script).unwrap();
        d
    }

    async fn resend_arrives(rx: &mut mpsc::Receiver<AppCommand>, id: &str) {
        let got = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap();
        assert_eq!(
            got,
            Some(AppCommand::ResendSwitch {
                object_id: id.into()
            })
        );
    }

    #[tokio::test]
    async fn an_accepted_request_runs_the_script_and_waits_for_it() {
        let d = install(
            "run",
            "#!/bin/sh\necho \"$@\" > \"$WEBOS_SAT_PREFIX/args\"\nsleep 1\nprintf '{\"result\":\"failed\",\"daemon\":\"none\",\"error\":\"boom\"}\\n'\nexit 3\n",
        );
        let (tx, mut rx) = mpsc::channel(4);
        let mut c = ConfigSwitches::fixture(true, false, &d, &d.join("run"));
        let started = std::time::Instant::now();
        let r = c.on_request(Setting::Music, true, None, &tx);
        assert_eq!(r, None);
        // The script sleeps: the change stays in flight and nothing is sent
        // until it has exited (a second request is refused for that, among
        // other reasons).
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(c.in_flight.load(Ordering::Relaxed));
        assert!(rx.try_recv().is_err());
        // It exited non-zero while this process lives: the state is sent
        // again, and not before the script was done.
        resend_arrives(&mut rx, MUSIC_ID).await;
        assert!(
            started.elapsed() >= Duration::from_millis(1000),
            "resent after {:?}",
            started.elapsed()
        );
        assert!(!c.in_flight.load(Ordering::Relaxed));
        assert_eq!(
            std::fs::read_to_string(d.join("args")).unwrap().trim(),
            "config music on"
        );
        // Its output file was consumed.
        assert!(!d.join("run").join(OUT_FILE).exists());
        // The daemon has no cooldown of its own: a second request now
        // starts the script again (ctl.sh refuses it when it is too soon).
        assert_eq!(c.on_request(Setting::Music, true, None, &tx), None);
        resend_arrives(&mut rx, MUSIC_ID).await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_slow_script_stays_in_flight_past_the_warning_time() {
        let d = install("slow", "#!/bin/sh\nsleep 1\nexit 0\n");
        let (tx, mut rx) = mpsc::channel(4);
        let mut c = ConfigSwitches::fixture(true, false, &d, &d.join("run"));
        c.slow_after = Duration::from_millis(200);
        assert_eq!(c.on_request(Setting::Aec, false, None, &tx), None);
        // Well past the warning time, the script still runs: still in flight,
        // so no second one can start.
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(c.in_flight.load(Ordering::Relaxed));
        assert!(rx.try_recv().is_err());
        let r = c.on_request(Setting::Music, true, None, &tx);
        assert!(r.is_some(), "refused while one runs");
        resend_arrives(&mut rx, AEC_ID).await;
        assert!(!c.in_flight.load(Ordering::Relaxed));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn the_script_gets_a_clean_environment() {
        // The test process has CARGO_* and HOME set; the script must see
        // none of what this process inherited, WEBOS_SAT_LOCK_HELD least of
        // all (it would make ctl.sh skip the install lock).
        let d = install(
            "env",
            "#!/bin/sh\nenv > \"$WEBOS_SAT_PREFIX/env\"\nexit 1\n",
        );
        let (tx, mut rx) = mpsc::channel(4);
        let mut c = ConfigSwitches::fixture(true, false, &d, &d.join("run"));
        assert_eq!(c.on_request(Setting::Aec, false, None, &tx), None);
        resend_arrives(&mut rx, AEC_ID).await;
        let env = std::fs::read_to_string(d.join("env")).unwrap();
        let names: Vec<&str> = env
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, _)| k))
            .collect();
        assert!(!names.contains(&"WEBOS_SAT_LOCK_HELD"), "{names:?}");
        assert!(!names.contains(&"WEBOS_SAT_LOCK"), "{names:?}");
        assert!(!names.contains(&"HOME"), "{names:?}");
        assert!(
            !names.iter().any(|n| n.starts_with("CARGO")),
            "inherited: {names:?}"
        );
        let get = |k: &str| {
            env.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .map(str::to_owned)
        };
        assert_eq!(get("WEBOS_SAT_PREFIX"), Some(d.display().to_string()));
        assert_eq!(get("WEBOS_SAT_SOURCE").as_deref(), Some("ha"));
        assert_eq!(get("PATH").as_deref(), Some(CTL_PATH));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_refusal_printed_by_the_script_resends_the_state() {
        // The one line the script prints, also the text parsed below.
        const LINE: &str = r#"{"schema":1,"result":"refused","daemon":"none","error":"the last change was 5 s ago; try again in 25 s","reason":"cooldown","retry_in_s":25}"#;
        let d = install(
            "ctl-refuses",
            &format!("#!/bin/sh\ncat <<'EOF'\n{LINE}\nEOF\nexit 1\n"),
        );
        let (tx, mut rx) = mpsc::channel(4);
        let mut c = ConfigSwitches::fixture(true, false, &d, &d.join("run"));
        assert_eq!(c.on_request(Setting::Aec, false, None, &tx), None);
        resend_arrives(&mut rx, AEC_ID).await;
        assert!(!c.in_flight.load(Ordering::Relaxed));
        // What the reaper read from the file (consumed now) is what describe
        // names: run the same parse on the same text.
        let o = parse_outcome(LINE).unwrap();
        assert_eq!(o.reason, "cooldown");
        assert!(o.describe().contains("cooldown"));
        assert!(!d.join("run").join(OUT_FILE).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn the_last_change_is_read_once_the_script_has_printed_it() {
        let d = tmp("last-change");
        let path = d.join(OUT_FILE);
        // Nothing there: nothing to log.
        let none = log_last_change(&path, Duration::from_millis(5), 3).await;
        assert_eq!(none, None);
        // An empty file (the script is still running) is left alone and
        // polled; the line arrives later.
        std::fs::write(&path, "").unwrap();
        let p2 = path.clone();
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            std::fs::write(
                &p2,
                "{\"schema\":1,\"result\":\"applied\",\"daemon\":\"restarted\",\"error\":\"\"}\n",
            )
            .unwrap();
        });
        let o = log_last_change(&path, Duration::from_millis(10), 50)
            .await
            .unwrap();
        writer.await.unwrap();
        assert_eq!(
            (o.result.as_str(), o.daemon.as_str()),
            ("applied", "restarted")
        );
        assert!(!path.exists(), "consumed");
        // A file that stays empty is not consumed and gives up.
        std::fs::write(&path, "").unwrap();
        assert_eq!(
            log_last_change(&path, Duration::from_millis(5), 3).await,
            None
        );
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
