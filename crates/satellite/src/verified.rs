//! The verified-TVs list (`data/verified-models.toml`), compiled into the
//! daemon. Matching is by series and firmware (owner decision 2026-11-04):
//! a TV whose model name matches a series (glob, case-insensitive) and whose
//! webOS major matches is in the series; with a listed firmware it is
//! `Verified`, with another one `SeriesOnly`. Anything else is `Unlisted`.
//! Never an error: a TV that is not listed is simply "not verified".

use serde::Deserialize;

/// The data file, embedded at build time.
const DATA: &str = include_str!("../../../data/verified-models.toml");

/// One tested firmware of a series.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Tested {
    /// `webos_manufacturing_version`.
    pub(crate) version: String,
}

/// One series.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Series {
    /// Display name, for example "LG OLED C3 (2023, webOS 8.x)".
    pub(crate) name: String,
    /// Model name globs (`?` one character, `*` any run), case-insensitive.
    pub(crate) models: Vec<String>,
    /// webOS major number the series covers, when set.
    #[serde(default)]
    pub(crate) webos: Option<String>,
    /// Firmware versions the project itself tested.
    #[serde(default)]
    pub(crate) firmware: Vec<Tested>,
}

#[derive(Debug, Deserialize)]
struct File {
    #[serde(default)]
    series: Vec<Series>,
}

/// Result of a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Series and firmware are listed.
    Verified(String),
    /// The series is listed, the firmware is not.
    SeriesOnly(String),
    /// Neither.
    Unlisted,
}

/// The embedded list. An unparsable file (a test makes sure it parses) is an
/// empty list, so a broken data file never stops the daemon.
pub(crate) fn series() -> Vec<Series> {
    toml::from_str::<File>(DATA).map_or_else(|_| Vec::new(), |f| f.series)
}

/// Glob match: `?` is one character, `*` any run (also empty); case-insensitive.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_uppercase().chars().collect();
    let t: Vec<char> = text.to_ascii_uppercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The webOS major number of a release string (`8.3.0` gives `8`).
fn major(release: &str) -> &str {
    release.split('.').next().unwrap_or("").trim()
}

/// Looks a TV up. `webos_release` is optional; a series with a `webos` major
/// does not match a TV whose release is known and different, and matches one
/// whose release is not known.
pub(crate) fn lookup(model: &str, webos_release: Option<&str>, firmware: &str) -> Standing {
    lookup_in(&series(), model, webos_release, firmware)
}

fn lookup_in(
    list: &[Series],
    model: &str,
    webos_release: Option<&str>,
    firmware: &str,
) -> Standing {
    let firmware = firmware.trim();
    for s in list {
        if !s.models.iter().any(|g| glob_match(g, model.trim())) {
            continue;
        }
        if let (Some(want), Some(have)) = (s.webos.as_deref(), webos_release) {
            if major(have) != want.trim() {
                continue;
            }
        }
        return if s.firmware.iter().any(|f| f.version.trim() == firmware) {
            Standing::Verified(s.name.clone())
        } else {
            Standing::SeriesOnly(s.name.clone())
        };
    }
    Standing::Unlisted
}

/// Whether any series lists this firmware (used when the model is not read,
/// for example by `--check-firmware`).
pub(crate) fn firmware_listed(firmware: &str) -> bool {
    let firmware = firmware.trim();
    series()
        .iter()
        .any(|s| s.firmware.iter().any(|f| f.version.trim() == firmware))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn data_file_parses_and_lists_the_c3_rows() {
        let list = series();
        assert!(!list.is_empty(), "data/verified-models.toml must parse");
        let c3 = list.iter().find(|s| s.name.contains("OLED C3")).unwrap();
        let fws: Vec<&str> = c3.firmware.iter().map(|f| f.version.as_str()).collect();
        assert_eq!(fws, ["03.30.74", "03.30.73"]);
        assert_eq!(c3.webos.as_deref(), Some("8"));
        for s in &list {
            assert!(!s.name.is_empty() && !s.models.is_empty());
        }
    }

    #[test]
    fn globs() {
        assert!(glob_match("OLED??C3*", "OLED42C34LA"));
        assert!(glob_match("OLED??C3*", "oled55c36lc"));
        assert!(glob_match("OLED??C3*", "OLED77C3PUA"));
        assert!(!glob_match("OLED??C3*", "OLED65G36LA"));
        assert!(!glob_match("OLED??C3*", "OLED42C24LA"));
        assert!(!glob_match("OLED??C3*", "OLED4C34"));
        assert!(glob_match("*", ""));
        assert!(glob_match("A*B*C", "AxxByyC"));
        assert!(!glob_match("A*B*C", "AxxByy"));
    }

    #[test]
    fn series_and_firmware_matching() {
        // Both tested units.
        assert!(matches!(
            lookup("OLED42C34LA", Some("8.3.0"), "03.30.74"),
            Standing::Verified(_)
        ));
        assert!(matches!(
            lookup("OLED55C36LC", Some("8.3.0"), "03.30.73"),
            Standing::Verified(_)
        ));
        // Another size of the series and a firmware nobody reported: the
        // series, firmware not yet reported.
        let Standing::SeriesOnly(name) = lookup("OLED77C3PUA", Some("8.1.0"), "03.20.10") else {
            panic!("expected the series");
        };
        assert_eq!(name, "LG OLED C3 (2023, webOS 8.x)");
        // The tested firmware on another size of the series is still verified.
        assert!(matches!(
            lookup("OLED65C3PUA", Some("8.3.0"), "03.30.74"),
            Standing::Verified(_)
        ));
        // A later webOS major is not the series any more.
        assert_eq!(
            lookup("OLED42C34LA", Some("9.0.0"), "03.40.00"),
            Standing::Unlisted
        );
        // Unknown release: the model decides.
        assert!(matches!(
            lookup("OLED42C34LA", None, "03.30.99"),
            Standing::SeriesOnly(_)
        ));
        // Another series.
        assert_eq!(
            lookup("OLED65G36LA", Some("8.3.0"), "03.30.74"),
            Standing::Unlisted
        );
    }

    #[test]
    fn firmware_only_lookup() {
        assert!(firmware_listed("03.30.74"));
        assert!(firmware_listed(" 03.30.73 "));
        assert!(!firmware_listed("03.30.99"));
    }
}
