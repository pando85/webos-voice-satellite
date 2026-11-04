//! The prefilled GitHub issue link (design 6.2).
//!
//! `https://github.com/<repo>/issues/new?template=compatibility-report.yml&title=<enc>&report=<enc>`
//!
//! No `body`, no `labels`: the form file supplies the label, and `report` is
//! the id of the form's textarea (`.github/ISSUE_TEMPLATE/compatibility-report.yml`).
//! The `report` field id is `hypothesis` until a prefilled link is tried on
//! GitHub, including how a `render: text` textarea takes the value.

/// Repository the link points at unless `[report] repo` says otherwise.
pub const DEFAULT_REPO: &str = "pando85/webos-voice-satellite";
/// Issue form file name under `.github/ISSUE_TEMPLATE/`.
pub const TEMPLATE: &str = "compatibility-report.yml";
/// Issue form textarea id that receives the short report.
pub const REPORT_FIELD: &str = "report";
/// Longest link the builder produces (design 7.4).
pub const MAX_URL_LEN: usize = 1500;

/// Whether `repo` looks like `owner/name` with GitHub-legal characters only.
#[must_use]
pub fn valid_repo(repo: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            && s != "."
            && s != ".."
    };
    repo.split_once('/')
        .is_some_and(|(owner, name)| ok(owner) && ok(name))
}

/// Percent-encodes UTF-8, keeping only the unreserved characters
/// `A-Z a-z 0-9 - . _ ~`.
#[must_use]
pub fn percent_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    out
}

/// Decodes [`percent_encode`] output (`+` is not a space here).
#[must_use]
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Builds the issue link. The caller has validated `repo` ([`valid_repo`]).
#[must_use]
pub fn issue_url(repo: &str, title: &str, report: &str) -> String {
    format!(
        "https://github.com/{repo}/issues/new?template={TEMPLATE}&title={}&{REPORT_FIELD}={}",
        percent_encode(title),
        percent_encode(report)
    )
}

/// A parsed issue link (for tests and for checking a QR payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUrl {
    /// `owner/name`.
    pub repo: String,
    /// `template` parameter.
    pub template: String,
    /// Decoded `title`.
    pub title: String,
    /// Decoded `report`.
    pub report: String,
}

/// Parses a link made by [`issue_url`]; `None` for anything else.
#[must_use]
pub fn parse_issue_url(url: &str) -> Option<ParsedUrl> {
    let rest = url.strip_prefix("https://github.com/")?;
    let (repo, query) = rest.split_once("/issues/new?")?;
    let (mut template, mut title, mut report) = (None, None, None);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        match k {
            "template" => template = Some(percent_decode(v)?),
            "title" => title = Some(percent_decode(v)?),
            REPORT_FIELD => report = Some(percent_decode(v)?),
            _ => return None,
        }
    }
    Some(ParsedUrl {
        repo: repo.to_string(),
        template: template?,
        title: title?,
        report: report?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_validation() {
        assert!(valid_repo(DEFAULT_REPO));
        assert!(valid_repo("a/b"));
        for bad in [
            "", "a", "a/", "/b", "a/b/c", "a b/c", "a/b?x=1", "../b", "a/..", "a/b&c",
        ] {
            assert!(!valid_repo(bad), "{bad}");
        }
    }

    #[test]
    fn encoding_keeps_only_unreserved() {
        assert_eq!(percent_encode("a b\n[x]: é"), "a%20b%0A%5Bx%5D%3A%20%C3%A9");
        assert_eq!(percent_encode("AZaz09-._~"), "AZaz09-._~");
    }

    #[test]
    fn round_trips_every_byte_sequence_we_can_build() {
        let s = "webos-satellite\nresult: works\n[Compat] 100% & more = ? # / \\ \"q\" é€😀";
        assert_eq!(percent_decode(&percent_encode(s)).as_deref(), Some(s));
    }

    #[test]
    fn decode_rejects_bad_input() {
        assert_eq!(percent_decode("%"), None);
        assert_eq!(percent_decode("%zz"), None);
        assert_eq!(percent_decode("%FF"), None, "not UTF-8");
    }

    #[test]
    fn url_shape_and_round_trip() {
        let u = issue_url(DEFAULT_REPO, "[Compat] X fw 1: works", "a\nb");
        assert!(u.starts_with(
            "https://github.com/pando85/webos-voice-satellite/issues/new?template=compatibility-report.yml&title="
        ));
        assert!(u.contains("&report=a%0Ab"));
        assert!(!u.contains("body="), "no body");
        assert!(!u.contains("labels="), "no labels");
        let p = parse_issue_url(&u).expect("parse");
        assert_eq!(p.repo, DEFAULT_REPO);
        assert_eq!(p.template, "compatibility-report.yml");
        assert_eq!(p.title, "[Compat] X fw 1: works");
        assert_eq!(p.report, "a\nb");
    }

    #[test]
    fn foreign_links_do_not_parse() {
        assert_eq!(parse_issue_url("https://example.org/x"), None);
        assert_eq!(
            parse_issue_url("https://github.com/a/b/issues/new?template=t&title=x&body=y"),
            None
        );
    }

    #[test]
    fn form_field_ids_match_the_issue_form_file() {
        let form = include_str!("../../../.github/ISSUE_TEMPLATE/compatibility-report.yml");
        assert!(form.contains(&format!("id: {REPORT_FIELD}")));
        assert!(form.contains("title: \"[Compat] \""));
    }
}
