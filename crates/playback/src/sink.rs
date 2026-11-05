//! Output sinks: PulseAudio via `pacat`, a WAV file for PC tests, and a null sink.
//!
//! [`Sink::play`] is async and returns a boxed future so the trait stays
//! object-safe (`&mut dyn Sink`). Cancellation (barge-in) works two ways:
//! call [`StopHandle::stop`] from another task, or drop the `play` future;
//! the `pacat` child is spawned with `kill_on_drop`, so either way it is
//! killed and reaped.
//!
//! [`Sink::open`] starts an incremental playback instead ([`SinkWriter`]):
//! [`crate::play_url`] writes each decoded chunk as it arrives, so a reply
//! or an announcement never has to be held in memory as a whole.

use std::fs::File;
use std::future::Future;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, Command};
use tokio::sync::watch;

use crate::error::PlaybackError;
use crate::pcm::Pcm;
use crate::resample::resample;

/// Output sample format requested from the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkFormat {
    /// Sample rate in Hz.
    pub rate_hz: u32,
    /// Number of interleaved channels.
    pub channels: u16,
}

/// Boxed future returned by [`Sink::play`].
pub type PlayFuture<'a> = Pin<Box<dyn Future<Output = Result<(), PlaybackError>> + Send + 'a>>;

/// Cloneable remote control that cancels the sink's current playback.
///
/// A stop issued while nothing plays is ignored (it does not affect a later
/// `play`).
#[derive(Debug, Clone)]
pub struct StopHandle {
    tx: Arc<watch::Sender<u64>>,
}

impl StopHandle {
    fn new() -> Self {
        Self {
            tx: Arc::new(watch::channel(0).0),
        }
    }

    /// Cancels the playback in progress, if any; `play` returns
    /// [`PlaybackError::Cancelled`].
    pub fn stop(&self) {
        self.tx.send_modify(|g| *g = g.wrapping_add(1));
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.tx.subscribe()
    }
}

/// Destination for decoded reply audio.
pub trait Sink: Send {
    /// The format this sink wants; [`crate::play_url`] resamples to it.
    fn format(&self) -> SinkFormat;

    /// Plays `pcm` to completion (resolves when the audio has finished).
    /// Input in another format is converted with [`resample`] first.
    fn play<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a>;

    /// Starts an incremental playback: write chunks already in
    /// [`Sink::format`] to the returned writer, then
    /// [`finish`](SinkWriter::finish) it. Dropping the writer cancels the
    /// playback; so does a [`StopHandle::stop`] during a write or the finish.
    ///
    /// # Errors
    /// [`PlaybackError::Sink`] when `pacat` cannot be started,
    /// [`PlaybackError::Io`] when a WAV file cannot be created.
    fn open(&mut self) -> Result<Box<dyn SinkWriter + '_>, PlaybackError>;

    /// Handle for cancelling `play` from another task.
    fn stop_handle(&self) -> StopHandle;

    /// Cancels current playback. Convenience for `self.stop_handle().stop()`.
    fn stop(&mut self) {
        self.stop_handle().stop();
    }
}

/// One incremental playback, from [`Sink::open`].
pub trait SinkWriter: Send {
    /// Writes audio in the sink's format (another format is converted, but
    /// chunk by chunk, so a stream should be converted before, with
    /// [`crate::StreamResampler`]). May wait while the output is full (the
    /// `pacat` pipe), which paces the caller.
    fn write<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a>;

    /// No more audio: resolves when everything written has been played.
    fn finish(&mut self) -> PlayFuture<'_>;
}

fn conform(pcm: &Pcm, fmt: SinkFormat) -> Pcm {
    if pcm.rate == fmt.rate_hz && pcm.channels == fmt.channels {
        pcm.clone()
    } else {
        resample(pcm, fmt.rate_hz, fmt.channels)
    }
}

// ---------------------------------------------------------------- PacatSink

/// Plays through the TV's PulseAudio by spawning `pacat` and writing raw PCM
/// to its stdin. No sink name is assumed: without `device` PulseAudio uses its
/// default sink.
#[derive(Debug, Clone)]
pub struct PacatSink {
    /// Path or name of the `pacat` binary.
    pub binary: PathBuf,
    /// Sample rate sent to PulseAudio.
    pub rate: u32,
    /// Channel count sent to PulseAudio.
    pub channels: u16,
    /// Stream volume as a PulseAudio integer (0 is mute, 65536 is 100%).
    /// `None` leaves PulseAudio's default for the stream.
    pub volume: Option<u32>,
    /// Software gain applied to the samples before they are written (1.0 is
    /// unity, 0.0 is digital silence). Independent of `volume`: on c3-a the
    /// stream volume requested with `--volume` was not honoured by PulseAudio,
    /// so in-process gain is the reliable level control.
    pub gain: f32,
    /// Sink name (`--device`); `None` selects the default sink.
    pub device: Option<String>,
    /// Extra arguments appended to the command line.
    pub extra_args: Vec<String>,
    /// Slack added to the audio duration before the child is killed.
    pub timeout_margin: Duration,
    stop: StopHandle,
}

impl Default for PacatSink {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("pacat"),
            rate: 44_100,
            channels: 2,
            volume: None,
            gain: 1.0,
            device: None,
            extra_args: Vec::new(),
            timeout_margin: Duration::from_secs(15),
            stop: StopHandle::new(),
        }
    }
}

impl PacatSink {
    /// A sink with the defaults (`pacat`, 44.1 kHz stereo, default device).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The arguments `pacat` is started with (exposed for tests and logs).
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let mut a = vec![
            "--playback".into(),
            "--raw".into(),
            "--format=s16le".into(),
            format!("--rate={}", self.rate),
            format!("--channels={}", self.channels),
            "--client-name=webos-voice-satellite".into(),
            "--stream-name=tts-reply".into(),
        ];
        if let Some(v) = self.volume {
            a.push(format!("--volume={v}"));
        }
        if let Some(d) = &self.device {
            a.push(format!("--device={d}"));
        }
        a.extend(self.extra_args.iter().cloned());
        a
    }

    fn spawn(&self) -> Result<Child, PlaybackError> {
        Command::new(&self.binary)
            .args(self.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| PlaybackError::Sink(format!("cannot start pacat: {e}")))
    }

    async fn run(&self, pcm: &Pcm) -> Result<(), PlaybackError> {
        let fmt = self.format();
        let mut out = conform(pcm, fmt);
        out.apply_gain(self.gain);
        let data = out.to_s16le_bytes();
        let mut stop_rx = self.stop.subscribe();

        let mut child = self.spawn()?;
        let mut stdin = child.stdin.take();
        let mut stderr = child.stderr.take();

        let io = async {
            if let Some(w) = stdin.as_mut() {
                // Chunked so a stop request is noticed between writes.
                for chunk in data.chunks(16 * 1024) {
                    w.write_all(chunk).await?;
                }
                w.shutdown().await?;
            }
            drop(stdin.take()); // EOF: pacat drains the stream and exits.
            child.wait().await
        };
        let deadline = conform_duration(data.len(), fmt) + self.timeout_margin;

        let outcome = tokio::select! {
            r = io => Outcome::Done(r),
            _ = stop_rx.changed() => Outcome::Stopped,
            () = tokio::time::sleep(deadline) => Outcome::TimedOut(deadline),
        };
        match outcome {
            Outcome::Done(Ok(status)) if status.success() => Ok(()),
            Outcome::Done(res) => Err(pacat_failure(res, stderr.as_mut()).await),
            Outcome::Stopped => {
                let _ = child.kill().await;
                Err(PlaybackError::Cancelled)
            }
            Outcome::TimedOut(d) => {
                let _ = child.kill().await;
                Err(PlaybackError::PlayTimeout(d))
            }
        }
    }
}

enum Outcome {
    Done(std::io::Result<std::process::ExitStatus>),
    Stopped,
    TimedOut(Duration),
}

/// The error for a `pacat` that exited unsuccessfully or whose pipe broke,
/// with the start of its stderr.
async fn pacat_failure(
    res: std::io::Result<std::process::ExitStatus>,
    stderr: Option<&mut ChildStderr>,
) -> PlaybackError {
    let mut msg = String::new();
    if let Some(e) = stderr {
        let mut buf = vec![0u8; 512];
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(1), e.read(&mut buf)).await {
            msg = String::from_utf8_lossy(&buf[..n]).trim().to_owned();
        }
    }
    match res {
        Ok(status) => PlaybackError::Sink(format!("pacat exited with {status}: {msg}")),
        Err(e) => PlaybackError::Sink(format!("pacat I/O failed: {e}: {msg}")),
    }
}

/// An incremental `pacat` playback ([`PacatSink::open`]).
///
/// Each write waits at most the chunk's duration plus the sink's
/// `timeout_margin` (a `pacat` that stops reading is killed), and
/// [`finish`](SinkWriter::finish) waits at most for the audio still to be
/// played plus the margin, as [`PacatSink::play`] does for a whole buffer.
struct PacatWriter {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<ChildStderr>,
    fmt: SinkFormat,
    gain: f32,
    margin: Duration,
    stop: watch::Receiver<u64>,
    /// When the first audio was written.
    started: Option<Instant>,
    /// Bytes written so far.
    written: usize,
}

enum Step {
    Wrote(std::io::Result<()>),
    Exited(std::io::Result<std::process::ExitStatus>),
    Stopped,
    TimedOut(Duration),
}

impl PacatWriter {
    async fn fail(&mut self, step: Step) -> PlaybackError {
        match step {
            Step::Wrote(r) => {
                // A broken pipe: pacat exited; report its status and stderr.
                drop(self.stdin.take());
                let res = match (
                    r,
                    tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await,
                ) {
                    (_, Ok(status)) => status,
                    (Err(e), Err(_)) => Err(e),
                    (Ok(()), Err(_)) => Err(std::io::Error::other("pacat did not exit")),
                };
                let _ = self.child.kill().await;
                pacat_failure(res, self.stderr.as_mut()).await
            }
            Step::Exited(res) => pacat_failure(res, self.stderr.as_mut()).await,
            Step::Stopped => {
                let _ = self.child.kill().await;
                PlaybackError::Cancelled
            }
            Step::TimedOut(d) => {
                let _ = self.child.kill().await;
                PlaybackError::PlayTimeout(d)
            }
        }
    }
}

impl SinkWriter for PacatWriter {
    fn write<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a> {
        Box::pin(async move {
            let mut out = conform(pcm, self.fmt);
            out.apply_gain(self.gain);
            let data = out.to_s16le_bytes();
            drop(out);
            self.started.get_or_insert_with(Instant::now);
            let deadline = conform_duration(data.len(), self.fmt) + self.margin;
            let Some(stdin) = self.stdin.as_mut() else {
                return Err(PlaybackError::Sink("pacat input already closed".into()));
            };
            let step = tokio::select! {
                r = stdin.write_all(&data) => Step::Wrote(r),
                _ = self.stop.changed() => Step::Stopped,
                () = tokio::time::sleep(deadline) => Step::TimedOut(deadline),
            };
            if matches!(step, Step::Wrote(Ok(()))) {
                self.written += data.len();
                return Ok(());
            }
            Err(self.fail(step).await)
        })
    }

    fn finish(&mut self) -> PlayFuture<'_> {
        Box::pin(async move {
            if let Some(mut w) = self.stdin.take() {
                // EOF: pacat drains the stream and exits.
                let _ = w.shutdown().await;
            }
            let played = self.started.map_or(Duration::ZERO, |t| t.elapsed());
            let deadline =
                conform_duration(self.written, self.fmt).saturating_sub(played) + self.margin;
            let step = tokio::select! {
                r = self.child.wait() => Step::Exited(r),
                _ = self.stop.changed() => Step::Stopped,
                () = tokio::time::sleep(deadline) => Step::TimedOut(deadline),
            };
            match step {
                Step::Exited(Ok(status)) if status.success() => Ok(()),
                other => Err(self.fail(other).await),
            }
        })
    }
}

fn conform_duration(bytes: usize, fmt: SinkFormat) -> Duration {
    let bytes_per_sec = u64::from(fmt.rate_hz) * u64::from(fmt.channels) * 2;
    if bytes_per_sec == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(bytes as u64 * 1000 / bytes_per_sec)
}

impl Sink for PacatSink {
    fn format(&self) -> SinkFormat {
        SinkFormat {
            rate_hz: self.rate,
            channels: self.channels,
        }
    }

    fn play<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a> {
        Box::pin(async move { self.run(pcm).await })
    }

    fn open(&mut self) -> Result<Box<dyn SinkWriter + '_>, PlaybackError> {
        let stop = self.stop.subscribe();
        let mut child = self.spawn()?;
        Ok(Box::new(PacatWriter {
            stdin: child.stdin.take(),
            stderr: child.stderr.take(),
            child,
            fmt: self.format(),
            gain: self.gain,
            margin: self.timeout_margin,
            stop,
            started: None,
            written: 0,
        }))
    }

    fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }
}

// -------------------------------------------------------------- WavFileSink

/// Writes the audio to a 16-bit PCM WAV file (overwritten on each `play`).
/// For PC tests; plays nothing audible.
#[derive(Debug, Clone)]
pub struct WavFileSink {
    /// Output path.
    pub path: PathBuf,
    /// Target format; defaults to 44.1 kHz stereo like [`PacatSink`].
    pub format: SinkFormat,
    stop: StopHandle,
}

impl WavFileSink {
    /// A sink writing 44.1 kHz stereo to `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: SinkFormat {
                rate_hz: 44_100,
                channels: 2,
            },
            stop: StopHandle::new(),
        }
    }
}

/// The canonical 44-byte header of a 16-bit PCM WAV file.
fn wav_header(rate: u32, channels: u16, data_len: u32) -> Vec<u8> {
    let block_align = channels * 2;
    let mut v = Vec::with_capacity(44);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    v.extend_from_slice(&block_align.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v
}

/// Serialises `pcm` as a canonical 44-byte-header WAV file.
#[must_use]
pub fn wav_bytes(pcm: &Pcm) -> Vec<u8> {
    let data_len = u32::try_from(pcm.samples.len() * 2).unwrap_or(u32::MAX);
    let mut v = wav_header(pcm.rate, pcm.channels, data_len);
    v.reserve(pcm.samples.len() * 2);
    for s in &pcm.samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

/// An incremental WAV file ([`WavFileSink::open`]): the header is written
/// with a length of 0 and corrected by [`finish`](SinkWriter::finish).
struct WavWriter {
    file: BufWriter<File>,
    fmt: SinkFormat,
    data_len: u64,
}

impl SinkWriter for WavWriter {
    fn write<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a> {
        Box::pin(async move {
            let out = conform(pcm, self.fmt);
            for s in &out.samples {
                self.file.write_all(&s.to_le_bytes())?;
            }
            self.data_len += out.samples.len() as u64 * 2;
            Ok(())
        })
    }

    fn finish(&mut self) -> PlayFuture<'_> {
        Box::pin(async move {
            let len = u32::try_from(self.data_len).unwrap_or(u32::MAX);
            let f = &mut self.file;
            f.seek(SeekFrom::Start(0))?;
            f.write_all(&wav_header(self.fmt.rate_hz, self.fmt.channels, len))?;
            f.flush()?;
            Ok(())
        })
    }
}

impl Sink for WavFileSink {
    fn format(&self) -> SinkFormat {
        self.format
    }

    fn play<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a> {
        Box::pin(async move {
            let out = conform(pcm, self.format);
            std::fs::write(&self.path, wav_bytes(&out))?;
            Ok(())
        })
    }

    /// Creates (or truncates) the file, as each `play` does.
    fn open(&mut self) -> Result<Box<dyn SinkWriter + '_>, PlaybackError> {
        let mut file = BufWriter::new(File::create(&self.path)?);
        file.write_all(&wav_header(self.format.rate_hz, self.format.channels, 0))?;
        Ok(Box::new(WavWriter {
            file,
            fmt: self.format,
            data_len: 0,
        }))
    }

    fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }
}

// ----------------------------------------------------------------- NullSink

/// Discards audio; records what it was given. For tests.
#[derive(Debug, Clone)]
pub struct NullSink {
    /// Format advertised to the pipeline.
    pub format: SinkFormat,
    /// Number of `play` calls so far.
    pub plays: usize,
    /// The most recent buffer received (with [`Sink::open`], the most
    /// recent chunk).
    pub last: Option<Pcm>,
    /// Frames received by the most recent playback.
    pub frames: usize,
    stop: StopHandle,
}

impl Default for NullSink {
    fn default() -> Self {
        Self {
            format: SinkFormat {
                rate_hz: 44_100,
                channels: 2,
            },
            plays: 0,
            last: None,
            frames: 0,
            stop: StopHandle::new(),
        }
    }
}

impl Sink for NullSink {
    fn format(&self) -> SinkFormat {
        self.format
    }

    fn play<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a> {
        Box::pin(async move {
            self.plays += 1;
            let out = conform(pcm, self.format);
            self.frames = out.frames();
            self.last = Some(out);
            Ok(())
        })
    }

    /// Counts as one play; keeps only the latest chunk.
    fn open(&mut self) -> Result<Box<dyn SinkWriter + '_>, PlaybackError> {
        self.plays += 1;
        self.frames = 0;
        self.last = None;
        Ok(Box::new(NullWriter(self)))
    }

    fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }
}

struct NullWriter<'s>(&'s mut NullSink);

impl SinkWriter for NullWriter<'_> {
    fn write<'a>(&'a mut self, pcm: &'a Pcm) -> PlayFuture<'a> {
        Box::pin(async move {
            let out = conform(pcm, self.0.format);
            self.0.frames += out.frames();
            self.0.last = Some(out);
            Ok(())
        })
    }

    fn finish(&mut self) -> PlayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}
