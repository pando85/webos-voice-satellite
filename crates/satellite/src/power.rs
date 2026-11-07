//! TV power state (always-on gap G1): a supervised `luna-send -i` child
//! subscribed to `com.webos.service.tvpower/power/getPowerState`, and one-shot
//! `luna-send -n 1` calls for the wake-and-darken behaviour
//! ([`crate::darken`]).
//!
//! The subscription runs on a pseudo-terminal: over a pipe `luna-send` printed
//! nothing for some services on the TV (D0-D1, D5), and a subscription never
//! exits to flush its output, so the child gets a terminal as it does under
//! `ssh -tt` (D5, D6 on c3-b). The child is restarted with a backoff when it
//! exits and killed on shutdown. Each reply line is parsed into a
//! [`PowerUpdate`]; lines without `state` or `processing` are ignored.
//!
//! Reply shape (seen on c3-b, firmware 03.30.73): one JSON object per line,
//! `{"returnValue":true,"state":"Active Standby","processing":"Request Power
//! Off","onOff":"off","reason":"remoteKey"}`; settled states carry no
//! `processing`.

use std::fmt::Write as _;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

/// The subscription.
pub(crate) const POWER_STATE_URI: &str = "luna://com.webos.service.tvpower/power/getPowerState";
/// First restart delay of the subscription child.
const BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Longest restart delay.
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A child that ran this long counts as healthy: the next delay starts over.
const BACKOFF_STABLE: Duration = Duration::from_secs(60);
/// A subscription answers at once with the current state; silence for this
/// long is reported once.
const FIRST_LINE_WAIT: Duration = Duration::from_secs(10);
/// Timeout of a one-shot call (`-w` is 3000 ms).
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// One reply of the subscription.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PowerUpdate {
    /// `state`, e.g. `Active`, `Screen Off`, `Active Standby`.
    pub(crate) state: Option<String>,
    /// `processing`: a transition in progress (`Request Power Off`, ...).
    pub(crate) processing: Option<String>,
    /// `reason`, e.g. `remoteKey`, `offTimer`.
    pub(crate) reason: Option<String>,
    /// `onOff`: `on` or `off`.
    pub(crate) on_off: Option<String>,
}

impl PowerUpdate {
    /// No transition in progress.
    #[cfg(test)]
    pub(crate) fn settled(&self) -> bool {
        self.processing.is_none()
    }

    /// `state` without spaces (`ActiveStandby`), for comparisons; LG writes
    /// both `ScreenSaver` and `Screen Off`.
    pub(crate) fn state_key(&self) -> String {
        compact(self.state.as_deref().unwrap_or(""))
    }

    /// `processing` without spaces, or empty.
    pub(crate) fn processing_key(&self) -> String {
        compact(self.processing.as_deref().unwrap_or(""))
    }

    /// Text for the HA sensor: the state, plus the transition in brackets.
    pub(crate) fn text(&self) -> String {
        let state = self.state.as_deref().unwrap_or("unknown");
        match &self.processing {
            Some(p) => format!("{state} ({p})"),
            None => state.to_owned(),
        }
    }

    /// One log line with every field present.
    pub(crate) fn describe(&self) -> String {
        let mut s = format!("state {}", self.state.as_deref().unwrap_or("-"));
        for (k, v) in [
            ("processing", &self.processing),
            ("reason", &self.reason),
            ("onOff", &self.on_off),
        ] {
            if let Some(v) = v {
                let _ = write!(s, ", {k} {v}");
            }
        }
        s
    }
}

/// `s` without spaces.
pub(crate) fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// A parsed output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Line {
    Update(PowerUpdate),
    /// `returnValue: false`, with `errorText` if any.
    Error(String),
    /// Anything else (blank lines, other fields only).
    Other,
}

/// The string value of `"key"` in a flat JSON object line, unescaping `\"`
/// and `\\`. Not a JSON parser: enough for `luna-send` reply lines.
pub(crate) fn json_str(line: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let mut from = 0;
    while let Some(i) = line[from..].find(&pat) {
        let after = from + i + pat.len();
        let rest = line[after..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            let body = rest.strip_prefix('"')?;
            let mut out = String::new();
            let mut esc = false;
            for c in body.chars() {
                if esc {
                    out.push(c);
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    return Some(out);
                } else {
                    out.push(c);
                }
            }
            return None;
        }
        from = after;
    }
    None
}

/// The boolean value of `"key"`.
pub(crate) fn json_bool(line: &str, key: &str) -> Option<bool> {
    let pat = format!("\"{key}\"");
    let i = line.find(&pat)?;
    let rest = line[i + pat.len()..].trim_start().strip_prefix(':')?;
    let rest = rest.trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

/// Parses one line of `luna-send` output.
pub(crate) fn parse_line(line: &str) -> Line {
    let line = line.trim();
    if !line.starts_with('{') {
        return Line::Other;
    }
    if json_bool(line, "returnValue") == Some(false) {
        return Line::Error(
            json_str(line, "errorText").unwrap_or_else(|| "returnValue false".to_owned()),
        );
    }
    let u = PowerUpdate {
        state: json_str(line, "state"),
        processing: json_str(line, "processing"),
        reason: json_str(line, "reason"),
        on_off: json_str(line, "onOff"),
    };
    if u.state.is_none() && u.processing.is_none() {
        Line::Other
    } else {
        Line::Update(u)
    }
}

/// Restart delays of the subscription child: 1, 2, 4 ... 60 s, starting
/// over after a child that ran at least a minute.
#[derive(Debug, Default)]
pub(crate) struct Backoff {
    failures: u32,
}

impl Backoff {
    /// The delay before the next start, after a child that ran for `ran`.
    pub(crate) fn after_exit(&mut self, ran: Duration) -> Duration {
        if ran >= BACKOFF_STABLE {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        let shift = (self.failures - 1).min(16);
        BACKOFF_BASE.saturating_mul(1 << shift).min(BACKOFF_MAX)
    }
}

/// Finds `luna-send`: a path with a `/` must exist; a name is looked up in
/// `PATH` and then in the usual system directories (the boot hook's `PATH`
/// may be short).
pub(crate) fn find_binary(p: &Path) -> Option<PathBuf> {
    if p.as_os_str().is_empty() {
        return None;
    }
    if p.components().count() > 1 || p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/bin", "/usr/sbin", "/bin", "/sbin"].map(PathBuf::from))
        .map(|d| d.join(p))
        .find(|c| c.is_file())
}

/// A pseudo-terminal: the master (read by a thread) and the slave (the
/// child's stdin, stdout and stderr).
fn open_pty() -> std::io::Result<(std::fs::File, std::os::fd::OwnedFd)> {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
    grantpt(&master)?;
    unlockpt(&master)?;
    let name = ptsname(&master, Vec::new())?;
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok((std::fs::File::from(master), slave))
}

/// Makes the child get SIGKILL when the thread that spawned it ends
/// (`PR_SET_PDEATHSIG`): a subscription must not outlive the daemon, also
/// when it does not get to kill its children (a panic abort, the watchdog's
/// exit, SIGKILL). A leftover `voiceinput` subscription would hold LG's
/// remote stream until a reboot. The daemon spawns from its single runtime
/// thread, which lives as long as the process. If the parent is already
/// gone when the child starts, the child does not run.
#[allow(unsafe_code)]
fn die_with_parent(cmd: &mut Command) {
    let parent = rustix::process::getpid();
    // SAFETY: the closure runs in the forked child before exec and only
    // makes two system calls (prctl and getppid), which are
    // async-signal-safe; it allocates nothing (both error paths convert an
    // `Errno`, a plain code, into an `io::Error` without a heap allocation)
    // and takes no lock.
    unsafe {
        cmd.pre_exec(move || {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))?;
            if rustix::process::getppid() != Some(parent) {
                // The parent is gone already: do not run.
                return Err(rustix::io::Errno::SRCH.into());
            }
            Ok(())
        });
    }
}

/// Starts `luna-send -i <uri> <payload>` on a pty; lines arrive on the
/// receiver, which closes when the child's terminal is gone. The child dies
/// with this process ([`die_with_parent`]).
pub(crate) fn start_child(
    luna: &Path,
    uri: &str,
    payload: &str,
) -> std::io::Result<(tokio::process::Child, mpsc::UnboundedReceiver<String>)> {
    let (master, slave) = open_pty()?;
    let mut cmd = Command::new(luna);
    cmd.arg("-i")
        .arg(uri)
        .arg(payload)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave))
        .kill_on_drop(true);
    die_with_parent(&mut cmd);
    let child = cmd.spawn()?;
    // The Command holds the slave descriptors: drop them, so the master
    // read ends (EIO) when the child is gone.
    drop(cmd);
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("luna-power".into())
        .spawn(move || {
            let mut r = std::io::BufReader::new(master);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf)
                            .trim_end_matches(['\r', '\n'])
                            .to_owned();
                        if tx.send(line).is_err() {
                            return;
                        }
                    }
                }
            }
        })?;
    Ok((child, rx))
}

/// The running subscription.
pub(crate) struct Watch {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Watch {
    /// Kills the child and waits (bounded) for the task.
    pub(crate) async fn shutdown(self) {
        let _ = self.stop.send(true);
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(3), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }
}

/// What a line of a subscription child was.
enum LineOutcome {
    /// Handled; counts as the first answer.
    Answer,
    /// Handled; not an answer (noise).
    Noise,
    /// The receiver is gone: stop the child and the task.
    Closed,
}

/// Starts the supervised power state subscription; updates go to `tx`.
pub(crate) fn spawn(luna: PathBuf, tx: mpsc::UnboundedSender<PowerUpdate>) -> Watch {
    spawn_with(
        luna,
        POWER_STATE_URI,
        "power",
        "TV power state",
        move |line| match parse_line(line) {
            Line::Update(u) => {
                if tx.send(u).is_err() {
                    return LineOutcome::Closed;
                }
                LineOutcome::Answer
            }
            Line::Error(e) => {
                log::warn!("power: getPowerState answered an error: {e}");
                LineOutcome::Answer
            }
            Line::Other => {
                log::debug!("power: luna-send line ignored ({} bytes)", line.len());
                LineOutcome::Noise
            }
        },
    )
}

/// Starts a supervised `luna-send -i <uri> {"subscribe":true}` child (same
/// terminal, backoff and shutdown as the power watch); every output line that
/// starts with `{` is passed to `on_line`, which returns false to stop. TV
/// control uses it for the volume and the foreground app.
pub(crate) fn spawn_lines(
    luna: PathBuf,
    uri: &'static str,
    label: &'static str,
    what: &'static str,
    mut on_line: impl FnMut(String) -> bool + Send + 'static,
) -> Watch {
    spawn_with(luna, uri, label, what, move |line| {
        if !line.trim_start().starts_with('{') {
            return LineOutcome::Noise;
        }
        if on_line(line.to_owned()) {
            LineOutcome::Answer
        } else {
            LineOutcome::Closed
        }
    })
}

fn spawn_with(
    luna: PathBuf,
    uri: &'static str,
    label: &'static str,
    what: &'static str,
    on_line: impl FnMut(&str) -> LineOutcome + Send + 'static,
) -> Watch {
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(run(luna, uri, label, what, on_line, stop_rx));
    Watch { stop, task }
}

async fn run(
    luna: PathBuf,
    uri: &'static str,
    label: &'static str,
    what: &'static str,
    mut on_line: impl FnMut(&str) -> LineOutcome,
    mut stop: watch::Receiver<bool>,
) {
    let mut backoff = Backoff::default();
    let mut start_warned = false;
    loop {
        let started = Instant::now();
        match start_child(&luna, uri, r#"{"subscribe":true}"#) {
            Ok((mut child, mut lines)) => {
                log::info!(
                    "{label}: subscribed to the {what} ({} pid {})",
                    luna.display(),
                    child.id().unwrap_or(0)
                );
                start_warned = false;
                let mut first_by = Some(Instant::now() + FIRST_LINE_WAIT);
                loop {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => {
                            let _ = child.kill().await;
                            return;
                        }
                        line = lines.recv() => {
                            let Some(line) = line else { break };
                            match on_line(&line) {
                                LineOutcome::Answer => first_by = None,
                                LineOutcome::Noise => {}
                                LineOutcome::Closed => {
                                    let _ = child.kill().await;
                                    return;
                                }
                            }
                        }
                        () = crate::daemon::sleep_opt(first_by) => {
                            first_by = None;
                            log::warn!("{label}: no answer from luna-send within {} s; still waiting", FIRST_LINE_WAIT.as_secs());
                        }
                    }
                }
                let status = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                let status = match status {
                    Ok(Ok(s)) => s.to_string(),
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => {
                        let _ = child.kill().await;
                        "killed (its terminal closed)".to_owned()
                    }
                };
                log::warn!(
                    "{label}: the subscription ended after {} s ({status})",
                    started.elapsed().as_secs()
                );
            }
            Err(e) => {
                if start_warned {
                    log::debug!("{label}: cannot start {}: {e}", luna.display());
                } else {
                    start_warned = true;
                    log::warn!("{label}: cannot start {}: {e}", luna.display());
                }
            }
        }
        let delay = backoff.after_exit(started.elapsed());
        log::info!(
            "{label}: restarting the subscription in {} s",
            delay.as_secs()
        );
        tokio::select! {
            _ = stop.changed() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// One `luna-send -n 1` call; returns the reply without whitespace (or the
/// error). Over a pipe: one-shot `tvpower` calls answered that way on c3-b.
pub(crate) async fn call(luna: &Path, uri: &str, payload: &str) -> Result<String, String> {
    let child = Command::new(luna)
        .args(["-n", "1", "-w", "3000", uri, payload])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", luna.display()))?;
    let out = tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("no answer within {} s", CALL_TIMEOUT.as_secs()))?
        .map_err(|e| e.to_string())?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(compact(&text))
}

/// One `luna-send -n 1` call on a pseudo-terminal, for TV control: over a
/// plain pipe the audio methods print nothing on the TV (EXP-010 Part A,
/// c3-b). Returns the raw output (whitespace kept: titles and labels have
/// spaces), at most `max` bytes; more is an error that says so without the
/// text.
pub(crate) async fn call_pty(
    luna: &Path,
    uri: &str,
    payload: &str,
    max: usize,
) -> Result<String, String> {
    call_pty_wait(luna, uri, payload, max, 3000).await
}

/// [`call_pty`] with `luna-send`'s reply wait (`-w`) in milliseconds.
pub(crate) async fn call_pty_wait(
    luna: &Path,
    uri: &str,
    payload: &str,
    max: usize,
    wait_ms: u32,
) -> Result<String, String> {
    let (master, slave) = open_pty().map_err(|e| format!("cannot open a terminal: {e}"))?;
    let mut cmd = Command::new(luna);
    let wait = wait_ms.to_string();
    cmd.args(["-n", "1", "-w", wait.as_str(), uri, payload])
        .stdin(Stdio::from(
            slave.try_clone().map_err(|e| format!("terminal: {e}"))?,
        ))
        .stdout(Stdio::from(
            slave.try_clone().map_err(|e| format!("terminal: {e}"))?,
        ))
        .stderr(Stdio::from(slave))
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", luna.display()))?;
    // Drop the slave descriptors the Command holds, so the master read ends
    // (EIO) when the child is gone.
    drop(cmd);
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("luna-call".into())
        .spawn(move || {
            use std::io::Read as _;
            let mut master = master;
            let mut total = 0_usize;
            let mut buf = [0_u8; 4096];
            loop {
                match master.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        total += n;
                        if tx.send(buf[..n].to_vec()).is_err() || total > max {
                            return;
                        }
                    }
                }
            }
        })
        .map_err(|e| format!("cannot start a reader thread: {e}"))?;
    let collect = async {
        let mut out: Vec<u8> = Vec::new();
        while let Some(chunk) = rx.recv().await {
            out.extend_from_slice(&chunk);
            if out.len() > max {
                return Err(format!("reply of more than {max} bytes"));
            }
        }
        Ok(out)
    };
    let out = tokio::time::timeout(CALL_TIMEOUT, collect)
        .await
        .map_err(|_| format!("no answer within {} s", CALL_TIMEOUT.as_secs()))??;
    // The terminal's end of input came with the child's exit; reap it.
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_settled_and_transitional_lines() {
        let l = r#"{"returnValue":true,"state":"Active","subscribed":true}"#;
        let Line::Update(u) = parse_line(l) else {
            panic!()
        };
        assert_eq!(u.state.as_deref(), Some("Active"));
        assert!(u.settled());
        assert_eq!(u.text(), "Active");
        // The stub format of the D6 watcher test, with spaces.
        let l = r#"{ "returnValue": true, "state": "Active Standby", "processing": "Request Power Off", "onOff": "off", "reason": "remoteKey" }"#;
        let Line::Update(u) = parse_line(l) else {
            panic!()
        };
        assert_eq!(u.state_key(), "ActiveStandby");
        assert_eq!(u.processing_key(), "RequestPowerOff");
        assert_eq!(u.reason.as_deref(), Some("remoteKey"));
        assert_eq!(u.on_off.as_deref(), Some("off"));
        assert!(!u.settled());
        assert_eq!(u.text(), "Active Standby (Request Power Off)");
        assert_eq!(
            u.describe(),
            "state Active Standby, processing Request Power Off, reason remoteKey, onOff off"
        );
        // A processing-only update.
        let Line::Update(u) = parse_line(r#"{"processing":"Prepare Resume","reason":"wakeOnLan"}"#)
        else {
            panic!()
        };
        assert_eq!(u.state, None);
        assert_eq!(u.text(), "unknown (Prepare Resume)");
    }

    #[test]
    fn errors_and_noise() {
        assert_eq!(
            parse_line(r#"{"returnValue":false,"errorCode":-1,"errorText":"Denied"}"#),
            Line::Error("Denied".to_owned())
        );
        assert_eq!(parse_line(""), Line::Other);
        assert_eq!(parse_line("** Message: connecting"), Line::Other);
        assert_eq!(parse_line(r#"{"returnValue":true}"#), Line::Other);
        assert_eq!(
            json_str(r#"{"a":"x\"y","powerState":"no","state":"ok"}"#, "a").as_deref(),
            Some("x\"y")
        );
        assert_eq!(
            json_str(r#"{"powerState":"no","state":"ok"}"#, "state").as_deref(),
            Some("ok")
        );
        assert_eq!(json_str(r#"{"state":5}"#, "state"), None);
        assert_eq!(json_str(r#"{"state":"open"#, "state"), None);
        assert_eq!(
            json_bool(r#"{"returnValue" : true}"#, "returnValue"),
            Some(true)
        );
    }

    #[test]
    fn backoff_doubles_caps_and_resets_after_a_healthy_run() {
        let mut b = Backoff::default();
        let quick = Duration::from_millis(100);
        let secs: Vec<u64> = (0..8).map(|_| b.after_exit(quick).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(b.after_exit(Duration::from_secs(61)).as_secs(), 1);
        assert_eq!(b.after_exit(quick).as_secs(), 2);
        for _ in 0..100 {
            assert!(b.after_exit(quick) <= BACKOFF_MAX);
        }
    }

    #[test]
    fn finds_binaries_by_path_and_name() {
        assert_eq!(find_binary(Path::new("/nonexistent/luna-send")), None);
        assert_eq!(find_binary(Path::new("")), None);
        assert_eq!(
            find_binary(Path::new("definitely-not-a-binary-webos-sat")),
            None
        );
        assert!(find_binary(Path::new("sh")).is_some());
    }

    fn fake_luna(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("luna-send");
        crate::testutil::write_script(&p, body);
        p
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscription_runs_on_a_terminal_restarts_and_stops() {
        let dir = std::env::temp_dir().join(format!("webos-sat-power-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Prints whether stdout is a terminal, two updates, then exits; the
        // supervisor starts it again after 1 s.
        let luna = fake_luna(
            &dir,
            r#"[ -t 1 ] && t=Active || t=NOTATTY
printf '{"returnValue":true,"state":"%s","subscribed":true}\n' "$t"
printf '{"returnValue":true,"state":"Active Standby","processing":"Request Power Off","onOff":"off","reason":"remoteKey"}\n'"#,
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let w = spawn(luna, tx);
        let mut got = Vec::new();
        while got.len() < 4 {
            let u = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .unwrap()
                .unwrap();
            got.push(u);
        }
        assert_eq!(
            got[0].state.as_deref(),
            Some("Active"),
            "stdout must be a tty"
        );
        assert_eq!(got[1].reason.as_deref(), Some("remoteKey"));
        assert_eq!(got[2].state.as_deref(), Some("Active"), "restarted");
        w.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_subscription_child_dies_with_the_thread_that_spawned_it() {
        let dir = std::env::temp_dir().join(format!("webos-sat-power-pd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pidfile = dir.join("pid");
        let luna = fake_luna(
            &dir,
            &format!("echo $$ > '{}'\nexec sleep 30", pidfile.display()),
        );
        // A thread with its own runtime spawns the child and ends without
        // killing it (as if the daemon died): kill_on_drop is bypassed.
        let read_pid = |f: &Path| {
            std::fs::read_to_string(f)
                .ok()
                .and_then(|t| t.trim().parse::<i32>().ok())
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (l, pf) = (luna.clone(), pidfile.clone());
        let pid = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let (child, _lines) = start_child(&l, "luna://x/y", "{}").unwrap();
                std::mem::forget(child);
            });
            // Wait until the child runs its script, then end the thread.
            loop {
                if let Some(p) = read_pid(&pf) {
                    break p;
                }
                assert!(std::time::Instant::now() < deadline, "no pid");
                std::thread::sleep(Duration::from_millis(20));
            }
        })
        .join()
        .unwrap();
        let alive = || {
            Path::new(&format!("/proc/{pid}")).exists()
                && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .unwrap_or_default()
                    .contains(") Z ")
        };
        while alive() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(), "child {pid} outlived the thread that spawned it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_kills_a_running_child() {
        let dir = std::env::temp_dir().join(format!("webos-sat-power-k-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pidfile = dir.join("pid");
        let luna = fake_luna(
            &dir,
            &format!(
                "echo $$ > '{}'\nprintf '{{\"state\":\"Active\"}}\\n'\nexec sleep 30",
                pidfile.display()
            ),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let w = spawn(luna, tx);
        let u = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(u.state.as_deref(), Some("Active"));
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        w.shutdown().await;
        // The child (exec'd into sleep) is gone and reaped.
        let alive = Path::new(&format!("/proc/{pid}")).exists()
            && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .unwrap_or_default()
                .contains(") Z ");
        assert!(!alive, "child {pid} still running");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn one_shot_call_returns_the_compact_reply() {
        let dir = std::env::temp_dir().join(format!("webos-sat-power-c-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let luna = fake_luna(
            &dir,
            r#"echo "{ \"returnValue\": false, \"errorText\": \"Keyblock1.5second\", \"args\": \"$*\" }""#,
        );
        let r = call(&luna, "luna://x/y", r#"{"reason":"remoteKey"}"#)
            .await
            .unwrap();
        assert!(r.contains("Keyblock1.5second"), "{r}");
        assert!(r.contains("-n1-w3000luna://x/y"), "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn call_pty_runs_on_a_terminal_and_keeps_whitespace() {
        let dir = std::env::temp_dir().join(format!("webos-sat-power-t-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let luna = fake_luna(
            &dir,
            r#"[ -t 1 ] && t=tty || t=pipe
echo "{ \"returnValue\": true, \"title\": \"App  A\", \"tty\": \"$t\", \"args\": \"$5 $6\" }""#,
        );
        let r = call_pty(&luna, "luna://x/y", r#"{"a":1}"#, 1 << 20)
            .await
            .unwrap();
        assert!(r.contains("\"App  A\""), "spaces are kept: {r}");
        assert!(r.contains("\"tty\": \"tty\""), "{r}");
        assert!(r.contains(r#"luna://x/y {"a":1}"#), "{r}");
        // A big reply over the limit is an error that names no content.
        let big = fake_luna(&dir, "head -c 5000 /dev/zero | tr '\\0' 'x'; echo");
        let e = call_pty(&big, "luna://x/y", "{}", 1000).await.unwrap_err();
        assert!(
            e.contains("more than 1000 bytes") && !e.contains("xxx"),
            "{e}"
        );
        // A binary that cannot start.
        let e = call_pty(Path::new("/nonexistent/luna-send"), "u", "{}", 1000)
            .await
            .unwrap_err();
        assert!(e.contains("cannot start"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generic_subscription_forwards_json_lines_and_stops() {
        let dir = std::env::temp_dir().join(format!("webos-sat-power-s-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let luna = fake_luna(
            &dir,
            r#"echo "** Message: noise"
echo "{\"returnValue\":true,\"subscribed\":true,\"uri\":\"$2\",\"payload\":\"$3\"}"
exec sleep 30"#,
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let w = spawn_lines(luna, "luna://svc/get", "test", "thing", move |l| {
            tx.send(l).is_ok()
        });
        let line = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            line.contains(r#""subscribed":true"#) && line.contains("luna://svc/get"),
            "{line}"
        );
        assert!(
            line.contains(r#"{\"subscribe\":true}"#) || line.contains("subscribe"),
            "{line}"
        );
        w.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
