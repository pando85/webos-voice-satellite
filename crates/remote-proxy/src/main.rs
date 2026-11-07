//! `remote-proxy`: the EXP-026 Part B test tool (docs/experiments/EXP-026-mic-key-grab-reinject.md).
//!
//! It takes the Magic Remote's input node (`EVIOCGRAB`) and writes every event
//! except the mic key (`EV_KEY` 428) into a sibling node the compositor already
//! reads, so LG's compositor never sees the mic key. Modes:
//!
//! - `dry-run`: read the source only (no grab, no write), count events;
//! - `inject-test`: write one pointer move (screen centre) into the target;
//! - `grab`: grab the source and re-send to the target without the mic key;
//! - `clone-probe`: create a uinput copy of the source (no grab, no events)
//!   and report whether `surface-manager` opens it.
//!
//! Nodes are found by exact name, never by `eventN`. The source and target
//! must have identical capabilities and absolute ranges, or nothing runs.
//!
//! Dead-man (all always on): a hard cap (`--secs`, at most 60, honoured by the
//! loop, with `alarm()` 2 s later as a backstop that kills the process); a
//! watchdog thread that releases held keys and `_exit`s when the loop has not
//! turned for over 1 s; an exit on SIGHUP, SIGINT, SIGTERM, stdin end of file
//! or a `q` line on stdin; and, outside the binary, `timeout -s KILL`.
//! Every exit releases the keys still down in the target; the kernel releases
//! the grab and destroys a uinput device when the process ends.
//!
//! Output: event types and codes, counts and delay statistics only.

mod caps;
mod deadman;
mod event;
mod filter;
mod sys;

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use caps::{Caps, Node};
use deadman::{CHECK_EVERY, Cap, Heartbeat, STALL_LIMIT};
use event::{ABS_X, ABS_Y, EV_ABS, EV_KEY, EV_SYN, EVENT_SIZE, Event, SYN_REPORT};
use filter::{Action, Forwarder, KeyBits, MIC_KEY, Stats};

const EXIT_REFUSED: i32 = 2;
const EXIT_WATCHDOG: i32 = 3;
const EXIT_PANIC: i32 = 4;
const EXIT_DEVICE_LOST: i32 = 5;

const USAGE: &str = "usage:
  remote-proxy dry-run     --source NAME [--secs N]
  remote-proxy inject-test --target NAME [--secs N]
  remote-proxy grab        --source NAME --target NAME [--secs N]
  remote-proxy clone-probe --source NAME [--secs N]
NAME is an exact input device name, e.g. \"LGE M-RCU - Builtin [0]\".
--secs: 1 to 60 (defaults: dry-run 30, inject-test 5, grab 60, clone-probe 20).
Stops at the cap, on SIGHUP/SIGINT/SIGTERM, on stdin end of file, or on a line with q.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    DryRun,
    InjectTest,
    Grab,
    CloneProbe,
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    mode: Mode,
    source: Option<String>,
    target: Option<String>,
    secs: u64,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mode = match argv.first().map(String::as_str) {
        Some("dry-run") => Mode::DryRun,
        Some("inject-test") => Mode::InjectTest,
        Some("grab") => Mode::Grab,
        Some("clone-probe") => Mode::CloneProbe,
        _ => return Err(USAGE.into()),
    };
    let mut source = None;
    let mut target = None;
    let mut secs = None;
    let mut it = argv[1..].iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--source" => source = Some(val()?),
            "--target" => target = Some(val()?),
            "--secs" => {
                let v = val()?;
                secs = Some(v.parse::<u64>().map_err(|_| format!("bad --secs {v:?}"))?);
            }
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    let default = match mode {
        Mode::DryRun => 30,
        Mode::InjectTest => 5,
        Mode::Grab => 60,
        Mode::CloneProbe => 20,
    };
    let secs = deadman::cap_secs(secs.unwrap_or(default))?;
    let needs_source = matches!(mode, Mode::DryRun | Mode::Grab | Mode::CloneProbe);
    let needs_target = matches!(mode, Mode::InjectTest | Mode::Grab);
    if needs_source && source.is_none() {
        return Err("--source is required".into());
    }
    if needs_target && target.is_none() {
        return Err("--target is required".into());
    }
    if !needs_target && target.is_some() {
        return Err("--target is not used by this mode".into());
    }
    if mode == Mode::InjectTest && source.is_some() {
        return Err("--source is not used by inject-test".into());
    }
    if source.is_some() && source == target {
        return Err("--source and --target must differ".into());
    }
    Ok(Args {
        mode,
        source,
        target,
        secs,
    })
}

/// What the watchdog and the panic hook release before `_exit`.
struct Emergency {
    target: Option<File>,
    keys: Arc<KeyBits>,
}

static EMERGENCY: OnceLock<Emergency> = OnceLock::new();

/// Releases the keys still down in the target (best effort) and ends the
/// process at once; the kernel then releases the grab.
fn emergency_exit(code: i32, why: &str) -> ! {
    sys::raw_stderr(&format!(
        "remote-proxy: {why}; releasing keys and exiting\n"
    ));
    if let Some(e) = EMERGENCY.get()
        && let Some(t) = &e.target
    {
        let rel = e.keys.take_release_frame();
        if !rel.is_empty() {
            let mut w: &File = t;
            let _ = w.write(&event::encode(&rel));
        }
    }
    sys::exit_now(code)
}

/// Why the loop woke.
enum Wake {
    Source,
    Tick,
    Stop(String),
}

/// The in-binary dead-man for one run.
struct DeadMan {
    cap: Cap,
    hb: Arc<Heartbeat>,
    sig: File,
    stdin_open: bool,
}

impl DeadMan {
    fn start(secs: u64) -> Result<Self, String> {
        sys::ignore_tty_stops();
        if sys::stdin_tty_foreground() == Some(false) {
            return Err(
                "stdin is a terminal but this process is not in its foreground \
                        group (a backgrounding wrapper such as coreutils `timeout` without \
                        --foreground?): the hangup dead-man would not reach it; refusing"
                    .into(),
            );
        }
        let now = Instant::now();
        let sig = sys::signal_fd().map_err(|e| format!("signalfd: {e}"))?;
        sys::alarm(Cap::alarm_secs(secs));
        let hb = Heartbeat::new(now);
        hb.beat();
        deadman::spawn_watchdog(Arc::clone(&hb), STALL_LIMIT, CHECK_EVERY, |age| {
            emergency_exit(
                EXIT_WATCHDOG,
                &format!("watchdog: loop stalled for {} ms", age.as_millis()),
            )
        })
        .map_err(|e| format!("cannot start the watchdog: {e}"))?;
        std::panic::set_hook(Box::new(|info| {
            emergency_exit(EXIT_PANIC, &format!("panic: {info}"));
        }));
        println!(
            "dead-man: cap {secs} s (alarm at {} s), watchdog {} ms, exit on SIGHUP/SIGINT/SIGTERM, stdin EOF or q",
            Cap::alarm_secs(secs),
            STALL_LIMIT.as_millis()
        );
        Ok(Self {
            cap: Cap::new(now, secs),
            hb,
            sig,
            stdin_open: true,
        })
    }

    /// Waits up to 200 ms for the source (if any), a signal or stdin.
    fn wait(&mut self, source: Option<RawFd>) -> Wake {
        let mut fds = vec![self.sig.as_raw_fd()];
        if self.stdin_open {
            fds.push(0);
        }
        if let Some(s) = source {
            fds.push(s);
        }
        let ready = match sys::poll_readable(&fds, 200) {
            Ok(r) => r,
            Err(e) => return Wake::Stop(format!("poll failed: {e}")),
        };
        self.hb.beat();
        if self.cap.expired(Instant::now()) {
            return Wake::Stop("cap reached".into());
        }
        if ready[0] {
            return match sys::read_signal(&self.sig) {
                Ok(n) => Wake::Stop(format!("signal {n}")),
                Err(e) => Wake::Stop(format!("signalfd read: {e}")),
            };
        }
        if self.stdin_open && ready[1] {
            let mut buf = [0u8; 256];
            match io::stdin().lock().read(&mut buf) {
                Ok(0) => return Wake::Stop("stdin end of file".into()),
                Ok(n) if buf[..n].contains(&b'q') => return Wake::Stop("q on stdin".into()),
                Ok(_) => {}
                Err(e) => return Wake::Stop(format!("stdin: {e}")),
            }
        }
        if source.is_some() && *ready.last().unwrap_or(&false) {
            Wake::Source
        } else {
            Wake::Tick
        }
    }
}

fn open_read(dev: &Path) -> Result<File, String> {
    File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(dev)
        .map_err(|e| format!("open {}: {e}", dev.display()))
}

fn open_write(dev: &Path) -> Result<File, String> {
    File::options()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(dev)
        .map_err(|e| format!("open {}: {e}", dev.display()))
}

/// Reads the available events; `Err` when the node went away.
fn read_events(f: &File, out: &mut Vec<Event>) -> Result<(), String> {
    let mut buf = vec![0u8; 64 * EVENT_SIZE];
    let mut r: &File = f;
    match r.read(&mut buf) {
        Ok(0) => Err("source node closed".into()),
        Ok(n) => {
            out.extend(buf[..n].chunks_exact(EVENT_SIZE).filter_map(event::parse));
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(()),
        Err(e) => Err(format!("source read: {e}")),
    }
}

fn now_realtime() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn sys_root() -> PathBuf {
    PathBuf::from(caps::SYS_INPUT)
}

/// Reads a node's capabilities with `EVIOCGBIT` on an open fd (any mode).
fn read_caps(root: &Path, node: &Node, f: &File) -> Result<Caps, String> {
    let get = |ev: u16| {
        sys::ev_bits(f, ev)
            .map(|b| caps::codes(&b))
            .map_err(|e| format!("EVIOCGBIT({ev}) on {}: {e}", node.event))
    };
    Ok(Caps {
        ev: get(0)?,
        key: get(EV_KEY)?,
        rel: get(event::EV_REL)?,
        abs: get(EV_ABS)?,
        msc: get(4)?,
        id: caps::read_id(root, node),
    })
}

fn describe(node: &Node, c: &Caps) {
    println!(
        "{} {:?}: id {:04x}:{:04x}:{:04x}:{:04x} ev {:?} rel {:?} abs {:?} msc {:?} keys {} has_428 {}",
        node.event,
        node.name,
        c.id[0],
        c.id[1],
        c.id[2],
        c.id[3],
        c.ev,
        c.rel,
        c.abs,
        c.msc,
        c.key.len(),
        c.key.contains(&MIC_KEY)
    );
}

fn log_abs(label: &str, f: &File, axes: &[u16]) -> Result<Vec<sys::AbsInfo>, String> {
    axes.iter()
        .map(|&a| {
            let i = sys::abs_info(f, a).map_err(|e| format!("EVIOCGABS({a}) on {label}: {e}"))?;
            println!(
                "{label} abs {a}: min {} max {} fuzz {} flat {} res {}",
                i.minimum, i.maximum, i.fuzz, i.flat, i.resolution
            );
            Ok(i)
        })
        .collect()
}

/// Same range, fuzz, flat and resolution (the current value may differ).
fn same_ranges(a: &[sys::AbsInfo], b: &[sys::AbsInfo]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            (x.minimum, x.maximum, x.fuzz, x.flat, x.resolution)
                == (y.minimum, y.maximum, y.fuzz, y.flat, y.resolution)
        })
}

fn print_summary(stats: &Stats, label: &str, mics: u32, reason: &str) {
    println!("end: {reason}");
    for ((kind, code), n) in stats.counts() {
        println!("count {} {code}: {n}", event::type_name(*kind));
    }
    println!("mic key events: {mics}");
    match stats.delay_summary() {
        Some((n, med, max)) => println!("{label} (us): frames {n} median {med} max {max}"),
        None => println!("{label}: no frames"),
    }
}

fn log_mic(start: Duration, value: i32, time: Duration, pressed_at: &mut Option<Duration>) {
    let rel = time.saturating_sub(start).as_millis();
    match value {
        1 => {
            *pressed_at = Some(time);
            println!("mic 428 press at +{rel} ms");
        }
        0 => {
            let held = pressed_at
                .take()
                .map(|p| time.saturating_sub(p).as_millis());
            println!("mic 428 release at +{rel} ms (held {held:?} ms)");
        }
        v => println!("mic 428 value {v} at +{rel} ms"),
    }
}

fn dry_run(a: &Args) -> Result<i32, String> {
    let root = sys_root();
    let src = caps::find(&root, a.source.as_deref().unwrap_or_default())?;
    let f = open_read(&src.dev)?;
    describe(&src, &read_caps(&root, &src, &f)?);
    let mut dm = DeadMan::start(a.secs)?;
    println!("dry-run: reading {} (no grab, no write)", src.event);
    let start = now_realtime();
    let (mut stats, mut mics, mut pressed) = (Stats::default(), 0u32, None);
    let mut evs = Vec::new();
    let reason = loop {
        match dm.wait(Some(f.as_raw_fd())) {
            Wake::Stop(r) => break r,
            Wake::Tick => {}
            Wake::Source => {
                evs.clear();
                if let Err(e) = read_events(&f, &mut evs) {
                    print_summary(&stats, "read delay", mics, &e);
                    return Ok(EXIT_DEVICE_LOST);
                }
                for ev in &evs {
                    stats.count(ev);
                    if ev.kind == EV_KEY && ev.code == MIC_KEY {
                        mics += 1;
                        log_mic(start, ev.value, ev.time, &mut pressed);
                    }
                    if ev.kind == EV_SYN && ev.code == SYN_REPORT {
                        stats.delay(now_realtime().saturating_sub(ev.time));
                    }
                }
            }
        }
    };
    print_summary(&stats, "read delay", mics, &reason);
    Ok(0)
}

#[allow(clippy::many_single_char_names)] // short names in one short function
fn inject_test(a: &Args) -> Result<i32, String> {
    let root = sys_root();
    let tgt = caps::find(&root, a.target.as_deref().unwrap_or_default())?;
    let f = open_write(&tgt.dev)?;
    let c = read_caps(&root, &tgt, &f)?;
    describe(&tgt, &c);
    if !c.abs.contains(&ABS_X) || !c.abs.contains(&ABS_Y) {
        return Err(format!("{} has no ABS_X/ABS_Y", tgt.event));
    }
    let _dm = DeadMan::start(a.secs)?;
    let ax = log_abs("target", &f, &[ABS_X, ABS_Y])?;
    let mid = |i: &sys::AbsInfo| i.minimum + (i.maximum - i.minimum) / 2;
    let frame = [
        Event::new(EV_ABS, ABS_X, mid(&ax[0])),
        Event::new(EV_ABS, ABS_Y, mid(&ax[1])),
        Event::syn(),
    ];
    let mut w: &File = &f;
    let n = w
        .write(&event::encode(&frame))
        .map_err(|e| format!("write {}: {e}", tgt.event))?;
    println!(
        "inject-test: wrote {n} bytes (ABS_X {} ABS_Y {} SYN) to {}",
        mid(&ax[0]),
        mid(&ax[1]),
        tgt.event
    );
    Ok(0)
}

// One linear sequence (checks, grab, loop, release) reads best as one function.
#[allow(clippy::too_many_lines)]
fn grab(a: &Args) -> Result<i32, String> {
    let root = sys_root();
    let src = caps::find(&root, a.source.as_deref().unwrap_or_default())?;
    let tgt = caps::find(&root, a.target.as_deref().unwrap_or_default())?;
    if src.event == tgt.event {
        return Err("source and target are the same node".into());
    }
    let sf = open_read(&src.dev)?;
    let tf = open_write(&tgt.dev)?;
    let (sc, tc) = (read_caps(&root, &src, &sf)?, read_caps(&root, &tgt, &tf)?);
    describe(&src, &sc);
    describe(&tgt, &tc);
    if let Some(d) = sc.first_difference(&tc) {
        return Err(format!("capabilities differ ({d}); refusing"));
    }
    if !sc.key.contains(&MIC_KEY) {
        return Err("the source has no key 428; refusing".into());
    }
    let axes = sc.abs.clone();
    if !same_ranges(
        &log_abs("source", &sf, &axes)?,
        &log_abs("target", &tf, &axes)?,
    ) {
        return Err("absolute ranges differ; refusing".into());
    }
    let keys = Arc::new(KeyBits::default());
    let emergency_target = tf.try_clone().map_err(|e| format!("dup target: {e}"))?;
    let _ = EMERGENCY.set(Emergency {
        target: Some(emergency_target),
        keys: Arc::clone(&keys),
    });
    let mut dm = DeadMan::start(a.secs)?;

    // Grab only with no key down on the source (at most 2 s), so the
    // compositor is never left with a key it saw go down and never up.
    let wait_until = Instant::now() + Duration::from_secs(2);
    loop {
        let st = sys::key_state(&sf).map_err(|e| format!("EVIOCGKEY: {e}"))?;
        if st.iter().all(|&b| b == 0) {
            break;
        }
        if Instant::now() >= wait_until {
            return Err("a key stayed down for 2 s; not grabbing".into());
        }
        if let Wake::Stop(r) = dm.wait(None) {
            return Err(format!("stopped before the grab: {r}"));
        }
    }
    sys::grab(&sf, true).map_err(|e| format!("EVIOCGRAB: {e}"))?;
    println!(
        "grab: {} -> {} (all events except KEY 428)",
        src.event, tgt.event
    );

    let start = now_realtime();
    let (mut stats, mut mics, mut pressed) = (Stats::default(), 0u32, None);
    let mut fwd = Forwarder::default();
    let (mut evs, mut actions) = (Vec::new(), Vec::new());
    let mut code = 0;
    let write_frame = |frame: &[Event], stats: &mut Stats, time: Option<Duration>| {
        let bytes = event::encode(frame);
        let mut w: &File = &tf;
        match w.write(&bytes) {
            Ok(n) if n == bytes.len() => {
                keys.apply(frame);
                if let Some(t) = time {
                    stats.delay(now_realtime().saturating_sub(t));
                }
                Ok(())
            }
            Ok(n) => Err(format!("short write to target: {n} of {}", bytes.len())),
            Err(e) => Err(format!("target write: {e}")),
        }
    };
    let reason = 'run: loop {
        match dm.wait(Some(sf.as_raw_fd())) {
            Wake::Stop(r) => break r,
            Wake::Tick => {}
            Wake::Source => {
                evs.clear();
                if let Err(e) = read_events(&sf, &mut evs) {
                    code = EXIT_DEVICE_LOST;
                    break e;
                }
                actions.clear();
                for &ev in &evs {
                    stats.count(&ev);
                    fwd.push(ev, &mut actions);
                }
                for act in actions.drain(..) {
                    match act {
                        Action::Forward { frame, time } => {
                            if let Err(e) = write_frame(&frame, &mut stats, Some(time)) {
                                code = EXIT_REFUSED;
                                break 'run e;
                            }
                        }
                        Action::Mic { value, time } => {
                            mics += 1;
                            log_mic(start, value, time, &mut pressed);
                            #[cfg(debug_assertions)]
                            test_stall_on_mic();
                        }
                        Action::Resync => {
                            println!("SYN_DROPPED: resyncing key state");
                            let st = sys::key_state(&sf).unwrap_or([0; 256]);
                            let rel = keys.resync_release_frame(&st);
                            if !rel.is_empty() {
                                let _ = write_frame(&rel, &mut stats, None);
                            }
                        }
                    }
                }
            }
        }
    };
    // Normal end: release what is still down in the target, then ungrab.
    let rel = keys.take_release_frame();
    if !rel.is_empty() {
        let codes: Vec<u16> = rel
            .iter()
            .filter(|e| e.kind == EV_KEY)
            .map(|e| e.code)
            .collect();
        println!("releasing keys still down in the target: {codes:?}");
        let mut w: &File = &tf;
        let _ = w.write(&event::encode(&rel));
    }
    let _ = sys::grab(&sf, false);
    print_summary(&stats, "frame delay", mics, &reason);
    Ok(code)
}

/// Debug builds only (never in the release binary for the TV): with
/// `REMOTE_PROXY_TEST_STALL_MS` set, a mic event blocks the loop for that long,
/// so the host test can check that the watchdog releases held keys and ends the
/// process (`crates/remote-proxy/tests/host_uinput.py`).
#[cfg(debug_assertions)]
fn test_stall_on_mic() {
    if let Some(ms) = std::env::var("REMOTE_PROXY_TEST_STALL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// The pid of a process by `comm`, from `/proc`.
fn pid_of(comm: &str) -> Option<u32> {
    std::fs::read_dir("/proc").ok()?.flatten().find_map(|e| {
        let pid: u32 = e.file_name().to_str()?.parse().ok()?;
        let c = std::fs::read_to_string(e.path().join("comm")).ok()?;
        (c.trim() == comm).then_some(pid)
    })
}

/// Whether process `pid` holds `dev` open.
fn holds(pid: u32, dev: &Path) -> bool {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| std::fs::read_link(e.path()).is_ok_and(|l| l == dev))
}

fn clone_probe(a: &Args) -> Result<i32, String> {
    let root = sys_root();
    let src = caps::find(&root, a.source.as_deref().unwrap_or_default())?;
    let sf = open_read(&src.dev)?;
    let sc = read_caps(&root, &src, &sf)?;
    describe(&src, &sc);
    let axes = sc.abs.clone();
    let infos = log_abs("source", &sf, &axes)?;
    let u = open_write(Path::new("/dev/uinput"))?;
    let set = |req: u32, bits: Vec<u16>| -> Result<(), String> {
        bits.into_iter().try_for_each(|b| {
            sys::ui_set_bit(&u, req, b).map_err(|e| format!("UI_SET bit {b}: {e}"))
        })
    };
    // Never EV_REP (20): lginput2 sends its own repeats (research note section 2).
    let ev: Vec<u16> = sc
        .ev
        .iter()
        .copied()
        .filter(|&b| b != 0 && b != 20)
        .collect();
    set(sys::UI_SET_EVBIT, ev)?;
    set(sys::UI_SET_KEYBIT, sc.key.clone())?;
    set(sys::UI_SET_RELBIT, sc.rel.clone())?;
    set(sys::UI_SET_ABSBIT, axes.clone())?;
    for (&ax, &info) in axes.iter().zip(&infos) {
        sys::ui_abs_setup(&u, ax, info).map_err(|e| format!("UI_ABS_SETUP {ax}: {e}"))?;
    }
    let id = sc.id;
    let mut dm = DeadMan::start(a.secs)?;
    sys::ui_create(&u, &src.name, id).map_err(|e| format!("UI_DEV_CREATE: {e}"))?;
    let created = Instant::now();
    println!(
        "clone-probe: created a uinput copy of {:?} (no grab, no events)",
        src.name
    );
    let compositor = pid_of("surface-manager");
    let (mut node, mut seen) = (None::<Node>, None::<u128>);
    let reason = loop {
        match dm.wait(None) {
            Wake::Stop(r) => break r,
            Wake::Tick | Wake::Source => {}
        }
        if node.is_none() {
            node = caps::list(&root)
                .into_iter()
                .find(|n| n.name == src.name && n.event != src.event);
            if let Some(n) = &node {
                println!(
                    "clone node: {} after {} ms",
                    n.event,
                    created.elapsed().as_millis()
                );
            }
        }
        if let (Some(n), Some(pid), None) = (&node, compositor, seen) {
            if holds(pid, &n.dev) {
                let ms = created.elapsed().as_millis();
                seen = Some(ms);
                println!("surface-manager opened {} after {ms} ms", n.event);
            }
        }
    };
    let _ = sys::ui_destroy(&u);
    println!("end: {reason}");
    match (compositor, seen) {
        (None, _) => println!("result: surface-manager not found"),
        (Some(_), Some(ms)) => println!("result: opened by surface-manager after {ms} ms"),
        (Some(_), None) => println!("result: NOT opened by surface-manager"),
    }
    Ok(0)
}

fn run() -> Result<i32, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = parse_args(&argv)?;
    match a.mode {
        Mode::DryRun => dry_run(&a),
        Mode::InjectTest => inject_test(&a),
        Mode::Grab => grab(&a),
        Mode::CloneProbe => clone_probe(&a),
    }
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("remote-proxy: {e}");
            std::process::exit(EXIT_REFUSED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Result<Args, String> {
        parse_args(&s.iter().map(|x| (*x).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn grab_needs_both_names_and_they_must_differ() {
        let a = args(&["grab", "--source", "A", "--target", "B"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.secs, 60);
        assert!(args(&["grab", "--source", "A"]).is_err());
        assert!(args(&["grab", "--source", "A", "--target", "A"]).is_err());
    }

    #[test]
    fn the_cap_cannot_exceed_60_seconds() {
        assert!(args(&["grab", "--source", "A", "--target", "B", "--secs", "61"]).is_err());
        assert!(args(&["dry-run", "--source", "A", "--secs", "0"]).is_err());
        let a = args(&["dry-run", "--source", "A"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.secs, 30);
        let a = args(&["clone-probe", "--source", "A"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.secs, 20);
    }

    #[test]
    fn modes_reject_arguments_they_do_not_use() {
        assert!(args(&["dry-run", "--source", "A", "--target", "B"]).is_err());
        assert!(args(&["inject-test", "--source", "A", "--target", "B"]).is_err());
        let a = args(&["inject-test", "--target", "B"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.secs, 5);
        assert!(args(&["bogus"]).is_err());
        assert!(args(&[]).is_err());
    }

    #[test]
    fn ranges_compare_without_the_current_value() {
        let a = sys::AbsInfo {
            value: 10,
            minimum: 0,
            maximum: 1920,
            ..Default::default()
        };
        let b = sys::AbsInfo { value: 99, ..a };
        let c = sys::AbsInfo { maximum: 3840, ..a };
        assert!(same_ranges(&[a], &[b]));
        assert!(!same_ranges(&[a], &[c]));
        assert!(!same_ranges(&[a], &[]));
    }
}
