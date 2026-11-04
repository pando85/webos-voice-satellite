//! "Wake and darken" (always-on design section 12, D6 variant A): after the
//! TV is turned off with the remote, wake it and turn its screen off, so it
//! looks off while the satellite keeps listening and replies stay audible.
//!
//! Pure state machine, driven by the power-state subscription
//! ([`crate::power`]) and time; it returns [`Action`]s that the daemon runs
//! as `luna-send` calls. A port of `tools/lifecycle/tv/d6-watcher.sh`, the
//! tool D6 verified on c3-b (firmware 03.30.73, 2026-11-03):
//!
//! 1. `Request Power Off` with `onOff` `off` remembers the reason and the
//!    settled state it started from.
//! 2. A settled `Active Standby` within [`REQUEST_WINDOW`] of it is acted on
//!    only when the mode is [`RemoteOff::KeepListening`], the reason is
//!    `remoteKey` (the remote, the API key and an HA turn-off all give it;
//!    timers, CEC, HomeKit and others have their own reasons and are left
//!    alone), nothing inhibits it, and the cooldown and hourly limit allow.
//! 3. [`Action::Wake`]: `powerOn {"reason":"remoteKey"}`, retried while it is
//!    refused with `Keyblock1.5second`. If the TV is still in
//!    `Active Standby` [`KEY_FALLBACK_AFTER`] later, the toggle key once
//!    ([`Action::Key`], only while that state holds).
//! 4. At the settled `Active`, [`Action::Darken`]: `turnOffScreen`; one
//!    retry if the TV is still `Active` [`DARKEN_RETRY_AFTER`] later. Done at
//!    a settled `Screen Off`. (Earlier, at the `Screen On` step, tvpowerd
//!    accepted it in D6 but refused it once on c3-b with "The current
//!    substate must be 'active'", so the daemon waits for `Active`, which
//!    follows `Screen On` within about 0.1 s.)
//! 5. Second-press rule: a power-off that starts from `Screen Off` is the
//!    user turning the TV on: wake, do not darken. D6 applied it to the
//!    `Screen Off` the watcher made; the daemon applies it to every
//!    `Screen Off`, because after a daemon restart it cannot know who made
//!    it, and a wrong guess must leave the TV lit, never dark (the user then
//!    presses again and gets the normal look-off).
//!
//! The machine never changes a TV setting; it only wakes the TV and turns
//! the screen off, as the remote and LG's own "screen off" would.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

use crate::config::RemoteOff;
use crate::power::PowerUpdate;

/// A settled `Active Standby` later than this after the request is not acted
/// on (D6: +3.0 to +3.3 s).
pub(crate) const REQUEST_WINDOW: Duration = Duration::from_secs(20);
/// A wake or darken that did not reach its state within this time is given up.
pub(crate) const PHASE_TIMEOUT: Duration = Duration::from_secs(15);
/// Toggle-key fallback when `powerOn` left the TV in `Active Standby` (the
/// `powerOn` retries take up to about 5 s).
pub(crate) const KEY_FALLBACK_AFTER: Duration = Duration::from_secs(7);
/// One `turnOffScreen` retry if the TV is still `Active` this long after it.
pub(crate) const DARKEN_RETRY_AFTER: Duration = Duration::from_secs(3);
/// The only power-off reason acted on.
pub(crate) const ACCEPTED_REASON: &str = "remoteKey";

/// What the daemon must call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// `tvpower/power/powerOn {"reason":"remoteKey"}`, with keyblock retries.
    Wake,
    /// `networkinput/controls/power` (a toggle): only sent while the last
    /// settled state is `Active Standby`.
    Key,
    /// `tvpower/power/turnOffScreen`.
    Darken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Waking {
        since: Instant,
        darken: bool,
        key_sent: bool,
    },
    Darkening {
        since: Instant,
        retried: bool,
    },
}

#[derive(Debug, Clone)]
struct Pending {
    at: Instant,
    reason: String,
    from: String,
}

/// The wake-and-darken state machine.
#[derive(Debug)]
pub(crate) struct Darkener {
    mode: RemoteOff,
    phase: Phase,
    /// The current `Screen Off` was made by this machine.
    dark: bool,
    pending: Option<Pending>,
    /// Last settled state (compact).
    settled: String,
    actions: VecDeque<Instant>,
    max_per_hour: usize,
    cooldown: Duration,
}

impl Darkener {
    pub(crate) fn new(mode: RemoteOff, max_per_hour: u32, cooldown: Duration) -> Self {
        Self {
            mode,
            phase: Phase::Idle,
            dark: false,
            pending: None,
            settled: String::new(),
            actions: VecDeque::new(),
            max_per_hour: usize::try_from(max_per_hour).unwrap_or(usize::MAX),
            cooldown,
        }
    }

    /// Changes the mode. Leaving [`RemoteOff::KeepListening`] stops a wake or
    /// darken in progress (no further calls).
    pub(crate) fn set_mode(&mut self, mode: RemoteOff) {
        self.mode = mode;
        if mode == RemoteOff::Normal {
            self.phase = Phase::Idle;
            self.pending = None;
        }
    }

    /// The current `Screen Off` was made by the satellite.
    #[cfg(test)]
    pub(crate) fn screen_dark_by_us(&self) -> bool {
        self.dark
    }

    /// One subscription update. `inhibit` names a reason not to act now
    /// (for example the kill switch); it is logged.
    pub(crate) fn on_update(
        &mut self,
        now: Instant,
        u: &PowerUpdate,
        inhibit: Option<&str>,
    ) -> Option<Action> {
        let s = u.state_key();
        let p = u.processing_key();
        let mut action = None;
        if p.is_empty() && !s.is_empty() {
            self.settled.clone_from(&s);
            match s.as_str() {
                "ScreenOff" => {
                    if let Phase::Darkening { since, .. } = self.phase {
                        self.dark = true;
                        self.phase = Phase::Idle;
                        log::info!(
                            "remote power-off: screen off, the TV looks off and keeps listening ({} ms after the darken call)",
                            now.saturating_duration_since(since).as_millis()
                        );
                    }
                }
                "Active" | "ScreenSaver" if self.phase == Phase::Idle => self.dark = false,
                _ => {}
            }
        }

        if p == "RequestPowerOff" && u.on_off.as_deref() == Some("off") {
            let reason = u.reason.clone().unwrap_or_default();
            log::info!(
                "remote power-off: request (reason {reason}, from {}, screen dark by the satellite: {})",
                if s.is_empty() { "-" } else { &s },
                self.dark
            );
            self.pending = Some(Pending {
                at: now,
                reason,
                from: s.clone(),
            });
        }

        if p.is_empty() && s == "ActiveStandby" {
            if let Some(req) = self.pending.take() {
                if now.saturating_duration_since(req.at) < REQUEST_WINDOW {
                    action = self.decide(now, &req, inhibit);
                }
            }
        }

        if let Phase::Waking {
            since,
            darken,
            key_sent,
        } = self.phase
        {
            let active = p.is_empty() && s == "Active";
            if darken && active {
                log::info!(
                    "remote power-off: on {} ms after the wake; turning the screen off",
                    now.saturating_duration_since(since).as_millis()
                );
                self.phase = Phase::Darkening {
                    since: now,
                    retried: false,
                };
                action = Some(Action::Darken);
            } else if !darken && active {
                log::info!(
                    "remote power-off: the TV is on again ({} ms after the wake; second press)",
                    now.saturating_duration_since(since).as_millis()
                );
                self.phase = Phase::Idle;
                self.dark = false;
            } else if !key_sent && p.is_empty() && s != "ActiveStandby" && !s.is_empty() {
                // Left standby: no key fallback needed any more.
                self.phase = Phase::Waking {
                    since,
                    darken,
                    key_sent: true,
                };
            }
        }
        action
    }

    fn decide(&mut self, now: Instant, req: &Pending, inhibit: Option<&str>) -> Option<Action> {
        let after = now.saturating_duration_since(req.at).as_millis();
        if self.mode != RemoteOff::KeepListening {
            log::info!(
                "remote power-off: standby {after} ms after the request; nothing to do (turn off normally)"
            );
            return None;
        }
        if req.reason != ACCEPTED_REASON {
            log::info!(
                "remote power-off: reason {} is not the remote; left alone",
                req.reason
            );
            return None;
        }
        if let Some(why) = inhibit {
            log::info!("remote power-off: not woken ({why})");
            return None;
        }
        while self
            .actions
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(3600))
        {
            self.actions.pop_front();
        }
        if self.actions.len() >= self.max_per_hour {
            log::warn!(
                "remote power-off: not woken: {} actions in the last hour (limit)",
                self.actions.len()
            );
            return None;
        }
        if let Some(last) = self.actions.back() {
            if now.saturating_duration_since(*last) < self.cooldown {
                log::warn!(
                    "remote power-off: not woken: cooldown ({} s since the last action)",
                    now.saturating_duration_since(*last).as_secs()
                );
                return None;
            }
        }
        self.actions.push_back(now);
        let second_press = req.from == "ScreenOff";
        if second_press {
            log::info!(
                "remote power-off from screen off (dark by the satellite: {}): second press, waking the TV with the screen on",
                self.dark
            );
        } else {
            log::info!(
                "remote power-off: standby {after} ms after the request; waking the TV to turn the screen off"
            );
        }
        self.dark = false;
        self.phase = Phase::Waking {
            since: now,
            darken: !second_press,
            key_sent: false,
        };
        Some(Action::Wake)
    }

    /// Time-based steps: the key fallback, the darken retry and timeouts.
    pub(crate) fn on_tick(&mut self, now: Instant) -> Option<Action> {
        match self.phase {
            Phase::Idle => None,
            Phase::Waking {
                since,
                darken,
                key_sent,
            } => {
                let t = now.saturating_duration_since(since);
                if t >= PHASE_TIMEOUT {
                    log::warn!(
                        "remote power-off: the TV did not wake within {} s (state {}); giving up",
                        PHASE_TIMEOUT.as_secs(),
                        self.settled
                    );
                    self.phase = Phase::Idle;
                    None
                } else if !key_sent && t >= KEY_FALLBACK_AFTER && self.settled == "ActiveStandby" {
                    log::info!("remote power-off: still in standby; sending the power key");
                    self.phase = Phase::Waking {
                        since,
                        darken,
                        key_sent: true,
                    };
                    Some(Action::Key)
                } else {
                    None
                }
            }
            Phase::Darkening { since, retried } => {
                let t = now.saturating_duration_since(since);
                if t >= PHASE_TIMEOUT {
                    log::warn!(
                        "remote power-off: no screen off within {} s (state {}); giving up",
                        PHASE_TIMEOUT.as_secs(),
                        self.settled
                    );
                    self.phase = Phase::Idle;
                    None
                } else if !retried && t >= DARKEN_RETRY_AFTER && self.settled == "Active" {
                    log::info!("remote power-off: still on; turning the screen off again");
                    self.phase = Phase::Darkening {
                        since,
                        retried: true,
                    };
                    Some(Action::Darken)
                } else {
                    None
                }
            }
        }
    }

    /// A wake or darken is in progress.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// The current mode.
    pub(crate) fn mode(&self) -> RemoteOff {
        self.mode
    }

    /// Whether this machine would act on a remote power-off now: the mode is
    /// keep-listening, nothing inhibits it, no wake or darken is in
    /// progress, the hourly budget has room and the cooldown has passed. TV
    /// control asks before it sends the power key for Home Assistant, so a
    /// refusal here becomes a refusal of the key (design section 4.2): the
    /// key would otherwise end in LG standby or Suspend. Does not change
    /// anything; the budget is spent when the machine acts.
    pub(crate) fn would_act(&self, now: Instant, inhibit: Option<&str>) -> Result<(), String> {
        if self.mode != RemoteOff::KeepListening {
            return Err("the remote power-off mode is not keep listening".to_owned());
        }
        if let Some(why) = inhibit {
            return Err(why.to_owned());
        }
        if self.phase != Phase::Idle {
            return Err("a wake or darken is in progress".to_owned());
        }
        let hour = Duration::from_secs(3600);
        let recent = self
            .actions
            .iter()
            .filter(|t| now.saturating_duration_since(**t) < hour)
            .count();
        if recent >= self.max_per_hour {
            return Err(format!("{recent} actions in the last hour (limit)"));
        }
        if let Some(last) = self.actions.back() {
            let since = now.saturating_duration_since(*last);
            if since < self.cooldown {
                return Err(format!(
                    "cooldown ({} s since the last action)",
                    since.as_secs()
                ));
            }
        }
        Ok(())
    }

    /// The satellite itself woke the TV (TV control's turn-on): the screen
    /// is no longer dark by the darkener's doing. Not while a wake or darken
    /// of its own is running.
    pub(crate) fn note_own_wake(&mut self) {
        if self.phase == Phase::Idle {
            self.dark = false;
        }
    }
}

/// Object id of the HA select.
pub(crate) const SELECT_OBJECT_ID: &str = "remote_power_off";
/// Its name in HA.
pub(crate) const SELECT_NAME: &str = "When the TV is turned off with the remote";
/// Option for [`RemoteOff::Normal`].
pub(crate) const OPTION_NORMAL: &str = "Turn off normally";
/// Option for [`RemoteOff::KeepListening`].
pub(crate) const OPTION_KEEP_LISTENING: &str = "Keep listening (screen off)";

/// The select entity.
pub(crate) fn select_entity() -> webos_esphome_api::SelectEntity {
    webos_esphome_api::SelectEntity {
        object_id: SELECT_OBJECT_ID.to_owned(),
        name: SELECT_NAME.to_owned(),
        icon: "mdi:power-standby".to_owned(),
        options: vec![OPTION_NORMAL.to_owned(), OPTION_KEEP_LISTENING.to_owned()],
        entity_category: webos_esphome_api::proto::EntityCategory::Config,
        command_echo: true,
    }
}

/// The select option for a mode.
pub(crate) fn option_for(mode: RemoteOff) -> &'static str {
    match mode {
        RemoteOff::Normal => OPTION_NORMAL,
        RemoteOff::KeepListening => OPTION_KEEP_LISTENING,
    }
}

/// The mode for a select option.
pub(crate) fn mode_for(option: &str) -> Option<RemoteOff> {
    match option {
        OPTION_NORMAL => Some(RemoteOff::Normal),
        OPTION_KEEP_LISTENING => Some(RemoteOff::KeepListening),
        _ => None,
    }
}

/// The word stored in the state file (the same as the config value).
pub(crate) fn file_word(mode: RemoteOff) -> &'static str {
    match mode {
        RemoteOff::Normal => "normal",
        RemoteOff::KeepListening => "keep_listening",
    }
}

/// Reads the stored mode; `Err` carries the reason it was not usable
/// (`Ok(None)`: no file yet).
pub(crate) fn load_mode(path: &std::path::Path) -> Result<Option<RemoteOff>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => match t.trim() {
            "normal" => Ok(Some(RemoteOff::Normal)),
            "keep_listening" => Ok(Some(RemoteOff::KeepListening)),
            other => Err(format!("unknown value {other:?}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn st(state: &str) -> PowerUpdate {
        PowerUpdate {
            state: Some(state.to_owned()),
            ..PowerUpdate::default()
        }
    }

    fn pr(state: &str, processing: &str, on_off: &str, reason: &str) -> PowerUpdate {
        PowerUpdate {
            state: Some(state.to_owned()),
            processing: Some(processing.to_owned()),
            reason: Some(reason.to_owned()),
            on_off: Some(on_off.to_owned()),
        }
    }

    struct T {
        d: Darkener,
        t0: Instant,
    }

    impl T {
        fn new(mode: RemoteOff) -> Self {
            Self {
                d: Darkener::new(mode, 10, Duration::from_secs(20)),
                t0: Instant::now(),
            }
        }
        fn at(&self, ms: u64) -> Instant {
            self.t0 + Duration::from_millis(ms)
        }
        fn up(&mut self, ms: u64, u: &PowerUpdate) -> Option<Action> {
            let now = self.at(ms);
            self.d.on_update(now, u, None)
        }
        /// The D6 sequence of a remote power-off from `from` into standby,
        /// starting at `ms`; returns the action at the settled standby.
        fn power_off(&mut self, ms: u64, from: &str, reason: &str) -> Option<Action> {
            assert_eq!(
                self.up(ms, &pr(from, "Request Power Off", "off", reason)),
                None
            );
            self.up(ms + 100, &pr(from, "Request Power Off Logo", "off", reason));
            self.up(
                ms + 2900,
                &pr("Active Standby", "Prepare Active Standby", "off", reason),
            );
            self.up(ms + 3100, &st("Active Standby"))
        }
        /// The wake sequence; returns the actions seen.
        fn wake(&mut self, ms: u64) -> Vec<Action> {
            let mut a = Vec::new();
            for (dt, u) in [
                (
                    0,
                    pr("Active Standby", "Prepare Power On", "on", "remoteKey"),
                ),
                (
                    1200,
                    pr("Active Standby", "LastInput Ready", "on", "remoteKey"),
                ),
                (1500, pr("Active Standby", "Screen On", "on", "remoteKey")),
                (2000, st("Active")),
            ] {
                a.extend(self.up(ms + dt, &u));
            }
            a
        }
    }

    #[test]
    fn select_options_and_mode_file_round_trip() {
        assert_eq!(
            mode_for(option_for(RemoteOff::Normal)),
            Some(RemoteOff::Normal)
        );
        assert_eq!(
            mode_for(option_for(RemoteOff::KeepListening)),
            Some(RemoteOff::KeepListening)
        );
        assert_eq!(mode_for("Darken"), None);
        assert_eq!(select_entity().options.len(), 2);
        let dir = std::env::temp_dir().join(format!("webos-sat-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let f = dir.join("remote_off");
        assert_eq!(load_mode(&f), Ok(None));
        for m in [RemoteOff::KeepListening, RemoteOff::Normal] {
            crate::volume::write_atomic(&f, &format!("{}\n", file_word(m))).unwrap_or_default();
            assert_eq!(load_mode(&f), Ok(Some(m)));
        }
        std::fs::write(&f, "on").unwrap_or_default();
        assert!(load_mode(&f).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normal_mode_never_acts() {
        let mut t = T::new(RemoteOff::Normal);
        assert_eq!(t.up(0, &st("Active")), None);
        assert_eq!(t.power_off(1000, "Active", "remoteKey"), None);
        assert_eq!(t.d.on_tick(t.at(20_000)), None);
        assert!(!t.d.busy());
    }

    #[test]
    fn wake_and_darken_then_second_press_wakes_lit() {
        let mut t = T::new(RemoteOff::KeepListening);
        t.up(0, &st("Active"));
        assert_eq!(t.power_off(1000, "Active", "remoteKey"), Some(Action::Wake));
        // Darken at the settled Active, once (not at Screen On).
        assert_eq!(t.wake(5000), vec![Action::Darken]);
        assert!(t.d.busy());
        assert_eq!(t.up(7300, &st("Screen Off")), None);
        assert!(!t.d.busy());
        assert!(t.d.screen_dark_by_us());
        // Second press from our Screen Off (after the cooldown): wake, no darken.
        assert_eq!(
            t.power_off(40_000, "Screen Off", "remoteKey"),
            Some(Action::Wake)
        );
        assert_eq!(t.wake(44_000), vec![]);
        assert!(!t.d.busy());
        assert!(!t.d.screen_dark_by_us());
        // A Screen Off the satellite did not make (or does not remember, after
        // a restart) also counts as a second press: the TV ends lit.
        t.up(60_000, &st("Screen Off"));
        assert!(!t.d.screen_dark_by_us());
        assert_eq!(
            t.power_off(62_000, "Screen Off", "remoteKey"),
            Some(Action::Wake)
        );
        assert_eq!(t.wake(66_000), vec![]);
        // A power-off from Active again is a normal wake and darken.
        assert_eq!(
            t.power_off(100_000, "Active", "remoteKey"),
            Some(Action::Wake)
        );
        assert_eq!(t.wake(104_000), vec![Action::Darken]);
    }

    #[test]
    fn other_reasons_are_left_alone() {
        let mut t = T::new(RemoteOff::KeepListening);
        t.up(0, &st("Active"));
        for reason in [
            "offTimer",
            "autoOff",
            "hdmiCec",
            "homekit",
            "sleepTimer",
            "",
        ] {
            assert_eq!(t.power_off(1000, "Active", reason), None, "{reason}");
        }
        assert!(!t.d.busy());
    }

    #[test]
    fn a_late_standby_and_a_power_on_request_are_not_acted_on() {
        let mut t = T::new(RemoteOff::KeepListening);
        t.up(0, &st("Active"));
        t.up(1000, &pr("Active", "Request Power Off", "off", "remoteKey"));
        assert_eq!(
            t.up(25_000, &st("Active Standby")),
            None,
            "outside the window"
        );
        // A standby without a request (for example after a reboot by qbd).
        assert_eq!(t.up(30_000, &st("Active Standby")), None);
        // A power-on request is not a power-off.
        t.up(
            31_000,
            &pr("Active Standby", "Request Power Off", "on", "remoteKey"),
        );
        assert_eq!(t.up(32_000, &st("Active Standby")), None);
    }

    #[test]
    fn inhibit_cooldown_and_hourly_limit() {
        let mut t = T::new(RemoteOff::KeepListening);
        t.up(0, &st("Active"));
        t.up(1000, &pr("Active", "Request Power Off", "off", "remoteKey"));
        assert_eq!(
            t.d.on_update(t.at(4000), &st("Active Standby"), Some("kill switch")),
            None
        );
        assert_eq!(
            t.power_off(10_000, "Active", "remoteKey"),
            Some(Action::Wake)
        );
        t.d.set_mode(RemoteOff::KeepListening);
        t.up(15_000, &st("Active"));
        // 10 s later: cooldown (20 s).
        assert_eq!(t.power_off(20_000, "Active", "remoteKey"), None);
        // Fill the hourly limit of 10.
        let mut ms = 40_000;
        for _ in 0..9 {
            assert_eq!(t.power_off(ms, "Active", "remoteKey"), Some(Action::Wake));
            t.up(ms + 5000, &st("Active"));
            t.d.set_mode(RemoteOff::KeepListening);
            ms += 30_000;
        }
        assert_eq!(
            t.power_off(ms, "Active", "remoteKey"),
            None,
            "limit reached"
        );
        // An hour after the first action there is room again.
        assert_eq!(
            t.power_off(10_000 + 3_600_000, "Active", "remoteKey"),
            Some(Action::Wake)
        );
    }

    #[test]
    fn key_fallback_darken_retry_and_timeouts() {
        let mut t = T::new(RemoteOff::KeepListening);
        t.up(0, &st("Active"));
        assert_eq!(t.power_off(1000, "Active", "remoteKey"), Some(Action::Wake));
        // powerOn did nothing: the key once, at +7 s, while still in standby.
        assert_eq!(t.d.on_tick(t.at(4100 + 6000)), None);
        assert_eq!(t.d.on_tick(t.at(4100 + 7000)), Some(Action::Key));
        assert_eq!(t.d.on_tick(t.at(4100 + 8000)), None, "only once");
        // Settled Active without a Screen On line: darken then.
        assert_eq!(t.up(13_000, &st("Active")), Some(Action::Darken));
        // Still Active 3 s later: one retry.
        assert_eq!(t.d.on_tick(t.at(16_000)), Some(Action::Darken));
        assert_eq!(t.d.on_tick(t.at(17_000)), None);
        // No Screen Off within 15 s: give up.
        assert_eq!(t.d.on_tick(t.at(28_100)), None);
        assert!(!t.d.busy());
        assert!(!t.d.screen_dark_by_us());
    }

    #[test]
    fn no_key_fallback_once_the_tv_left_standby_and_mode_off_stops() {
        let mut t = T::new(RemoteOff::KeepListening);
        t.up(0, &st("Active"));
        assert_eq!(t.power_off(1000, "Active", "remoteKey"), Some(Action::Wake));
        t.up(
            5000,
            &pr("Active Standby", "Prepare Power On", "on", "remoteKey"),
        );
        t.up(6000, &st("Screen Saver"));
        assert_eq!(t.d.on_tick(t.at(12_000)), None, "left standby: no key");
        t.d.set_mode(RemoteOff::Normal);
        assert!(!t.d.busy());
        assert_eq!(t.up(7000, &st("Active")), None);
    }

    #[test]
    fn would_act_mirrors_the_conditions_of_acting() {
        let mut t = T::new(RemoteOff::Normal);
        assert!(
            t.d.would_act(t.at(0), None)
                .unwrap_err()
                .contains("not keep listening")
        );
        t.d.set_mode(RemoteOff::KeepListening);
        assert_eq!(t.d.mode(), RemoteOff::KeepListening);
        assert_eq!(t.d.would_act(t.at(0), None), Ok(()));
        assert_eq!(
            t.d.would_act(t.at(0), Some("kill switch")),
            Err("kill switch".to_owned())
        );
        // Acting spends the budget and starts the cooldown; would_act itself
        // changes nothing.
        t.up(0, &st("Active"));
        for _ in 0..5 {
            assert_eq!(t.d.would_act(t.at(500), None), Ok(()));
        }
        assert_eq!(t.power_off(1000, "Active", "remoteKey"), Some(Action::Wake));
        // A wake is in progress.
        assert!(
            t.d.would_act(t.at(5000), None)
                .unwrap_err()
                .contains("in progress")
        );
        t.up(5000, &st("Active"));
        t.up(5500, &st("Screen Off"));
        assert!(!t.d.busy());
        // Cooldown: 20 s from the action at 4100 ms.
        assert!(
            t.d.would_act(t.at(10_000), None)
                .unwrap_err()
                .contains("cooldown")
        );
        assert_eq!(t.d.would_act(t.at(25_000), None), Ok(()));
        // The hourly limit: nine more actions fill it.
        let mut ms = 40_000;
        for _ in 0..9 {
            t.up(ms, &st("Active"));
            t.d.set_mode(RemoteOff::KeepListening);
            assert_eq!(
                t.power_off(ms + 100, "Active", "remoteKey"),
                Some(Action::Wake)
            );
            t.up(ms + 5000, &st("Active"));
            t.up(ms + 5200, &st("Screen Off"));
            ms += 30_000;
        }
        t.d.set_mode(RemoteOff::KeepListening);
        let err = t.d.would_act(t.at(ms), None).unwrap_err();
        assert!(err.contains("10 actions in the last hour"), "{err}");
        // An hour after the first action there is room again.
        assert_eq!(t.d.would_act(t.at(3_700_000), None), Ok(()));
    }

    #[test]
    fn an_own_wake_clears_the_dark_by_us_bookkeeping_but_not_mid_action() {
        let mut t = T::new(RemoteOff::KeepListening);
        t.up(0, &st("Active"));
        assert_eq!(t.power_off(1000, "Active", "remoteKey"), Some(Action::Wake));
        t.d.on_update(t.at(5000), &st("Active"), None);
        t.d.on_update(t.at(5100), &st("Screen Off"), None);
        assert!(t.d.screen_dark_by_us());
        t.d.note_own_wake();
        assert!(!t.d.screen_dark_by_us());
        // In the middle of a wake the flag is left alone.
        t.up(10_000, &st("Active"));
        assert_eq!(
            t.power_off(40_000, "Active", "remoteKey"),
            Some(Action::Wake)
        );
        assert!(t.d.busy());
        t.d.note_own_wake();
        assert!(t.d.busy());
    }
}
