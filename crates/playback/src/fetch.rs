//! HTTP(S) download of the TTS reply with rustls and bundled Mozilla roots.
//!
//! The TV has no usable system CA store for a static musl binary, so trust
//! anchors come from `webpki-roots` (compiled in). Certificates are always
//! validated. There is deliberately no switch to disable validation.
//!
//! By default ([`FetchConfig::guarded`]) the target rules of [`crate::guard`]
//! apply, as for music streams: http and https only, at most
//! [`FetchConfig::max_redirects`] redirects followed by hand with no `https`
//! to `http` downgrade, and no loopback, link-local, multicast or own-device
//! address (checked after DNS resolution); private LAN addresses are allowed
//! (Home Assistant lives there).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig, parse_pem};

use crate::error::PlaybackError;
use crate::guard::{self, GuardConnector, MAX_HEADER_BYTES, OwnAddrs};
use crate::stream::lock;

/// Default maximum response body size (10 MiB).
pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Settings for [`fetch`].
#[derive(Debug, Clone)]
pub struct FetchConfig {
    /// Overall deadline for the whole request including the body.
    pub timeout: Duration,
    /// Deadline for establishing the TCP connection.
    pub connect_timeout: Duration,
    /// Maximum accepted body size in bytes.
    pub max_bytes: u64,
    /// PEM file with trust anchors that **replaces** the bundled Mozilla roots
    /// (for a private CA in front of HA). `None` uses the bundled roots.
    pub ca_file: Option<PathBuf>,
    /// Apply the fetch target rules of [`crate::guard`] (default `true`).
    /// `false` keeps ureq's own redirect handling and no address checks.
    pub guarded: bool,
    /// Redirects followed at most when [`Self::guarded`] (3).
    pub max_redirects: u8,
    /// **Tests only, never from configuration:** with [`Self::guarded`],
    /// let loopback targets (127.0.0.0/8, `::1`) through so a local test
    /// server can be used; every other rule still applies.
    pub insecure_allow_loopback: bool,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(5),
            max_bytes: DEFAULT_MAX_BYTES,
            ca_file: None,
            guarded: true,
            max_redirects: 3,
            insecure_allow_loopback: false,
        }
    }
}

/// A downloaded body with the metadata useful for choosing a decoder.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// Response body.
    pub bytes: Vec<u8>,
    /// `Content-Type` header without parameters, lower-cased.
    pub content_type: Option<String>,
}

/// Downloads `url` and returns the body (at most `cfg.max_bytes`).
///
/// # Errors
/// [`PlaybackError::BadUrl`], [`PlaybackError::Tls`], [`PlaybackError::Status`],
/// [`PlaybackError::Timeout`], [`PlaybackError::TooLarge`],
/// [`PlaybackError::TargetRefused`] or [`PlaybackError::Http`].
pub async fn fetch(url: &str, cfg: &FetchConfig) -> Result<Vec<u8>, PlaybackError> {
    Ok(fetch_with_meta(url, cfg).await?.bytes)
}

/// Like [`fetch`] but also returns the content type.
///
/// The blocking `ureq` client runs on tokio's blocking pool; the deadline in
/// `cfg.timeout` bounds it, so the task cannot linger.
///
/// # Errors
/// Same as [`fetch`].
pub async fn fetch_with_meta(url: &str, cfg: &FetchConfig) -> Result<Fetched, PlaybackError> {
    check_scheme(url)?;
    if cfg.guarded {
        guard::check_url(url)?;
    }
    let url = url.to_owned();
    let cfg = cfg.clone();
    tokio::task::spawn_blocking(move || fetch_blocking(&url, &cfg))
        .await
        .map_err(|e| PlaybackError::Http(format!("fetch task failed: {e}")))?
}

pub(crate) fn check_scheme(url: &str) -> Result<(), PlaybackError> {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(())
    } else {
        Err(PlaybackError::BadUrl("scheme must be http or https".into()))
    }
}

fn load_ca_file(path: &PathBuf) -> Result<Vec<Certificate<'static>>, PlaybackError> {
    let pem = std::fs::read(path)?;
    let mut certs = Vec::new();
    for item in parse_pem(&pem) {
        if let PemItem::Certificate(c) =
            item.map_err(|e| PlaybackError::Tls(format!("bad CA file: {e}")))?
        {
            certs.push(c);
        }
    }
    if certs.is_empty() {
        return Err(PlaybackError::Tls("CA file contains no certificate".into()));
    }
    Ok(certs)
}

/// TLS settings shared by [`fetch`] and the music stream: rustls with the
/// bundled Mozilla roots, or only the certificates in `ca_file`. Validation
/// is always on.
pub(crate) fn tls_config(ca_file: Option<&PathBuf>) -> Result<TlsConfig, PlaybackError> {
    let mut tls = TlsConfig::builder();
    if let Some(path) = ca_file {
        tls = tls.root_certs(RootCerts::Specific(Arc::new(load_ca_file(path)?)));
    }
    Ok(tls.build())
}

fn fetch_blocking(url: &str, cfg: &FetchConfig) -> Result<Fetched, PlaybackError> {
    let resp = if cfg.guarded {
        get_guarded(url, cfg)?
    } else {
        let tls = tls_config(cfg.ca_file.as_ref())?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(cfg.timeout))
            .timeout_connect(Some(cfg.connect_timeout))
            .tls_config(tls)
            .user_agent("webos-voice-satellite")
            .build()
            .into();
        agent.get(url).call().map_err(map_err)?
    };
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase());
    let bytes = resp
        .into_body()
        .into_with_config()
        .limit(cfg.max_bytes)
        .read_to_vec()
        .map_err(|e| match e {
            ureq::Error::BodyExceedsLimit(_) => PlaybackError::TooLarge {
                limit: cfg.max_bytes,
            },
            other => map_err(other),
        })?;
    Ok(Fetched {
        bytes,
        content_type,
    })
}

/// The guarded request: the address check in front of the TCP connector,
/// redirects by hand, one overall deadline across all hops.
fn get_guarded(
    url: &str,
    cfg: &FetchConfig,
) -> Result<ureq::http::Response<ureq::Body>, PlaybackError> {
    use ureq::unversioned::resolver::DefaultResolver;
    use ureq::unversioned::transport::{Connector, RustlsConnector, TcpConnector};
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(cfg.timeout))
        .timeout_connect(Some(cfg.connect_timeout))
        .tls_config(tls_config(cfg.ca_file.as_ref())?)
        .proxy(None)
        .max_redirects(0)
        .max_response_header_size(MAX_HEADER_BYTES)
        .user_agent("webos-voice-satellite")
        .build();
    let refused = Arc::new(Mutex::new(None));
    let connector = ()
        .chain(GuardConnector {
            allow_loopback: cfg.insecure_allow_loopback,
            on_unknown: OwnAddrs::Skip,
            refused: Arc::clone(&refused),
        })
        .chain(TcpConnector::default())
        .chain(RustlsConnector::default());
    let agent = ureq::Agent::with_parts(config, connector, DefaultResolver::default());
    let deadline = Instant::now() + cfg.timeout;
    let map = |e: ureq::Error| match lock(&refused).take() {
        Some(reason) => PlaybackError::TargetRefused(reason.into()),
        None => map_err(e),
    };
    // Each hop gets what is left of the overall deadline.
    let remaining = || {
        Some(
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        )
    };
    guard::get_following(&agent, url, cfg.max_redirects, remaining, map)
}

pub(crate) fn map_err(e: ureq::Error) -> PlaybackError {
    match e {
        ureq::Error::StatusCode(s) => PlaybackError::Status(s),
        ureq::Error::Timeout(_) => PlaybackError::Timeout,
        // The message of `BadUri` can echo the URL, which may carry a token.
        ureq::Error::BadUri(_) => PlaybackError::BadUrl("malformed URL".into()),
        ureq::Error::BodyExceedsLimit(limit) => PlaybackError::TooLarge { limit },
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => {
            PlaybackError::Tls(e.to_string())
        }
        other => {
            let msg = other.to_string();
            // rustls errors surfaced through std::io keep "certificate" in the text.
            if msg.to_ascii_lowercase().contains("certificate") {
                PlaybackError::Tls(msg)
            } else {
                PlaybackError::Http(msg)
            }
        }
    }
}
