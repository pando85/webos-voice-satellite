//! The allow-listed status mirror (section 8.4): fields copied one by one
//! from `ctl.sh status --json` and `ctl.sh tv-settings --json`, each value
//! re-checked, never the whole output (no prefix, log, pid, firmware string,
//! name, uid, build, compat report or key detail).

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

/// Facts the action preconditions use (section 9.1).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Facts {
    pub(crate) luna: Option<bool>,
    pub(crate) always_ready_raw: Option<String>,
    pub(crate) running: Option<bool>,
    pub(crate) key: Option<String>,
}

/// The cache the worker refreshes.
#[derive(Debug, Default)]
pub(crate) struct Mirror {
    device: Option<Value>,
    tv: Option<Value>,
    device_error: String,
    tv_error: String,
    updated: Option<Instant>,
}

impl Mirror {
    /// Stores one refresh: each part is the command's stdout or an error.
    pub(crate) fn update(
        &mut self,
        device: Result<Vec<u8>, String>,
        tv: Result<Vec<u8>, String>,
        now: Instant,
    ) {
        (self.device, self.device_error) = part(device, device_part);
        (self.tv, self.tv_error) = part(tv, tv_part);
        self.updated = Some(now);
    }

    pub(crate) fn facts(&self) -> Facts {
        let d = self.device.as_ref();
        let t = self.tv.as_ref();
        let s = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
        Facts {
            luna: t.and_then(|t| t.get("luna")).and_then(Value::as_bool),
            always_ready_raw: s(t.and_then(|t| t.pointer("/always_ready/raw"))),
            running: d.and_then(|d| d.get("running")).and_then(Value::as_bool),
            key: s(d.and_then(|d| d.get("key"))),
        }
    }

    /// `GET /api/state`.
    pub(crate) fn to_json(&self, pending: &Value, now: Instant) -> Value {
        let age = self
            .updated
            .map(|t| (now.saturating_duration_since(t).as_secs_f64() * 10.0).round() / 10.0);
        json!({
            "schema": 1,
            "age_s": age,
            "device": self.device,
            "tv": self.tv,
            "errors": {"device": self.device_error, "tv": self.tv_error},
            "pending": pending,
        })
    }

    pub(crate) fn age(&self, now: Instant) -> Option<Duration> {
        self.updated.map(|t| now.saturating_duration_since(t))
    }
}

fn part(r: Result<Vec<u8>, String>, f: fn(&Value) -> Value) -> (Option<Value>, String) {
    match r {
        Err(e) => (None, one_line(&e)),
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if v.is_object() => (Some(f(&v)), String::new()),
            _ => (None, "unreadable output".to_owned()),
        },
    }
}

/// One line of printable ASCII, at most 160 characters (control characters
/// become spaces, anything else is dropped).
pub(crate) fn one_line(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            ' '..='~' => Some(c),
            c if c.is_control() => Some(' '),
            _ => None,
        })
        .take(160)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A short word (letters, digits, `_ .:+-` and space, at most `max`), else null.
fn word(v: Option<&Value>, max: usize) -> Value {
    match v.and_then(Value::as_str) {
        Some(s)
            if s.len() <= max
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_ .:+-".contains(&b)) =>
        {
            Value::String(s.to_owned())
        }
        _ => Value::Null,
    }
}

fn boolean(v: Option<&Value>) -> Value {
    v.and_then(Value::as_bool).map_or(Value::Null, Value::Bool)
}

fn error_text(v: Option<&Value>) -> Value {
    Value::String(v.and_then(Value::as_str).map(one_line).unwrap_or_default())
}

/// A number from a JSON number or a digit string, within `0..=max`.
fn number(v: Option<&Value>, max: u64) -> Option<u64> {
    let n = match v? {
        Value::Number(n) => n.as_u64()?,
        Value::String(s)
            if !s.is_empty() && s.len() <= 6 && s.bytes().all(|b| b.is_ascii_digit()) =>
        {
            s.parse().ok()?
        }
        _ => return None,
    };
    (n <= max).then_some(n)
}

fn device_part(s: &Value) -> Value {
    let g = |k: &str| s.get(k);
    let key = match g("key").and_then(Value::as_str) {
        Some(k @ ("fixed" | "paired" | "unpaired" | "cleared" | "none")) => Value::from(k),
        _ => Value::Null,
    };
    let pairing = match g("pairing") {
        Some(Value::Object(p)) => json!({
            "open": boolean(p.get("open")),
            "ends_in_s": number(p.get("ends_in_s"), 86_400),
        }),
        _ => Value::Null,
    };
    // {"state": word, "since_s": N|null}: the HA API connection from
    // run/voice.json (P3). Only these two fields: no address, name or version.
    let ha = match g("ha") {
        Some(Value::Object(h)) => json!({
            "state": match h.get("state").and_then(Value::as_str) {
                Some(w @ ("connected" | "disconnected" | "unpaired" | "unknown")) => Value::from(w),
                _ => Value::Null,
            },
            "since_s": number(h.get("since_s"), 10 * 366 * 86_400),
        }),
        _ => Value::Null,
    };
    let default = number(g("reply_volume_default"), 100);
    let stored = number(g("reply_volume"), 100);
    json!({
        "installed": boolean(g("installed")),
        "installed_version": word(g("installed_version"), 32),
        "running": boolean(g("running")),
        "key": key,
        "port": number(g("port"), 65_535),
        "reply_volume": stored.or(default),
        "reply_volume_default": default,
        "pairing_window_s": number(g("pairing_window_s"), 86_400),
        // {"open": bool, "ends_in_s": N|null} from run/voice.json (P2).
        "pairing": pairing,
        "ha": ha,
    })
}

fn tv_part(t: &Value) -> Value {
    let p = |path: &str| t.pointer(path);
    let last_change = match t.get("last_change") {
        Some(Value::Object(m)) => {
            let mut out = Map::new();
            for k in ["setting", "from", "to", "at"] {
                out.insert(k.to_owned(), word(m.get(k), 32));
            }
            Value::Object(out)
        }
        _ => Value::Null,
    };
    json!({
        "luna": boolean(t.get("luna")),
        "always_ready": {
            "state": word(p("/always_ready/state"), 32),
            "raw": word(p("/always_ready/raw"), 32),
            "error": error_text(p("/always_ready/error")),
        },
        "hi_lg": {
            "state": word(p("/hi_lg/state"), 32),
            "thinq": boolean(p("/hi_lg/thinq")),
            "amazon": boolean(p("/hi_lg/amazon")),
            "dangbei": boolean(p("/hi_lg/dangbei")),
            "error": error_text(p("/hi_lg/error")),
        },
        "remote_off": {
            "state": word(p("/remote_off/state"), 32),
            "active": boolean(p("/remote_off/active")),
        },
        "mic": {
            "state": word(p("/mic/state"), 32),
            "kill_switch": boolean(p("/mic/kill_switch")),
        },
        "power_state": word(t.get("power_state"), 32),
        "last_change": last_change,
    })
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

    const STATUS: &str = r#"{"package_version":"0.4.0","installed":true,"installed_version":"0.4.0","build":"webos-satellite 0.4.0 (armv7)","update_pending":false,"uid":0,"machine":"armv7l","running":true,"pid":1234,"supervised":true,"autostart":false,"disabled":false,"crashloop":false,"mic_off":false,"firmware":"03.30.74 CANARY","firmware_ok":true,"firmware_verified":true,"compat":{"x":"CANARY"},"key":"unpaired","name":"CANARY-name","port":"6053","remote_off":"normal","reply_volume":"","reply_volume_default":"40","pairing_window_s":"600","pairing":{"open":true,"ends_in_s":321,"x":"CANARY"},"ha":{"state":"connected","since_s":42,"address":"CANARY-addr","version":"CANARY-ver","peer":"10.0.0.9"},"prefix":"/var/lib/CANARY","log":["CANARY log line"]}"#;
    const TV: &str = r#"{"schema":1,"luna":true,"daemon_running":true,"always_ready":{"state":"off","raw":"off","error":""},"hi_lg":{"state":"on","thinq":true,"amazon":false,"dangbei":null,"error":""},"remote_off":{"state":"keep_listening","active":true},"mic":{"state":"listening","kill_switch":false},"power_state":"Active","last_change":{"setting":"hi-lg","from":"off","to":"on","at":"2026-11-05T10:00:00Z"},"canary":"CANARY"}"#;

    fn mirror() -> (Mirror, Instant) {
        let now = Instant::now();
        let mut m = Mirror::default();
        m.update(Ok(STATUS.into()), Ok(TV.into()), now);
        (m, now)
    }

    #[test]
    fn only_allow_listed_fields_are_copied() {
        let (m, now) = mirror();
        let v = m.to_json(&Value::Null, now + Duration::from_millis(1200));
        let text = v.to_string();
        assert!(!text.contains("CANARY"), "{text}");
        for leaked in [
            "prefix", "\"log\"", "\"pid\"", "firmware", "\"name\"", "uid", "build", "machine",
        ] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
        assert_eq!(v["age_s"], json!(1.2));
        assert_eq!(v["device"]["port"], json!(6053));
        assert_eq!(v["device"]["reply_volume"], json!(40));
        assert_eq!(v["device"]["reply_volume_default"], json!(40));
        assert_eq!(v["device"]["key"], json!("unpaired"));
        assert_eq!(v["device"]["pairing_window_s"], json!(600));
        assert_eq!(
            v["device"]["pairing"],
            json!({"open": true, "ends_in_s": 321})
        );
        assert_eq!(
            v["device"]["ha"],
            json!({"state": "connected", "since_s": 42})
        );
        assert!(!text.contains("10.0.0.9") && !text.contains("address"));
        assert_eq!(v["tv"]["hi_lg"]["thinq"], json!(true));
        assert_eq!(v["tv"]["hi_lg"]["dangbei"], Value::Null);
        assert_eq!(v["tv"]["remote_off"]["state"], json!("keep_listening"));
        assert_eq!(v["tv"]["last_change"]["at"], json!("2026-11-05T10:00:00Z"));
        assert_eq!(
            m.facts(),
            Facts {
                luna: Some(true),
                always_ready_raw: Some("off".into()),
                running: Some(true),
                key: Some("unpaired".into()),
            }
        );
    }

    #[test]
    fn hostile_values_are_dropped() {
        let now = Instant::now();
        let mut m = Mirror::default();
        let status = r#"{"installed":"yes","installed_version":"<script>","running":1,"key":"stolen","port":"99999","reply_volume":"250","reply_volume_default":"-3"}"#;
        let tv = r#"{"luna":true,"always_ready":{"state":"off\nx","raw":"off","error":"line1\nline2 é"},"power_state":"Active Standby"}"#;
        m.update(Ok(status.into()), Ok(tv.into()), now);
        let v = m.to_json(&Value::Null, now);
        assert_eq!(v["device"]["installed"], Value::Null);
        assert_eq!(v["device"]["installed_version"], Value::Null);
        assert_eq!(v["device"]["running"], Value::Null);
        assert_eq!(v["device"]["key"], Value::Null);
        assert_eq!(v["device"]["port"], Value::Null);
        assert_eq!(v["device"]["reply_volume"], Value::Null);
        assert_eq!(v["tv"]["always_ready"]["state"], Value::Null);
        assert_eq!(v["tv"]["always_ready"]["error"], json!("line1 line2"));
        assert_eq!(v["tv"]["power_state"], json!("Active Standby"));
        assert_eq!(v["tv"]["hi_lg"]["state"], Value::Null);
    }

    /// `ha` (P3): only `state` from its four words and `since_s` as a bounded
    /// whole number; anything else is null, and a non-object `ha` is null.
    #[test]
    fn ha_connection_is_allow_listed() {
        let now = Instant::now();
        let mut m = Mirror::default();
        for (ha, want) in [
            (
                r#"{"state":"unknown","since_s":null}"#,
                json!({"state": "unknown", "since_s": null}),
            ),
            (
                r#"{"state":"disconnected","since_s":"17"}"#,
                json!({"state": "disconnected", "since_s": 17}),
            ),
            (
                r#"{"state":"connected to 10.0.0.9","since_s":-1}"#,
                json!({"state": null, "since_s": null}),
            ),
            (
                r#"{"state":"unpaired","since_s":1e9,"host":"10.0.0.9"}"#,
                json!({"state": "unpaired", "since_s": null}),
            ),
            (r#""connected""#, Value::Null),
        ] {
            let status = format!(r#"{{"installed":true,"ha":{ha}}}"#);
            m.update(Ok(status.into_bytes()), Err("x".into()), now);
            let v = m.to_json(&Value::Null, now);
            assert_eq!(v["device"]["ha"], want, "{ha}");
            assert!(!v.to_string().contains("10.0.0.9"), "{ha}");
        }
        m.update(Ok(br#"{"installed":true}"#.to_vec()), Err("x".into()), now);
        assert_eq!(m.to_json(&Value::Null, now)["device"]["ha"], Value::Null);
    }

    #[test]
    fn a_failed_part_is_null_with_an_error() {
        let now = Instant::now();
        let mut m = Mirror::default();
        m.update(Ok(STATUS.into()), Err("timeout".into()), now);
        let v = m.to_json(&Value::Null, now);
        assert!(v["device"].is_object());
        assert_eq!(v["tv"], Value::Null);
        assert_eq!(v["errors"]["tv"], json!("timeout"));
        m.update(Ok(b"not json".to_vec()), Ok(b"[1]".to_vec()), now);
        let v = m.to_json(&Value::Null, now);
        assert_eq!(v["device"], Value::Null);
        assert_eq!(v["errors"]["device"], json!("unreadable output"));
        assert_eq!(v["errors"]["tv"], json!("unreadable output"));
        assert_eq!(m.facts(), Facts::default());
        assert_eq!(
            Mirror::default().to_json(&Value::Null, now)["age_s"],
            Value::Null
        );
    }

    #[test]
    fn one_line_cleans() {
        assert_eq!(one_line("a\nb\r\tc"), "a b  c");
        assert_eq!(one_line(&"x".repeat(500)).len(), 160);
        assert_eq!(one_line("\u{1b}[31mred"), "[31mred");
    }
}
