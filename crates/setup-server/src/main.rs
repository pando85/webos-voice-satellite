//! `webos-satellite-setup`: the phone setup web server of the TV app
//! (ADR 0009, docs/design/phone-setup.md). A short-lived, std-only HTTP/1.1
//! subset server: one connection at a time on one thread, `ctl.sh` commands
//! on a second (worker) thread, every change approved on the TV remote
//! (except turning the microphone off), and the session state in a volatile
//! directory for `ctl.sh setup status`.

mod actions;
mod assets;
mod config;
mod files;
mod http;
mod log;
mod mirror;
mod secret;
mod session;
mod worker;

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crate::actions::CmdOutcome;
use crate::config::{Command, Config};
use crate::files::{Dir, Taken};
use crate::http::ReadError;
use crate::log::Log;
use crate::session::{Core, EndReason};
use crate::worker::{Job, JobDone, Runner};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TARGET: &str = env!("WEBOS_SETUP_TARGET");
/// The install marker `install.sh` writes (packaging/webos/lib.sh).
const MARKER: &str = ".webos-satellite-install";
const TICK: Duration = Duration::from_millis(100);
const READ_LIMIT: Duration = Duration::from_secs(3);
const WRITE_LIMIT: Duration = Duration::from_secs(3);
const STATE_EVERY: Duration = Duration::from_secs(2);
const BACKLOG: i32 = 4;

fn main() {
    std::process::exit(run_main());
}

fn run_main() -> i32 {
    match config::parse(std::env::args_os().skip(1), |k| std::env::var_os(k)) {
        Err(msg) => {
            eprintln!("webos-satellite-setup: {msg}");
            eprint!("{}", config::USAGE);
            2
        }
        Ok(Command::Version) => {
            println!("webos-satellite-setup {VERSION} ({TARGET})");
            0
        }
        Ok(Command::Help) => {
            print!("{}", config::USAGE);
            0
        }
        Ok(Command::Run(cfg)) => match serve(&cfg) {
            Ok(()) => 0,
            Err(msg) => {
                eprintln!("webos-satellite-setup: {msg}");
                1
            }
        },
    }
}

/// Private, link-local or (as a test switch) loopback peers only; never
/// `100.64/10` (section 5.1).
fn peer_allowed(ip: Ipv4Addr, allow_loopback: bool) -> bool {
    ip.is_private() || ip.is_link_local() || (allow_loopback && ip.is_loopback())
}

/// An IPv4 listener on a kernel-chosen port, backlog 4, close-on-exec (the
/// children never inherit it), non-blocking.
fn listen(ip: Ipv4Addr) -> std::io::Result<TcpListener> {
    use rustix::net::{AddressFamily, SocketFlags, SocketType};
    let fd = rustix::net::socket_with(
        AddressFamily::INET,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )?;
    rustix::net::bind(&fd, &SocketAddrV4::new(ip, 0))?;
    rustix::net::listen(&fd, BACKLOG)?;
    let l = TcpListener::from(fd);
    l.set_nonblocking(true)?;
    Ok(l)
}

fn serve(cfg: &Config) -> Result<(), String> {
    if !cfg.prefix.join(MARKER).is_file() {
        return Err(format!(
            "{} is not a webos-satellite install (no {MARKER})",
            cfg.prefix.display()
        ));
    }
    if !cfg.ctl.is_file() {
        return Err(format!("ctl.sh not found: {}", cfg.ctl.display()));
    }
    let dir = Dir::prepare(&cfg.dir)
        .map_err(|e| format!("session directory {}: {e}", cfg.dir.display()))?;
    let mut log = Log::open(&cfg.log);
    let listener = listen(cfg.bind).map_err(|e| format!("cannot listen: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot listen: {e}"))?
        .port();
    let mut core = Core::new(cfg.timing, Box::new(secret::Urandom), port, Instant::now())
        .map_err(|e| format!("no random source: {e}"))?;
    let mut env = vec![(
        "WEBOS_SAT_PREFIX".to_owned(),
        cfg.prefix.clone().into_os_string(),
    )];
    for k in worker::PASS_ENV {
        if let Some(v) = std::env::var_os(k) {
            env.push(((*k).to_owned(), v));
        }
    }
    let runner = Runner {
        ctl: cfg.ctl.clone(),
        timeout: cfg.command_timeout,
        env,
    };
    let (jobs, done) =
        worker::spawn(runner).map_err(|e| format!("cannot start the worker: {e}"))?;

    let mut last_beat = dir.heartbeat();
    let mut last_write: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if dir.stop_requested() {
            core.end(EndReason::Done, now);
        }
        let beat = dir.heartbeat();
        if beat.is_some() && beat != last_beat {
            core.on_heartbeat(now);
            last_beat = beat;
        }
        match dir.take_answer() {
            Taken::Answer(a) => core.on_answer(&a, now),
            Taken::Invalid => log.line("answer invalid"),
            Taken::Nothing => {}
        }
        while let Ok(d) = done.try_recv() {
            core.on_job_done(d, now);
        }
        core.tick(now);
        if let Some(job) = core.next_job(now)
            && let Err(e) = jobs.send(job)
        {
            // The worker is gone: report the job as failed, never hang.
            let failed = match e.0 {
                Job::Refresh => JobDone::Refresh {
                    device: Err("worker stopped".into()),
                    tv: Err("worker stopped".into()),
                },
                Job::Action { id, action } => JobDone::Action {
                    id,
                    action,
                    outcome: CmdOutcome::Failed("worker stopped"),
                },
            };
            core.on_job_done(failed, now);
        }
        for l in core.take_log() {
            log.line(&l);
        }
        if core.ended().is_some() {
            break;
        }
        if core.take_dirty()
            || last_write.is_none_or(|t| now.saturating_duration_since(t) >= STATE_EVERY)
        {
            write_state(&dir, &core, now, &mut log);
            last_write = Some(now);
        }
        match listener.accept() {
            Ok((stream, SocketAddr::V4(peer))) => {
                connection(stream, *peer.ip(), &mut core, &dir, cfg.allow_loopback);
            }
            Ok(_) => {}
            Err(_) => thread::sleep(TICK),
        }
    }
    // Closed first, so the port is gone before the TV app sees "ended".
    drop(listener);
    dir.remove(files::ANSWER);
    write_state(&dir, &core, Instant::now(), &mut log);
    for l in core.take_log() {
        log.line(&l);
    }
    Ok(())
}

fn write_state(dir: &Dir, core: &Core, now: Instant, log: &mut Log) {
    let bytes = serde_json::to_vec(&core.state_json(now)).unwrap_or_default();
    if dir.write_state(&bytes).is_err() {
        log.line("cannot write state.json");
    }
}

/// Serves one connection: at most one request, then close.
fn connection(
    mut stream: TcpStream,
    peer: Ipv4Addr,
    core: &mut Core,
    dir: &Dir,
    allow_loopback: bool,
) {
    if !peer_allowed(peer, allow_loopback) {
        core.on_rejected_peer(peer, Instant::now());
        return;
    }
    let Ok(SocketAddr::V4(local)) = stream.local_addr() else {
        return;
    };
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let deadline = Instant::now() + READ_LIMIT;
    let resp = match http::read_request(&mut stream, deadline, || dir.stop_requested()) {
        Ok(req) => core.handle(peer, local, &req, Instant::now()),
        Err(ReadError::Empty) => return,
        Err(ReadError::Parse(e)) => core.on_parse_error(peer, e, Instant::now()),
    };
    let _ = stream.set_write_timeout(Some(WRITE_LIMIT));
    let _ = stream.write_all(&resp.to_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
    // Read what the client may still send (briefly), so closing does not
    // reset the connection before it read the reply.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut sink = [0u8; 2048];
    for _ in 0..4 {
        match stream.read(&mut sink) {
            Ok(n) if n > 0 => {}
            _ => break,
        }
    }
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

    #[test]
    fn peer_filter() {
        for ip in [
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.23",
            "169.254.1.1",
        ] {
            assert!(peer_allowed(ip.parse().expect("ip"), false), "{ip}");
        }
        for ip in [
            "100.64.0.1",
            "100.127.255.254",
            "8.8.8.8",
            "172.32.0.1",
            "192.169.0.1",
            "127.0.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(!peer_allowed(ip.parse().expect("ip"), false), "{ip}");
        }
        assert!(peer_allowed(Ipv4Addr::LOCALHOST, true));
    }

    #[test]
    fn listener_is_random_and_ipv4() {
        let a = listen(Ipv4Addr::LOCALHOST).expect("listen");
        let b = listen(Ipv4Addr::LOCALHOST).expect("listen");
        let (pa, pb) = (a.local_addr().expect("addr"), b.local_addr().expect("addr"));
        assert!(pa.is_ipv4() && pb.is_ipv4());
        assert_ne!(pa.port(), pb.port());
        assert_ne!(pa.port(), 6053);
    }
}
