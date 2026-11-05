//! HTTP(S) download of the TTS reply with rustls and bundled Mozilla roots.
//!
//! The TV has no usable system CA store for a static musl binary, so trust
//! anchors come from `webpki-roots` (compiled in). Certificates are always
//! validated. There is deliberately no switch to disable validation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig, parse_pem};

use crate::error::PlaybackError;

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
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(5),
            max_bytes: DEFAULT_MAX_BYTES,
            ca_file: None,
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
/// [`PlaybackError::Timeout`], [`PlaybackError::TooLarge`] or
/// [`PlaybackError::Http`].
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
    let tls = tls_config(cfg.ca_file.as_ref())?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(cfg.timeout))
        .timeout_connect(Some(cfg.connect_timeout))
        .tls_config(tls)
        .user_agent("webos-voice-satellite")
        .build()
        .into();

    let resp = agent.get(url).call().map_err(map_err)?;
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
