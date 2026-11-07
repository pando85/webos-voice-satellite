//! `struct input_event` records in this process's layout.
//!
//! The record starts with a `struct timeval` of two native `long`s, then
//! `u16 type`, `u16 code`, `i32 value`: 16 bytes for the 32-bit ARM build the
//! TV runs (the 64-bit kernel translates), 24 bytes on a 64-bit host.

use std::time::Duration;

/// Size of one record for this process.
pub(crate) const EVENT_SIZE: usize = 2 * std::mem::size_of::<usize>() + 8;

pub(crate) const EV_SYN: u16 = 0;
pub(crate) const EV_KEY: u16 = 1;
pub(crate) const EV_REL: u16 = 2;
pub(crate) const EV_ABS: u16 = 3;
pub(crate) const SYN_REPORT: u16 = 0;
pub(crate) const SYN_DROPPED: u16 = 3;
pub(crate) const ABS_X: u16 = 0;
pub(crate) const ABS_Y: u16 = 1;

/// One input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Event {
    /// Kernel event time (CLOCK_REALTIME by default), zero for events we create.
    pub(crate) time: Duration,
    pub(crate) kind: u16,
    pub(crate) code: u16,
    pub(crate) value: i32,
}

impl Event {
    /// An event with a zero time (the kernel stamps injected events itself).
    pub(crate) const fn new(kind: u16, code: u16, value: i32) -> Self {
        Self {
            time: Duration::ZERO,
            kind,
            code,
            value,
        }
    }

    /// `SYN_REPORT`.
    pub(crate) const fn syn() -> Self {
        Self::new(EV_SYN, SYN_REPORT, 0)
    }
}

/// Parses one record; `None` if `rec` is not exactly [`EVENT_SIZE`] bytes.
pub(crate) fn parse(rec: &[u8]) -> Option<Event> {
    const L: usize = std::mem::size_of::<usize>();
    if rec.len() != EVENT_SIZE {
        return None;
    }
    let long = |at: usize| -> u64 {
        let mut b = [0u8; 8];
        b[..L].copy_from_slice(&rec[at..at + L]);
        u64::from_ne_bytes(b)
    };
    let sec = long(0);
    let usec = long(L).min(999_999);
    let kind = u16::from_ne_bytes([rec[2 * L], rec[2 * L + 1]]);
    let code = u16::from_ne_bytes([rec[2 * L + 2], rec[2 * L + 3]]);
    let value = i32::from_ne_bytes([
        rec[2 * L + 4],
        rec[2 * L + 5],
        rec[2 * L + 6],
        rec[2 * L + 7],
    ]);
    #[allow(clippy::cast_possible_truncation)]
    let time = Duration::new(sec, (usec * 1000) as u32);
    Some(Event {
        time,
        kind,
        code,
        value,
    })
}

/// Encodes events into consecutive records (time fields zero).
pub(crate) fn encode(events: &[Event]) -> Vec<u8> {
    const L: usize = std::mem::size_of::<usize>();
    let mut out = vec![0u8; events.len() * EVENT_SIZE];
    for (i, ev) in events.iter().enumerate() {
        let r = &mut out[i * EVENT_SIZE..(i + 1) * EVENT_SIZE];
        r[2 * L..2 * L + 2].copy_from_slice(&ev.kind.to_ne_bytes());
        r[2 * L + 2..2 * L + 4].copy_from_slice(&ev.code.to_ne_bytes());
        r[2 * L + 4..2 * L + 8].copy_from_slice(&ev.value.to_ne_bytes());
    }
    out
}

/// Short name of an event type for the log.
pub(crate) fn type_name(kind: u16) -> &'static str {
    match kind {
        EV_SYN => "SYN",
        EV_KEY => "KEY",
        EV_REL => "REL",
        EV_ABS => "ABS",
        4 => "MSC",
        _ => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_then_parse_round_trips_type_code_value() {
        let evs = [
            Event::new(EV_KEY, 103, 1),
            Event::new(EV_ABS, ABS_X, -5),
            Event::syn(),
        ];
        let bytes = encode(&evs);
        assert_eq!(bytes.len(), 3 * EVENT_SIZE);
        let back: Vec<Event> = bytes.chunks(EVENT_SIZE).filter_map(parse).collect();
        assert_eq!(back, evs);
    }

    #[test]
    fn parse_reads_the_kernel_time_and_rejects_a_wrong_size() {
        const L: usize = std::mem::size_of::<usize>();
        let mut rec = encode(&[Event::new(EV_KEY, 428, 1)]);
        rec[..L].copy_from_slice(&7usize.to_ne_bytes());
        rec[L..2 * L].copy_from_slice(&250_000usize.to_ne_bytes());
        let ev = parse(&rec).unwrap_or_else(|| panic!("record"));
        assert_eq!(ev.time, Duration::from_millis(7250));
        assert!(parse(&rec[1..]).is_none());
    }
}
