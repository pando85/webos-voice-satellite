//! The worker thread (section 3.2): runs `ctl.sh` commands, one at a time,
//! so the HTTP loop never waits for one. Each command runs without a shell,
//! with a fixed argument list, a cleared environment (a fixed `PATH` plus an
//! allow-list of `WEBOS_SAT_*` variables), stdin from `/dev/null`, stdout
//! capped at 64 KiB, stderr discarded, in its own process group, and is
//! killed (the whole group) at its time limit.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::actions::{Action, CmdOutcome};

const PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const MAX_OUT: usize = 64 * 1024;

/// Variables passed through to `ctl.sh` (they select the install, tests'
/// fakes and timings; none carries a secret). `WEBOS_SAT_PREFIX` is always
/// set from `--prefix`.
pub(crate) const PASS_ENV: &[&str] = &[
    "WEBOS_SAT_INITD",
    "WEBOS_SAT_LOCK",
    "WEBOS_SAT_ALLOW_NONROOT",
    "WEBOS_SAT_LUNA_SEND",
    "WEBOS_SAT_SETTLE",
];

#[derive(Debug, Clone)]
pub(crate) enum Job {
    /// `ctl.sh status --json` and `ctl.sh tv-settings --json`.
    Refresh,
    Action {
        id: String,
        action: Action,
    },
}

#[derive(Debug)]
pub(crate) enum JobDone {
    Refresh {
        device: Result<Vec<u8>, String>,
        tv: Result<Vec<u8>, String>,
    },
    Action {
        id: String,
        action: Action,
        outcome: CmdOutcome,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Runner {
    pub(crate) ctl: PathBuf,
    pub(crate) timeout: Duration,
    pub(crate) env: Vec<(String, OsString)>,
}

impl Runner {
    pub(crate) fn run(&self, args: &[String]) -> CmdOutcome {
        let mut cmd = Command::new(&self.ctl);
        cmd.args(args)
            .env_clear()
            .env("PATH", PATH)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        // ETXTBSY: the file was just written while another thread forked
        // (seen in tests); a short retry, never more.
        let mut spawned = cmd.spawn();
        for _ in 0..3 {
            match &spawned {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    thread::sleep(Duration::from_millis(50));
                    spawned = cmd.spawn();
                }
                _ => break,
            }
        }
        let Ok(mut child) = spawned else {
            return CmdOutcome::Failed("cannot start ctl.sh");
        };
        let stdout = child.stdout.take();
        let (tx, rx) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("setup-read".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut buf = Vec::new();
                if let Some(s) = stdout {
                    let _ = s.take(MAX_OUT as u64 + 1).read_to_end(&mut buf);
                }
                let _ = tx.send(buf);
            });
        let deadline = Instant::now() + self.timeout;
        let kill = |child: &mut std::process::Child| {
            if let Some(pid) = i32::try_from(child.id())
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        };
        if reader.is_err() {
            kill(&mut child);
            return CmdOutcome::Failed("cannot read ctl.sh");
        }
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    kill(&mut child);
                    return CmdOutcome::TimedOut;
                }
                Err(_) => {
                    kill(&mut child);
                    return CmdOutcome::Failed("cannot wait for ctl.sh");
                }
            }
        };
        // A grandchild still holding the pipe must not block the worker.
        let Ok(stdout) = rx.recv_timeout(Duration::from_secs(2)) else {
            return CmdOutcome::Failed("ctl.sh output did not end");
        };
        if stdout.len() > MAX_OUT {
            return CmdOutcome::Failed("ctl.sh output too large");
        }
        CmdOutcome::Exited {
            success: status.success(),
            code: status.code(),
            stdout,
        }
    }

    fn json(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        match self.run(&args) {
            CmdOutcome::Exited { stdout, .. } if !stdout.is_empty() => Ok(stdout),
            CmdOutcome::Exited { code, .. } => Err(code.map_or_else(
                || "killed".to_owned(),
                |c| format!("no output (exit status {c})"),
            )),
            CmdOutcome::TimedOut => Err("timeout".to_owned()),
            CmdOutcome::Failed(why) => Err(why.to_owned()),
        }
    }

    fn handle(&self, job: Job) -> JobDone {
        match job {
            Job::Refresh => JobDone::Refresh {
                device: self.json(&["status", "--json"]),
                tv: self.json(&["tv-settings", "--json"]),
            },
            Job::Action { id, action } => JobDone::Action {
                id,
                action,
                outcome: self.run(&action.argv()),
            },
        }
    }
}

/// Starts the worker; it ends when the job sender is dropped.
pub(crate) fn spawn(runner: Runner) -> std::io::Result<(Sender<Job>, Receiver<JobDone>)> {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (done_tx, done_rx) = mpsc::channel();
    thread::Builder::new()
        .name("setup-worker".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            while let Ok(job) = job_rx.recv() {
                if done_tx.send(runner.handle(job)).is_err() {
                    break;
                }
            }
        })?;
    Ok((job_tx, done_rx))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::assert_is_empty,
        clippy::format_push_string,
        clippy::cast_possible_truncation
    )]

    use super::*;
    use crate::files::tests::scratch;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn fake(script: &str) -> (PathBuf, Runner) {
        let dir = scratch("worker");
        fs::create_dir_all(&dir).expect("mkdir");
        let ctl = dir.join("ctl.sh");
        fs::write(&ctl, format!("#!/bin/sh\n{script}\n")).expect("write");
        fs::set_permissions(&ctl, fs::Permissions::from_mode(0o755)).expect("chmod");
        let runner = Runner {
            ctl,
            timeout: Duration::from_millis(1500),
            env: vec![("WEBOS_SAT_PREFIX".into(), "/x".into())],
        };
        (dir, runner)
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arguments_arrive_verbatim_and_the_environment_is_cleared() {
        let (dir, r) = fake(
            r#"printf '%s|' "$@"; printf 'PATH=%s PREFIX=%s HOME=%s PWD=%s' "$PATH" "$WEBOS_SAT_PREFIX" "${HOME:-unset}" "$(pwd)""#,
        );
        let out = r.run(&args(&["set", "hi-lg", "off; reboot"]));
        let CmdOutcome::Exited {
            success: true,
            stdout,
            ..
        } = out
        else {
            panic!("{out:?}")
        };
        let text = String::from_utf8(stdout).expect("utf8");
        assert_eq!(
            text,
            format!("set|hi-lg|off; reboot|PATH={PATH} PREFIX=/x HOME=unset PWD=/")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_hung_command_and_its_children_are_killed() {
        let (dir, r) = fake("sleep 30 & sleep 30");
        let start = Instant::now();
        assert_eq!(r.run(&args(&["x"])), CmdOutcome::TimedOut);
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn output_is_capped() {
        let (dir, r) = fake("head -c 70000 /dev/zero");
        assert_eq!(
            r.run(&args(&["x"])),
            CmdOutcome::Failed("ctl.sh output too large")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exit_status_and_missing_binary() {
        let (dir, r) = fake("exit 3");
        assert_eq!(
            r.run(&args(&["x"])),
            CmdOutcome::Exited {
                success: false,
                code: Some(3),
                stdout: vec![]
            }
        );
        let missing = Runner {
            ctl: dir.join("nope"),
            ..r
        };
        assert_eq!(
            missing.run(&args(&["x"])),
            CmdOutcome::Failed("cannot start ctl.sh")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_worker_thread_runs_jobs() {
        let (dir, r) = fake(
            r#"case $1 in status) echo '{"running":true}';; tv-settings) exit 1;; mic) exit 0;; esac"#,
        );
        let (tx, rx) = spawn(r).expect("spawn");
        tx.send(Job::Refresh).expect("send");
        match rx.recv_timeout(Duration::from_secs(5)).expect("done") {
            JobDone::Refresh { device, tv } => {
                assert_eq!(device.expect("device"), b"{\"running\":true}\n");
                assert_eq!(tv, Err("no output (exit status 1)".into()));
            }
            other @ JobDone::Action { .. } => panic!("{other:?}"),
        }
        tx.send(Job::Action {
            id: "00000001".into(),
            action: Action::Mic(false),
        })
        .expect("send");
        match rx.recv_timeout(Duration::from_secs(5)).expect("done") {
            JobDone::Action { id, outcome, .. } => {
                assert_eq!(id, "00000001");
                assert!(matches!(outcome, CmdOutcome::Exited { success: true, .. }));
            }
            other @ JobDone::Refresh { .. } => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }
}
