//! Fake satellite: runs the ESPHome API server with a synthetic microphone
//! (440 Hz sine, 32 ms chunks) and simulated playback, printing every app event
//! as an `EVENT ...` line on stdout. Used by `tools/esphome-fake-ha/`.
//!
//! ```text
//! cargo run -p webos-esphome-api --example fake_device -- \
//!     [--bind 127.0.0.1] [--port 6053] [--name fake-tv] [--mode wake|ptt|none] \
//!     [--features 23] [--mac 02:00:00:00:00:01] [--mdns] [--demo-entities] [--no-capture] \
//!     [--pair-key-file FILE [--pair-window SECS] [--pair-closed]] \
//!     [--music [--music-format flac|wav] [--music-no-announce] [--music-allow-plaintext]]
//! ```
//!
//! The Noise key is read from the `WEBOS_ESPHOME_PSK` environment variable
//! (plaintext if unset). `--pair-key-file` enables pairing instead: the device
//! starts unpaired (or with the key stored in FILE) and HA can set a key while
//! the window is open (`--pair-closed`: closed at start). Stdin commands:
//! `ptt`, `wake`, `stop`, `mute`, `unmute`, `capture` / `nocapture` (the
//! microphone delivers audio or not, as a busy PCM does; `--no-capture`: not
//! at start), `pair` (open the window),
//! `reset-pairing`, `number VALUE|unknown`, `binary on|off|unknown`,
//! `switch on|off|unknown`, `text VALUE|unknown`, `select OPTION|unknown`,
//! `options A,B,C` and `mp on|off|playing VOLUME|unknown muted|unmuted`
//! (states and options of the `--demo-entities`), `music idle|playing|paused`
//! (the music state of the built-in player, with `--music`), `quit`. Log level: `FAKE_DEVICE_LOG=info|debug|trace`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write as _;
use std::time::Duration;

use tokio::io::AsyncBufReadExt as _;
use tokio::sync::mpsc;
use webos_esphome_api::entities::{
    FEATURE_TURN_OFF, FEATURE_TURN_ON, FEATURE_VOLUME_MUTE, FEATURE_VOLUME_SET, FEATURE_VOLUME_STEP,
};
use webos_esphome_api::proto::{EntityCategory, NumberMode};
use webos_esphome_api::{
    AppCommand, AppEvent, BinarySensorEntity, ButtonEntity, Config, MediaPlayerEntity,
    MusicCommand, MusicConfig, MusicState, NumberEntity, PipelineMode, PlayerState, PlayerStatus,
    Provisioning, SelectEntity, Server, SwitchEntity, TextSensorEntity,
};

/// `mp on|off|playing VOLUME|unknown muted|unmuted`.
fn parse_mp(args: &str) -> Option<PlayerStatus> {
    let mut it = args.split_whitespace();
    let state = match it.next()? {
        "on" => PlayerState::On,
        "off" => PlayerState::Off,
        "playing" => PlayerState::Playing,
        _ => return None,
    };
    let volume = match it.next()? {
        "unknown" => None,
        v => Some(v.parse().ok()?),
    };
    let muted = it.next()? == "muted";
    Some(PlayerStatus {
        state,
        volume,
        muted,
    })
}

/// A number, two buttons and two binary sensors, to exercise the generic
/// entity types from the fake-HA harness (the ids mirror TV control).
fn add_demo_entities(cfg: &mut Config) {
    cfg.numbers.push(NumberEntity {
        object_id: "demo_volume".to_owned(),
        name: "Demo volume".to_owned(),
        icon: "mdi:volume-high".to_owned(),
        min_value: 0.0,
        max_value: 100.0,
        step: 1.0,
        unit: "%".to_owned(),
        device_class: "volume".to_owned(),
        mode: NumberMode::Slider,
        entity_category: EntityCategory::None,
    });
    cfg.buttons.push(ButtonEntity {
        object_id: "demo_press".to_owned(),
        name: "Demo button".to_owned(),
        icon: "mdi:gesture-tap".to_owned(),
        device_class: String::new(),
        entity_category: EntityCategory::None,
    });
    cfg.buttons.push(ButtonEntity {
        object_id: "demo_config_press".to_owned(),
        name: "Demo config button".to_owned(),
        icon: String::new(),
        device_class: "restart".to_owned(),
        entity_category: EntityCategory::Config,
    });
    cfg.switches.push(SwitchEntity {
        object_id: "demo_switch".to_owned(),
        name: "Demo switch".to_owned(),
        icon: "mdi:television".to_owned(),
        device_class: "switch".to_owned(),
        entity_category: EntityCategory::None,
    });
    cfg.text_sensors.push(TextSensorEntity {
        object_id: "demo_text".to_owned(),
        name: "Demo text".to_owned(),
        icon: String::new(),
        device_class: String::new(),
        entity_category: EntityCategory::Diagnostic,
    });
    cfg.selects.push(SelectEntity {
        object_id: "demo_select".to_owned(),
        name: "Demo select".to_owned(),
        icon: "mdi:apps".to_owned(),
        options: vec!["App A".to_owned(), "App B".to_owned(), "Other".to_owned()],
        entity_category: EntityCategory::None,
        command_echo: false,
    });
    cfg.binary_sensors.push(BinarySensorEntity {
        object_id: "demo_state".to_owned(),
        name: "Demo state".to_owned(),
        icon: String::new(),
        device_class: "running".to_owned(),
        entity_category: EntityCategory::Diagnostic,
    });
    cfg.media_players.push(MediaPlayerEntity {
        object_id: "demo_tv".to_owned(),
        name: "TV".to_owned(),
        icon: "mdi:television".to_owned(),
        feature_flags: FEATURE_VOLUME_SET
            | FEATURE_VOLUME_STEP
            | FEATURE_VOLUME_MUTE
            | FEATURE_TURN_ON
            | FEATURE_TURN_OFF,
        entity_category: EntityCategory::None,
    });
}

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
        // The harness prints the (otherwise hidden) music URL in full.
        AppEvent::Music(MusicCommand::Play { url, announce }) => {
            format!("MusicPlay {{ url: {url:?}, announce: {announce} }}")
        }
        AppEvent::Music(MusicCommand::EnqueueRefused { url }) => {
            format!("MusicEnqueueRefused {{ url: {url:?} }}")
        }
        AppEvent::Music(other) => format!("Music{other:?}"),
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

fn parse_args() -> (Config, Option<PipelineMode>, bool) {
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
    let mut capture = true;
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
            "--demo-entities" => add_demo_entities(&mut cfg),
            "--no-capture" => capture = false,
            "--music" => {
                cfg.music.get_or_insert_with(MusicConfig::default);
            }
            "--music-format" => {
                let f = val();
                cfg.music
                    .get_or_insert_with(MusicConfig::default)
                    .format
                    .format = f;
            }
            "--music-no-announce" => {
                cfg.music.get_or_insert_with(MusicConfig::default).announce = false;
            }
            "--music-allow-plaintext" => {
                cfg.music
                    .get_or_insert_with(MusicConfig::default)
                    .allow_plaintext = true;
            }
            "--pair-key-file" => {
                cfg.provisioning
                    .get_or_insert_with(Provisioning::default)
                    .key_file = val().into();
            }
            "--pair-window" => {
                let secs: f64 = val().parse().expect("--pair-window: seconds");
                cfg.provisioning
                    .get_or_insert_with(Provisioning::default)
                    .window = Duration::from_secs_f64(secs);
            }
            "--pair-closed" => {
                cfg.provisioning
                    .get_or_insert_with(Provisioning::default)
                    .open_at_start = false;
            }
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
    (cfg, mode, capture)
}

#[tokio::main(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // one flat command table
async fn main() {
    let level = match std::env::var("FAKE_DEVICE_LOG").as_deref() {
        Ok("trace") => log::LevelFilter::Trace,
        Ok("debug") => log::LevelFilter::Debug,
        Ok("warn") => log::LevelFilter::Warn,
        _ => log::LevelFilter::Info,
    };
    let _ = log::set_logger(Box::leak(Box::new(StderrLogger(level))));
    log::set_max_level(level);

    let (mut cfg, mode, capture) = parse_args();
    if cfg.provisioning.is_some() {
        cfg.noise_psk = None;
    }
    let server = Server::bind(cfg).await.expect("bind");
    print_line(&format!("LISTENING {}", server.local_addr().unwrap()));

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    if !capture {
        cmd_tx.send(AppCommand::CaptureLive(false)).await.unwrap();
    }
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
                "capture" => AppCommand::CaptureLive(true),
                "nocapture" => AppCommand::CaptureLive(false),
                "pair" => AppCommand::OpenPairingWindow,
                "reset-pairing" => AppCommand::ResetPairing,
                "quit" => std::process::exit(0),
                "" => continue,
                other if other.starts_with("number ") => AppCommand::SetNumber {
                    object_id: "demo_volume".to_owned(),
                    value: other["number ".len()..].trim().parse().ok(),
                },
                other if other.starts_with("switch ") => AppCommand::SetSwitch {
                    object_id: "demo_switch".to_owned(),
                    state: match other["switch ".len()..].trim() {
                        "on" => Some(true),
                        "off" => Some(false),
                        _ => None,
                    },
                },
                other if other.starts_with("text ") => AppCommand::SetTextSensor {
                    object_id: "demo_text".to_owned(),
                    state: match other["text ".len()..].trim() {
                        "unknown" => None,
                        v => Some(v.to_owned()),
                    },
                },
                other if other.starts_with("select ") => match other["select ".len()..].trim() {
                    "unknown" => AppCommand::ClearSelect {
                        object_id: "demo_select".to_owned(),
                    },
                    v => AppCommand::SetSelect {
                        object_id: "demo_select".to_owned(),
                        option: v.to_owned(),
                    },
                },
                other if other.starts_with("options ") => AppCommand::SetSelectOptions {
                    object_id: "demo_select".to_owned(),
                    options: other["options ".len()..]
                        .split(',')
                        .map(|o| o.trim().to_owned())
                        .collect(),
                },
                other if other.starts_with("music ") => {
                    AppCommand::SetMusicState(match other["music ".len()..].trim() {
                        "idle" => MusicState::Idle,
                        "playing" => MusicState::Playing,
                        "paused" => MusicState::Paused,
                        _ => {
                            eprintln!("bad music command {other}");
                            continue;
                        }
                    })
                }
                other if other.starts_with("mp ") => {
                    let Some(status) = parse_mp(&other["mp ".len()..]) else {
                        eprintln!("bad mp command {other}");
                        continue;
                    };
                    AppCommand::SetMediaPlayer {
                        object_id: "demo_tv".to_owned(),
                        status,
                    }
                }
                other if other.starts_with("binary ") => AppCommand::SetBinarySensor {
                    object_id: "demo_state".to_owned(),
                    state: match other["binary ".len()..].trim() {
                        "on" => Some(true),
                        "off" => Some(false),
                        _ => None,
                    },
                },
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
