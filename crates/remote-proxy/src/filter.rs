//! The forwarding rule: whole frames (up to `SYN_REPORT`) pass to the sibling,
//! minus every `EV_KEY` event of the mic key; the keys the sibling holds down
//! are tracked so they can be released on any exit.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::event::{EV_KEY, EV_SYN, Event, SYN_DROPPED, SYN_REPORT};

/// LG's `KEY_VOICE` (Linux `KEY_VOICEMAIL`), the Magic Remote's mic button.
pub(crate) const MIC_KEY: u16 = 428;

/// Key codes the tracker can hold (LG's kernel has codes above the standard 767).
pub(crate) const KEY_CODES: usize = 2048;

/// Keys currently down **in the sibling** (as we sent them). Atomic so the
/// watchdog and the panic hook can release them while the main thread is stuck.
#[derive(Debug)]
pub(crate) struct KeyBits([AtomicU64; KEY_CODES / 64]);

impl Default for KeyBits {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicU64::new(0)))
    }
}

impl KeyBits {
    fn set(&self, code: u16, down: bool) {
        let c = usize::from(code);
        if c >= KEY_CODES {
            return;
        }
        let bit = 1u64 << (c % 64);
        if down {
            self.0[c / 64].fetch_or(bit, Ordering::SeqCst);
        } else {
            self.0[c / 64].fetch_and(!bit, Ordering::SeqCst);
        }
    }

    /// Codes marked down, ascending.
    pub(crate) fn down(&self) -> Vec<u16> {
        let mut out = Vec::new();
        for (w, word) in self.0.iter().enumerate() {
            let v = word.load(Ordering::SeqCst);
            for b in 0..64 {
                if v & (1 << b) != 0 {
                    #[allow(clippy::cast_possible_truncation)]
                    out.push((w * 64 + b) as u16);
                }
            }
        }
        out
    }

    /// Records the key events of a frame that was written to the sibling.
    pub(crate) fn apply(&self, frame: &[Event]) {
        for ev in frame.iter().filter(|e| e.kind == EV_KEY) {
            self.set(ev.code, ev.value != 0);
        }
    }

    /// A frame releasing every key marked down (empty if none), and clears
    /// the marks. Used on every exit path and by the emergency release.
    pub(crate) fn take_release_frame(&self) -> Vec<Event> {
        let down = self.down();
        if down.is_empty() {
            return Vec::new();
        }
        let mut frame: Vec<Event> = down.iter().map(|&c| Event::new(EV_KEY, c, 0)).collect();
        frame.push(Event::syn());
        for c in down {
            self.set(c, false);
        }
        frame
    }

    /// After `SYN_DROPPED`: a frame releasing the keys we hold down in the
    /// sibling that are up in the source (`source_state`: the `EVIOCGKEY`
    /// bitmap, bit n in byte n/8). Presses lost in the drop are not replayed.
    pub(crate) fn resync_release_frame(&self, source_state: &[u8]) -> Vec<Event> {
        let up_in_source = |c: u16| {
            let c = usize::from(c);
            source_state
                .get(c / 8)
                .is_none_or(|byte| byte & (1 << (c % 8)) == 0)
        };
        let stale: Vec<u16> = self
            .down()
            .into_iter()
            .filter(|&c| up_in_source(c))
            .collect();
        if stale.is_empty() {
            return Vec::new();
        }
        let mut frame: Vec<Event> = stale.iter().map(|&c| Event::new(EV_KEY, c, 0)).collect();
        frame.push(Event::syn());
        for c in stale {
            self.set(c, false);
        }
        frame
    }
}

/// What the forwarder asks the caller to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Write this frame (ends with `SYN_REPORT`) to the sibling, then
    /// [`KeyBits::apply`] it. `time` is the source frame's kernel time.
    Forward { frame: Vec<Event>, time: Duration },
    /// An event of a caught key (value 1 press, 0 release, 2 repeat), never
    /// forwarded.
    Caught {
        code: u16,
        value: i32,
        time: Duration,
    },
    /// The kernel dropped events: read `EVIOCGKEY` of the source and write
    /// [`KeyBits::resync_release_frame`].
    Resync,
}

/// Frame assembly and the caught-key filter.
#[derive(Debug)]
pub(crate) struct Forwarder {
    frame: Vec<Event>,
    dropping: bool,
    /// `EV_KEY` codes taken out of the stream (ADR 0016: the mic key only;
    /// the list is the extension point for future features).
    catch: Vec<u16>,
}

impl Default for Forwarder {
    fn default() -> Self {
        Self::new(vec![MIC_KEY])
    }
}

impl Forwarder {
    /// A forwarder that takes `catch` out of the stream.
    pub(crate) fn new(catch: Vec<u16>) -> Self {
        Self {
            frame: Vec::new(),
            dropping: false,
            catch,
        }
    }

    /// Feeds one source event.
    pub(crate) fn push(&mut self, ev: Event, out: &mut Vec<Action>) {
        if ev.kind == EV_SYN && ev.code == SYN_DROPPED {
            // Discard the partial frame and everything up to the next report.
            self.frame.clear();
            self.dropping = true;
            return;
        }
        if self.dropping {
            if ev.kind == EV_SYN && ev.code == SYN_REPORT {
                self.dropping = false;
                out.push(Action::Resync);
            }
            return;
        }
        if ev.kind == EV_KEY && self.catch.contains(&ev.code) {
            out.push(Action::Caught {
                code: ev.code,
                value: ev.value,
                time: ev.time,
            });
            return;
        }
        if ev.kind == EV_SYN && ev.code == SYN_REPORT {
            if !self.frame.is_empty() {
                let mut frame = std::mem::take(&mut self.frame);
                frame.push(Event::syn());
                out.push(Action::Forward {
                    frame,
                    time: ev.time,
                });
            }
            return;
        }
        self.frame.push(ev);
    }
}

/// Counts per (type, code) and delay statistics, for the end-of-run log.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    counts: BTreeMap<(u16, u16), u64>,
    delays_us: Vec<u32>,
}

/// At most this many delay samples are kept (about 30 min of pointer frames).
const MAX_DELAYS: usize = 200_000;

impl Stats {
    pub(crate) fn count(&mut self, ev: &Event) {
        *self.counts.entry((ev.kind, ev.code)).or_insert(0) += 1;
    }

    pub(crate) fn delay(&mut self, d: Duration) {
        if self.delays_us.len() < MAX_DELAYS {
            self.delays_us
                .push(u32::try_from(d.as_micros()).unwrap_or(u32::MAX));
        }
    }

    /// (samples, median, max) in microseconds.
    pub(crate) fn delay_summary(&self) -> Option<(usize, u32, u32)> {
        if self.delays_us.is_empty() {
            return None;
        }
        let mut v = self.delays_us.clone();
        v.sort_unstable();
        Some((v.len(), v[v.len() / 2], v[v.len() - 1]))
    }

    pub(crate) fn counts(&self) -> impl Iterator<Item = (&(u16, u16), &u64)> {
        self.counts.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{ABS_X, ABS_Y, EV_ABS, EV_REL};

    fn feed(f: &mut Forwarder, evs: &[Event]) -> Vec<Action> {
        let mut out = Vec::new();
        for &e in evs {
            f.push(e, &mut out);
        }
        out
    }

    fn frames(actions: &[Action]) -> Vec<Vec<Event>> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Forward { frame, .. } => Some(frame.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn only_the_mic_key_is_dropped_everything_else_passes_unchanged() {
        let mut f = Forwarder::default();
        let input = [
            Event::new(EV_ABS, ABS_X, 100),
            Event::new(EV_ABS, ABS_Y, 200),
            Event::syn(),
            Event::new(EV_KEY, MIC_KEY, 1),
            Event::syn(),
            Event::new(EV_REL, 8, -1),
            Event::syn(),
            Event::new(EV_KEY, 103, 1),
            Event::new(EV_KEY, MIC_KEY, 0),
            Event::syn(),
            Event::new(EV_KEY, 115, 2),
            Event::syn(),
            Event::new(EV_KEY, 1199, 1),
            Event::syn(),
        ];
        let out = feed(&mut f, &input);
        let fr = frames(&out);
        // Every non-mic event arrives, in order, framed by SYN_REPORT.
        let passed: Vec<Event> = fr.concat();
        let expected: Vec<Event> = input
            .iter()
            .copied()
            .filter(|e| !(e.kind == EV_KEY && e.code == MIC_KEY))
            .collect();
        // The mic-only frame produced no frame at all, so drop its SYN too.
        let mut exp_no_empty = expected.clone();
        exp_no_empty.remove(3);
        assert_eq!(passed, exp_no_empty);
        assert!(
            passed
                .iter()
                .all(|e| !(e.kind == EV_KEY && e.code == MIC_KEY))
        );
        let mics: Vec<i32> = out
            .iter()
            .filter_map(|a| match a {
                Action::Caught { value, .. } => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(mics, vec![1, 0]);
    }

    #[test]
    fn a_configured_catch_list_takes_exactly_those_keys() {
        let mut f = Forwarder::new(vec![398, 399]);
        let out = feed(
            &mut f,
            &[
                Event::new(EV_KEY, 398, 1),
                Event::new(EV_KEY, MIC_KEY, 1),
                Event::syn(),
            ],
        );
        assert_eq!(
            frames(&out),
            vec![vec![Event::new(EV_KEY, MIC_KEY, 1), Event::syn()]]
        );
        assert!(out.iter().any(|a| matches!(
            a,
            Action::Caught {
                code: 398,
                value: 1,
                ..
            }
        )));
    }

    #[test]
    fn other_codes_near_428_are_not_dropped() {
        let mut f = Forwarder::default();
        let out = feed(
            &mut f,
            &[
                Event::new(EV_KEY, 427, 1),
                Event::new(EV_KEY, 429, 1),
                Event::new(EV_REL, MIC_KEY, 1),
                Event::syn(),
            ],
        );
        assert_eq!(frames(&out)[0].len(), 4);
    }

    #[test]
    fn syn_dropped_discards_until_the_next_report_then_asks_for_a_resync() {
        let mut f = Forwarder::default();
        let out = feed(
            &mut f,
            &[
                Event::new(EV_KEY, 103, 1),
                Event::new(EV_SYN, SYN_DROPPED, 0),
                Event::new(EV_KEY, 103, 0),
                Event::syn(),
                Event::new(EV_KEY, 28, 1),
                Event::syn(),
            ],
        );
        assert_eq!(out[0], Action::Resync);
        assert_eq!(
            frames(&out),
            vec![vec![Event::new(EV_KEY, 28, 1), Event::syn()]]
        );
    }

    #[test]
    fn keys_still_down_are_released_on_exit_and_only_once() {
        let keys = KeyBits::default();
        keys.apply(&[Event::new(EV_KEY, 103, 1), Event::syn()]);
        keys.apply(&[Event::new(EV_KEY, 115, 2), Event::syn()]); // repeat = down
        keys.apply(&[Event::new(EV_KEY, 28, 1), Event::syn()]);
        keys.apply(&[Event::new(EV_KEY, 28, 0), Event::syn()]);
        keys.apply(&[Event::new(EV_KEY, 1199, 1), Event::syn()]);
        let rel = keys.take_release_frame();
        assert_eq!(
            rel,
            vec![
                Event::new(EV_KEY, 103, 0),
                Event::new(EV_KEY, 115, 0),
                Event::new(EV_KEY, 1199, 0),
                Event::syn()
            ]
        );
        assert_eq!(keys.take_release_frame(), Vec::new());
    }

    #[test]
    fn resync_releases_only_keys_that_are_up_in_the_source() {
        let keys = KeyBits::default();
        keys.apply(&[Event::new(EV_KEY, 103, 1), Event::new(EV_KEY, 108, 1)]);
        let mut state = [0u8; 256];
        state[108 / 8] |= 1 << (108 % 8); // 108 still held in the source
        assert_eq!(
            keys.resync_release_frame(&state),
            vec![Event::new(EV_KEY, 103, 0), Event::syn()]
        );
        assert_eq!(keys.down(), vec![108]);
    }

    #[test]
    fn delay_summary_is_median_and_max() {
        let mut s = Stats::default();
        for us in [300, 100, 200, 5000, 400] {
            s.delay(Duration::from_micros(us));
        }
        assert_eq!(s.delay_summary(), Some((5, 300, 5000)));
    }
}
