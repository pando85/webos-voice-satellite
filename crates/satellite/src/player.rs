//! Reply playback: streamed TTS (SPEAKER mode) and URL media (TTS URL mode,
//! announcements), to `pacat`, a WAV file or nowhere.
//!
//! One playback at a time. Each playback runs in its own task and reports a
//! [`Done`] with its id; the daemon ignores reports of playbacks it already
//! stopped, so a late report can never finish a newer reply.
//!
//! Streamed TTS is written to `pacat` as it arrives, at its native 16 kHz mono
//! (PulseAudio converts it), with the software gain applied to every sample
//! before the write. The gain is `sink.gain` times the reply volume
//! ([`crate::volume`]), read for every chunk so a volume change from HA also
//! affects a reply that is playing; URL media takes the gain at its start.
//! With `sink.gain = 0` every byte written is zero: that is how silent tests
//! on c3-a work, because `pacat --volume` is ignored there.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use webos_playback::{FetchConfig, NullSink, PacatSink, Pcm, Sink, SinkFormat, WavFileSink};

use crate::config::{SinkKind, SinkSection};
use crate::volume::SharedGain;

/// What a playback was for; decides which "finished" message HA gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Pipeline TTS reply → `PlaybackFinished`.
    Tts,
    /// Announcement → `AnnounceFinished`.
    Announce,
    /// `play_media` with `announce: true` on the music player (design D3):
    /// played like an announcement, over the ducked music; HA is told
    /// nothing (no voice announcement is running).
    MediaAnnounce,
    /// The timer alarm chime: HA is told nothing.
    Alarm,
}

/// Statistics of a finished playback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PlayStats {
    /// Duration of the audio written to the sink.
    pub(crate) audio: Duration,
    /// PCM bytes written to the sink.
    pub(crate) bytes: usize,
    /// Non-zero samples written (`Some(0)` proves a silent write). `None` for
    /// decoded URL media (announcements, URL-mode TTS): those samples go
    /// through `PacatSink`, which applies the same gain but does not count.
    pub(crate) nonzero_samples: Option<u64>,
    /// WAV file written, if any.
    pub(crate) file: Option<PathBuf>,
}

/// Report of a playback task.
#[derive(Debug)]
pub(crate) struct Done {
    pub(crate) id: u64,
    pub(crate) kind: Kind,
    pub(crate) result: Result<PlayStats, String>,
}

/// Most streamed TTS audio that may wait for the sink (about 65 s of 16 kHz
/// mono s16le). HA streams a reply only slightly ahead of real time, so the
/// queue stays near empty unless the sink stalls (pacat blocked); then audio
/// beyond this is dropped (and counted) instead of growing without bound.
pub(crate) const MAX_QUEUED_BYTES: usize = 2 << 20;

/// Sender half of a byte-bounded chunk queue.
struct ChunkTx {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    max: usize,
    dropped_chunks: u64,
}

/// Receiver half of a byte-bounded chunk queue.
pub(crate) struct ChunkRx {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    queued: Arc<AtomicUsize>,
}

fn chunk_queue(max: usize) -> (ChunkTx, ChunkRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    let queued = Arc::new(AtomicUsize::new(0));
    (
        ChunkTx {
            tx,
            queued: Arc::clone(&queued),
            max,
            dropped_chunks: 0,
        },
        ChunkRx { rx, queued },
    )
}

impl ChunkTx {
    /// Queues a chunk; returns false (and drops it) when the queue is full.
    fn push(&mut self, data: Vec<u8>) -> bool {
        let len = data.len();
        if self.queued.load(Ordering::Relaxed).saturating_add(len) > self.max {
            self.dropped_chunks += 1;
            if self.dropped_chunks == 1 {
                log::warn!(
                    "player: the sink is {} KiB behind; dropping TTS audio until it catches up",
                    self.max / 1024
                );
            }
            return false;
        }
        self.queued.fetch_add(len, Ordering::Relaxed);
        if self.tx.send(data).is_err() {
            self.queued.fetch_sub(len, Ordering::Relaxed);
        }
        true
    }
}

impl Drop for ChunkTx {
    fn drop(&mut self) {
        if self.dropped_chunks > 0 {
            log::warn!(
                "player: {} TTS chunks dropped because the sink fell behind",
                self.dropped_chunks
            );
        }
    }
}

impl ChunkRx {
    async fn recv(&mut self) -> Option<Vec<u8>> {
        let data = self.rx.recv().await?;
        self.queued.fetch_sub(data.len(), Ordering::Relaxed);
        Some(data)
    }
}

struct Active {
    id: u64,
    kind: Kind,
    /// Dropping or firing it cancels the task.
    cancel: Option<oneshot::Sender<()>>,
    /// Streamed TTS chunks; `None` after `TTS_STREAM_END`.
    chunks: Option<ChunkTx>,
    task: tokio::task::JoinHandle<()>,
}

/// The playback controller owned by the daemon loop.
pub(crate) struct Player {
    cfg: SinkSection,
    /// Reply volume gain (multiplies `cfg.gain`).
    volume: SharedGain,
    done_tx: mpsc::UnboundedSender<Done>,
    next_id: u64,
    current: Option<Active>,
    /// Test aid (`--test-fetch-loopback`): URL media may come from 127.0.0.1.
    fetch_loopback: bool,
}

/// Applies `gain` to s16le bytes (via [`Pcm::apply_gain`]) and returns the
/// result plus the number of non-zero samples in it. A trailing odd byte is
/// dropped.
pub(crate) fn gain_s16le(data: &[u8], gain: f32) -> (Vec<u8>, u64) {
    let mut pcm = Pcm {
        rate: 16_000,
        channels: 1,
        samples: data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect(),
    };
    pcm.apply_gain(gain);
    let nonzero = pcm.samples.iter().filter(|&&s| s != 0).count() as u64;
    (pcm.to_s16le_bytes(), nonzero)
}

/// `scheme://host/first-segment/…` without query or the rest of the path
/// (HA media URLs carry tokens in the path or query; never log those).
pub(crate) fn redact_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("?", url));
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let mut parts = rest.splitn(3, '/');
    let host = parts.next().unwrap_or("");
    match parts.next() {
        Some(seg) if !seg.is_empty() => format!("{scheme}://{host}/{seg}/…"),
        _ => format!("{scheme}://{host}/"),
    }
}

fn duration_of(bytes: usize, rate: u32, channels: u16) -> Duration {
    let per_sec = u64::from(rate) * u64::from(channels) * 2;
    if per_sec == 0 {
        return Duration::ZERO;
    }
    Duration::from_micros(bytes as u64 * 1_000_000 / per_sec)
}

fn wav_path(dir: &std::path::Path, prefix: &str, id: u64) -> PathBuf {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    dir.join(format!("{prefix}-{secs}-{id}.wav"))
}

impl Player {
    pub(crate) fn new(
        cfg: SinkSection,
        volume: SharedGain,
        done_tx: mpsc::UnboundedSender<Done>,
    ) -> Self {
        Self {
            cfg,
            volume,
            done_tx,
            next_id: 0,
            current: None,
            fetch_loopback: false,
        }
    }

    /// Lets URL media reach loopback addresses (PC tests only).
    pub(crate) fn set_fetch_loopback(&mut self, on: bool) {
        self.fetch_loopback = on;
    }

    /// Whether something is playing (or streaming in).
    pub(crate) fn is_active(&self) -> bool {
        self.current.is_some()
    }

    /// What is playing, if anything.
    pub(crate) fn current_kind(&self) -> Option<Kind> {
        self.current.as_ref().map(|a| a.kind)
    }

    /// Like [`Player::stop`], but leaves the timer alarm alone: HA's stop
    /// and abort messages are about replies and announcements.
    pub(crate) fn stop_reply(&mut self) -> Option<Kind> {
        if self.current_kind() == Some(Kind::Alarm) {
            return None;
        }
        self.stop()
    }

    /// Starts the timer alarm chime, which plays until stopped or `max`
    /// (plus a short margin as a backstop) has passed. The gain is read for
    /// every slice, like a streamed reply. Returns the kind of a playback
    /// that had to be stopped for it.
    pub(crate) fn start_alarm(&mut self, max: Duration) -> Option<Kind> {
        self.start_alarm_with(max, ALARM_BACKSTOP)
    }

    fn start_alarm_with(&mut self, max: Duration, backstop: Duration) -> Option<Kind> {
        let stopped = self.stop();
        let (id, cancel_rx, cancel_tx) = self.begin();
        let cfg = self.cfg.clone();
        let volume = self.volume.clone();
        let done_tx = self.done_tx.clone();
        let task = tokio::spawn(async move {
            let result = alarm_task(&cfg, &volume, id, max + backstop, cancel_rx).await;
            let _ = done_tx.send(Done {
                id,
                kind: Kind::Alarm,
                result,
            });
        });
        self.current = Some(Active {
            id,
            kind: Kind::Alarm,
            cancel: Some(cancel_tx),
            chunks: None,
            task,
        });
        stopped
    }

    fn begin(&mut self) -> (u64, oneshot::Receiver<()>, oneshot::Sender<()>) {
        self.next_id += 1;
        let (tx, rx) = oneshot::channel();
        (self.next_id, rx, tx)
    }

    /// SPEAKER mode: a TTS stream starts. Returns the kind of a playback that
    /// had to be stopped for it (the caller reports it as failed).
    pub(crate) fn start_stream(&mut self, rate: u32, channels: u16) -> Option<Kind> {
        let stopped = self.stop();
        let (id, cancel_rx, cancel_tx) = self.begin();
        let (chunk_tx, chunk_rx) = chunk_queue(MAX_QUEUED_BYTES);
        let cfg = self.cfg.clone();
        let volume = self.volume.clone();
        let done_tx = self.done_tx.clone();
        let task = tokio::spawn(async move {
            let result = match cfg.kind {
                SinkKind::Pacat => {
                    stream_pacat(&cfg, &volume, rate, channels, chunk_rx, cancel_rx).await
                }
                SinkKind::Wav => {
                    let path = wav_path(&cfg.dir, "reply", id);
                    stream_wav(path, rate, channels, chunk_rx, cancel_rx).await
                }
                SinkKind::Null => stream_null(rate, channels, chunk_rx, cancel_rx).await,
            };
            let _ = done_tx.send(Done {
                id,
                kind: Kind::Tts,
                result,
            });
        });
        self.current = Some(Active {
            id,
            kind: Kind::Tts,
            cancel: Some(cancel_tx),
            chunks: Some(chunk_tx),
            task,
        });
        stopped
    }

    /// SPEAKER mode: one PCM chunk of the current stream.
    pub(crate) fn push(&mut self, data: Vec<u8>) {
        match self.current.as_mut().and_then(|a| a.chunks.as_mut()) {
            Some(tx) => {
                tx.push(data);
            }
            None => log::debug!("player: TTS audio without an open stream; dropped"),
        }
    }

    /// SPEAKER mode: the stream ended; the task drains and reports.
    pub(crate) fn end_stream(&mut self) {
        if let Some(a) = self.current.as_mut() {
            a.chunks = None;
        }
    }

    /// Fetches and plays `url` (TTS URL mode or an announcement). Returns the
    /// kind of a playback that had to be stopped for it.
    pub(crate) fn play_url(
        &mut self,
        url: String,
        preannounce: Option<String>,
        kind: Kind,
    ) -> Option<Kind> {
        let stopped = self.stop();
        let (id, cancel_rx, cancel_tx) = self.begin();
        let cfg = self.cfg.clone();
        let gain = self.cfg.gain * self.volume.get();
        let done_tx = self.done_tx.clone();
        let fetch = FetchConfig {
            insecure_allow_loopback: self.fetch_loopback,
            ..FetchConfig::default()
        };
        let task = tokio::spawn(async move {
            let result = play_url_task(
                &cfg,
                &fetch,
                gain,
                (id, kind),
                &url,
                preannounce.as_deref(),
                cancel_rx,
            )
            .await;
            let _ = done_tx.send(Done { id, kind, result });
        });
        self.current = Some(Active {
            id,
            kind,
            cancel: Some(cancel_tx),
            chunks: None,
            task,
        });
        stopped
    }

    /// Cancels the current playback; returns its kind.
    pub(crate) fn stop(&mut self) -> Option<Kind> {
        let mut a = self.current.take()?;
        if let Some(c) = a.cancel.take() {
            let _ = c.send(());
        }
        log::info!("player: playback {} stopped", a.id);
        Some(a.kind)
    }

    /// Accepts a report if it belongs to the current playback.
    pub(crate) fn finish(&mut self, done: &Done) -> bool {
        if self.current.as_ref().is_some_and(|a| a.id == done.id) {
            self.current = None;
            true
        } else {
            false
        }
    }

    /// Stops playback and waits (bounded) for the task, so `pacat` is gone.
    pub(crate) async fn shutdown(&mut self) {
        if let Some(mut a) = self.current.take() {
            if let Some(c) = a.cancel.take() {
                let _ = c.send(());
            }
            let _ = tokio::time::timeout(Duration::from_secs(3), &mut a.task).await;
        }
    }
}

/// `--latency-msec` for `pacat`, or nothing when `sink.latency_msec` is 0.
fn latency_args(cfg: &SinkSection) -> Option<String> {
    (cfg.latency_msec > 0).then(|| format!("--latency-msec={}", cfg.latency_msec))
}

fn pacat_stream_args(cfg: &SinkSection, rate: u32, channels: u16) -> Vec<String> {
    pacat_named_args(cfg, rate, channels, "tts-reply")
}

fn pacat_named_args(cfg: &SinkSection, rate: u32, channels: u16, stream: &str) -> Vec<String> {
    let mut a = vec![
        "--playback".to_owned(),
        "--raw".to_owned(),
        "--format=s16le".to_owned(),
        format!("--rate={rate}"),
        format!("--channels={channels}"),
        "--client-name=webos-voice-satellite".to_owned(),
        format!("--stream-name={stream}"),
    ];
    a.extend(latency_args(cfg));
    if let Some(d) = &cfg.device {
        a.push(format!("--device={d}"));
    }
    a
}

async fn stream_pacat(
    cfg: &SinkSection,
    volume: &SharedGain,
    rate: u32,
    channels: u16,
    mut rx: ChunkRx,
    mut cancel: oneshot::Receiver<()>,
) -> Result<PlayStats, String> {
    let mut child = Command::new(&cfg.binary)
        .args(pacat_stream_args(cfg, rate, channels))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start pacat: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "pacat has no stdin".to_owned())?;
    let mut stats = PlayStats {
        nonzero_samples: Some(0),
        ..PlayStats::default()
    };
    loop {
        tokio::select! {
            // A stop also drops the chunk sender; the cancel must win.
            biased;
            _ = &mut cancel => {
                let _ = child.kill().await;
                return Err("cancelled".to_owned());
            }
            chunk = rx.recv() => {
                let Some(chunk) = chunk else { break };
                let (out, nonzero) = gain_s16le(&chunk, cfg.gain * volume.get());
                stats.bytes += out.len();
                *stats.nonzero_samples.get_or_insert(0) += nonzero;
                if let Err(e) = stdin.write_all(&out).await {
                    let _ = child.kill().await;
                    return Err(format!("writing to pacat failed: {e}"));
                }
            }
        }
    }
    stats.audio = duration_of(stats.bytes, rate, channels);
    let _ = stdin.shutdown().await;
    drop(stdin);
    // pacat drains PulseAudio's buffer and exits.
    let deadline = stats.audio + Duration::from_secs(15);
    tokio::select! {
        exit = child.wait() => match exit {
            Ok(s) if s.success() => Ok(stats),
            Ok(s) => Err(format!("pacat exited with {s}")),
            Err(e) => Err(format!("waiting for pacat failed: {e}")),
        },
        _ = &mut cancel => {
            let _ = child.kill().await;
            Err("cancelled".to_owned())
        }
        () = tokio::time::sleep(deadline) => {
            let _ = child.kill().await;
            Err(format!("pacat did not finish within {deadline:?}"))
        }
    }
}

async fn collect(
    mut rx: ChunkRx,
    mut cancel: oneshot::Receiver<()>,
    keep: bool,
) -> Result<(Vec<u8>, usize), String> {
    let mut buf = Vec::new();
    let mut total = 0;
    loop {
        tokio::select! {
            biased;
            _ = &mut cancel => return Err("cancelled".to_owned()),
            chunk = rx.recv() => {
                let Some(chunk) = chunk else { break };
                total += chunk.len();
                if keep {
                    buf.extend_from_slice(&chunk);
                }
            }
        }
    }
    Ok((buf, total))
}

async fn stream_wav(
    path: PathBuf,
    rate: u32,
    channels: u16,
    rx: ChunkRx,
    cancel: oneshot::Receiver<()>,
) -> Result<PlayStats, String> {
    let (buf, total) = collect(rx, cancel, true).await?;
    let pcm = Pcm {
        rate,
        channels,
        samples: buf
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect(),
    };
    let nonzero = pcm.samples.iter().filter(|&&s| s != 0).count() as u64;
    let bytes = webos_playback::wav_bytes(&pcm);
    let p = path.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&p, bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(PlayStats {
        audio: duration_of(total, rate, channels),
        bytes: total,
        nonzero_samples: Some(nonzero),
        file: Some(path),
    })
}

async fn stream_null(
    rate: u32,
    channels: u16,
    rx: ChunkRx,
    cancel: oneshot::Receiver<()>,
) -> Result<PlayStats, String> {
    let (_, total) = collect(rx, cancel, false).await?;
    Ok(PlayStats {
        audio: duration_of(total, rate, channels),
        bytes: total,
        // Nothing is written anywhere.
        nonzero_samples: Some(0),
        file: None,
    })
}

/// `gain`: `sink.gain` times the reply volume, for `pacat` only.
fn url_sink(cfg: &SinkSection, gain: f32, id: u64, kind: Kind) -> (Box<dyn Sink>, Option<PathBuf>) {
    match cfg.kind {
        SinkKind::Pacat => {
            let mut p = PacatSink::new();
            p.binary.clone_from(&cfg.binary);
            p.rate = cfg.rate;
            p.channels = cfg.channels;
            p.gain = gain;
            p.device.clone_from(&cfg.device);
            p.extra_args.extend(latency_args(cfg));
            (Box::new(p), None)
        }
        SinkKind::Wav => {
            let prefix = match kind {
                Kind::Tts => "reply-url",
                Kind::Announce => "announce",
                Kind::MediaAnnounce => "media-announce",
                Kind::Alarm => "alarm",
            };
            let path = wav_path(&cfg.dir, prefix, id);
            let mut w = WavFileSink::new(&path);
            w.format = SinkFormat {
                rate_hz: cfg.rate,
                channels: cfg.channels,
            };
            (Box::new(w), Some(path))
        }
        SinkKind::Null => (Box::new(NullSink::default()), None),
    }
}

/// Longest stretch of chime audio written ahead of real time: bounds how
/// long a stop or a volume change takes to be heard.
const ALARM_AHEAD: Duration = Duration::from_millis(400);
/// One write to `pacat`.
const ALARM_SLICE: Duration = Duration::from_millis(100);
/// How long the task outlives the cap if the daemon never stops it.
const ALARM_BACKSTOP: Duration = Duration::from_secs(3);

/// The chime, repeated until cancelled or `limit` passes.
async fn alarm_task(
    cfg: &SinkSection,
    volume: &SharedGain,
    id: u64,
    limit: Duration,
    mut cancel: oneshot::Receiver<()>,
) -> Result<PlayStats, String> {
    let cycle: Vec<u8> = crate::alarm::chime_cycle()
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    match cfg.kind {
        SinkKind::Pacat => alarm_pacat(cfg, volume, &cycle, limit, &mut cancel).await,
        SinkKind::Wav | SinkKind::Null => {
            // One cycle to a file (or nowhere), then "ring" silently.
            let (out, nonzero) = gain_s16le(&cycle, cfg.gain * volume.get());
            let file = if cfg.kind == SinkKind::Wav {
                let path = wav_path(&cfg.dir, "alarm", id);
                let pcm = Pcm {
                    rate: crate::alarm::CHIME_RATE,
                    channels: 1,
                    samples: out
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect(),
                };
                let bytes = webos_playback::wav_bytes(&pcm);
                let p = path.clone();
                tokio::task::spawn_blocking(move || std::fs::write(&p, bytes))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                Some(path)
            } else {
                None
            };
            let stats = PlayStats {
                audio: crate::alarm::CYCLE,
                bytes: out.len(),
                nonzero_samples: Some(nonzero),
                file,
            };
            tokio::select! {
                _ = &mut cancel => Err("cancelled".to_owned()),
                () = tokio::time::sleep(limit) => Ok(stats),
            }
        }
    }
}

async fn alarm_pacat(
    cfg: &SinkSection,
    volume: &SharedGain,
    cycle: &[u8],
    limit: Duration,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<PlayStats, String> {
    let mut child = Command::new(&cfg.binary)
        .args(pacat_named_args(
            cfg,
            crate::alarm::CHIME_RATE,
            1,
            "timer-alarm",
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start pacat: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "pacat has no stdin".to_owned())?;
    let slice_bytes = duration_bytes(ALARM_SLICE, crate::alarm::CHIME_RATE);
    let started = Instant::now();
    let mut stats = PlayStats {
        nonzero_samples: Some(0),
        ..PlayStats::default()
    };
    'ring: loop {
        for slice in cycle.chunks(slice_bytes) {
            // Pace the writes: pacat's pipe would otherwise take seconds of
            // audio at once, and a stop would be heard that much later.
            let ahead = stats.audio.saturating_sub(started.elapsed());
            if ahead > ALARM_AHEAD {
                tokio::select! {
                    biased;
                    _ = &mut *cancel => {
                        let _ = child.kill().await;
                        return Err("cancelled".to_owned());
                    }
                    () = tokio::time::sleep(ahead.saturating_sub(ALARM_AHEAD)) => {}
                }
            }
            if started.elapsed() >= limit {
                break 'ring;
            }
            let (out, nonzero) = gain_s16le(slice, cfg.gain * volume.get());
            tokio::select! {
                biased;
                _ = &mut *cancel => {
                    let _ = child.kill().await;
                    return Err("cancelled".to_owned());
                }
                r = stdin.write_all(&out) => {
                    if let Err(e) = r {
                        let _ = child.kill().await;
                        return Err(format!("writing to pacat failed: {e}"));
                    }
                }
            }
            stats.bytes += out.len();
            *stats.nonzero_samples.get_or_insert(0) += nonzero;
            stats.audio = duration_of(stats.bytes, crate::alarm::CHIME_RATE, 1);
        }
    }
    // Past the limit: let the little that is buffered play, then end.
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    let _ = child.kill().await;
    Ok(stats)
}

/// Bytes of 16-bit mono audio in `d` (an even number).
fn duration_bytes(d: Duration, rate: u32) -> usize {
    (rate as usize * 2 * d.as_millis() as usize / 1000) & !1
}

async fn play_url_task(
    cfg: &SinkSection,
    fetch: &FetchConfig,
    gain: f32,
    (id, kind): (u64, Kind),
    url: &str,
    preannounce: Option<&str>,
    mut cancel: oneshot::Receiver<()>,
) -> Result<PlayStats, String> {
    let (mut sink, file) = url_sink(cfg, gain, id, kind);
    let stop = sink.stop_handle();
    let started = Instant::now();
    let play = async {
        if let Some(pre) = preannounce {
            if let Err(e) = webos_playback::play_url(pre, fetch, sink.as_mut()).await {
                log::warn!("player: pre-announcement {} failed: {e}", redact_url(pre));
            }
        }
        webos_playback::play_url(url, fetch, sink.as_mut()).await
    };
    tokio::pin!(play);
    let res = tokio::select! {
        r = &mut play => r,
        _ = &mut cancel => {
            stop.stop();
            let _ = play.await;
            return Err("cancelled".to_owned());
        }
    };
    match res {
        Ok(t) => {
            log::debug!(
                "player: {} fetched {} bytes in {} ms, decode {} ms, resample {} ms, play {} ms (total {} ms)",
                redact_url(url),
                t.bytes,
                t.fetch.as_millis(),
                t.decode.as_millis(),
                t.resample.as_millis(),
                t.play.as_millis(),
                started.elapsed().as_millis()
            );
            Ok(PlayStats {
                audio: t.audio,
                bytes: t.bytes,
                nonzero_samples: None,
                file,
            })
        }
        Err(e) => Err(format!("{} failed: {e}", redact_url(url))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn gain_zero_writes_only_zeros() {
        let data: Vec<u8> = [1000_i16, -32768, 32767, 5, 0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let (out, nonzero) = gain_s16le(&data, 0.0);
        assert_eq!(out.len(), data.len());
        assert!(out.iter().all(|&b| b == 0));
        assert_eq!(nonzero, 0);
        let (out, nonzero) = gain_s16le(&data, 1.0);
        assert_eq!(out, data);
        assert_eq!(nonzero, 4);
        let (out, _) = gain_s16le(&data, 0.5);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), 500);
        // Odd trailing byte is dropped.
        assert_eq!(gain_s16le(&[1, 2, 3], 1.0).0.len(), 2);
    }

    #[test]
    fn urls_are_redacted() {
        assert_eq!(
            redact_url("http://192.0.2.1:8123/api/tts_proxy/abcdef.flac?token=x"),
            "http://192.0.2.1:8123/api/…"
        );
        assert_eq!(
            redact_url("https://ha.example/?authSig=1"),
            "https://ha.example/"
        );
        assert!(
            !redact_url("http://h/api/esphome/ffmpeg_proxy/1/secret.flac?authSig=zz")
                .contains("secret")
        );
    }

    #[test]
    fn pacat_args_carry_rate_and_options() {
        let mut cfg = SinkSection::default();
        let a = pacat_stream_args(&cfg, 16_000, 1);
        assert!(a.contains(&"--rate=16000".to_owned()));
        assert!(a.contains(&"--channels=1".to_owned()));
        assert!(!a.iter().any(|x| x.starts_with("--volume")));
        assert!(
            a.contains(&"--latency-msec=200".to_owned()),
            "200 ms is the default"
        );
        cfg.latency_msec = 350;
        cfg.device = Some("pcm_output".to_owned());
        let a = pacat_stream_args(&cfg, 16_000, 1);
        assert!(a.contains(&"--latency-msec=350".to_owned()));
        assert!(a.contains(&"--device=pcm_output".to_owned()));
        cfg.latency_msec = 0;
        let a = pacat_stream_args(&cfg, 16_000, 1);
        assert!(
            !a.iter().any(|x| x.starts_with("--latency-msec")),
            "0 leaves PulseAudio's default"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chunk_queue_is_bounded_by_bytes_and_frees_on_receive() {
        let (mut tx, mut rx) = chunk_queue(3000);
        assert!(tx.push(vec![0; 1024]));
        assert!(tx.push(vec![0; 1024]));
        assert!(!tx.push(vec![0; 1024]), "third chunk exceeds 3000 bytes");
        assert_eq!(tx.dropped_chunks, 1);
        assert_eq!(rx.queued.load(Ordering::Relaxed), 2048);
        assert_eq!(rx.recv().await.unwrap().len(), 1024);
        assert_eq!(rx.queued.load(Ordering::Relaxed), 1024);
        assert!(tx.push(vec![0; 1024]), "room again after the sink took one");
        drop(tx);
        assert_eq!(rx.recv().await.unwrap().len(), 1024);
        assert_eq!(rx.recv().await.unwrap().len(), 1024);
        assert!(rx.recv().await.is_none());
        assert_eq!(rx.queued.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stalled_sink_drops_instead_of_growing() {
        // A pacat that never reads its input: the player's queue must stop at
        // the byte limit while HA keeps sending.
        let dir = std::env::temp_dir().join(format!("webos-sat-stall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-pacat");
        crate::testutil::write_script(&script, "sleep 30");
        let mut cfg = SinkSection::default();
        cfg.binary.clone_from(&script);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, SharedGain::new(1.0), tx);
        p.start_stream(16_000, 1);
        // 16 MiB offered; the pipe (64 KiB) plus the queue limit can hold far less.
        for _ in 0..16 {
            for _ in 0..1024 {
                p.push(vec![1; 1024]);
                tokio::task::yield_now().await;
            }
        }
        let tx = p.current.as_ref().and_then(|a| a.chunks.as_ref()).unwrap();
        let queued = tx.queued.load(Ordering::Relaxed);
        assert!(queued <= MAX_QUEUED_BYTES, "queued {queued}");
        assert!(tx.dropped_chunks > 0);
        p.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn chunk(v: i16, n: usize) -> Vec<u8> {
        std::iter::repeat_n(v, n)
            .flat_map(i16::to_le_bytes)
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wav_stream_writes_a_file_and_reports() {
        let dir = std::env::temp_dir().join(format!("webos-sat-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = SinkSection::default();
        cfg.kind = SinkKind::Wav;
        cfg.dir.clone_from(&dir);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, SharedGain::new(1.0), tx);
        assert!(p.start_stream(16_000, 1).is_none());
        p.push(chunk(100, 512));
        p.push(chunk(0, 512));
        p.end_stream();
        let done = rx.recv().await.unwrap();
        assert!(p.finish(&done));
        assert!(!p.is_active());
        let stats = done.result.unwrap();
        assert_eq!(stats.bytes, 2048);
        assert_eq!(stats.audio, Duration::from_millis(64));
        assert_eq!(stats.nonzero_samples, Some(512));
        let file = stats.file.unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().len(), 44 + 2048);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_cancels_and_late_report_is_ignored() {
        let mut cfg = SinkSection::default();
        cfg.kind = SinkKind::Null;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, SharedGain::new(1.0), tx);
        p.start_stream(16_000, 1);
        p.push(chunk(1, 512));
        assert_eq!(p.stop(), Some(Kind::Tts));
        let done = rx.recv().await.unwrap();
        assert!(done.result.is_err());
        assert!(
            !p.finish(&done),
            "stopped playback must not be finished again"
        );

        // A new stream replaces an old one; the old report is ignored.
        p.start_stream(16_000, 1);
        assert_eq!(p.start_stream(16_000, 1), Some(Kind::Tts));
        p.end_stream();
        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        let (old, new) = if first.id < second.id {
            (first, second)
        } else {
            (second, first)
        };
        assert!(!p.finish(&old));
        assert!(p.finish(&new));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pacat_stream_applies_sink_gain_times_reply_volume() {
        let dir = std::env::temp_dir().join(format!("webos-sat-pacat-vol-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("written.raw");
        let script = dir.join("fake-pacat");
        crate::testutil::write_script(&script, &format!("cat > '{}'", out.display()));
        let mut cfg = SinkSection::default();
        cfg.binary.clone_from(&script);
        cfg.gain = 0.5;
        let volume = SharedGain::new(crate::volume::gain_for(40));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, volume.clone(), tx);
        p.start_stream(16_000, 1);
        p.push(chunk(10_000, 4));
        // Wait until the first chunk reached the fake pacat.
        for _ in 0..300 {
            if std::fs::metadata(&out).is_ok_and(|m| m.len() >= 8) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // A change from HA applies to the chunks that follow.
        volume.set(1.0);
        p.push(chunk(10_000, 4));
        p.end_stream();
        let done = rx.recv().await.unwrap();
        assert!(done.result.is_ok());
        let written: Vec<i16> = std::fs::read(&out)
            .unwrap()
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        // 10000 * 0.5 * -30 dB (0.0316) = 158; then 10000 * 0.5 = 5000.
        assert_eq!(written.len(), 8);
        assert!(
            written[..4].iter().all(|&s| (157..=159).contains(&s)),
            "{written:?}"
        );
        assert!(written[4..].iter().all(|&s| s == 5000), "{written:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pacat_stream_with_fake_binary_writes_zeros_at_gain_zero() {
        // `sh -c 'cat > FILE'` stands in for pacat: it records what we write.
        let dir = std::env::temp_dir().join(format!("webos-sat-pacat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("written.raw");
        let script = dir.join("fake-pacat");
        crate::testutil::write_script(&script, &format!("cat > '{}'", out.display()));
        let mut cfg = SinkSection::default();
        cfg.binary.clone_from(&script);
        cfg.gain = 0.0;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, SharedGain::new(1.0), tx);
        p.start_stream(16_000, 1);
        p.push(chunk(12_345, 512));
        p.push(chunk(-7, 512));
        p.end_stream();
        let done = rx.recv().await.unwrap();
        let stats = done.result.unwrap();
        assert_eq!(stats.nonzero_samples, Some(0));
        let written = std::fs::read(&out).unwrap();
        assert_eq!(written.len(), 2048);
        assert!(written.iter().all(|&b| b == 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fake_pacat_dir(tag: &str) -> (PathBuf, PathBuf, PathBuf, SinkSection) {
        let dir = std::env::temp_dir().join(format!("webos-sat-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("written.raw");
        let args = dir.join("args.txt");
        let script = dir.join("fake-pacat");
        crate::testutil::write_script(
            &script,
            &format!(
                "echo \"$@\" > '{}'\ncat > '{}'",
                args.display(),
                out.display()
            ),
        );
        let mut cfg = SinkSection::default();
        cfg.binary.clone_from(&script);
        (dir, out, args, cfg)
    }

    fn samples(path: &std::path::Path) -> Vec<i16> {
        std::fs::read(path)
            .unwrap()
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn alarm_chime_streams_to_pacat_paced_and_scaled() {
        let (dir, out, args, mut cfg) = fake_pacat_dir("alarm-pacat");
        cfg.gain = 0.5;
        let volume = SharedGain::new(1.0);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, volume, tx);
        assert!(p.start_alarm(Duration::from_secs(60)).is_none());
        assert_eq!(p.current_kind(), Some(Kind::Alarm));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        // A stop for replies and aborts leaves the alarm alone.
        assert_eq!(p.stop_reply(), None);
        assert_eq!(p.current_kind(), Some(Kind::Alarm));
        assert_eq!(p.stop(), Some(Kind::Alarm));
        let done = rx.recv().await.unwrap();
        assert_eq!(done.kind, Kind::Alarm);
        assert!(!p.finish(&done), "a stopped alarm is not reported again");
        let a = std::fs::read_to_string(&args).unwrap();
        assert!(a.contains("--rate=16000"), "{a}");
        assert!(a.contains("--channels=1"), "{a}");
        assert!(a.contains("--stream-name=timer-alarm"), "{a}");
        assert!(a.contains("--latency-msec=200"), "{a}");
        let s = samples(&out);
        // Paced near real time: about 1.5 s plus the 400 ms lead, not a flood.
        assert!(
            (16_000..=38_400).contains(&s.len()),
            "{} samples written",
            s.len()
        );
        // Gain 0.5 on the 0.7 full-scale chime.
        let peak = s.iter().map(|x| i32::from(*x).abs()).max().unwrap();
        assert!((9_000..=12_000).contains(&peak), "peak {peak}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn alarm_chime_repeats_and_ends_at_its_limit() {
        let (dir, out, _args, cfg) = fake_pacat_dir("alarm-limit");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, SharedGain::new(1.0), tx);
        // Limit 2.9 s: a full 2.6 s cycle and a start of the second.
        p.start_alarm_with(Duration::from_millis(2900), Duration::ZERO);
        let done = rx.recv().await.unwrap();
        assert!(p.finish(&done));
        let st = done.result.unwrap();
        assert!(st.nonzero_samples.unwrap() > 10_000);
        let s = samples(&out);
        let cycle = crate::alarm::chime_cycle();
        assert!(s.len() > cycle.len(), "{} samples", s.len());
        assert_eq!(
            &s[..cycle.len()],
            &cycle[..],
            "the first cycle is the chime"
        );
        assert_eq!(
            &s[cycle.len()..cycle.len() + 1000],
            &cycle[..1000],
            "it repeats"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn alarm_at_gain_zero_writes_only_zeros_and_a_reply_replaces_it() {
        let (dir, out, _args, mut cfg) = fake_pacat_dir("alarm-silent");
        cfg.gain = 0.0;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, SharedGain::new(1.0), tx);
        p.start_alarm(Duration::from_secs(60));
        tokio::time::sleep(Duration::from_millis(600)).await;
        // A reply stream takes the speaker: the alarm is reported as stopped.
        assert_eq!(p.start_stream(16_000, 1), Some(Kind::Alarm));
        assert_eq!(p.current_kind(), Some(Kind::Tts));
        let first = rx.recv().await.unwrap();
        assert_eq!(first.kind, Kind::Alarm);
        assert!(!p.finish(&first));
        let s = samples(&out);
        assert!(!s.is_empty());
        assert!(s.iter().all(|x| *x == 0));
        p.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn alarm_wav_sink_writes_one_cycle() {
        let dir = std::env::temp_dir().join(format!("webos-sat-alarm-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = SinkSection::default();
        cfg.kind = SinkKind::Wav;
        cfg.dir.clone_from(&dir);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, SharedGain::new(1.0), tx);
        p.start_alarm_with(Duration::from_millis(100), Duration::ZERO);
        let done = rx.recv().await.unwrap();
        assert!(p.finish(&done));
        let st = done.result.unwrap();
        let file = st.file.unwrap();
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("alarm-"), "{name}");
        let cycle_bytes = crate::alarm::chime_cycle().len() * 2;
        assert_eq!(
            std::fs::metadata(&file).unwrap().len(),
            44 + cycle_bytes as u64
        );
        assert!(st.nonzero_samples.unwrap() > 10_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
