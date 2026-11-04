//! The luna methods TV control calls, their payloads, and the check that a
//! setter is declared on the TV.
//!
//! **Parameter names**, all `verified-on-target` on c3-b (LG C3, firmware
//! `03.30.73`, 2026-11-04; EXP-010 Part A, D5, D6, EXP-009) and **c3-b only**:
//! `turnOffScreen {}`, `turnOnScreen {}`, `powerOn {"reason":"remoteKey"}`, the
//! power key `{}`, `setVolume {"volume":N}`, `volumeUp {}`, `volumeDown {}`,
//! `apiadapter/audio/setMute {"mute":bool}`, `switchInput {"inputId":id}` and
//! `launch {"id":appId}`. The `setVolume` parameter name was taken from what
//! aiowebostv sends (`observed-on-other-device`) and confirmed by the call;
//! the others come from the apiadapter's own scripts on the TV. Other firmware
//! or units may differ, which is why every write is read back and a mismatch
//! disables the control: there is no retry with another parameter set.

use serde_json::{Value, json};

/// `tvpower/power/getPowerState`.
pub const URI_POWER_STATE: &str = "luna://com.webos.service.tvpower/power/getPowerState";
/// `tvpower/power/turnOffScreen` (verified on c3-b).
pub const URI_SCREEN_OFF: &str = "luna://com.webos.service.tvpower/power/turnOffScreen";
/// `tvpower/power/turnOnScreen` (verified on c3-b).
pub const URI_SCREEN_ON: &str = "luna://com.webos.service.tvpower/power/turnOnScreen";
/// `tvpower/power/powerOn` (verified on c3-b with the payload below).
pub const URI_POWER_ON: &str = "luna://com.webos.service.tvpower/power/powerOn";
/// The remote's power key as the TV's own SSAP gateway sends it (verified on
/// c3-b).
pub const URI_POWER_KEY: &str = "luna://com.webos.service.networkinput/controls/power";
/// `audio/master/getVolume` (verified on c3-b).
pub const URI_GET_VOLUME: &str = "luna://com.webos.service.audio/master/getVolume";
/// `audio/master/setVolume` (verified on c3-b).
pub const URI_SET_VOLUME: &str = "luna://com.webos.service.audio/master/setVolume";
/// `audio/master/volumeUp` (verified on c3-b).
pub const URI_VOLUME_UP: &str = "luna://com.webos.service.audio/master/volumeUp";
/// `audio/master/volumeDown` (verified on c3-b).
pub const URI_VOLUME_DOWN: &str = "luna://com.webos.service.audio/master/volumeDown";
/// The mute setter, which forwards to `audio/master/muteVolume` (verified on
/// c3-b).
pub const URI_SET_MUTE: &str = "luna://com.webos.service.apiadapter/audio/setMute";
/// `apiadapter/tv/getExternalInputList` (verified on c3-b).
pub const URI_INPUT_LIST: &str = "luna://com.webos.service.apiadapter/tv/getExternalInputList";
/// `apiadapter/tv/switchInput` (verified on c3-b for a disconnected input).
pub const URI_SWITCH_INPUT: &str = "luna://com.webos.service.apiadapter/tv/switchInput";
/// `applicationManager/getForegroundAppInfo` (verified on c3-b).
pub const URI_FOREGROUND: &str = "luna://com.webos.applicationManager/getForegroundAppInfo";
/// `applicationManager/listLaunchPoints` (verified on c3-b).
pub const URI_LAUNCH_POINTS: &str = "luna://com.webos.applicationManager/listLaunchPoints";
/// `applicationManager/launch` (verified on c3-b for the satellite's own app).
pub const URI_LAUNCH: &str = "luna://com.webos.applicationManager/launch";
/// `media/getForegroundAppInfo` (verified on c3-b).
pub const URI_PLAY_STATE: &str = "luna://com.webos.media/getForegroundAppInfo";

/// Payload of `powerOn` (verified on c3-b).
pub const PAYLOAD_POWER_ON: &str = r#"{"reason":"remoteKey"}"#;
/// An empty payload.
pub const PAYLOAD_EMPTY: &str = "{}";
/// Payload of a getter subscription.
pub const PAYLOAD_SUBSCRIBE: &str = r#"{"subscribe":true}"#;

/// `setVolume` payload.
#[must_use]
pub fn payload_set_volume(volume: u32) -> String {
    json!({ "volume": volume }).to_string()
}

/// `setMute` payload.
#[must_use]
pub fn payload_set_mute(muted: bool) -> String {
    json!({ "mute": muted }).to_string()
}

/// `switchInput` payload.
#[must_use]
pub fn payload_switch_input(input_id: &str) -> String {
    json!({ "inputId": input_id }).to_string()
}

/// `launch` payload (`{"id": appId}`).
#[must_use]
pub fn payload_launch(app_id: &str) -> String {
    json!({ "id": app_id }).to_string()
}

/// A setter whose method name is checked against the TV's declaration files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Setter {
    /// `master/setVolume`.
    VolumeSet,
    /// `master/volumeUp`.
    VolumeUp,
    /// `master/volumeDown`.
    VolumeDown,
    /// `audio/setMute`.
    Mute,
    /// `tv/switchInput`.
    Input,
    /// `applicationManager/launch`.
    Launch,
}

impl Setter {
    /// Every setter that is checked.
    pub const ALL: [Self; 6] = [
        Self::VolumeSet,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::Mute,
        Self::Input,
        Self::Launch,
    ];

    /// `(service, category, method)` as a declaration lists it.
    #[must_use]
    pub fn path(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::VolumeSet => ("com.webos.service.audio", "master", "setVolume"),
            Self::VolumeUp => ("com.webos.service.audio", "master", "volumeUp"),
            Self::VolumeDown => ("com.webos.service.audio", "master", "volumeDown"),
            Self::Mute => ("com.webos.service.apiadapter", "audio", "setMute"),
            Self::Input => ("com.webos.service.apiadapter", "tv", "switchInput"),
            Self::Launch => ("com.webos.applicationManager", "", "launch"),
        }
    }

    /// The setter's name for log lines.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.path().2
    }
}

/// What a scan of the declaration files found for one setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declared {
    /// A declaration names the method.
    Yes,
    /// Declaration files of the service were read and none names the method:
    /// the control is read-only (`docs/design/tv-control.md` section 6,
    /// step 3).
    No,
    /// No declaration file of the service could be read, so nothing is known.
    /// The control is allowed, with a warning: the read-back after every
    /// write is the guard.
    Unchecked,
}

/// All strings in a JSON value (keys and values, recursively): API permission
/// files map groups to lists of `service/category/method` strings.
fn strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(m) => {
            for (k, x) in m {
                out.push(k);
                strings(x, out);
            }
        }
        _ => {}
    }
}

/// Whether one declaration `entry` covers `service/category/method`: the exact
/// method, the whole category (`service/category/*`) or the whole service
/// (`service/*`), with or without a `luna://` prefix.
fn covers(entry: &str, setter: Setter) -> bool {
    let (service, category, method) = setter.path();
    let e = entry.strip_prefix("luna://").unwrap_or(entry);
    let full = if category.is_empty() {
        format!("{service}/{method}")
    } else {
        format!("{service}/{category}/{method}")
    };
    let cat_wild = if category.is_empty() {
        format!("{service}/*")
    } else {
        format!("{service}/{category}/*")
    };
    e == full || e == cat_wild || e == format!("{service}/*")
}

/// Decides [`Declared`] for `setter` from the contents of the declaration
/// files whose name mentions the setter's service. `files` holds
/// `(file name, JSON text)` pairs; files that are not JSON count as unread.
#[must_use]
pub fn declared_in(files: &[(String, String)], setter: Setter) -> Declared {
    let (service, _, _) = setter.path();
    let mut read_any = false;
    for (name, text) in files {
        if !name.contains(service) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        read_any = true;
        let mut all = Vec::new();
        strings(&v, &mut all);
        if all.iter().any(|s| covers(s, setter)) {
            return Declared::Yes;
        }
    }
    if read_any {
        Declared::No
    } else {
        Declared::Unchecked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, text: &str) -> (String, String) {
        (name.to_owned(), text.to_owned())
    }

    #[test]
    fn payloads_are_valid_json_with_the_expected_fields() {
        let v: Value = serde_json::from_str(&payload_set_volume(12)).unwrap();
        assert_eq!(v["volume"], 12);
        let v: Value = serde_json::from_str(&payload_set_mute(true)).unwrap();
        assert_eq!(v["mute"], true);
        let v: Value = serde_json::from_str(&payload_switch_input("HDMI_1")).unwrap();
        assert_eq!(v["inputId"], "HDMI_1");
        // Quoting is safe for odd ids.
        let v: Value = serde_json::from_str(&payload_launch("a\"b\\c")).unwrap();
        assert_eq!(v["id"], "a\"b\\c");
        assert_eq!(PAYLOAD_POWER_ON, r#"{"reason":"remoteKey"}"#);
    }

    #[test]
    fn declaration_match_exact_wildcard_and_missing() {
        let audio = file(
            "com.webos.service.audio.api.json",
            r#"{"audio.operation":["com.webos.service.audio/master/setVolume","com.webos.service.audio/master/volumeUp"],"audio.all":["luna://com.webos.service.audio/internal/*"]}"#,
        );
        let files = [
            audio,
            file(
                "other.json",
                r#"["com.webos.service.audio/master/muteVolume"]"#,
            ),
        ];
        assert_eq!(declared_in(&files, Setter::VolumeSet), Declared::Yes);
        assert_eq!(declared_in(&files, Setter::VolumeUp), Declared::Yes);
        // Files of the service were read and none names the method.
        assert_eq!(declared_in(&files, Setter::VolumeDown), Declared::No);
        // No file of that service: unknown, not "missing".
        assert_eq!(declared_in(&files, Setter::Mute), Declared::Unchecked);
        assert_eq!(declared_in(&files, Setter::Input), Declared::Unchecked);
        // The method is only in a file of another name: not counted.
        let other = [file(
            "other.json",
            r#"["com.webos.service.audio/master/setVolume"]"#,
        )];
        assert_eq!(declared_in(&other, Setter::VolumeSet), Declared::Unchecked);
        assert_eq!(declared_in(&[], Setter::Launch), Declared::Unchecked);
        // Wildcards and the luna:// prefix.
        let wild = [file(
            "com.webos.service.apiadapter.json",
            r#"{"g":["luna://com.webos.service.apiadapter/tv/*"]}"#,
        )];
        assert_eq!(declared_in(&wild, Setter::Input), Declared::Yes);
        let app = [file(
            "com.webos.applicationManager.json",
            r#"{"g":["com.webos.applicationManager/launch"]}"#,
        )];
        assert_eq!(declared_in(&app, Setter::Launch), Declared::Yes);
        // A file that is not JSON counts as unread.
        let junk = [file("com.webos.service.audio.api.json", "not json")];
        assert_eq!(declared_in(&junk, Setter::VolumeSet), Declared::Unchecked);
    }

    #[test]
    fn setter_names() {
        assert_eq!(Setter::Mute.name(), "setMute");
        assert_eq!(Setter::ALL.len(), 6);
    }
}
