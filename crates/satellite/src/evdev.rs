//! Passive reader of one key on a Linux input device, for the Magic
//! Remote's mic button (`[remote]`, docs/design/remote-mic-button.md).
//!
//! - The device is found by its **name** (`/sys/class/input/eventN/device/
//!   name`), never by index: `event2` on c3-a and c3-b today (EXP-022), but
//!   the numbering follows the order the nodes were created.
//! - It is opened **read-only and never grabbed** (no `EVIOCGRAB`): every
//!   other reader, LG's compositor first, keeps getting every event. Nothing
//!   is written to the device.
//! - Only `EV_KEY` events of the one code are used: value 1 is a press, 0 a
//!   release; 2 (autorepeat, not seen for this key) is ignored. Every other
//!   event (other keys, the pointer) is dropped unread beyond its type and
//!   code; nothing is logged or kept.
//! - When the device goes away (a read error or the end of the file) a press
//!   still held is reported released, and the device is looked up again,
//!   with a growing pause (1 s doubling to 30 s) and a log line only at the
//!   1st, 2nd, 4th, 8th ... loss in a row. When it is not there, it is
//!   looked up again every few seconds (logged once).
//! - The kernel's own event time is reported with each press and release
//!   (the hold length is measured from it, not from when the thread read
//!   the event).
//! - `SYN_DROPPED` (the kernel's buffer overflowed): the events up to the
//!   next `SYN_REPORT` are discarded and a held key is reported released
//!   ("lost"), so the state is never stuck down; the next press starts
//!   anew. A press while the key is already down (a release that was
//!   missed) is reported as a lost release followed by the press. The
//!   uinput node survives a Bluetooth disconnect of the remote (EXP-022), so
//!   a release that never comes is not detected here: the hold's cap ends
//!   it.
//!
//! Records follow the kernel's layout for this process: a `struct timeval`
//! of two `long`s, then `u16` type, `u16` code and `i32` value (16 bytes for
//! the 32-bit build, 24 for the 64-bit one; 16 bytes seen on c3-b, D5).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::mpsc;

/// Size of one `struct input_event` for this process.
pub(crate) const EVENT_SIZE: usize = 2 * std::mem::size_of::<usize>() + 8;
/// `EV_SYN`, with its codes `SYN_REPORT` and `SYN_DROPPED`.
const EV_SYN: u16 = 0;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
/// `EV_KEY`.
const EV_KEY: u16 = 1;
/// Longest pause between two attempts after the device went away.
const LOSS_PAUSE_MAX: Duration = Duration::from_secs(30);

/// What the reader reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// The device was found by name and opened (again).
    Found,
    /// The key went down, at this kernel event time.
    Pressed(Duration),
    /// The key went up at this kernel event time, or `None`: the release
    /// was lost (the device went away, the kernel dropped events, or a
    /// press came without a release).
    Released(Option<Duration>),
}

/// Where input devices are listed and opened (fixed on the TV, a temporary
/// directory in tests).
#[derive(Debug, Clone)]
pub(crate) struct InputPaths {
    /// `/sys/class/input`.
    pub(crate) sys: PathBuf,
    /// `/dev/input`.
    pub(crate) dev: PathBuf,
}

impl Default for InputPaths {
    fn default() -> Self {
        Self {
            sys: PathBuf::from("/sys/class/input"),
            dev: PathBuf::from("/dev/input"),
        }
    }
}

/// How often the device is looked for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// While it is not there.
    pub(crate) rescan: Duration,
    /// After it went away.
    pub(crate) after_loss: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            rescan: Duration::from_secs(5),
            after_loss: Duration::from_secs(1),
        }
    }
}

/// The `eventN` node whose device name is exactly `name`, the lowest N
/// first. Reads only the `name` attribute of each input device.
pub(crate) fn find_device(paths: &InputPaths, name: &str) -> Option<PathBuf> {
    let mut nodes: Vec<(u32, String)> = std::fs::read_dir(&paths.sys)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let file = e.file_name().into_string().ok()?;
            let n = file.strip_prefix("event")?.parse::<u32>().ok()?;
            Some((n, file))
        })
        .collect();
    nodes.sort_unstable();
    nodes.into_iter().find_map(|(_, file)| {
        let text =
            std::fs::read_to_string(paths.sys.join(&file).join("device").join("name")).ok()?;
        (text.trim_end_matches(['\n', '\r']) == name).then(|| paths.dev.join(&file))
    })
}

/// One record: the kernel's event time, type, code and value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) time: Duration,
    pub(crate) ty: u16,
    pub(crate) code: u16,
    pub(crate) value: i32,
}

/// Parses one record (`struct timeval` of two native `long`s first).
pub(crate) fn parse_event(rec: &[u8]) -> Option<Event> {
    const L: usize = std::mem::size_of::<usize>();
    let rec = rec.get(..EVENT_SIZE)?;
    let long = |b: &[u8]| {
        let mut a = [0_u8; L];
        a.copy_from_slice(b);
        isize::from_ne_bytes(a)
    };
    let sec = u64::try_from(long(&rec[..L])).unwrap_or(0);
    let usec = u32::try_from(long(&rec[L..2 * L]).clamp(0, 999_999)).unwrap_or(0);
    let b = &rec[2 * L..];
    Some(Event {
        time: Duration::new(sec, usec * 1000),
        ty: u16::from_ne_bytes([b[0], b[1]]),
        code: u16::from_ne_bytes([b[2], b[3]]),
        value: i32::from_ne_bytes([b[4], b[5], b[6], b[7]]),
    })
}

/// Key state from the records of one device, sending only changes.
#[derive(Debug)]
struct KeyState {
    code: u16,
    down: bool,
    /// After `SYN_DROPPED`: discard until the next `SYN_REPORT`.
    dropping: bool,
}

impl KeyState {
    fn new(code: u16) -> Self {
        Self {
            code,
            down: false,
            dropping: false,
        }
    }

    fn on_event(&mut self, ev: Event, out: &mut Vec<Key>) {
        if ev.ty == EV_SYN && ev.code == SYN_DROPPED {
            self.dropping = true;
            out.extend(self.lost());
            return;
        }
        if self.dropping {
            if ev.ty == EV_SYN && ev.code == SYN_REPORT {
                self.dropping = false;
            }
            return;
        }
        if ev.ty != EV_KEY || ev.code != self.code {
            return;
        }
        match ev.value {
            1 => {
                // A press while down: the release was missed.
                out.extend(self.lost());
                self.down = true;
                out.push(Key::Pressed(ev.time));
            }
            0 if self.down => {
                self.down = false;
                out.push(Key::Released(Some(ev.time)));
            }
            _ => {}
        }
    }

    /// A held key's release was lost.
    fn lost(&mut self) -> Option<Key> {
        std::mem::take(&mut self.down).then_some(Key::Released(None))
    }
}

/// Pause before looking again after the `n`-th loss in a row (1 s, 2 s,
/// 4 s ... at most [`LOSS_PAUSE_MAX`]), and whether that loss is logged.
fn loss_pause(base: Duration, n: u32) -> (Duration, bool) {
    let shift = n.saturating_sub(1).min(16);
    (
        base.saturating_mul(1 << shift)
            .min(LOSS_PAUSE_MAX.max(base)),
        n.is_power_of_two(),
    )
}

/// Sleeps `d` in short steps; false when `stop` was set meanwhile.
fn nap(stop: &AtomicBool, total: Duration) -> bool {
    let slice = Duration::from_millis(100);
    let mut left = total;
    while !left.is_zero() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let s = left.min(slice);
        std::thread::sleep(s);
        left -= s;
    }
    !stop.load(Ordering::Relaxed)
}

/// Starts the reader thread. Changes of the key go to `tx`; the thread ends
/// when `stop` is set (checked between reads and while waiting for the
/// device; a read that blocks on the device ends with the process) or when
/// the receiver is gone.
pub(crate) fn spawn(
    paths: InputPaths,
    name: String,
    code: u16,
    timing: Timing,
    tx: mpsc::UnboundedSender<Key>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("remote-button".into())
        .spawn(move || run(&paths, &name, code, timing, &tx, &stop))
}

fn run(
    paths: &InputPaths,
    name: &str,
    code: u16,
    timing: Timing,
    tx: &mpsc::UnboundedSender<Key>,
    stop: &AtomicBool,
) {
    let mut missing_logged = false;
    let mut state = KeyState::new(code);
    let mut losses = 0_u32;
    while !stop.load(Ordering::Relaxed) {
        let Some(path) = find_device(paths, name) else {
            if !missing_logged {
                log::info!(
                    "remote button: no input device named {name:?}; looking again every {} s (the remote may not be connected)",
                    timing.rescan.as_secs()
                );
                missing_logged = true;
            }
            if !nap(stop, timing.rescan) {
                return;
            }
            continue;
        };
        missing_logged = false;
        match read_device(&path, name, &mut state, tx, stop, losses == 0) {
            Ended::Stop => return,
            Ended::Lost { why, delivered } => {
                state.dropping = false;
                if let Some(k) = state.lost() {
                    if tx.send(k).is_err() {
                        return;
                    }
                }
                // A device that delivered events was working: start over.
                losses = if delivered {
                    1
                } else {
                    losses.saturating_add(1)
                };
                let (pause, loud) = loss_pause(timing.after_loss, losses);
                if loud {
                    log::info!(
                        "remote button: {} ({why}, {losses} in a row); looking for {name:?} again in {} s",
                        path.display(),
                        pause.as_secs_f32()
                    );
                } else {
                    log::debug!(
                        "remote button: {} ({why}, {losses} in a row)",
                        path.display()
                    );
                }
                if !nap(stop, pause) {
                    return;
                }
            }
        }
    }
}

enum Ended {
    Stop,
    /// The device went away; `delivered`: it gave at least one record.
    Lost {
        why: String,
        delivered: bool,
    },
}

fn read_device(
    path: &Path,
    name: &str,
    state: &mut KeyState,
    tx: &mpsc::UnboundedSender<Key>,
    stop: &AtomicBool,
    loud: bool,
) -> Ended {
    // Read-only, no grab, no write: other readers are not affected.
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return Ended::Lost {
                why: format!("cannot open it: {e}"),
                delivered: false,
            };
        }
    };
    if loud {
        log::info!(
            "remote button: watching {name:?} ({}) for key {}, passive read (no grab)",
            path.display(),
            state.code
        );
    } else {
        log::debug!("remote button: watching {} again", path.display());
    }
    if tx.send(Key::Found).is_err() {
        return Ended::Stop;
    }
    let mut buf = vec![0_u8; EVENT_SIZE * 32];
    let mut have = 0_usize;
    let mut delivered = false;
    let mut keys = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ended::Stop;
        }
        let n = match f.read(&mut buf[have..]) {
            Ok(0) => {
                return Ended::Lost {
                    why: "the device went away".to_owned(),
                    delivered,
                };
            }
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Ended::Lost {
                    why: format!("read failed: {e}"),
                    delivered,
                };
            }
        };
        have += n;
        let whole = have / EVENT_SIZE * EVENT_SIZE;
        delivered |= whole > 0;
        for rec in buf[..whole].chunks_exact(EVENT_SIZE) {
            if let Some(ev) = parse_event(rec) {
                state.on_event(ev, &mut keys);
            }
        }
        for k in keys.drain(..) {
            if tx.send(k).is_err() {
                return Ended::Stop;
            }
        }
        buf.copy_within(whole..have, 0);
        have -= whole;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// One record in this process's layout, at kernel time `ms`.
    pub(crate) fn record_at(ms: u64, ty: u16, code: u16, value: i32) -> Vec<u8> {
        const L: usize = std::mem::size_of::<usize>();
        let sec = isize::try_from(ms / 1000).unwrap();
        let usec = isize::try_from(ms % 1000 * 1000).unwrap();
        let mut r = Vec::with_capacity(EVENT_SIZE);
        r.extend_from_slice(&sec.to_ne_bytes());
        r.extend_from_slice(&usec.to_ne_bytes());
        assert_eq!(r.len(), 2 * L);
        r.extend_from_slice(&ty.to_ne_bytes());
        r.extend_from_slice(&code.to_ne_bytes());
        r.extend_from_slice(&value.to_ne_bytes());
        r
    }

    /// A fake `/sys/class/input` and `/dev/input` with devices `(n, name)`;
    /// each `/dev/input/eventN` is an empty regular file.
    pub(crate) fn fake_inputs(tag: &str, devices: &[(u32, &str)]) -> (PathBuf, InputPaths) {
        let root =
            std::env::temp_dir().join(format!("webos-sat-evdev-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = InputPaths {
            sys: root.join("sys"),
            dev: root.join("dev"),
        };
        std::fs::create_dir_all(&paths.dev).unwrap();
        for (n, name) in devices {
            let d = paths.sys.join(format!("event{n}")).join("device");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
            std::fs::write(paths.dev.join(format!("event{n}")), b"").unwrap();
        }
        // Entries that are not event nodes are skipped.
        std::fs::create_dir_all(paths.sys.join("input3")).unwrap();
        std::fs::create_dir_all(paths.sys.join("mouse0")).unwrap();
        (root, paths)
    }

    #[test]
    fn records_have_the_kernel_layout_for_this_process() {
        assert_eq!(
            EVENT_SIZE,
            if cfg!(target_pointer_width = "64") {
                24
            } else {
                16
            }
        );
        let r = record_at(1_234_567, EV_KEY, 428, 1);
        assert_eq!(
            parse_event(&r),
            Some(Event {
                time: Duration::from_millis(1_234_567),
                ty: 1,
                code: 428,
                value: 1
            })
        );
        assert_eq!(parse_event(&r[..EVENT_SIZE - 1]), None);
    }

    #[test]
    fn the_device_is_found_by_its_exact_name_never_by_index() {
        let (root, paths) = fake_inputs(
            "find",
            &[
                (1, "LGE RCU"),
                (12, "LGE M-RCU - Builtin [0]"),
                (3, "LGE M-RCU - Builtin [1]"),
                (2, "LGE M-RCU - Builtin [0] extra"),
            ],
        );
        assert_eq!(
            find_device(&paths, "LGE M-RCU - Builtin [0]"),
            Some(paths.dev.join("event12"))
        );
        assert_eq!(find_device(&paths, "LGE M-RCU - Builtin"), None);
        assert_eq!(find_device(&paths, ""), None);
        let gone = InputPaths {
            sys: root.join("nope"),
            dev: paths.dev.clone(),
        };
        assert_eq!(find_device(&gone, "LGE RCU"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn ev(ms: u64, ty: u16, code: u16, value: i32) -> Event {
        Event {
            time: Duration::from_millis(ms),
            ty,
            code,
            value,
        }
    }

    fn feed(s: &mut KeyState, evs: &[Event]) -> Vec<Key> {
        let mut out = Vec::new();
        for e in evs {
            s.on_event(*e, &mut out);
        }
        out
    }

    #[test]
    fn only_press_and_release_of_the_one_key_count_with_their_kernel_time() {
        let mut s = KeyState::new(428);
        let t = Duration::from_millis;
        assert!(feed(&mut s, &[ev(1, EV_KEY, 103, 1), ev(2, 2, 428, 1)]).is_empty());
        assert!(
            feed(&mut s, &[ev(3, EV_KEY, 428, 0)]).is_empty(),
            "release without press"
        );
        assert_eq!(
            feed(&mut s, &[ev(10, EV_KEY, 428, 1)]),
            [Key::Pressed(t(10))]
        );
        assert!(
            feed(&mut s, &[ev(20, EV_KEY, 428, 2)]).is_empty(),
            "autorepeat"
        );
        assert_eq!(
            feed(&mut s, &[ev(900, EV_KEY, 428, 0)]),
            [Key::Released(Some(t(900)))]
        );
        assert_eq!(s.lost(), None);
        feed(&mut s, &[ev(1000, EV_KEY, 428, 1)]);
        assert_eq!(s.lost(), Some(Key::Released(None)));
    }

    #[test]
    fn a_missed_release_and_dropped_events_never_leave_the_key_stuck() {
        let mut s = KeyState::new(428);
        let t = Duration::from_millis;
        feed(&mut s, &[ev(10, EV_KEY, 428, 1)]);
        // A second press without a release: the release was missed.
        assert_eq!(
            feed(&mut s, &[ev(500, EV_KEY, 428, 1)]),
            [Key::Released(None), Key::Pressed(t(500))]
        );
        // The kernel dropped events: the held key is released (lost) and
        // everything up to the next SYN_REPORT is discarded.
        assert_eq!(
            feed(
                &mut s,
                &[
                    ev(600, EV_SYN, SYN_DROPPED, 0),
                    ev(601, EV_KEY, 428, 0),
                    ev(602, EV_KEY, 428, 1),
                ]
            ),
            [Key::Released(None)]
        );
        assert!(feed(&mut s, &[ev(603, EV_SYN, SYN_REPORT, 0)]).is_empty());
        // Afterwards the state follows the events again.
        assert_eq!(
            feed(&mut s, &[ev(700, EV_KEY, 428, 1)]),
            [Key::Pressed(t(700))]
        );
        // Dropped while up: nothing to release.
        let mut s = KeyState::new(428);
        assert!(feed(&mut s, &[ev(1, EV_SYN, SYN_DROPPED, 0)]).is_empty());
    }

    #[test]
    fn losses_in_a_row_back_off_and_log_less() {
        let b = Duration::from_secs(1);
        let pauses: Vec<(u64, bool)> = (1..=7)
            .map(|n| {
                let (p, loud) = loss_pause(b, n);
                (p.as_secs(), loud)
            })
            .collect();
        assert_eq!(
            pauses,
            [
                (1, true),
                (2, true),
                (4, false),
                (8, true),
                (16, false),
                (30, false),
                (30, false)
            ]
        );
        assert_eq!(loss_pause(b, 64), (Duration::from_secs(30), true));
    }

    #[test]
    fn the_reader_reports_presses_of_a_fake_device_and_a_loss_as_release() {
        let (root, paths) = fake_inputs("read", &[(0, "LGE RCU"), (2, "LGE M-RCU - Builtin [0]")]);
        let mut bytes = Vec::new();
        for (ms, ty, code, value) in [
            (5, EV_KEY, 103, 1), // Up on the same node: ignored
            (5, 0, 0, 0),        // SYN_REPORT
            (1000, EV_KEY, 428, 1),
            (1000, 0, 0, 0),
            (1800, EV_KEY, 428, 0),
            (1800, 0, 0, 0),
            (3000, EV_KEY, 428, 1), // held when the device goes away
        ] {
            bytes.extend(record_at(ms, ty, code, value));
        }
        // A trailing partial record is ignored.
        bytes.extend_from_slice(&[1, 2, 3]);
        std::fs::write(paths.dev.join("event2"), &bytes).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let h = spawn(
            paths,
            "LGE M-RCU - Builtin [0]".into(),
            428,
            Timing {
                rescan: Duration::from_secs(60),
                after_loss: Duration::from_secs(60),
            },
            tx,
            Arc::clone(&stop),
        )
        .unwrap();
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while got.len() < 5 && std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(k) => got.push(k),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        // The end of the file is the device going away: the held key is
        // released.
        assert_eq!(
            got,
            [
                Key::Found,
                Key::Pressed(Duration::from_millis(1000)),
                Key::Released(Some(Duration::from_millis(1800))),
                Key::Pressed(Duration::from_millis(3000)),
                Key::Released(None)
            ]
        );
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_device_is_looked_for_until_stopped() {
        let (root, paths) = fake_inputs("missing", &[(0, "LGE RCU")]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let h = spawn(
            paths,
            "LGE M-RCU - Builtin [0]".into(),
            428,
            Timing::default(),
            tx,
            Arc::clone(&stop),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
