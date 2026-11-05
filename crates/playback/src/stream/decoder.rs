//! Incremental, non-seekable decoding of a byte stream into PCM chunks.

use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::common::Limit;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;

use crate::error::PlaybackError;
use crate::pcm::Pcm;
use crate::resample::mix_channels;

/// Lowest sample rate accepted from a stream header.
pub const MIN_STREAM_RATE: u32 = 8_000;
/// Highest sample rate accepted from a stream header.
pub const MAX_STREAM_RATE: u32 = 192_000;
/// Most channels accepted from a stream header (more than 2 are downmixed
/// to stereo, so a chunk always has 1 or 2 channels).
pub const MAX_STREAM_CHANNELS: u16 = 8;
/// Largest decoded packet accepted, in frames (FLAC's maximum block is
/// 65535 frames; MP3 and Vorbis packets are far smaller).
pub const MAX_PACKET_FRAMES: usize = 65_536;
/// Compressed bytes the decoder may read before the first decoded frame
/// (format probing scans at most 1 MiB, plus header metadata).
pub const FIRST_FRAME_BUDGET: u64 = 4 * 1024 * 1024;
/// Compressed bytes the decoder may read between two decoded packets
/// before the stream is declared garbage (stops an endless garbage stream
/// from keeping the demuxer searching for sync forever).
pub const GAP_BUDGET: u64 = 1024 * 1024;
/// Consecutive corrupt packets tolerated before the stream fails.
pub const MAX_CONSECUTIVE_ERRORS: u32 = 64;

const BUDGET_MSG: &str = "no decodable audio within the byte budget";

/// Counts bytes and fails once more than the current budget were read since
/// the decoder last made progress.
struct Budgeted {
    inner: Box<dyn Read + Send + Sync>,
    used: Arc<AtomicU64>,
    budget: Arc<AtomicU64>,
}

impl Read for Budgeted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.used.load(Ordering::Relaxed) > self.budget.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, BUDGET_MSG));
        }
        let n = self.inner.read(buf)?;
        self.used.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn is_budget_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::InvalidData && e.to_string() == BUDGET_MSG
}

/// Builds a symphonia hint from a MIME type (`audio/flac`) or a file
/// extension (`flac`, `.mp3`).
fn make_hint(hint: Option<&str>) -> Hint {
    let mut h = Hint::new();
    if let Some(s) = hint {
        let s = s.trim().to_ascii_lowercase();
        if s.contains('/') {
            h.mime_type(&s);
        } else if !s.is_empty() {
            h.with_extension(s.trim_start_matches('.'));
        }
    }
    h
}

/// Decodes a non-seekable byte stream (FLAC, MP3, WAV, Ogg Vorbis) into
/// interleaved `s16` chunks of a fixed duration.
///
/// The sample rate and channel count come from the stream itself; a change
/// in the middle of the stream is an error. Streams with more than two
/// channels are downmixed to stereo. Corrupt packets are skipped (counted),
/// but a run of [`MAX_CONSECUTIVE_ERRORS`] or [`GAP_BUDGET`] bytes without a
/// decoded packet ends the stream with [`PlaybackError::Decode`], so garbage
/// input cannot keep it busy forever.
///
/// This is the exact code the `music-decode` thread runs, usable on its own
/// for tests and fuzzing.
pub struct StreamDecoder {
    format: Box<dyn FormatReader + 'static>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    chunk_ms: u32,
    /// Source rate and channels, fixed by the first decoded packet.
    src: Option<(u32, u16)>,
    pending: Vec<i16>,
    scratch: Vec<i16>,
    used: Arc<AtomicU64>,
    budget: Arc<AtomicU64>,
    consecutive_errors: u32,
    eof: bool,
    /// An error held back until the audio decoded before it was returned.
    deferred: Option<PlaybackError>,
    frames_decoded: u64,
    packets_skipped: u64,
}

impl std::fmt::Debug for StreamDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamDecoder")
            .field("src", &self.src)
            .field("frames_decoded", &self.frames_decoded)
            .field("packets_skipped", &self.packets_skipped)
            .finish_non_exhaustive()
    }
}

impl StreamDecoder {
    /// Probes `source` and prepares the decoder. Blocks until the source has
    /// delivered enough bytes to recognise the format.
    ///
    /// `hint` is a MIME type or a file extension; it only speeds up probing.
    /// `chunk_ms` is the duration of each returned chunk (at least 1 ms).
    ///
    /// # Errors
    /// [`PlaybackError::Decode`] when the bytes are not a supported audio
    /// format or contain no audio track.
    pub fn new(
        source: Box<dyn Read + Send + Sync>,
        hint: Option<&str>,
        chunk_ms: u32,
    ) -> Result<Self, PlaybackError> {
        let used = Arc::new(AtomicU64::new(0));
        let budget = Arc::new(AtomicU64::new(FIRST_FRAME_BUDGET));
        let reader = Budgeted {
            inner: source,
            used: Arc::clone(&used),
            budget: Arc::clone(&budget),
        };
        let mss = MediaSourceStream::new(
            Box::new(ReadOnlySource::new(reader)),
            MediaSourceStreamOptions::default(),
        );
        let meta = MetadataOptions::default()
            .limit_tag_bytes(Limit::Maximum(64 * 1024))
            .limit_visual_bytes(Limit::Maximum(0));
        let format = symphonia::default::get_probe()
            .probe(&make_hint(hint), mss, FormatOptions::default(), meta)
            .map_err(|e| map_sym(e, "unrecognised audio"))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| PlaybackError::Decode("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| PlaybackError::Decode("no audio codec parameters".into()))?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| PlaybackError::Decode(format!("no decoder: {e}")))?;
        let track_id = track.id;
        Ok(Self {
            format,
            decoder,
            track_id,
            chunk_ms: chunk_ms.max(1),
            src: None,
            pending: Vec::new(),
            scratch: Vec::new(),
            used,
            budget,
            consecutive_errors: 0,
            eof: false,
            deferred: None,
            frames_decoded: 0,
            packets_skipped: 0,
        })
    }

    /// Output sample rate and channel count (1 or 2), known after the first
    /// decoded packet.
    #[must_use]
    pub fn format(&self) -> Option<(u32, u16)> {
        self.src.map(|(rate, ch)| (rate, ch.min(2)))
    }

    /// Frames decoded so far.
    #[must_use]
    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    /// Corrupt packets skipped so far.
    #[must_use]
    pub fn packets_skipped(&self) -> u64 {
        self.packets_skipped
    }

    fn chunk_samples(&self) -> usize {
        match self.format() {
            Some((rate, ch)) => {
                let frames = (u64::from(rate) * u64::from(self.chunk_ms) / 1000).max(1);
                usize::try_from(frames).unwrap_or(usize::MAX / 4) * usize::from(ch)
            }
            None => usize::MAX,
        }
    }

    fn take_chunk(&mut self, n: usize) -> Option<Pcm> {
        let (rate, channels) = self.format()?;
        // Copy out exactly `n` samples: a chunk must not carry the (larger)
        // capacity of `pending`, the queue bound counts samples, not bytes
        // allocated.
        let samples: Vec<i16> = self.pending.drain(..n.min(self.pending.len())).collect();
        Some(Pcm {
            rate,
            channels,
            samples,
        })
    }

    /// Returns the next chunk of `chunk_ms` (the last one may be shorter),
    /// `Ok(None)` at the end of the stream.
    ///
    /// # Errors
    /// [`PlaybackError::Decode`] for unreadable or garbage data,
    /// [`PlaybackError::Unsupported`] for a format change in the middle of
    /// the stream or parameters outside the accepted range. An I/O error from
    /// the source is reported as [`PlaybackError::Decode`]; the music stream
    /// replaces it with the fetch error that caused it. Audio decoded before
    /// an error is returned first; the error comes on the next call.
    pub fn next_chunk(&mut self) -> Result<Option<Pcm>, PlaybackError> {
        if let Some(e) = self.deferred.take() {
            return Err(e);
        }
        loop {
            let want = self.chunk_samples();
            if self.pending.len() >= want {
                return Ok(self.take_chunk(want));
            }
            if self.eof {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                let n = self.pending.len();
                return Ok(self.take_chunk(n));
            }
            if let Err(e) = self.decode_one() {
                if self.pending.is_empty() {
                    return Err(e);
                }
                // Deliver the audio decoded before the error first.
                self.deferred = Some(e);
                let n = self.pending.len();
                return Ok(self.take_chunk(n));
            }
        }
    }

    fn corrupt(&mut self) -> Result<(), PlaybackError> {
        self.packets_skipped += 1;
        self.consecutive_errors += 1;
        if self.consecutive_errors > MAX_CONSECUTIVE_ERRORS {
            return Err(PlaybackError::Decode(
                "too many corrupt packets in a row".into(),
            ));
        }
        Ok(())
    }

    /// Reads and decodes one packet into `pending`, or sets `eof`.
    fn decode_one(&mut self) -> Result<(), PlaybackError> {
        let packet = match self.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => {
                self.eof = true;
                return Ok(());
            }
            Err(SymError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                self.eof = true;
                return Ok(());
            }
            Err(SymError::DecodeError(_)) => return self.corrupt(),
            Err(e) => return Err(map_sym(e, "read")),
        };
        if packet.track_id != self.track_id {
            return Ok(());
        }
        let buf = match self.decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(SymError::DecodeError(_)) => return self.corrupt(),
            Err(e) => return Err(map_sym(e, "decode")),
        };
        let frames = buf.frames();
        if frames == 0 {
            return Ok(());
        }
        if frames > MAX_PACKET_FRAMES {
            return Err(PlaybackError::Unsupported(format!(
                "decoded packet of {frames} frames"
            )));
        }
        let rate = buf.spec().rate();
        let channels = u16::try_from(buf.spec().channels().count()).unwrap_or(u16::MAX);
        if !(MIN_STREAM_RATE..=MAX_STREAM_RATE).contains(&rate) {
            return Err(PlaybackError::Unsupported(format!("sample rate {rate} Hz")));
        }
        if channels == 0 || channels > MAX_STREAM_CHANNELS {
            return Err(PlaybackError::Unsupported(format!("{channels} channels")));
        }
        match self.src {
            None => self.src = Some((rate, channels)),
            Some(s) if s != (rate, channels) => {
                return Err(PlaybackError::Unsupported(
                    "sample rate or channel count changed mid-stream".into(),
                ));
            }
            Some(_) => {}
        }
        buf.copy_to_vec_interleaved(&mut self.scratch);
        if channels > 2 {
            let multi = Pcm {
                rate,
                channels,
                samples: std::mem::take(&mut self.scratch),
            };
            self.pending
                .extend_from_slice(&mix_channels(&multi, 2).samples);
            self.scratch = multi.samples;
        } else {
            self.pending.extend_from_slice(&self.scratch);
        }
        self.frames_decoded += frames as u64;
        self.consecutive_errors = 0;
        self.used.store(0, Ordering::Relaxed);
        self.budget.store(GAP_BUDGET, Ordering::Relaxed);
        Ok(())
    }
}

/// Maps a symphonia error; `what` names the stage for the message.
fn map_sym(e: SymError, what: &str) -> PlaybackError {
    match e {
        SymError::IoError(io) if is_budget_error(&io) => PlaybackError::Decode(BUDGET_MSG.into()),
        SymError::ResetRequired => {
            PlaybackError::Unsupported("stream parameters changed mid-stream".into())
        }
        other => PlaybackError::Decode(format!("{what}: {other}")),
    }
}
