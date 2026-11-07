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
//!   still held is reported released, and the device is looked up again.
//!   When it is not there, it is looked up again every few seconds.
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
/// `EV_KEY`.
const EV_KEY: u16 = 1;

/// What the reader reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// The device was found by name and opened (again).
    Found,
    Pressed,
    Released,
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

/// Type, code and value of one record.
pub(crate) fn parse_event(rec: &[u8]) -> Option<(u16, u16, i32)> {
    let off = EVENT_SIZE - 8;
    let b = rec.get(off..EVENT_SIZE)?;
    Some((
        u16::from_ne_bytes([b[0], b[1]]),
        u16::from_ne_bytes([b[2], b[3]]),
        i32::from_ne_bytes([b[4], b[5], b[6], b[7]]),
    ))
}

/// Key state from the records of one device, sending only changes.
#[derive(Debug)]
struct KeyState {
    code: u16,
    down: bool,
}

impl KeyState {
    fn on_event(&mut self, ty: u16, code: u16, value: i32) -> Option<Key> {
        if ty != EV_KEY || code != self.code {
            return None;
        }
        match value {
            1 if !self.down => {
                self.down = true;
                Some(Key::Pressed)
            }
            0 if self.down => {
                self.down = false;
                Some(Key::Released)
            }
            _ => None,
        }
    }

    /// The device went away: a held key is released.
    fn lost(&mut self) -> Option<Key> {
        std::mem::take(&mut self.down).then_some(Key::Released)
    }
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
    let mut state = KeyState { code, down: false };
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
        match read_device(&path, name, &mut state, tx, stop) {
            Ended::Stop => return,
            Ended::Lost(why) => {
                if let Some(k) = state.lost() {
                    if tx.send(k).is_err() {
                        return;
                    }
                }
                log::info!(
                    "remote button: {} ({why}); looking for {name:?} again",
                    path.display()
                );
                if !nap(stop, timing.after_loss) {
                    return;
                }
            }
        }
    }
}

enum Ended {
    Stop,
    Lost(String),
}

fn read_device(
    path: &Path,
    name: &str,
    state: &mut KeyState,
    tx: &mpsc::UnboundedSender<Key>,
    stop: &AtomicBool,
) -> Ended {
    // Read-only, no grab, no write: other readers are not affected.
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return Ended::Lost(format!("cannot open it: {e}")),
    };
    log::info!(
        "remote button: watching {name:?} ({}) for key {}, passive read (no grab)",
        path.display(),
        state.code
    );
    if tx.send(Key::Found).is_err() {
        return Ended::Stop;
    }
    let mut buf = vec![0_u8; EVENT_SIZE * 32];
    let mut have = 0_usize;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ended::Stop;
        }
        let n = match f.read(&mut buf[have..]) {
            Ok(0) => return Ended::Lost("the device went away".to_owned()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Ended::Lost(format!("read failed: {e}")),
        };
        have += n;
        let whole = have / EVENT_SIZE * EVENT_SIZE;
        for rec in buf[..whole].chunks_exact(EVENT_SIZE) {
            let Some((ty, code, value)) = parse_event(rec) else {
                continue;
            };
            if let Some(k) = state.on_event(ty, code, value) {
                if tx.send(k).is_err() {
                    return Ended::Stop;
                }
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

    /// One record in this process's layout.
    pub(crate) fn record(ty: u16, code: u16, value: i32) -> Vec<u8> {
        let mut r = vec![0_u8; EVENT_SIZE - 8];
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
        let r = record(EV_KEY, 428, 1);
        assert_eq!(parse_event(&r), Some((1, 428, 1)));
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

    #[test]
    fn only_press_and_release_of_the_one_key_count() {
        let mut s = KeyState {
            code: 428,
            down: false,
        };
        assert_eq!(s.on_event(EV_KEY, 103, 1), None);
        assert_eq!(s.on_event(2, 428, 1), None);
        assert_eq!(s.on_event(EV_KEY, 428, 0), None, "release without press");
        assert_eq!(s.on_event(EV_KEY, 428, 1), Some(Key::Pressed));
        assert_eq!(s.on_event(EV_KEY, 428, 1), None, "no second press");
        assert_eq!(s.on_event(EV_KEY, 428, 2), None, "autorepeat");
        assert_eq!(s.on_event(EV_KEY, 428, 0), Some(Key::Released));
        assert_eq!(s.lost(), None);
        s.on_event(EV_KEY, 428, 1);
        assert_eq!(s.lost(), Some(Key::Released));
    }

    #[test]
    fn the_reader_reports_presses_of_a_fake_device_and_a_loss_as_release() {
        let (root, paths) = fake_inputs("read", &[(0, "LGE RCU"), (2, "LGE M-RCU - Builtin [0]")]);
        let mut bytes = Vec::new();
        for (ty, code, value) in [
            (EV_KEY, 103, 1), // Up on the same node: ignored
            (0, 0, 0),        // SYN_REPORT
            (EV_KEY, 428, 1),
            (0, 0, 0),
            (EV_KEY, 428, 0),
            (0, 0, 0),
            (EV_KEY, 428, 1), // held when the device goes away
        ] {
            bytes.extend(record(ty, code, value));
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
                Key::Pressed,
                Key::Released,
                Key::Pressed,
                Key::Released
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
