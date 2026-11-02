//! Firmware guard: compares the TV's firmware version with the allow-list in
//! `safety.validated_firmware`.
//!
//! Source (read-only, verified on c3-a): `/var/run/nyx/os_info.json`, a flat
//! JSON object that nyx writes at boot on a tmpfs. Its
//! `webos_manufacturing_version` is the LG firmware string (`03.30.74` on
//! c3-a, the same value `getSystemInfo` reports as `firmwareVersion`).

use std::fmt;
use std::path::Path;

use crate::config::Settings;

/// Key of the firmware version in `os_info.json`.
const VERSION_KEY: &str = "webos_manufacturing_version";

/// What the guard knows about the running firmware.
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

/// Result of the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The version is in the allow-list.
    Validated(FirmwareInfo),
    /// The version is not in the allow-list.
    NotValidated(FirmwareInfo),
    /// The information could not be read or parsed.
    Unreadable(String),
    /// The allow-list is empty: the guard is off.
    Off,
}

impl Verdict {
    /// True if capture may run.
    pub(crate) fn allows_capture(&self) -> bool {
        matches!(self, Self::Validated(_) | Self::Off)
    }

    /// Exit code of `--check-firmware`.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Validated(_) => 0,
            Self::NotValidated(_) | Self::Unreadable(_) => 1,
            Self::Off => 3,
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validated(i) => write!(f, "{i}: validated"),
            Self::NotValidated(i) => write!(
                f,
                "{i}: NOT in safety.validated_firmware; the microphone stays disabled until this firmware is validated and added"
            ),
            Self::Unreadable(e) => write!(
                f,
                "cannot read the firmware version ({e}); the microphone stays disabled"
            ),
            Self::Off => f.write_str("firmware guard off (safety.validated_firmware is empty)"),
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

/// Runs the guard for `settings`.
pub(crate) fn check(settings: &Settings) -> Verdict {
    let allow = &settings.safety.validated_firmware;
    if allow.is_empty() {
        return Verdict::Off;
    }
    match read(&settings.safety.firmware_info) {
        Ok(info) if allow.iter().any(|v| v.trim() == info.version) => Verdict::Validated(info),
        Ok(info) => Verdict::NotValidated(info),
        Err(e) => Verdict::Unreadable(e),
    }
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

    fn settings_with(dir: &Path, text: Option<&str>, allow: &[&str]) -> Settings {
        let path = dir.join("os_info.json");
        let _ = std::fs::remove_file(&path);
        if let Some(t) = text {
            std::fs::write(&path, t).unwrap();
        }
        let mut s = Settings::default();
        s.safety.firmware_info = path;
        s.safety.validated_firmware = allow.iter().map(|v| (*v).to_owned()).collect();
        s
    }

    #[test]
    fn verdicts() {
        let dir = std::env::temp_dir().join(format!("webos-sat-fw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let v = check(&settings_with(&dir, Some(OS_INFO), &["03.30.74"]));
        assert!(matches!(v, Verdict::Validated(_)) && v.allows_capture());
        assert_eq!(v.exit_code(), 0);

        let v = check(&settings_with(
            &dir,
            Some(OS_INFO),
            &["03.30.80", "04.00.00"],
        ));
        assert!(matches!(v, Verdict::NotValidated(_)) && !v.allows_capture());
        assert_eq!(v.exit_code(), 1);
        assert!(v.to_string().contains("NOT in safety.validated_firmware"));

        let v = check(&settings_with(&dir, None, &["03.30.74"]));
        assert!(matches!(v, Verdict::Unreadable(_)) && !v.allows_capture());
        assert_eq!(v.exit_code(), 1);

        let v = check(&settings_with(&dir, Some("not json"), &["03.30.74"]));
        assert!(matches!(v, Verdict::Unreadable(_)));

        let v = check(&settings_with(&dir, None, &[]));
        assert_eq!(v, Verdict::Off);
        assert!(v.allows_capture());
        assert_eq!(v.exit_code(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
