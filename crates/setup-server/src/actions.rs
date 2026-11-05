//! The fixed action table (section 9.1): each action maps to one `ctl.sh`
//! argument list built here from enumerated words or a validated integer; no
//! request byte reaches a command.

use serde::Deserialize;
use serde_json::Value;

use crate::mirror::Facts;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    AlwaysReady(bool),
    HiLg(bool),
    /// `true`: keep listening after the power button.
    RemoteOff(bool),
    /// `true`: microphone on (the kill switch removed).
    Mic(bool),
    Pair,
    ReplyVolume(u8),
}

/// The request body of `POST /api/actions`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    action: String,
    #[serde(default)]
    value: Option<String>,
}

fn on_off(v: Option<&str>) -> Option<bool> {
    match v? {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

impl Action {
    /// Parses and validates the JSON body. Unknown keys, duplicate keys,
    /// other types, unknown actions or values are all `None` (400).
    pub(crate) fn from_body(body: &[u8]) -> Option<Action> {
        // serde's derived struct also accepts the array form; only an object.
        if body.trim_ascii_start().first() != Some(&b'{') {
            return None;
        }
        let b: Body = serde_json::from_slice(body).ok()?;
        let v = b.value.as_deref();
        match b.action.as_str() {
            "always-ready" => on_off(v).map(Action::AlwaysReady),
            "hi-lg" => on_off(v).map(Action::HiLg),
            "remote-off" => match v? {
                "normal" => Some(Action::RemoteOff(false)),
                "keep" => Some(Action::RemoteOff(true)),
                _ => None,
            },
            "mic" => on_off(v).map(Action::Mic),
            "pair" => v.is_none().then_some(Action::Pair),
            "reply-volume" => volume(v?).map(Action::ReplyVolume),
            _ => None,
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Action::AlwaysReady(_) => "always-ready",
            Action::HiLg(_) => "hi-lg",
            Action::RemoteOff(_) => "remote-off",
            Action::Mic(_) => "mic",
            Action::Pair => "pair",
            Action::ReplyVolume(_) => "reply-volume",
        }
    }

    /// The value word (`None` for `pair`).
    pub(crate) fn value(self) -> Option<String> {
        let w = |b: bool| if b { "on" } else { "off" };
        match self {
            Action::AlwaysReady(b) | Action::HiLg(b) | Action::Mic(b) => Some(w(b).to_owned()),
            Action::RemoteOff(k) => Some(if k { "keep" } else { "normal" }.to_owned()),
            Action::Pair => None,
            Action::ReplyVolume(n) => Some(n.to_string()),
        }
    }

    /// Owner decision 2026-11-05: turning the microphone off is the safe
    /// direction and runs without the OK on the TV (the TV app shows a
    /// notice from `last_result`, verdict `auto`). Everything else waits.
    pub(crate) fn needs_confirmation(self) -> bool {
        self != Action::Mic(false)
    }

    /// The arguments after the `ctl.sh` path.
    pub(crate) fn argv(self) -> Vec<String> {
        let v = self.value().unwrap_or_default();
        let s = |a: &[&str]| a.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        match self {
            Action::AlwaysReady(_) => s(&["set", "always-ready", &v]),
            Action::HiLg(_) => s(&["set", "hi-lg", &v]),
            Action::RemoteOff(_) => s(&["set", "remote-off", &v]),
            Action::Mic(_) => s(&["mic", &v]),
            Action::Pair => s(&["pair"]),
            Action::ReplyVolume(_) => s(&["set", "reply-volume", &v]),
        }
    }

    /// `ctl.sh set` prints one JSON line with `result`; `mic` and `pair` print
    /// none (their exit status is the result).
    fn prints_json(self) -> bool {
        !matches!(self, Action::Mic(_) | Action::Pair)
    }

    /// The precondition of the table, from the cached state; `Err` is the
    /// English sentence of the 422 answer.
    pub(crate) fn check(self, f: &Facts) -> Result<(), &'static str> {
        const UNKNOWN: &str = "The TV state is not known yet. Try again in a few seconds.";
        const NO_LUNA: &str = "The TV settings service is not available on this TV.";
        match self {
            Action::AlwaysReady(_) => match (f.luna, f.always_ready_raw.as_deref()) {
                (None, _) => Err(UNKNOWN),
                (Some(false), _) => Err(NO_LUNA),
                (Some(true), Some("off" | "alwaysReady")) => Ok(()),
                (Some(true), _) => {
                    Err("Always Ready cannot be changed from here in its current state.")
                }
            },
            Action::HiLg(_) => match f.luna {
                None => Err(UNKNOWN),
                Some(false) => Err(NO_LUNA),
                Some(true) => Ok(()),
            },
            Action::Pair => match (f.running, f.key.as_deref()) {
                (None, _) => Err(UNKNOWN),
                (Some(true), Some("unpaired" | "cleared")) => Ok(()),
                (Some(false), _) => {
                    Err("The satellite is not running, so it cannot open the pairing window.")
                }
                (Some(true), _) => Err("The pairing window is only for an unpaired TV."),
            },
            Action::RemoteOff(_) | Action::Mic(_) | Action::ReplyVolume(_) => Ok(()),
        }
    }

    /// The `result` and `error` of a finished command (section 4.3).
    pub(crate) fn outcome(self, out: &CmdOutcome) -> (String, String) {
        match out {
            CmdOutcome::TimedOut => ("failed".into(), "timeout".into()),
            CmdOutcome::Failed(why) => ("failed".into(), (*why).into()),
            CmdOutcome::Exited {
                success,
                code,
                stdout,
            } => {
                if self.prints_json() {
                    match set_result(stdout) {
                        Some(r) => r,
                        None => ("failed".into(), "no result from ctl.sh".into()),
                    }
                } else if *success {
                    ("ok".into(), String::new())
                } else {
                    let e =
                        code.map_or_else(|| "killed".to_owned(), |c| format!("exit status {c}"));
                    ("failed".into(), e)
                }
            }
        }
    }
}

/// `0` to `100`, decimal, no sign, no leading zero, at most 3 digits.
fn volume(v: &str) -> Option<u8> {
    if v.is_empty() || v.len() > 3 || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if v.len() > 1 && v.starts_with('0') {
        return None;
    }
    let n: u8 = v.parse().ok()?;
    (n <= 100).then_some(n)
}

/// The last stdout line that is a JSON object with a known `result`.
fn set_result(stdout: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(stdout).ok()?;
    text.lines().rev().find_map(|line| {
        let v: Value = serde_json::from_str(line.trim()).ok()?;
        let r = v.get("result")?.as_str()?;
        if !matches!(r, "applied" | "already" | "mismatch" | "failed" | "refused") {
            return None;
        }
        let e = v.get("error").and_then(Value::as_str).unwrap_or("");
        Some((r.to_owned(), crate::mirror::one_line(e)))
    })
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CmdOutcome {
    Exited {
        success: bool,
        code: Option<i32>,
        stdout: Vec<u8>,
    },
    TimedOut,
    Failed(&'static str),
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::assert_is_empty,
        clippy::format_push_string,
        clippy::cast_possible_truncation
    )]

    use super::*;

    fn a(body: &str) -> Option<Action> {
        Action::from_body(body.as_bytes())
    }

    #[test]
    fn every_row_builds_its_argv() {
        let rows: &[(&str, &[&str])] = &[
            (
                r#"{"action":"always-ready","value":"on"}"#,
                &["set", "always-ready", "on"],
            ),
            (
                r#"{"action":"always-ready","value":"off"}"#,
                &["set", "always-ready", "off"],
            ),
            (
                r#"{"action":"hi-lg","value":"on"}"#,
                &["set", "hi-lg", "on"],
            ),
            (
                r#"{"action":"hi-lg","value":"off"}"#,
                &["set", "hi-lg", "off"],
            ),
            (
                r#"{"action":"remote-off","value":"normal"}"#,
                &["set", "remote-off", "normal"],
            ),
            (
                r#"{"action":"remote-off","value":"keep"}"#,
                &["set", "remote-off", "keep"],
            ),
            (r#"{"action":"mic","value":"off"}"#, &["mic", "off"]),
            (r#"{"action":"mic","value":"on"}"#, &["mic", "on"]),
            (r#"{"action":"pair"}"#, &["pair"]),
            (r#"{"action":"pair","value":null}"#, &["pair"]),
            (
                r#"{"action":"reply-volume","value":"0"}"#,
                &["set", "reply-volume", "0"],
            ),
            (
                r#"{"action":"reply-volume","value":"40"}"#,
                &["set", "reply-volume", "40"],
            ),
            (
                r#"{"action":"reply-volume","value":"100"}"#,
                &["set", "reply-volume", "100"],
            ),
        ];
        for (body, argv) in rows {
            let act = a(body).unwrap_or_else(|| panic!("{body}"));
            assert_eq!(act.argv(), argv.to_vec(), "{body}");
        }
    }

    #[test]
    fn invalid_bodies_are_refused() {
        for body in [
            r#"{"action":"mic","value":"On"}"#,
            r#"{"action":"mic","value":"off ","x":1}"#,
            r#"{"action":"mic","value":"off","value":"on"}"#,
            r#"{"action":"mic","action":"pair"}"#,
            r#"{"action":"mic"}"#,
            r#"{"action":"mic","value":true}"#,
            r#"{"action":"pair","value":"x"}"#,
            r#"{"action":"start"}"#,
            r#"{"action":"reply-volume","value":"101"}"#,
            r#"{"action":"reply-volume","value":"-1"}"#,
            r#"{"action":"reply-volume","value":"040"}"#,
            r#"{"action":"reply-volume","value":"1000"}"#,
            r#"{"action":"reply-volume","value":"4 0"}"#,
            r#"{"action":"reply-volume","value":"40;reboot"}"#,
            r#"{"action":"reply-volume","value":40}"#,
            r#"{"action":"reply-volume","value":""}"#,
            r#"{"action":"remote-off","value":"keep_listening"}"#,
            r#"["mic","off"]"#,
            r#""mic""#,
            "",
            "{}",
            r#"{"action":"mic","value":"off"} x"#,
        ] {
            assert_eq!(a(body), None, "{body}");
        }
    }

    #[test]
    fn only_mic_off_skips_the_confirmation() {
        assert!(!Action::Mic(false).needs_confirmation());
        for act in [
            Action::Mic(true),
            Action::Pair,
            Action::AlwaysReady(false),
            Action::HiLg(false),
            Action::RemoteOff(false),
            Action::ReplyVolume(0),
        ] {
            assert!(act.needs_confirmation(), "{act:?}");
        }
    }

    #[test]
    fn preconditions() {
        let mut f = Facts::default();
        assert!(Action::Pair.check(&f).is_err());
        assert!(Action::HiLg(true).check(&f).is_err());
        assert!(Action::Mic(true).check(&f).is_ok());
        assert!(Action::ReplyVolume(5).check(&f).is_ok());
        assert!(Action::RemoteOff(true).check(&f).is_ok());
        f.luna = Some(true);
        f.always_ready_raw = Some("allEnabled".into());
        assert!(Action::AlwaysReady(true).check(&f).is_err());
        f.always_ready_raw = Some("alwaysReady".into());
        assert!(Action::AlwaysReady(false).check(&f).is_ok());
        assert!(Action::HiLg(true).check(&f).is_ok());
        f.luna = Some(false);
        assert!(Action::HiLg(true).check(&f).is_err());
        f.running = Some(true);
        f.key = Some("paired".into());
        assert!(Action::Pair.check(&f).is_err());
        f.key = Some("unpaired".into());
        assert!(Action::Pair.check(&f).is_ok());
        f.key = Some("cleared".into());
        assert!(Action::Pair.check(&f).is_ok());
        f.running = Some(false);
        assert!(Action::Pair.check(&f).is_err());
    }

    #[test]
    fn outcomes() {
        let ex = |success, out: &str| CmdOutcome::Exited {
            success,
            code: Some(i32::from(!success)),
            stdout: out.as_bytes().to_vec(),
        };
        let set = Action::HiLg(false);
        assert_eq!(
            set.outcome(&ex(
                true,
                "{\"schema\":1,\"result\":\"applied\",\"error\":\"\"}\n"
            )),
            ("applied".into(), String::new())
        );
        assert_eq!(
            set.outcome(&ex(
                false,
                "noise\n{\"result\":\"mismatch\",\"error\":\"the TV reports on\\nsecret\"}\n"
            )),
            ("mismatch".into(), "the TV reports on secret".into())
        );
        assert_eq!(set.outcome(&ex(true, "{\"result\":\"weird\"}")).0, "failed");
        assert_eq!(
            set.outcome(&ex(true, "")),
            ("failed".into(), "no result from ctl.sh".into())
        );
        assert_eq!(
            Action::Mic(false).outcome(&ex(true, "")),
            ("ok".into(), String::new())
        );
        assert_eq!(
            Action::Pair.outcome(&ex(false, "")),
            ("failed".into(), "exit status 1".into())
        );
        assert_eq!(
            Action::Pair.outcome(&CmdOutcome::TimedOut),
            ("failed".into(), "timeout".into())
        );
    }
}
