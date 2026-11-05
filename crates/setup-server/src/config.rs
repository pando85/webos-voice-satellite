//! Command line (no config file: the server never reads the satellite's
//! config or key).

use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use crate::session::Timing;

pub(crate) const USAGE: &str = "\
usage: webos-satellite-setup [OPTIONS]
       webos-satellite-setup --version | --help

The phone setup web server (docs/design/phone-setup.md). Started only by
`ctl.sh setup start`; ends by itself (done, 10 minutes, idle, heartbeat lost,
refusals, bad requests, parse errors). Prints nothing on success: the session
state is in DIR/state.json.

  --prefix DIR       install directory, must hold the install marker
                     (default $WEBOS_SAT_PREFIX or /var/lib/webos-satellite)
  --ctl FILE         the ctl.sh to run (default PREFIX/bin/ctl.sh)
  --dir DIR          session directory, created 0700 (default
                     $WEBOS_SAT_SETUP_DIR or /tmp/webos-satellite-setup)
  --log FILE         event log, appended, 0600 (default PREFIX/log/setup.log)
  --bind IPV4        listen address (default 0.0.0.0); the port is random

Test switches (each may only shorten the design value):
  --lifetime-s N (600)  --idle-s N (180)  --heartbeat-grace-s N (15)
  --request-timeout-s N (60)  --command-timeout-s N (25)  --cooldown-s N (3)
  --refresh-s N (3)  --allow-loopback (accept 127.0.0.0/8 peers)
";

pub(crate) const DEFAULT_PREFIX: &str = "/var/lib/webos-satellite";
pub(crate) const DEFAULT_DIR: &str = "/tmp/webos-satellite-setup";

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) prefix: PathBuf,
    pub(crate) ctl: PathBuf,
    pub(crate) dir: PathBuf,
    pub(crate) log: PathBuf,
    pub(crate) bind: Ipv4Addr,
    pub(crate) timing: Timing,
    pub(crate) command_timeout: Duration,
    pub(crate) allow_loopback: bool,
}

#[derive(Debug)]
pub(crate) enum Command {
    Version,
    Help,
    Run(Box<Config>),
}

fn secs(v: &str, min: u64, max: u64, flag: &str) -> Result<Duration, String> {
    match v.parse::<u64>() {
        Ok(n) if (min..=max).contains(&n) && v.bytes().all(|b| b.is_ascii_digit()) => {
            Ok(Duration::from_secs(n))
        }
        _ => Err(format!(
            "{flag} takes a whole number of seconds from {min} to {max}"
        )),
    }
}

fn abs(v: &str, flag: &str) -> Result<PathBuf, String> {
    if v.starts_with('/') {
        Ok(PathBuf::from(v))
    } else {
        Err(format!("{flag} needs an absolute path"))
    }
}

/// Parses the arguments (without the program name). `env` reads the
/// environment (a closure for tests).
pub(crate) fn parse(
    args: impl IntoIterator<Item = OsString>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<Command, String> {
    let mut args = args.into_iter().map(|a| {
        a.into_string()
            .map_err(|_| "arguments must be UTF-8".to_owned())
    });
    let mut prefix: Option<PathBuf> = None;
    let mut ctl = None;
    let mut dir = None;
    let mut log = None;
    let mut bind = Ipv4Addr::UNSPECIFIED;
    let mut t = Timing::default();
    let mut command_timeout = Duration::from_secs(25);
    let mut allow_loopback = false;
    while let Some(a) = args.next() {
        let a = a?;
        match a.as_str() {
            "--version" | "-V" => return Ok(Command::Version),
            "--help" | "-h" => return Ok(Command::Help),
            "--allow-loopback" => {
                allow_loopback = true;
                continue;
            }
            _ => {}
        }
        let v = args.next().ok_or_else(|| format!("{a} needs a value"))??;
        match a.as_str() {
            "--prefix" => prefix = Some(abs(&v, &a)?),
            "--ctl" => ctl = Some(abs(&v, &a)?),
            "--dir" => dir = Some(abs(&v, &a)?),
            "--log" => log = Some(abs(&v, &a)?),
            "--bind" => {
                bind = v
                    .parse()
                    .map_err(|_| "--bind takes an IPv4 address".to_owned())?;
            }
            "--lifetime-s" => t.lifetime = secs(&v, 1, 600, &a)?,
            "--idle-s" => t.idle = secs(&v, 1, 180, &a)?,
            "--heartbeat-grace-s" => t.heartbeat_grace = secs(&v, 1, 15, &a)?,
            "--request-timeout-s" => t.request_timeout = secs(&v, 1, 60, &a)?,
            "--command-timeout-s" => command_timeout = secs(&v, 1, 25, &a)?,
            "--cooldown-s" => t.cooldown = secs(&v, 0, 3, &a)?,
            "--refresh-s" => t.refresh = secs(&v, 1, 3, &a)?,
            _ => return Err(format!("unknown option {a}")),
        }
    }
    t.app_gone = t.app_gone.min(t.heartbeat_grace);
    let from_env = |k: &str, flag: &str| -> Result<Option<PathBuf>, String> {
        match env(k) {
            None => Ok(None),
            Some(v) => {
                let v = v.into_string().map_err(|_| format!("{k} must be UTF-8"))?;
                abs(&v, flag).map(Some)
            }
        }
    };
    let prefix = match prefix {
        Some(p) => p,
        None => from_env("WEBOS_SAT_PREFIX", "WEBOS_SAT_PREFIX")?
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PREFIX)),
    };
    let dir = match dir {
        Some(d) => d,
        None => from_env("WEBOS_SAT_SETUP_DIR", "WEBOS_SAT_SETUP_DIR")?
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DIR)),
    };
    Ok(Command::Run(Box::new(Config {
        ctl: ctl.unwrap_or_else(|| prefix.join("bin/ctl.sh")),
        log: log.unwrap_or_else(|| prefix.join("log/setup.log")),
        prefix,
        dir,
        bind,
        timing: t,
        command_timeout,
        allow_loopback,
    })))
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

    fn run(args: &[&str], env: &[(&str, &str)]) -> Result<Config, String> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        match parse(args.iter().map(OsString::from), |k| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| OsString::from(v))
        })? {
            Command::Run(c) => Ok(*c),
            other => Err(format!("{other:?}")),
        }
    }

    #[test]
    fn defaults() {
        let c = run(&[], &[]).expect("config");
        assert_eq!(c.prefix, PathBuf::from(DEFAULT_PREFIX));
        assert_eq!(c.ctl, PathBuf::from("/var/lib/webos-satellite/bin/ctl.sh"));
        assert_eq!(
            c.log,
            PathBuf::from("/var/lib/webos-satellite/log/setup.log")
        );
        assert_eq!(c.dir, PathBuf::from(DEFAULT_DIR));
        assert_eq!(c.bind, Ipv4Addr::UNSPECIFIED);
        assert_eq!(c.timing.lifetime, Duration::from_secs(600));
        assert_eq!(c.timing.app_gone, Duration::from_secs(8));
        assert!(!c.allow_loopback);
        let c = run(
            &[],
            &[("WEBOS_SAT_PREFIX", "/p"), ("WEBOS_SAT_SETUP_DIR", "/d")],
        )
        .expect("config");
        assert_eq!(c.ctl, PathBuf::from("/p/bin/ctl.sh"));
        assert_eq!(c.dir, PathBuf::from("/d"));
    }

    #[test]
    fn flags() {
        let c = run(
            &[
                "--prefix",
                "/p",
                "--ctl",
                "/c.sh",
                "--dir",
                "/d",
                "--log",
                "/l",
                "--bind",
                "127.0.0.1",
                "--lifetime-s",
                "2",
                "--heartbeat-grace-s",
                "2",
                "--allow-loopback",
                "--cooldown-s",
                "0",
            ],
            &[],
        )
        .expect("config");
        assert_eq!(c.ctl, PathBuf::from("/c.sh"));
        assert_eq!(c.bind, Ipv4Addr::LOCALHOST);
        assert_eq!(c.timing.lifetime, Duration::from_secs(2));
        assert_eq!(c.timing.app_gone, Duration::from_secs(2));
        assert_eq!(c.timing.cooldown, Duration::ZERO);
        assert!(c.allow_loopback);
    }

    #[test]
    fn refusals() {
        for args in [
            &["--lifetime-s", "601"][..],
            &["--lifetime-s", "0"],
            &["--lifetime-s", "+5"],
            &["--idle-s", "181"],
            &["--heartbeat-grace-s", "16"],
            &["--command-timeout-s", "26"],
            &["--bind", "::1"],
            &["--bind", "localhost"],
            &["--prefix", "relative"],
            &["--prefix"],
            &["--token", "x"],
        ] {
            assert!(run(args, &[]).is_err(), "{args:?}");
        }
        assert!(run(&[], &[("WEBOS_SAT_PREFIX", "rel")]).is_err());
        assert!(matches!(
            parse([OsString::from("--version")], |_| None),
            Ok(Command::Version)
        ));
    }
}
