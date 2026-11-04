//! Rate limits of the setters (`docs/design/tv-control.md` section 7): one
//! call in flight per control, volume steps at most 4 per second, input and
//! app changes at most one per 2 s, and all setters together at most 30 per
//! minute. The power-off path keeps its own limits (the darkener's cooldown
//! and hourly budget) and is not counted here.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// A control with its own in-flight slot and, for some, its own interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    /// `tv_screen`.
    Screen,
    /// `tv_turn_on` and `tv_power`.
    Power,
    /// `tv_volume` and the volume buttons.
    Volume,
    /// `tv_mute`.
    Mute,
    /// `tv_input`.
    Input,
    /// `tv_app`.
    Apps,
    /// `tv_play_state` (a sensor; it has no setter).
    PlayState,
}

impl Control {
    /// The name used in log lines.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Power => "power",
            Self::Volume => "volume",
            Self::Mute => "mute",
            Self::Input => "input",
            Self::Apps => "apps",
            Self::PlayState => "play_state",
        }
    }
}

/// All setters together per minute.
pub const GLOBAL_PER_MINUTE: usize = 30;
/// Volume step calls per second.
pub const VOLUME_STEPS_PER_SECOND: usize = 4;
/// Minimum time between two input or app changes.
pub const INPUT_APP_INTERVAL: Duration = Duration::from_secs(2);

/// The limiter.
#[derive(Debug, Default)]
pub struct Limiter {
    global: VecDeque<Instant>,
    steps: VecDeque<Instant>,
    last: HashMap<Control, Instant>,
    inflight: HashSet<Control>,
}

fn prune(q: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while q
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= window)
    {
        q.pop_front();
    }
}

impl Limiter {
    /// A call for `control` is in flight.
    #[must_use]
    pub fn busy(&self, control: Control) -> bool {
        self.inflight.contains(&control)
    }

    /// Takes a slot for a setter call, or says why not. On success the call
    /// is in flight until [`Limiter::done`]. `step` marks a volume up or down
    /// call; `counted` false leaves the global budget alone (the power-off
    /// path).
    ///
    /// # Errors
    ///
    /// The reason, for one log line.
    pub fn acquire(
        &mut self,
        control: Control,
        now: Instant,
        step: bool,
        counted: bool,
    ) -> Result<(), &'static str> {
        prune(&mut self.global, now, Duration::from_secs(60));
        prune(&mut self.steps, now, Duration::from_secs(1));
        if self.inflight.contains(&control) {
            return Err("the previous call is still running");
        }
        if matches!(control, Control::Input | Control::Apps)
            && self
                .last
                .get(&control)
                .is_some_and(|t| now.saturating_duration_since(*t) < INPUT_APP_INTERVAL)
        {
            return Err("input and app changes are limited to one per 2 s");
        }
        if step && self.steps.len() >= VOLUME_STEPS_PER_SECOND {
            return Err("volume steps are limited to 4 per second");
        }
        if counted && self.global.len() >= GLOBAL_PER_MINUTE {
            return Err("TV control calls are limited to 30 per minute");
        }
        self.inflight.insert(control);
        self.last.insert(control, now);
        if step {
            self.steps.push_back(now);
        }
        if counted {
            self.global.push_back(now);
        }
        Ok(())
    }

    /// The call for `control` finished (reply, error or timeout).
    pub fn done(&mut self, control: Control) {
        self.inflight.remove(&control);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn one_call_in_flight_per_control() {
        let t0 = Instant::now();
        let mut l = Limiter::default();
        l.acquire(Control::Volume, t0, false, true).unwrap();
        assert!(l.busy(Control::Volume));
        assert!(l.acquire(Control::Volume, at(t0, 10), false, true).is_err());
        // Another control is independent.
        l.acquire(Control::Mute, at(t0, 10), false, true).unwrap();
        l.done(Control::Volume);
        l.acquire(Control::Volume, at(t0, 20), false, true).unwrap();
    }

    #[test]
    fn volume_steps_are_limited_to_four_per_second() {
        let t0 = Instant::now();
        let mut l = Limiter::default();
        for n in 0..4 {
            l.acquire(Control::Volume, at(t0, n * 100), true, true)
                .unwrap();
            l.done(Control::Volume);
        }
        assert!(l.acquire(Control::Volume, at(t0, 450), true, true).is_err());
        // A set (not a step) is not limited by the step window.
        l.acquire(Control::Volume, at(t0, 450), false, true)
            .unwrap();
        l.done(Control::Volume);
        // The window slides.
        l.acquire(Control::Volume, at(t0, 1001), true, true)
            .unwrap();
    }

    #[test]
    fn input_and_app_changes_are_two_seconds_apart() {
        let t0 = Instant::now();
        let mut l = Limiter::default();
        l.acquire(Control::Input, t0, false, true).unwrap();
        l.done(Control::Input);
        assert!(
            l.acquire(Control::Input, at(t0, 1500), false, true)
                .is_err()
        );
        l.acquire(Control::Apps, at(t0, 1500), false, true).unwrap();
        l.done(Control::Apps);
        l.acquire(Control::Input, at(t0, 2000), false, true)
            .unwrap();
    }

    #[test]
    fn thirty_calls_per_minute_in_all_and_power_off_is_not_counted() {
        let t0 = Instant::now();
        let mut l = Limiter::default();
        for n in 0..GLOBAL_PER_MINUTE as u64 {
            l.acquire(Control::Screen, at(t0, n * 100), false, true)
                .unwrap();
            l.done(Control::Screen);
        }
        assert!(l.acquire(Control::Mute, at(t0, 5000), false, true).is_err());
        // Not counted: the power-off path.
        l.acquire(Control::Power, at(t0, 5000), false, false)
            .unwrap();
        l.done(Control::Power);
        // A minute later the budget is back.
        l.acquire(Control::Mute, at(t0, 62_000), false, true)
            .unwrap();
    }

    #[test]
    fn control_names() {
        assert_eq!(Control::Screen.name(), "screen");
        assert_eq!(Control::PlayState.name(), "play_state");
    }
}
