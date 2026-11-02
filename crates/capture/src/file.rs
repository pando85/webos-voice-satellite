//! A file-backed source for testing without hardware.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::source::{ReferenceState, SampleFormat, StreamFormat};
use crate::{AudioSource, CaptureError};

/// `Read + Seek + Send`, boxed inside [`FileSource`].
trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Sign-extends the low 24 bits of a sample.
#[inline]
pub(crate) fn sign_extend_s24(raw: i32) -> i32 {
    (raw << 8) >> 8
}

/// Plays back a raw interleaved file with 4-byte little-endian containers as
/// an [`AudioSource`]: S24_LE by default (the layout `arecord -f S24_LE -t raw`
/// writes), or S32_LE with [`FileSource::format`].
///
/// At end of file [`AudioSource::read`] returns `Ok(0)`, unless looping is on.
/// A trailing partial frame is dropped.
pub struct FileSource {
    reader: Box<dyn ReadSeek>,
    channels: u16,
    rate: u32,
    format: SampleFormat,
    realtime: bool,
    looping: bool,
    bytes: Vec<u8>,
    frames_done: u64,
    started: Option<Instant>,
    /// Bytes read from the file since the last rewind.
    pass_bytes: u64,
}

impl std::fmt::Debug for FileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSource")
            .field("channels", &self.channels)
            .field("rate", &self.rate)
            .field("format", &self.format)
            .field("realtime", &self.realtime)
            .field("looping", &self.looping)
            .field("frames_done", &self.frames_done)
            .finish_non_exhaustive()
    }
}

impl FileSource {
    /// Opens `path`. `channels` and `rate` describe the file contents.
    ///
    /// # Errors
    /// [`CaptureError::Io`] if the file cannot be opened, or
    /// [`CaptureError::Unsupported`] if `channels` or `rate` is zero.
    pub fn open(path: impl AsRef<Path>, channels: u16, rate: u32) -> Result<Self, CaptureError> {
        let file = File::open(path)?;
        Self::from_reader(BufReader::new(file), channels, rate)
    }

    /// Wraps any seekable reader of raw S24_LE data.
    ///
    /// # Errors
    /// [`CaptureError::Unsupported`] if `channels` or `rate` is zero.
    pub fn from_reader<R>(reader: R, channels: u16, rate: u32) -> Result<Self, CaptureError>
    where
        R: Read + Seek + Send + 'static,
    {
        if channels == 0 || rate == 0 {
            return Err(CaptureError::Unsupported(
                "channels and rate must be non-zero".into(),
            ));
        }
        Ok(Self {
            reader: Box::new(reader),
            channels,
            rate,
            format: SampleFormat::S24Le,
            realtime: false,
            looping: false,
            bytes: Vec::new(),
            frames_done: 0,
            started: None,
            pass_bytes: 0,
        })
    }

    /// Paces reads to the sample rate, like a live microphone.
    #[must_use]
    pub fn realtime(mut self, realtime: bool) -> Self {
        self.realtime = realtime;
        self
    }

    /// Restarts from the beginning at end of file instead of returning `Ok(0)`.
    #[must_use]
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    /// Sample format of the file (default S24_LE). Samples are returned in
    /// the S24 scale whatever the format ([`SampleFormat::to_s24`]).
    #[must_use]
    pub fn format(mut self, format: SampleFormat) -> Self {
        self.format = format;
        self
    }

    /// Seeks back to the start of the file and restarts real-time pacing.
    ///
    /// # Errors
    /// [`CaptureError::Io`] if the seek fails.
    pub fn rewind(&mut self) -> Result<(), CaptureError> {
        self.reader.seek(SeekFrom::Start(0))?;
        self.pass_bytes = 0;
        self.frames_done = 0;
        self.started = None;
        Ok(())
    }

    fn fill(&mut self, want: usize) -> io::Result<usize> {
        // Read up to `want` bytes (a whole number of frames), looping on short
        // reads; stops early only at EOF.
        self.bytes.resize(want, 0);
        let frame_bytes = usize::from(self.channels) * 4;
        let mut got = 0;
        while got < want {
            match self.reader.read(&mut self.bytes[got..want]) {
                Ok(0) => {
                    // EOF. Loop only if the file holds at least one whole
                    // frame, otherwise rewinding would spin forever.
                    if self.looping && self.pass_bytes >= frame_bytes as u64 {
                        self.reader.seek(SeekFrom::Start(0))?;
                        self.pass_bytes = 0;
                        // A frame cannot straddle the loop point.
                        got -= got % frame_bytes;
                    } else {
                        break;
                    }
                }
                Ok(n) => {
                    got += n;
                    self.pass_bytes += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(got)
    }
}

impl AudioSource for FileSource {
    fn read(&mut self, out: &mut [i32]) -> Result<usize, CaptureError> {
        let ch = usize::from(self.channels);
        let frames = out.len() / ch;
        if frames == 0 {
            return Ok(0);
        }
        if self.realtime {
            let start = *self.started.get_or_insert_with(Instant::now);
            // Block until the end of this chunk is due, as a real device would.
            let end_frames = self.frames_done + frames as u64;
            let due = Duration::from_micros(end_frames * 1_000_000 / u64::from(self.rate));
            if let Some(wait) = due.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let got = self.fill(frames * ch * 4)?;
        let whole = got / (ch * 4);
        for (dst, chunk) in out
            .iter_mut()
            .zip(self.bytes[..whole * ch * 4].chunks_exact(4))
        {
            *dst = self
                .format
                .to_s24(i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        self.frames_done += whole as u64;
        Ok(whole)
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn rate(&self) -> u32 {
        self.rate
    }
}

/// A mic file and an echo-reference file read in lockstep (PC tests of the
/// echo canceller with a recorded mic + reference pair).
///
/// Every mic read pulls the matching number of reference frames
/// (`mic frames * reference rate / mic rate`, carried exactly), so both
/// files advance on the same clock. A reference file shorter than the mic
/// is padded with silence. With looping on, the pair rewinds **both** files
/// when the mic file ends and bumps the reference epoch.
pub struct FilePair {
    mic: FileSource,
    reference: FileSource,
    looping: bool,
    mic_total: u64,
    ref_total: u64,
    pending: Vec<i32>,
    scratch: Vec<i32>,
    epoch: u64,
}

impl std::fmt::Debug for FilePair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilePair")
            .field("mic", &self.mic)
            .field("reference", &self.reference)
            .field("looping", &self.looping)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

impl FilePair {
    /// Pairs two sources. Their own looping flags are ignored (the pair
    /// loops both together when `looping` is set); real-time pacing should
    /// be off (the caller paces).
    #[must_use]
    pub fn new(mic: FileSource, reference: FileSource, looping: bool) -> Self {
        Self {
            mic: mic.looping(false),
            reference: reference.looping(false),
            looping,
            mic_total: 0,
            ref_total: 0,
            pending: Vec::new(),
            scratch: Vec::new(),
            epoch: 0,
        }
    }

    fn pull_reference(&mut self) -> Result<(), CaptureError> {
        let due = self.mic_total * u64::from(self.reference.rate) / u64::from(self.mic.rate);
        let want = usize::try_from(due.saturating_sub(self.ref_total)).unwrap_or(0);
        let ch = usize::from(self.reference.channels);
        self.scratch.resize(want * ch, 0);
        let mut got = 0;
        while got < want {
            let n = self
                .reference
                .read(&mut self.scratch[got * ch..want * ch])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        // Past the end of the reference file: silence.
        self.scratch[got * ch..].fill(0);
        self.pending.extend_from_slice(&self.scratch);
        self.ref_total = due;
        Ok(())
    }
}

impl AudioSource for FilePair {
    fn read(&mut self, out: &mut [i32]) -> Result<usize, CaptureError> {
        let mut n = self.mic.read(out)?;
        if n == 0
            && self.looping
            && self.mic_total > 0
            && out.len() >= usize::from(self.mic.channels)
        {
            self.mic.rewind()?;
            self.reference.rewind()?;
            self.mic_total = 0;
            self.ref_total = 0;
            self.pending.clear();
            self.epoch += 1;
            n = self.mic.read(out)?;
        }
        self.mic_total += n as u64;
        self.pull_reference()?;
        Ok(n)
    }

    fn channels(&self) -> u16 {
        self.mic.channels
    }

    fn rate(&self) -> u32 {
        self.mic.rate
    }

    fn reference_format(&self) -> Option<StreamFormat> {
        Some(StreamFormat {
            channels: self.reference.channels,
            rate: self.reference.rate,
        })
    }

    fn take_reference(&mut self, out: &mut Vec<i32>) {
        out.append(&mut self.pending);
    }

    fn reference_epoch(&self) -> u64 {
        self.epoch
    }

    fn reference_state(&self) -> ReferenceState {
        ReferenceState::Active {
            linked: true,
            unlinked_reason: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn bytes(samples: &[i32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn reads_frames_and_sign_extends() {
        // 0x00FFFFFF is -1 in S24 (zero-padded container), -0x800000 stays.
        let data = bytes(&[0x00FF_FFFF, 5, -0x80_0000, 0x7F_FFFF]);
        let mut src =
            FileSource::from_reader(Cursor::new(data), 2, 16_000).unwrap_or_else(|e| panic!("{e}"));
        let mut out = [0; 8];
        assert_eq!(src.read(&mut out).unwrap_or(99), 2);
        assert_eq!(out[..4], [-1, 5, -0x80_0000, 0x7F_FFFF]);
        assert_eq!(src.read(&mut out).unwrap_or(99), 0);
        assert_eq!((src.channels(), src.rate()), (2, 16_000));
    }

    #[test]
    fn short_final_read_and_partial_frame_dropped() {
        // 3 frames of stereo + 1 stray sample (+ 2 stray bytes).
        let mut data = bytes(&[1, 2, 3, 4, 5, 6, 7]);
        data.extend_from_slice(&[9, 9]);
        let mut src =
            FileSource::from_reader(Cursor::new(data), 2, 8_000).unwrap_or_else(|e| panic!("{e}"));
        let mut out = [0; 4];
        assert_eq!(src.read(&mut out).unwrap_or(99), 2);
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(src.read(&mut out).unwrap_or(99), 1);
        assert_eq!(out[..2], [5, 6]);
        assert_eq!(src.read(&mut out).unwrap_or(99), 0);
    }

    #[test]
    fn odd_output_len_leaves_partial_frame_untouched() {
        let data = bytes(&[1, 2, 3, 4]);
        let mut src =
            FileSource::from_reader(Cursor::new(data), 2, 8_000).unwrap_or_else(|e| panic!("{e}"));
        let mut out = [77; 3];
        assert_eq!(src.read(&mut out).unwrap_or(99), 1);
        assert_eq!(out, [1, 2, 77]);
    }

    #[test]
    fn looping_wraps_without_splitting_frames() {
        let data = bytes(&[1, 2, 3, 4]);
        let mut src = FileSource::from_reader(Cursor::new(data), 2, 8_000)
            .unwrap_or_else(|e| panic!("{e}"))
            .looping(true);
        let mut out = [0; 10];
        assert_eq!(src.read(&mut out).unwrap_or(99), 5);
        assert_eq!(out, [1, 2, 3, 4, 1, 2, 3, 4, 1, 2]);
    }

    #[test]
    fn looping_empty_file_ends() {
        let mut src = FileSource::from_reader(Cursor::new(Vec::new()), 1, 8_000)
            .unwrap_or_else(|e| panic!("{e}"))
            .looping(true);
        let mut out = [0; 4];
        assert_eq!(src.read(&mut out).unwrap_or(99), 0);
    }

    #[test]
    fn realtime_paces_reads() {
        let data = bytes(&vec![0; 800]);
        let mut src = FileSource::from_reader(Cursor::new(data), 1, 8_000)
            .unwrap_or_else(|e| panic!("{e}"))
            .realtime(true);
        let t = Instant::now();
        let mut out = [0; 400];
        assert_eq!(src.read(&mut out).unwrap_or(99), 400);
        assert_eq!(src.read(&mut out).unwrap_or(99), 400);
        // 800 frames at 8 kHz = 100 ms.
        let ms = t.elapsed().as_millis();
        assert!((90..400).contains(&ms), "{ms} ms");
    }

    #[test]
    fn looping_file_shorter_than_a_frame_ends() {
        let mut src = FileSource::from_reader(Cursor::new(vec![1, 2, 3, 4]), 2, 8_000)
            .unwrap_or_else(|e| panic!("{e}"))
            .looping(true);
        let mut out = [0; 4];
        assert_eq!(src.read(&mut out).unwrap_or(99), 0);
    }

    #[test]
    fn rejects_zero_channels() {
        assert!(FileSource::from_reader(Cursor::new(Vec::new()), 0, 8_000).is_err());
        assert!(FileSource::from_reader(Cursor::new(Vec::new()), 2, 0).is_err());
    }

    #[test]
    fn s32_file_is_scaled_to_s24() {
        let data = bytes(&[0x1234_5600, -0x100, i32::MAX, i32::MIN]);
        let mut src = FileSource::from_reader(Cursor::new(data), 2, 48_000)
            .unwrap_or_else(|e| panic!("{e}"))
            .format(SampleFormat::S32Le);
        let mut out = [0; 4];
        assert_eq!(src.read(&mut out).unwrap_or(99), 2);
        assert_eq!(out, [0x12_3456, -1, 0x7F_FFFF, -0x80_0000]);
    }

    #[test]
    fn rewind_restarts_the_file() {
        let mut src = FileSource::from_reader(Cursor::new(bytes(&[1, 2, 3])), 1, 8_000)
            .unwrap_or_else(|e| panic!("{e}"));
        let mut out = [0; 3];
        assert_eq!(src.read(&mut out).unwrap_or(99), 3);
        src.rewind().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(src.read(&mut out[..2]).unwrap_or(99), 2);
        assert_eq!(out[..2], [1, 2]);
    }

    fn pair(mic: &[i32], reference: &[i32], looping: bool) -> FilePair {
        let mic = FileSource::from_reader(Cursor::new(bytes(mic)), 1, 16_000)
            .unwrap_or_else(|e| panic!("{e}"));
        let reference = FileSource::from_reader(Cursor::new(bytes(reference)), 2, 48_000)
            .unwrap_or_else(|e| panic!("{e}"));
        FilePair::new(mic, reference, looping)
    }

    #[test]
    fn file_pair_reads_the_reference_in_lockstep() {
        // 4 mic frames at 16 kHz <-> 12 stereo reference frames at 48 kHz.
        let reference: Vec<i32> = (0..24).collect();
        let mut p = pair(&[10, 11, 12, 13], &reference, false);
        let mut out = [0; 3];
        let mut r = Vec::new();
        assert_eq!(p.read(&mut out).unwrap_or(99), 3);
        p.take_reference(&mut r);
        assert_eq!(r, (0..18).collect::<Vec<_>>());
        assert_eq!(p.read(&mut out).unwrap_or(99), 1);
        p.take_reference(&mut r);
        assert_eq!(r, (0..24).collect::<Vec<_>>());
        // End of both: no frames, no reference.
        assert_eq!(p.read(&mut out).unwrap_or(99), 0);
        let mut r2: Vec<i32> = Vec::new();
        p.take_reference(&mut r2);
        assert_eq!(r2.len(), 0);
        assert_eq!(
            p.reference_format(),
            Some(StreamFormat {
                channels: 2,
                rate: 48_000
            })
        );
        assert_eq!(p.reference_epoch(), 0);
    }

    #[test]
    fn file_pair_pads_a_short_reference_and_loops_together() {
        let mut p = pair(&[1, 2], &[7, 7, 7, 7], true);
        let mut out = [0; 2];
        let mut r = Vec::new();
        assert_eq!(p.read(&mut out).unwrap_or(99), 2);
        p.take_reference(&mut r);
        // 2 mic frames -> 6 stereo frames: 2 from the file, 4 of silence.
        assert_eq!(r, [7, 7, 7, 7, 0, 0, 0, 0, 0, 0, 0, 0]);
        r.clear();
        // Looping: the next read rewinds both and bumps the epoch.
        assert_eq!(p.read(&mut out[..1]).unwrap_or(99), 1);
        assert_eq!(out[0], 1);
        p.take_reference(&mut r);
        assert_eq!(r, [7, 7, 7, 7, 0, 0]);
        assert_eq!(p.reference_epoch(), 1);
    }

    #[test]
    fn missing_file_is_io_error() {
        assert!(matches!(
            FileSource::open("/nonexistent/definitely-not-here.raw", 2, 16_000),
            Err(CaptureError::Io(_))
        ));
    }
}
