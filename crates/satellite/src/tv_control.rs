//! TV control in the daemon: runs the luna calls of the sans-IO controller
//! (`webos-tv-control`, [design](../../../docs/design/tv-control.md)), keeps
//! the supervised subscriptions, and maps the controller's entities and
//! states onto the ESPHome API crate. Capture, voice and playback know
//! nothing about it.
//!
//! Calls run as `luna-send -n 1 -w 3000` on a pseudo-terminal
//! ([`power::call_pty`]): over a plain pipe the audio methods print nothing
//! on the TV (EXP-010 Part A, c3-b). Nothing here is called with a TV
//! identifier: `apiadapter/system/getSystemInfo` (which returns the serial
//! number) is never used, and log lines carry control names and results only,
//! never titles, ids, labels or reply bodies (those stay out of the log at
//! `info` and `warn`; `debug` is the controller's own business).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use webos_esphome_api::entities::{
    FEATURE_TURN_OFF, FEATURE_TURN_ON, FEATURE_VOLUME_MUTE, FEATURE_VOLUME_SET, FEATURE_VOLUME_STEP,
};
use webos_esphome_api::proto::{EntityCategory, NumberMode};
use webos_esphome_api::{
    AppCommand, AppEvent, ButtonEntity, MediaPlayerEntity, NumberEntity, PlayerCommand,
    PlayerState, PlayerStatus, SelectEntity, SwitchEntity, TextSensorEntity,
};
use webos_tv_control::luna::MAX_REPLY_BYTES;
use webos_tv_control::setters::{self, Setter};
use webos_tv_control::{
    Category, Command, Config, Controller, Effect, EntityDef, Kind, Level, MediaAction,
    MediaFeatures, MediaPower, PowerOffEnv, Purpose, StateUpdate, StateValue, Topic,
};

use crate::config::Settings;
use crate::power::{self, Watch};

/// The satellite's own app id (the `make ipk` default `APP_ID`): left out of
/// the app list.
pub(crate) const OWN_APP_ID: &str = "io.github.pando85.voicesatellite";
/// Where the TV declares its luna methods (read-only file reads).
pub(crate) const DECLARATION_DIR: &str = "/usr/share/luna-service2/api-permissions.d";
/// Largest declaration file read.
const DECLARATION_MAX_BYTES: u64 = 1 << 20;
/// Total time of the startup capability check; an unanswered getter counts
/// as failed (design section 6).
pub(crate) const PROBE_BUDGET: Duration = Duration::from_secs(10);
/// How often the controller's clock is driven.
pub(crate) const TICK: Duration = Duration::from_millis(250);
/// Trigger file (in the report directory) that `ctl.sh set` creates after a
/// change of Always Ready or LG hands-free: the daemon reads both sensors'
/// getters now instead of at the next 60 s poll (design 7).
pub(crate) const LG_REFRESH_TRIGGER: &str = "lg-settings-refresh";

/// A message from a running luna call or subscription.
#[derive(Debug)]
pub(crate) enum TvMsg {
    /// The result of a call.
    Reply(Purpose, Result<String, String>),
    /// A line of a subscription.
    Line(Topic, String),
}

/// What running effects produced for the rest of the daemon.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// States and options for the ESPHome server.
    pub(crate) commands: Vec<AppCommand>,
    /// The satellite woke the TV itself: tell the darkener.
    pub(crate) own_wake: bool,
}

/// The entities for the ESPHome server.
#[derive(Debug, Default)]
pub(crate) struct Entities {
    pub(crate) switches: Vec<SwitchEntity>,
    pub(crate) buttons: Vec<ButtonEntity>,
    pub(crate) numbers: Vec<NumberEntity>,
    pub(crate) selects: Vec<SelectEntity>,
    pub(crate) text_sensors: Vec<TextSensorEntity>,
    pub(crate) media_players: Vec<MediaPlayerEntity>,
}

impl Entities {
    /// Adds them to the server configuration.
    pub(crate) fn add_to(self, cfg: &mut webos_esphome_api::Config) {
        cfg.switches.extend(self.switches);
        cfg.buttons.extend(self.buttons);
        cfg.numbers.extend(self.numbers);
        cfg.selects.extend(self.selects);
        cfg.text_sensors.extend(self.text_sensors);
        cfg.media_players.extend(self.media_players);
    }
}

/// HA's media player feature bits for the TV media player: controls only,
/// never play media or announce (ADR 0007).
pub(crate) fn media_feature_flags(f: MediaFeatures) -> u32 {
    [
        (f.volume_set, FEATURE_VOLUME_SET),
        (f.volume_step, FEATURE_VOLUME_STEP),
        (f.mute, FEATURE_VOLUME_MUTE),
        (f.turn_on, FEATURE_TURN_ON),
        (f.turn_off, FEATURE_TURN_OFF),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |acc, (_, bit)| acc | bit)
}

fn category(c: Category) -> EntityCategory {
    match c {
        Category::None => EntityCategory::None,
        Category::Config => EntityCategory::Config,
        Category::Diagnostic => EntityCategory::Diagnostic,
    }
}

/// Maps the controller's entity table onto the ESPHome entity types.
pub(crate) fn map_entities(defs: &[EntityDef]) -> Entities {
    let mut out = Entities::default();
    for d in defs {
        let (object_id, name, icon) = (d.id.to_owned(), d.name.to_owned(), d.icon.to_owned());
        match &d.kind {
            Kind::Switch => out.switches.push(SwitchEntity {
                object_id,
                name,
                icon,
                device_class: String::new(),
                entity_category: category(d.category),
            }),
            Kind::Button => out.buttons.push(ButtonEntity {
                object_id,
                name,
                icon,
                device_class: String::new(),
                entity_category: category(d.category),
            }),
            Kind::Number { max } => out.numbers.push(NumberEntity {
                object_id,
                name,
                icon,
                min_value: 0.0,
                max_value: *max,
                step: 1.0,
                // LG volume steps, not percent: no unit and no device class.
                unit: String::new(),
                device_class: String::new(),
                mode: NumberMode::Slider,
                entity_category: category(d.category),
            }),
            Kind::Select { options } => out.selects.push(SelectEntity {
                object_id,
                name,
                icon,
                options: options.clone(),
                entity_category: category(d.category),
                // The TV owns the state: a choice is a command, not a state.
                command_echo: false,
            }),
            Kind::TextSensor => out.text_sensors.push(TextSensorEntity {
                object_id,
                name,
                icon,
                device_class: String::new(),
                entity_category: category(d.category),
            }),
            Kind::MediaPlayer { features } => out.media_players.push(MediaPlayerEntity {
                object_id,
                name,
                icon,
                feature_flags: media_feature_flags(*features),
                entity_category: category(d.category),
            }),
        }
    }
    out
}

/// The server command for a state.
pub(crate) fn state_command(u: StateUpdate) -> AppCommand {
    let object_id = u.id.to_owned();
    match u.value {
        StateValue::Switch(state) => AppCommand::SetSwitch { object_id, state },
        StateValue::Number(value) => AppCommand::SetNumber { object_id, value },
        StateValue::Select(Some(option)) => AppCommand::SetSelect { object_id, option },
        StateValue::Select(None) => AppCommand::ClearSelect { object_id },
        StateValue::Text(state) => AppCommand::SetTextSensor { object_id, state },
        StateValue::Media(m) => AppCommand::SetMediaPlayer {
            object_id,
            status: PlayerStatus {
                state: match m.power {
                    MediaPower::On => PlayerState::On,
                    MediaPower::Playing => PlayerState::Playing,
                    MediaPower::Off => PlayerState::Off,
                },
                volume: m.volume,
                muted: m.muted,
            },
        },
    }
}

/// An HA event that is a TV control command, if it is one. Object ids of TV
/// control all start with `tv_`; the remote power-off select and the
/// microphone switch are not TV control.
pub(crate) fn command_for(ev: &AppEvent) -> Option<Command> {
    match ev {
        AppEvent::SwitchChanged { object_id, state } if object_id.starts_with("tv_") => {
            Some(Command::Switch {
                id: object_id.clone(),
                state: *state,
            })
        }
        AppEvent::NumberChanged { object_id, value } if object_id.starts_with("tv_") => {
            Some(Command::Number {
                id: object_id.clone(),
                value: *value,
            })
        }
        AppEvent::SelectChanged { object_id, option } if object_id.starts_with("tv_") => {
            Some(Command::Select {
                id: object_id.clone(),
                option: option.clone(),
            })
        }
        AppEvent::ButtonPressed { object_id } if object_id.starts_with("tv_") => {
            Some(Command::Button {
                id: object_id.clone(),
            })
        }
        AppEvent::MediaPlayerCommand { object_id, command } if object_id.starts_with("tv_") => {
            let action = match *command {
                PlayerCommand::SetVolume(v) => MediaAction::SetVolume(v),
                PlayerCommand::VolumeUp => MediaAction::VolumeUp,
                PlayerCommand::VolumeDown => MediaAction::VolumeDown,
                PlayerCommand::Mute(m) => MediaAction::Mute(m),
                PlayerCommand::TurnOn => MediaAction::TurnOn,
                PlayerCommand::TurnOff => MediaAction::TurnOff,
                // Not advertised by the TV media player (no playback control).
                PlayerCommand::Play | PlayerCommand::Pause | PlayerCommand::Stop => return None,
            };
            Some(Command::Media {
                id: object_id.clone(),
                action,
            })
        }
        _ => None,
    }
}

/// The controller's configuration from `[tv_control]`. `blocked` is the
/// reason the setters are off (the firmware guard), if any.
pub(crate) fn controller_config(settings: &Settings, blocked: Option<String>) -> Config {
    let t = &settings.tv_control;
    Config {
        screen: t.screen,
        power_on: t.power_on,
        power_off: t.power_off,
        volume: t.volume,
        volume_limit: t.volume_limit,
        mute: t.mute,
        input: t.input,
        apps: t.apps,
        app_limit: t.app_limit as usize,
        play_state: t.play_state,
        media_player: t.media_player,
        lg_settings: t.lg_settings,
        setters_blocked: blocked,
        own_app_ids: vec![OWN_APP_ID.to_owned()],
    }
}

/// The declaration files of the audio, apiadapter and application manager
/// (SAM) services in `dir`: `(file name, text)`. Unreadable files are skipped; a
/// missing directory gives none (every setter is then `Unchecked`).
pub(crate) fn read_declarations(dir: &Path) -> Vec<(String, String)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !setters::is_declaration_file(&name) {
                return None;
            }
            let meta = e.metadata().ok()?;
            if !meta.is_file() || meta.len() > DECLARATION_MAX_BYTES {
                return None;
            }
            Some((name, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    out.sort();
    out
}

fn log_effect(level: Level, msg: &str) {
    match level {
        Level::Debug => log::debug!("{msg}"),
        Level::Info => log::info!("{msg}"),
        Level::Warn => log::warn!("{msg}"),
    }
}

/// Result of [`start`].
pub(crate) struct Started {
    pub(crate) tv: Tv,
    pub(crate) entities: Entities,
    /// Effects of the startup check (log lines, the first states, the
    /// subscriptions): run them with [`Tv::exec`] once the server runs.
    pub(crate) effects: Vec<Effect>,
    /// Replies and subscription lines.
    pub(crate) rx: mpsc::UnboundedReceiver<TvMsg>,
}

/// The startup capability check (design section 6): config, one read-only
/// call per wanted getter with a total budget of [`PROBE_BUDGET`], and the
/// declaration files. Never a write. Entities are listed whatever the
/// outcome; a failed control shows no state.
pub(crate) async fn start(
    settings: &Settings,
    luna: PathBuf,
    blocked: Option<String>,
    declarations: &Path,
) -> Started {
    let mut ctl = Controller::new(controller_config(settings, blocked));
    let files = read_declarations(declarations);
    for s in Setter::ALL {
        ctl.set_declared(s, setters::declared_in(&files, s));
    }
    let mut jobs: JoinSet<(Purpose, Result<String, String>)> = JoinSet::new();
    let spawn_read =
        |jobs: &mut JoinSet<_>, purpose: Purpose, uri: &'static str, payload: String| {
            let luna = luna.clone();
            jobs.spawn(async move {
                let r = power::call_pty(&luna, uri, &payload, MAX_REPLY_BYTES + 1).await;
                (purpose, r)
            });
        };
    let mut effects = Vec::new();
    for f in ctl.start() {
        if let Effect::Call {
            purpose,
            uri,
            payload,
        } = f
        {
            spawn_read(&mut jobs, purpose, uri, payload);
        }
    }
    let deadline = tokio::time::Instant::now() + PROBE_BUDGET;
    while !ctl.probe_complete() {
        match tokio::time::timeout_at(deadline, jobs.join_next()).await {
            Ok(Some(Ok((purpose, result)))) => {
                for f in ctl.on_reply(purpose, result, Instant::now()) {
                    match f {
                        Effect::Call {
                            purpose,
                            uri,
                            payload,
                        } => spawn_read(&mut jobs, purpose, uri, payload),
                        other => effects.push(other),
                    }
                }
            }
            // Out of time, or a call task died: the rest counts as failed.
            _ => break,
        }
    }
    jobs.abort_all();
    effects.extend(ctl.finish_probe(Instant::now()));
    let entities = map_entities(&ctl.entities());
    let (tx, rx) = mpsc::unbounded_channel();
    Started {
        tv: Tv {
            ctl,
            luna,
            tx,
            subs: HashMap::new(),
        },
        entities,
        effects,
        rx,
    }
}

/// The running TV control.
pub(crate) struct Tv {
    ctl: Controller,
    luna: PathBuf,
    tx: mpsc::UnboundedSender<TvMsg>,
    subs: HashMap<Topic, Watch>,
}

impl Tv {
    /// Runs effects: spawns the luna calls and subscriptions, logs, and
    /// returns what the server and the darkener need.
    pub(crate) fn exec(&mut self, fx: Vec<Effect>) -> Outcome {
        let mut out = Outcome::default();
        for f in fx {
            match f {
                Effect::Call {
                    purpose,
                    uri,
                    payload,
                } => self.spawn_call(purpose, uri, payload),
                Effect::Subscribe(topic) => self.subscribe(topic),
                Effect::Unsubscribe(topic) => {
                    if let Some(w) = self.subs.remove(&topic) {
                        tokio::spawn(w.shutdown());
                    }
                }
                Effect::State(u) => out.commands.push(state_command(u)),
                Effect::OwnWake => out.own_wake = true,
                Effect::Log(level, msg) => log_effect(level, &msg),
            }
        }
        out
    }

    fn spawn_call(&self, purpose: Purpose, uri: &'static str, payload: String) {
        let luna = self.luna.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = power::call_pty(&luna, uri, &payload, MAX_REPLY_BYTES + 1).await;
            let _ = tx.send(TvMsg::Reply(purpose, r));
        });
    }

    fn subscribe(&mut self, topic: Topic) {
        let Some(uri) = topic.subscription_uri() else {
            return;
        };
        if self.subs.contains_key(&topic) {
            return;
        }
        let tx = self.tx.clone();
        let what = match topic {
            Topic::Volume => "TV volume",
            _ => "foreground app",
        };
        let w = power::spawn_lines(self.luna.clone(), uri, "tv_control", what, move |line| {
            tx.send(TvMsg::Line(topic, line)).is_ok()
        });
        self.subs.insert(topic, w);
    }

    /// A reply or subscription line arrived.
    pub(crate) fn on_msg(&mut self, msg: TvMsg) -> Outcome {
        let now = Instant::now();
        let fx = match msg {
            TvMsg::Reply(purpose, result) => self.ctl.on_reply(purpose, result, now),
            TvMsg::Line(topic, line) => self.ctl.on_subscription_line(topic, &line, now),
        };
        self.exec(fx)
    }

    /// An HA event: a TV control command is run, anything else is not ours.
    pub(crate) fn on_event(&mut self, ev: &AppEvent) -> Option<Outcome> {
        let cmd = command_for(ev)?;
        let fx = self.ctl.on_command(cmd, Instant::now());
        Some(self.exec(fx))
    }

    /// A power-state update (the state without spaces, and whether a
    /// transition is in progress).
    pub(crate) fn on_power(&mut self, state_key: &str, processing: bool) -> Outcome {
        let fx = self.ctl.on_power(state_key, processing, Instant::now());
        self.exec(fx)
    }

    /// `ctl.sh set` changed an LG setting (the refresh trigger file was
    /// consumed): read the two sensors' getters now.
    pub(crate) fn refresh_lg_settings(&mut self) -> Outcome {
        let fx = self.ctl.refresh_lg_settings(Instant::now());
        self.exec(fx)
    }

    /// The system resumed from a suspend.
    pub(crate) fn on_resume(&mut self) -> Outcome {
        let fx = self.ctl.on_resume(Instant::now());
        self.exec(fx)
    }

    /// Drives the clock (about every [`TICK`]) with the daemon's view of the
    /// power-off preconditions.
    pub(crate) fn tick(&mut self, env: PowerOffEnv) -> Outcome {
        let mut fx = self.ctl.set_env(env);
        fx.extend(self.ctl.tick(Instant::now()));
        self.exec(fx)
    }

    /// Select options that changed (an app was installed) and wait to be
    /// applied: the daemon applies them when no conversation is running.
    pub(crate) fn pending_options(&self) -> Vec<(&'static str, Vec<String>)> {
        self.ctl.pending_options()
    }

    /// The daemon applied the options of `id` (it sent them to the server).
    pub(crate) fn options_applied(&mut self, id: &'static str) -> Outcome {
        let fx = self.ctl.options_applied(id);
        self.exec(fx)
    }

    /// Per control `ok`, `off` or `failed`, for the compatibility report
    /// (check C10).
    pub(crate) fn control_results(
        &self,
    ) -> Vec<(
        webos_compat::report::ControlKind,
        webos_compat::report::ControlResult,
    )> {
        use webos_compat::report::{ControlKind as K, ControlResult as R};
        use webos_tv_control::{Control, ControlStatus};
        [
            (K::Screen, Control::Screen),
            (K::Mute, Control::Mute),
            (K::Volume, Control::Volume),
            (K::Input, Control::Input),
            (K::App, Control::Apps),
        ]
        .into_iter()
        .map(|(k, c)| {
            (
                k,
                match self.ctl.control_status(c) {
                    ControlStatus::Ok => R::Ok,
                    ControlStatus::Off => R::Off,
                    ControlStatus::Failed => R::Failed,
                },
            )
        })
        .collect()
    }

    /// Stops the subscription children.
    pub(crate) async fn shutdown(self) {
        for (_, w) in self.subs {
            w.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use webos_tv_control::entities as e;
    use webos_tv_control::setters::Declared;

    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webos-sat-tv-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake `luna-send`: answers by URI from synthetic JSON, appends every
    /// call to `calls`, and prints whether stdout is a terminal.
    fn fake_luna(dir: &Path, volume_fails: bool) -> PathBuf {
        let calls = dir.join("calls");
        let volume = if volume_fails {
            r#"echo '{"returnValue":false,"errorCode":-1,"errorText":"Denied"}'"#
        } else {
            r#"echo '{"returnValue":true,"volumeStatus":{"volume":12,"maxVolume":60,"muteStatus":false,"soundOutput":"external_arc"}}'"#
        };
        let script = format!(
            r#"[ -t 1 ] || echo "NOT-A-TTY" >> '{calls}'
if [ "$1" = "-i" ]; then
  echo "SUB $2 $3" >> '{calls}'
  case "$2" in
    */getVolume) echo '{{"returnValue":true,"subscribed":true,"volumeStatus":{{"volume":12,"maxVolume":60,"muteStatus":false,"soundOutput":"external_arc"}}}}' ;;
    *) echo '{{"returnValue":true,"subscribed":true,"appId":"app.a"}}' ;;
  esac
  exec sleep 30
fi
uri="$5"; payload="$6"
echo "$uri $payload" >> '{calls}'
case "$uri" in
  */getPowerState) echo '{{"returnValue":true,"state":"Active"}}' ;;
  */getVolume) {volume} ;;
  */getForegroundAppInfo) echo '{{"returnValue":true,"appId":"app.a"}}' ;;
  */getExternalInputList) echo '{{"returnValue":true,"devices":[{{"id":"HDMI_1","label":"HDMI 1","connected":true,"appId":"app.hdmi1"}}]}}' ;;
  */listLaunchPoints) echo '{{"returnValue":true,"launchPoints":[{{"id":"app.a","title":"App A"}},{{"id":"app.b","title":"App B"}}]}}' ;;
  */setVolume) echo '{{"returnValue":true,"volume":20,"soundOutput":""}}' ;;
  */getSystemSettings) cat '{lg}/lg_always_ready' 2>/dev/null || echo '{{"returnValue":true,"category":"general","settings":{{"lifeOnScreenMode":"off"}}}}' ;;
  */getActivation) cat '{lg}/lg_hands_free' 2>/dev/null || echo '{{"returnValue":true,"settingStatus":false,"activationState":false,"thinQ":{{"settingStatus":false}},"amazon":{{"settingStatus":false}}}}' ;;
  *) echo '{{"returnValue":true}}' ;;
esac
"#,
            calls = calls.display(),
            lg = dir.display()
        );
        let p = dir.join("luna-send");
        crate::testutil::write_script(&p, &script);
        p
    }

    fn calls(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default()
    }

    fn all_on() -> Settings {
        let mut s = Settings::default();
        let t = &mut s.tv_control;
        t.power_off = true;
        t.volume = true;
        t.mute = true;
        t.input = true;
        t.apps = true;
        t.play_state = true;
        s
    }

    #[test]
    fn entity_mapping_keeps_ids_categories_and_the_owner_of_the_state() {
        let ctl = Controller::new(Config {
            volume: true,
            mute: true,
            input: true,
            apps: true,
            power_off: true,
            play_state: true,
            ..Config::default()
        });
        let ents = map_entities(&ctl.entities());
        let ids = |v: &[SwitchEntity]| v.iter().map(|s| s.object_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&ents.switches), ["tv_screen", "tv_power", "tv_mute"]);
        assert_eq!(
            ents.buttons
                .iter()
                .map(|b| b.object_id.as_str())
                .collect::<Vec<_>>(),
            [
                "tv_turn_on",
                "tv_volume_up",
                "tv_volume_down",
                "tv_refresh_lists"
            ]
        );
        let refresh = ents
            .buttons
            .iter()
            .find(|b| b.object_id == "tv_refresh_lists")
            .unwrap();
        assert_eq!(refresh.entity_category, EntityCategory::Config);
        let n = &ents.numbers[0];
        assert_eq!(
            (n.object_id.as_str(), n.min_value, n.max_value, n.step),
            ("tv_volume", 0.0, 100.0, 1.0)
        );
        assert!(n.unit.is_empty(), "LG steps, not percent");
        assert_eq!(n.mode, NumberMode::Slider);
        assert_eq!(ents.selects.len(), 2);
        assert!(
            ents.selects
                .iter()
                .all(|s| !s.command_echo && s.entity_category == EntityCategory::None)
        );
        assert_eq!(ents.selects[0].options, ["Other"]);
        assert!(
            ents.text_sensors
                .iter()
                .all(|t| t.entity_category == EntityCategory::Diagnostic)
        );
        // The TV media player: controls only (volume set and step, mute,
        // turn on and off), never play media or announce.
        assert_eq!(ents.media_players.len(), 1);
        let mp = &ents.media_players[0];
        assert_eq!(
            (mp.object_id.as_str(), mp.name.as_str(), mp.entity_category),
            ("tv_media_player", "TV", EntityCategory::None)
        );
        assert_eq!(mp.feature_flags, 4 | 1024 | 8 | 128 | 256);
        assert_eq!(
            media_feature_flags(MediaFeatures {
                mute: true,
                ..MediaFeatures::default()
            }),
            FEATURE_VOLUME_MUTE
        );
        // The server accepts the whole set (ids valid, no collisions).
        let mut esp = webos_esphome_api::Config::default();
        esp.power_sensor = true;
        esp.selects.push(crate::darken::select_entity());
        ents.add_to(&mut esp);
        esp.validate().unwrap();
        assert_eq!(esp.media_players.len(), 1);
    }

    #[test]
    fn the_media_player_state_and_commands_map_both_ways() {
        let m = |power, volume, muted| {
            state_command(StateUpdate {
                id: e::TV_MEDIA_PLAYER,
                value: StateValue::Media(webos_tv_control::MediaState {
                    power,
                    volume,
                    muted,
                }),
            })
        };
        assert_eq!(
            m(MediaPower::Playing, Some(0.2), true),
            AppCommand::SetMediaPlayer {
                object_id: "tv_media_player".into(),
                status: PlayerStatus {
                    state: PlayerState::Playing,
                    volume: Some(0.2),
                    muted: true
                }
            }
        );
        for (p, s) in [
            (MediaPower::On, PlayerState::On),
            (MediaPower::Off, PlayerState::Off),
        ] {
            assert!(matches!(
                m(p, None, false),
                AppCommand::SetMediaPlayer { status, .. } if status.state == s && status.volume.is_none()
            ));
        }
        let ev = |command| AppEvent::MediaPlayerCommand {
            object_id: "tv_media_player".into(),
            command,
        };
        for (c, a) in [
            (PlayerCommand::SetVolume(0.5), MediaAction::SetVolume(0.5)),
            (PlayerCommand::VolumeUp, MediaAction::VolumeUp),
            (PlayerCommand::VolumeDown, MediaAction::VolumeDown),
            (PlayerCommand::Mute(true), MediaAction::Mute(true)),
            (PlayerCommand::Mute(false), MediaAction::Mute(false)),
            (PlayerCommand::TurnOn, MediaAction::TurnOn),
            (PlayerCommand::TurnOff, MediaAction::TurnOff),
        ] {
            assert_eq!(
                command_for(&ev(c)),
                Some(Command::Media {
                    id: "tv_media_player".into(),
                    action: a
                })
            );
        }
        for c in [
            PlayerCommand::Play,
            PlayerCommand::Pause,
            PlayerCommand::Stop,
        ] {
            assert!(command_for(&ev(c)).is_none());
        }
        // Not a TV control media player.
        assert!(
            command_for(&AppEvent::MediaPlayerCommand {
                object_id: "other".into(),
                command: PlayerCommand::TurnOff
            })
            .is_none()
        );
        // The satellite's own media player events are not TV control.
        assert!(command_for(&AppEvent::VolumeChanged(30)).is_none());
        assert!(command_for(&AppEvent::MediaStop).is_none());
    }

    #[test]
    fn states_and_events_map_to_server_commands() {
        let sc = |id, value| state_command(StateUpdate { id, value });
        assert_eq!(
            sc(e::TV_SCREEN, StateValue::Switch(Some(true))),
            AppCommand::SetSwitch {
                object_id: "tv_screen".into(),
                state: Some(true)
            }
        );
        assert_eq!(
            sc(e::TV_VOLUME, StateValue::Number(None)),
            AppCommand::SetNumber {
                object_id: "tv_volume".into(),
                value: None
            }
        );
        assert_eq!(
            sc(e::TV_APP, StateValue::Select(Some("App A".into()))),
            AppCommand::SetSelect {
                object_id: "tv_app".into(),
                option: "App A".into()
            }
        );
        assert_eq!(
            sc(e::TV_APP, StateValue::Select(None)),
            AppCommand::ClearSelect {
                object_id: "tv_app".into()
            }
        );
        assert_eq!(
            sc(e::TV_SOUND_OUTPUT, StateValue::Text(Some("x".into()))),
            AppCommand::SetTextSensor {
                object_id: "tv_sound_output".into(),
                state: Some("x".into())
            }
        );
        // Only tv_ entities are TV control; the remote power-off select and
        // the microphone switch are not.
        let sel = |id: &str| AppEvent::SelectChanged {
            object_id: id.into(),
            option: "x".into(),
        };
        assert!(command_for(&sel("remote_power_off")).is_none());
        assert_eq!(
            command_for(&sel("tv_input")),
            Some(Command::Select {
                id: "tv_input".into(),
                option: "x".into()
            })
        );
        assert!(command_for(&AppEvent::MuteChanged(true)).is_none());
        assert!(
            command_for(&AppEvent::SwitchChanged {
                object_id: "mute_microphone".into(),
                state: true
            })
            .is_none()
        );
        assert_eq!(
            command_for(&AppEvent::NumberChanged {
                object_id: "tv_volume".into(),
                value: 5.0
            }),
            Some(Command::Number {
                id: "tv_volume".into(),
                value: 5.0
            })
        );
        assert_eq!(
            command_for(&AppEvent::ButtonPressed {
                object_id: "tv_turn_on".into()
            }),
            Some(Command::Button {
                id: "tv_turn_on".into()
            })
        );
    }

    #[test]
    fn controller_config_follows_the_settings() {
        let mut s = all_on();
        s.tv_control.volume_limit = Some(25);
        s.tv_control.app_limit = 7;
        let c = controller_config(&s, Some("firmware".to_owned()));
        assert!(
            c.screen
                && c.power_on
                && c.power_off
                && c.volume
                && c.mute
                && c.input
                && c.apps
                && c.play_state
        );
        assert_eq!((c.volume_limit, c.app_limit), (Some(25), 7));
        assert_eq!(c.setters_blocked.as_deref(), Some("firmware"));
        assert_eq!(c.own_app_ids, [OWN_APP_ID]);
        let c = controller_config(&Settings::default(), None);
        assert_eq!(
            c,
            Config {
                own_app_ids: vec![OWN_APP_ID.to_owned()],
                ..Config::default()
            }
        );
    }

    #[test]
    fn declaration_files_are_filtered_and_read() {
        let d = tempdir("decl");
        std::fs::write(
            d.join("com.webos.service.audio.api.json"),
            r#"{"g":["com.webos.service.audio/master/setVolume"]}"#,
        )
        .unwrap();
        std::fs::write(d.join("com.webos.service.other.api.json"), "{}").unwrap();
        std::fs::create_dir(d.join("com.webos.service.apiadapter.d")).unwrap();
        let files = read_declarations(&d);
        assert_eq!(files.len(), 1);
        assert_eq!(
            setters::declared_in(&files, Setter::VolumeSet),
            Declared::Yes
        );
        assert_eq!(setters::declared_in(&files, Setter::VolumeUp), Declared::No);
        assert_eq!(
            setters::declared_in(&files, Setter::Input),
            Declared::Unchecked
        );
        assert!(read_declarations(&d.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_check_runs_read_only_calls_on_a_terminal_and_lists_the_entities() {
        let d = tempdir("start");
        let luna = fake_luna(&d, false);
        let mut st = start(&all_on(), luna, None, &d.join("no-declarations")).await;
        let log = calls(&d);
        assert!(
            !log.contains("NOT-A-TTY"),
            "luna-send must get a terminal: {log}"
        );
        for uri in [
            "getPowerState",
            "getVolume",
            "getForegroundAppInfo",
            "getExternalInputList",
            "listLaunchPoints",
            r#"getSystemSettings {"category":"general","keys":["lifeOnScreenMode"]}"#,
            "getActivation {}",
        ] {
            assert!(log.contains(uri), "{uri} not called: {log}");
        }
        for write in [
            "setVolume",
            "setMute",
            "switchInput",
            "launch ",
            "turnOffScreen",
            "turnOnScreen",
            "powerOn",
            "controls/power",
            "setSystemSettings",
            "setActivation",
        ] {
            assert!(
                !log.contains(write),
                "startup must not write ({write}): {log}"
            );
        }
        assert!(!log.contains("getSystemInfo"));
        // The range comes from the TV, the lists from the TV.
        assert_eq!(st.entities.numbers[0].max_value, 60.0);
        assert_eq!(st.entities.selects[0].options, ["HDMI 1", "Other"]);
        assert_eq!(st.entities.selects[1].options, ["App A", "App B", "Other"]);
        // The first states and the two subscriptions are in the effects.
        let out = st.tv.exec(std::mem::take(&mut st.effects));
        assert!(out.commands.contains(&AppCommand::SetNumber {
            object_id: "tv_volume".into(),
            value: Some(12.0)
        }));
        assert!(out.commands.contains(&AppCommand::SetSelect {
            object_id: "tv_app".into(),
            option: "App A".into()
        }));
        // The TV media player: on (the fake reports no playState), 12 of 60.
        assert!(out.commands.contains(&AppCommand::SetMediaPlayer {
            object_id: "tv_media_player".into(),
            status: PlayerStatus {
                state: PlayerState::On,
                volume: Some(0.2),
                muted: false
            }
        }));
        assert_eq!(st.entities.media_players.len(), 1);
        // The two read-only LG sensors: diagnostic text sensors with their
        // first states (nothing else is listed for them).
        let lg: Vec<_> = st
            .entities
            .text_sensors
            .iter()
            .filter(|t| t.object_id.starts_with("lg_"))
            .map(|t| (t.object_id.as_str(), t.name.as_str(), t.entity_category))
            .collect();
        assert_eq!(
            lg,
            [
                (
                    "lg_always_ready",
                    "Always Ready",
                    EntityCategory::Diagnostic
                ),
                ("lg_hands_free", "LG hands-free", EntityCategory::Diagnostic)
            ]
        );
        for (id, state) in [("lg_always_ready", "Off"), ("lg_hands_free", "Off")] {
            assert!(
                out.commands.contains(&AppCommand::SetTextSensor {
                    object_id: id.into(),
                    state: Some(state.into())
                }),
                "{id}"
            );
        }
        // Subscription children run and their first lines come back as messages.
        let mut got = 0;
        while got < 2 {
            let m = tokio::time::timeout(Duration::from_secs(10), st.rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(m, TvMsg::Line(_, _)));
            st.tv.on_msg(m);
            got += 1;
        }
        assert!(calls(&d).contains("SUB luna://com.webos.service.audio/master/getVolume"));
        st.tv.shutdown().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_getter_still_lists_the_entity_with_no_state_and_commands_make_no_call() {
        let d = tempdir("fail");
        let luna = fake_luna(&d, true);
        let mut st = start(&all_on(), luna, None, &d.join("no-declarations")).await;
        // Listed, with the default range (the TV did not say).
        assert_eq!(st.entities.numbers[0].max_value, 100.0);
        let out = st.tv.exec(std::mem::take(&mut st.effects));
        assert!(out.commands.contains(&AppCommand::SetNumber {
            object_id: "tv_volume".into(),
            value: None
        }));
        assert!(out.commands.contains(&AppCommand::SetSwitch {
            object_id: "tv_mute".into(),
            state: None
        }));
        // A command is refused: no setter is called.
        let out = st
            .tv
            .on_event(&AppEvent::NumberChanged {
                object_id: "tv_volume".into(),
                value: 20.0,
            })
            .unwrap();
        assert!(!out.own_wake);
        tokio::time::sleep(Duration::from_millis(400)).await;
        st.tv.tick(PowerOffEnv::default());
        tokio::time::sleep(Duration::from_millis(400)).await;
        let log = calls(&d);
        assert!(
            !log.contains("setVolume") && !log.contains("setMute"),
            "no write after a failed getter: {log}"
        );
        st.tv.shutdown().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_command_runs_one_setter_with_the_verified_payload_and_reads_back() {
        let d = tempdir("cmd");
        let luna = fake_luna(&d, false);
        let mut st = start(&all_on(), luna, None, &d.join("no-declarations")).await;
        let fx = std::mem::take(&mut st.effects);
        st.tv.exec(fx);
        // The setters are not declared (no files): Unchecked is allowed.
        let out = st
            .tv
            .on_event(&AppEvent::NumberChanged {
                object_id: "tv_volume".into(),
                value: 20.0,
            })
            .unwrap();
        assert!(out.commands.is_empty());
        // Coalescing: the call is made on a tick after 250 ms.
        tokio::time::sleep(Duration::from_millis(300)).await;
        st.tv.tick(PowerOffEnv::default());
        let mut set_seen = false;
        for _ in 0..20 {
            let m = tokio::time::timeout(Duration::from_secs(5), st.rx.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(&m, TvMsg::Reply(Purpose::Set(_), _)) {
                set_seen = true;
            }
            st.tv.on_msg(m);
            if set_seen {
                break;
            }
        }
        assert!(set_seen);
        assert!(
            calls(&d).contains(r#"luna://com.webos.service.audio/master/setVolume {"volume":20}"#),
            "{}",
            calls(&d)
        );
        st.tv.shutdown().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_media_player_command_runs_the_same_setter() {
        let d = tempdir("mp");
        let luna = fake_luna(&d, false);
        let mut st = start(&all_on(), luna, None, &d.join("no-declarations")).await;
        let fx = std::mem::take(&mut st.effects);
        st.tv.exec(fx);
        // Half of maxVolume (60): setVolume 30, after the coalescing delay.
        let out = st
            .tv
            .on_event(&AppEvent::MediaPlayerCommand {
                object_id: "tv_media_player".into(),
                command: PlayerCommand::SetVolume(0.5),
            })
            .unwrap();
        assert!(out.commands.is_empty(), "not echoed");
        tokio::time::sleep(Duration::from_millis(300)).await;
        st.tv.tick(PowerOffEnv::default());
        let mut set_seen = false;
        for _ in 0..20 {
            let m = tokio::time::timeout(Duration::from_secs(5), st.rx.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(&m, TvMsg::Reply(Purpose::Set(_), _)) {
                set_seen = true;
            }
            st.tv.on_msg(m);
            if set_seen {
                break;
            }
        }
        assert!(set_seen);
        assert!(
            calls(&d).contains(r#"luna://com.webos.service.audio/master/setVolume {"volume":30}"#),
            "{}",
            calls(&d)
        );
        st.tv.shutdown().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_probe_that_hangs_ends_within_the_budget_and_counts_as_failed() {
        let d = tempdir("hang");
        // Answers the power state, then hangs on everything else.
        let p = d.join("luna-send");
        crate::testutil::write_script(
            &p,
            "case \"$5\" in */getPowerState) echo '{\"returnValue\":true,\"state\":\"Active\"}';; *) exec sleep 30;; esac",
        );
        let t0 = Instant::now();
        let st = start(&all_on(), p, None, &d).await;
        assert!(
            t0.elapsed() < PROBE_BUDGET + Duration::from_secs(3),
            "{:?}",
            t0.elapsed()
        );
        assert!(
            st.entities.numbers.len() == 1,
            "entities are listed even when nothing answered"
        );
        st.tv.shutdown().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lg_sensors_follow_the_tv_on_a_refresh_and_never_write() {
        let d = tempdir("lg");
        let luna = fake_luna(&d, false);
        let mut st = start(&all_on(), luna, None, &d.join("no-declarations")).await;
        let _ = st.tv.exec(std::mem::take(&mut st.effects));
        // ctl.sh set changed both (synthetic replies in the fake's files).
        std::fs::write(
            d.join("lg_always_ready"),
            r#"{"returnValue":true,"settings":{"lifeOnScreenMode":"allEnabled"}}"#,
        )
        .unwrap();
        std::fs::write(
            d.join("lg_hands_free"),
            r#"{"returnValue":true,"settingStatus":true,"thinQ":{"settingStatus":true}}"#,
        )
        .unwrap();
        let before = calls(&d).matches("getSystemSettings").count();
        let out = st.tv.refresh_lg_settings();
        assert!(out.commands.is_empty(), "the states come with the replies");
        let mut seen = Vec::new();
        while seen.len() < 2 {
            let m = tokio::time::timeout(Duration::from_secs(10), st.rx.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(
                m,
                TvMsg::Reply(Purpose::Read(Topic::AlwaysReady | Topic::HandsFree), _)
            ) {
                seen.extend(st.tv.on_msg(m).commands);
            } else {
                st.tv.on_msg(m);
            }
        }
        assert!(seen.contains(&AppCommand::SetTextSensor {
            object_id: "lg_always_ready".into(),
            state: Some("On with wallpaper".into())
        }));
        assert!(seen.contains(&AppCommand::SetTextSensor {
            object_id: "lg_hands_free".into(),
            state: Some("On".into())
        }));
        assert_eq!(calls(&d).matches("getSystemSettings").count(), before + 1);
        // No command event reaches either sensor, and no setter was called.
        assert!(
            st.tv
                .on_event(&AppEvent::SelectChanged {
                    object_id: "lg_always_ready".into(),
                    option: "On".into(),
                })
                .is_none(),
            "not a TV control command"
        );
        let log = calls(&d);
        assert!(!log.contains("setSystemSettings") && !log.contains("setActivation"));
        assert!(!log.contains("NOT-A-TTY"));
        st.tv.shutdown().await;
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn lg_settings_follow_the_config() {
        let mut s = Settings::default();
        assert!(controller_config(&s, None).lg_settings);
        s.tv_control.lg_settings = false;
        assert!(!controller_config(&s, None).lg_settings);
    }
}
