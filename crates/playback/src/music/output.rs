//! Where the music engine writes: a second `pacat`, a streaming WAV file, or
//! (in tests) a recorder.
//!
//! An output is opened **per track from the decoded stream format**
//! ([`OutputFactory::open`] gets the rate and channels of the first chunk),
//! never from configuration. Writes are raw `s16le`.
//!
//! The engine paces its own writes (a write-ahead of about 60 ms), so an
//! output needs no clock of its own: the streaming WAV file is therefore
//! written in real time like the real sink, in small writes, and never holds
//! the track in memory.

use std::future::Future;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};

use crate::sink::SinkFormat;

/// Boxed future of an output operation.
pub type OutFuture<'a> = Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>>;

/// One open output stream.
pub trait MusicOutput: Send {
    /// Writes raw `s16le` interleaved audio.
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> OutFuture<'a>;
    /// Ends the stream normally: what was written plays out, then the output
    /// is closed (`pacat` drains and exits).
    fn finish(&mut self) -> OutFuture<'_>;
    /// Ends the stream at once, discarding unplayed audio, and reaps the
    /// process or file (nothing is left running when it returns).
    fn abort(&mut self) -> OutFuture<'_>;
}

/// Opens an output for a track.
pub trait OutputFactory: Send {
    /// Opens an output for a stream of the given decoded format.
    ///
    /// # Errors
    /// The output could not be started (for example `pacat` is missing).
    fn open(&mut self, format: SinkFormat) -> std::io::Result<Box<dyn MusicOutput>>;
}

// ------------------------------------------------------------------- pacat

/// Settings of the second `pacat` (`[music]` and `[sink]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacatConfig {
    /// Path or name of the `pacat` binary.
    pub binary: PathBuf,
    /// PulseAudio sink (`--device`); `None` uses the default sink.
    pub device: Option<String>,
    /// `--latency-msec` (design: 100 for the music slot); 0 omits the flag.
    pub latency_msec: u32,
    /// `--stream-name`.
    pub stream_name: String,
}

impl Default for PacatConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("pacat"),
            device: None,
            latency_msec: 100,
            stream_name: "music".to_owned(),
        }
    }
}

impl PacatConfig {
    /// The arguments `pacat` is started with for `format`.
    #[must_use]
    pub fn args(&self, format: SinkFormat) -> Vec<String> {
        let mut a = vec![
            "--playback".to_owned(),
            "--raw".to_owned(),
            "--format=s16le".to_owned(),
            format!("--rate={}", format.rate_hz),
            format!("--channels={}", format.channels),
            "--client-name=webos-voice-satellite".to_owned(),
            format!("--stream-name={}", self.stream_name),
        ];
        if self.latency_msec > 0 {
            a.push(format!("--latency-msec={}", self.latency_msec));
        }
        if let Some(d) = &self.device {
            a.push(format!("--device={d}"));
        }
        a
    }
}

/// Opens a `pacat` child per track.
#[derive(Debug, Clone)]
pub struct PacatFactory {
    cfg: PacatConfig,
}

impl PacatFactory {
    /// A factory with the given settings.
    #[must_use]
    pub fn new(cfg: PacatConfig) -> Self {
        Self { cfg }
    }
}

impl OutputFactory for PacatFactory {
    fn open(&mut self, format: SinkFormat) -> std::io::Result<Box<dyn MusicOutput>> {
        let mut child = Command::new(&self.cfg.binary)
            .args(self.cfg.args(format))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take();
        Ok(Box::new(PacatOutput { child, stdin }))
    }
}

struct PacatOutput {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl MusicOutput for PacatOutput {
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> OutFuture<'a> {
        Box::pin(async move {
            match self.stdin.as_mut() {
                Some(w) => w.write_all(bytes).await,
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "pacat input closed",
                )),
            }
        })
    }

    fn finish(&mut self) -> OutFuture<'_> {
        Box::pin(async move {
            drop(self.stdin.take()); // EOF: pacat drains its buffer and exits.
            let status = self.child.wait().await?;
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("pacat exited with {status}")))
            }
        })
    }

    fn abort(&mut self) -> OutFuture<'_> {
        Box::pin(async move {
            drop(self.stdin.take());
            let _ = self.child.kill().await; // kills and reaps
            Ok(())
        })
    }
}

// ------------------------------------------------------------ streaming WAV

/// Writes each track to `<dir>/<prefix>-<n>.wav`, a file that grows while the
/// track plays (header sizes are patched on finish and abort, so a stopped
/// track is still a valid file). For PC tests; plays nothing audible.
#[derive(Debug)]
pub struct WavStreamFactory {
    dir: PathBuf,
    prefix: String,
    next: u32,
    paths: Arc<Mutex<Vec<PathBuf>>>,
}

impl WavStreamFactory {
    /// A factory writing into `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, prefix: &str) -> Self {
        Self {
            dir: dir.into(),
            prefix: prefix.to_owned(),
            next: 0,
            paths: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The files opened so far, shared so a test can read them while the
    /// engine owns the factory.
    #[must_use]
    pub fn paths(&self) -> Arc<Mutex<Vec<PathBuf>>> {
        Arc::clone(&self.paths)
    }
}

impl OutputFactory for WavStreamFactory {
    fn open(&mut self, format: SinkFormat) -> std::io::Result<Box<dyn MusicOutput>> {
        self.next += 1;
        let path = self.dir.join(format!("{}-{}.wav", self.prefix, self.next));
        let out = WavStreamOutput::create(&path, format)?;
        if let Ok(mut p) = self.paths.lock() {
            p.push(path);
        }
        Ok(Box::new(out))
    }
}

struct WavStreamOutput {
    file: Option<std::fs::File>,
    data_len: u64,
}

const WAV_HEADER: usize = 44;

fn wav_header(format: SinkFormat, data_len: u32) -> [u8; WAV_HEADER] {
    let block_align = format.channels * 2;
    let mut h = [0u8; WAV_HEADER];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&data_len.saturating_add(36).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&format.channels.to_le_bytes());
    h[24..28].copy_from_slice(&format.rate_hz.to_le_bytes());
    h[28..32].copy_from_slice(&(format.rate_hz * u32::from(block_align)).to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

impl WavStreamOutput {
    fn create(path: &Path, format: SinkFormat) -> std::io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&wav_header(format, 0))?;
        // Remember the format in the header itself: patched later.
        Ok(Self {
            file: Some(file),
            data_len: 0,
        })
    }

    fn patch(&mut self) -> std::io::Result<()> {
        if let Some(mut f) = self.file.take() {
            let len = u32::try_from(self.data_len).unwrap_or(u32::MAX);
            f.seek(SeekFrom::Start(4))?;
            f.write_all(&len.saturating_add(36).to_le_bytes())?;
            f.seek(SeekFrom::Start(40))?;
            f.write_all(&len.to_le_bytes())?;
            f.flush()?;
        }
        Ok(())
    }
}

impl MusicOutput for WavStreamOutput {
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> OutFuture<'a> {
        Box::pin(async move {
            match self.file.as_mut() {
                Some(f) => {
                    f.write_all(bytes)?;
                    self.data_len += bytes.len() as u64;
                    Ok(())
                }
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "wav output closed",
                )),
            }
        })
    }

    fn finish(&mut self) -> OutFuture<'_> {
        Box::pin(async move { self.patch() })
    }

    fn abort(&mut self) -> OutFuture<'_> {
        Box::pin(async move { self.patch() })
    }
}

/// Reads a mono or stereo 16-bit WAV file written by [`WavStreamFactory`]:
/// the format and the interleaved samples.
///
/// # Errors
/// The file is unreadable or not a canonical 44-byte-header PCM16 WAV.
pub fn read_wav(path: &Path) -> std::io::Result<(SinkFormat, Vec<i16>)> {
    let b = std::fs::read(path)?;
    let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_owned());
    if b.len() < WAV_HEADER || &b[0..4] != b"RIFF" || &b[36..40] != b"data" {
        return Err(bad("not a canonical WAV"));
    }
    let channels = u16::from_le_bytes([b[22], b[23]]);
    let rate_hz = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
    let len = u32::from_le_bytes([b[40], b[41], b[42], b[43]]) as usize;
    let end = (WAV_HEADER + len).min(b.len());
    let samples = b[WAV_HEADER..end]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok((SinkFormat { rate_hz, channels }, samples))
}

// -------------------------------------------------------------- test output

/// What a [`RecordingFactory`] saw, for engine tests.
#[cfg(test)]
pub(crate) mod recording {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::time::Duration;
    use tokio::time::Instant;

    /// One opened output.
    #[derive(Debug, Default, Clone)]
    pub(crate) struct Opened {
        pub(crate) format: Option<SinkFormat>,
        /// (time since the factory was made, bytes written).
        pub(crate) writes: Vec<(Duration, usize)>,
        pub(crate) data: Vec<u8>,
        pub(crate) finished: bool,
        pub(crate) aborted: bool,
    }

    /// Everything recorded, shared with the test.
    #[derive(Debug, Default)]
    pub(crate) struct Log {
        pub(crate) opens: Vec<Opened>,
        /// Stall the write with this global index for this long.
        pub(crate) stall: Option<(usize, Duration)>,
        pub(crate) write_count: usize,
        pub(crate) fail_open: bool,
    }

    pub(crate) type Shared = Arc<Mutex<Log>>;

    pub(crate) struct RecordingFactory {
        pub(crate) log: Shared,
        pub(crate) t0: Instant,
    }

    impl RecordingFactory {
        pub(crate) fn new() -> (Self, Shared) {
            let log = Shared::default();
            (
                Self {
                    log: Arc::clone(&log),
                    t0: Instant::now(),
                },
                log,
            )
        }
    }

    impl OutputFactory for RecordingFactory {
        fn open(&mut self, format: SinkFormat) -> std::io::Result<Box<dyn MusicOutput>> {
            let mut l = self.log.lock().unwrap();
            if l.fail_open {
                return Err(std::io::Error::other("open refused"));
            }
            l.opens.push(Opened {
                format: Some(format),
                ..Opened::default()
            });
            let index = l.opens.len() - 1;
            Ok(Box::new(RecordingOutput {
                log: Arc::clone(&self.log),
                index,
                t0: self.t0,
            }))
        }
    }

    struct RecordingOutput {
        log: Shared,
        index: usize,
        t0: Instant,
    }

    impl MusicOutput for RecordingOutput {
        fn write<'a>(&'a mut self, bytes: &'a [u8]) -> OutFuture<'a> {
            Box::pin(async move {
                let stall = {
                    let mut l = self.log.lock().unwrap();
                    let n = l.write_count;
                    l.write_count += 1;
                    match l.stall {
                        Some((at, d)) if at == n => Some(d),
                        _ => None,
                    }
                };
                if let Some(d) = stall {
                    tokio::time::sleep(d).await;
                }
                let mut l = self.log.lock().unwrap();
                let t = self.t0.elapsed();
                let o = &mut l.opens[self.index];
                o.writes.push((t, bytes.len()));
                o.data.extend_from_slice(bytes);
                Ok(())
            })
        }

        fn finish(&mut self) -> OutFuture<'_> {
            Box::pin(async move {
                self.log.lock().unwrap().opens[self.index].finished = true;
                Ok(())
            })
        }

        fn abort(&mut self) -> OutFuture<'_> {
            Box::pin(async move {
                self.log.lock().unwrap().opens[self.index].aborted = true;
                Ok(())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn pacat_args_follow_the_decoded_format() {
        let cfg = PacatConfig::default();
        let a = cfg.args(SinkFormat {
            rate_hz: 44_100,
            channels: 2,
        });
        assert!(a.contains(&"--rate=44100".to_owned()));
        assert!(a.contains(&"--channels=2".to_owned()));
        assert!(a.contains(&"--format=s16le".to_owned()));
        assert!(a.contains(&"--stream-name=music".to_owned()));
        assert!(a.contains(&"--latency-msec=100".to_owned()));
        assert!(!a.iter().any(|x| x.starts_with("--device")));
        let a = PacatConfig {
            latency_msec: 0,
            device: Some("sink0".to_owned()),
            ..cfg
        }
        .args(SinkFormat {
            rate_hz: 22_050,
            channels: 1,
        });
        assert!(a.contains(&"--rate=22050".to_owned()));
        assert!(a.contains(&"--device=sink0".to_owned()));
        assert!(!a.iter().any(|x| x.starts_with("--latency")));
    }

    #[tokio::test]
    async fn wav_stream_is_a_valid_file_after_abort() {
        let dir = std::env::temp_dir().join(format!("webos-music-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = WavStreamFactory::new(&dir, "t");
        let fmt = SinkFormat {
            rate_hz: 8000,
            channels: 2,
        };
        let mut o = f.open(fmt).unwrap();
        o.write(&[1, 0, 2, 0, 3, 0, 4, 0]).await.unwrap();
        o.abort().await.unwrap();
        let path = f.paths().lock().unwrap()[0].clone();
        let (got, samples) = read_wav(&path).unwrap();
        assert_eq!(got, fmt);
        assert_eq!(samples, vec![1, 2, 3, 4]);
        assert!(o.write(&[0, 0]).await.is_err(), "closed after abort");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
