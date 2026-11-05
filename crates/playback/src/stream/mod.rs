//! Streaming music playback source: HTTP(S) fetch plus incremental decode.
//!
//! [`open`] starts two named threads and returns a [`MusicStream`] at once:
//!
//! ```text
//!  socket --(ureq, idle timeout per read)--> music-fetch
//!     --> ByteRing (compressed, StreamConfig::ring_bytes, 512 KiB)
//!     --> music-decode (symphonia, StreamDecoder)
//!     --> PcmQueue (decoded chunks, StreamConfig::queue_ms, at most 384 KiB)
//!     --> MusicStream::recv (the consumer: the music engine)
//! ```
//!
//! # Consumer contract (frozen for the M4 music engine)
//!
//! - Items are [`Pcm`] chunks: interleaved `s16`, `channels` is 1 or 2,
//!   `rate` in 8000..=192000 Hz, both taken from the stream's own header.
//!   Every chunk of one stream has the same `rate` and `channels` (a change
//!   ends the stream with [`PlaybackError::Unsupported`]), so the consumer
//!   opens its output from the first chunk. Each chunk holds
//!   `StreamConfig::chunk_ms` of audio, except possibly the last.
//! - The music engine consumes it as a [`MusicSource`] (implemented here):
//!   `engine.play(Box::new(stream::open(url, &cfg)?))`.
//! - [`MusicStream::recv`] (async) and [`MusicStream::blocking_recv`] return
//!   `Some(Ok(chunk))` for audio, then either `None` (normal end: the body
//!   was complete, by length or by connection close) or one
//!   `Some(Err(e))` followed by `None`. Terminal errors include
//!   [`PlaybackError::IdleTimeout`] (no byte for `idle_timeout`),
//!   [`PlaybackError::EndedEarly`] (shorter than its `Content-Length`),
//!   [`PlaybackError::Status`], [`PlaybackError::Tls`],
//!   [`PlaybackError::Http`] and [`PlaybackError::Decode`]. Audio buffered
//!   before an error is delivered first.
//! - Not reading pauses the stream: the queue and the ring fill, the fetch
//!   thread stops reading the socket and TCP pushes back on the server. The
//!   idle timeout only counts time spent waiting **in a socket read**, so a
//!   paused consumer does not trip it; the pause limit (D8) is the
//!   consumer's job (it calls [`MusicStream::cancel`]).
//! - [`MusicStream::cancel`] (or a [`Canceller`] from another task, or
//!   dropping the stream) shuts the socket down, so a blocked read returns
//!   at once; after it `recv` returns `None`. [`MusicStream::close`] cancels
//!   and joins both threads, normally within milliseconds; the threads are
//!   detached if they do not finish within the given time (only possible
//!   while DNS resolution or a TCP connect is in progress, which are bounded
//!   by `connect_timeout`).
//! - A URL is fetched once: there is no retry and no reconnect (a new
//!   request for an HA proxy URL would restart the track).
//! - Error texts never contain the URL; use [`crate::guard::redact_url`]
//!   to log one.
//! - Fetch targets follow [`crate::guard`]: at most `max_redirects`, no
//!   `https` to `http`, no loopback, link-local or own-device address
//!   (checked after DNS on the address connected to), header and URL caps.
//!   A refusal is [`PlaybackError::TargetRefused`] and nothing is fetched.

mod buffers;
mod decoder;

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use decoder::{
    FIRST_FRAME_BUDGET, GAP_BUDGET, MAX_CONSECUTIVE_ERRORS, MAX_PACKET_FRAMES, MAX_STREAM_CHANNELS,
    MAX_STREAM_RATE, MIN_STREAM_RATE, StreamDecoder,
};

pub(crate) use self::buffers::lock;
use self::buffers::{ByteRing, PcmQueue, Pop};
use crate::error::PlaybackError;
use crate::fetch::{map_err, tls_config};
use crate::guard::{self, GuardConnector, MAX_HEADER_BYTES};
use crate::music::{MusicSource, SourceItem};
use crate::pcm::Pcm;

/// Name of the thread that reads the socket.
pub const FETCH_THREAD_NAME: &str = "music-fetch";
/// Name of the thread that decodes.
pub const DECODE_THREAD_NAME: &str = "music-decode";
/// Hard cap on the decoded queue, whatever `queue_ms` and the rate say.
pub const MAX_QUEUE_BYTES: usize = 384 * 1024;
/// Size of one socket read.
const READ_BUF: usize = 16 * 1024;

/// Settings for [`open`].
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Deadline for DNS resolution and for the TCP connect, each.
    pub connect_timeout: Duration,
    /// Deadline from the start of the request to the end of the response
    /// headers (connect, TLS, request, headers).
    pub response_timeout: Duration,
    /// A socket read that waits this long without a byte ends the stream
    /// with [`PlaybackError::IdleTimeout`] (default D11: 10 s).
    pub idle_timeout: Duration,
    /// PEM trust anchors replacing the bundled Mozilla roots (as for TTS).
    pub ca_file: Option<PathBuf>,
    /// Capacity of the compressed byte ring.
    pub ring_bytes: usize,
    /// Duration of decoded audio the queue holds before the decoder waits
    /// (capped at [`MAX_QUEUE_BYTES`]).
    pub queue_ms: u32,
    /// Duration of each decoded chunk.
    pub chunk_ms: u32,
    /// Redirects followed at most (D14: 3); each target is checked again.
    pub max_redirects: u8,
    /// **Tests only, never from configuration:** allow loopback targets
    /// (127.0.0.0/8, `::1`) so a local test server can be used; every other
    /// rule of [`crate::guard`] still applies.
    pub insecure_allow_loopback: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            response_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(10),
            ca_file: None,
            ring_bytes: 512 * 1024,
            queue_ms: 1000,
            chunk_ms: 20,
            max_redirects: 3,
            insecure_allow_loopback: false,
        }
    }
}

/// Counters of a running stream, see [`MusicStream::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Body bytes read from the socket.
    pub bytes_received: u64,
    /// Compressed bytes waiting for the decoder.
    pub ring_bytes: usize,
    /// Decoded samples (all channels) waiting for the consumer.
    pub queued_samples: usize,
    /// Frames decoded so far.
    pub frames_decoded: u64,
    /// Corrupt packets skipped so far.
    pub packets_skipped: u64,
    /// Output rate and channel count, once the first packet was decoded.
    pub format: Option<(u32, u16)>,
}

struct Shared {
    stopped: AtomicBool,
    /// The agent of the running request; `Agent::shutdown` interrupts reads
    /// blocked on its sockets.
    agent: Mutex<Option<ureq::Agent>>,
    ring: ByteRing,
    queue: PcmQueue,
    content_type: Mutex<Option<String>>,
    bytes_received: AtomicU64,
    frames_decoded: AtomicU64,
    packets_skipped: AtomicU64,
    format: Mutex<Option<(u32, u16)>>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Stops the fetch side: wakes the ring and shuts the socket down.
    fn stop_fetch(&self) {
        self.ring.stop();
        if let Some(agent) = lock(&self.agent).take() {
            agent.shutdown();
        }
    }

    /// Stops everything (cancel or consumer gone).
    fn cancel(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.queue.stop();
        self.stop_fetch();
    }
}

/// Cancels a [`MusicStream`] from anywhere (cheap to clone).
#[derive(Clone)]
pub struct Canceller(Arc<Shared>);

impl Canceller {
    /// Stops the stream: the socket is shut down, both threads exit, and the
    /// stream's `recv` returns `None`.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

impl std::fmt::Debug for Canceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Canceller")
    }
}

/// A running music stream; see the [module documentation](self) for the
/// contract. Dropping it cancels the stream without waiting.
pub struct MusicStream {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for MusicStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MusicStream")
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl MusicStream {
    /// Next chunk; see the contract in the [module documentation](self).
    /// Cancel safe.
    pub async fn recv(&mut self) -> Option<Result<Pcm, PlaybackError>> {
        std::future::poll_fn(|cx| self.poll_recv(cx)).await
    }

    /// Poll form of [`recv`](Self::recv): `Pending` registers the waker,
    /// which the decode thread wakes on the next chunk, the end or a cancel.
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Pcm, PlaybackError>>> {
        self.shared.queue.poll_pop(cx)
    }

    /// Blocking variant of [`recv`](Self::recv), for threads and tests.
    /// Must not be called from an async task.
    pub fn blocking_recv(&mut self) -> Option<Result<Pcm, PlaybackError>> {
        self.shared.queue.pop_blocking()
    }

    /// Non-blocking variant: `Ok(None)` when nothing is queued yet,
    /// `Err(())` when the stream is over.
    ///
    /// # Errors
    /// `Err(())` once the stream has ended (normal end, after the terminal
    /// error, or cancelled).
    #[allow(clippy::result_unit_err)]
    pub fn try_recv(&mut self) -> Result<Option<Result<Pcm, PlaybackError>>, ()> {
        match self.shared.queue.try_pop() {
            Pop::Item(x) => Ok(Some(x)),
            Pop::Empty => Ok(None),
            Pop::Closed => Err(()),
        }
    }

    /// A handle that cancels this stream from another task or thread.
    #[must_use]
    pub fn canceller(&self) -> Canceller {
        Canceller(Arc::clone(&self.shared))
    }

    /// Cancels the stream (see [`Canceller::cancel`]); does not wait.
    pub fn cancel(&self) {
        self.shared.cancel();
    }

    /// Cancels the stream and waits up to `timeout` for both threads.
    /// Returns true when both threads have exited.
    ///
    /// Blocks: from async code call it on a blocking thread, or just drop the
    /// stream (cancelled, threads exit on their own).
    #[must_use]
    pub fn close(mut self, timeout: Duration) -> bool {
        self.shared.cancel();
        let deadline = Instant::now() + timeout;
        loop {
            if self.threads.iter().all(JoinHandle::is_finished) {
                for t in self.threads.drain(..) {
                    let _ = t.join();
                }
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// True when both threads have exited.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.threads.iter().all(JoinHandle::is_finished)
    }

    /// `Content-Type` of the response (without parameters), once known.
    #[must_use]
    pub fn content_type(&self) -> Option<String> {
        lock(&self.shared.content_type).clone()
    }

    /// Current counters.
    #[must_use]
    pub fn stats(&self) -> StreamStats {
        let s = &self.shared;
        StreamStats {
            bytes_received: s.bytes_received.load(Ordering::Relaxed),
            ring_bytes: s.ring.len(),
            queued_samples: s.queue.queued_samples(),
            frames_decoded: s.frames_decoded.load(Ordering::Relaxed),
            packets_skipped: s.packets_skipped.load(Ordering::Relaxed),
            format: *lock(&s.format),
        }
    }
}

/// The adapter for the music engine (M4): a [`MusicStream`] is a
/// [`MusicSource`]. `Ok(chunk)` becomes [`SourceItem::Pcm`], the terminal
/// error [`SourceItem::Error`] (its text never holds the URL) and the normal
/// end [`SourceItem::End`]. Dropping the source (the engine replaces or stops
/// the track) cancels the stream and shuts its socket down.
impl MusicSource for MusicStream {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<SourceItem> {
        self.poll_recv(cx).map(|item| match item {
            Some(Ok(pcm)) => SourceItem::Pcm(pcm),
            Some(Err(e)) => SourceItem::Error(e.to_string()),
            None => SourceItem::End,
        })
    }
}

impl Drop for MusicStream {
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

/// Starts fetching and decoding `url` on two named threads and returns at
/// once; connection and HTTP errors arrive through [`MusicStream::recv`].
///
/// # Errors
/// [`PlaybackError::BadUrl`] for a scheme other than `http` or `https`,
/// [`PlaybackError::TargetRefused`] for a URL over the length cap, and
/// [`PlaybackError::Io`] when a thread cannot be spawned.
pub fn open(url: &str, cfg: &StreamConfig) -> Result<MusicStream, PlaybackError> {
    guard::check_url(url)?;
    let shared = Arc::new(Shared {
        stopped: AtomicBool::new(false),
        agent: Mutex::new(None),
        ring: ByteRing::new(cfg.ring_bytes),
        queue: PcmQueue::new(),
        content_type: Mutex::new(None),
        bytes_received: AtomicU64::new(0),
        frames_decoded: AtomicU64::new(0),
        packets_skipped: AtomicU64::new(0),
        format: Mutex::new(None),
    });
    let ext_hint = url_extension(url);
    let mut stream = MusicStream {
        shared: Arc::clone(&shared),
        threads: Vec::with_capacity(2),
    };

    let (s, u, c) = (Arc::clone(&shared), url.to_owned(), cfg.clone());
    stream.threads.push(
        std::thread::Builder::new()
            .name(FETCH_THREAD_NAME.into())
            .spawn(move || fetch_main(&s, &u, &c))?,
    );
    let (s, c) = (Arc::clone(&shared), cfg.clone());
    stream.threads.push(
        std::thread::Builder::new()
            .name(DECODE_THREAD_NAME.into())
            .spawn(move || decode_main(&s, &c, ext_hint))?,
    );
    Ok(stream)
}

/// Extension of the last path segment (`flac` for `.../abc.flac?x`).
fn url_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let last = path.rsplit('/').next()?;
    let (_, ext) = last.rsplit_once('.')?;
    (!ext.is_empty() && ext.len() <= 5 && ext.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| ext.to_ascii_lowercase())
}

fn build_agent(
    cfg: &StreamConfig,
    refused: &Arc<Mutex<Option<&'static str>>>,
) -> Result<ureq::Agent, PlaybackError> {
    use ureq::unversioned::resolver::DefaultResolver;
    use ureq::unversioned::transport::{Connector, RustlsConnector, TcpConnector};
    let config = ureq::Agent::config_builder()
        .timeout_resolve(Some(cfg.connect_timeout))
        .timeout_connect(Some(cfg.connect_timeout))
        .timeout_recv_response(Some(cfg.response_timeout))
        // The idle timeout: a socket read timeout, restarted for each read.
        // There is deliberately no body or global budget (endless streams).
        .timeout_per_read(Some(cfg.idle_timeout))
        .timeout_per_write(Some(cfg.response_timeout))
        .tls_config(tls_config(cfg.ca_file.as_ref())?)
        // Never use a proxy from the environment: the target checks apply
        // to the address actually connected to.
        .proxy(None)
        // Redirects are followed by hand so every hop is checked.
        .max_redirects(0)
        .max_response_header_size(MAX_HEADER_BYTES)
        .user_agent("webos-voice-satellite")
        .build();
    // The guard sees the resolved addresses before the TCP connector uses
    // them (and before TLS); `Agent::shutdown` reaches the sockets the TCP
    // connector registers.
    let connector = ()
        .chain(GuardConnector {
            allow_loopback: cfg.insecure_allow_loopback,
            refused: Arc::clone(refused),
        })
        .chain(TcpConnector::default())
        .chain(RustlsConnector::default());
    Ok(ureq::Agent::with_parts(
        config,
        connector,
        DefaultResolver::default(),
    ))
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn fetch_main(shared: &Shared, url: &str, cfg: &StreamConfig) {
    let res = fetch_body(shared, url, cfg);
    let err = match res {
        Ok(()) if !shared.stopped() => None,
        Ok(()) => Some(PlaybackError::Cancelled),
        Err(_) if shared.stopped() => Some(PlaybackError::Cancelled),
        Err(e) => Some(e),
    };
    shared.ring.finish(err);
    // The connection is closed when the agent and the reader drop here.
    lock(&shared.agent).take();
}

fn fetch_body(shared: &Shared, url: &str, cfg: &StreamConfig) -> Result<(), PlaybackError> {
    let refused = Arc::new(Mutex::new(None));
    let agent = build_agent(cfg, &refused)?;
    {
        let mut slot = lock(&shared.agent);
        if shared.stopped() {
            return Err(PlaybackError::Cancelled);
        }
        *slot = Some(agent.clone());
    }
    let map = |e: ureq::Error| match lock(&refused).take() {
        Some(reason) => PlaybackError::TargetRefused(reason.into()),
        None => map_stream_err(e, cfg),
    };
    let mut url = url.to_owned();
    let mut hops = 0u8;
    let resp = loop {
        let resp = agent.get(&url).call().map_err(map)?;
        let status = resp.status().as_u16();
        if is_redirect(status) {
            hops += 1;
            if hops > cfg.max_redirects {
                return Err(PlaybackError::TargetRefused("too many redirects".into()));
            }
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| PlaybackError::Http("redirect without a location".into()))?;
            url = guard::next_url(&url, location)?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(PlaybackError::Status(status));
        }
        break resp;
    };
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase());
    *lock(&shared.content_type) = content_type;
    let mut reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(map_body_err(e, cfg)),
        };
        if n == 0 {
            return Ok(());
        }
        shared.bytes_received.fetch_add(n as u64, Ordering::Relaxed);
        if !shared.ring.write(&buf[..n]) {
            return Err(PlaybackError::Cancelled);
        }
    }
}

fn map_stream_err(e: ureq::Error, cfg: &StreamConfig) -> PlaybackError {
    match e {
        ureq::Error::Timeout(ureq::Timeout::PerRead) => {
            PlaybackError::IdleTimeout(cfg.idle_timeout)
        }
        ureq::Error::Io(io) => map_io(&io, cfg).unwrap_or_else(|| map_err(ureq::Error::Io(io))),
        ureq::Error::LargeResponseHeader(..) => {
            PlaybackError::Http("response headers too large".into())
        }
        other => map_err(other),
    }
}

fn map_io(e: &io::Error, cfg: &StreamConfig) -> Option<PlaybackError> {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            Some(PlaybackError::IdleTimeout(cfg.idle_timeout))
        }
        io::ErrorKind::UnexpectedEof => Some(PlaybackError::EndedEarly),
        _ => None,
    }
}

fn map_body_err(e: io::Error, cfg: &StreamConfig) -> PlaybackError {
    if let Some(p) = map_io(&e, cfg) {
        return p;
    }
    map_stream_err(ureq::Error::from(e), cfg)
}

/// Adapts the ring to `Read` for the decoder.
struct RingReader(Arc<Shared>);

impl Read for RingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.ring.read(buf)
    }
}

fn decode_main(shared: &Arc<Shared>, cfg: &StreamConfig, ext_hint: Option<String>) {
    let res = decode_loop(shared, cfg, ext_hint);
    // Whatever happened, the fetch side has nothing more to do.
    shared.stop_fetch();
    match res {
        Ok(()) => shared.queue.finish(None),
        Err(e) => shared.queue.finish(Some(e)),
    }
}

/// Prefers the fetch error (the real cause) over the decoder's view of it.
fn upstream_or(shared: &Shared, e: PlaybackError) -> PlaybackError {
    if shared.stopped() {
        return PlaybackError::Cancelled;
    }
    if shared.ring.failed() {
        return shared.ring.take_error().unwrap_or(e);
    }
    e
}

fn decode_loop(
    shared: &Arc<Shared>,
    cfg: &StreamConfig,
    ext_hint: Option<String>,
) -> Result<(), PlaybackError> {
    shared.ring.wait_ready();
    if shared.stopped() {
        return Err(PlaybackError::Cancelled);
    }
    if shared.ring.failed() && shared.ring.len() == 0 {
        return Err(upstream_or(
            shared,
            PlaybackError::Http("fetch failed".into()),
        ));
    }
    let hint = lock(&shared.content_type).clone().or(ext_hint);
    let mut dec = StreamDecoder::new(
        Box::new(RingReader(Arc::clone(shared))),
        hint.as_deref(),
        cfg.chunk_ms,
    )
    .map_err(|e| upstream_or(shared, e))?;
    let mut sized = false;
    loop {
        let next = dec.next_chunk();
        shared
            .frames_decoded
            .store(dec.frames_decoded(), Ordering::Relaxed);
        shared
            .packets_skipped
            .store(dec.packets_skipped(), Ordering::Relaxed);
        let chunk = match next {
            Ok(Some(c)) => c,
            Ok(None) => {
                // A clean end of the demuxer may still hide a fetch error
                // (for example a truncated chunked body).
                return match shared.ring.take_error() {
                    Some(e) => Err(upstream_or(shared, e)),
                    None if shared.stopped() => Err(PlaybackError::Cancelled),
                    None => Ok(()),
                };
            }
            Err(e) => return Err(upstream_or(shared, e)),
        };
        if !sized {
            let per_sec = chunk.rate as usize * usize::from(chunk.channels);
            let cap = (per_sec * cfg.queue_ms as usize / 1000).min(MAX_QUEUE_BYTES / 2);
            shared.queue.set_capacity(cap);
            *lock(&shared.format) = Some((chunk.rate, chunk.channels));
            sized = true;
        }
        if !shared.queue.push(chunk) {
            return Err(PlaybackError::Cancelled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::url_extension;

    #[test]
    fn extension_from_url() {
        assert_eq!(
            url_extension("http://h/api/esphome/ffmpeg_proxy/d/abc.flac").as_deref(),
            Some("flac")
        );
        assert_eq!(
            url_extension("http://h/x/y.MP3?token=1.wav").as_deref(),
            Some("mp3")
        );
        assert_eq!(url_extension("http://h/stream"), None);
        assert_eq!(url_extension("http://h.example/"), None);
    }
}
