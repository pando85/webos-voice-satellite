//! The timer alarm: what plays when one of HA's voice timers finishes.
//!
//! Three parts, all independent of audio output and I/O:
//!
//! - [`Alarm`], a small state machine (idle, queued, ringing) with a time
//!   cap. The daemon feeds it HA's timer events, the wake word, the stop
//!   button and the player's state; it answers with [`Step`]s.
//! - [`chime_cycle`], a built-in two-tone chime generated in code (no audio
//!   asset), which the player repeats while the alarm rings.
//! - [`timers_text`], the text of the "Timers" sensor.
//!
//! HA owns timers: on `FINISHED` it has already dropped the timer
//! (`intent/timers.py::_timer_finished` pops it before calling the device's
//! handler), so a finished timer cannot be cancelled in HA, and HA has no
//! "stop ringing" message. Stopping the sound is the device's job: the wake
//! word, the "Stop timer alarm" button or the media player's stop.

// Sample synthesis: counts and times here are small and bounded (a few
// seconds of 16 kHz audio), so the numeric casts cannot lose anything that
// matters.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::BTreeSet;
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use webos_esphome_api::proto::EntityCategory;
use webos_esphome_api::{BinarySensorEntity, ButtonEntity, TextSensorEntity, TimerTable};

/// Object id of the "Stop timer alarm" button.
pub(crate) const STOP_BUTTON_ID: &str = "stop_timer_alarm";
/// Object id of the "Timers" text sensor.
pub(crate) const TIMERS_SENSOR_ID: &str = "timers";
/// Object id of the "Timer alarm" binary sensor (on while ringing).
pub(crate) const RINGING_SENSOR_ID: &str = "timer_alarm";

/// Default time cap of one alarm, in seconds.
pub(crate) const DEFAULT_MAX_S: u32 = 300;
/// Largest accepted `timer_alarm_max_s`.
pub(crate) const MAX_MAX_S: u32 = 3600;
/// Most distinct finished timers one alarm remembers.
const MAX_IDS: usize = 64;

/// The entities of the timer feature for HA: the "Timers" text sensor, and
/// with an alarm sound the "Timer alarm" binary sensor and the stop button.
pub(crate) fn entities(
    with_alarm: bool,
) -> (
    Vec<ButtonEntity>,
    Vec<TextSensorEntity>,
    Vec<BinarySensorEntity>,
) {
    let text = TextSensorEntity {
        object_id: TIMERS_SENSOR_ID.to_owned(),
        name: "Timers".to_owned(),
        icon: "mdi:timer-sand".to_owned(),
        device_class: String::new(),
        entity_category: EntityCategory::None,
    };
    if !with_alarm {
        return (Vec::new(), vec![text], Vec::new());
    }
    let button = ButtonEntity {
        object_id: STOP_BUTTON_ID.to_owned(),
        name: "Stop timer alarm".to_owned(),
        icon: "mdi:alarm-off".to_owned(),
        device_class: String::new(),
        entity_category: EntityCategory::None,
    };
    let ringing = BinarySensorEntity {
        object_id: RINGING_SENSOR_ID.to_owned(),
        name: "Timer alarm".to_owned(),
        icon: "mdi:alarm-bell".to_owned(),
        device_class: String::new(),
        entity_category: EntityCategory::None,
    };
    (vec![button], vec![text], vec![ringing])
}

/// Where the alarm is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Nothing to sound.
    Idle,
    /// A timer finished but the speaker (or a conversation) is busy; it
    /// starts as soon as that ends.
    Queued,
    /// The chime is playing.
    Ringing,
}

/// What the daemon must do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Start the chime; it may play for at most this long.
    Start(Duration),
    /// The cap passed: stop the chime and give up.
    Expire,
}

/// The alarm state machine. Time is passed in, never read.
#[derive(Debug)]
pub(crate) struct Alarm {
    max: Duration,
    phase: Phase,
    /// Latest moment the alarm may still sound.
    deadline: Option<Instant>,
    /// Finished timers that are making it ring.
    ids: BTreeSet<String>,
}

impl Alarm {
    /// `max` is the cap of one alarm; zero turns the sound off.
    pub(crate) fn new(max: Duration) -> Self {
        Self {
            max,
            phase: Phase::Idle,
            deadline: None,
            ids: BTreeSet::new(),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        !self.max.is_zero()
    }

    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    /// A timer finished. Returns false when the sound is off. A timer that
    /// finishes while the alarm rings or waits joins it and restarts the cap
    /// (it is news nobody has acknowledged).
    pub(crate) fn on_finished(&mut self, id: &str, now: Instant) -> bool {
        if !self.enabled() {
            return false;
        }
        if self.ids.len() < MAX_IDS {
            self.ids.insert(id.to_owned());
        }
        self.deadline = Some(now + self.max);
        if self.phase == Phase::Idle {
            self.phase = Phase::Queued;
        }
        true
    }

    /// HA cancelled or changed timer `id`. When it was the last one making
    /// the alarm ring, the alarm stops; returns true then.
    pub(crate) fn timer_gone(&mut self, id: &str) -> bool {
        if self.ids.remove(id) && self.ids.is_empty() && self.phase != Phase::Idle {
            self.reset();
            return true;
        }
        false
    }

    /// Decides the next step. `busy`: a reply, an announcement or a
    /// conversation is using the speaker or the microphone.
    pub(crate) fn poll(&mut self, now: Instant, busy: bool) -> Option<Step> {
        let deadline = self.deadline?;
        if self.phase == Phase::Idle {
            return None;
        }
        if now >= deadline {
            return Some(Step::Expire);
        }
        (self.phase == Phase::Queued && !busy).then(|| Step::Start(deadline - now))
    }

    /// The chime started playing.
    pub(crate) fn started(&mut self) {
        if self.phase == Phase::Queued {
            self.phase = Phase::Ringing;
        }
    }

    /// A reply or announcement took the speaker from the ringing alarm: it
    /// waits and resumes afterwards (within the cap).
    pub(crate) fn preempted(&mut self) {
        if self.phase == Phase::Ringing {
            self.phase = Phase::Queued;
        }
    }

    /// The chime task ended by itself. `ok` false means the sink failed:
    /// the alarm gives up. A task that ended while the cap was extended by a
    /// later timer is started again.
    pub(crate) fn ended(&mut self, ok: bool, now: Instant) {
        if self.phase != Phase::Ringing {
            return;
        }
        match self.deadline {
            Some(d) if ok && d > now + Duration::from_secs(1) => self.phase = Phase::Queued,
            _ => self.reset(),
        }
    }

    /// Stops the alarm (wake word, button, cap, ...). Returns the phase it
    /// was in, `None` when it was idle.
    pub(crate) fn stop(&mut self) -> Option<Phase> {
        let was = self.phase;
        self.reset();
        (was != Phase::Idle).then_some(was)
    }

    fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.deadline = None;
        self.ids.clear();
    }
}

/// Sample rate of [`chime_cycle`]: the rate of streamed replies, which
/// PulseAudio converts for the sink.
pub(crate) const CHIME_RATE: u32 = 16_000;

/// Length of one cycle of the chime.
pub(crate) const CYCLE: Duration = Duration::from_millis(2600);

/// Length of one note including its fade, in seconds.
const NOTE_S: f32 = 1.2;

/// Adds a bell-like note (fundamental plus two quiet partials, a 4 ms
/// attack and an exponential decay) to `buf` from `start_s`.
fn add_note(buf: &mut [f32], start_s: f32, freq: f32, level: f32) {
    let rate = CHIME_RATE as f32;
    let first = (start_s * rate) as usize;
    let len = (NOTE_S * rate) as usize;
    let fade = (0.1 * rate) as usize;
    for n in 0..len {
        let Some(slot) = buf.get_mut(first + n) else {
            break;
        };
        let t = n as f32 / rate;
        // A raised-cosine fade over the last 100 ms ends the note cleanly.
        let tail = if n + fade > len {
            f32::midpoint(
                1.0,
                (std::f32::consts::PI * (n + fade - len) as f32 / fade as f32).cos(),
            )
        } else {
            1.0
        };
        let env = (t / 0.004).min(1.0) * (-t / 0.22).exp() * tail;
        let tone = (TAU * freq * t).sin()
            + 0.35 * (TAU * 2.0 * freq * t).sin() * (-t / 0.15).exp()
            + 0.12 * (TAU * 3.0 * freq * t).sin() * (-t / 0.08).exp();
        *slot += level * env * tone;
    }
}

/// One cycle of the alarm chime: a falling two-tone "ding-dong" (C6 then
/// G5), played twice, then a pause. Mono, [`CHIME_RATE`] Hz, s16le samples,
/// peak about 0.7 of full scale (the reply volume scales it down), starting
/// and ending at silence so the repeat has no click.
pub(crate) fn chime_cycle() -> Vec<i16> {
    const C6: f32 = 1046.5;
    const G5: f32 = 783.99;
    let total = (CYCLE.as_secs_f32() * CHIME_RATE as f32) as usize;
    let mut buf = vec![0.0_f32; total];
    for start in [0.0_f32, 0.9] {
        add_note(&mut buf, start, C6, 0.5);
        add_note(&mut buf, start + 0.32, G5, 0.5);
    }
    let peak = buf.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    let scale = if peak > 0.0 {
        0.7 * 32767.0 / peak
    } else {
        0.0
    };
    buf.iter().map(|s| (s * scale) as i16).collect()
}

/// The text of the "Timers" sensor: `none`, `ringing`, or the number of
/// timers with the next one's time left rounded up to whole minutes
/// (`2 timers, next in 5 min`), so it changes at most once a minute.
pub(crate) fn timers_text(table: &TimerTable, ringing: bool, now: Instant) -> String {
    if ringing {
        return "ringing".to_owned();
    }
    let list = table.by_remaining(now);
    let Some(next) = list.first() else {
        return "none".to_owned();
    };
    let mins = next.remaining(now).as_secs().div_ceil(60).max(1);
    let n = list.len();
    let paused = if next.is_active { "" } else { " (paused)" };
    format!(
        "{n} timer{}, next in {mins} min{paused}",
        if n == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn alarm() -> (Alarm, Instant) {
        (Alarm::new(Duration::from_secs(300)), Instant::now())
    }

    #[test]
    fn finished_timer_starts_the_chime_with_the_cap() {
        let (mut a, t0) = alarm();
        assert_eq!(a.poll(t0, false), None);
        assert!(a.on_finished("t1", t0));
        assert_eq!(a.phase(), Phase::Queued);
        assert_eq!(
            a.poll(t0 + Duration::from_secs(1), false),
            Some(Step::Start(Duration::from_secs(299)))
        );
        a.started();
        assert_eq!(a.phase(), Phase::Ringing);
        // Ringing: nothing to do until the cap.
        assert_eq!(a.poll(t0 + Duration::from_secs(10), false), None);
        assert_eq!(
            a.poll(t0 + Duration::from_secs(300), false),
            Some(Step::Expire)
        );
        assert_eq!(a.stop(), Some(Phase::Ringing));
        assert_eq!(a.phase(), Phase::Idle);
        assert_eq!(a.poll(t0 + Duration::from_secs(301), false), None);
    }

    #[test]
    fn disabled_alarm_never_rings() {
        let mut a = Alarm::new(Duration::ZERO);
        assert!(!a.enabled());
        assert!(!a.on_finished("t1", Instant::now()));
        assert_eq!(a.phase(), Phase::Idle);
    }

    #[test]
    fn busy_speaker_queues_the_alarm_until_it_is_free() {
        let (mut a, t0) = alarm();
        a.on_finished("t1", t0);
        let at = |s| t0 + Duration::from_secs(s);
        // A reply is playing: wait.
        assert_eq!(a.poll(at(1), true), None);
        assert_eq!(a.poll(at(20), true), None);
        assert_eq!(a.phase(), Phase::Queued);
        // Free: starts, with the cap counted from the finish, not the start.
        assert_eq!(
            a.poll(at(21), false),
            Some(Step::Start(Duration::from_secs(279)))
        );
        a.started();
        // An announcement takes the speaker: back to the queue, then resume.
        a.preempted();
        assert_eq!(a.phase(), Phase::Queued);
        assert_eq!(a.poll(at(40), true), None);
        assert_eq!(
            a.poll(at(41), false),
            Some(Step::Start(Duration::from_secs(259)))
        );
    }

    #[test]
    fn queued_alarm_expires_if_the_speaker_stays_busy() {
        let (mut a, t0) = alarm();
        a.on_finished("t1", t0);
        assert_eq!(
            a.poll(t0 + Duration::from_secs(300), true),
            Some(Step::Expire)
        );
    }

    #[test]
    fn stop_is_idempotent_and_clears_the_queue() {
        let (mut a, t0) = alarm();
        assert_eq!(a.stop(), None);
        a.on_finished("t1", t0);
        assert_eq!(a.stop(), Some(Phase::Queued));
        assert_eq!(a.stop(), None);
        assert_eq!(a.poll(t0, false), None);
        // A new finish rings again.
        a.on_finished("t2", t0);
        assert_eq!(a.phase(), Phase::Queued);
    }

    #[test]
    fn second_timer_restarts_the_cap_and_both_must_go() {
        let (mut a, t0) = alarm();
        let at = |s| t0 + Duration::from_secs(s);
        a.on_finished("t1", t0);
        a.started();
        a.on_finished("t2", at(200));
        assert_eq!(a.phase(), Phase::Ringing, "joins the ringing alarm");
        assert_eq!(a.poll(at(300), false), None, "cap moved to 500 s");
        assert_eq!(a.poll(at(500), false), Some(Step::Expire));
        // HA cancelling one of two keeps ringing; the last one stops it.
        assert!(!a.timer_gone("t1"));
        assert!(!a.timer_gone("unknown"));
        assert!(a.timer_gone("t2"));
        assert_eq!(a.phase(), Phase::Idle);
    }

    #[test]
    fn chime_task_ending_early_restarts_only_when_the_cap_was_extended() {
        let (mut a, t0) = alarm();
        let at = |s| t0 + Duration::from_secs(s);
        a.on_finished("t1", t0);
        a.started();
        // Ended by itself at the cap: done.
        a.ended(true, at(302));
        assert_eq!(a.phase(), Phase::Idle);
        // Extended by a later timer: start it again.
        a.on_finished("t1", t0);
        a.started();
        a.on_finished("t2", at(299));
        a.ended(true, at(302));
        assert_eq!(a.phase(), Phase::Queued);
        // A failing sink gives up.
        a.started();
        a.ended(false, at(303));
        assert_eq!(a.phase(), Phase::Idle);
        assert_eq!(a.poll(at(304), false), None);
    }

    #[test]
    fn chime_is_a_clean_two_tone_cycle() {
        let c = chime_cycle();
        assert_eq!(c.len(), (CYCLE.as_secs_f32() * CHIME_RATE as f32) as usize);
        let peak = c.iter().map(|s| i32::from(*s).abs()).max().unwrap();
        assert!((20_000..=24_000).contains(&peak), "peak {peak}");
        // Silent start and (near) silent end: the repeat has no click.
        assert_eq!(c[0], 0);
        assert!(c[..4].iter().all(|s| s.abs() < 1500), "{:?}", &c[..4]);
        assert!(c[c.len() - 4..].iter().all(|s| s.abs() < 50));
        // The pause at the end of the cycle is quiet, the notes are loud.
        let rms = |a: &[i16]| {
            (a.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / a.len() as f64).sqrt()
        };
        let rate = CHIME_RATE as usize;
        assert!(rms(&c[..rate / 5]) > 3000.0, "first ding");
        assert!(rms(&c[c.len() - rate / 5..]) < 100.0, "tail pause");
        // The first note is C6 (about 1046 Hz): count zero crossings.
        let win = &c[200..1600];
        let crossings = win.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        let hz = crossings as f32 / 2.0 / (1400.0 / CHIME_RATE as f32);
        assert!((900.0..1250.0).contains(&hz), "{hz} Hz");
    }

    #[test]
    fn timers_text_summarises_the_table() {
        use webos_esphome_api::{AppEvent, TimerEvent};
        let now = Instant::now();
        let mut table = TimerTable::new();
        assert_eq!(timers_text(&table, false, now), "none");
        let ev = |event, id: &str, left, active| AppEvent::Timer {
            event,
            id: id.to_owned(),
            name: "secret name".to_owned(),
            total_seconds: 600,
            seconds_left: left,
            is_active: active,
        };
        table.apply(&ev(TimerEvent::Started, "a", 600, true), now);
        assert_eq!(timers_text(&table, false, now), "1 timer, next in 10 min");
        table.apply(&ev(TimerEvent::Started, "b", 20, true), now);
        assert_eq!(timers_text(&table, false, now), "2 timers, next in 1 min");
        let later = now + Duration::from_secs(20);
        assert_eq!(timers_text(&table, false, later), "2 timers, next in 1 min");
        table.apply(&ev(TimerEvent::Updated, "b", 20, false), now);
        table.apply(&ev(TimerEvent::Cancelled, "a", 0, true), now);
        assert_eq!(
            timers_text(&table, false, later),
            "1 timer, next in 1 min (paused)"
        );
        assert_eq!(timers_text(&table, true, later), "ringing");
        assert!(!timers_text(&table, false, later).contains("secret"));
    }
}
