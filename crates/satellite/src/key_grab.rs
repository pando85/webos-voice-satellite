//! The mic key taken at the input layer (ADR 0016): the daemon's side of the
//! `remote-proxy serve` helper.
//!
//! While the mic button feature runs on a firmware where EXP-026 passed, a
//! helper process grabs the Magic Remote's input devices and re-sends every
//! event except the mic key into an idle sibling, so LG's compositor never
//! sees the key (no toast, no LG assistant). It reports the key's presses
//! and releases, and a heartbeat, on its stdout; this module turns them into
//! the same [`Key`] events the passive reader ([`crate::evdev`]) produces, so
//! the push-to-talk machine does not change.
//!
//! Dead-man (ADR 0016 decision 4), the kernel releasing the grab whenever the
//! helper ends:
//! 1. the helper's own loop watchdog (1 s);
//! 2. the parent link: the helper's stdin is a pipe from here (end of file
//!    ends it) and it is started with `PR_SET_PDEATHSIG = SIGKILL`;
//! 3. here: no line for [`STALL`] after `ready` (heartbeats come every
//!    500 ms) and the helper is killed, then restarted with backoff;
//! 4. the supervisor and `ctl.sh` kill a helper left by a dead daemon (pid
//!    file `run/remote-proxy.pid`).
//!
//! ADR 0013's configd override stays in place as the safety net: when the
//! helper is down, a press shows LG's toast instead of LG's assistant.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

use crate::config::{KeyGrab, RemoteSection};
use crate::evdev::Key;

/// Firmware versions where EXP-026 Part B (B2 grab, B3 kill test) passed,
/// with the sibling that receives the re-sent events there.
pub(crate) const VERIFIED_FIRMWARE: &[(&str, &str)] = &[("03.30.74", "LGE M-RCU - Builtin [2]")];

/// The remote's slots: every device whose name starts with this.
pub(crate) const SOURCE_PREFIX: &str = "LGE M-RCU - Builtin [";

/// The helper's file name in `<install>/bin`.
pub(crate) const HELPER: &str = "remote-proxy";

/// Its pid file in `<install>/run` (read by `ctl.sh` and the supervisor).
pub(crate) const PID_FILE: &str = "remote-proxy.pid";

/// After `ready`: longest silence before the helper counts as stalled.
pub(crate) const STALL: Duration = Duration::from_millis(1500);
/// Before `ready`: the helper may wait up to 2 s for keys to be released.
const START_WAIT: Duration = Duration::from_secs(5);
/// How long a stop waits for the helper's own clean exit before `SIGKILL`.
const STOP_WAIT: Duration = Duration::from_secs(1);
/// Restart backoff.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A run that lasted this long resets the backoff.
const STABLE_RUN: Duration = Duration::from_secs(60);

/// Everything the supervisor needs to run the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) helper: PathBuf,
    pub(crate) target: String,
    pub(crate) catch: u16,
    /// Where the pid file goes.
    pub(crate) run_dir: PathBuf,
}

impl Plan {
    fn args(&self, check: bool) -> Vec<String> {
        let mut a = vec![
            "serve".to_owned(),
            "--target".to_owned(),
            self.target.clone(),
            "--source-prefix".to_owned(),
            SOURCE_PREFIX.to_owned(),
            "--catch".to_owned(),
            self.catch.to_string(),
        ];
        if check {
            a.push("--check".to_owned());
        }
        a
    }
}

/// Decides whether the grab may be used: the config allows it, the firmware
/// is on the verified list (or a target is configured by hand), and the
/// helper exists. `Err` says why not (the configd override alone is then
/// used, as before ADR 0016).
pub(crate) fn plan(
    r: &RemoteSection,
    firmware: Option<&str>,
    install_prefix: Option<&Path>,
    run_dir: &Path,
) -> Result<Plan, String> {
    if r.key_grab == KeyGrab::Off {
        return Err("key_grab = \"off\"".to_owned());
    }
    let from_table = firmware.and_then(|f| {
        VERIFIED_FIRMWARE
            .iter()
            .find(|(v, _)| *v == f)
            .map(|(_, t)| (*t).to_owned())
    });
    let target = match (&r.grab_target, from_table) {
        (Some(t), _) => t.clone(),
        (None, Some(t)) => t,
        (None, None) => {
            return Err(format!(
                "firmware {} has not passed EXP-026 (the grab is verified on {})",
                firmware.unwrap_or("unknown"),
                VERIFIED_FIRMWARE
                    .iter()
                    .map(|(v, _)| *v)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    };
    let helper = match (&r.grab_helper, install_prefix) {
        (Some(h), _) => h.clone(),
        (None, Some(p)) => p.join("bin").join(HELPER),
        (None, None) => return Err("no install directory, so no helper".to_owned()),
    };
    if !helper.is_file() {
        return Err(format!("{} not found", helper.display()));
    }
    Ok(Plan {
        helper,
        target,
        catch: r.key_code,
        run_dir: run_dir.to_path_buf(),
    })
}

/// Runs `helper serve ... --check` (discovery and comparisons only: no grab,
/// no write). `Ok`: the helper's `ok <sources> <target>` line.
pub(crate) async fn preflight(p: &Plan) -> Result<String, String> {
    let mut cmd = Command::new(&p.helper);
    cmd.args(p.args(true))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::power::die_with_parent(&mut cmd);
    let out = match timeout(Duration::from_secs(5), cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("{}: {e}", p.helper.display())),
        Err(_) => return Err("the helper's check took longer than 5 s".to_owned()),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().last().unwrap_or_default().trim().to_owned();
    if out.status.success() && line.starts_with("ok ") {
        Ok(line)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "the helper's check failed: {}",
            err.lines().last().unwrap_or(&line).trim()
        ))
    }
}

/// One protocol line of the helper.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Line {
    Ready {
        sources: String,
        target: String,
    },
    Key {
        code: u16,
        value: i32,
        time: Duration,
    },
    Heartbeat {
        frames: u64,
        median_us: u32,
        max_us: u32,
    },
    End(String),
    Other(String),
}

pub(crate) fn parse_line(l: &str) -> Line {
    let mut w = l.split_whitespace();
    match w.next() {
        Some("ready") => match (w.next(), w.next()) {
            (Some(s), Some(t)) => Line::Ready {
                sources: s.to_owned(),
                target: t.to_owned(),
            },
            _ => Line::Other(l.to_owned()),
        },
        Some("key") => {
            let v: Vec<&str> = w.collect();
            match (
                v.first().and_then(|x| x.parse::<u16>().ok()),
                v.get(1).and_then(|x| x.parse::<i32>().ok()),
                v.get(2).and_then(|x| x.parse::<u64>().ok()),
                v.get(3).and_then(|x| x.parse::<u32>().ok()),
            ) {
                (Some(code), Some(value), Some(sec), Some(usec)) if usec < 1_000_000 => Line::Key {
                    code,
                    value,
                    time: Duration::new(sec, usec * 1000),
                },
                _ => Line::Other(l.to_owned()),
            }
        }
        Some("hb") => {
            let v: Vec<&str> = w.collect();
            match (
                v.first().and_then(|x| x.parse::<u64>().ok()),
                v.get(1).and_then(|x| x.parse::<u32>().ok()),
                v.get(2).and_then(|x| x.parse::<u32>().ok()),
            ) {
                (Some(frames), Some(median_us), Some(max_us)) => Line::Heartbeat {
                    frames,
                    median_us,
                    max_us,
                },
                _ => Line::Other(l.to_owned()),
            }
        }
        Some("end") => Line::End(l.get(4..).unwrap_or_default().trim().to_owned()),
        _ => Line::Other(l.to_owned()),
    }
}

/// What the status reports show about the grab.
#[derive(Debug, Clone, Default)]
pub(crate) struct GrabStatus {
    /// `starting`, `grabbing`, `restarting` or `stopped`.
    pub(crate) state: &'static str,
    pub(crate) sources: String,
    pub(crate) target: String,
    pub(crate) restarts: u32,
    pub(crate) last_end: String,
    pub(crate) frames: u64,
    /// The worst heartbeat window's maximum and the last window's median.
    pub(crate) median_us: u32,
    pub(crate) max_us: u32,
}

/// One line for `voice.json`, `ctl.sh status` and the TV app.
pub(crate) fn describe(s: &GrabStatus) -> String {
    match s.state {
        "grabbing" => format!(
            "on: {} -> {} (restarts {}, delay median {} us, max {} us)",
            s.sources, s.target, s.restarts, s.median_us, s.max_us
        ),
        "restarting" => format!(
            "restarting (restarts {}; last end: {})",
            s.restarts,
            s.last_end.chars().take(120).collect::<String>()
        ),
        other => other.to_owned(),
    }
}

/// A running supervisor.
pub(crate) struct Handle {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    pub(crate) status: Arc<Mutex<GrabStatus>>,
}

impl Handle {
    /// Stops the helper (its clean exit releases held keys and the grab;
    /// `SIGKILL` after [`STOP_WAIT`]) and waits for the supervisor.
    pub(crate) async fn stop(self) {
        let _ = self.stop.send(true);
        if timeout(STOP_WAIT * 3, self.task).await.is_err() {
            log::warn!("remote grab: the supervisor did not stop in time");
        }
    }
}

/// Starts the supervisor: runs the helper, restarts it with backoff, and
/// sends the mic key's presses and releases to `tx`.
pub(crate) fn spawn(plan: Plan, tx: mpsc::UnboundedSender<Key>) -> Handle {
    spawn_with(plan, tx, STALL)
}

pub(crate) fn spawn_with(plan: Plan, tx: mpsc::UnboundedSender<Key>, stall: Duration) -> Handle {
    let (stop_tx, stop_rx) = watch::channel(false);
    let status = Arc::new(Mutex::new(GrabStatus {
        state: "starting",
        ..GrabStatus::default()
    }));
    let st = Arc::clone(&status);
    let task = tokio::spawn(async move { supervise(plan, tx, stop_rx, st, stall).await });
    Handle {
        stop: stop_tx,
        task,
        status,
    }
}

fn set(status: &Mutex<GrabStatus>, f: impl FnOnce(&mut GrabStatus)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

async fn supervise(
    plan: Plan,
    tx: mpsc::UnboundedSender<Key>,
    mut stop: watch::Receiver<bool>,
    status: Arc<Mutex<GrabStatus>>,
    stall: Duration,
) {
    let pid_file = plan.run_dir.join(PID_FILE);
    let mut backoff = BACKOFF_FIRST;
    loop {
        if *stop.borrow() {
            break;
        }
        let started = tokio::time::Instant::now();
        let end = run_once(&plan, &tx, &mut stop, &status, &pid_file, stall).await;
        let _ = std::fs::remove_file(&pid_file);
        if *stop.borrow() {
            break;
        }
        if started.elapsed() >= STABLE_RUN {
            backoff = BACKOFF_FIRST;
        }
        log::warn!(
            "remote grab: the helper ended ({end}); restarting in {} s (meanwhile LG's toast shows on a press)",
            backoff.as_secs()
        );
        set(&status, |s| {
            s.state = "restarting";
            s.restarts = s.restarts.saturating_add(1);
            s.last_end.clone_from(&end);
        });
        tokio::select! {
            () = tokio::time::sleep(backoff) => {}
            _ = stop.changed() => break,
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    set(&status, |s| s.state = "stopped");
}

/// One run of the helper; returns why it ended.
#[allow(clippy::too_many_lines)] // one select loop over the helper's lines
async fn run_once(
    plan: &Plan,
    tx: &mpsc::UnboundedSender<Key>,
    stop: &mut watch::Receiver<bool>,
    status: &Mutex<GrabStatus>,
    pid_file: &Path,
    stall: Duration,
) -> String {
    let mut cmd = Command::new(&plan.helper);
    cmd.args(plan.args(false))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::power::die_with_parent(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return format!("cannot start {}: {e}", plan.helper.display()),
    };
    if let Some(pid) = child.id() {
        let _ = std::fs::write(pid_file, format!("{pid}\n"));
    }
    // The parent link: kept open while the helper should run.
    let stdin = child.stdin.take();
    if let Some(err) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                log::info!(
                    "remote grab: helper: {}",
                    l.chars().take(200).collect::<String>()
                );
            }
        });
    }
    let Some(out) = child.stdout.take() else {
        let _ = child.start_kill();
        return "no stdout pipe".to_owned();
    };
    let mut lines = BufReader::new(out).lines();
    let mut ready = false;
    let mut held = false;
    let mut end: Option<String> = None;
    let why = loop {
        let wait = if ready { stall } else { START_WAIT };
        tokio::select! {
            r = timeout(wait, lines.next_line()) => match r {
                Err(_) => {
                    let _ = child.start_kill();
                    break format!(
                        "no heartbeat for {} ms: killed",
                        wait.as_millis()
                    );
                }
                Ok(Ok(Some(l))) => match parse_line(&l) {
                    Line::Ready { sources, target } => {
                        ready = true;
                        log::info!("remote grab: {sources} -> {target} (the mic key never reaches LG)");
                        set(status, |s| {
                            s.state = "grabbing";
                            s.sources = sources;
                            s.target = target;
                        });
                        let _ = tx.send(Key::Found);
                    }
                    Line::Key { code, value, time } if code == plan.catch => match value {
                        1 => {
                            held = true;
                            let _ = tx.send(Key::Pressed(time));
                        }
                        0 => {
                            held = false;
                            let _ = tx.send(Key::Released(Some(time)));
                        }
                        _ => {}
                    },
                    Line::Key { .. } | Line::Other(_) => {}
                    Line::Heartbeat { frames, median_us, max_us } => set(status, |s| {
                        s.frames = frames;
                        s.median_us = median_us;
                        s.max_us = s.max_us.max(max_us);
                    }),
                    Line::End(r) => end = Some(r),
                },
                Ok(Ok(None)) => break end.take().unwrap_or_else(|| "its output ended".to_owned()),
                Ok(Err(e)) => {
                    let _ = child.start_kill();
                    break format!("reading its output: {e}");
                }
            },
            _ = stop.changed() => {
                // A clean stop: end of file on its stdin; it releases held
                // keys and the grab, then exits.
                drop(stdin);
                if timeout(STOP_WAIT, child.wait()).await.is_err() {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    log::warn!("remote grab: the helper did not stop in {} ms: killed", STOP_WAIT.as_millis());
                }
                if held {
                    let _ = tx.send(Key::Released(None));
                }
                return "stopped".to_owned();
            }
        }
    };
    let code = timeout(STOP_WAIT, child.wait()).await;
    if held {
        let _ = tx.send(Key::Released(None));
    }
    if let Ok(Ok(st)) = code {
        format!("{why}; {st}")
    } else {
        let _ = child.start_kill();
        why
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn section() -> RemoteSection {
        RemoteSection::default()
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("keygrab-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bin")).unwrap();
        std::fs::create_dir_all(d.join("run")).unwrap();
        d
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("bin").join(HELPER);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn the_plan_follows_the_firmware_table_the_config_and_the_helper() {
        let d = tmp("plan");
        let run = d.join("run");
        let r = section();
        // no helper yet
        assert!(
            plan(&r, Some("03.30.74"), Some(&d), &run)
                .unwrap_err()
                .contains("not found")
        );
        script(&d, "exit 0");
        let p = plan(&r, Some("03.30.74"), Some(&d), &run).unwrap();
        assert_eq!(p.target, "LGE M-RCU - Builtin [2]");
        assert_eq!(p.catch, 428);
        assert_eq!(p.helper, d.join("bin").join(HELPER));
        // other firmware: not verified
        let e = plan(&r, Some("03.30.73"), Some(&d), &run).unwrap_err();
        assert!(e.contains("03.30.73") && e.contains("03.30.74"), "{e}");
        assert!(plan(&r, None, Some(&d), &run).is_err());
        // a configured target overrides the table (the owner's experiments)
        let r2 = RemoteSection {
            grab_target: Some("LGE M-RCU - Builtin [1]".into()),
            ..section()
        };
        assert_eq!(
            plan(&r2, Some("03.30.73"), Some(&d), &run).unwrap().target,
            "LGE M-RCU - Builtin [1]"
        );
        // off
        let r3 = RemoteSection {
            key_grab: KeyGrab::Off,
            ..section()
        };
        assert!(
            plan(&r3, Some("03.30.74"), Some(&d), &run)
                .unwrap_err()
                .contains("off")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn protocol_lines_parse() {
        assert_eq!(
            parse_line("ready event2,event3 event4"),
            Line::Ready {
                sources: "event2,event3".into(),
                target: "event4".into()
            }
        );
        assert_eq!(
            parse_line("key 428 1 1700000000 250000"),
            Line::Key {
                code: 428,
                value: 1,
                time: Duration::new(1_700_000_000, 250_000_000)
            }
        );
        assert_eq!(
            parse_line("hb 886 104 3166"),
            Line::Heartbeat {
                frames: 886,
                median_us: 104,
                max_us: 3166
            }
        );
        assert_eq!(
            parse_line("end stdin end of file"),
            Line::End("stdin end of file".into())
        );
        assert!(matches!(parse_line("key 428 1 x 0"), Line::Other(_)));
        assert!(matches!(parse_line("key 428 1 5 1000000"), Line::Other(_)));
        assert!(matches!(parse_line("garbage"), Line::Other(_)));
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<Key>, ms: u64) -> Option<Key> {
        timeout(Duration::from_millis(ms), rx.recv())
            .await
            .ok()
            .flatten()
    }

    fn plan_for(d: &Path, helper: PathBuf) -> Plan {
        Plan {
            helper,
            target: "T".into(),
            catch: 428,
            run_dir: d.join("run"),
        }
    }

    #[tokio::test]
    async fn presses_and_releases_reach_the_machine_and_a_stop_ends_the_helper_cleanly() {
        let d = tmp("keys");
        // A helper that reports ready, one press and release, then beats
        // until its stdin ends, and says why.
        let h = script(
            &d,
            "echo 'ready event2 event4'; echo 'key 428 1 10 0'; echo 'key 428 0 10 500000'; \
             echo 'key 115 1 10 0'; \
             ( while :; do echo 'hb 1 100 200'; sleep 0.2; done ) & b=$!; cat >/dev/null; kill $b; echo 'end stdin end of file'",
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = spawn(plan_for(&d, h), tx);
        assert_eq!(next(&mut rx, 3000).await, Some(Key::Found));
        assert_eq!(
            next(&mut rx, 1000).await,
            Some(Key::Pressed(Duration::from_secs(10)))
        );
        assert_eq!(
            next(&mut rx, 1000).await,
            Some(Key::Released(Some(Duration::from_millis(10_500))))
        );
        // other codes are ignored; the pid file is written
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(d.join("run").join(PID_FILE).exists());
        let st = handle.status.lock().unwrap().clone();
        assert_eq!(st.state, "grabbing");
        assert_eq!(
            (st.sources.as_str(), st.target.as_str()),
            ("event2", "event4")
        );
        assert_eq!(st.frames, 1);
        let t0 = std::time::Instant::now();
        handle.stop().await;
        assert!(
            t0.elapsed() < Duration::from_millis(1500),
            "{:?}",
            t0.elapsed()
        );
        assert!(!d.join("run").join(PID_FILE).exists());
        assert_eq!(next(&mut rx, 200).await, None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_stalled_helper_is_killed_within_the_limit_and_a_held_key_is_released() {
        let d = tmp("stall");
        // Ready, a press, then silence (a hang): no heartbeat at all.
        let h = script(
            &d,
            "echo 'ready event2 event4'; echo 'key 428 1 10 0'; exec sleep 30",
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = spawn_with(plan_for(&d, h), tx, Duration::from_millis(300));
        assert_eq!(next(&mut rx, 3000).await, Some(Key::Found));
        assert_eq!(
            next(&mut rx, 1000).await,
            Some(Key::Pressed(Duration::from_secs(10)))
        );
        let t0 = std::time::Instant::now();
        // The stall limit (300 ms here, 1.5 s in the product) kills it, and
        // the held key is reported released (lost).
        assert_eq!(next(&mut rx, 2000).await, Some(Key::Released(None)));
        assert!(
            t0.elapsed() < Duration::from_millis(1000),
            "{:?}",
            t0.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let st = handle.status.lock().unwrap().clone();
        assert_eq!(st.state, "restarting");
        assert_eq!(st.restarts, 1);
        assert!(st.last_end.contains("no heartbeat"), "{}", st.last_end);
        // It comes back (backoff 1 s) and reports ready again.
        assert_eq!(next(&mut rx, 3000).await, Some(Key::Found));
        handle.stop().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn the_preflight_reports_the_helpers_verdict() {
        let d = tmp("pre");
        let ok = script(&d, "echo 'ok event2,event3 event4'");
        assert_eq!(
            preflight(&plan_for(&d, ok)).await.unwrap(),
            "ok event2,event3 event4"
        );
        let bad = script(&d, "echo 'remote-proxy: capabilities differ' >&2; exit 2");
        let e = preflight(&plan_for(&d, bad)).await.unwrap_err();
        assert!(e.contains("capabilities differ"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
