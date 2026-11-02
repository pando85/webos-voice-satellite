//! Fake satellite: runs the ESPHome API server with a synthetic microphone
//! (440 Hz sine, 32 ms chunks) and simulated playback, printing every app event
//! as an `EVENT ...` line on stdout. Used by `tools/esphome-fake-ha/`.
//!
//! ```text
//! cargo run -p webos-esphome-api --example fake_device -- \
//!     [--bind 127.0.0.1] [--port 6053] [--name fake-tv] [--mode wake|ptt|none] \
//!     [--features 23] [--mac 02:00:00:00:00:01] [--mdns]
//! ```
//!
//! The Noise key is read from the `WEBOS_ESPHOME_PSK` environment variable
//! (plaintext if unset). Stdin commands: `ptt`, `wake`, `stop`, `mute`,
//! `unmute`, `quit`. Log level: `FAKE_DEVICE_LOG=info|debug|trace`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write as _;
use std::time::Duration;

use tokio::io::AsyncBufReadExt as _;
use tokio::sync::mpsc;
use webos_esphome_api::{AppCommand, AppEvent, Config, PipelineMode, Server};

struct StderrLogger(log::LevelFilter);

impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= self.0
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

fn event_line(ev: &AppEvent) -> String {
    match ev {
        AppEvent::TtsAudio(data) => format!("TtsAudio(len={})", data.len()),
        // `AppEvent`'s `Debug` hides payloads (so logs cannot leak them); this
        // test harness prints them in full for the checks in fake_ha.py.
        AppEvent::Transcript(t) => format!("Transcript({t:?})"),
        AppEvent::IntentEnd {
            speech,
            continue_conversation,
        } => format!(
            "IntentEnd {{ speech: {speech:?}, continue_conversation: {continue_conversation} }}"
        ),
        AppEvent::TtsUrl(u) => format!("TtsUrl({u:?})"),
        AppEvent::Announce {
            url,
            preannounce_url,
            text,
            start_conversation,
        } => format!(
            "Announce {{ url: {url:?}, preannounce_url: {preannounce_url:?}, text: {text:?}, start_conversation: {start_conversation} }}"
        ),
        AppEvent::Error { code, message } => {
            format!("Error {{ code: {code:?}, message: {message:?} }}")
        }
        AppEvent::StartStreaming {
            wake_word,
            conversation_id,
        } => format!(
            "StartStreaming {{ wake_word: {wake_word}, conversation_id: {conversation_id:?} }}"
        ),
        // The harness prints the (otherwise hidden) timer name in full.
        AppEvent::Timer {
            event,
            id,
            name,
            total_seconds,
            seconds_left,
            is_active,
        } => format!(
            "Timer {{ event: {event}, id: {id:?}, name: {name:?}, total_seconds: {total_seconds}, seconds_left: {seconds_left}, is_active: {is_active} }}"
        ),
        other => format!("{other:?}"),
    }
}

fn print_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// 512 samples (32 ms at 16 kHz) of a 440 Hz sine starting at `phase`.
fn sine_chunk(phase: &mut f32) -> Vec<i16> {
    const STEP: f32 = 2.0 * std::f32::consts::PI * 440.0 / 16_000.0;
    (0..512)
        .map(|_| {
            *phase = (*phase + STEP) % (2.0 * std::f32::consts::PI);
            #[allow(clippy::cast_possible_truncation)]
            let s = (phase.sin() * 3000.0) as i16;
            s
        })
        .collect()
}

fn parse_args() -> (Config, Option<PipelineMode>) {
    let mut cfg = Config {
        name: "fake-tv".to_owned(),
        friendly_name: "Fake TV Satellite".to_owned(),
        bind_addr: "127.0.0.1".parse().unwrap(),
        noise_psk: std::env::var("WEBOS_ESPHOME_PSK")
            .ok()
            .filter(|k| !k.is_empty()),
        ..Config::default()
    };
    let mut mode = Some(PipelineMode::WakeWord);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--bind" => cfg.bind_addr = val().parse().expect("--bind: IP address"),
            "--port" => cfg.port = val().parse().expect("--port: number"),
            "--name" => cfg.name = val(),
            "--mac" => cfg.mac = Some(val()),
            "--features" => cfg.voice_features = val().parse().expect("--features: number"),
            "--mdns" => cfg.mdns = true,
            "--mode" => {
                mode = match val().as_str() {
                    "wake" => Some(PipelineMode::WakeWord),
                    "ptt" | "none" => None,
                    other => panic!("unknown mode {other}"),
                };
            }
            other => panic!("unknown argument {other}"),
        }
    }
    (cfg, mode)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let level = match std::env::var("FAKE_DEVICE_LOG").as_deref() {
        Ok("trace") => log::LevelFilter::Trace,
        Ok("debug") => log::LevelFilter::Debug,
        Ok("warn") => log::LevelFilter::Warn,
        _ => log::LevelFilter::Info,
    };
    let _ = log::set_logger(Box::leak(Box::new(StderrLogger(level))));
    log::set_max_level(level);

    let (cfg, mode) = parse_args();
    let server = Server::bind(cfg).await.expect("bind");
    print_line(&format!("LISTENING {}", server.local_addr().unwrap()));

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    if let Some(mode) = mode {
        cmd_tx
            .send(AppCommand::StartPipeline { mode })
            .await
            .unwrap();
    }
    let server_task = tokio::spawn(server.run(ev_tx, cmd_rx));

    // Stdin control.
    let stdin_tx = cmd_tx.clone();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let cmd = match line.trim() {
                "ptt" => AppCommand::StartPipeline {
                    mode: PipelineMode::PushToTalk,
                },
                "wake" => AppCommand::StartPipeline {
                    mode: PipelineMode::WakeWord,
                },
                "stop" => AppCommand::StopPipeline,
                "mute" => AppCommand::SetMuted(true),
                "unmute" => AppCommand::SetMuted(false),
                "quit" => std::process::exit(0),
                "" => continue,
                other => {
                    eprintln!("unknown command {other}");
                    continue;
                }
            };
            if stdin_tx.send(cmd).await.is_err() {
                break;
            }
        }
    });

    let mut streaming = false;
    let mut phase = 0.0_f32;
    let mut mic = tokio::time::interval(Duration::from_millis(32));
    loop {
        tokio::select! {
            ev = ev_rx.recv() => {
                let Some(ev) = ev else { break };
                print_line(&format!("EVENT {}", event_line(&ev)));
                match ev {
                    AppEvent::StartStreaming { .. } => streaming = true,
                    AppEvent::StopStreaming | AppEvent::HaDisconnected => streaming = false,
                    AppEvent::TtsUrl(_) | AppEvent::TtsStreamEnd => {
                        let tx = cmd_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            let _ = tx.send(AppCommand::PlaybackFinished { success: true }).await;
                        });
                    }
                    AppEvent::Announce { .. } => {
                        let tx = cmd_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            let _ = tx.send(AppCommand::AnnounceFinished { success: true }).await;
                        });
                    }
                    _ => {}
                }
            }
            _ = mic.tick() => {
                if streaming {
                    // Drop audio rather than block if the server is backed up.
                    let _ = cmd_tx.try_send(AppCommand::Audio(sine_chunk(&mut phase)));
                }
            }
        }
    }
    drop(cmd_tx);
    let _ = server_task.await;
}
