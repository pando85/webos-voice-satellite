//! The two bounded stages between the music threads.
//!
//! [`ByteRing`]: compressed bytes from `music-fetch` to `music-decode`.
//! [`PcmQueue`]: decoded chunks from `music-decode` to the consumer (the M4
//! engine, an async task). Both block the producer while full, so a slow
//! consumer stops the socket reads and TCP flow control pushes back on the
//! server; nothing grows with the length of the stream.

use std::collections::VecDeque;
use std::io;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use crate::error::PlaybackError;
use crate::pcm::Pcm;

/// Locks a mutex, ignoring poisoning (a panicking thread aborts in release;
/// in tests the state is still consistent because every critical section is
/// a few field updates).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(g).unwrap_or_else(PoisonError::into_inner)
}

/// How the fetch side ended.
#[derive(Debug)]
enum FetchEnd {
    Running,
    /// The body ended normally (length reached or connection closed).
    Complete,
    /// The fetch failed; the error is taken once by the decoder.
    Failed(Option<PlaybackError>),
}

struct RingState {
    buf: VecDeque<u8>,
    cap: usize,
    end: FetchEnd,
    stopped: bool,
}

/// Bounded byte FIFO from the fetch thread to the decode thread.
pub(crate) struct ByteRing {
    st: Mutex<RingState>,
    cv: Condvar,
}

impl ByteRing {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            st: Mutex::new(RingState {
                buf: VecDeque::with_capacity(cap.max(1)),
                cap: cap.max(1),
                end: FetchEnd::Running,
                stopped: false,
            }),
            cv: Condvar::new(),
        }
    }

    /// Appends all of `data`, waiting while the ring is full. Returns false
    /// when the ring was stopped (the decoder or the consumer gave up).
    pub(crate) fn write(&self, mut data: &[u8]) -> bool {
        let mut g = lock(&self.st);
        while !data.is_empty() {
            while g.buf.len() >= g.cap && !g.stopped {
                g = wait(&self.cv, g);
            }
            if g.stopped {
                return false;
            }
            let n = (g.cap - g.buf.len()).min(data.len());
            g.buf.extend(&data[..n]);
            data = &data[n..];
            self.cv.notify_all();
        }
        true
    }

    /// Marks the end of the body: `None` for a normal end, `Some` for a
    /// fetch error, reported by the decoder after the buffered bytes.
    pub(crate) fn finish(&self, err: Option<PlaybackError>) {
        let mut g = lock(&self.st);
        if matches!(g.end, FetchEnd::Running) {
            g.end = match err {
                None => FetchEnd::Complete,
                Some(e) => FetchEnd::Failed(Some(e)),
            };
        }
        self.cv.notify_all();
    }

    /// Wakes and fails both sides; buffered bytes are dropped.
    pub(crate) fn stop(&self) {
        let mut g = lock(&self.st);
        g.stopped = true;
        g.buf.clear();
        g.buf.shrink_to_fit();
        self.cv.notify_all();
    }

    /// Waits until there is at least one byte or the fetch has ended.
    pub(crate) fn wait_ready(&self) {
        let mut g = lock(&self.st);
        while g.buf.is_empty() && matches!(g.end, FetchEnd::Running) && !g.stopped {
            g = wait(&self.cv, g);
        }
    }

    /// The fetch error, once (later calls return `None`).
    pub(crate) fn take_error(&self) -> Option<PlaybackError> {
        match &mut lock(&self.st).end {
            FetchEnd::Failed(e) => e.take(),
            _ => None,
        }
    }

    /// True when the fetch ended with an error (taken or not).
    pub(crate) fn failed(&self) -> bool {
        matches!(lock(&self.st).end, FetchEnd::Failed(_))
    }

    /// Bytes buffered now.
    pub(crate) fn len(&self) -> usize {
        lock(&self.st).buf.len()
    }

    /// Blocking read for the decoder: data, `Ok(0)` at a normal end, or an
    /// error after a failed fetch or a stop.
    pub(crate) fn read(&self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut g = lock(&self.st);
        loop {
            if g.stopped {
                return Err(io::Error::other("stream stopped"));
            }
            if !g.buf.is_empty() {
                let n = g.buf.len().min(out.len());
                let (a, b) = g.buf.as_slices();
                let na = a.len().min(n);
                out[..na].copy_from_slice(&a[..na]);
                out[na..n].copy_from_slice(&b[..n - na]);
                g.buf.drain(..n);
                self.cv.notify_all();
                return Ok(n);
            }
            match g.end {
                FetchEnd::Running => g = wait(&self.cv, g),
                FetchEnd::Complete => return Ok(0),
                FetchEnd::Failed(_) => return Err(io::Error::other("fetch failed")),
            }
        }
    }
}

/// Result of a non-blocking [`PcmQueue::try_pop`].
pub(crate) enum Pop {
    /// A chunk, or the terminal error (delivered once).
    Item(Result<Pcm, PlaybackError>),
    /// Nothing yet; the stream is still running.
    Empty,
    /// The stream is over (normal end, after the error, or stopped).
    Closed,
}

enum QueueEnd {
    Running,
    Done,
    Failed(Option<PlaybackError>),
}

struct QueueState {
    items: VecDeque<Pcm>,
    samples: usize,
    cap_samples: usize,
    end: QueueEnd,
    stopped: bool,
    /// The async consumer waiting for data; registered under the same lock
    /// as the state it checked, so no wake-up is lost.
    waker: Option<Waker>,
}

/// Bounded queue of decoded chunks, counted in samples.
pub(crate) struct PcmQueue {
    st: Mutex<QueueState>,
    /// Producer waits here for space; blocking consumers wait for data.
    cv: Condvar,
}

impl PcmQueue {
    pub(crate) fn new() -> Self {
        Self {
            st: Mutex::new(QueueState {
                items: VecDeque::new(),
                samples: 0,
                cap_samples: 1,
                end: QueueEnd::Running,
                stopped: false,
                waker: None,
            }),
            cv: Condvar::new(),
        }
    }

    /// Sets the bound once the decoded format is known.
    pub(crate) fn set_capacity(&self, cap_samples: usize) {
        lock(&self.st).cap_samples = cap_samples.max(1);
    }

    /// Appends a chunk, waiting while the queue is full (one chunk is always
    /// accepted into an empty queue). Returns false when stopped.
    pub(crate) fn push(&self, pcm: Pcm) -> bool {
        let mut g = lock(&self.st);
        while !g.stopped && !g.items.is_empty() && g.samples + pcm.samples.len() > g.cap_samples {
            g = wait(&self.cv, g);
        }
        if g.stopped {
            return false;
        }
        g.samples += pcm.samples.len();
        g.items.push_back(pcm);
        let w = g.waker.take();
        drop(g);
        self.cv.notify_all();
        if let Some(w) = w {
            w.wake();
        }
        true
    }

    /// Ends the stream after the queued chunks: `None` normal, `Some` error.
    pub(crate) fn finish(&self, err: Option<PlaybackError>) {
        let mut g = lock(&self.st);
        if matches!(g.end, QueueEnd::Running) {
            g.end = match err {
                None => QueueEnd::Done,
                Some(e) => QueueEnd::Failed(Some(e)),
            };
        }
        let w = g.waker.take();
        drop(g);
        self.cv.notify_all();
        if let Some(w) = w {
            w.wake();
        }
    }

    /// Drops everything and wakes both sides; later pops return `Closed`.
    pub(crate) fn stop(&self) {
        let mut g = lock(&self.st);
        g.stopped = true;
        g.items.clear();
        g.samples = 0;
        let w = g.waker.take();
        drop(g);
        self.cv.notify_all();
        if let Some(w) = w {
            w.wake();
        }
    }

    pub(crate) fn try_pop(&self) -> Pop {
        let mut g = lock(&self.st);
        Self::pop_locked(&mut g, &self.cv)
    }

    fn pop_locked(g: &mut QueueState, cv: &Condvar) -> Pop {
        if g.stopped {
            return Pop::Closed;
        }
        if let Some(p) = g.items.pop_front() {
            g.samples -= p.samples.len();
            cv.notify_all();
            return Pop::Item(Ok(p));
        }
        match &mut g.end {
            QueueEnd::Running => Pop::Empty,
            QueueEnd::Done => Pop::Closed,
            QueueEnd::Failed(e) => match e.take() {
                Some(e) => Pop::Item(Err(e)),
                None => Pop::Closed,
            },
        }
    }

    /// Async pop: `Pending` with the waker registered when empty.
    pub(crate) fn poll_pop(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Pcm, PlaybackError>>> {
        let mut g = lock(&self.st);
        match Self::pop_locked(&mut g, &self.cv) {
            Pop::Item(x) => Poll::Ready(Some(x)),
            Pop::Closed => Poll::Ready(None),
            Pop::Empty => {
                if !g.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                    g.waker = Some(cx.waker().clone());
                }
                Poll::Pending
            }
        }
    }

    /// Blocking pop (for threads and tests, never inside an async task).
    pub(crate) fn pop_blocking(&self) -> Option<Result<Pcm, PlaybackError>> {
        let mut g = lock(&self.st);
        loop {
            match Self::pop_locked(&mut g, &self.cv) {
                Pop::Item(x) => return Some(x),
                Pop::Closed => return None,
                Pop::Empty => g = wait(&self.cv, g),
            }
        }
    }

    /// Decoded frames buffered now, given the channel count.
    pub(crate) fn queued_samples(&self) -> usize {
        lock(&self.st).samples
    }
}
