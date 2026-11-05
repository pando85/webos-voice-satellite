//! What the music engine consumes: PCM chunks with their format.
//!
//! This is the contract between the decoder side (the streaming core, M3a)
//! and the engine (M4). The decoder runs on its own thread, probes the
//! stream header, and sends [`SourceItem`]s; the engine pulls them with
//! [`MusicSource::poll_chunk`] only when it needs more audio, so a bounded
//! channel between the two gives backpressure all the way to the socket.
//!
//! The rate and channel count travel with every [`Pcm`] chunk and the engine
//! opens its output from the **first** chunk, never from configuration. A
//! later chunk in another format ends the track with an error (a stream does
//! not change format; a new track is a new source).

use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::mpsc;

use crate::pcm::Pcm;

/// One step of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceItem {
    /// Interleaved s16 audio. Chunks may have any length (the decoder's are
    /// about 20 ms); the engine slices them itself.
    Pcm(Pcm),
    /// The stream finished normally.
    End,
    /// The stream failed (idle timeout, network error, decode error). The
    /// message must not contain the URL.
    Error(String),
}

/// A pull-style audio source, polled by the engine task.
///
/// `poll_chunk` must be **cancel safe** like any `poll` function: the engine
/// stops polling to enforce its pacing and may call it again later. It must
/// not block the thread; a decoder thread feeds it through a channel
/// ([`ChannelSource`]). Dropping the source cancels the stream, so a
/// decoder-backed source's `Drop` should stop its threads.
pub trait MusicSource: Send {
    /// The next item, or `Pending` (with the waker registered) when the
    /// decoder has nothing ready.
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<SourceItem>;
}

/// A source fed by a tokio channel: the decoder thread calls
/// `blocking_send` (a bounded channel gives backpressure), the engine polls.
///
/// A sender dropped without [`SourceItem::End`] counts as end of stream.
#[derive(Debug)]
pub struct ChannelSource {
    rx: mpsc::Receiver<SourceItem>,
}

impl ChannelSource {
    /// A bounded channel and its source.
    #[must_use]
    pub fn channel(capacity: usize) -> (mpsc::Sender<SourceItem>, Self) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (tx, Self { rx })
    }
}

impl MusicSource for ChannelSource {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<SourceItem> {
        self.rx.poll_recv(cx).map(|i| i.unwrap_or(SourceItem::End))
    }
}

/// What a [`ToneSource`] generates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    /// A sine of the given frequency.
    Sine(u32),
    /// A constant value (a DC test signal for gain and envelope tests).
    Dc,
    /// A counter `1, 2, 3, ...` wrapping at 30000, one value per frame (the
    /// same on every channel), to check that no sample is lost or repeated.
    Counter,
}

/// A generated source for tests and silent trials: never pending, so it
/// produces audio as fast as the engine asks and the engine's pacer decides
/// the speed.
#[derive(Debug, Clone)]
pub struct ToneSource {
    rate: u32,
    channels: u16,
    wave: Wave,
    amplitude: i16,
    /// Frames still to produce; `None` is endless.
    remaining: Option<u64>,
    chunk_frames: usize,
    produced: u64,
    ended: bool,
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // a test signal generator
impl ToneSource {
    /// A source of `wave` at `amplitude`, `duration` long (`None` for endless),
    /// in 20 ms chunks.
    #[must_use]
    pub fn new(
        rate: u32,
        channels: u16,
        wave: Wave,
        amplitude: i16,
        duration: Option<Duration>,
    ) -> Self {
        let frames = |d: Duration| (d.as_nanos() * u128::from(rate) / 1_000_000_000) as u64;
        Self {
            rate,
            channels,
            wave,
            amplitude,
            remaining: duration.map(frames),
            chunk_frames: (rate / 50).max(1) as usize,
            produced: 0,
            ended: false,
        }
    }

    /// Changes the chunk length in frames (test of odd chunk sizes).
    #[must_use]
    pub fn with_chunk_frames(mut self, frames: usize) -> Self {
        self.chunk_frames = frames.max(1);
        self
    }

    fn chunk(&mut self) -> Pcm {
        let want = self.chunk_frames as u64;
        let n = self.remaining.map_or(want, |r| r.min(want));
        let ch = usize::from(self.channels);
        let mut samples = Vec::with_capacity(n as usize * ch);
        for i in 0..n {
            let k = self.produced + i;
            let v = match self.wave {
                Wave::Dc => self.amplitude,
                Wave::Counter => i16::try_from(k % 30_000 + 1).unwrap_or(i16::MAX),
                Wave::Sine(f) => {
                    let phase = (k * u64::from(f) % u64::from(self.rate)) as f32 / self.rate as f32
                        * std::f32::consts::TAU;
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        (phase.sin() * f32::from(self.amplitude)) as i16
                    }
                }
            };
            for _ in 0..ch {
                samples.push(v);
            }
        }
        self.produced += n;
        if let Some(r) = &mut self.remaining {
            *r -= n;
        }
        Pcm {
            rate: self.rate,
            channels: self.channels,
            samples,
        }
    }
}

impl MusicSource for ToneSource {
    fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Poll<SourceItem> {
        if self.ended || self.remaining == Some(0) {
            self.ended = true;
            return Poll::Ready(SourceItem::End);
        }
        Poll::Ready(SourceItem::Pcm(self.chunk()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::task::Waker;

    fn pull(s: &mut impl MusicSource) -> SourceItem {
        let mut cx = Context::from_waker(Waker::noop());
        match s.poll_chunk(&mut cx) {
            Poll::Ready(i) => i,
            Poll::Pending => panic!("pending"),
        }
    }

    #[test]
    fn tone_source_ends_after_its_duration_in_whole_chunks() {
        let mut s = ToneSource::new(48_000, 2, Wave::Dc, 100, Some(Duration::from_millis(50)));
        let mut frames = 0;
        loop {
            match pull(&mut s) {
                SourceItem::Pcm(p) => {
                    assert_eq!((p.rate, p.channels), (48_000, 2));
                    frames += p.frames();
                }
                SourceItem::End => break,
                SourceItem::Error(e) => panic!("{e}"),
            }
        }
        assert_eq!(frames, 2400);
        assert_eq!(pull(&mut s), SourceItem::End, "stays ended");
    }

    #[test]
    fn counter_continues_across_chunks() {
        let mut s = ToneSource::new(8000, 1, Wave::Counter, 0, None).with_chunk_frames(3);
        let mut all = Vec::new();
        for _ in 0..3 {
            if let SourceItem::Pcm(p) = pull(&mut s) {
                all.extend(p.samples);
            }
        }
        assert_eq!(all, (1..=9).collect::<Vec<i16>>());
    }

    #[tokio::test]
    async fn channel_source_ends_when_the_sender_closes() {
        let (tx, mut s) = ChannelSource::channel(2);
        tx.send(SourceItem::Pcm(Pcm::silence(8000, 1, 4)))
            .await
            .unwrap_or(());
        drop(tx);
        let first = std::future::poll_fn(|cx| s.poll_chunk(cx)).await;
        assert!(matches!(first, SourceItem::Pcm(_)));
        assert_eq!(
            std::future::poll_fn(|cx| s.poll_chunk(cx)).await,
            SourceItem::End
        );
    }
}
