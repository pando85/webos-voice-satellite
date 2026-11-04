//! Firmware status (ADR 0006): reads the TV's firmware version and says
//! whether it, with the model, is in the verified list ([`crate::verified`])
//! or in the owner's `safety.validated_firmware`. A status, not a gate: only
//! `safety.firmware_policy = "strict"` turns "not verified" into a closed
//! microphone. Also the firmware-changed marker (`firmware_seen_file`).
//!
//! Source (read-only, verified on c3-a): `/var/run/nyx/os_info.json`, a flat
//! JSON object that nyx writes at boot on a tmpfs. Its
//! `webos_manufacturing_version` is the LG firmware string (`03.30.74` on
//! c3-a, the same value `getSystemInfo` reports as `firmwareVersion`).

use std::fmt;
use std::path::Path;

use crate::config::{FirmwarePolicy, Settings};
use crate::verified::{self, Standing};

/// Key of the firmware version in `os_info.json`.
const VERSION_KEY: &str = "webos_manufacturing_version";

/// What is known about the running firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FirmwareInfo {
    /// LG firmware version (`webos_manufacturing_version`).
    pub(crate) version: String,
    /// webOS release (`webos_release`), informational.
    pub(crate) release: Option<String>,
    /// Build timestamp (`webos_build_datetime`), informational.
    pub(crate) build: Option<String>,
}

impl fmt::Display for FirmwareInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "firmware {} (webOS {}, build {})",
            self.version,
            self.release.as_deref().unwrap_or("?"),
            self.build.as_deref().unwrap_or("?")
        )
    }
}

/// What is known about the model when a verdict is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Model<'a> {
    /// The model name the TV reports.
    Known(&'a str),
    /// The daemon asked and the TV did not answer.
    NotRead,
    /// Not asked (`--check-firmware`): the firmware alone decides.
    NotChecked,
}

/// Result of the check (ADR 0006): a status, never a block by itself. Only
/// `firmware_policy = "strict"` turns it into a gate ([`Verdict::blocks`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Model and firmware are verified (why), or the firmware is in
    /// `safety.validated_firmware`.
    Verified(FirmwareInfo, String),
    /// The series is verified; this firmware is not yet reported.
    SeriesOnly(FirmwareInfo, String),
    /// Neither the model series nor the firmware is listed. `strict`: the
    /// gate is on.
    NotVerified { info: FirmwareInfo, strict: bool },
    /// The firmware or the model could not be read. `strict`: the gate is on.
    Unreadable { why: String, strict: bool },
    /// Strict mode with an empty `validated_firmware`: the gate is off.
    Off,
}

impl Verdict {
    /// True if the microphone must stay off: only in strict mode, on a
    /// firmware that is not in the list or cannot be read.
    pub(crate) fn blocks(&self) -> bool {
        matches!(
            self,
            Self::NotVerified { strict: true, .. } | Self::Unreadable { strict: true, .. }
        )
    }

    /// Exit code of `--check-firmware`: 0 verified, 1 not verified or
    /// unreadable (informational unless strict), 3 strict gate off.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Verified(..) => 0,
            Self::SeriesOnly(..) | Self::NotVerified { .. } | Self::Unreadable { .. } => 1,
            Self::Off => 3,
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Verified(i, why) => write!(f, "{i}: verified ({why})"),
            Self::SeriesOnly(i, series) => write!(
                f,
                "{i}: not verified: {series} is a verified series, but this firmware is not yet reported; the microphone starts anyway"
            ),
            Self::NotVerified { info, strict: true } => write!(
                f,
                "{info}: NOT in safety.validated_firmware (firmware_policy = \"strict\"); the microphone stays disabled until this firmware is validated and added"
            ),
            Self::NotVerified {
                info,
                strict: false,
            } => write!(
                f,
                "{info}: not verified; the microphone starts anyway (report if it does not work)"
            ),
            Self::Unreadable { why, strict: true } => write!(
                f,
                "cannot read the firmware version ({why}); firmware_policy = \"strict\": the microphone stays disabled"
            ),
            Self::Unreadable { why, strict: false } => write!(
                f,
                "cannot read the firmware version ({why}): unknown; the microphone starts anyway"
            ),
            Self::Off => f.write_str(
                "firmware_policy = \"strict\" with an empty safety.validated_firmware: the gate is off",
            ),
        }
    }
}

/// The string value of `key` in a flat JSON object, or `None`. Handles
/// whitespace and backslash escapes of `"` and `\`; enough for nyx's file.
pub(crate) fn json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut rest = text;
    while let Some(i) = rest.find(&needle) {
        rest = &rest[i + needle.len()..];
        let after = rest.trim_start();
        let Some(after) = after.strip_prefix(':') else {
            continue;
        };
        let after = after.trim_start();
        let after = after.strip_prefix('"')?;
        let mut out = String::new();
        let mut chars = after.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => out.push(chars.next()?),
                c => out.push(c),
            }
        }
        return None;
    }
    None
}

/// Parses the nyx OS information.
pub(crate) fn parse(text: &str) -> Result<FirmwareInfo, String> {
    let version = json_string(text, VERSION_KEY)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("no {VERSION_KEY}"))?;
    Ok(FirmwareInfo {
        version: version.trim().to_owned(),
        release: json_string(text, "webos_release"),
        build: json_string(text, "webos_build_datetime"),
    })
}

/// Reads the firmware information from `path`.
pub(crate) fn read(path: &Path) -> Result<FirmwareInfo, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Decides the verdict for `settings` from an already read `info` and `model`.
pub(crate) fn evaluate(
    settings: &Settings,
    info: Result<FirmwareInfo, String>,
    model: Model<'_>,
) -> Verdict {
    let owner = &settings.safety.validated_firmware;
    let strict = settings.safety.firmware_policy == FirmwarePolicy::Strict;
    if strict {
        // The old gate: only the owner's list counts.
        if owner.is_empty() {
            return Verdict::Off;
        }
        return match info {
            Ok(i) if owner.iter().any(|v| v.trim() == i.version) => {
                Verdict::Verified(i, "in safety.validated_firmware".to_owned())
            }
            Ok(info) => Verdict::NotVerified { info, strict },
            Err(why) => Verdict::Unreadable { why, strict },
        };
    }
    let i = match info {
        Ok(i) => i,
        Err(why) => return Verdict::Unreadable { why, strict },
    };
    if owner.iter().any(|v| v.trim() == i.version) {
        return Verdict::Verified(i, "in safety.validated_firmware".to_owned());
    }
    match model {
        Model::Known(m) => match verified::lookup(m, i.release.as_deref(), &i.version) {
            Standing::Verified(series) => Verdict::Verified(i, series),
            Standing::SeriesOnly(series) => Verdict::SeriesOnly(i, series),
            Standing::Unlisted => Verdict::NotVerified { info: i, strict },
        },
        Model::NotChecked => {
            if verified::firmware_listed(&i.version) {
                Verdict::Verified(
                    i,
                    "firmware is in the verified list; model not checked".to_owned(),
                )
            } else {
                Verdict::NotVerified { info: i, strict }
            }
        }
        Model::NotRead => Verdict::Unreadable {
            why: "the model name was not read".to_owned(),
            strict,
        },
    }
}

/// Reads the firmware and runs the check without a model (`--check-firmware`
/// and the strict gate).
pub(crate) fn check(settings: &Settings) -> Verdict {
    evaluate(
        settings,
        read(&settings.safety.firmware_info),
        Model::NotChecked,
    )
}

/// What `safety.firmware_seen_file` says about the current firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seen {
    /// No usable file yet: first start on this install.
    First,
    /// Same version as last time.
    Same,
    /// Another version; the old one (sanitized).
    Changed(String),
}

/// Keeps what looks like a version string, at most 40 characters.
fn tidy(v: &str) -> String {
    v.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(40)
        .collect()
}

/// Compares `current` with the stored version.
pub(crate) fn check_seen(path: &Path, current: &str) -> Seen {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Seen::First;
    };
    let old = tidy(text.lines().next().unwrap_or("").trim());
    if old.is_empty() {
        Seen::First
    } else if old == tidy(current) {
        Seen::Same
    } else {
        Seen::Changed(old)
    }
}

/// Stores `version` as the firmware the daemon runs on (atomic replace).
pub(crate) fn write_seen(path: &Path, version: &str) -> std::io::Result<()> {
    crate::volume::write_atomic(path, &format!("{}\n", tidy(version)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Shape of c3-a's file (values from docs/discovery/c3-a/D0-D1-runtime.md).
    const OS_INFO: &str = r#"{
    "core_os_kernel_version": "5.4.96-307",
    "webos_build_datetime": "20240226022140",
    "webos_manufacturing_version": "03.30.74",
    "webos_name": "webOS TV",
    "webos_prerelease": "",
    "webos_release": "8.3.0"
}"#;

    #[test]
    fn parses_nyx_os_info() {
        let i = parse(OS_INFO).unwrap();
        assert_eq!(i.version, "03.30.74");
        assert_eq!(i.release.as_deref(), Some("8.3.0"));
        assert_eq!(i.build.as_deref(), Some("20240226022140"));
        assert_eq!(
            i.to_string(),
            "firmware 03.30.74 (webOS 8.3.0, build 20240226022140)"
        );
    }

    #[test]
    fn json_string_edge_cases() {
        assert_eq!(json_string(r#"{"a":"x\"y"}"#, "a").as_deref(), Some("x\"y"));
        assert_eq!(json_string(r#"{"a" : "1"}"#, "a").as_deref(), Some("1"));
        // A key that appears as a value first is skipped.
        assert_eq!(
            json_string(r#"{"b":"a","a":"2"}"#, "a").as_deref(),
            Some("2")
        );
        assert_eq!(json_string(r#"{"a":3}"#, "a"), None);
        assert_eq!(json_string(r#"{"a":"unterminated"#, "a"), None);
        assert!(parse(r#"{"webos_manufacturing_version": ""}"#).is_err());
        assert!(parse("{}").is_err());
    }

    fn info(version: &str, release: &str) -> FirmwareInfo {
        FirmwareInfo {
            version: version.to_owned(),
            release: Some(release.to_owned()),
            build: None,
        }
    }

    fn settings(strict: bool, owner: &[&str]) -> Settings {
        let mut s = Settings::default();
        s.safety.firmware_policy = if strict {
            FirmwarePolicy::Strict
        } else {
            FirmwarePolicy::Warn
        };
        s.safety.validated_firmware = owner.iter().map(|v| (*v).to_owned()).collect();
        s
    }

    /// Each row of the behaviour table of design section 2.1 that concerns
    /// the status: the microphone is never blocked in warn mode.
    #[test]
    fn warn_mode_never_blocks() {
        let s = settings(false, &[]);
        // Both tested units.
        let v = evaluate(
            &s,
            Ok(info("03.30.74", "8.3.0")),
            Model::Known("OLED42C34LA"),
        );
        assert!(matches!(v, Verdict::Verified(..)) && !v.blocks());
        assert_eq!(v.exit_code(), 0);
        let v = evaluate(
            &s,
            Ok(info("03.30.73", "8.3.0")),
            Model::Known("OLED55C36LC"),
        );
        assert!(matches!(v, Verdict::Verified(..)));
        // Same series, another firmware.
        let v = evaluate(
            &s,
            Ok(info("03.40.00", "8.3.0")),
            Model::Known("OLED65C3PUA"),
        );
        assert!(matches!(v, Verdict::SeriesOnly(..)) && !v.blocks());
        assert_eq!(v.exit_code(), 1);
        assert!(v.to_string().contains("starts anyway"));
        // Another model and firmware: not verified, still not blocked.
        let v = evaluate(
            &s,
            Ok(info("99.99.99", "11.0.0")),
            Model::Known("OLED65G36LA"),
        );
        assert!(matches!(v, Verdict::NotVerified { strict: false, .. }) && !v.blocks());
        assert_eq!(v.exit_code(), 1);
        // Unreadable firmware or model: unknown, not blocked.
        let v = evaluate(&s, Err("no file".to_owned()), Model::NotChecked);
        assert!(matches!(v, Verdict::Unreadable { strict: false, .. }) && !v.blocks());
        let v = evaluate(&s, Ok(info("03.30.74", "8.3.0")), Model::NotRead);
        assert!(matches!(v, Verdict::Unreadable { .. }) && !v.blocks());
        // The firmware alone (--check-firmware).
        let v = evaluate(&s, Ok(info("03.30.74", "8.3.0")), Model::NotChecked);
        assert_eq!(v.exit_code(), 0);
        let v = evaluate(&s, Ok(info("03.30.99", "8.3.0")), Model::NotChecked);
        assert_eq!(v.exit_code(), 1);
    }

    #[test]
    fn owner_list_extends_the_verified_list_in_warn_mode() {
        let s = settings(false, &["77.77.77"]);
        let v = evaluate(&s, Ok(info("77.77.77", "9.0.0")), Model::Known("X1"));
        assert!(matches!(v, Verdict::Verified(..)));
        let v = evaluate(&s, Ok(info("77.77.77", "9.0.0")), Model::NotRead);
        assert!(matches!(v, Verdict::Verified(..)));
    }

    /// Strict mode is the old gate: only the owner's list counts.
    #[test]
    fn strict_mode_is_the_old_gate() {
        let s = settings(true, &["03.30.74"]);
        let v = evaluate(&s, Ok(info("03.30.74", "8.3.0")), Model::NotChecked);
        assert!(matches!(v, Verdict::Verified(..)) && !v.blocks());
        assert_eq!(v.exit_code(), 0);
        // Even a built-in verified firmware is refused if not in the list.
        let v = evaluate(
            &s,
            Ok(info("03.30.73", "8.3.0")),
            Model::Known("OLED55C36LC"),
        );
        assert!(v.blocks());
        assert_eq!(v.exit_code(), 1);
        assert!(v.to_string().contains("NOT in safety.validated_firmware"));
        let v = evaluate(&s, Err("gone".to_owned()), Model::NotChecked);
        assert!(v.blocks() && v.to_string().contains("stays disabled"));
        // Empty list: gate off (exit 3), as before.
        let v = evaluate(
            &settings(true, &[]),
            Ok(info("03.30.74", "8.3.0")),
            Model::NotChecked,
        );
        assert_eq!(v, Verdict::Off);
        assert!(!v.blocks());
        assert_eq!(v.exit_code(), 3);
    }

    #[test]
    fn check_reads_the_file() {
        let dir = std::env::temp_dir().join(format!("webos-sat-fw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("os_info.json");
        std::fs::write(&path, OS_INFO).unwrap();
        let mut s = settings(false, &[]);
        s.safety.firmware_info = path.clone();
        assert_eq!(check(&s).exit_code(), 0);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(check(&s), Verdict::Unreadable { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn firmware_changed_marker() {
        let dir = std::env::temp_dir().join(format!("webos-sat-fwseen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("firmware_seen");
        assert_eq!(check_seen(&path, "03.30.74"), Seen::First);
        write_seen(&path, "03.30.74").unwrap();
        assert_eq!(check_seen(&path, "03.30.74"), Seen::Same);
        assert_eq!(
            check_seen(&path, "03.30.80"),
            Seen::Changed("03.30.74".to_owned())
        );
        // Not written by the check: the change stays until acknowledged.
        assert_eq!(
            check_seen(&path, "03.30.80"),
            Seen::Changed("03.30.74".to_owned())
        );
        write_seen(&path, "03.30.80").unwrap();
        assert_eq!(check_seen(&path, "03.30.80"), Seen::Same);
        // Junk is cleaned and an empty file is "first".
        std::fs::write(&path, "  \n").unwrap();
        assert_eq!(check_seen(&path, "03.30.80"), Seen::First);
        std::fs::write(&path, "03.30.74 \u{1b}[31m evil\n").unwrap();
        assert_eq!(
            check_seen(&path, "03.30.80"),
            Seen::Changed("03.30.7431mevil".to_owned())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
