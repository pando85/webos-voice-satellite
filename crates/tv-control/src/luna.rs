//! Parsers for the `luna-send` replies TV control reads.
//!
//! Field names come from the getters read on c3-b (webOS TV 8.3.0, firmware
//! `03.30.73`, 2026-11-04, `verified-on-target` for that unit only; see
//! `docs/design/tv-control.md` section 5). Every parser is tolerant of extra
//! fields and strict about the fields the design uses: a reply that lacks
//! them is an error, never a guessed value. Error values never carry a reply
//! body, so they are safe to log.

use std::fmt;

use serde_json::Value;

/// Largest reply accepted (the app list is about 35 KB on c3-b).
pub const MAX_REPLY_BYTES: usize = 1 << 20;

/// A reply with `returnValue: false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LunaError {
    /// `errorCode`, if present.
    pub code: Option<i64>,
    /// `errorText`, if present (a short LG message such as
    /// `Keyblock1.5second`).
    pub text: Option<String>,
}

impl fmt::Display for LunaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.code, &self.text) {
            (Some(c), Some(t)) => write!(f, "luna error {c}: {t}"),
            (Some(c), None) => write!(f, "luna error {c}"),
            (None, Some(t)) => write!(f, "luna error: {t}"),
            (None, None) => f.write_str("luna error (returnValue false)"),
        }
    }
}

/// Why a reply was not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    /// The call did not run or did not answer (the text says why; never a
    /// reply body).
    Transport(String),
    /// More than [`MAX_REPLY_BYTES`].
    TooLarge(usize),
    /// No output.
    Empty,
    /// Output that is not a JSON object.
    Malformed,
    /// `returnValue: false`.
    Luna(LunaError),
    /// A JSON object without the fields the design needs (the text names the
    /// field).
    Shape(String),
}

impl fmt::Display for ReplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(t) => f.write_str(t),
            Self::TooLarge(n) => write!(
                f,
                "reply of {n} bytes is over the {MAX_REPLY_BYTES} byte limit"
            ),
            Self::Empty => f.write_str("no output"),
            Self::Malformed => f.write_str("output is not a JSON object"),
            Self::Luna(e) => e.fmt(f),
            Self::Shape(s) => write!(f, "unexpected reply: {s}"),
        }
    }
}

impl std::error::Error for ReplyError {}

impl ReplyError {
    /// The luna `errorText`, if this is a luna error (used to spot the
    /// transient `Keyblock1.5second`).
    #[must_use]
    pub fn luna_text(&self) -> Option<&str> {
        match self {
            Self::Luna(e) => e.text.as_deref(),
            _ => None,
        }
    }
}

/// Parses the output of one `luna-send` call (or one line of a
/// subscription) into a JSON object whose `returnValue` is true.
///
/// # Errors
///
/// [`ReplyError`] for empty, oversized or malformed output, a luna error, or
/// a reply with no `returnValue: true`.
pub fn parse_reply(text: &str) -> Result<Value, ReplyError> {
    if text.len() > MAX_REPLY_BYTES {
        return Err(ReplyError::TooLarge(text.len()));
    }
    let t = text.trim();
    if t.is_empty() {
        return Err(ReplyError::Empty);
    }
    // luna-send may print a message line before the reply.
    let v: Value = serde_json::from_str(t)
        .ok()
        .or_else(|| {
            t.lines()
                .rev()
                .map(str::trim)
                .filter(|l| l.starts_with('{'))
                .find_map(|l| serde_json::from_str(l).ok())
        })
        .ok_or(ReplyError::Malformed)?;
    if !v.is_object() {
        return Err(ReplyError::Malformed);
    }
    match v.get("returnValue").and_then(Value::as_bool) {
        Some(true) => Ok(v),
        Some(false) => Err(ReplyError::Luna(LunaError {
            code: v.get("errorCode").and_then(Value::as_i64),
            text: v
                .get("errorText")
                .and_then(Value::as_str)
                .map(|s| s.chars().take(120).collect()),
        })),
        None => Err(ReplyError::Shape("no returnValue".to_owned())),
    }
}

/// A subscription reply carries `subscribed: true`.
#[must_use]
pub fn is_subscribed(v: &Value) -> bool {
    v.get("subscribed").and_then(Value::as_bool) == Some(true)
}

fn shape(field: &str) -> ReplyError {
    ReplyError::Shape(format!("missing or invalid {field}"))
}

/// TV power state from `tvpower/power/getPowerState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerReply {
    /// `state` (`Active`, `Screen Off`, ...).
    pub state: String,
    /// A transition is in progress (`processing` present).
    pub processing: bool,
}

/// Parses a power state reply.
///
/// # Errors
///
/// [`ReplyError::Shape`] without a string `state`.
pub fn power_state(v: &Value) -> Result<PowerReply, ReplyError> {
    let state = v
        .get("state")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| shape("state"))?;
    Ok(PowerReply {
        state: state.to_owned(),
        processing: v
            .get("processing")
            .and_then(Value::as_str)
            .is_some_and(|p| !p.is_empty()),
    })
}

/// `audio/master/getVolume`: `volumeStatus{volume, muteStatus, maxVolume,
/// soundOutput}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeStatus {
    /// Current volume in the TV's own steps.
    pub volume: u32,
    /// Highest volume (100 on c3-b).
    pub max_volume: u32,
    /// Muted.
    pub muted: bool,
    /// Sound output, for example `external_arc` or `tv_speaker`.
    pub sound_output: Option<String>,
}

/// Parses a `getVolume` reply (also a subscription line). The status object
/// is `volumeStatus`, or the root object itself.
///
/// # Errors
///
/// [`ReplyError::Shape`] when `volume`, `maxVolume` or the mute flag
/// (`muteStatus`, or `muted`) is missing or of the wrong type.
pub fn volume(v: &Value) -> Result<VolumeStatus, ReplyError> {
    let s = v.get("volumeStatus").filter(|s| s.is_object()).unwrap_or(v);
    let num = |k: &str| {
        s.get(k)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let volume = num("volume").ok_or_else(|| shape("volume"))?;
    let max_volume = num("maxVolume")
        .filter(|m| *m > 0)
        .ok_or_else(|| shape("maxVolume"))?;
    let muted = s
        .get("muteStatus")
        .or_else(|| s.get("muted"))
        .and_then(Value::as_bool)
        .ok_or_else(|| shape("muteStatus"))?;
    let sound_output = s
        .get("soundOutput")
        .and_then(Value::as_str)
        .filter(|o| !o.is_empty() && o.len() <= 64)
        .map(str::to_owned);
    Ok(VolumeStatus {
        volume,
        max_volume,
        muted,
        sound_output,
    })
}

/// One entry of `apiadapter/tv/getExternalInputList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalInput {
    /// The input's `id`.
    pub id: String,
    /// The TV's label (for example `HDMI 1`).
    pub label: String,
    /// The app that shows this input, if the reply names one.
    pub app_id: Option<String>,
    /// A source is connected.
    pub connected: bool,
}

/// Parses an external input list. Entries without `id` or `label` are
/// skipped.
///
/// # Errors
///
/// [`ReplyError::Shape`] when `devices` is not an array.
pub fn external_inputs(v: &Value) -> Result<Vec<ExternalInput>, ReplyError> {
    let devices = v
        .get("devices")
        .and_then(Value::as_array)
        .ok_or_else(|| shape("devices"))?;
    Ok(devices
        .iter()
        .filter_map(|d| {
            let id = d
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())?;
            let label = d
                .get("label")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())?;
            Some(ExternalInput {
                id: id.to_owned(),
                label: label.to_owned(),
                app_id: d
                    .get("appId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                connected: d.get("connected").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

/// Parses `applicationManager/getForegroundAppInfo`: the foreground app id,
/// `None` when no app is in the foreground (an empty `appId`).
///
/// # Errors
///
/// [`ReplyError::Shape`] without an `appId`.
pub fn foreground_app(v: &Value) -> Result<Option<String>, ReplyError> {
    let id = v
        .get("appId")
        .or_else(|| {
            v.get("foregroundAppInfo")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|f| f.get("appId"))
        })
        .and_then(Value::as_str)
        .ok_or_else(|| shape("appId"))?;
    Ok((!id.is_empty()).then(|| id.to_owned()))
}

/// Parses `com.webos.media/getForegroundAppInfo`: the `playState` value, if
/// the media service reports one (at the top level or in the first
/// `foregroundAppInfo` entry).
#[must_use]
pub fn play_state(v: &Value) -> Option<String> {
    v.get("playState")
        .or_else(|| {
            v.get("foregroundAppInfo")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|f| f.get("playState"))
        })
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .map(str::to_owned)
}

/// The Always Ready setting (`general/lifeOnScreenMode`), reduced to what
/// Home Assistant shows. The raw words are `verified-on-target` on c3-a
/// (EXP-011, 2026-11-05): `off`, `none` (hidden, inactive), `lifeOnScreen`,
/// `alwaysReady` (the dark variant) and `allEnabled` (with wallpaper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlwaysReady {
    /// `off` or `none`.
    Off,
    /// `alwaysReady`.
    On,
    /// `allEnabled`.
    OnWithWallpaper,
    /// Any other word (`lifeOnScreen` included), kept short and printable
    /// for the log; never shown in Home Assistant.
    Other(String),
}

impl AlwaysReady {
    /// What Home Assistant shows: only these four strings.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::On => "On",
            Self::OnWithWallpaper => "On with wallpaper",
            Self::Other(_) => "Other",
        }
    }
}

/// Parses `settingsservice/getSystemSettings` for `lifeOnScreenMode`
/// (`settings.lifeOnScreenMode`, a string).
///
/// # Errors
///
/// [`ReplyError::Shape`] without that string.
pub fn always_ready(v: &Value) -> Result<AlwaysReady, ReplyError> {
    let raw = v
        .get("settings")
        .and_then(|s| s.get("lifeOnScreenMode"))
        .and_then(Value::as_str)
        .ok_or_else(|| shape("settings.lifeOnScreenMode"))?;
    Ok(match raw {
        "off" | "none" => AlwaysReady::Off,
        "alwaysReady" => AlwaysReady::On,
        "allEnabled" => AlwaysReady::OnWithWallpaper,
        other => AlwaysReady::Other(
            if !other.is_empty()
                && other.len() <= 32
                && other
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                other.to_owned()
            } else {
                "(unprintable)".to_owned()
            },
        ),
    })
}

/// LG hands-free ("Hi LG"), reduced to what Home Assistant shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandsFree {
    /// Every `settingStatus` that is reported is `false`.
    Off,
    /// The top level or a provider reports `settingStatus: true`.
    On,
    /// A `settingStatus` that is not a boolean.
    Other,
}

impl HandsFree {
    /// What Home Assistant shows: only these three strings.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::On => "On",
            Self::Other => "Other",
        }
    }
}

/// The providers of `getActivation` (c3-a, EXP-011): the top level carries
/// the same fields, one object per provider.
const HANDS_FREE_PROVIDERS: [&str; 3] = ["thinQ", "amazon", "dangbei"];

/// Parses `voiceinput/setting/getActivation`. Working rule (design 6.1,
/// EXP-011): on when the top level or any provider reports `settingStatus:
/// true`, off when all that are reported are `false`. Trigger words, other
/// provider names and every other field are ignored.
///
/// # Errors
///
/// [`ReplyError::Shape`] when no `settingStatus` is reported at all.
pub fn hands_free(v: &Value) -> Result<HandsFree, ReplyError> {
    let statuses: Vec<&Value> = std::iter::once(v)
        .chain(HANDS_FREE_PROVIDERS.iter().filter_map(|p| v.get(*p)))
        .filter_map(|o| o.get("settingStatus"))
        .collect();
    if statuses.is_empty() {
        return Err(shape("settingStatus"));
    }
    if statuses.iter().any(|s| s.as_bool() == Some(true)) {
        Ok(HandsFree::On)
    } else if statuses.iter().all(|s| s.as_bool() == Some(false)) {
        Ok(HandsFree::Off)
    } else {
        Ok(HandsFree::Other)
    }
}

/// One launch point, reduced to the three fields TV control keeps. Everything
/// else in the reply (icons, paths, user data) is dropped here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPoint {
    /// The app id (`id`, or `appId`).
    pub id: String,
    /// The title the launcher shows.
    pub title: String,
    /// The launcher does not show it (`visible: false` or `hidden: true`,
    /// when the reply says so).
    pub hidden: bool,
}

/// Parses `applicationManager/listLaunchPoints`.
///
/// The field that marks hidden launch points is **not confirmed** (the c3-b
/// reply was not available when this was written): `visible: false` and
/// `hidden: true` are honoured when present, otherwise every entry with an id
/// and a title is kept (`docs/design/tv-control.md` section 8, step 2).
///
/// # Errors
///
/// [`ReplyError::Shape`] when `launchPoints` is not an array.
pub fn launch_points(v: &Value) -> Result<Vec<LaunchPoint>, ReplyError> {
    let points = v
        .get("launchPoints")
        .and_then(Value::as_array)
        .ok_or_else(|| shape("launchPoints"))?;
    Ok(points
        .iter()
        .filter_map(|p| {
            let id = p
                .get("id")
                .or_else(|| p.get("appId"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())?;
            let title = p
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            let hidden = p.get("visible").and_then(Value::as_bool) == Some(false)
                || p.get("hidden").and_then(Value::as_bool) == Some(true);
            Some(LaunchPoint {
                id: id.to_owned(),
                title: title.chars().take(100).collect(),
                hidden,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn ok(text: &str) -> Value {
        parse_reply(text).unwrap()
    }

    #[test]
    fn reply_envelope() {
        assert_eq!(parse_reply(""), Err(ReplyError::Empty));
        assert_eq!(parse_reply("  \n"), Err(ReplyError::Empty));
        assert_eq!(parse_reply("not json"), Err(ReplyError::Malformed));
        assert_eq!(parse_reply("[1,2]"), Err(ReplyError::Malformed));
        assert_eq!(
            parse_reply(r#"{"state":"Active"}"#),
            Err(ReplyError::Shape("no returnValue".to_owned()))
        );
        let e = parse_reply(
            r#"{"returnValue":false,"errorCode":-1,"errorText":"Keyblock1.5second","secret":"x"}"#,
        )
        .unwrap_err();
        assert_eq!(e.luna_text(), Some("Keyblock1.5second"));
        assert_eq!(e.to_string(), "luna error -1: Keyblock1.5second");
        assert_eq!(
            parse_reply(r#"{"returnValue":false}"#)
                .unwrap_err()
                .to_string(),
            "luna error (returnValue false)"
        );
        // A message line before the reply is tolerated.
        let v = ok("** Message: info\n{\"returnValue\":true,\"subscribed\":true}\n");
        assert!(is_subscribed(&v));
        assert!(!is_subscribed(&ok(r#"{"returnValue":true}"#)));
        let big = format!(
            "{{\"returnValue\":true,\"pad\":\"{}\"}}",
            "x".repeat(MAX_REPLY_BYTES)
        );
        assert!(matches!(parse_reply(&big), Err(ReplyError::TooLarge(_))));
    }

    #[test]
    fn error_text_never_carries_a_reply_body() {
        let e = parse_reply(r#"{"returnValue":false,"errorText":"Denied","serial":"SN123"}"#)
            .unwrap_err();
        assert!(!e.to_string().contains("SN123"));
        let e = parse_reply(r#"{"returnValue":true,"volumeStatus":{"serial":"SN123"}}"#)
            .map(|v| volume(&v))
            .unwrap()
            .unwrap_err();
        assert!(!e.to_string().contains("SN123"), "{e}");
    }

    #[test]
    fn power_state_reply() {
        let p = power_state(&ok(
            r#"{"returnValue":true,"state":"Active Standby","processing":"Request Power Off","onOff":"off"}"#,
        ))
        .unwrap();
        assert_eq!((p.state.as_str(), p.processing), ("Active Standby", true));
        let p = power_state(&ok(
            r#"{"returnValue":true,"state":"Active","subscribed":true}"#,
        ))
        .unwrap();
        assert!(!p.processing);
        assert!(power_state(&ok(r#"{"returnValue":true}"#)).is_err());
    }

    #[test]
    fn volume_reply_variants() {
        let v = volume(&ok(
            r#"{"returnValue":true,"callerId":"x","volumeStatus":{"volume":12,"maxVolume":100,"muteStatus":false,"soundOutput":"external_arc","mode":"normal","adjustVolume":true}}"#,
        ))
        .unwrap();
        assert_eq!(
            v,
            VolumeStatus {
                volume: 12,
                max_volume: 100,
                muted: false,
                sound_output: Some("external_arc".to_owned()),
            }
        );
        // Muted, flat shape (a subscription line), `muted` spelling.
        let v = volume(&ok(
            r#"{"returnValue":true,"subscribed":true,"volume":0,"maxVolume":60,"muted":true}"#,
        ))
        .unwrap();
        assert_eq!(
            (v.volume, v.max_volume, v.muted, v.sound_output),
            (0, 60, true, None)
        );
        // Missing or wrongly typed fields are errors, never defaults.
        for bad in [
            r#"{"returnValue":true,"volumeStatus":{"maxVolume":100,"muteStatus":false}}"#,
            r#"{"returnValue":true,"volumeStatus":{"volume":-1,"maxVolume":100,"muteStatus":false}}"#,
            r#"{"returnValue":true,"volumeStatus":{"volume":5,"muteStatus":false}}"#,
            r#"{"returnValue":true,"volumeStatus":{"volume":5,"maxVolume":0,"muteStatus":false}}"#,
            r#"{"returnValue":true,"volumeStatus":{"volume":5,"maxVolume":100}}"#,
            r#"{"returnValue":true,"volumeStatus":{"volume":"5","maxVolume":100,"muteStatus":false}}"#,
        ] {
            assert!(
                matches!(volume(&ok(bad)), Err(ReplyError::Shape(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn input_list_reply() {
        let l = external_inputs(&ok(r#"{"returnValue":true,"devices":[
                {"id":"HDMI_1","label":"HDMI 1","connected":true,"appId":"app.hdmi1","subList":[]},
                {"id":"HDMI_2","label":"HDMI 2","connected":false,"appId":"app.hdmi2"},
                {"label":"no id"},
                {"id":"X"},
                {"id":"AV","label":"AV"}
            ]}"#))
        .unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].app_id.as_deref(), Some("app.hdmi1"));
        assert!(l[0].connected && !l[1].connected && !l[2].connected);
        assert_eq!(l[2].app_id, None);
        assert_eq!(
            external_inputs(&ok(r#"{"returnValue":true,"devices":[]}"#)).unwrap(),
            []
        );
        assert!(external_inputs(&ok(r#"{"returnValue":true}"#)).is_err());
    }

    #[test]
    fn foreground_and_play_state_replies() {
        assert_eq!(
            foreground_app(&ok(r#"{"returnValue":true,"appId":"app.a","windowId":""}"#)).unwrap(),
            Some("app.a".to_owned())
        );
        assert_eq!(
            foreground_app(&ok(r#"{"returnValue":true,"appId":""}"#)).unwrap(),
            None
        );
        assert_eq!(
            foreground_app(&ok(
                r#"{"returnValue":true,"foregroundAppInfo":[{"appId":"app.b","playState":"playing"}]}"#
            ))
            .unwrap(),
            Some("app.b".to_owned())
        );
        assert!(foreground_app(&ok(r#"{"returnValue":true}"#)).is_err());
        assert_eq!(
            play_state(&ok(r#"{"returnValue":true,"playState":"paused"}"#)).as_deref(),
            Some("paused")
        );
        assert_eq!(
            play_state(&ok(
                r#"{"returnValue":true,"foregroundAppInfo":[{"appId":"a","playState":"playing"}]}"#
            ))
            .as_deref(),
            Some("playing")
        );
        assert_eq!(play_state(&ok(r#"{"returnValue":true}"#)), None);
    }

    #[test]
    fn always_ready_reply() {
        let r = |w: &str| {
            always_ready(&ok(&format!(
                r#"{{"returnValue":true,"category":"general","settings":{{"lifeOnScreenMode":"{w}"}}}}"#
            )))
        };
        assert_eq!(r("off").unwrap(), AlwaysReady::Off);
        assert_eq!(r("none").unwrap(), AlwaysReady::Off);
        assert_eq!(r("alwaysReady").unwrap(), AlwaysReady::On);
        assert_eq!(r("allEnabled").unwrap(), AlwaysReady::OnWithWallpaper);
        assert_eq!(
            r("lifeOnScreen").unwrap(),
            AlwaysReady::Other("lifeOnScreen".to_owned())
        );
        // An unknown word never reaches Home Assistant as itself.
        assert_eq!(r("brandNew").unwrap().label(), "Other");
        assert_eq!(
            r("a b").unwrap(),
            AlwaysReady::Other("(unprintable)".to_owned())
        );
        assert_eq!(
            r(&"x".repeat(40)).unwrap(),
            AlwaysReady::Other("(unprintable)".to_owned())
        );
        let labels: Vec<&str> = [
            AlwaysReady::Off,
            AlwaysReady::On,
            AlwaysReady::OnWithWallpaper,
            AlwaysReady::Other("x".to_owned()),
        ]
        .iter()
        .map(AlwaysReady::label)
        .collect();
        assert_eq!(labels, ["Off", "On", "On with wallpaper", "Other"]);
        for bad in [
            r#"{"returnValue":true}"#,
            r#"{"returnValue":true,"settings":{}}"#,
            r#"{"returnValue":true,"settings":{"lifeOnScreenMode":3}}"#,
            r#"{"returnValue":true,"settings":{"lifeOnScreenMode":null}}"#,
            r#"{"returnValue":true,"lifeOnScreenMode":"off"}"#,
        ] {
            assert!(
                matches!(always_ready(&ok(bad)), Err(ReplyError::Shape(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn hands_free_reply() {
        let h = |t: &str| hands_free(&ok(t));
        // c3-a shape, all off.
        assert_eq!(
            h(
                r#"{"returnValue":true,"settingStatus":false,"activationState":false,
                "systemStatus":true,"supportPlatform":["thinQ"],"supportLanguage":["en-US"],
                "thinQ":{"settingStatus":false,"activationState":false},
                "amazon":{"settingStatus":false,"activationState":false},
                "dangbei":{"settingStatus":false,"activationState":false}}"#
            )
            .unwrap(),
            HandsFree::Off
        );
        // On at the top level, with the trigger word fields.
        assert_eq!(
            h(
                r#"{"returnValue":true,"settingStatus":true,"activationState":true,
                "currentTriggerWord":"hiLG","availableTriggerWord":["hiLG"],
                "thinQ":{"settingStatus":true,"currentTriggerWord":"hiLG"}}"#
            )
            .unwrap(),
            HandsFree::On
        );
        // Any provider on is on, even when the top level says off.
        assert_eq!(
            h(
                r#"{"returnValue":true,"settingStatus":false,"thinQ":{"settingStatus":false},
                "amazon":{"settingStatus":true}}"#
            )
            .unwrap(),
            HandsFree::On
        );
        // Only the top level, or only a provider.
        assert_eq!(
            h(r#"{"returnValue":true,"settingStatus":false}"#).unwrap(),
            HandsFree::Off
        );
        assert_eq!(
            h(r#"{"returnValue":true,"thinQ":{"settingStatus":false}}"#).unwrap(),
            HandsFree::Off
        );
        // A status that is not a boolean is Other (never a guess); a true
        // one still wins.
        assert_eq!(
            h(r#"{"returnValue":true,"settingStatus":"yes"}"#).unwrap(),
            HandsFree::Other
        );
        assert_eq!(
            h(r#"{"returnValue":true,"settingStatus":null,"thinQ":{"settingStatus":false}}"#)
                .unwrap(),
            HandsFree::Other
        );
        assert_eq!(
            h(r#"{"returnValue":true,"settingStatus":"x","thinQ":{"settingStatus":true}}"#)
                .unwrap(),
            HandsFree::On
        );
        // Nothing reported: an error, not a default.
        for bad in [
            r#"{"returnValue":true}"#,
            r#"{"returnValue":true,"activationState":true}"#,
            r#"{"returnValue":true,"thinQ":{},"amazon":"x"}"#,
        ] {
            assert!(matches!(h(bad), Err(ReplyError::Shape(_))), "{bad}");
        }
        assert_eq!(
            [HandsFree::Off, HandsFree::On, HandsFree::Other].map(HandsFree::label),
            ["Off", "On", "Other"]
        );
    }

    #[test]
    fn launch_points_keep_three_fields_and_honour_hidden_markers() {
        let l = launch_points(&ok(r#"{"returnValue":true,"launchPoints":[
                {"id":"app.a","title":"App A","icon":"/secret/path.png","userData":{"token":"T"}},
                {"id":"app.b","title":"  App B  ","visible":false},
                {"appId":"app.c","title":"App C","hidden":true},
                {"id":"app.d","title":""},
                {"title":"no id"}
            ]}"#))
        .unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(
            (l[0].id.as_str(), l[0].title.as_str(), l[0].hidden),
            ("app.a", "App A", false)
        );
        assert_eq!((l[1].title.as_str(), l[1].hidden), ("App B", true));
        assert!(l[2].hidden && l[2].id == "app.c");
        assert!(launch_points(&ok(r#"{"returnValue":true}"#)).is_err());
    }
}
