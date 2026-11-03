//! `webos-satellite`: Home Assistant voice satellite for rooted LG webOS TVs.
//!
//! The daemon serves the ESPHome native API (TCP 6053) so Home Assistant adds
//! the TV as an Assist satellite. It streams the TV mic (ALSA, or a raw file
//! for tests) through the front-end to HA while HA asks for audio, and plays
//! the TTS reply through `pacat`, a WAV file or nowhere. Wake word, STT,
//! conversation and TTS run in HA (ADR 0001, ADR 0003).
//!
//! `--self-test` keeps the Phase 1.1 read-only runtime report.

#![cfg_attr(
    test,
    allow(clippy::assert_is_empty, clippy::field_reassign_with_default)
)]

mod capture;
mod config;
mod daemon;
mod darken;
mod firmware;
mod limits;
mod logger;
mod player;
mod power;
mod resume;
mod selftest;
mod volume;
mod watchdog;

use std::process::ExitCode;

/// See the musl note in this crate's Cargo.toml.
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use config::{Command, USAGE};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const TARGET: &str = env!("WEBOS_SATELLITE_TARGET");

fn main() -> ExitCode {
    let cmd = match config::parse_args(std::env::args().skip(1)) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("webos-satellite: {e}\n(see webos-satellite --help)");
            return ExitCode::from(2);
        }
    };
    match cmd {
        Command::Version => {
            println!("webos-satellite {VERSION} ({TARGET})");
            ExitCode::SUCCESS
        }
        Command::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::SelfTest => selftest::self_test(),
        Command::CheckConfig(s) => {
            for w in s.warnings() {
                eprintln!("warning: {w}");
            }
            println!("config ok: {}", s.summary());
            ExitCode::SUCCESS
        }
        Command::CheckFirmware(s) => {
            let v = firmware::check(&s);
            println!("{v}");
            ExitCode::from(v.exit_code())
        }
        Command::Run(s, triggers) => {
            // validate() already checked the level.
            logger::init(s.log_level().unwrap_or(log::LevelFilter::Info));
            // Before any thread exists: niceness is per thread on Linux.
            let limits = limits::apply(&s.limits);
            for w in s.warnings() {
                log::warn!("config: {w}");
            }
            log::info!("{limits}");
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("cannot start the runtime: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match rt.block_on(daemon::run(*s, triggers)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    log::error!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
