//! Token sanitizer and the fail-closed output scan (design 7.3).
//!
//! The report builder never receives raw dumps; the few free strings it does
//! take (model, firmware, kernel, card ids, power states, format) pass
//! [`sanitize_token`]: identifier-looking values are **dropped whole**
//! (`None`), everything else keeps only `[A-Za-z0-9._+-]` and single spaces up
//! to [`MAX_TOKEN`] characters. [`scan`] is the last gate over finished text.
//!
//! Limits, stated honestly: a short serial number with no pattern in a free
//! string cannot be told from a model name by any filter. The defence is that
//! no field takes a serial, device name, host or address at all, and that
//! patterns (MAC, IP, long mixed runs, URLs, paths, host suffixes) are dropped.

/// Longest sanitized token.
pub const MAX_TOKEN: usize = 48;
/// Longest allowed line in a report.
pub const MAX_LINE: usize = 200;
/// An alphanumeric run this long that contains a digit looks like a serial,
/// a key or a hash and is dropped from free strings.
const SERIAL_RUN: usize = 16;
/// A run of base64/hex characters this long fails the final scan.
const LONG_RUN: usize = 32;

const HOST_SUFFIXES: [&str; 7] = [
    ".local",
    ".lan",
    ".home",
    ".internal",
    ".localdomain",
    ".localhost",
    ".arpa",
];

/// A rule of the final scan, named for logging (the text that tripped it is
/// never logged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// A MAC address.
    Mac,
    /// An IPv4 address.
    Ipv4,
    /// An IPv6 address.
    Ipv6,
    /// A hex or base64 run of 32 or more characters.
    LongToken,
    /// A URL other than the project's GitHub repository.
    Url,
    /// A line over 200 characters.
    LongLine,
}

impl Rule {
    /// Stable name for the log line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Rule::Mac => "mac",
            Rule::Ipv4 => "ipv4",
            Rule::Ipv6 => "ipv6",
            Rule::LongToken => "long-token",
            Rule::Url => "url",
            Rule::LongLine => "long-line",
        }
    }
}

type Matcher = fn(&str) -> bool;

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn has_mac(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 17 {
        return false;
    }
    (0..=b.len() - 17).any(|i| {
        let sep = b[i + 2];
        (sep == b':' || sep == b'-')
            && (0..6).all(|k| {
                let j = i + 3 * k;
                is_hex(b[j]) && is_hex(b[j + 1]) && (k == 5 || b[j + 2] == sep)
            })
    })
}

fn has_ipv4(s: &str) -> bool {
    s.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .any(|run| {
            let groups: Vec<&str> = run.split('.').collect();
            groups.windows(4).any(|w| {
                w.iter().all(|g| {
                    (1..=3).contains(&g.len())
                        && g.bytes().all(|b| b.is_ascii_digit())
                        && g.parse::<u16>().is_ok_and(|v| v <= 255)
                })
            })
        })
}

fn has_ipv6(s: &str) -> bool {
    s.split(|c: char| !(c.is_ascii_hexdigit() || c == ':'))
        .any(|run| {
            run.bytes().filter(|&b| b == b':').count() >= 2
                && run.bytes().any(|b| b.is_ascii_hexdigit())
        })
}

fn longest_run(s: &str, keep: impl Fn(u8) -> bool) -> usize {
    let (mut best, mut cur) = (0, 0);
    for b in s.bytes() {
        if keep(b) {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

fn has_long_token(s: &str) -> bool {
    longest_run(s, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'_' | b'-')
    }) >= LONG_RUN
}

/// Alphanumeric runs that look like a serial, a key or a bare MAC.
fn has_serial_run(s: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric()).any(|run| {
        let digit = run.bytes().any(|b| b.is_ascii_digit());
        let all_hex = run.bytes().all(is_hex);
        (run.len() >= SERIAL_RUN && digit) || (run.len() == 12 && all_hex)
    })
}

fn has_host_suffix(s: &str) -> bool {
    s.split_whitespace().any(|w| {
        let w = w.to_ascii_lowercase();
        HOST_SUFFIXES.iter().any(|suf| w.ends_with(suf))
    })
}

/// True when `raw` carries something that must not be reported: a MAC, an IP,
/// a URL, an e-mail, a path, a host name with a domain suffix, a long hex or
/// base64 run, or a serial-like alphanumeric run.
#[must_use]
pub fn looks_like_identifier(raw: &str) -> bool {
    raw.contains("://")
        || raw.contains('@')
        || raw.contains('/')
        || raw.contains('\\')
        || has_mac(raw)
        || has_ipv4(raw)
        || has_ipv6(raw)
        || has_long_token(raw)
        || has_serial_run(raw)
        || has_host_suffix(raw)
}

/// Sanitizes a free string for a report. Returns `None` when the value looks
/// like an identifier (see [`looks_like_identifier`]) or nothing is left.
/// Otherwise keeps `[A-Za-z0-9._+-]` and single spaces, at most [`MAX_TOKEN`]
/// characters. The cleaned value is checked again because dropping characters
/// can create a pattern.
#[must_use]
pub fn sanitize_token(raw: &str) -> Option<String> {
    if looks_like_identifier(raw) {
        return None;
    }
    let mut out = String::new();
    let mut last_space = true;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-') {
            out.push(c);
            last_space = false;
        } else if c == ' ' && !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    let out = out.trim().chars().take(MAX_TOKEN).collect::<String>();
    let out = out.trim().to_string();
    if out.is_empty() || looks_like_identifier(&out) {
        None
    } else {
        Some(out)
    }
}

/// Keeps the leading numeric version of a kernel release (`5.4.96-lg+x` to
/// `5.4.96`), dropping any build string.
#[must_use]
pub fn numeric_version(raw: &str) -> Option<String> {
    let lead: String = raw
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let lead = lead.trim_end_matches('.');
    if lead.is_empty() || lead.len() > 24 {
        return None;
    }
    sanitize_token(lead)
}

/// The fail-closed scan over a finished text: no MAC, IPv4 or IPv6 address,
/// no hex/base64 run of 32 or more characters, no URL other than
/// `https://github.com/<repo>/`, no line over 200 characters.
///
/// # Errors
///
/// The first rule that matched.
pub fn scan(text: &str, repo: &str) -> Result<(), Rule> {
    let allowed = format!("https://github.com/{repo}/");
    let mut url_ok = true;
    for (i, _) in text.match_indices("http") {
        url_ok &= text[i..].starts_with(&allowed);
    }
    for (i, _) in text.match_indices("://") {
        url_ok &= i >= 5 && text[i - 5..].starts_with("https://");
    }
    if !url_ok {
        return Err(Rule::Url);
    }
    if text.lines().any(|l| l.chars().count() > MAX_LINE) {
        return Err(Rule::LongLine);
    }
    let rules: [(Rule, Matcher); 4] = [
        (Rule::Mac, has_mac),
        (Rule::Ipv4, has_ipv4),
        (Rule::Ipv6, has_ipv6),
        (Rule::LongToken, has_long_token),
    ];
    for (rule, f) in rules {
        // Check line by line so a pattern cannot span a line break, and
        // strip the allowed URL, which legitimately has a long path.
        if text.lines().any(|l| f(&l.replace(&allowed, ""))) {
            return Err(rule);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_normal_values() {
        for v in [
            "OLED42C34LA",
            "03.30.74",
            "8.3.0",
            "5.4.96",
            "armv7l",
            "LGSIC",
            "lg115x",
            "WoV PDM Mic snd-soc-dummy-dai-0",
            "S24_LE",
            "Screen Off",
            "Active Standby",
        ] {
            assert_eq!(sanitize_token(v).as_deref(), Some(v), "{v}");
        }
    }

    #[test]
    fn drops_disallowed_characters_and_collapses_spaces() {
        assert_eq!(sanitize_token("  A  (B) =C  ").as_deref(), Some("A B C"));
        assert_eq!(sanitize_token("").as_deref(), None);
        assert_eq!(sanitize_token("()=").as_deref(), None);
    }

    #[test]
    fn caps_at_48_characters() {
        let long = "ab ".repeat(40);
        let t = sanitize_token(&long).expect("token");
        assert!(t.chars().count() <= MAX_TOKEN);
    }

    #[test]
    fn identifier_like_values_are_dropped_whole() {
        for v in [
            "aa:bb:cc:dd:ee:ff",
            "AA-BB-CC-DD-EE-FF",
            "192.168.1.20",
            "fe80::1ff:fe23:4567:890a",
            "http://homeassistant.local:8123",
            "https://ha.example.org/api/webhook/abc",
            "me@example.org",
            "/var/run/nyx/os_info.json",
            "livingroom-tv.local",
            "tv.home.arpa",
            "LGTV7XK2M9QD4R8T1B",
            "712KCDM1A234ZXCVBNMQ",
            "aabbccddeeff",
            "dGhpcyBpcyBhIHNlY3JldCBrZXkgZm9yIHRoZSBzYXRlbGxpdGU=",
            "0123456789abcdef0123456789abcdef",
        ] {
            assert_eq!(sanitize_token(v), None, "{v}");
        }
    }

    #[test]
    fn a_pattern_created_by_dropping_characters_is_caught() {
        // "%" is dropped, leaving an IPv4 address.
        assert_eq!(sanitize_token("10.0.0%.1"), None);
    }

    #[test]
    fn kernel_version_keeps_only_the_number() {
        assert_eq!(
            numeric_version("5.4.96-lg+build.7").as_deref(),
            Some("5.4.96")
        );
        assert_eq!(numeric_version("5.4.96.").as_deref(), Some("5.4.96"));
        assert_eq!(numeric_version("custom"), None);
        assert_eq!(numeric_version(""), None);
    }

    #[test]
    fn scan_rules() {
        let repo = "pando85/webos-voice-satellite";
        assert_eq!(scan("result: works\napp: version=0.1.1", repo), Ok(()));
        assert_eq!(scan("x aa:bb:cc:dd:ee:ff y", repo), Err(Rule::Mac));
        assert_eq!(scan("x aa-bb-cc-dd-ee-ff y", repo), Err(Rule::Mac));
        assert_eq!(scan("ip 10.20.30.40", repo), Err(Rule::Ipv4));
        assert_eq!(
            scan("ip 03.30.74.1 ok", repo),
            Err(Rule::Ipv4),
            "fails closed"
        );
        assert_eq!(scan("fw 03.30.74", repo), Ok(()));
        assert_eq!(scan("ip fe80::1", repo), Err(Rule::Ipv6));
        assert_eq!(scan(&"a".repeat(32), repo), Err(Rule::LongToken));
        assert_eq!(scan(&"a".repeat(31), repo), Ok(()));
        assert_eq!(scan("see http://example.org", repo), Err(Rule::Url));
        assert_eq!(scan("see ftp://example.org", repo), Err(Rule::Url));
        assert_eq!(
            scan("see https://github.com/someone/else/x", repo),
            Err(Rule::Url)
        );
        assert_eq!(
            scan(
                "see https://github.com/pando85/webos-voice-satellite/issues/new",
                repo
            ),
            Ok(())
        );
        assert_eq!(
            scan(&format!("a\n{}", "b ".repeat(101)), repo),
            Err(Rule::LongLine)
        );
    }

    #[test]
    fn scan_does_not_trip_on_report_punctuation() {
        let repo = "pando85/webos-voice-satellite";
        let text = "webos-satellite compatibility report v1\nresult: works\npcm-card: id=LGSIC, play=2, cap=0-2\nlevels: window=10s, rms=-67.6, peak=-53.6, clipped=0, chunks=312, dropped=0\ntv: model=OLED42C34LA, webos=8.3.0, fw=03.30.74\nchecks: C1=pass, C6=warn(quiet)";
        assert_eq!(scan(text, repo), Ok(()));
    }
}
