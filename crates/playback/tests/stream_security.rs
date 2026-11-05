//! M3b: fetch target rules of the music stream, end to end against local
//! servers. Production refuses loopback; these tests allow only loopback
//! (`insecure_allow_loopback`) so every other rule still applies.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::io::Write;
use std::net::{IpAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{Framing, flac_bytes, read_head, respond, serve, serve_body, serve_on, sine};
use webos_playback::PlaybackError;
use webos_playback::guard::{MAX_HEADER_BYTES, MAX_URL_LEN};
use webos_playback::stream::{self, StreamConfig};

fn strict() -> StreamConfig {
    StreamConfig {
        idle_timeout: Duration::from_secs(2),
        ..StreamConfig::default()
    }
}

fn loopback_ok() -> StreamConfig {
    StreamConfig {
        insecure_allow_loopback: true,
        ..strict()
    }
}

/// The terminal item of a stream (`None` for a clean end) and the samples.
fn end_of(url: &str, cfg: &StreamConfig) -> (usize, Option<PlaybackError>) {
    let mut s = stream::open(url, cfg).unwrap();
    let mut n = 0;
    while let Some(x) = s.blocking_recv() {
        match x {
            Ok(c) => n += c.samples.len(),
            Err(e) => return (n, Some(e)),
        }
    }
    (n, None)
}

fn refused(e: Option<&PlaybackError>, why: &str) -> bool {
    matches!(e, Some(PlaybackError::TargetRefused(r)) if r == why)
}

/// This host's primary IPv4 address (no packet is sent).
fn own_ipv4() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback()).then_some(ip)
}

fn tone_server() -> common::Server {
    serve_body(
        "audio/flac",
        flac_bytes(&sine(48_000, 2, 440.0, 0.2), 4096),
        Framing::Chunked,
    )
}

#[test]
fn loopback_is_refused_without_connecting() {
    let srv = tone_server();
    for url in [
        srv.url("/x.flac"),
        format!("http://localhost:{}/x.flac", srv.addr.port()),
        format!("http://[::ffff:127.0.0.1]:{}/x.flac", srv.addr.port()),
    ] {
        let (n, e) = end_of(&url, &strict());
        assert_eq!(n, 0);
        assert!(refused(e.as_ref(), "loopback address"), "{url}: {e:?}");
    }
    assert_eq!(srv.hits.load(Ordering::SeqCst), 0, "no connection was made");
}

#[test]
fn this_devices_own_address_is_refused_without_connecting() {
    let Some(ip) = own_ipv4() else {
        eprintln!("no IPv4 route; skipped");
        return;
    };
    let body = Arc::new(flac_bytes(&sine(48_000, 2, 440.0, 0.2), 4096));
    let srv = serve_on(&format!("{ip}:0"), move |mut s, _| {
        respond(&mut s, "audio/flac", &body, Framing::Length);
    });
    let (_, e) = end_of(&srv.url("/x.flac"), &loopback_ok());
    assert!(refused(e.as_ref(), "address of this device"), "{e:?}");
    assert_eq!(srv.hits.load(Ordering::SeqCst), 0);
}

#[test]
fn unspecified_and_link_local_targets_are_refused() {
    for (url, why) in [
        ("http://0.0.0.0:9/x", "unspecified address"),
        ("http://[::]:9/x", "unspecified address"),
        (
            "http://169.254.169.254/latest/meta-data",
            "link-local address",
        ),
        ("http://[fe80::1]:9/x", "link-local address"),
        ("http://224.0.0.1:9/x", "multicast or broadcast address"),
    ] {
        let (_, e) = end_of(url, &loopback_ok());
        assert!(refused(e.as_ref(), why), "{url}: {e:?}");
    }
}

/// A server where `/r/N` redirects to `/r/N-1` (with the given Location
/// style) and `/r/0` serves a tone.
fn redirect_chain(absolute: bool) -> common::Server {
    let body = Arc::new(flac_bytes(&sine(48_000, 2, 440.0, 0.2), 4096));
    let addr = Arc::new(std::sync::OnceLock::<String>::new());
    let a2 = Arc::clone(&addr);
    let srv = serve(move |mut s, _| {
        let head = read_head(&mut s);
        let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
        let n: u32 = path.trim_start_matches("/r/").parse().unwrap_or(0);
        if n == 0 {
            let h = "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(h.as_bytes());
            let _ = s.write_all(&body);
            return;
        }
        let loc = if absolute {
            format!("http://{}/r/{}", a2.get().unwrap(), n - 1)
        } else {
            format!("{}", n - 1) // relative to /r/
        };
        let r = format!("HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n");
        let _ = s.write_all(r.as_bytes());
    });
    addr.set(srv.addr.to_string()).unwrap();
    srv
}

#[test]
fn at_most_three_redirects_are_followed() {
    for absolute in [true, false] {
        let srv = redirect_chain(absolute);
        let (n, e) = end_of(&srv.url("/r/3"), &loopback_ok());
        assert!(e.is_none(), "{e:?}");
        assert!(n > 0);
        assert_eq!(srv.hits.load(Ordering::SeqCst), 4);

        let (n, e) = end_of(&srv.url("/r/4"), &loopback_ok());
        assert_eq!(n, 0);
        assert!(refused(e.as_ref(), "too many redirects"), "{e:?}");
    }
}

fn redirect_to(location: String) -> common::Server {
    serve(move |mut s, _| {
        let _ = read_head(&mut s);
        let r = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
        );
        let _ = s.write_all(r.as_bytes());
    })
}

#[test]
fn each_redirect_target_is_checked_after_dns() {
    let (_, e) = end_of(
        &redirect_to("http://169.254.169.254/x".into()).url("/a"),
        &loopback_ok(),
    );
    assert!(refused(e.as_ref(), "link-local address"), "{e:?}");

    if let Some(ip) = own_ipv4() {
        let inner = tone_server();
        let own = serve_on(&format!("{ip}:0"), |mut s, _| {
            let _ = read_head(&mut s);
        });
        let (_, e) = end_of(
            &redirect_to(format!("http://{}/x", own.addr)).url("/b"),
            &loopback_ok(),
        );
        assert!(refused(e.as_ref(), "address of this device"), "{e:?}");
        assert_eq!(own.hits.load(Ordering::SeqCst), 0);
        drop(inner);
    }
}

#[test]
fn redirects_to_other_schemes_or_without_location_fail() {
    let (_, e) = end_of(
        &redirect_to("file:///etc/shadow".into()).url("/f"),
        &loopback_ok(),
    );
    assert!(matches!(e, Some(PlaybackError::BadUrl(_))), "{e:?}");

    let srv = serve(|mut s, _| {
        let _ = read_head(&mut s);
        let _ = s.write_all(b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n");
    });
    let (_, e) = end_of(&srv.url("/nl"), &loopback_ok());
    assert!(matches!(e, Some(PlaybackError::Http(_))), "{e:?}");
}

#[test]
fn oversized_response_headers_are_refused() {
    let srv = serve(|mut s, _| {
        let _ = read_head(&mut s);
        let big = "a".repeat(MAX_HEADER_BYTES + 100);
        let r = format!("HTTP/1.1 200 OK\r\nX-Big: {big}\r\nContent-Length: 0\r\n\r\n");
        let _ = s.write_all(r.as_bytes());
    });
    let (_, e) = end_of(&srv.url("/h"), &loopback_ok());
    assert!(
        matches!(&e, Some(PlaybackError::Http(m)) if m.contains("headers too large")),
        "{e:?}"
    );
}

#[test]
fn long_urls_and_other_schemes_are_refused_before_any_thread() {
    let long = format!("http://example.invalid/{}", "a".repeat(MAX_URL_LEN));
    assert!(matches!(
        stream::open(&long, &strict()),
        Err(PlaybackError::TargetRefused(_))
    ));
    for u in ["file:///etc/passwd", "ftp://h/x", "h/x", "http://"] {
        assert!(
            matches!(stream::open(u, &strict()), Err(PlaybackError::BadUrl(_))),
            "{u}"
        );
    }
}

#[test]
fn refusals_never_contain_the_url_or_the_address() {
    let srv = tone_server();
    let url = format!(
        "http://localhost:{}/secret-token/x.flac?auth=zz",
        srv.addr.port()
    );
    let (_, e) = end_of(&url, &strict());
    let msg = e.unwrap().to_string();
    for bad in ["secret", "auth", "localhost", "127.0.0.1"] {
        assert!(!msg.contains(bad), "{msg}");
    }
}
