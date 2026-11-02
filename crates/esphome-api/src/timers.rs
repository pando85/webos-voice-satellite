//! Voice assistant timers (`VoiceAssistantFeature.TIMERS`, bit 8).
//!
//! When the device advertises TIMERS, HA's ESPHome integration registers a
//! timer handler for the device and forwards every HA intent timer event
//! (set, add or remove time, pause, resume, cancel, finish) as a
//! `VoiceAssistantTimerEventResponse`. The device only displays or sounds
//! timers; HA owns them and counts them down. Reference behaviour:
//! HA 2026.10.4 `esphome/assist_satellite.py::handle_timer_event` and
//! `intent/timers.py`.
//!
//! Two facts from that code shape the design:
//!
//! - HA sends `STARTED` when a timer is created, `UPDATED` for every change
//!   (time added or removed, paused, resumed), `CANCELLED` and `FINISHED`.
//!   Pausing is an `UPDATED` with `is_active = false`.
//! - HA does **not** replay existing timers when the device (re)connects: the
//!   handler is registered fresh and sees only later events. The application
//!   keeps the state itself, which is what [`TimerTable`] is for.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use crate::app::AppEvent;
use crate::proto as pb;

/// What happened to a timer (`VoiceAssistantTimerEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerEvent {
    /// A timer was created.
    Started,
    /// Time was added or removed, or the timer was paused or resumed.
    Updated,
    /// The timer was cancelled.
    Cancelled,
    /// The timer ran out. In Phase 4 the TV plays a chime here.
    Finished,
}

impl TimerEvent {
    /// Maps the wire value; `None` for values this crate does not know.
    #[must_use]
    pub fn from_proto(value: i32) -> Option<Self> {
        match pb::VoiceAssistantTimerEvent::try_from(value).ok()? {
            pb::VoiceAssistantTimerEvent::VoiceAssistantTimerStarted => Some(Self::Started),
            pb::VoiceAssistantTimerEvent::VoiceAssistantTimerUpdated => Some(Self::Updated),
            pb::VoiceAssistantTimerEvent::VoiceAssistantTimerCancelled => Some(Self::Cancelled),
            pb::VoiceAssistantTimerEvent::VoiceAssistantTimerFinished => Some(Self::Finished),
        }
    }

    /// Lower-case name for log lines.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Updated => "updated",
            Self::Cancelled => "cancelled",
            Self::Finished => "finished",
        }
    }
}

impl fmt::Display for TimerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Maps a `VoiceAssistantTimerEventResponse` to an [`AppEvent::Timer`].
/// `None` if the event type is unknown.
pub(crate) fn to_app_event(msg: pb::VoiceAssistantTimerEventResponse) -> Option<AppEvent> {
    Some(AppEvent::Timer {
        event: TimerEvent::from_proto(msg.event_type)?,
        id: msg.timer_id,
        name: msg.name,
        total_seconds: msg.total_seconds,
        seconds_left: msg.seconds_left,
        is_active: msg.is_active,
    })
}

/// One timer as last reported by HA.
#[derive(Clone, PartialEq, Eq)]
pub struct Timer {
    /// HA's timer id.
    pub id: String,
    /// User-chosen name (may be empty). User words: never log it.
    pub name: String,
    /// Seconds on the timer when it was created (grows when time is added).
    pub total_seconds: u32,
    /// Seconds left when HA sent the event.
    pub seconds_left: u32,
    /// Ticking (false while paused).
    pub is_active: bool,
    /// When HA's event arrived, the base for [`Timer::remaining`].
    pub received_at: Instant,
}

impl Timer {
    /// Seconds left at `now`: counts down only while the timer is active.
    #[must_use]
    pub fn remaining(&self, now: Instant) -> Duration {
        let left = Duration::from_secs(u64::from(self.seconds_left));
        if self.is_active {
            left.saturating_sub(now.saturating_duration_since(self.received_at))
        } else {
            left
        }
    }
}

impl fmt::Debug for Timer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timer")
            .field("id", &self.id)
            .field("name_bytes", &self.name.len())
            .field("total_seconds", &self.total_seconds)
            .field("seconds_left", &self.seconds_left)
            .field("is_active", &self.is_active)
            .finish_non_exhaustive()
    }
}

/// Most timers kept; the oldest-inserted unknown ids beyond this are refused,
/// so a misbehaving peer cannot grow the table without bound.
pub const MAX_TIMERS: usize = 64;

/// The timers HA has told the device about.
///
/// Feed every [`AppEvent::Timer`] to [`TimerTable::apply`]. The table
/// survives HA reconnects (it lives in the application), which is the only
/// state recovery available because HA does not replay timers. On
/// `HaDisconnected` the application may keep sounding timers it knows about
/// from their local [`Timer::remaining`] time; HA's later events correct it.
#[derive(Debug, Default)]
pub struct TimerTable {
    timers: HashMap<String, Timer>,
}

impl TimerTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event. Returns the timer's state after the event for
    /// `Started` and `Updated`, and the removed timer (if it was known) for
    /// `Cancelled` and `Finished`. Other events are ignored (`None`).
    pub fn apply(&mut self, ev: &AppEvent, now: Instant) -> Option<Timer> {
        let AppEvent::Timer {
            event,
            id,
            name,
            total_seconds,
            seconds_left,
            is_active,
        } = ev
        else {
            return None;
        };
        match event {
            TimerEvent::Started | TimerEvent::Updated => {
                if !self.timers.contains_key(id) && self.timers.len() >= MAX_TIMERS {
                    log::warn!("timers: table full ({MAX_TIMERS}); event for a new timer dropped");
                    return None;
                }
                let t = Timer {
                    id: id.clone(),
                    name: name.clone(),
                    total_seconds: *total_seconds,
                    seconds_left: *seconds_left,
                    is_active: *is_active,
                    received_at: now,
                };
                self.timers.insert(id.clone(), t.clone());
                Some(t)
            }
            TimerEvent::Cancelled | TimerEvent::Finished => self.timers.remove(id),
        }
    }

    /// Looks a timer up by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Timer> {
        self.timers.get(id)
    }

    /// Number of known timers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.timers.len()
    }

    /// No timers known.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }

    /// Known timers, soonest first (paused timers last).
    #[must_use]
    pub fn by_remaining(&self, now: Instant) -> Vec<&Timer> {
        let mut v: Vec<&Timer> = self.timers.values().collect();
        v.sort_by_key(|t| (!t.is_active, t.remaining(now), t.id.clone()));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(event: pb::VoiceAssistantTimerEvent, id: &str, left: u32, active: bool) -> AppEvent {
        to_app_event(pb::VoiceAssistantTimerEventResponse {
            event_type: event as i32,
            timer_id: id.to_owned(),
            name: "pasta".to_owned(),
            total_seconds: 600,
            seconds_left: left,
            is_active: active,
        })
        .unwrap()
    }

    #[test]
    fn decodes_all_four_event_types() {
        use pb::VoiceAssistantTimerEvent as E;
        let cases = [
            (E::VoiceAssistantTimerStarted, TimerEvent::Started),
            (E::VoiceAssistantTimerUpdated, TimerEvent::Updated),
            (E::VoiceAssistantTimerCancelled, TimerEvent::Cancelled),
            (E::VoiceAssistantTimerFinished, TimerEvent::Finished),
        ];
        for (wire, want) in cases {
            let AppEvent::Timer {
                event,
                id,
                name,
                total_seconds,
                seconds_left,
                is_active,
            } = msg(wire, "t1", 42, true)
            else {
                panic!("not a timer event");
            };
            assert_eq!(event, want);
            assert_eq!(
                (
                    id.as_str(),
                    name.as_str(),
                    total_seconds,
                    seconds_left,
                    is_active
                ),
                ("t1", "pasta", 600, 42, true)
            );
        }
        assert_eq!(TimerEvent::Finished.to_string(), "finished");
    }

    #[test]
    fn unknown_event_type_is_dropped() {
        let m = pb::VoiceAssistantTimerEventResponse {
            event_type: 99,
            ..Default::default()
        };
        assert!(to_app_event(m).is_none());
    }

    #[test]
    fn debug_hides_the_timer_name() {
        let ev = msg(
            pb::VoiceAssistantTimerEvent::VoiceAssistantTimerStarted,
            "t1",
            5,
            true,
        );
        let d = format!("{ev:?}");
        assert!(!d.contains("pasta"), "{d}");
        assert!(
            d.contains("t1") && d.contains("Started") && d.contains("seconds_left: 5"),
            "{d}"
        );
        let t0 = Instant::now();
        let mut table = TimerTable::new();
        let t = table.apply(&ev, t0).unwrap();
        assert!(!format!("{t:?}").contains("pasta"));
    }

    #[test]
    fn table_tracks_lifecycle_and_pause() {
        use pb::VoiceAssistantTimerEvent as E;
        let t0 = Instant::now();
        let mut table = TimerTable::new();
        assert!(table.is_empty());
        table.apply(&msg(E::VoiceAssistantTimerStarted, "a", 100, true), t0);
        table.apply(&msg(E::VoiceAssistantTimerStarted, "b", 50, true), t0);
        assert_eq!(table.len(), 2);
        let order: Vec<_> = table
            .by_remaining(t0)
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(order, ["b", "a"]);

        let at = t0 + Duration::from_secs(10);
        assert_eq!(
            table.get("a").unwrap().remaining(at),
            Duration::from_secs(90)
        );
        // Pause b: HA sends UPDATED with is_active=false; it stops counting down.
        table.apply(&msg(E::VoiceAssistantTimerUpdated, "b", 40, false), at);
        let later = at + Duration::from_secs(30);
        assert_eq!(
            table.get("b").unwrap().remaining(later),
            Duration::from_secs(40)
        );
        assert_eq!(table.by_remaining(later)[0].id, "a");
        // Remaining never goes negative.
        assert_eq!(
            table
                .get("a")
                .unwrap()
                .remaining(t0 + Duration::from_secs(999)),
            Duration::ZERO
        );

        let gone = table.apply(&msg(E::VoiceAssistantTimerFinished, "a", 0, false), later);
        assert_eq!(gone.unwrap().id, "a");
        assert!(
            table
                .apply(
                    &msg(E::VoiceAssistantTimerCancelled, "zzz", 0, false),
                    later
                )
                .is_none()
        );
        table.apply(&msg(E::VoiceAssistantTimerCancelled, "b", 40, false), later);
        assert!(table.is_empty());
        assert!(table.apply(&AppEvent::HaDisconnected, later).is_none());
    }

    #[test]
    fn table_is_bounded() {
        let t0 = Instant::now();
        let mut table = TimerTable::new();
        for i in 0..MAX_TIMERS + 5 {
            table.apply(
                &msg(
                    pb::VoiceAssistantTimerEvent::VoiceAssistantTimerStarted,
                    &format!("t{i}"),
                    5,
                    true,
                ),
                t0,
            );
        }
        assert_eq!(table.len(), MAX_TIMERS);
        // Updating a known timer still works at the limit.
        let upd = msg(
            pb::VoiceAssistantTimerEvent::VoiceAssistantTimerUpdated,
            "t0",
            9,
            true,
        );
        assert_eq!(table.apply(&upd, t0).unwrap().seconds_left, 9);
    }
}
