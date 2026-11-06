//! Engine tests. Most run on tokio's paused clock with a recording output or
//! the streaming WAV output, so the timing numbers are exact and a test of
//! several seconds takes milliseconds. Two use a fake `pacat` script in real
//! time.

#![allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::unchecked_time_subtraction,
    clippy::too_many_lines
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::sleep;

use super::output::recording::{RecordingFactory, Shared as LogHandle};
use super::*;
use crate::pcm::Pcm;

const MS: fn(u64) -> Duration = Duration::from_millis;

type Events = UnboundedReceiver<MusicEvent>;

fn engine_with(cfg: MusicConfig, volume: f32) -> (MusicEngine, Events, LogHandle) {
    let (factory, log) = RecordingFactory::new();
    let (e, ev) = MusicEngine::spawn(cfg, Box::new(factory), MusicVolume::new(volume));
    (e, ev, log)
}

fn tone(rate: u32, ch: u16, wave: Wave, amp: i16, ms: u64) -> Box<dyn MusicSource> {
    Box::new(ToneSource::new(rate, ch, wave, amp, Some(MS(ms))))
}

fn endless(rate: u32, ch: u16, wave: Wave, amp: i16) -> Box<dyn MusicSource> {
    Box::new(ToneSource::new(rate, ch, wave, amp, None))
}

async fn next_end(ev: &mut Events) -> (TrackId, EndReason) {
    loop {
        match ev.recv().await {
            Some(MusicEvent::Ended { id, reason }) => return (id, reason),
            Some(MusicEvent::Started { .. }) => {}
            None => panic!("engine ended without an Ended event"),
        }
    }
}

fn samples_of(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// Audio time of every write, as (time of the write, frames in it).
fn writes_of(log: &LogHandle, open: usize, ch: usize) -> Vec<(Duration, u64)> {
    log.lock().unwrap().opens[open]
        .writes
        .iter()
        .map(|&(t, b)| (t, (b / 2 / ch) as u64))
        .collect()
}

/// Largest lead of written audio over the clock that started at the first
/// write, over all writes.
fn max_ahead(writes: &[(Duration, u64)], rate: u32) -> Duration {
    let t0 = writes[0].0;
    let mut cum = 0u64;
    let mut worst = Duration::ZERO;
    for &(t, n) in writes {
        cum += n;
        let audio = frames_to_dur(cum, rate);
        worst = worst.max(audio.saturating_sub(t - t0));
    }
    worst
}

// ------------------------------------------------------------- format, gain

#[tokio::test(start_paused = true)]
async fn the_output_follows_the_decoded_format_of_each_track() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    e.play(tone(44_100, 1, Wave::Sine(440), 10_000, 100));
    assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);
    e.play(tone(48_000, 2, Wave::Sine(440), 10_000, 100));
    assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);
    e.play(tone(22_050, 1, Wave::Sine(440), 10_000, 100));
    assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);

    let l = log.lock().unwrap();
    let fmts: Vec<_> = l.opens.iter().map(|o| o.format.unwrap()).collect();
    assert_eq!(
        fmts,
        vec![
            SinkFormat {
                rate_hz: 44_100,
                channels: 1
            },
            SinkFormat {
                rate_hz: 48_000,
                channels: 2
            },
            SinkFormat {
                rate_hz: 22_050,
                channels: 1
            },
        ]
    );
    assert!(l.opens.iter().all(|o| o.finished && !o.aborted));
    // 100 ms of each, nothing lost.
    assert_eq!(l.opens[0].data.len(), 4410 * 2);
    assert_eq!(l.opens[1].data.len(), 4800 * 2 * 2);
    assert_eq!(l.opens[2].data.len(), 2205 * 2);
    assert_eq!(e.stats().tracks, 3);
    drop(l);
    assert_eq!(e.state(), MusicState::Idle);
}

#[tokio::test(start_paused = true)]
async fn started_event_carries_the_stream_format() {
    let (e, mut ev, _log) = engine_with(MusicConfig::default(), 1.0);
    let id = e.play(tone(32_000, 2, Wave::Dc, 1, 50));
    assert_eq!(
        ev.recv().await,
        Some(MusicEvent::Started {
            id,
            rate: 32_000,
            channels: 2
        })
    );
}

#[tokio::test(start_paused = true)]
async fn written_samples_are_input_times_volume_times_trim() {
    // DC 10000, volume 0.5, trim -6.02 dB (0.5): 2500 exactly in Q15.
    let cfg = MusicConfig {
        gain_db: -6.0206,
        ..MusicConfig::default()
    };
    let (e, mut ev, log) = engine_with(cfg, 0.5);
    e.play(tone(48_000, 2, Wave::Dc, 10_000, 200));
    next_end(&mut ev).await;
    let data = samples_of(&log.lock().unwrap().opens[0].data);
    assert_eq!(data.len(), 9600 * 2);
    assert!(
        data.iter().all(|&s| (2499..=2501).contains(&s)),
        "{:?}",
        &data[..4]
    );
}

#[tokio::test(start_paused = true)]
async fn volume_zero_writes_only_zeros() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 0.0);
    e.play(tone(48_000, 2, Wave::Sine(440), 20_000, 200));
    next_end(&mut ev).await;
    let data = samples_of(&log.lock().unwrap().opens[0].data);
    assert_eq!(data.len(), 9600 * 2);
    assert!(data.iter().all(|&s| s == 0));
}

#[tokio::test(start_paused = true)]
async fn a_volume_change_ramps_per_sample() {
    let vol = MusicVolume::new(1.0);
    let (factory, log) = RecordingFactory::new();
    let (e, mut ev) = MusicEngine::spawn(MusicConfig::default(), Box::new(factory), vol.clone());
    e.play(tone(48_000, 1, Wave::Dc, 10_000, 1000));
    sleep(MS(300)).await;
    vol.set(0.5);
    next_end(&mut ev).await;
    let data = samples_of(&log.lock().unwrap().opens[0].data);
    assert_eq!(data.len(), 48_000);
    assert_eq!(data[0], 10_000);
    assert_eq!(data[47_999], 5000);
    let max_step = data
        .windows(2)
        .map(|w| (i32::from(w[1]) - i32::from(w[0])).abs())
        .max()
        .unwrap();
    // 30 ms ramp of 16384 Q15 over 1440 frames: 3.5 per sample at 10000.
    assert!(max_step <= 5, "max step {max_step}");
    assert!(data.windows(2).all(|w| w[1] <= w[0]), "monotonic");
}

// ----------------------------------------------------------------- pacing

#[tokio::test(start_paused = true)]
async fn writes_never_run_more_than_the_write_ahead_ahead_of_the_clock() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    e.play(tone(48_000, 2, Wave::Dc, 1000, 3000));
    next_end(&mut ev).await;
    let w = writes_of(&log, 0, 2);
    let ahead = max_ahead(&w, 48_000);
    println!("max write-ahead {ahead:?} over {} writes", w.len());
    assert!(ahead <= MS(60) + Duration::from_micros(25), "{ahead:?}");
    assert!(
        ahead >= MS(50),
        "the pacer should use its budget: {ahead:?}"
    );
    // The whole 3 s was written, in 10 ms slices, in about 3 s minus the lead.
    let total: u64 = w.iter().map(|x| x.1).sum();
    assert_eq!(total, 144_000);
    let span = w.last().unwrap().0 - w[0].0;
    assert!(
        span >= MS(2900) && span <= MS(2950),
        "writes spread over {span:?}"
    );
    assert!(w.iter().all(|x| x.1 == 480), "10 ms slices");
}

#[tokio::test(start_paused = true)]
async fn a_late_source_gets_bounded_silence_and_no_catch_up_burst() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    let (tx, src) = ChannelSource::channel(4);
    e.play(Box::new(src));
    let chunk = || {
        SourceItem::Pcm(Pcm {
            rate: 48_000,
            channels: 2,
            samples: vec![1000; 960 * 2],
        })
    };
    for _ in 0..5 {
        tx.send(chunk()).await.unwrap();
    }
    // The source goes quiet for two seconds.
    sleep(MS(2000)).await;
    let resumed_at = log.lock().unwrap().opens[0].writes.last().unwrap().0;
    for _ in 0..5 {
        tx.send(chunk()).await.unwrap();
    }
    tx.send(SourceItem::End).await.unwrap();
    next_end(&mut ev).await;

    let s = e.stats();
    assert_eq!(s.underruns, 1, "one underrun: {s:?}");
    assert!(s.clock_resets >= 1, "{s:?}");
    // Silence is bounded by max_silence (200 ms = 9600 frames), not by the
    // 2 s gap, and is within one slice of the cap.
    assert!(
        (9600..=9600 + 480).contains(&s.silence_frames),
        "silence {} frames",
        s.silence_frames
    );
    // Nothing was written during the rest of the gap.
    let w = log.lock().unwrap().opens[0].writes.clone();
    let quiet_start = w
        .iter()
        .map(|x| x.0)
        .filter(|t| *t < resumed_at + MS(1))
        .max()
        .unwrap();
    assert!(
        quiet_start < MS(500),
        "silence stopped early: {quiet_start:?}"
    );
    // The burst after the gap is the new audio at most 60 ms ahead, not the
    // 2 s the clock would owe.
    let post: Vec<_> = w.iter().filter(|x| x.0 >= MS(2000)).collect();
    let t_first = post[0].0;
    let burst: usize = post
        .iter()
        .filter(|x| x.0 <= t_first + MS(1))
        .map(|x| x.1 / 2 / 2)
        .sum();
    assert!(burst <= 7 * 480, "burst of {burst} frames after the gap");
}

#[tokio::test(start_paused = true)]
async fn a_blocked_write_resets_the_clock_instead_of_flooding() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    log.lock().unwrap().stall = Some((30, MS(1500)));
    e.play(tone(48_000, 2, Wave::Dc, 1000, 4000));
    next_end(&mut ev).await;
    let s = e.stats();
    assert!(s.clock_resets >= 1, "{s:?}");
    assert_eq!(s.underruns, 0, "the source was never late");
    let w = log.lock().unwrap().opens[0].writes.clone();
    // The stalled write is #30; the burst that follows is at most the
    // write-ahead (plus the stalled slice itself).
    let t_done = w[30].0;
    assert!(t_done >= MS(1500));
    let burst: usize = w
        .iter()
        .filter(|x| x.0 >= t_done && x.0 <= t_done + MS(1))
        .map(|x| x.1 / 2 / 2)
        .sum();
    assert!(
        burst <= 7 * 480,
        "burst of {burst} frames after a blocked write"
    );
    // After the reset the lead is bounded again.
    let after: Vec<(Duration, u64)> = w
        .iter()
        .skip(30)
        .map(|&(t, b)| (t, (b / 4) as u64))
        .collect();
    assert!(max_ahead(&after, 48_000) <= MS(71));
}

// ----------------------------------------------------------- pause, stop

#[tokio::test(start_paused = true)]
async fn pause_writes_nothing_and_resume_loses_and_repeats_nothing() {
    let cfg = MusicConfig {
        fade: Duration::ZERO,
        ..MusicConfig::default()
    };
    let (e, mut ev, log) = engine_with(cfg, 1.0);
    e.play(tone(48_000, 1, Wave::Counter, 0, 1000));
    sleep(MS(300)).await;
    e.pause();
    sleep(MS(10)).await;
    assert_eq!(e.state(), MusicState::Paused);
    let n_at_pause = log.lock().unwrap().opens[0].writes.len();
    sleep(MS(5000)).await;
    assert_eq!(
        log.lock().unwrap().opens[0].writes.len(),
        n_at_pause,
        "no writes while paused"
    );
    e.resume();
    sleep(MS(1)).await;
    assert_eq!(e.state(), MusicState::Playing);
    assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);

    let data = samples_of(&log.lock().unwrap().opens[0].data);
    let expected: Vec<i16> = (0..48_000u32)
        .map(|k| i16::try_from(k % 30_000 + 1).unwrap())
        .collect();
    assert_eq!(data, expected, "every sample exactly once, in order");
    // No burst after the long pause: the first slices after resume stay
    // within the write-ahead.
    let w = writes_of(&log, 0, 1);
    let t_res = w[n_at_pause].0;
    let burst: u64 = w
        .iter()
        .filter(|x| x.0 >= t_res && x.0 <= t_res + MS(1))
        .map(|x| x.1)
        .sum();
    assert!(burst <= 7 * 480, "burst of {burst} frames after resume");
}

#[tokio::test(start_paused = true)]
async fn pause_and_resume_fade_through_zero() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    e.play(tone(48_000, 1, Wave::Dc, 10_000, 1000));
    sleep(MS(300)).await;
    e.pause();
    sleep(MS(2000)).await;
    let n = log.lock().unwrap().opens[0].data.len() / 2;
    let before = samples_of(&log.lock().unwrap().opens[0].data);
    assert!(
        before[n - 1].abs() <= 100,
        "fade-out ends near zero: {}",
        before[n - 1]
    );
    assert!(before[n - 1000..n].windows(2).all(|w| w[1] <= w[0]));
    e.resume();
    next_end(&mut ev).await;
    let data = samples_of(&log.lock().unwrap().opens[0].data);
    assert!(
        data[n].abs() <= 100,
        "fade-in starts near zero: {}",
        data[n]
    );
    assert_eq!(*data.last().unwrap(), 10_000);
    let max_step = data
        .windows(2)
        .map(|w| (i32::from(w[1]) - i32::from(w[0])).abs())
        .max()
        .unwrap();
    // 10 ms fade of 10000: 21 per sample, no jump.
    assert!(max_step <= 25, "max step {max_step}");
}

#[tokio::test(start_paused = true)]
async fn a_long_pause_ends_the_track() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    e.play(endless(48_000, 2, Wave::Dc, 100));
    sleep(MS(200)).await;
    e.pause();
    sleep(MS(599_000)).await;
    assert_eq!(e.state(), MusicState::Paused);
    sleep(MS(2000)).await;
    assert_eq!(next_end(&mut ev).await.1, EndReason::PauseTimeout);
    assert_eq!(e.state(), MusicState::Idle);
    assert!(log.lock().unwrap().opens[0].aborted);
}

struct DropFlag(Arc<AtomicBool>, ToneSource);

impl MusicSource for DropFlag {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<SourceItem> {
        self.1.poll_chunk(cx)
    }
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn stop_kills_the_output_drops_the_source_and_goes_idle() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    let dropped = Arc::new(AtomicBool::new(false));
    e.play(Box::new(DropFlag(
        Arc::clone(&dropped),
        ToneSource::new(48_000, 2, Wave::Dc, 100, None),
    )));
    sleep(MS(500)).await;
    assert_eq!(e.state(), MusicState::Playing);
    e.stop();
    assert_eq!(next_end(&mut ev).await.1, EndReason::Stopped);
    assert_eq!(e.state(), MusicState::Idle);
    assert!(dropped.load(Ordering::SeqCst), "the source must be dropped");
    {
        let l = log.lock().unwrap();
        assert!(l.opens[0].aborted && !l.opens[0].finished);
    }
    let n = log.lock().unwrap().opens[0].writes.len();
    sleep(MS(1000)).await;
    assert_eq!(
        log.lock().unwrap().opens[0].writes.len(),
        n,
        "no writes after stop"
    );
    // Stop and pause while idle are ignored.
    e.stop();
    e.pause();
    sleep(MS(10)).await;
    assert_eq!(e.state(), MusicState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_new_track_replaces_the_current_one() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    let a = e.play(endless(44_100, 2, Wave::Dc, 100));
    sleep(MS(300)).await;
    let b = e.play(tone(48_000, 1, Wave::Dc, 100, 100));
    assert_eq!(next_end(&mut ev).await, (a, EndReason::Replaced));
    assert_eq!(next_end(&mut ev).await, (b, EndReason::Finished));
    let l = log.lock().unwrap();
    assert!(l.opens[0].aborted);
    assert_eq!(l.opens[1].format.unwrap().rate_hz, 48_000);
    assert!(l.opens[1].finished);
}

#[tokio::test(start_paused = true)]
async fn a_format_change_inside_a_stream_ends_it_with_an_error() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    let (tx, src) = ChannelSource::channel(4);
    e.play(Box::new(src));
    tx.send(SourceItem::Pcm(Pcm::silence(44_100, 2, 441)))
        .await
        .unwrap();
    tx.send(SourceItem::Pcm(Pcm::silence(48_000, 2, 480)))
        .await
        .unwrap();
    let (_, reason) = next_end(&mut ev).await;
    assert!(
        matches!(&reason, EndReason::Error(m) if m.contains("format")),
        "{reason:?}"
    );
    let l = log.lock().unwrap();
    assert_eq!(l.opens.len(), 1, "no second output for the same track");
    assert!(l.opens[0].finished, "what was written plays out");
}

#[tokio::test(start_paused = true)]
async fn source_errors_and_open_failures_end_the_track() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    let (tx, src) = ChannelSource::channel(4);
    e.play(Box::new(src));
    tx.send(SourceItem::Pcm(Pcm::silence(48_000, 2, 480)))
        .await
        .unwrap();
    tx.send(SourceItem::Error("stream went idle".to_owned()))
        .await
        .unwrap();
    assert_eq!(
        next_end(&mut ev).await.1,
        EndReason::Error("stream went idle".to_owned())
    );
    assert_eq!(e.state(), MusicState::Idle);

    log.lock().unwrap().fail_open = true;
    e.play(tone(48_000, 2, Wave::Dc, 1, 100));
    let (_, r) = next_end(&mut ev).await;
    assert!(matches!(r, EndReason::Error(m) if m.contains("cannot open")));
    assert_eq!(e.state(), MusicState::Idle);

    // Unsupported formats are refused before an output is opened.
    log.lock().unwrap().fail_open = false;
    let (tx, src) = ChannelSource::channel(1);
    e.play(Box::new(src));
    tx.send(SourceItem::Pcm(Pcm {
        rate: 48_000,
        channels: 0,
        samples: vec![],
    }))
    .await
    .unwrap();
    let (_, r) = next_end(&mut ev).await;
    assert!(matches!(r, EndReason::Error(m) if m.contains("unsupported")));
}

// ------------------------------------------------------------------- duck

fn wav_engine(
    cfg: MusicConfig,
    dir_tag: &str,
) -> (
    MusicEngine,
    Events,
    Arc<Mutex<Vec<std::path::PathBuf>>>,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("webos-music-{dir_tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let factory = WavStreamFactory::new(&dir, "music");
    let paths = factory.paths();
    let (e, ev) = MusicEngine::spawn(cfg, Box::new(factory), MusicVolume::new(1.0));
    (e, ev, paths, dir)
}

fn first_index(data: &[i16], from: usize, pred: impl Fn(i16) -> bool) -> usize {
    from + data[from..].iter().position(|&s| pred(s)).unwrap()
}

#[tokio::test(start_paused = true)]
async fn duck_attack_hold_and_release_on_the_wav_sink() {
    let (e, mut ev, paths, dir) = wav_engine(MusicConfig::default(), "duck");
    e.play(tone(48_000, 2, Wave::Dc, 16_000, 5000));
    sleep(MS(1000)).await;

    let f0 = e.stats().frames_written as usize;
    e.duck_begin(DuckSource::WakeWord);
    sleep(MS(1500)).await;
    let f2 = e.stats().frames_written as usize;
    e.duck_end(DuckSource::WakeWord);
    assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);

    let path = paths.lock().unwrap()[0].clone();
    let (fmt, raw) = read_wav(&path).unwrap();
    assert_eq!(
        fmt,
        SinkFormat {
            rate_hz: 48_000,
            channels: 2
        }
    );
    let left: Vec<i16> = raw.chunks(2).map(|f| f[0]).collect();
    assert_eq!(left.len(), 240_000);
    assert!(raw.chunks(2).all(|f| f[0] == f[1]), "both channels equal");

    let slice = 480usize;
    let attack = 2400usize;
    // Before the duck: unity, bit exact.
    assert!(left[..f0].iter().all(|&s| s == 16_000));
    // The ramp starts within one slice of what was already written, and is
    // at -20 dB (1600) within one slice plus the attack.
    let started = first_index(&left, f0, |s| s < 16_000);
    let reached = first_index(&left, f0, |s| s <= 1_634);
    assert!(
        started - f0 <= slice,
        "duck starts {} frames late",
        started - f0
    );
    assert!(
        reached - f0 <= slice + attack + 2,
        "-20 dB reached {} frames after the call",
        reached - f0
    );
    // The duck, as heard: the 60 ms already in the pipeline plus the attack.
    let ahead_at_call = frames_to_dur(f0 as u64, 48_000).saturating_sub(MS(1000));
    let lag = ahead_at_call + frames_to_dur((reached - f0) as u64, 48_000);
    println!(
        "duck: written {ahead_at_call:?} ahead at the call; ramp starts {} frames later, \
         reaches -20 dB {} frames ({:?}) later; engine-side lag {lag:?} \
         (plus pacat latency 100 ms = {:?} against the 400 ms threshold)",
        started - f0,
        reached - f0,
        frames_to_dur((reached - f0) as u64, 48_000),
        lag + MS(100)
    );
    assert!(lag + MS(100) <= MS(400));
    // Held at the duck level until the end of the hold.
    let hold = 38_400usize;
    let settled = first_index(&left, f0, |s| s <= 1_602);
    assert!(settled - f0 <= slice + attack + 2);
    assert!(
        left[settled..f2 + hold - slice]
            .iter()
            .all(|&s| (1_598..=1_602).contains(&s))
    );
    // The release starts after the hold and ends at unity within the release
    // time plus a slice.
    let rel_start = first_index(&left, settled, |s| s > 1_602);
    assert!(
        rel_start >= f2 + hold - slice,
        "released early at {rel_start}"
    );
    assert!(
        rel_start <= f2 + hold + slice + 2,
        "released late at {rel_start}"
    );
    let back = first_index(&left, rel_start, |s| s == 16_000);
    assert!(
        back - rel_start <= 48_000 + 2,
        "release took {} frames",
        back - rel_start
    );
    assert!(left[back..].iter().all(|&s| s == 16_000));
    println!(
        "release starts {} frames after duck_end + 800 ms (written position), takes {} frames",
        rel_start as i64 - (f2 + hold) as i64,
        back - rel_start
    );
    // No zipper: the largest per-sample step over the whole file.
    let max_step = left
        .windows(2)
        .map(|w| (i32::from(w[1]) - i32::from(w[0])).abs())
        .max()
        .unwrap();
    println!("max per-sample step across attack and release: {max_step}");
    // Attack slope 14400 / 2400 = 6 per sample.
    assert!(max_step <= 7, "max step {max_step}");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(start_paused = true)]
async fn overlapping_duck_sources_release_after_the_last_one() {
    let (e, mut ev, paths, dir) = wav_engine(MusicConfig::default(), "refcount");
    e.play(tone(48_000, 2, Wave::Dc, 16_000, 6000));
    sleep(MS(500)).await;
    e.duck_begin(DuckSource::WakeWord);
    sleep(MS(500)).await;
    e.duck_begin(DuckSource::Reply);
    sleep(MS(500)).await;
    e.duck_end(DuckSource::WakeWord); // the run ends, the reply goes on
    sleep(MS(2000)).await; // far more than hold + release
    let f_before_end = e.stats().frames_written as usize;
    e.duck_end(DuckSource::Reply);
    assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);

    let (_, raw) = read_wav(&paths.lock().unwrap()[0]).unwrap();
    let left: Vec<i16> = raw.chunks(2).map(|f| f[0]).collect();
    // Ducked from just after 500 ms until the reply ended: the first end did
    // not release it.
    let ducked_from = first_index(&left, 24_000, |s| s <= 1_602);
    assert!(
        left[ducked_from..f_before_end]
            .iter()
            .all(|&s| (1_598..=1_602).contains(&s)),
        "released while the reply was still active"
    );
    // After the reply: hold, then back to unity.
    let end_idx = f_before_end;
    let back = first_index(&left, end_idx, |s| s == 16_000);
    assert!(back >= end_idx + 38_400, "back at {back} (end {end_idx})");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(start_paused = true)]
async fn mode_none_leaves_the_music_alone() {
    let cfg = MusicConfig {
        duck: DuckConfig {
            mode: DuckMode::None,
            ..DuckConfig::default()
        },
        ..MusicConfig::default()
    };
    let (e, mut ev, log) = engine_with(cfg, 1.0);
    e.play(tone(48_000, 1, Wave::Dc, 5000, 1000));
    sleep(MS(200)).await;
    e.duck_begin(DuckSource::WakeWord);
    next_end(&mut ev).await;
    let data = samples_of(&log.lock().unwrap().opens[0].data);
    assert!(data.iter().all(|&s| s == 5000));
}

#[tokio::test(start_paused = true)]
async fn mode_pause_pauses_for_the_sources_and_resumes_after_the_hold() {
    let cfg = MusicConfig {
        fade: Duration::ZERO,
        duck: DuckConfig {
            mode: DuckMode::Pause,
            ..DuckConfig::default()
        },
        ..MusicConfig::default()
    };
    let (e, mut ev, log) = engine_with(cfg, 1.0);
    e.play(tone(48_000, 1, Wave::Counter, 0, 2000));
    sleep(MS(300)).await;
    e.duck_begin(DuckSource::Reply);
    sleep(MS(20)).await;
    let n = log.lock().unwrap().opens[0].writes.len();
    sleep(MS(1000)).await;
    assert_eq!(
        log.lock().unwrap().opens[0].writes.len(),
        n,
        "paused while the reply plays"
    );
    assert_eq!(
        e.state(),
        MusicState::Playing,
        "a duck pause is not the user's pause"
    );
    e.duck_end(DuckSource::Reply);
    sleep(MS(700)).await;
    assert_eq!(log.lock().unwrap().opens[0].writes.len(), n, "held 800 ms");
    sleep(MS(200)).await;
    assert!(
        log.lock().unwrap().opens[0].writes.len() > n,
        "resumed after the hold"
    );
    assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);
    let data = samples_of(&log.lock().unwrap().opens[0].data);
    let expected: Vec<i16> = (0..96_000u32)
        .map(|k| i16::try_from(k % 30_000 + 1).unwrap())
        .collect();
    assert_eq!(data, expected);
}

#[tokio::test(start_paused = true)]
async fn a_track_started_during_a_duck_starts_ducked() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    e.duck_begin(DuckSource::Alarm);
    e.play(tone(48_000, 1, Wave::Dc, 10_000, 300));
    next_end(&mut ev).await;
    let data = samples_of(&log.lock().unwrap().opens[0].data);
    assert!(
        data.iter().all(|&s| (999..=1001).contains(&s)),
        "{}",
        data[0]
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_ends_the_track_and_the_task() {
    let (e, mut ev, log) = engine_with(MusicConfig::default(), 1.0);
    e.play(endless(48_000, 2, Wave::Dc, 100));
    sleep(MS(200)).await;
    e.shutdown().await;
    assert_eq!(next_end(&mut ev).await.1, EndReason::Stopped);
    assert!(log.lock().unwrap().opens[0].aborted);
}

// ------------------------------------------------- real pacat child process

#[cfg(unix)]
mod fake_pacat {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(tag: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("webos-music-pacat-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("fake-pacat");
        std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, bin)
    }

    #[tokio::test]
    async fn pacat_is_opened_from_the_decoded_format_and_gets_the_audio() {
        let (dir, bin) = script(
            "fmt",
            &format!(
                "echo \"$@\" > {0}/args\ncat > {0}/out",
                "\"$(dirname \"$0\")\""
            ),
        );
        let f = PacatFactory::new(PacatConfig {
            binary: bin,
            ..PacatConfig::default()
        });
        let (e, mut ev) =
            MusicEngine::spawn(MusicConfig::default(), Box::new(f), MusicVolume::new(1.0));
        e.play(tone(22_050, 1, Wave::Sine(440), 8000, 300));
        assert_eq!(next_end(&mut ev).await.1, EndReason::Finished);
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        for want in [
            "--rate=22050",
            "--channels=1",
            "--format=s16le",
            "--latency-msec=100",
            "--stream-name=music",
        ] {
            assert!(args.contains(want), "{args}");
        }
        let out = std::fs::read(dir.join("out")).unwrap();
        assert_eq!(out.len(), 6615 * 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn stop_leaves_no_pacat_behind() {
        let (dir, bin) = script(
            "stop",
            "echo $$ > \"$(dirname \"$0\")/pid\"\nexec cat > /dev/null",
        );
        let f = PacatFactory::new(PacatConfig {
            binary: bin,
            ..PacatConfig::default()
        });
        let (e, mut ev) =
            MusicEngine::spawn(MusicConfig::default(), Box::new(f), MusicVolume::new(1.0));
        e.play(endless(48_000, 2, Wave::Dc, 100));
        // Wait for the fake pacat to start and write its pid (about 300 ms
        // when idle, longer under load; the file may be seen half written).
        let mut waited = 0;
        let pid: u32 = loop {
            tokio::time::sleep(MS(50)).await;
            waited += 50;
            if let Some(p) = std::fs::read_to_string(dir.join("pid"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                break p;
            }
            assert!(waited < 20_000, "the fake pacat never started");
        };
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
        e.stop();
        assert_eq!(next_end(&mut ev).await.1, EndReason::Stopped);
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "pacat {pid} still running after stop"
        );
        e.shutdown().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_missing_pacat_ends_the_track_with_an_error() {
        let f = PacatFactory::new(PacatConfig {
            binary: "/nonexistent/pacat".into(),
            ..PacatConfig::default()
        });
        let (e, mut ev) =
            MusicEngine::spawn(MusicConfig::default(), Box::new(f), MusicVolume::new(1.0));
        e.play(tone(48_000, 2, Wave::Dc, 1, 100));
        let (_, r) = next_end(&mut ev).await;
        assert!(matches!(r, EndReason::Error(m) if m.contains("cannot open")));
    }
}
