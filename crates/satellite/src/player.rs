//! Reply playback: streamed TTS (SPEAKER mode) and URL media (TTS URL mode,
//! announcements), to `pacat`, a WAV file or nowhere.
//!
//! One playback at a time. Each playback runs in its own task and reports a
//! [`Done`] with its id; the daemon ignores reports of playbacks it already
//! stopped, so a late report can never finish a newer reply.
//!
//! Streamed TTS is written to `pacat` as it arrives, at its native 16 kHz mono
//! (PulseAudio converts it), with the software gain applied to every sample
//! before the write. With `sink.gain = 0` every byte written is zero: that is
//! how silent tests on c3-a work, because `pacat --volume` is ignored there.

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

/// What a playback was for; decides which "finished" message HA gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Pipeline TTS reply → `PlaybackFinished`.
    Tts,
    /// Announcement → `AnnounceFinished`.
    Announce,
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
    done_tx: mpsc::UnboundedSender<Done>,
    next_id: u64,
    current: Option<Active>,
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
    pub(crate) fn new(cfg: SinkSection, done_tx: mpsc::UnboundedSender<Done>) -> Self {
        Self {
            cfg,
            done_tx,
            next_id: 0,
            current: None,
        }
    }

    /// Whether something is playing (or streaming in).
    pub(crate) fn is_active(&self) -> bool {
        self.current.is_some()
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
        let done_tx = self.done_tx.clone();
        let task = tokio::spawn(async move {
            let result = match cfg.kind {
                SinkKind::Pacat => stream_pacat(&cfg, rate, channels, chunk_rx, cancel_rx).await,
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
        let done_tx = self.done_tx.clone();
        let task = tokio::spawn(async move {
            let result =
                play_url_task(&cfg, id, kind, &url, preannounce.as_deref(), cancel_rx).await;
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

fn pacat_stream_args(cfg: &SinkSection, rate: u32, channels: u16) -> Vec<String> {
    let mut a = vec![
        "--playback".to_owned(),
        "--raw".to_owned(),
        "--format=s16le".to_owned(),
        format!("--rate={rate}"),
        format!("--channels={channels}"),
        "--client-name=webos-voice-satellite".to_owned(),
        "--stream-name=tts-reply".to_owned(),
    ];
    if let Some(ms) = cfg.latency_msec {
        a.push(format!("--latency-msec={ms}"));
    }
    if let Some(d) = &cfg.device {
        a.push(format!("--device={d}"));
    }
    a
}

async fn stream_pacat(
    cfg: &SinkSection,
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
                let (out, nonzero) = gain_s16le(&chunk, cfg.gain);
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

fn url_sink(cfg: &SinkSection, id: u64, kind: Kind) -> (Box<dyn Sink>, Option<PathBuf>) {
    match cfg.kind {
        SinkKind::Pacat => {
            let mut p = PacatSink::new();
            p.binary.clone_from(&cfg.binary);
            p.rate = cfg.rate;
            p.channels = cfg.channels;
            p.gain = cfg.gain;
            p.device.clone_from(&cfg.device);
            (Box::new(p), None)
        }
        SinkKind::Wav => {
            let prefix = match kind {
                Kind::Tts => "reply-url",
                Kind::Announce => "announce",
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

async fn play_url_task(
    cfg: &SinkSection,
    id: u64,
    kind: Kind,
    url: &str,
    preannounce: Option<&str>,
    mut cancel: oneshot::Receiver<()>,
) -> Result<PlayStats, String> {
    let (mut sink, file) = url_sink(cfg, id, kind);
    let stop = sink.stop_handle();
    let fetch = FetchConfig::default();
    let started = Instant::now();
    let play = async {
        if let Some(pre) = preannounce {
            if let Err(e) = webos_playback::play_url(pre, &fetch, sink.as_mut()).await {
                log::warn!("player: pre-announcement {} failed: {e}", redact_url(pre));
            }
        }
        webos_playback::play_url(url, &fetch, sink.as_mut()).await
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
        cfg.latency_msec = Some(200);
        cfg.device = Some("pcm_output".to_owned());
        let a = pacat_stream_args(&cfg, 16_000, 1);
        assert!(a.contains(&"--latency-msec=200".to_owned()));
        assert!(a.contains(&"--device=pcm_output".to_owned()));
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
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = SinkSection::default();
        cfg.binary.clone_from(&script);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, tx);
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
        let mut p = Player::new(cfg, tx);
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
        let mut p = Player::new(cfg, tx);
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
    async fn pacat_stream_with_fake_binary_writes_zeros_at_gain_zero() {
        // `sh -c 'cat > FILE'` stands in for pacat: it records what we write.
        let dir = std::env::temp_dir().join(format!("webos-sat-pacat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("written.raw");
        let script = dir.join("fake-pacat");
        std::fs::write(&script, format!("#!/bin/sh\ncat > '{}'\n", out.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = SinkSection::default();
        cfg.binary.clone_from(&script);
        cfg.gain = 0.0;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Player::new(cfg, tx);
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
}
