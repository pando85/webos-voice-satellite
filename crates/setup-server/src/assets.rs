//! The embedded phone page and its assets (section 11): served from the
//! binary, never from the filesystem, through this fixed table.
//!
//! The files under `crates/setup-server/assets/` are **placeholders** until
//! the phone page (step P6) and the shared `voicepower.js` (step P5) exist.
//! To switch, point the `include_bytes!` paths below at
//! `packaging/webos/app/phone/{phone.html,phone.js,phone.css}` and
//! `packaging/webos/app/voicepower.js`, and keep the names: the page loads
//! them as `/assets/<name>`. Nothing else in the server changes.

pub(crate) struct Asset {
    pub(crate) name: &'static str,
    pub(crate) content_type: &'static str,
    pub(crate) body: &'static [u8],
}

/// `GET /` (with a session).
pub(crate) const PAGE: &[u8] = include_bytes!("../assets/phone.html");

/// `GET /assets/<name>` (with a session).
pub(crate) const ASSETS: &[Asset] = &[
    Asset {
        name: "phone.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("../assets/phone.js"),
    },
    Asset {
        name: "phone.css",
        content_type: "text/css; charset=utf-8",
        body: include_bytes!("../assets/phone.css"),
    },
    Asset {
        name: "voicepower.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("../assets/voicepower.js"),
    },
];

/// `GET /` without a valid session (403).
pub(crate) const NO_SESSION_PAGE: &[u8] = include_bytes!("../assets/no-session.html");
/// A second use of the QR link (410).
pub(crate) const LINK_USED_PAGE: &[u8] = include_bytes!("../assets/link-used.html");

/// Responses are at most 128 KiB (section 6.4); assets 60 KB in total.
const fn total() -> usize {
    let mut n = PAGE.len();
    let mut i = 0;
    while i < ASSETS.len() {
        n += ASSETS[i].body.len();
        i += 1;
    }
    n
}
const _: () = assert!(total() <= 60 * 1024, "phone assets over the 60 KB budget");

pub(crate) fn find(name: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.name == name)
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

    #[test]
    fn pages_reference_only_local_assets() {
        let page = std::str::from_utf8(PAGE).expect("utf8");
        for a in ASSETS {
            assert!(!a.body.is_empty(), "{}", a.name);
        }
        assert!(page.contains("/assets/phone.js"));
        // The CSP forbids inline script and style and remote assets.
        for p in [PAGE, NO_SESSION_PAGE, LINK_USED_PAGE] {
            let text = std::str::from_utf8(p).expect("utf8");
            assert!(
                !text.contains("<script>") && !text.contains("style=") && !text.contains("<style")
            );
            assert!(!text.contains("http://") && !text.contains("https://"));
        }
    }
}
