//! `webos-satellite`: Home Assistant voice satellite for rooted LG webOS TVs.
//!
//! Phase 1.1 scope: print version and build target; `--self-test` reports
//! runtime facts (uname, page size, rlimits, memory) read-only. No audio device
//! and no network access.

use std::fmt::Write as _;
use std::process::ExitCode;

use rustix::process::{Resource, getrlimit};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TARGET: &str = env!("WEBOS_SATELLITE_TARGET");

const USAGE: &str = "usage: webos-satellite [--version | --self-test | --help]";

/// Compile-time facts about the build. `neon`/`vfp3` are not reported because
/// `cfg!(target_feature)` cannot see those unstable ARM features on stable rustc;
/// the float check below exercises the FPU at runtime instead.
fn build_features() -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "arch={} pointer_width={} endian={} os={} env={}",
        std::env::consts::ARCH,
        usize::BITS,
        if cfg!(target_endian = "little") {
            "little"
        } else {
            "big"
        },
        std::env::consts::OS,
        if cfg!(target_env = "musl") {
            "musl"
        } else {
            "other"
        },
    );
    let _ = write!(out, " static_crt={}", cfg!(target_feature = "crt-static"));
    out
}

/// Returns the lines of `/proc/self/status` that report memory, as `key=value`.
fn memory_lines(status: &str) -> Vec<String> {
    const KEYS: [&str; 6] = ["VmPeak", "VmSize", "VmHWM", "VmRSS", "VmData", "Threads"];
    status
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            KEYS.contains(&key)
                .then(|| format!("{key}={}", value.trim()))
        })
        .collect()
}

/// Small floating-point loop whose operands the optimizer cannot fold, so the
/// FPU (VFP/NEON on the 32-bit ARM build) is exercised at runtime. A missing
/// or disabled FPU would raise `SIGILL` here rather than later in the DSP code.
fn float_check() -> f32 {
    let mut acc = std::hint::black_box(0.5_f32);
    let step = std::hint::black_box(1.25_f32);
    for i in 0..64_u8 {
        acc = acc * 0.5 + step * f32::from(i);
    }
    acc
}

fn fmt_limit(value: Option<u64>) -> String {
    value.map_or_else(|| "unlimited".to_owned(), |v| v.to_string())
}

fn self_test() -> ExitCode {
    println!("webos-satellite {VERSION}");
    println!("build_target={TARGET}");
    println!("build_features: {}", build_features());

    let uname = rustix::system::uname();
    println!(
        "uname: sysname={} release={} machine={}",
        uname.sysname().to_string_lossy(),
        uname.release().to_string_lossy(),
        uname.machine().to_string_lossy(),
    );
    println!("page_size={}", rustix::param::page_size());
    println!("float_check={:.4}", float_check());

    for (name, resource) in [
        ("stack", Resource::Stack),
        ("nofile", Resource::Nofile),
        ("as", Resource::As),
        ("data", Resource::Data),
        ("memlock", Resource::Memlock),
        ("nproc", Resource::Nproc),
        ("core", Resource::Core),
    ] {
        let limit = getrlimit(resource);
        println!(
            "rlimit {name}: soft={} hard={}",
            fmt_limit(limit.current),
            fmt_limit(limit.maximum)
        );
    }

    match std::fs::read_to_string("/proc/self/status") {
        Ok(status) => {
            for line in memory_lines(&status) {
                println!("proc_status {line}");
            }
        }
        Err(err) => {
            eprintln!("self-test: cannot read /proc/self/status: {err}");
            return ExitCode::FAILURE;
        }
    }
    println!("self-test: ok");
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("--version") => {
            println!("webos-satellite {VERSION} ({TARGET})");
            ExitCode::SUCCESS
        }
        Some("--self-test") => self_test(),
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_lines_filters_known_keys() {
        let status = "Name:\tx\nVmSize:\t  100 kB\nVmRSS:\t 42 kB\nOther:\t1\nThreads:\t3\n";
        assert_eq!(
            memory_lines(status),
            ["VmSize=100 kB", "VmRSS=42 kB", "Threads=3"]
        );
    }

    #[test]
    fn limit_formatting() {
        assert_eq!(fmt_limit(None), "unlimited");
        assert_eq!(fmt_limit(Some(7)), "7");
    }

    #[test]
    fn float_check_is_deterministic() {
        assert!((float_check() - float_check()).abs() < f32::EPSILON);
        assert!(float_check().is_finite());
    }

    #[test]
    fn target_is_set() {
        assert_ne!(TARGET, "");
        assert!(build_features().contains("pointer_width="));
    }
}
