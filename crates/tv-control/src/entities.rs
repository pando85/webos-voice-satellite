//! The Home Assistant entities of TV control (`docs/design/tv-control.md`
//! section 3.1) as plain data. The daemon maps them onto the ESPHome API
//! crate's entity types; this crate knows nothing about ESPHome.

/// Object id of `tv_screen` (switch).
pub const TV_SCREEN: &str = "tv_screen";
/// Object id of `tv_turn_on` (button).
pub const TV_TURN_ON: &str = "tv_turn_on";
/// Object id of `tv_power` (switch).
pub const TV_POWER: &str = "tv_power";
/// Object id of `tv_volume` (number).
pub const TV_VOLUME: &str = "tv_volume";
/// Object id of `tv_volume_up` (button).
pub const TV_VOLUME_UP: &str = "tv_volume_up";
/// Object id of `tv_volume_down` (button).
pub const TV_VOLUME_DOWN: &str = "tv_volume_down";
/// Object id of `tv_mute` (switch).
pub const TV_MUTE: &str = "tv_mute";
/// Object id of `tv_input` (select).
pub const TV_INPUT: &str = "tv_input";
/// Object id of `tv_app` (select).
pub const TV_APP: &str = "tv_app";
/// Object id of `tv_refresh_lists` (button).
pub const TV_REFRESH_LISTS: &str = "tv_refresh_lists";
/// Object id of `tv_sound_output` (text sensor).
pub const TV_SOUND_OUTPUT: &str = "tv_sound_output";
/// Object id of `tv_play_state` (text sensor).
pub const TV_PLAY_STATE: &str = "tv_play_state";
/// Object id of the Always Ready sensor (text sensor, diagnostic, read-only).
pub const LG_ALWAYS_READY: &str = "lg_always_ready";
/// Object id of the LG hands-free sensor (text sensor, diagnostic, read-only).
pub const LG_HANDS_FREE: &str = "lg_hands_free";
/// Object id of `tv_media_player` (the TV as a media player, ADR 0007).
pub const TV_MEDIA_PLAYER: &str = "tv_media_player";

/// What the TV media player offers (each needs its control enabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // one bool per HA feature
pub struct MediaFeatures {
    /// Volume set (`volume`).
    pub volume_set: bool,
    /// Volume up and down (`volume`): one TV step each, as the remote.
    pub volume_step: bool,
    /// Mute and unmute (`mute`).
    pub mute: bool,
    /// Turn on (`power_on`).
    pub turn_on: bool,
    /// Turn off (`screen` or `power_off`).
    pub turn_off: bool,
}

impl MediaFeatures {
    /// Any feature at all.
    #[must_use]
    pub fn any(self) -> bool {
        self.volume_set || self.volume_step || self.mute || self.turn_on || self.turn_off
    }
}

/// The TV media player's power and playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaPower {
    /// Lit (`Active`, `Screen Saver`), the media service does not report
    /// `playing`.
    On,
    /// Lit and the media service reports `playState` `playing`.
    Playing,
    /// Looks off: `Screen Off` or `Active Standby`.
    Off,
}

/// The TV media player's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaState {
    /// Power and playback.
    pub power: MediaPower,
    /// TV volume over its `maxVolume` (0.0 to 1.0), `None` when not known.
    pub volume: Option<f32>,
    /// Muted (`false` when not known: the media player has no unknown here).
    pub muted: bool,
}

/// Home Assistant's entity category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A primary entity.
    None,
    /// Configuration.
    Config,
    /// Diagnostic.
    Diagnostic,
}

/// The kind of an entity.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// An on/off control.
    Switch,
    /// A stateless action.
    Button,
    /// A slider from 0 to `max` in steps of 1, no unit.
    Number {
        /// Highest value (the TV's `maxVolume`).
        max: f32,
    },
    /// A select, with its options (the last one is `Other`).
    Select {
        /// The options in display order.
        options: Vec<String>,
    },
    /// A read-only string.
    TextSensor,
    /// The TV as a control-only media player.
    MediaPlayer {
        /// What it offers.
        features: MediaFeatures,
    },
}

/// One entity.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityDef {
    /// Stable object id.
    pub id: &'static str,
    /// Name shown in HA after the device name.
    pub name: &'static str,
    /// Material Design icon.
    pub icon: &'static str,
    /// Entity category.
    pub category: Category,
    /// Kind and its parameters.
    pub kind: Kind,
}

/// The state of an entity, as the controller wants HA to see it. `None` means
/// "no valid state" (HA shows unknown).
#[derive(Debug, Clone, PartialEq)]
pub enum StateValue {
    /// A switch.
    Switch(Option<bool>),
    /// A number.
    Number(Option<f32>),
    /// A select option.
    Select(Option<String>),
    /// A text sensor.
    Text(Option<String>),
    /// The TV media player. It has no "unknown": until the first state is
    /// sent HA shows unknown, after that a state that cannot be told (a
    /// transition, an unknown power state) is held.
    Media(MediaState),
}

/// A state to send.
#[derive(Debug, Clone, PartialEq)]
pub struct StateUpdate {
    /// Object id of the entity.
    pub id: &'static str,
    /// The state.
    pub value: StateValue,
}
