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
}

pub(crate) fn parse(argv: &[String]) -> Result<ServeArgs, String> {
    let mut target = None;
    let mut prefix = DEFAULT_PREFIX.to_owned();
    let mut catch = Vec::new();
    let mut check = false;
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
            eprintln!(
                "serve: {} {:?} skipped: capabilities differ from the target ({d})",
                node.event, node.name
            );
            continue;
        }
        let r: Vec<sys::AbsInfo> = axes
            .iter()
            .map(|&x| sys::abs_info(&file, x).map_err(|e| format!("EVIOCGABS({x}): {e}")))
            .collect::<Result<_, _>>()?;
        if !same_ranges(&r, &tr) {
            eprintln!(
                "serve: {} {:?} skipped: ABS ranges differ from the target",
                node.event, node.name
            );
            continue;
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
    // keeps a key it saw go down.
    let until = Instant::now() + KEY_DOWN_WAIT;
    for s in &sources {
        loop {
            let st = sys::key_state(&s.file).map_err(|e| format!("EVIOCGKEY: {e}"))?;
            if st.iter().all(|&b| b == 0) {
                break;
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
        sys::grab(&s.file, true).map_err(|e| format!("EVIOCGRAB {}: {e}", s.node.event))?;
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
    let mut code = 0;
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
                        if let Err(e) = say(&format!(
                            "key {c} {value} {} {}",
                            time.as_secs(),
                            time.subsec_micros()
                        )) {
                            break 'run e;
                        }
                    }
                    Action::Resync => {
                        let st = sys::key_state(&s.file).unwrap_or([0; 256]);
                        let rel = keys.resync_release_frame(&st);
                        if !rel.is_empty() {
                            let _ = write_frame(&tf, &rel, &keys);
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
