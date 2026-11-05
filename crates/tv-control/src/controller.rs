//! The TV control state machine (`docs/design/tv-control.md`).
//!
//! Sans-IO: the owner feeds it power-state updates, luna replies, subscription
//! lines, Home Assistant commands and the time, and runs the [`Effect`]s it
//! returns (luna calls, entity states, log lines). It spawns nothing, reads no
//! file and knows nothing about capture, voice or audio.
//!
//! Rules it enforces, each with a test:
//!
//! - **Never a guess.** A control whose getter failed, whose setter is not
//!   declared, or whose last write did not read back reports a missing state
//!   and refuses commands. There is no fallback to another method or
//!   parameter set; the only retry is the verified `powerOn` after
//!   `Keyblock1.5second`.
//! - **Never a write at startup**; setters run only for an HA command.
//! - **Power state gates** (design section 3.4): `Active Standby` and
//!   transitions refuse everything, `Screen Off` allows volume, mute, input
//!   and app, `Suspend` never reaches the daemon.
//! - **Power-off** only with every precondition of design section 4.2
//!   ([`Controller::power_off_check`]).
//! - **Limits** ([`crate::limits`]) and the `volume_limit` cap.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::entities::{self as ent, Category, EntityDef, Kind, StateUpdate, StateValue};
use crate::limits::{Control, Limiter};
use crate::lists::{self, App, Input, OTHER};
use crate::luna::{self, ExternalInput, LaunchPoint, ReplyError, VolumeStatus};
use crate::setters::{self, Declared, Setter};

/// Coalescing delay of volume set commands (latest value wins).
pub const VOLUME_COALESCE: Duration = Duration::from_millis(250);
/// First read-back after a write.
pub const READ_BACK_DELAY: Duration = Duration::from_millis(500);
/// Later read-backs (input and app changes take a moment); at most two
/// reads, never a second write.
pub const READ_BACK_RETRY: Duration = Duration::from_millis(2500);
/// The key block of tvpowerd after a settle (D6).
pub const KEY_BLOCK: Duration = Duration::from_millis(1500);
/// `powerOn` retry after `Keyblock1.5second`.
pub const POWER_ON_RETRY: Duration = Duration::from_millis(1600);
/// `powerOn` attempts (the first and one retry).
pub const POWER_ON_TRIES: u8 = 2;
/// Poll interval of volume and play state, when no subscription runs.
pub const POLL_SLOW: Duration = Duration::from_secs(10);
/// Poll interval of the foreground app, when no subscription runs.
pub const POLL_FOREGROUND: Duration = Duration::from_secs(5);
/// How long a change seen by polling may go without a pushed line before the
/// subscription of an unconfirmed topic is dropped (polling only).
pub const PUSH_GRACE: Duration = Duration::from_secs(3);
/// Minimum time between two list refreshes.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// Poll interval of the two LG settings sensors (Always Ready, LG
/// hands-free): two short read-only calls a minute.
pub const POLL_LG_SETTINGS: Duration = Duration::from_secs(60);

/// Which lists and controls are on, and the guards that are decided outside.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one bool per TOML switch
pub struct Config {
    /// `tv_screen`.
    pub screen: bool,
    /// `tv_turn_on`.
    pub power_on: bool,
    /// `tv_power` (opt-in).
    pub power_off: bool,
    /// `tv_volume` and the step buttons.
    pub volume: bool,
    /// Optional cap on the TV volume HA may set. `None` (the default): no cap
    /// below the TV's own `maxVolume`. With a cap, a set above it is clamped
    /// to it, "volume up" stops at it, and the HA slider ends at it; a
    /// volume above the cap (set at the TV) can still be lowered by any set
    /// or "volume down".
    pub volume_limit: Option<u32>,
    /// `tv_mute`.
    pub mute: bool,
    /// `tv_input`.
    pub input: bool,
    /// `tv_app`.
    pub apps: bool,
    /// Most apps in the select.
    pub app_limit: usize,
    /// `tv_play_state`.
    pub play_state: bool,
    /// `tv_media_player`: the TV as a media player (ADR 0007), so HA's
    /// native volume, mute and turn on/off intents work. Its features follow
    /// the other flags; it reads the play state (polled) for `playing`.
    pub media_player: bool,
    /// The read-only sensors `lg_always_ready` and `lg_hands_free`
    /// (ADR 0008 decision 4): two getters, no setter, polled every 60 s while
    /// the TV is lit.
    pub lg_settings: bool,
    /// `Some(reason)` while every setter is refused (the firmware guard).
    /// Getters and sensors still report.
    pub setters_blocked: Option<String>,
    /// App ids that are left out of the app list (the satellite's own app).
    pub own_app_ids: Vec<String>,
}

impl Default for Config {
    /// The defaults: screen and power-on on, power-off off (opt-in), volume,
    /// mute, input and apps on (EXP-010 Part A passed on c3-b, 2026-11-04;
    /// each control is still checked per TV at startup), the TV media player
    /// on (owner, 2026-11-04).
    fn default() -> Self {
        Self {
            screen: true,
            power_on: true,
            power_off: false,
            volume: true,
            volume_limit: None,
            mute: true,
            input: true,
            apps: true,
            app_limit: 30,
            play_state: false,
            media_player: true,
            lg_settings: true,
            setters_blocked: None,
            own_app_ids: Vec::new(),
        }
    }
}

/// What the daemon knows that the power-off needs (design section 4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerOffEnv {
    /// The select "When the TV is turned off with the remote" is on "Keep
    /// listening (screen off)".
    pub keep_listening: bool,
    /// The kill switch file is present.
    pub kill_switch: bool,
    /// Whether the wake-and-darken machine would act now: `Err(reason)` when
    /// it would refuse (cooldown, hourly budget, ...).
    pub darkener: Result<(), String>,
}

impl Default for PowerOffEnv {
    /// Nothing is allowed until the daemon says so.
    fn default() -> Self {
        Self {
            keep_listening: false,
            kill_switch: false,
            darkener: Err("not known yet".to_owned()),
        }
    }
}

/// Why a power-off is refused (the first failing precondition).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerOffRefusal {
    /// `[tv_control] power_off` is false.
    NotEnabled,
    /// Setters are blocked (firmware guard).
    SettersBlocked(String),
    /// The always-on select is not on "Keep listening".
    NotKeepListening,
    /// The power state is not known yet.
    PowerStateUnknown,
    /// A power transition is in progress.
    Transition,
    /// Not `Active` or `Screen Saver`: the key is a toggle and must not be
    /// sent from `Screen Off` or `Active Standby`.
    WrongState,
    /// The kill switch is present.
    KillSwitch,
    /// The darkener would refuse (reason).
    Darkener(String),
}

impl fmt::Display for PowerOffRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEnabled => f.write_str("power_off is not enabled in [tv_control]"),
            Self::SettersBlocked(w) => write!(f, "TV control setters are off ({w})"),
            Self::NotKeepListening => f.write_str(
                "\"When the TV is turned off with the remote\" is not on \"Keep listening\"",
            ),
            Self::PowerStateUnknown => f.write_str("the TV power state is not known"),
            Self::Transition => f.write_str("the TV is changing power state"),
            Self::WrongState => f.write_str("the TV is not on (Active or Screen Saver)"),
            Self::KillSwitch => f.write_str("the kill switch is present"),
            Self::Darkener(w) => write!(f, "wake-and-darken would not act ({w})"),
        }
    }
}

/// A TV power state as TV control distinguishes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    /// `Active`.
    Active,
    /// `Screen Saver`.
    ScreenSaver,
    /// `Screen Off`.
    ScreenOff,
    /// `Active Standby`.
    ActiveStandby,
    /// `Suspend`.
    Suspend,
    /// Any other state name.
    Other,
}

impl PowerState {
    /// From a state name without spaces (`ActiveStandby`).
    #[must_use]
    pub fn from_key(key: &str) -> Self {
        match key {
            "Active" => Self::Active,
            "ScreenSaver" => Self::ScreenSaver,
            "ScreenOff" => Self::ScreenOff,
            "ActiveStandby" => Self::ActiveStandby,
            "Suspend" => Self::Suspend,
            _ => Self::Other,
        }
    }
}

/// The power state as the availability rules see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pwr {
    Unknown,
    Transition,
    /// `Active` or `Screen Saver`.
    Lit,
    /// `Screen Off`.
    Dark,
    Standby,
    Other,
}

/// A thing to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    /// `getPowerState`.
    Power,
    /// `getVolume` (volume, mute, sound output).
    Volume,
    /// The foreground app.
    Foreground,
    /// The external input list.
    Inputs,
    /// The launch points.
    Apps,
    /// The media play state.
    PlayState,
    /// Always Ready (`lifeOnScreenMode`), read-only.
    AlwaysReady,
    /// LG hands-free (`getActivation`), read-only.
    HandsFree,
}

impl Topic {
    fn name(self) -> &'static str {
        match self {
            Self::Power => "power state",
            Self::Volume => "volume",
            Self::Foreground => "foreground app",
            Self::Inputs => "input list",
            Self::Apps => "app list",
            Self::PlayState => "play state",
            Self::AlwaysReady => "Always Ready setting",
            Self::HandsFree => "LG hands-free setting",
        }
    }

    fn uri(self) -> &'static str {
        match self {
            Self::Power => setters::URI_POWER_STATE,
            Self::Volume => setters::URI_GET_VOLUME,
            Self::Foreground => setters::URI_FOREGROUND,
            Self::Inputs => setters::URI_INPUT_LIST,
            Self::Apps => setters::URI_LAUNCH_POINTS,
            Self::PlayState => setters::URI_PLAY_STATE,
            Self::AlwaysReady => setters::URI_GET_SYSTEM_SETTINGS,
            Self::HandsFree => setters::URI_GET_ACTIVATION,
        }
    }

    /// The payload of the getter.
    fn payload(self) -> &'static str {
        match self {
            Self::AlwaysReady => setters::PAYLOAD_LIFE_ON_SCREEN_MODE,
            _ => setters::PAYLOAD_EMPTY,
        }
    }

    /// The luna URI a subscription for this topic uses (volume and foreground
    /// app only).
    #[must_use]
    pub fn subscription_uri(self) -> Option<&'static str> {
        match self {
            Self::Volume | Self::Foreground => Some(self.uri()),
            _ => None,
        }
    }
}

/// A setter call, as the reply handler needs to know it.
#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    /// `turnOffScreen`.
    ScreenOff,
    /// `turnOnScreen`, for the screen switch ([`Control::Screen`]) or for the
    /// power paths ([`Control::Power`]); the control owns the in-flight slot.
    ScreenOn(Control),
    /// The remote's power key (power-off in the keep-listening mode).
    PowerKey,
    /// `powerOn`, attempt 1 or 2.
    PowerOn(u8),
    /// `setVolume`.
    VolumeSet(u32),
    /// `volumeUp`.
    VolumeUp,
    /// `volumeDown`.
    VolumeDown,
    /// The mute setter.
    Mute(bool),
    /// `switchInput` with this input id (never logged).
    Input(String),
    /// `launch` with this app id (never logged).
    Launch(String),
}

impl Action {
    fn control(&self) -> Control {
        match self {
            Self::ScreenOff => Control::Screen,
            Self::ScreenOn(c) => *c,
            Self::PowerKey | Self::PowerOn(_) => Control::Power,
            Self::VolumeSet(_) | Self::VolumeUp | Self::VolumeDown => Control::Volume,
            Self::Mute(_) => Control::Mute,
            Self::Input(_) => Control::Input,
            Self::Launch(_) => Control::Apps,
        }
    }

    /// The method name for log lines (no ids, no values).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::ScreenOff => "turnOffScreen",
            Self::ScreenOn(_) => "turnOnScreen",
            Self::PowerKey => "power key",
            Self::PowerOn(_) => "powerOn",
            Self::VolumeSet(_) => "setVolume",
            Self::VolumeUp => "volumeUp",
            Self::VolumeDown => "volumeDown",
            Self::Mute(_) => "setMute",
            Self::Input(_) => "switchInput",
            Self::Launch(_) => "launch",
        }
    }
}

impl fmt::Debug for Action {
    /// Names only: an input id or an app id may say what the owner watches.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a luna call is made; passed back with the reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// A getter.
    Read(Topic),
    /// A setter.
    Set(Action),
}

/// Log level of an [`Effect::Log`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Debug (titles and ids may appear here only).
    Debug,
    /// Info.
    Info,
    /// Warn.
    Warn,
}

/// What the owner must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Run `luna-send -n 1 -w 3000 <uri> <payload>` and pass the output (or
    /// the failure) to [`Controller::on_reply`].
    Call {
        /// Passed back with the reply.
        purpose: Purpose,
        /// The luna URI.
        uri: &'static str,
        /// The JSON payload.
        payload: String,
    },
    /// Start a supervised `luna-send -i` subscription for the topic and feed
    /// its lines to [`Controller::on_subscription_line`].
    Subscribe(Topic),
    /// Stop that subscription (it is not supported; the controller polls).
    Unsubscribe(Topic),
    /// Send this state to HA.
    State(StateUpdate),
    /// The satellite itself wakes the TV from `Screen Off` or standby: tell
    /// the darkener, so its "screen dark by us" bookkeeping stays right.
    OwnWake,
    /// A log line.
    Log(Level, String),
}

/// A command from HA.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// A switch was turned on or off.
    Switch {
        /// The entity's object id.
        id: String,
        /// The state asked for.
        state: bool,
    },
    /// A number was set.
    Number {
        /// The entity's object id.
        id: String,
        /// The value asked for.
        value: f32,
    },
    /// A select option was chosen.
    Select {
        /// The entity's object id.
        id: String,
        /// The option.
        option: String,
    },
    /// A button was pressed.
    Button {
        /// The entity's object id.
        id: String,
    },
    /// A media player command.
    Media {
        /// The entity's object id.
        id: String,
        /// What HA asked for.
        action: MediaAction,
    },
}

/// A command HA sends to the TV media player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaAction {
    /// Volume as a share of the TV's `maxVolume` (0.0 to 1.0).
    SetVolume(f32),
    /// One TV volume step up (`volumeUp`, as the remote).
    VolumeUp,
    /// One TV volume step down.
    VolumeDown,
    /// Mute (`true`) or unmute.
    Mute(bool),
    /// Turn on: as the `tv_turn_on` button.
    TurnOn,
    /// Turn off: the opt-in power-off when it is enabled and the always-on
    /// mode is selected, else the screen off.
    TurnOff,
}

impl MediaAction {
    fn name(self) -> &'static str {
        match self {
            Self::SetVolume(_) => "volume set",
            Self::VolumeUp => "volume up",
            Self::VolumeDown => "volume down",
            Self::Mute(true) => "mute",
            Self::Mute(false) => "unmute",
            Self::TurnOn => "turn on",
            Self::TurnOff => "turn off",
        }
    }
}

/// What a read-back must show.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expect {
    Volume(u32),
    VolumeStep { from: u32, up: bool },
    Mute(bool),
    Foreground(String),
}

#[derive(Debug, Clone)]
struct Check {
    control: Control,
    topic: Topic,
    expect: Expect,
    due: Instant,
    issued: bool,
    reads_left: u8,
}

/// What the TV says is in the foreground.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Foreground {
    /// Not read, or the read failed.
    Unknown,
    /// No app is in the foreground.
    NoApp,
    /// This app id.
    App(String),
}

/// Wanted entity state.
enum Want {
    /// Leave the state HA has.
    Hold,
    Set(StateValue),
}

/// Startup and runtime status of one control, for the compatibility report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlStatus {
    /// Enabled and working.
    Ok,
    /// Not enabled in the configuration.
    Off,
    /// Enabled but its getter failed or it was switched off after a failure.
    Failed,
}

/// The state machine.
pub struct Controller {
    cfg: Config,
    env: PowerOffEnv,
    declared: HashMap<Setter, Declared>,
    ready: bool,

    power: Option<PowerState>,
    transition: bool,
    settled_at: Option<Instant>,
    vol: Option<VolumeStatus>,
    fg: Foreground,
    raw_inputs: Vec<ExternalInput>,
    raw_points: Vec<LaunchPoint>,
    inputs: Vec<Input>,
    apps: Vec<App>,
    play: Option<String>,
    always_ready: Option<luna::AlwaysReady>,
    hands_free: Option<luna::HandsFree>,

    /// Options HA was told (what the selects list), `Other` last.
    listed_inputs: Vec<String>,
    listed_apps: Vec<String>,
    /// Options waiting for the daemon to apply them.
    pending_options: HashMap<&'static str, Vec<String>>,

    getter_ok: HashSet<Topic>,
    warned_read: HashSet<Topic>,
    disabled: HashMap<Control, String>,
    probe_pending: HashSet<Topic>,
    resume_pending: HashSet<Topic>,

    limiter: Limiter,
    sent: HashMap<&'static str, StateValue>,
    pending_volume: Option<(u32, Instant)>,
    check: Option<Check>,
    power_on_at: Option<(Instant, u8)>,
    polling: HashSet<Topic>,
    subscribed: HashSet<Topic>,
    /// Topics whose subscription has delivered a first line.
    first_line_seen: HashSet<Topic>,
    /// Subscriptions whose first reply lacked `subscribed: true`: kept
    /// running next to polling until a second (pushed) line proves that they
    /// stream, or a change seen by polling shows that they do not.
    unconfirmed: HashSet<Topic>,
    /// An unconfirmed topic whose polled value changed without a pushed line,
    /// and since when.
    suspect: HashMap<Topic, Instant>,
    next_poll: HashMap<Topic, Instant>,
    reads_inflight: HashSet<Topic>,
    last_refresh: Option<Instant>,
    /// Lists a refresh button press is still waiting for, and the reasons
    /// of the reads of it that failed (for the one summary line).
    refresh_pending: Vec<Topic>,
    refresh_failed: Vec<String>,
}

/// An LG setting value as the log shows it: the label in lower case, and an
/// unknown Always Ready word (printable, at most 32 characters).
trait LgValue: PartialEq {
    fn describe(&self) -> String;
}

impl LgValue for luna::AlwaysReady {
    fn describe(&self) -> String {
        match self {
            Self::Other(word) => format!("other ({word})"),
            other => other.label().to_lowercase(),
        }
    }
}

impl LgValue for luna::HandsFree {
    fn describe(&self) -> String {
        self.label().to_lowercase()
    }
}

fn log(level: Level, msg: impl Into<String>) -> Effect {
    Effect::Log(level, msg.into())
}

/// The TV volume as the media player's share of `maxVolume` (0.0 to 1.0);
/// `None` for a TV that reports no maximum.
#[must_use]
pub fn volume_share(volume: u32, max_volume: u32) -> Option<f32> {
    if max_volume == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)] // TV volumes are tiny
    let share = volume.min(max_volume) as f32 / max_volume as f32;
    Some(share)
}

/// The media player's share (0.0 to 1.0, clamped) in TV volume steps of
/// `maxVolume`, not yet rounded (the volume set rounds to the nearest step).
#[must_use]
pub fn share_to_steps(share: f32, max_volume: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)] // TV volumes are tiny
    let max = max_volume as f32;
    share.clamp(0.0, 1.0) * max
}

impl Controller {
    /// A controller for `cfg`. Setters are treated as not declared until
    /// [`Controller::set_declared`] says otherwise.
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            env: PowerOffEnv::default(),
            declared: HashMap::new(),
            ready: false,
            power: None,
            transition: false,
            settled_at: None,
            vol: None,
            fg: Foreground::Unknown,
            raw_inputs: Vec::new(),
            raw_points: Vec::new(),
            inputs: Vec::new(),
            apps: Vec::new(),
            play: None,
            always_ready: None,
            hands_free: None,
            listed_inputs: vec![OTHER.to_owned()],
            listed_apps: vec![OTHER.to_owned()],
            pending_options: HashMap::new(),
            getter_ok: HashSet::new(),
            warned_read: HashSet::new(),
            disabled: HashMap::new(),
            probe_pending: HashSet::new(),
            resume_pending: HashSet::new(),
            limiter: Limiter::default(),
            sent: HashMap::new(),
            pending_volume: None,
            check: None,
            power_on_at: None,
            polling: HashSet::new(),
            subscribed: HashSet::new(),
            first_line_seen: HashSet::new(),
            unconfirmed: HashSet::new(),
            suspect: HashMap::new(),
            next_poll: HashMap::new(),
            reads_inflight: HashSet::new(),
            last_refresh: None,
            refresh_pending: Vec::new(),
            refresh_failed: Vec::new(),
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Records whether the TV declares a setter. An undeclared setter makes
    /// its control read-only ([`Declared::No`]).
    pub fn set_declared(&mut self, setter: Setter, declared: Declared) {
        self.declared.insert(setter, declared);
    }

    /// The daemon's view of the power-off preconditions; call it whenever the
    /// select, the kill switch or the darkener's verdict may have changed
    /// (about once a second). Returns the state changes.
    pub fn set_env(&mut self, env: PowerOffEnv) -> Vec<Effect> {
        if env == self.env {
            return Vec::new();
        }
        self.env = env;
        self.sync()
    }

    // ----- what is wanted ------------------------------------------------

    fn wanted(&self, topic: Topic) -> bool {
        match topic {
            Topic::Power => true,
            Topic::Volume => self.cfg.volume || self.cfg.mute,
            Topic::Foreground => self.cfg.input || self.cfg.apps,
            Topic::Inputs => self.cfg.input,
            Topic::Apps => self.cfg.apps,
            Topic::PlayState => self.play_reader(),
            Topic::AlwaysReady | Topic::HandsFree => self.cfg.lg_settings,
        }
    }

    /// The play state is read: for its sensor, or for the media player's
    /// `playing`.
    fn play_reader(&self) -> bool {
        self.cfg.play_state || self.media_listed()
    }

    /// The TV media player's features: each follows its control's flag.
    #[must_use]
    pub fn media_features(&self) -> ent::MediaFeatures {
        ent::MediaFeatures {
            volume_set: self.cfg.volume,
            volume_step: self.cfg.volume,
            mute: self.cfg.mute,
            turn_on: self.cfg.power_on,
            turn_off: self.cfg.screen || self.cfg.power_off,
        }
    }

    /// The TV media player is listed: enabled and it offers something.
    fn media_listed(&self) -> bool {
        self.cfg.media_player && self.media_features().any()
    }

    const TOPICS: [Topic; 8] = [
        Topic::Power,
        Topic::Volume,
        Topic::Foreground,
        Topic::Inputs,
        Topic::Apps,
        Topic::PlayState,
        Topic::AlwaysReady,
        Topic::HandsFree,
    ];

    /// The two LG settings sensors' getters.
    const LG_TOPICS: [Topic; 2] = [Topic::AlwaysReady, Topic::HandsFree];

    /// The entities of the current configuration, in display order. Select
    /// options and the volume range are those HA was told.
    #[must_use]
    #[allow(clippy::too_many_lines)] // one flat table
    pub fn entities(&self) -> Vec<EntityDef> {
        let mut v = Vec::new();
        let mut add = |id, name, icon, category, kind| {
            v.push(EntityDef {
                id,
                name,
                icon,
                category,
                kind,
            });
        };
        if self.media_listed() {
            // HA names it after the device ("<device> TV"); the owner adds
            // the Assist aliases ("tele del hall") in HA.
            add(
                ent::TV_MEDIA_PLAYER,
                "TV",
                "mdi:television",
                Category::None,
                Kind::MediaPlayer {
                    features: self.media_features(),
                },
            );
        }
        if self.cfg.screen {
            add(
                ent::TV_SCREEN,
                "TV screen",
                "mdi:television",
                Category::None,
                Kind::Switch,
            );
        }
        if self.cfg.power_on {
            add(
                ent::TV_TURN_ON,
                "Turn on TV",
                "mdi:television-play",
                Category::None,
                Kind::Button,
            );
        }
        if self.cfg.power_off {
            add(
                ent::TV_POWER,
                "TV power (keep listening)",
                "mdi:power",
                Category::None,
                Kind::Switch,
            );
        }
        if self.cfg.volume {
            #[allow(clippy::cast_precision_loss)] // a TV volume range is tiny
            let max = self
                .vol
                .as_ref()
                .map_or(100, |v| self.volume_ceiling(v.max_volume)) as f32;
            add(
                ent::TV_VOLUME,
                "TV volume",
                "mdi:volume-high",
                Category::None,
                Kind::Number { max },
            );
            add(
                ent::TV_VOLUME_UP,
                "TV volume up",
                "mdi:volume-plus",
                Category::None,
                Kind::Button,
            );
            add(
                ent::TV_VOLUME_DOWN,
                "TV volume down",
                "mdi:volume-minus",
                Category::None,
                Kind::Button,
            );
        }
        if self.cfg.mute {
            add(
                ent::TV_MUTE,
                "TV mute",
                "mdi:volume-off",
                Category::None,
                Kind::Switch,
            );
        }
        if self.cfg.input {
            add(
                ent::TV_INPUT,
                "TV input",
                "mdi:video-input-hdmi",
                Category::None,
                Kind::Select {
                    options: self.listed_inputs.clone(),
                },
            );
        }
        if self.cfg.apps {
            add(
                ent::TV_APP,
                "TV app",
                "mdi:apps",
                Category::None,
                Kind::Select {
                    options: self.listed_apps.clone(),
                },
            );
        }
        if self.cfg.input || self.cfg.apps {
            add(
                ent::TV_REFRESH_LISTS,
                "Refresh TV inputs and apps",
                "mdi:refresh",
                Category::Config,
                Kind::Button,
            );
        }
        if self.cfg.volume || self.cfg.mute {
            add(
                ent::TV_SOUND_OUTPUT,
                "TV sound output",
                "mdi:speaker",
                Category::Diagnostic,
                Kind::TextSensor,
            );
        }
        if self.cfg.play_state {
            add(
                ent::TV_PLAY_STATE,
                "TV play state",
                "mdi:play-pause",
                Category::Diagnostic,
                Kind::TextSensor,
            );
        }
        if self.cfg.lg_settings {
            // Read-only (ADR 0008 decision 4): no switch, select or button.
            add(
                ent::LG_ALWAYS_READY,
                "Always Ready",
                "mdi:television-ambient-light",
                Category::Diagnostic,
                Kind::TextSensor,
            );
            add(
                ent::LG_HANDS_FREE,
                "LG hands-free",
                "mdi:account-voice",
                Category::Diagnostic,
                Kind::TextSensor,
            );
        }
        v
    }

    // ----- availability ----------------------------------------------------

    fn class(&self) -> Pwr {
        let Some(p) = self.power else {
            return Pwr::Unknown;
        };
        if self.transition {
            return Pwr::Transition;
        }
        match p {
            PowerState::Active | PowerState::ScreenSaver => Pwr::Lit,
            PowerState::ScreenOff => Pwr::Dark,
            PowerState::ActiveStandby => Pwr::Standby,
            PowerState::Suspend | PowerState::Other => Pwr::Other,
        }
    }

    /// The getters a control needs all answered.
    fn getter_ready(&self, control: Control) -> bool {
        let ok = |t: Topic| self.getter_ok.contains(&t);
        match control {
            Control::Volume | Control::Mute => ok(Topic::Volume),
            Control::Input => ok(Topic::Inputs) && ok(Topic::Foreground),
            Control::Apps => ok(Topic::Apps) && ok(Topic::Foreground),
            Control::PlayState => ok(Topic::PlayState),
            Control::Screen | Control::Power => true,
        }
    }

    /// What the compatibility report says about a control: `Off` (not
    /// enabled), `Failed` (its getter did not answer or it was switched off
    /// after a failed write) or `Ok`.
    #[must_use]
    pub fn control_status(&self, control: Control) -> ControlStatus {
        if !self.control_enabled(control) {
            ControlStatus::Off
        } else if self.getter_ready(control) && !self.disabled.contains_key(&control) {
            ControlStatus::Ok
        } else {
            ControlStatus::Failed
        }
    }

    /// The control's state can be shown (enabled, getter answered, not
    /// disabled after a failed write).
    fn state_known(&self, control: Control) -> bool {
        let enabled = match control {
            Control::Volume => self.cfg.volume,
            Control::Mute => self.cfg.mute,
            Control::Input => self.cfg.input,
            Control::Apps => self.cfg.apps,
            Control::PlayState => self.cfg.play_state,
            Control::Screen => self.cfg.screen,
            Control::Power => true,
        };
        enabled && self.getter_ready(control) && !self.disabled.contains_key(&control)
    }

    fn want(&self, id: &'static str) -> Want {
        use StateValue as S;
        let class = self.class();
        match id {
            ent::TV_SCREEN => match class {
                Pwr::Transition => Want::Hold,
                Pwr::Lit => Want::Set(S::Switch(Some(true))),
                Pwr::Dark => Want::Set(S::Switch(Some(false))),
                _ => Want::Set(S::Switch(None)),
            },
            ent::TV_POWER => {
                if self.power_gates().is_err() {
                    return Want::Set(S::Switch(None));
                }
                match class {
                    Pwr::Transition => Want::Hold,
                    Pwr::Lit => Want::Set(S::Switch(Some(true))),
                    Pwr::Dark | Pwr::Standby => Want::Set(S::Switch(Some(false))),
                    _ => Want::Set(S::Switch(None)),
                }
            }
            ent::TV_VOLUME | ent::TV_MUTE => {
                let control = if id == ent::TV_VOLUME {
                    Control::Volume
                } else {
                    Control::Mute
                };
                self.live(class, control, |s| match (&s.vol, id) {
                    (Some(v), ent::TV_VOLUME) =>
                    {
                        #[allow(clippy::cast_precision_loss)]
                        S::Number(Some(v.volume as f32))
                    }
                    (Some(v), _) => S::Switch(Some(v.muted)),
                    (None, ent::TV_VOLUME) => S::Number(None),
                    (None, _) => S::Switch(None),
                })
            }
            ent::TV_INPUT => self.live(class, Control::Input, |s| {
                S::Select(s.select_state(&s.listed_inputs, |app| {
                    s.inputs
                        .iter()
                        .find(|i| i.app_id.as_deref() == Some(app))
                        .map(|i| i.label.as_str())
                }))
            }),
            ent::TV_APP => self.live(class, Control::Apps, |s| {
                S::Select(s.select_state(&s.listed_apps, |app| {
                    s.apps
                        .iter()
                        .find(|a| a.id == app)
                        .map(|a| a.title.as_str())
                }))
            }),
            ent::TV_SOUND_OUTPUT => match class {
                // Not read in standby (design section 7): keep the last value.
                Pwr::Transition | Pwr::Standby => Want::Hold,
                _ => {
                    if self.state_known(Control::Volume) || self.state_known(Control::Mute) {
                        Want::Set(S::Text(
                            self.vol.as_ref().and_then(|v| v.sound_output.clone()),
                        ))
                    } else {
                        Want::Set(S::Text(None))
                    }
                }
            },
            ent::TV_PLAY_STATE => match class {
                Pwr::Transition => Want::Hold,
                Pwr::Lit if self.state_known(Control::PlayState) => {
                    Want::Set(S::Text(self.play.clone()))
                }
                _ => Want::Set(S::Text(None)),
            },
            ent::LG_ALWAYS_READY | ent::LG_HANDS_FREE => {
                let label = if id == ent::LG_ALWAYS_READY {
                    self.always_ready.as_ref().map(luna::AlwaysReady::label)
                } else {
                    self.hands_free.map(luna::HandsFree::label)
                };
                match class {
                    Pwr::Transition => Want::Hold,
                    // The setting does not change with the power state and
                    // is only read while lit or dark: the last value stays
                    // in standby; a failed read leaves it missing.
                    _ => Want::Set(S::Text(label.map(str::to_owned))),
                }
            }
            ent::TV_MEDIA_PLAYER => self.media_want(class),
            _ => Want::Hold,
        }
    }

    /// The TV media player (ADR 0007): on or playing while lit, off in
    /// `Screen Off` and `Active Standby` (the TV looks off; in `Screen Off`
    /// an app's sound may still play); the volume as a share of `maxVolume`
    /// and the mute where they are live. A transition or an unknown power
    /// state is held: the media player has no "unknown" once a state was
    /// sent.
    fn media_want(&self, class: Pwr) -> Want {
        let power = match class {
            Pwr::Lit => {
                let playing = self.getter_ok.contains(&Topic::PlayState)
                    && self.play.as_deref() == Some("playing");
                if playing {
                    ent::MediaPower::Playing
                } else {
                    ent::MediaPower::On
                }
            }
            Pwr::Dark | Pwr::Standby => ent::MediaPower::Off,
            Pwr::Transition | Pwr::Unknown | Pwr::Other => return Want::Hold,
        };
        let vol = self
            .vol
            .as_ref()
            .filter(|_| matches!(class, Pwr::Lit | Pwr::Dark));
        let volume = vol
            .filter(|_| self.state_known(Control::Volume))
            .and_then(|v| volume_share(v.volume, v.max_volume));
        let muted = vol
            .filter(|_| self.state_known(Control::Mute))
            .is_some_and(|v| v.muted);
        Want::Set(StateValue::Media(ent::MediaState {
            power,
            volume,
            muted,
        }))
    }

    /// A control whose state is live in `Active`, `Screen Saver` and
    /// `Screen Off`: missing in the other states, held during a transition.
    fn live(&self, class: Pwr, control: Control, f: impl Fn(&Self) -> StateValue) -> Want {
        let none = |v: &StateValue| match v {
            StateValue::Switch(_) => StateValue::Switch(None),
            StateValue::Number(_) => StateValue::Number(None),
            StateValue::Select(_) => StateValue::Select(None),
            StateValue::Text(_) => StateValue::Text(None),
            StateValue::Media(m) => StateValue::Media(*m),
        };
        match class {
            Pwr::Transition => Want::Hold,
            Pwr::Lit | Pwr::Dark if self.state_known(control) => Want::Set(f(self)),
            _ => Want::Set(none(&f(self))),
        }
    }

    /// The select option for the foreground app: the entry `find` returns for
    /// it if HA was told that option, else `Other`; none while the foreground
    /// app is unknown.
    fn select_state<'a>(
        &'a self,
        listed: &[String],
        find: impl Fn(&str) -> Option<&'a str>,
    ) -> Option<String> {
        match &self.fg {
            Foreground::Unknown => None,
            Foreground::NoApp => Some(OTHER.to_owned()),
            Foreground::App(app) => Some(
                find(app)
                    .filter(|label| listed.iter().any(|l| l == label))
                    .map_or_else(|| OTHER.to_owned(), str::to_owned),
            ),
        }
    }

    fn all_ids(&self) -> impl Iterator<Item = &'static str> {
        [
            ent::TV_SCREEN,
            ent::TV_POWER,
            ent::TV_VOLUME,
            ent::TV_MUTE,
            ent::TV_INPUT,
            ent::TV_APP,
            ent::TV_SOUND_OUTPUT,
            ent::TV_PLAY_STATE,
            ent::LG_ALWAYS_READY,
            ent::LG_HANDS_FREE,
            ent::TV_MEDIA_PLAYER,
        ]
        .into_iter()
        .filter(|id| match *id {
            ent::TV_MEDIA_PLAYER => self.media_listed(),
            ent::TV_SCREEN => self.cfg.screen,
            ent::TV_POWER => self.cfg.power_off,
            ent::TV_VOLUME => self.cfg.volume,
            ent::TV_MUTE => self.cfg.mute,
            ent::TV_INPUT => self.cfg.input,
            ent::TV_APP => self.cfg.apps,
            ent::TV_SOUND_OUTPUT => self.cfg.volume || self.cfg.mute,
            ent::TV_PLAY_STATE => self.cfg.play_state,
            _ => self.cfg.lg_settings,
        })
    }

    /// Emits the states that differ from what HA has. Nothing before the
    /// startup check finished and while a resume re-read is running.
    fn sync(&mut self) -> Vec<Effect> {
        if !self.ready || !self.resume_pending.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let ids: Vec<&'static str> = self.all_ids().collect();
        for id in ids {
            if let Want::Set(value) = self.want(id) {
                if self.sent.get(id) != Some(&value) {
                    self.sent.insert(id, value.clone());
                    out.push(Effect::State(StateUpdate { id, value }));
                }
            }
        }
        out
    }

    /// The state HA was last sent for an entity (for tests and the daemon's
    /// diagnostics).
    #[must_use]
    pub fn sent_state(&self, id: &str) -> Option<&StateValue> {
        self.sent.get(id)
    }

    fn resend(&mut self, ids: &[&'static str]) -> Vec<Effect> {
        for id in ids {
            self.sent.remove(id);
        }
        self.sync()
    }

    // ----- power-off preconditions --------------------------------------

    /// Preconditions that hold regardless of the current power state
    /// (design section 4.2, items 1, 2 and 4, plus the firmware guard).
    fn power_gates(&self) -> Result<(), PowerOffRefusal> {
        if !self.cfg.power_off {
            return Err(PowerOffRefusal::NotEnabled);
        }
        if let Some(why) = &self.cfg.setters_blocked {
            return Err(PowerOffRefusal::SettersBlocked(why.clone()));
        }
        if !self.env.keep_listening {
            return Err(PowerOffRefusal::NotKeepListening);
        }
        if self.power.is_none() {
            return Err(PowerOffRefusal::PowerStateUnknown);
        }
        if self.env.kill_switch {
            return Err(PowerOffRefusal::KillSwitch);
        }
        if let Err(why) = &self.env.darkener {
            return Err(PowerOffRefusal::Darkener(why.clone()));
        }
        Ok(())
    }

    /// All of the design's power-off preconditions (section 4.2): enabled in
    /// the config, the always-on select on "Keep listening", the power state
    /// settled `Active` or `Screen Saver`, the kill switch absent and the
    /// darkener willing (cooldown and hourly budget). Plus the firmware
    /// guard. The key is a toggle, so it is never sent otherwise.
    ///
    /// # Errors
    ///
    /// The first precondition that fails.
    pub fn power_off_check(&self) -> Result<(), PowerOffRefusal> {
        self.power_gates()?;
        match self.class() {
            Pwr::Lit => Ok(()),
            Pwr::Transition => Err(PowerOffRefusal::Transition),
            Pwr::Unknown => Err(PowerOffRefusal::PowerStateUnknown),
            _ => Err(PowerOffRefusal::WrongState),
        }
    }

    // ----- startup, resume, power ------------------------------------------

    fn read_call(&mut self, topic: Topic) -> Effect {
        self.reads_inflight.insert(topic);
        Effect::Call {
            purpose: Purpose::Read(topic),
            uri: topic.uri(),
            payload: topic.payload().to_owned(),
        }
    }

    /// Starts the capability check: one read-only call per wanted getter
    /// (design section 6). Feed the replies to [`Controller::on_reply`], then
    /// call [`Controller::finish_probe`].
    pub fn start(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        for topic in Self::TOPICS {
            if self.wanted(topic) {
                self.probe_pending.insert(topic);
                out.push(self.read_call(topic));
            }
        }
        out
    }

    /// Every probe call answered.
    #[must_use]
    pub fn probe_complete(&self) -> bool {
        self.probe_pending.is_empty()
    }

    /// Ends the capability check (also when the time budget ran out: the
    /// unanswered getters count as failed) and returns the first states and
    /// the subscriptions to start.
    #[allow(clippy::too_many_lines)] // one linear sequence
    pub fn finish_probe(&mut self, now: Instant) -> Vec<Effect> {
        let mut out = Vec::new();
        let timed_out: Vec<Topic> = Self::TOPICS
            .into_iter()
            .filter(|t| self.probe_pending.contains(t))
            .collect();
        for t in timed_out {
            self.reads_inflight.remove(&t);
            out.extend(self.read_failed(t, &ReplyError::Transport("no answer in time".to_owned())));
        }
        self.probe_pending.clear();
        self.rebuild_lists();
        self.listed_inputs = lists::options(self.inputs.iter().map(|i| i.label.as_str()));
        self.listed_apps = lists::options(self.apps.iter().map(|a| a.title.as_str()));
        self.pending_options.clear();
        self.ready = true;

        let mut caps = Vec::new();
        for (control, label) in [
            (Control::Volume, "volume"),
            (Control::Mute, "mute"),
            (Control::Input, "input"),
            (Control::Apps, "apps"),
            (Control::PlayState, "play_state"),
        ] {
            let on = match control {
                Control::Volume => self.cfg.volume,
                Control::Mute => self.cfg.mute,
                Control::Input => self.cfg.input,
                Control::Apps => self.cfg.apps,
                _ => self.cfg.play_state,
            };
            if on {
                caps.push(format!(
                    "{label}={}",
                    if self.getter_ready(control) {
                        "ok"
                    } else {
                        "no state"
                    }
                ));
            }
        }
        if self.cfg.lg_settings {
            for topic in Self::LG_TOPICS {
                caps.push(format!(
                    "{}={}",
                    if topic == Topic::AlwaysReady {
                        "lg_always_ready"
                    } else {
                        "lg_hands_free"
                    },
                    if self.getter_ok.contains(&topic) {
                        "ok"
                    } else {
                        "no state"
                    }
                ));
            }
        }
        out.push(log(
            Level::Info,
            format!(
                "tv_control: ready (power state {}; {}){}",
                self.power_label(),
                if caps.is_empty() {
                    "no readers".to_owned()
                } else {
                    caps.join(" ")
                },
                self.cfg
                    .setters_blocked
                    .as_ref()
                    .map_or_else(String::new, |w| format!("; setters off: {w}"))
            ),
        ));
        for (control, setters_for) in [
            (
                Control::Volume,
                &[Setter::VolumeSet, Setter::VolumeUp, Setter::VolumeDown][..],
            ),
            (Control::Mute, &[Setter::Mute][..]),
            (Control::Input, &[Setter::Input][..]),
            (Control::Apps, &[Setter::Launch][..]),
        ] {
            if !self.control_enabled(control) {
                continue;
            }
            for s in setters_for {
                match self.declared.get(s).copied().unwrap_or(Declared::No) {
                    Declared::Yes => {}
                    Declared::No => out.push(log(
                        Level::Warn,
                        format!(
                            "tv_control: {} is not declared by the TV: {} is read-only",
                            s.name(),
                            control.name()
                        ),
                    )),
                    Declared::Unchecked => out.push(log(
                        Level::Warn,
                        format!(
                            "tv_control: cannot check that the TV declares {} (no declaration file readable); the read-back after each write is the guard",
                            s.name()
                        ),
                    )),
                }
            }
        }
        for topic in [Topic::Volume, Topic::Foreground] {
            if self.wanted(topic) && self.getter_ok.contains(&topic) {
                self.polling.insert(topic);
                out.push(Effect::Subscribe(topic));
            }
        }
        if self.play_reader() && self.getter_ok.contains(&Topic::PlayState) {
            self.polling.insert(Topic::PlayState);
        }
        // The two LG settings are polled whatever the startup read said: a
        // read in standby may fail and the next one recovers (read-only,
        // one warn line at most).
        for topic in Self::LG_TOPICS {
            if self.wanted(topic) {
                self.polling.insert(topic);
            }
        }
        for topic in &self.polling {
            self.next_poll
                .insert(*topic, now + Self::poll_every(*topic));
        }
        out.extend(self.sync());
        out
    }

    fn poll_every(topic: Topic) -> Duration {
        match topic {
            Topic::Foreground => POLL_FOREGROUND,
            Topic::AlwaysReady | Topic::HandsFree => POLL_LG_SETTINGS,
            _ => POLL_SLOW,
        }
    }

    fn control_enabled(&self, control: Control) -> bool {
        match control {
            Control::Volume => self.cfg.volume,
            Control::Mute => self.cfg.mute,
            Control::Input => self.cfg.input,
            Control::Apps => self.cfg.apps,
            Control::PlayState => self.cfg.play_state,
            Control::Screen => self.cfg.screen,
            Control::Power => self.cfg.power_on || self.cfg.power_off,
        }
    }

    fn power_label(&self) -> &'static str {
        match self.power {
            None => "unknown",
            Some(PowerState::Active) => "Active",
            Some(PowerState::ScreenSaver) => "Screen Saver",
            Some(PowerState::ScreenOff) => "Screen Off",
            Some(PowerState::ActiveStandby) => "Active Standby",
            Some(PowerState::Suspend) => "Suspend",
            Some(PowerState::Other) => "other",
        }
    }

    /// A power-state update from the subscription: the state name without
    /// spaces (empty if the update carries none) and whether a transition is
    /// in progress.
    pub fn on_power(&mut self, state_key: &str, processing: bool, now: Instant) -> Vec<Effect> {
        let before = self.class();
        self.transition = processing;
        if !processing && !state_key.is_empty() {
            let s = PowerState::from_key(state_key);
            if self.power != Some(s) {
                self.settled_at = Some(now);
            }
            self.power = Some(s);
        } else if self.power.is_none() && !state_key.is_empty() {
            self.power = Some(PowerState::from_key(state_key));
            self.settled_at = Some(now);
        }
        let after = self.class();
        let mut out = Vec::new();
        // Back from standby (or first known): values may have changed, read now.
        if self.ready
            && matches!(after, Pwr::Lit | Pwr::Dark)
            && matches!(before, Pwr::Standby | Pwr::Other | Pwr::Unknown)
        {
            for t in [
                Topic::Volume,
                Topic::Foreground,
                Topic::PlayState,
                Topic::AlwaysReady,
                Topic::HandsFree,
            ] {
                if self.polling.contains(&t) && !self.reads_inflight.contains(&t) {
                    out.push(self.read_call(t));
                }
            }
        }
        // Lit again after Screen Off: the play state (polled only while lit)
        // may be stale; the media player shows it.
        if self.ready
            && after == Pwr::Lit
            && before == Pwr::Dark
            && self.polling.contains(&Topic::PlayState)
            && !self.reads_inflight.contains(&Topic::PlayState)
        {
            out.push(self.read_call(Topic::PlayState));
        }
        out.extend(self.sync());
        out
    }

    /// Reads the two LG settings now, outside the 60 s poll: a person or
    /// `ctl.sh set` changed one and HA should show it within a second (the
    /// daemon calls this when the `lg-settings-refresh` trigger file is
    /// consumed). Only where the poll would read (`Active`, `Screen Saver`,
    /// `Screen Off`); a read that is already running is followed by another
    /// one on the next tick, so a reply that predates the change does not
    /// stay. No effect without `lg_settings`.
    pub fn refresh_lg_settings(&mut self, now: Instant) -> Vec<Effect> {
        if !self.ready || !self.cfg.lg_settings || !matches!(self.class(), Pwr::Lit | Pwr::Dark) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for topic in Self::LG_TOPICS {
            if self.reads_inflight.contains(&topic) {
                self.next_poll.insert(topic, now);
            } else {
                self.next_poll.insert(topic, now + POLL_LG_SETTINGS);
                out.push(self.read_call(topic));
            }
        }
        out
    }

    /// The system resumed from a suspend: every getter is read again before
    /// any state is sent (design section 3.4), and the lists are re-read.
    pub fn on_resume(&mut self, _now: Instant) -> Vec<Effect> {
        if !self.ready {
            return Vec::new();
        }
        self.check = None;
        self.pending_volume = None;
        self.sent.clear();
        let mut out = vec![log(
            Level::Info,
            "tv_control: resumed; reading the TV again",
        )];
        for topic in Self::TOPICS {
            if self.wanted(topic) {
                self.resume_pending.insert(topic);
                out.push(self.read_call(topic));
            }
        }
        out
    }

    // ----- replies --------------------------------------------------------

    /// The result of a luna call: its output, or why it did not run.
    pub fn on_reply(
        &mut self,
        purpose: Purpose,
        result: Result<String, String>,
        now: Instant,
    ) -> Vec<Effect> {
        let parsed = result
            .map_err(ReplyError::Transport)
            .and_then(|t| luna::parse_reply(&t));
        match purpose {
            Purpose::Read(topic) => self.on_read(topic, parsed, now),
            Purpose::Set(action) => self.on_set(&action, parsed.map(|_| ()), now),
        }
    }

    fn on_read(
        &mut self,
        topic: Topic,
        parsed: Result<Value, ReplyError>,
        now: Instant,
    ) -> Vec<Effect> {
        self.reads_inflight.remove(&topic);
        self.probe_pending.remove(&topic);
        self.resume_pending.remove(&topic);
        let mut out = Vec::new();
        let mut failed = None;
        let before = self
            .unconfirmed
            .contains(&topic)
            .then(|| self.fingerprint(topic));
        match parsed.and_then(|v| self.apply_read(topic, &v, now)) {
            Ok(fx) => {
                if before.is_some_and(|b| b != self.fingerprint(topic)) {
                    // Polling saw a change that no pushed line announced.
                    self.suspect.entry(topic).or_insert(now);
                }
                self.getter_ok.insert(topic);
                if self.warned_read.remove(&topic) {
                    out.push(log(
                        Level::Info,
                        format!("tv_control: the {} answers again", topic.name()),
                    ));
                }
                out.extend(fx);
            }
            Err(e) => {
                out.extend(self.read_failed(topic, &e));
                failed = Some(e.to_string());
            }
        }
        out.extend(self.refresh_progress(topic, failed));
        out.extend(self.evaluate_check(topic, now));
        out.extend(self.sync());
        out
    }

    /// One summary line when the last list read of a refresh button press
    /// is in: how many inputs and apps, or what failed. Counts only (the
    /// titles are not logged).
    fn refresh_progress(&mut self, topic: Topic, failed: Option<String>) -> Option<Effect> {
        let at = self.refresh_pending.iter().position(|t| *t == topic)?;
        self.refresh_pending.remove(at);
        if let Some(why) = failed {
            self.refresh_failed.push(format!("{}: {why}", topic.name()));
        }
        if !self.refresh_pending.is_empty() {
            return None;
        }
        let failures = std::mem::take(&mut self.refresh_failed);
        Some(if failures.is_empty() {
            log(
                Level::Info,
                format!(
                    "tv_refresh_lists: done: {} inputs, {} apps",
                    self.inputs.len(),
                    self.apps.len()
                ),
            )
        } else {
            log(
                Level::Warn,
                format!(
                    "tv_refresh_lists: failed to read ({}); keeping the old lists: {} inputs, {} apps",
                    failures.join("; "),
                    self.inputs.len(),
                    self.apps.len()
                ),
            )
        })
    }

    /// What a subscription of `topic` would have to keep current.
    fn fingerprint(&self, topic: Topic) -> String {
        match topic {
            Topic::Volume => format!("{:?}", self.vol),
            Topic::Foreground => format!("{:?}", self.fg),
            _ => String::new(),
        }
    }

    fn read_failed(&mut self, topic: Topic, e: &ReplyError) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.warned_read.insert(topic) {
            out.push(log(
                Level::Warn,
                format!(
                    "tv_control: cannot read the {}: {e}; its entity shows no state",
                    topic.name()
                ),
            ));
        }
        match topic {
            Topic::Volume => self.vol = None,
            Topic::Foreground => self.fg = Foreground::Unknown,
            Topic::PlayState => self.play = None,
            Topic::AlwaysReady => self.always_ready = None,
            Topic::HandsFree => self.hands_free = None,
            _ => {}
        }
        self.getter_ok.remove(&topic);
        out
    }

    /// Stores a getter reply. Returns effects that come with it.
    fn apply_read(
        &mut self,
        topic: Topic,
        v: &Value,
        now: Instant,
    ) -> Result<Vec<Effect>, ReplyError> {
        match topic {
            Topic::Power => {
                let p = luna::power_state(v)?;
                let key: String = p.state.chars().filter(|c| !c.is_whitespace()).collect();
                return Ok(self.on_power(&key, p.processing, now));
            }
            Topic::Volume => self.vol = Some(luna::volume(v)?),
            Topic::Foreground => {
                self.fg = luna::foreground_app(v)?.map_or(Foreground::NoApp, Foreground::App);
            }
            Topic::PlayState => self.play = luna::play_state(v),
            Topic::AlwaysReady => {
                let now_value = luna::always_ready(v)?;
                let before = self.always_ready.replace(now_value.clone());
                return Ok(Self::lg_change("always-ready", before, &now_value));
            }
            Topic::HandsFree => {
                let now_value = luna::hands_free(v)?;
                let before = self.hands_free.replace(now_value);
                return Ok(Self::lg_change("hands-free", before, &now_value));
            }
            Topic::Inputs => {
                self.raw_inputs = luna::external_inputs(v)?;
                return Ok(self.lists_changed());
            }
            Topic::Apps => {
                self.raw_points = luna::launch_points(v)?;
                return Ok(self.lists_changed());
            }
        }
        Ok(Vec::new())
    }

    /// The log line for a change of an LG setting seen by a read (never made
    /// by the satellite, ADR 0008 decision 3); none for the first value.
    fn lg_change<T: LgValue>(what: &str, before: Option<T>, now: &T) -> Vec<Effect> {
        match before {
            Some(b) if b != *now => vec![log(
                Level::Info,
                format!(
                    "lg setting: {what} {} -> {} (not changed by the satellite)",
                    b.describe(),
                    now.describe()
                ),
            )],
            _ => Vec::new(),
        }
    }

    fn rebuild_lists(&mut self) {
        self.inputs = lists::build_inputs(&self.raw_inputs);
        let input_apps: Vec<String> = self
            .raw_inputs
            .iter()
            .filter_map(|i| i.app_id.clone())
            .collect();
        self.apps = lists::build_apps(
            &self.raw_points,
            &self.cfg.own_app_ids,
            &input_apps,
            self.cfg.app_limit,
        );
    }

    /// After a list read: rebuild, and queue new select options if the lists
    /// HA knows differ.
    fn lists_changed(&mut self) -> Vec<Effect> {
        self.rebuild_lists();
        if !self.ready {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (id, listed, now_opts) in [
            (
                ent::TV_INPUT,
                &self.listed_inputs,
                lists::options(self.inputs.iter().map(|i| i.label.as_str())),
            ),
            (
                ent::TV_APP,
                &self.listed_apps,
                lists::options(self.apps.iter().map(|a| a.title.as_str())),
            ),
        ] {
            let enabled = if id == ent::TV_INPUT {
                self.cfg.input
            } else {
                self.cfg.apps
            };
            if !enabled {
                continue;
            }
            let pending = self.pending_options.get(id);
            let differs = match pending {
                Some(p) => *p != now_opts,
                None => *listed != now_opts,
            };
            if differs {
                if *listed == now_opts {
                    self.pending_options.remove(id);
                } else {
                    out.push(log(
                        Level::Info,
                        format!(
                            "tv_control: the {} list changed ({} entries); Home Assistant lists the entities again when no conversation is running",
                            if id == ent::TV_INPUT { "input" } else { "app" },
                            now_opts.len() - 1
                        ),
                    ));
                    self.pending_options.insert(id, now_opts);
                }
            }
        }
        out
    }

    /// Select options the daemon should apply to HA (when no conversation is
    /// running), then confirm with [`Controller::options_applied`].
    #[must_use]
    pub fn pending_options(&self) -> Vec<(&'static str, Vec<String>)> {
        let mut v: Vec<_> = self
            .pending_options
            .iter()
            .map(|(id, o)| (*id, o.clone()))
            .collect();
        v.sort_by_key(|(id, _)| *id);
        v
    }

    /// The daemon applied the pending options of `id`: HA lists them now.
    pub fn options_applied(&mut self, id: &'static str) -> Vec<Effect> {
        let Some(opts) = self.pending_options.remove(id) else {
            return Vec::new();
        };
        if id == ent::TV_INPUT {
            self.listed_inputs = opts;
        } else {
            self.listed_apps = opts;
        }
        self.resend(&[id])
    }

    // ----- subscriptions ----------------------------------------------------

    /// One line of a subscription (`getVolume` or the foreground app).
    pub fn on_subscription_line(&mut self, topic: Topic, line: &str, now: Instant) -> Vec<Effect> {
        let first = self.first_line_seen.insert(topic);
        let parsed = luna::parse_reply(line);
        let mut out = Vec::new();
        match parsed {
            Ok(v) => {
                if first {
                    if luna::is_subscribed(&v) {
                        self.subscribed.insert(topic);
                        self.polling.remove(&topic);
                        out.push(log(
                            Level::Info,
                            format!("tv_control: subscribed to the {}", topic.name()),
                        ));
                    } else {
                        // No `subscribed` flag (the volume getter on c3-a and
                        // c3-b). EXP-010 A3 saw pushes on such a subscription,
                        // so keep it, and poll until a second line shows
                        // that it streams.
                        let every = Self::poll_every(topic);
                        self.polling.insert(topic);
                        self.next_poll.insert(topic, now + every);
                        self.unconfirmed.insert(topic);
                        out.push(log(
                            Level::Info,
                            format!(
                                "tv_control: the {} reply has no subscribed flag; keeping the subscription and polling every {} s until an update is pushed",
                                topic.name(),
                                every.as_secs()
                            ),
                        ));
                    }
                } else if self.unconfirmed.remove(&topic) {
                    self.suspect.remove(&topic);
                    self.subscribed.insert(topic);
                    self.polling.remove(&topic);
                    out.push(log(
                        Level::Info,
                        format!(
                            "tv_control: an update was pushed on the {} subscription; it streams without the flag, polling stopped",
                            topic.name()
                        ),
                    ));
                }
                match self.apply_read(topic, &v, now) {
                    Ok(fx) => out.extend(fx),
                    Err(e) => out.push(log(
                        Level::Debug,
                        format!("tv_control: {} update ignored: {e}", topic.name()),
                    )),
                }
            }
            Err(e) => {
                out.push(log(
                    Level::Debug,
                    format!("tv_control: {} line ignored: {e}", topic.name()),
                ));
                if first && matches!(e, ReplyError::Luna(_)) {
                    self.polling.insert(topic);
                    self.next_poll.insert(topic, now + Self::poll_every(topic));
                    out.push(Effect::Unsubscribe(topic));
                }
            }
        }
        out.extend(self.sync());
        out
    }

    // ----- commands ---------------------------------------------------------

    fn refuse(&mut self, id: &'static str, why: &str) -> Vec<Effect> {
        let mut out = vec![log(Level::Info, format!("{id}: refused: {why}"))];
        out.extend(self.resend(&[id]));
        out
    }

    fn noop(&mut self, id: &'static str, why: &str) -> Vec<Effect> {
        let mut out = vec![log(Level::Debug, format!("{id}: nothing to do: {why}"))];
        out.extend(self.resend(&[id]));
        out
    }

    /// Checks a setter may be used: not blocked by the firmware guard, the
    /// control not disabled, and the method declared.
    fn setter_allowed(&self, control: Control, setter: Option<Setter>) -> Result<(), String> {
        if let Some(why) = &self.cfg.setters_blocked {
            return Err(format!("TV control setters are off ({why})"));
        }
        if let Some(why) = self.disabled.get(&control) {
            return Err(format!("{} is disabled ({why})", control.name()));
        }
        if let Some(s) = setter {
            if self.declared.get(&s).copied().unwrap_or(Declared::No) == Declared::No {
                return Err(format!("the TV does not declare {}", s.name()));
            }
        }
        if !self.getter_ready(control) {
            return Err(format!("the TV's {} cannot be read", control.name()));
        }
        Ok(())
    }

    fn call(action: Action, uri: &'static str, payload: String) -> Effect {
        Effect::Call {
            purpose: Purpose::Set(action),
            uri,
            payload,
        }
    }

    /// An HA command.
    pub fn on_command(&mut self, cmd: Command, now: Instant) -> Vec<Effect> {
        let mut out = match cmd {
            Command::Switch { id, state } => match id.as_str() {
                ent::TV_SCREEN if self.cfg.screen => self.cmd_screen(state, now),
                ent::TV_POWER if self.cfg.power_off => self.cmd_power(state, now),
                ent::TV_MUTE if self.cfg.mute => self.cmd_mute(state, now),
                _ => vec![log(
                    Level::Debug,
                    "tv_control: switch command for an unknown entity ignored",
                )],
            },
            Command::Number { id, value } => match id.as_str() {
                ent::TV_VOLUME if self.cfg.volume => self.cmd_volume_set(value, now),
                _ => vec![log(
                    Level::Debug,
                    "tv_control: number command for an unknown entity ignored",
                )],
            },
            Command::Select { id, option } => match id.as_str() {
                ent::TV_INPUT if self.cfg.input => self.cmd_input(&option, now),
                ent::TV_APP if self.cfg.apps => self.cmd_app(&option, now),
                _ => vec![log(
                    Level::Debug,
                    "tv_control: select command for an unknown entity ignored",
                )],
            },
            Command::Button { id } => match id.as_str() {
                ent::TV_TURN_ON if self.cfg.power_on => self.cmd_turn_on(now),
                ent::TV_VOLUME_UP if self.cfg.volume => self.cmd_volume_step(true, now),
                ent::TV_VOLUME_DOWN if self.cfg.volume => self.cmd_volume_step(false, now),
                ent::TV_REFRESH_LISTS if self.cfg.input || self.cfg.apps => self.cmd_refresh(now),
                _ => vec![log(
                    Level::Debug,
                    "tv_control: button for an unknown entity ignored",
                )],
            },
            Command::Media { id, action } => match id.as_str() {
                ent::TV_MEDIA_PLAYER if self.media_listed() => self.cmd_media(action, now),
                _ => vec![log(
                    Level::Debug,
                    "tv_control: media player command for an unknown entity ignored",
                )],
            },
        };
        out.extend(self.sync());
        out
    }

    /// A TV media player command (ADR 0007): the same paths, limits,
    /// read-backs and refusals as the separate entities. Volume set maps the
    /// share to `maxVolume` steps (rounded; `volume_limit` clamps as for the
    /// number); volume up and down are one TV step each (as the remote and
    /// the step buttons); turn on is the `tv_turn_on` button; turn off is the
    /// opt-in power-off (`power_off` enabled and the always-on mode
    /// selected, with all of its preconditions) or else the screen off.
    fn cmd_media(&mut self, action: MediaAction, now: Instant) -> Vec<Effect> {
        let id = ent::TV_MEDIA_PLAYER;
        let mut out = vec![log(Level::Info, format!("{id}: {}", action.name()))];
        let f = self.media_features();
        out.extend(match action {
            MediaAction::SetVolume(share) if f.volume_set => {
                if !share.is_finite() {
                    self.refuse(id, "not a volume")
                } else if let Some(max) = self.vol.as_ref().map(|v| v.max_volume) {
                    self.cmd_volume_set(share_to_steps(share, max), now)
                } else {
                    self.refuse(id, "the TV volume is not known")
                }
            }
            MediaAction::VolumeUp if f.volume_step => self.cmd_volume_step(true, now),
            MediaAction::VolumeDown if f.volume_step => self.cmd_volume_step(false, now),
            MediaAction::Mute(m) if f.mute => self.cmd_mute(m, now),
            MediaAction::TurnOn if f.turn_on => self.cmd_turn_on(now),
            MediaAction::TurnOff if f.turn_off => match self.class() {
                Pwr::Dark | Pwr::Standby => self.noop(id, "the TV is already off"),
                _ if self.cfg.power_off && self.env.keep_listening => self.cmd_power(false, now),
                _ if self.cfg.screen => self.cmd_screen(false, now),
                _ => self.refuse(
                    id,
                    "turn off needs the always-on mode for the power-off, or the screen control",
                ),
            },
            _ => self.refuse(id, "not enabled in [tv_control]"),
        });
        out
    }

    fn cmd_screen(&mut self, on: bool, now: Instant) -> Vec<Effect> {
        let id = ent::TV_SCREEN;
        match (self.class(), on) {
            (Pwr::Lit, false) => {
                match self.screen_call(Action::ScreenOff, setters::URI_SCREEN_OFF, now) {
                    Ok(fx) => fx,
                    Err(why) => self.refuse(id, &why),
                }
            }
            (Pwr::Dark, true) => self.wake_screen(id, now),
            (Pwr::Lit, true) => self.noop(id, "the screen is on"),
            (Pwr::Dark, false) => self.noop(id, "the screen is off"),
            (Pwr::Transition, _) => self.refuse(id, "the TV is changing power state"),
            _ => self.refuse(id, "not available in this power state"),
        }
    }

    fn screen_call(
        &mut self,
        action: Action,
        uri: &'static str,
        now: Instant,
    ) -> Result<Vec<Effect>, String> {
        self.setter_allowed(Control::Screen, None)?;
        self.limiter
            .acquire(Control::Screen, now, false, true)
            .map_err(str::to_owned)?;
        Ok(vec![
            log(Level::Info, format!("tv_screen: {}", action.name())),
            Self::call(action, uri, setters::PAYLOAD_EMPTY.to_owned()),
        ])
    }

    /// `turnOnScreen` from `Screen Off`, as the satellite's own wake.
    fn wake_screen(&mut self, id: &'static str, now: Instant) -> Vec<Effect> {
        let control = if id == ent::TV_SCREEN {
            Control::Screen
        } else {
            Control::Power
        };
        if let Err(why) = self.setter_allowed(control, None).and_then(|()| {
            self.limiter
                .acquire(control, now, false, true)
                .map_err(str::to_owned)
        }) {
            return self.refuse(id, &why);
        }
        vec![
            log(Level::Info, format!("{id}: turnOnScreen")),
            Effect::OwnWake,
            Self::call(
                Action::ScreenOn(control),
                setters::URI_SCREEN_ON,
                setters::PAYLOAD_EMPTY.to_owned(),
            ),
        ]
    }

    /// `powerOn` from `Active Standby`, after the key block of the settle.
    fn wake_standby(&mut self, id: &'static str, now: Instant) -> Vec<Effect> {
        if let Err(why) = self.setter_allowed(Control::Power, None) {
            return self.refuse(id, &why);
        }
        let at = self.settled_at.map_or(now, |s| (s + KEY_BLOCK).max(now));
        self.power_on_at = Some((at, 1));
        vec![log(Level::Info, format!("{id}: powerOn from standby"))]
    }

    fn cmd_turn_on(&mut self, now: Instant) -> Vec<Effect> {
        let id = ent::TV_TURN_ON;
        match self.class() {
            Pwr::Lit => self.noop(id, "the TV is on"),
            Pwr::Dark => self.wake_screen(id, now),
            Pwr::Standby => self.wake_standby(id, now),
            Pwr::Transition => self.refuse(id, "the TV is changing power state"),
            _ => self.refuse(id, "not available in this power state"),
        }
    }

    fn cmd_power(&mut self, on: bool, now: Instant) -> Vec<Effect> {
        let id = ent::TV_POWER;
        if let Err(why) = self.power_gates() {
            return self.refuse(id, &why.to_string());
        }
        match (self.class(), on) {
            (Pwr::Lit, false) => {
                if let Err(why) = self.power_off_check() {
                    return self.refuse(id, &why.to_string());
                }
                if let Err(why) = self.limiter.acquire(Control::Power, now, false, false) {
                    return self.refuse(id, why);
                }
                vec![
                    log(
                        Level::Info,
                        "tv_power: power-off requested (the remote's power key; the satellite wakes the TV and turns the screen off)",
                    ),
                    Self::call(
                        Action::PowerKey,
                        setters::URI_POWER_KEY,
                        setters::PAYLOAD_EMPTY.to_owned(),
                    ),
                ]
            }
            (Pwr::Dark, true) => self.wake_screen(id, now),
            (Pwr::Standby, true) => self.wake_standby(id, now),
            (Pwr::Lit, true) => self.noop(id, "the TV is on"),
            (Pwr::Dark | Pwr::Standby, false) => self.noop(id, "the TV is already off"),
            (Pwr::Transition, _) => self.refuse(id, "the TV is changing power state"),
            _ => self.refuse(id, "not available in this power state"),
        }
    }

    /// The power state allows volume, mute, input and app actions.
    fn live_power(&self) -> Result<(), &'static str> {
        match self.class() {
            Pwr::Lit | Pwr::Dark => Ok(()),
            Pwr::Transition => Err("the TV is changing power state"),
            _ => Err("not available in this power state"),
        }
    }

    /// The highest volume HA may set: the TV's `maxVolume`, or the
    /// configured cap when that is lower.
    fn volume_ceiling(&self, max_volume: u32) -> u32 {
        self.cfg
            .volume_limit
            .map_or(max_volume, |l| l.min(max_volume))
    }

    fn cmd_volume_set(&mut self, value: f32, now: Instant) -> Vec<Effect> {
        let id = ent::TV_VOLUME;
        if let Err(why) = self
            .live_power()
            .map_err(str::to_owned)
            .and_then(|()| self.setter_allowed(Control::Volume, Some(Setter::VolumeSet)))
        {
            return self.refuse(id, &why);
        }
        let Some(max) = self.vol.as_ref().map(|v| v.max_volume) else {
            return self.refuse(id, "the TV volume is not known");
        };
        if !value.is_finite() || value < 0.0 {
            return self.refuse(id, "not a volume");
        }
        let limit = self.volume_ceiling(max);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        // finite, >= 0, then capped
        let mut target = (value.round().min(1_000_000.0) as u32).min(max);
        let mut out = Vec::new();
        if target > limit {
            target = limit;
            out.push(log(
                Level::Info,
                format!("tv_volume: request above the volume ceiling; using {limit}"),
            ));
        }
        self.pending_volume = Some((target, now + VOLUME_COALESCE));
        out
    }

    fn cmd_volume_step(&mut self, up: bool, now: Instant) -> Vec<Effect> {
        let id = ent::TV_VOLUME;
        let setter = if up {
            Setter::VolumeUp
        } else {
            Setter::VolumeDown
        };
        if let Err(why) = self
            .live_power()
            .map_err(str::to_owned)
            .and_then(|()| self.setter_allowed(Control::Volume, Some(setter)))
        {
            return self.refuse(id, &why);
        }
        let Some(v) = self.vol.clone() else {
            return self.refuse(id, "the TV volume is not known");
        };
        let limit = self.volume_ceiling(v.max_volume);
        if up && v.volume >= limit {
            return self.refuse(id, "at the volume limit");
        }
        if !up && v.volume == 0 {
            return self.noop(id, "the volume is 0");
        }
        if let Err(why) = self.limiter.acquire(Control::Volume, now, true, true) {
            return self.refuse(id, why);
        }
        let (action, uri) = if up {
            (Action::VolumeUp, setters::URI_VOLUME_UP)
        } else {
            (Action::VolumeDown, setters::URI_VOLUME_DOWN)
        };
        self.check = Some(Check {
            control: Control::Volume,
            topic: Topic::Volume,
            expect: Expect::VolumeStep { from: v.volume, up },
            due: now,
            issued: false,
            reads_left: 1,
        });
        // The check becomes due when the reply arrives (on_set).
        vec![Self::call(action, uri, setters::PAYLOAD_EMPTY.to_owned())]
    }

    fn cmd_mute(&mut self, muted: bool, now: Instant) -> Vec<Effect> {
        let id = ent::TV_MUTE;
        if let Err(why) = self
            .live_power()
            .map_err(str::to_owned)
            .and_then(|()| self.setter_allowed(Control::Mute, Some(Setter::Mute)))
        {
            return self.refuse(id, &why);
        }
        if self.vol.as_ref().is_some_and(|v| v.muted == muted) {
            return self.noop(id, "already in that state");
        }
        if let Err(why) = self.limiter.acquire(Control::Mute, now, false, true) {
            return self.refuse(id, why);
        }
        self.check = Some(Check {
            control: Control::Mute,
            topic: Topic::Volume,
            expect: Expect::Mute(muted),
            due: now,
            issued: false,
            reads_left: 1,
        });
        vec![Self::call(
            Action::Mute(muted),
            setters::URI_SET_MUTE,
            setters::payload_set_mute(muted),
        )]
    }

    fn cmd_input(&mut self, option: &str, now: Instant) -> Vec<Effect> {
        let id = ent::TV_INPUT;
        if option == OTHER {
            return self.refuse(id, "\"Other\" cannot be chosen");
        }
        if let Err(why) = self
            .live_power()
            .map_err(str::to_owned)
            .and_then(|()| self.setter_allowed(Control::Input, Some(Setter::Input)))
        {
            return self.refuse(id, &why);
        }
        let Some(input) = self.inputs.iter().find(|i| i.label == option).cloned() else {
            return self.refuse(id, "not an input of this TV");
        };
        if let Err(why) = self.limiter.acquire(Control::Input, now, false, true) {
            return self.refuse(id, why);
        }
        if let Some(app) = &input.app_id {
            self.check = Some(Check {
                control: Control::Input,
                topic: Topic::Foreground,
                expect: Expect::Foreground(app.clone()),
                due: now,
                issued: false,
                reads_left: 1,
            });
        }
        vec![
            log(Level::Info, "tv_input: switchInput"),
            Self::call(
                Action::Input(input.id.clone()),
                setters::URI_SWITCH_INPUT,
                setters::payload_switch_input(&input.id),
            ),
        ]
    }

    fn cmd_app(&mut self, option: &str, now: Instant) -> Vec<Effect> {
        let id = ent::TV_APP;
        if option == OTHER {
            return self.refuse(id, "\"Other\" cannot be chosen");
        }
        if let Err(why) = self
            .live_power()
            .map_err(str::to_owned)
            .and_then(|()| self.setter_allowed(Control::Apps, Some(Setter::Launch)))
        {
            return self.refuse(id, &why);
        }
        let Some(app) = self.apps.iter().find(|a| a.title == option).cloned() else {
            return self.refuse(id, "not an app of this TV");
        };
        if let Err(why) = self.limiter.acquire(Control::Apps, now, false, true) {
            return self.refuse(id, why);
        }
        self.check = Some(Check {
            control: Control::Apps,
            topic: Topic::Foreground,
            expect: Expect::Foreground(app.id.clone()),
            due: now,
            issued: false,
            reads_left: 1,
        });
        vec![
            log(Level::Info, "tv_app: launch"),
            Self::call(
                Action::Launch(app.id.clone()),
                setters::URI_LAUNCH,
                setters::payload_launch(&app.id),
            ),
        ]
    }

    fn cmd_refresh(&mut self, now: Instant) -> Vec<Effect> {
        if self
            .last_refresh
            .is_some_and(|t| now.saturating_duration_since(t) < REFRESH_INTERVAL)
        {
            return vec![log(
                Level::Info,
                "tv_refresh_lists: refused: refreshed a moment ago",
            )];
        }
        self.last_refresh = Some(now);
        let mut out = vec![log(
            Level::Info,
            "tv_refresh_lists: reading the input and app lists",
        )];
        self.refresh_pending.clear();
        self.refresh_failed.clear();
        for t in [Topic::Inputs, Topic::Apps] {
            if self.wanted(t) {
                self.refresh_pending.push(t);
                if !self.reads_inflight.contains(&t) {
                    out.push(self.read_call(t));
                }
            }
        }
        out
    }

    // ----- setter replies -----------------------------------------------------

    /// Controls a failed write switches off (until a restart or a list
    /// refresh): volume, mute, input and apps, whose setters were checked on
    /// c3-b only. The power paths are not: their transient refusals (`busy`,
    /// `Keyblock1.5second`, "substate must be active") are known and a
    /// retry by the owner works.
    fn disabled_on_failure(control: Control) -> bool {
        matches!(
            control,
            Control::Volume | Control::Mute | Control::Input | Control::Apps
        )
    }

    fn entities_of(control: Control) -> &'static [&'static str] {
        match control {
            Control::Screen => &[ent::TV_SCREEN],
            Control::Power => &[ent::TV_POWER, ent::TV_SCREEN],
            Control::Volume => &[ent::TV_VOLUME, ent::TV_SOUND_OUTPUT],
            Control::Mute => &[ent::TV_MUTE],
            Control::Input => &[ent::TV_INPUT],
            Control::Apps => &[ent::TV_APP],
            Control::PlayState => &[ent::TV_PLAY_STATE],
        }
    }

    fn disable(&mut self, control: Control, why: &str) -> Vec<Effect> {
        self.disabled.insert(control, why.to_owned());
        let mut out = vec![log(
            Level::Warn,
            format!(
                "tv_control: {} is disabled until a restart or a list refresh: {why}",
                control.name()
            ),
        )];
        out.extend(self.resend(Self::entities_of(control)));
        out
    }

    fn on_set(
        &mut self,
        action: &Action,
        result: Result<(), ReplyError>,
        now: Instant,
    ) -> Vec<Effect> {
        let control = action.control();
        self.limiter.done(control);
        if let Err(e) = result {
            {
                if let Action::PowerOn(n) = action {
                    if e.luna_text().is_some_and(|t| t.contains("Keyblock")) && *n < POWER_ON_TRIES
                    {
                        self.power_on_at = Some((now + POWER_ON_RETRY, n + 1));
                        return vec![log(
                            Level::Info,
                            "powerOn: the key is blocked for 1.5 s; trying again",
                        )];
                    }
                }
                if self.check.as_ref().is_some_and(|c| c.control == control) {
                    self.check = None;
                }
                let mut out = vec![log(
                    Level::Warn,
                    format!("{}: {} failed: {e}", control.name(), action.name()),
                )];
                if Self::disabled_on_failure(control) {
                    out.extend(self.disable(control, &format!("{} failed", action.name())));
                } else {
                    out.extend(self.resend(Self::entities_of(control)));
                }
                out
            }
        } else {
            {
                let mut out = vec![log(
                    Level::Info,
                    format!("{}: {} accepted", control.name(), action.name()),
                )];
                if let Some(c) = self.check.as_mut().filter(|c| c.control == control) {
                    c.due = now + READ_BACK_DELAY;
                }
                if matches!(
                    action,
                    Action::PowerKey | Action::ScreenOff | Action::ScreenOn(_) | Action::PowerOn(_)
                ) {
                    // The power state subscription is the read-back.
                    self.power_on_at = None;
                }
                out.extend(self.sync());
                out
            }
        }
    }

    /// After a one-shot read of `topic`: judge a pending read-back.
    fn evaluate_check(&mut self, topic: Topic, now: Instant) -> Vec<Effect> {
        let Some(c) = self.check.clone().filter(|c| c.topic == topic && c.issued) else {
            return Vec::new();
        };
        let matched = match &c.expect {
            Expect::Volume(t) => self.vol.as_ref().is_some_and(|v| v.volume == *t),
            Expect::VolumeStep { from, up } => self.vol.as_ref().is_some_and(|v| {
                if *up {
                    v.volume > *from
                } else {
                    v.volume < *from
                }
            }),
            Expect::Mute(m) => self.vol.as_ref().is_some_and(|v| v.muted == *m),
            Expect::Foreground(app) => self.fg == Foreground::App(app.clone()),
        };
        if matched {
            self.check = None;
            return vec![log(
                Level::Info,
                format!("{}: read back matches", c.control.name()),
            )];
        }
        if c.reads_left > 0 {
            self.check = Some(Check {
                due: now + READ_BACK_RETRY,
                issued: false,
                reads_left: c.reads_left - 1,
                ..c
            });
            return Vec::new();
        }
        self.check = None;
        self.disable(
            c.control,
            "the TV does not report the change (read-back mismatch)",
        )
    }

    // ----- time ---------------------------------------------------------------

    /// Time-driven work: coalesced volume, read-backs, a deferred `powerOn`,
    /// and polling. Call it about every 250 ms.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut out = Vec::new();
        // Coalesced volume set.
        if let Some((target, due)) = self.pending_volume {
            if now >= due && !self.limiter.busy(Control::Volume) {
                self.pending_volume = None;
                out.extend(self.issue_volume(target, now));
            }
        }
        // Read-back: due once the setter replied (on_set moves `due`); a
        // check whose call is still in flight waits.
        if let Some(c) = self.check.clone() {
            if !c.issued
                && now >= c.due
                && !self.limiter.busy(c.control)
                && !self.reads_inflight.contains(&c.topic)
            {
                if let Some(c) = &mut self.check {
                    c.issued = true;
                }
                out.push(self.read_call(c.topic));
            }
        }
        // Deferred powerOn.
        if let Some((at, attempt)) = self.power_on_at {
            if now >= at && !self.limiter.busy(Control::Power) {
                self.power_on_at = None;
                if matches!(self.class(), Pwr::Standby) {
                    if let Err(why) = self.limiter.acquire(Control::Power, now, false, true) {
                        out.push(log(Level::Info, format!("tv_turn_on: refused: {why}")));
                    } else {
                        out.push(Effect::OwnWake);
                        out.push(Self::call(
                            Action::PowerOn(attempt),
                            setters::URI_POWER_ON,
                            setters::PAYLOAD_POWER_ON.to_owned(),
                        ));
                    }
                }
            }
        }
        // A subscription that pushed nothing for a change polling saw.
        let mut dropped: Vec<Topic> = self
            .suspect
            .iter()
            .filter(|(t, since)| {
                self.unconfirmed.contains(*t)
                    && now.saturating_duration_since(**since) >= PUSH_GRACE
            })
            .map(|(t, _)| *t)
            .collect();
        dropped.sort_by_key(|t| t.name());
        for topic in dropped {
            self.suspect.remove(&topic);
            self.unconfirmed.remove(&topic);
            out.push(log(
                Level::Warn,
                format!(
                    "tv_control: the {} subscription pushed nothing for a change; polling only",
                    topic.name()
                ),
            ));
            out.push(Effect::Unsubscribe(topic));
        }
        // Polling, only where the TV is in a state it answers in.
        for topic in [
            Topic::Volume,
            Topic::Foreground,
            Topic::PlayState,
            Topic::AlwaysReady,
            Topic::HandsFree,
        ] {
            if !self.polling.contains(&topic) || self.reads_inflight.contains(&topic) {
                continue;
            }
            let live = match topic {
                Topic::PlayState => matches!(self.class(), Pwr::Lit),
                _ => matches!(self.class(), Pwr::Lit | Pwr::Dark),
            };
            let due = self.next_poll.get(&topic).is_none_or(|t| now >= *t);
            if live && due {
                self.next_poll.insert(topic, now + Self::poll_every(topic));
                out.push(self.read_call(topic));
            }
        }
        out.extend(self.sync());
        out
    }

    fn issue_volume(&mut self, target: u32, now: Instant) -> Vec<Effect> {
        let id = ent::TV_VOLUME;
        if let Err(why) = self
            .live_power()
            .map_err(str::to_owned)
            .and_then(|()| self.setter_allowed(Control::Volume, Some(Setter::VolumeSet)))
        {
            return self.refuse(id, &why);
        }
        if self.vol.as_ref().is_some_and(|v| v.volume == target) {
            return self.noop(id, "already at that volume");
        }
        if let Err(why) = self.limiter.acquire(Control::Volume, now, false, true) {
            return self.refuse(id, why);
        }
        self.check = Some(Check {
            control: Control::Volume,
            topic: Topic::Volume,
            expect: Expect::Volume(target),
            due: now,
            issued: false,
            reads_left: 1,
        });
        vec![Self::call(
            Action::VolumeSet(target),
            setters::URI_SET_VOLUME,
            setters::payload_set_volume(target),
        )]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
