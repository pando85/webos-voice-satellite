//! Fetch target policy for music streams (design M3b, default D14).
//!
//! The daemon runs as root and fetches URLs that any Home Assistant client
//! (or Music Assistant) names, so a stream may only reach the LAN or the
//! internet, never the TV itself:
//!
//! - only `http` and `https`; at most [`MAX_URL_LEN`] bytes;
//! - at most `StreamConfig::max_redirects` (3) redirects, each target
//!   checked like the first, and never from `https` to `http`;
//! - the address is checked **after DNS resolution, on the set of addresses
//!   ureq actually connects to** (a connector in front of the TCP connector,
//!   so there is no second lookup and no rebinding gap): loopback,
//!   unspecified (`0.0.0.0/8`, `::`), link-local (`169.254.0.0/16`,
//!   `fe80::/10`), multicast and broadcast, and **any address of this
//!   device's own interfaces** are refused; IPv4-mapped IPv6 addresses are
//!   checked as IPv4. Private LAN ranges are allowed (HA and MA live there);
//! - response headers at most [`MAX_HEADER_BYTES`];
//! - no environment proxy (the check must see the real target);
//! - no URL in any error text; [`redact_url`] for log lines.
//!
//! "This device's addresses" are listed from the interfaces (`getifaddrs`,
//! through the `if-addrs` crate) on every check, so a DHCP change is seen
//! and nothing depends on the kernel's bind behaviour (`ip_nonlocal_bind`
//! makes bind probes useless). If the list cannot be read, [`OwnAddrs::Refuse`]
//! (music) refuses the target, [`OwnAddrs::Skip`] (TTS and announcements)
//! skips only that one rule, with one warning in the log, and keeps the rest.

use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ureq::unversioned::transport::{ConnectionDetails, Connector};

use crate::error::PlaybackError;
use crate::stream::lock;

/// Longest URL accepted (the first one and every redirect target).
pub const MAX_URL_LEN: usize = 4096;
/// Largest response header block accepted.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// What to do when this device's own addresses cannot be listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnAddrs {
    /// Refuse the target (fail closed): the music stream, which is opt-in.
    Refuse,
    /// Skip the own-address rule only, log a warning once, keep every other
    /// rule: TTS and announcement fetches, which must not break.
    Skip,
}

/// The fixed address rules (everything but "this device's own addresses").
fn check_fixed(ip: IpAddr) -> Result<(), &'static str> {
    match ip {
        IpAddr::V4(a) => {
            if a.is_loopback() {
                return Err("loopback address");
            }
            if a.octets()[0] == 0 {
                return Err("unspecified address");
            }
            if a.is_link_local() {
                return Err("link-local address");
            }
            if a.is_multicast() || a.is_broadcast() {
                return Err("multicast or broadcast address");
            }
        }
        IpAddr::V6(a) => {
            if a.is_loopback() {
                return Err("loopback address");
            }
            if a.is_unspecified() {
                return Err("unspecified address");
            }
            if a.segments()[0] & 0xffc0 == 0xfe80 {
                return Err("link-local address");
            }
            if a.is_multicast() {
                return Err("multicast or broadcast address");
            }
        }
    }
    Ok(())
}

/// The addresses of this device's interfaces (canonical form), all of them,
/// up or down. Read afresh on every call: a fetch is rare and the list
/// changes with DHCP.
fn own_addresses() -> io::Result<Vec<IpAddr>> {
    Ok(if_addrs::get_if_addrs()?
        .iter()
        .map(|i| i.ip().to_canonical())
        .collect())
}

/// Why a target address is refused, or `Ok` when it may be fetched.
///
/// `own` lists this device's addresses, or `None` when they could not be
/// read; `on_unknown` then decides (see [`OwnAddrs`]).
fn check_with(
    ip: IpAddr,
    own: Option<&[IpAddr]>,
    on_unknown: OwnAddrs,
) -> Result<(), &'static str> {
    let ip = ip.to_canonical();
    check_fixed(ip)?;
    match own {
        Some(list) if list.contains(&ip) => Err("address of this device"),
        Some(_) => Ok(()),
        None => match on_unknown {
            OwnAddrs::Refuse => Err("cannot tell this device's addresses"),
            OwnAddrs::Skip => {
                static WARNED: AtomicBool = AtomicBool::new(false);
                if !WARNED.swap(true, Ordering::Relaxed) {
                    log::warn!(
                        "cannot list this device's addresses; the own-address rule of the fetch guard is skipped (the other rules still apply)"
                    );
                }
                Ok(())
            }
        },
    }
}

/// Why a target address is refused, or `Ok` when it may be fetched.
///
/// # Errors
/// A short reason (never the address) for loopback, unspecified,
/// link-local, multicast or broadcast addresses and for this device's own
/// addresses (or, with [`OwnAddrs::Refuse`], when those cannot be listed).
pub fn check_addr(ip: IpAddr, on_unknown: OwnAddrs) -> Result<(), &'static str> {
    // Only interface addresses need the list; skip the syscall for the
    // fixed refusals.
    let ip = ip.to_canonical();
    check_fixed(ip)?;
    let own = own_addresses().ok();
    check_with(ip, own.as_deref(), on_unknown)
}

/// Checks the scheme and length of a URL.
///
/// # Errors
/// [`PlaybackError::BadUrl`] for another scheme, [`PlaybackError::TargetRefused`]
/// for a URL longer than [`MAX_URL_LEN`] or with control characters.
pub fn check_url(url: &str) -> Result<(), PlaybackError> {
    if url.len() > MAX_URL_LEN {
        return Err(PlaybackError::TargetRefused("URL too long".into()));
    }
    if url.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return Err(PlaybackError::TargetRefused(
            "URL contains spaces or control characters".into(),
        ));
    }
    if scheme_of(url).is_none() {
        return Err(PlaybackError::BadUrl("scheme must be http or https".into()));
    }
    Ok(())
}

/// `"http"` or `"https"` (lower case), `None` for anything else.
fn scheme_of(url: &str) -> Option<&'static str> {
    let (s, rest) = url.split_once("://")?;
    if rest.is_empty() {
        return None;
    }
    if s.eq_ignore_ascii_case("https") {
        Some("https")
    } else if s.eq_ignore_ascii_case("http") {
        Some("http")
    } else {
        None
    }
}

/// `scheme://authority` of an absolute http(s) URL.
fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    let end = url[after..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| after + i);
    &url[..end]
}

/// Resolves a `Location` header against the URL that answered with it and
/// applies the redirect rules: http(s) only, no `https` to `http`, length
/// and character caps.
///
/// # Errors
/// [`PlaybackError::TargetRefused`] or [`PlaybackError::BadUrl`]; the text
/// never contains either URL.
pub fn next_url(base: &str, location: &str) -> Result<String, PlaybackError> {
    let loc = location.trim();
    if loc.is_empty() {
        return Err(PlaybackError::TargetRefused("empty redirect".into()));
    }
    let base_scheme = scheme_of(base).ok_or_else(|| PlaybackError::BadUrl("bad base".into()))?;
    let resolved = if scheme_of(loc).is_some() {
        loc.to_owned()
    } else if let Some(rest) = loc.strip_prefix("//") {
        format!("{base_scheme}://{rest}")
    } else if loc.starts_with('/') {
        format!("{}{loc}", origin(base))
    } else if loc
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|first| first.contains(':'))
    {
        // Another scheme (`file:`, `ftp:`, `data:` ...).
        return Err(PlaybackError::BadUrl(
            "redirect scheme must be http or https".into(),
        ));
    } else {
        let path_end = base.find(['?', '#']).unwrap_or(base.len());
        let path = &base[..path_end];
        let o = origin(base);
        if loc.starts_with('?') {
            format!("{path}{loc}")
        } else {
            let dir = match path[o.len()..].rfind('/') {
                Some(i) => &path[..=(o.len() + i)],
                None => "",
            };
            if dir.is_empty() {
                format!("{o}/{loc}")
            } else {
                format!("{dir}{loc}")
            }
        }
    };
    check_url(&resolved)?;
    if base_scheme == "https" && scheme_of(&resolved) == Some("http") {
        return Err(PlaybackError::TargetRefused(
            "redirect from https to http".into(),
        ));
    }
    Ok(resolved)
}

/// `scheme://host/first-segment/…` without query or the rest of the path,
/// for log lines (stream URLs carry tokens in the path or the query).
#[must_use]
pub fn redact_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("?", url));
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let mut parts = rest.splitn(3, '/');
    // Drop any user:password@ from the authority.
    let host = parts.next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    match parts.next() {
        Some(seg) if !seg.is_empty() => format!("{scheme}://{host}/{seg}/…"),
        _ => format!("{scheme}://{host}/"),
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// GET `url` on a guarded agent (built with `max_redirects(0)` and a
/// [`GuardConnector`]), following at most `max_redirects` redirects by hand
/// so every hop goes through [`next_url`] and the connector again.
/// `timeout` gives the per-request overall deadline of each hop (`None`
/// keeps the agent's settings); `map` turns a ureq error into ours.
///
/// # Errors
/// The mapped transport error, [`PlaybackError::TargetRefused`] for too
/// many redirects or a refused hop, [`PlaybackError::Status`] for a
/// non-success status.
pub(crate) fn get_following(
    agent: &ureq::Agent,
    url: &str,
    max_redirects: u8,
    mut timeout: impl FnMut() -> Option<std::time::Duration>,
    map: impl Fn(ureq::Error) -> PlaybackError,
) -> Result<ureq::http::Response<ureq::Body>, PlaybackError> {
    let mut url = url.to_owned();
    let mut hops = 0u8;
    loop {
        let req = agent.get(&url);
        let resp = match timeout() {
            Some(t) => req.config().timeout_global(Some(t)).build().call(),
            None => req.call(),
        }
        .map_err(&map)?;
        let status = resp.status().as_u16();
        if is_redirect(status) {
            hops += 1;
            if hops > max_redirects {
                return Err(PlaybackError::TargetRefused("too many redirects".into()));
            }
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| PlaybackError::Http("redirect without a location".into()))?;
            url = next_url(&url, location)?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(PlaybackError::Status(status));
        }
        return Ok(resp);
    }
}

/// First connector of the music agent's chain: checks every resolved
/// address before the TCP connector uses them. The reason of a refusal is
/// left in `refused` (ureq's error type cannot carry it).
#[derive(Debug)]
pub(crate) struct GuardConnector {
    /// Tests only: let loopback through (all other checks stay).
    pub(crate) allow_loopback: bool,
    /// Failure policy of the own-address rule.
    pub(crate) on_unknown: OwnAddrs,
    pub(crate) refused: Arc<Mutex<Option<&'static str>>>,
}

impl Connector<()> for GuardConnector {
    type Out = ();

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<()>,
    ) -> Result<Option<()>, ureq::Error> {
        if details.addrs.is_empty() {
            *lock(&self.refused) = Some("no address");
            return Err(refused_error());
        }
        // Every address ureq may try is checked, so whichever one the TCP
        // connector ends up connected to was allowed.
        // One listing of the device's addresses for the whole set.
        let own = own_addresses().ok();
        for a in &details.addrs {
            let ip = a.ip().to_canonical();
            if self.allow_loopback && ip.is_loopback() {
                continue;
            }
            if let Err(reason) = check_with(ip, own.as_deref(), self.on_unknown) {
                *lock(&self.refused) = Some(reason);
                return Err(refused_error());
            }
        }
        Ok(chained)
    }
}

fn refused_error() -> ureq::Error {
    ureq::Error::Io(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "target refused",
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    }

    #[test]
    fn refused_and_allowed_addresses() {
        for (a, why) in [
            ("127.0.0.1", "loopback address"),
            ("127.8.9.10", "loopback address"),
            ("::1", "loopback address"),
            ("::ffff:127.0.0.1", "loopback address"),
            ("0.0.0.0", "unspecified address"),
            ("0.1.2.3", "unspecified address"),
            ("::", "unspecified address"),
            ("169.254.1.1", "link-local address"),
            ("::ffff:169.254.169.254", "link-local address"),
            ("fe80::1", "link-local address"),
            ("febf::1", "link-local address"),
            ("224.0.0.1", "multicast or broadcast address"),
            ("255.255.255.255", "multicast or broadcast address"),
            ("ff02::1", "multicast or broadcast address"),
        ] {
            assert_eq!(check_addr(ip(a), OwnAddrs::Refuse), Err(why), "{a}");
            assert_eq!(check_addr(ip(a), OwnAddrs::Skip), Err(why), "{a} (skip)");
        }
        // Private LAN ranges and public addresses (TEST-NET, documentation
        // prefixes: never assigned to this host).
        for a in [
            "192.0.2.10",
            "198.51.100.7",
            "203.0.113.5",
            "2001:db8::5",
            "::ffff:203.0.113.5",
        ] {
            assert_eq!(check_addr(ip(a), OwnAddrs::Refuse), Ok(()), "{a}");
        }
    }

    #[test]
    fn own_interface_addresses_are_refused() {
        // Every address getifaddrs reports that the fixed rules let through
        // is refused as this device's, with no network traffic and whatever
        // ip_nonlocal_bind is.
        let Ok(list) = own_addresses() else {
            eprintln!("no interface list; skipped");
            return;
        };
        for a in &list {
            if check_fixed(*a).is_ok() {
                for p in [OwnAddrs::Refuse, OwnAddrs::Skip] {
                    assert_eq!(check_addr(*a, p), Err("address of this device"));
                }
            }
        }
    }

    #[test]
    fn own_rule_uses_the_list_and_is_not_a_bind_probe() {
        // A kernel with ip_nonlocal_bind would bind anything; the list does
        // not care. Unlisted addresses pass, listed ones are refused.
        let mine = ip("192.0.2.77");
        let other = ip("192.0.2.78");
        let own = [mine, ip("2001:db8::77")];
        for p in [OwnAddrs::Refuse, OwnAddrs::Skip] {
            assert_eq!(
                check_with(mine, Some(&own), p),
                Err("address of this device")
            );
            assert_eq!(
                check_with(ip("::ffff:192.0.2.77"), Some(&own), p),
                Err("address of this device")
            );
            assert_eq!(
                check_with(ip("2001:db8::77"), Some(&own), p),
                Err("address of this device")
            );
            assert_eq!(check_with(other, Some(&own), p), Ok(()));
            assert_eq!(check_with(ip("2001:db8::78"), Some(&own), p), Ok(()));
            // The fixed rules come first.
            assert_eq!(
                check_with(ip("127.0.0.1"), Some(&own), p),
                Err("loopback address")
            );
        }
    }

    #[test]
    fn unreadable_list_fails_closed_for_music_and_open_for_replies() {
        assert_eq!(
            check_with(ip("192.0.2.9"), None, OwnAddrs::Refuse),
            Err("cannot tell this device's addresses")
        );
        assert_eq!(check_with(ip("192.0.2.9"), None, OwnAddrs::Skip), Ok(()));
        // Skip drops only the own-address rule.
        for (a, why) in [
            ("127.0.0.1", "loopback address"),
            ("169.254.1.1", "link-local address"),
            ("224.0.0.1", "multicast or broadcast address"),
        ] {
            assert_eq!(check_with(ip(a), None, OwnAddrs::Skip), Err(why));
        }
    }

    #[test]
    fn redirects_resolve_and_follow_the_rules() {
        let b = "http://ha.lan:8123/api/esphome/ffmpeg_proxy/d/x.flac?t=1";
        assert_eq!(next_url(b, "/other").unwrap(), "http://ha.lan:8123/other");
        assert_eq!(
            next_url(b, "y.flac").unwrap(),
            "http://ha.lan:8123/api/esphome/ffmpeg_proxy/d/y.flac"
        );
        assert_eq!(
            next_url(b, "?t=2").unwrap(),
            "http://ha.lan:8123/api/esphome/ffmpeg_proxy/d/x.flac?t=2"
        );
        assert_eq!(next_url(b, "//ma.lan/s").unwrap(), "http://ma.lan/s");
        assert_eq!(
            next_url("http://h", "rel").unwrap(),
            "http://h/rel",
            "no path in the base"
        );
        assert_eq!(next_url(b, "HTTPS://x/y").unwrap(), "HTTPS://x/y");
        // Downgrade and other schemes.
        let s = "https://ha.lan/a";
        assert!(matches!(
            next_url(s, "http://ha.lan/b"),
            Err(PlaybackError::TargetRefused(_))
        ));
        assert!(matches!(
            next_url("https://ha.lan/a", "//x/y"),
            Ok(u) if u == "https://x/y"
        ));
        assert_eq!(next_url(s, "https://x/").unwrap(), "https://x/");
        for bad in [
            "file:///etc/passwd",
            "ftp://x/",
            "data:audio/flac,xx",
            "javascript:x",
        ] {
            assert!(next_url(b, bad).is_err(), "{bad}");
        }
        assert!(next_url(b, "").is_err());
        assert!(next_url(b, "/a b").is_err());
        let long = format!("/{}", "a".repeat(MAX_URL_LEN));
        assert!(matches!(
            next_url(b, &long),
            Err(PlaybackError::TargetRefused(_))
        ));
        // Errors never carry the URLs.
        let e = next_url(s, "http://secret-host/token")
            .unwrap_err()
            .to_string();
        assert!(!e.contains("secret") && !e.contains("token"), "{e}");
    }

    #[test]
    fn url_checks() {
        assert!(check_url("http://h/x").is_ok());
        assert!(check_url("HTTPS://h/x").is_ok());
        assert!(matches!(
            check_url("ftp://h/x"),
            Err(PlaybackError::BadUrl(_))
        ));
        assert!(matches!(
            check_url("http://"),
            Err(PlaybackError::BadUrl(_))
        ));
        assert!(check_url("http://h/\n").is_err());
        assert!(check_url(&format!("http://h/{}", "a".repeat(MAX_URL_LEN))).is_err());
    }

    #[test]
    fn redaction() {
        assert_eq!(
            redact_url("http://ha:8123/api/esphome/ffmpeg_proxy/dev/tok.flac?x=1"),
            "http://ha:8123/api/…"
        );
        assert_eq!(
            redact_url("https://u:p@ma.lan/flow"),
            "https://ma.lan/flow/…"
        );
        assert_eq!(redact_url("http://h?token=1"), "http://h/");
    }
}
