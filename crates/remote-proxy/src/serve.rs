//! `remote-proxy serve`: the product helper of ADR 0016.
//!
//! It grabs the remote's input devices (every `--source-prefix` node except
//! the target, with capabilities and `ABS` ranges identical to the target's)
//! and re-sends every event except the caught keys, frame by frame, into the
//! target sibling. It talks to its parent (the satellite daemon) over stdout,
//! one line per message:
//!
//! - `ready <source events, comma separated> <target event>`
//! - `key <code> <value> <sec> <usec>`: a caught key's event with its kernel time
//! - `hb <frames> <median_us> <max_us>`: heartbeat every 500 ms, with the
//!   forwarding delay of the frames since the last heartbeat
//! - `end <reason>`: the last line
//!
//! Human-readable notes go to stderr. Its stdin is the parent link: end of
//! file (the parent died or closed it) ends the run. Every exit releases the
//! keys still down in the target; the kernel releases the grabs when the
//! process ends. `--check` does the discovery and the comparisons only (no
//! grab, no write) and prints `ok <sources> <target>`.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::caps::{self, Caps, Node};
use crate::event::{self, Event};
use crate::filter::{Action, Forwarder, KeyBits, MIC_KEY};
use crate::{
    DeadMan, EMERGENCY, EXIT_DEVICE_LOST, EXIT_REFUSED, Emergency, open_read, open_write,
    read_caps, read_events, same_ranges, sys,
};

/// Default device-name prefix of the remote's slots (`LGE M-RCU - Builtin [n]`).
const DEFAULT_PREFIX: &str = "LGE M-RCU - Builtin [";
/// Heartbeat period on stdout.
const HEARTBEAT: Duration = Duration::from_millis(500);
/// How long a source may keep a key down before the grab gives up.
const KEY_DOWN_WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ServeArgs {
    target: String,
    prefix: String,
    catch: Vec<u16>,
    check: bool,
    /// A device that must be among the sources (the daemon's key device).
    require: Option<String>,
}

pub(crate) fn parse(argv: &[String]) -> Result<ServeArgs, String> {
    let mut target = None;
    let mut prefix = DEFAULT_PREFIX.to_owned();
    let mut catch = Vec::new();
    let mut check = false;
    let mut require = None;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--target" => target = Some(val()?),
            "--source-prefix" => prefix = val()?,
            "--catch" => {
                let v = val()?;
                let c = v.parse::<u16>().map_err(|_| format!("bad --catch {v:?}"))?;
                if c == 0 || usize::from(c) >= crate::filter::KEY_CODES {
                    return Err(format!("--catch {c} is out of range"));
                }
                if !catch.contains(&c) {
                    catch.push(c);
                }
            }
            "--check" => check = true,
            "--require-source" => require = Some(val()?),
            other => return Err(format!("serve: unknown argument {other:?}")),
        }
    }
    let target = target.ok_or("serve: --target is required")?;
    if prefix.is_empty() {
        return Err("serve: --source-prefix must not be empty".into());
    }
    if catch.is_empty() {
        catch.push(MIC_KEY);
    }
    Ok(ServeArgs {
        target,
        prefix,
        catch,
        check,
        require,
    })
}

/// One grabbed source.
struct Source {
    node: Node,
    file: File,
    fwd: Forwarder,
}

/// Finds the target and the sources; checks capabilities and ranges.
fn discover(a: &ServeArgs) -> Result<(Node, File, Caps, Vec<Source>), String> {
    let root = PathBuf::from(caps::SYS_INPUT);
    let tgt = caps::find(&root, &a.target)?;
    let tf = open_write(&tgt.dev)?;
    let tc = read_caps(&root, &tgt, &tf)?;
    let axes = tc.abs.clone();
    let tr: Vec<sys::AbsInfo> = axes
        .iter()
        .map(|&x| sys::abs_info(&tf, x).map_err(|e| format!("EVIOCGABS({x}) on target: {e}")))
        .collect::<Result<_, _>>()?;
    let mut sources = Vec::new();
    for node in caps::list(&root) {
        if !node.name.starts_with(&a.prefix) || node.name == tgt.name {
            continue;
        }
        let file = open_read(&node.dev)?;
        let c = read_caps(&root, &node, &file)?;
        if let Some(d) = c.first_difference(&tc) {
            // A slot of the remote that cannot be re-sent through the target
            // would stay ungrabbed: its mic key would reach LG. Refuse.
            return Err(format!(
                "{} {:?} has capabilities that differ from the target's ({d}); not grabbing",
                node.event, node.name
            ));
        }
        let r: Vec<sys::AbsInfo> = axes
            .iter()
            .map(|&x| sys::abs_info(&file, x).map_err(|e| format!("EVIOCGABS({x}): {e}")))
            .collect::<Result<_, _>>()?;
        if !same_ranges(&r, &tr) {
            return Err(format!(
                "{} {:?} has ABS ranges that differ from the target's; not grabbing",
                node.event, node.name
            ));
        }
        sources.push(Source {
            node,
            file,
            fwd: Forwarder::new(a.catch.clone()),
        });
    }
    if sources.is_empty() {
        return Err(format!(
            "no source device named {:?}* with the target's capabilities",
            a.prefix
        ));
    }
    if let Some(req) = &a.require {
        if !sources.iter().any(|s| &s.node.name == req) {
            return Err(format!("{req:?} is not among the sources; not grabbing"));
        }
    }
    if !a.catch.iter().all(|k| tc.key.contains(k)) {
        return Err(format!(
            "the devices do not declare every caught key {:?}",
            a.catch
        ));
    }
    Ok((tgt, tf, tc, sources))
}

fn names(sources: &[Source]) -> String {
    sources
        .iter()
        .map(|s| s.node.event.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Writes one protocol line; a parent that is gone ends the run (`Err`).
fn say(line: &str) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")
        .and_then(|()| out.flush())
        .map_err(|e| format!("parent pipe: {e}"))
}

// One linear sequence (discover, grab, loop, release) reads best as one function.
#[allow(clippy::too_many_lines)]
pub(crate) fn serve(argv: &[String]) -> Result<i32, String> {
    let a = parse(argv)?;
    // Keys an earlier helper left down in the target (a SIGKILL gives it no
    // chance to release them) are released first, before any check that may
    // refuse the start; a missing target is skipped here (discovery says so).
    if !a.check {
        if let Some(n) = release_target_keys(&a.target)? {
            if n > 0 {
                eprintln!("serve: released {n} key(s) left down in the target");
            }
        }
    }
    let (tgt, tf, _tc, mut sources) = discover(&a)?;
    if a.check {
        println!("ok {} {}", names(&sources), tgt.event);
        return Ok(0);
    }
    let keys = Arc::new(KeyBits::default());
    let _ = EMERGENCY.set(Emergency {
        target: Some(tf.try_clone().map_err(|e| format!("dup target: {e}"))?),
        keys: Arc::clone(&keys),
    });
    let mut dm = DeadMan::start_with(None)?;

    // Grab each source only with no key down on it, so the compositor never
    // keeps a key it saw go down; a key that went down between the check and
    // the grab is caught by a second check after it (then ungrab and wait).
    let until = Instant::now() + KEY_DOWN_WAIT;
    for s in &sources {
        loop {
            let st = sys::key_state(&s.file).map_err(|e| format!("EVIOCGKEY: {e}"))?;
            if st.iter().all(|&b| b == 0) {
                sys::grab(&s.file, true).map_err(|e| format!("EVIOCGRAB {}: {e}", s.node.event))?;
                // Events queued for us since the device was opened (before
                // the grab) already reached the compositor: drop them, so no
                // key acts twice.
                discard_queued(&s.file)?;
                let after = sys::key_state(&s.file).map_err(|e| format!("EVIOCGKEY: {e}"))?;
                if after.iter().all(|&b| b == 0) {
                    break;
                }
                let _ = sys::grab(&s.file, false);
            }
            if Instant::now() >= until {
                for g in &sources {
                    let _ = sys::grab(&g.file, false);
                }
                say(&format!("end key held on {}", s.node.event))?;
                return Ok(EXIT_REFUSED);
            }
            dm.wait_fds(&[])?;
        }
    }
    say(&format!("ready {} {}", names(&sources), tgt.event))?;
    eprintln!(
        "serve: grabbed {} -> {} (all events except {:?})",
        names(&sources),
        tgt.event,
        a.catch
    );

    let mut delays: Vec<u32> = Vec::new();
    let mut frames_total: u64 = 0;
    let mut next_hb = Instant::now() + HEARTBEAT;
    let mut evs = Vec::new();
    let mut acts = Vec::new();
    let fds: Vec<RawFd> = sources.iter().map(|s| s.file.as_raw_fd()).collect();
    let files: Vec<File> = sources
        .iter()
        .map(|s| s.file.try_clone())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("dup source: {e}"))?;
    let mut code = 0;
    // Caught keys reported down to the parent and not yet up.
    let mut caught_down: Vec<u16> = Vec::new();
    let reason = 'run: loop {
        let ready = match dm.wait_fds(&fds) {
            Ok(r) => r,
            Err(r) => break r,
        };
        for (i, s) in sources.iter_mut().enumerate() {
            if !ready.get(i).copied().unwrap_or(false) {
                continue;
            }
            evs.clear();
            if let Err(e) = read_events(&s.file, &mut evs) {
                code = EXIT_DEVICE_LOST;
                break 'run format!("source {} lost: {e}", s.node.event);
            }
            acts.clear();
            for &ev in &evs {
                s.fwd.push(ev, &mut acts);
            }
            for act in acts.drain(..) {
                match act {
                    Action::Forward { frame, time } => {
                        if let Err(e) = write_frame(&tf, &frame, &keys) {
                            code = EXIT_DEVICE_LOST;
                            break 'run e;
                        }
                        frames_total += 1;
                        if delays.len() < 10_000 {
                            let d = crate::now_realtime().saturating_sub(time);
                            delays.push(u32::try_from(d.as_micros()).unwrap_or(u32::MAX));
                        }
                    }
                    Action::Caught {
                        code: c,
                        value,
                        time,
                    } => {
                        match value {
                            0 => caught_down.retain(|&k| k != c),
                            1 if !caught_down.contains(&c) => caught_down.push(c),
                            _ => {}
                        }
                        if let Err(e) = say(&format!(
                            "key {c} {value} {} {}",
                            time.as_secs(),
                            time.subsec_micros()
                        )) {
                            break 'run e;
                        }
                    }
                    Action::Resync => {
                        // The keys still down anywhere: one remote may use
                        // several slots, so the sources' states are OR-ed.
                        let st = combined_state(&files);
                        let rel = keys.resync_release_frame(&st);
                        if !rel.is_empty() {
                            let _ = write_frame(&tf, &rel, &keys);
                        }
                        let now = crate::now_realtime();
                        for c in stale_caught(&caught_down, &st) {
                            caught_down.retain(|&k| k != c);
                            if let Err(e) = say(&format!(
                                "key {c} 0 {} {}",
                                now.as_secs(),
                                now.subsec_micros()
                            )) {
                                break 'run e;
                            }
                        }
                    }
                }
            }
        }
        if Instant::now() >= next_hb {
            next_hb = Instant::now() + HEARTBEAT;
            let (med, max) = median_max(&mut delays);
            delays.clear();
            if let Err(e) = say(&format!("hb {frames_total} {med} {max}")) {
                break 'run e;
            }
        }
    };
    let rel = keys.take_release_frame();
    if !rel.is_empty() {
        let mut w: &File = &tf;
        let _ = w.write(&event::encode(&rel));
    }
    for s in &sources {
        let _ = sys::grab(&s.file, false);
    }
    let _ = say(&format!("end {reason}"));
    eprintln!("serve: end: {reason}");
    Ok(code)
}

fn write_frame(tf: &File, frame: &[Event], keys: &KeyBits) -> Result<(), String> {
    let bytes = event::encode(frame);
    let mut w: &File = tf;
    match w.write(&bytes) {
        Ok(n) if n == bytes.len() => {
            keys.apply(frame);
            Ok(())
        }
        Ok(n) => Err(format!("short write to target: {n} of {}", bytes.len())),
        Err(e) => Err(format!("target write: {e}")),
    }
}

/// Reads `f` (non-blocking) until nothing is queued; the events are dropped.
fn discard_queued(f: &File) -> Result<(), String> {
    let mut evs = Vec::new();
    for _ in 0..10_000 {
        evs.clear();
        read_events(f, &mut evs)?;
        if evs.is_empty() {
            return Ok(());
        }
    }
    Err("a source kept delivering events while its queue was emptied".to_owned())
}

/// Releases every key that is down on the device named `target` (found by
/// name; `Ok(None)` when there is no such device). Used at every serve start
/// and by `release-target` after a helper was killed.
pub(crate) fn release_target_keys(target: &str) -> Result<Option<usize>, String> {
    let root = PathBuf::from(caps::SYS_INPUT);
    let Ok(node) = caps::find(&root, target) else {
        return Ok(None);
    };
    let tf = open_write(&node.dev)?;
    let held = sys::key_state(&tf).map_err(|e| format!("EVIOCGKEY on {}: {e}", node.event))?;
    let rel = release_all(&held);
    if rel.is_empty() {
        return Ok(Some(0));
    }
    let mut w: &File = &tf;
    w.write(&event::encode(&rel))
        .map_err(|e| format!("write {}: {e}", node.event))?;
    Ok(Some(rel.len() - 1))
}

/// `remote-proxy release-target --target NAME`: one shot, after a helper was
/// killed with `SIGKILL` (ADR 0016 dead-man): releases the keys it left down.
pub(crate) fn release_target(argv: &[String]) -> Result<i32, String> {
    let target = match argv {
        [flag, name] if flag == "--target" && !name.is_empty() => name,
        _ => return Err("usage: remote-proxy release-target --target NAME".to_owned()),
    };
    match release_target_keys(target)? {
        Some(n) => println!("released {n} key(s) on {target:?}"),
        None => println!("no input device named {target:?}; nothing to release"),
    }
    Ok(0)
}

/// A frame releasing every key set in `state` (`EVIOCGKEY` bytes), with
/// `SYN_REPORT`; empty when nothing is down.
fn release_all(state: &[u8]) -> Vec<Event> {
    let mut f: Vec<Event> = crate::caps::codes(state)
        .into_iter()
        .map(|c| Event::new(crate::event::EV_KEY, c, 0))
        .collect();
    if !f.is_empty() {
        f.push(Event::syn());
    }
    f
}

/// The OR of the sources' key states (bit n in byte n/8).
fn combined_state(files: &[File]) -> [u8; 256] {
    let mut all = [0u8; 256];
    for f in files {
        if let Ok(st) = sys::key_state(f) {
            for (a, b) in all.iter_mut().zip(st) {
                *a |= b;
            }
        }
    }
    all
}

/// Caught keys reported down that are up in `state` (their release was lost
/// in a `SYN_DROPPED`).
fn stale_caught(down: &[u16], state: &[u8]) -> Vec<u16> {
    down.iter()
        .copied()
        .filter(|&c| {
            let c = usize::from(c);
            state.get(c / 8).is_none_or(|b| b & (1 << (c % 8)) == 0)
        })
        .collect()
}

/// (median, max) of `v` in microseconds; (0, 0) when empty.
fn median_max(v: &mut [u32]) -> (u32, u32) {
    if v.is_empty() {
        return (0, 0);
    }
    v.sort_unstable();
    (v[v.len() / 2], v[v.len() - 1])
}

/// True for an `EV_KEY` event of one of `catch` (used by tests).
#[cfg(test)]
fn is_caught(ev: &Event, catch: &[u16]) -> bool {
    ev.kind == crate::event::EV_KEY && catch.contains(&ev.code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &[&str]) -> Result<ServeArgs, String> {
        parse(&s.iter().map(|x| (*x).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn serve_args_default_to_the_mic_key_and_the_remote_prefix() {
        let a = p(&["--target", "LGE M-RCU - Builtin [2]"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.catch, vec![428]);
        assert_eq!(a.prefix, "LGE M-RCU - Builtin [");
        assert!(!a.check);
        let b = p(&[
            "--target", "T", "--catch", "428", "--catch", "398", "--catch", "428", "--check",
        ])
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.catch, vec![428, 398]);
        assert!(b.check);
        assert!(p(&[]).is_err());
        assert!(p(&["--target", "T", "--catch", "0"]).is_err());
        assert!(p(&["--target", "T", "--catch", "4000"]).is_err());
        assert!(p(&["--target", "T", "--source-prefix", ""]).is_err());
        assert!(p(&["--target", "T", "--secs", "5"]).is_err());
    }

    #[test]
    fn releases_and_stale_caught_keys_from_a_key_state() {
        let mut st = [0u8; 256];
        assert_eq!(release_all(&st), Vec::new());
        st[103 / 8] |= 1 << (103 % 8);
        st[428 / 8] |= 1 << (428 % 8);
        let rel = release_all(&st);
        assert_eq!(
            rel,
            vec![
                Event::new(crate::event::EV_KEY, 103, 0),
                Event::new(crate::event::EV_KEY, 428, 0),
                Event::syn()
            ]
        );
        assert_eq!(stale_caught(&[428], &st), Vec::<u16>::new());
        st[428 / 8] = 0;
        assert_eq!(stale_caught(&[428], &st), vec![428]);
        let b = p(&[
            "--target",
            "T",
            "--require-source",
            "LGE M-RCU - Builtin [0]",
        ])
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.require.as_deref(), Some("LGE M-RCU - Builtin [0]"));
    }

    #[test]
    fn median_and_max_of_the_window() {
        assert_eq!(median_max(&mut []), (0, 0));
        assert_eq!(median_max(&mut [5, 1, 9]), (5, 9));
        assert!(is_caught(&Event::new(crate::event::EV_KEY, 428, 1), &[428]));
        assert!(!is_caught(
            &Event::new(crate::event::EV_KEY, 427, 1),
            &[428]
        ));
    }
}
