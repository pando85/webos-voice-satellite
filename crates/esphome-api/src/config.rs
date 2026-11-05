//! Server configuration.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

use crate::DEFAULT_PORT;
use crate::pairing::Provisioning;

/// `VoiceAssistantFeature` bits (aioesphomeapi 46.6.0 `model.py`), reported in
/// `DeviceInfoResponse.voice_assistant_feature_flags`.
pub mod features {
    /// Voice assistant present (required, else HA creates no satellite).
    pub const VOICE_ASSISTANT: u32 = 1;
    /// TTS is streamed to the device as audio instead of a URL.
    pub const SPEAKER: u32 = 2;
    /// Audio travels over the API connection, not UDP (required here).
    pub const API_AUDIO: u32 = 4;
    /// Timer events: HA forwards its timers to the device, which receives them
    /// as `AppEvent::Timer`. Implemented, but not in [`DEFAULT`]: enable it with
    /// [`DEFAULT_WITH_TIMERS`] once the application handles timers, because HA
    /// would otherwise register a handler whose events nothing acts on.
    pub const TIMERS: u32 = 8;
    /// Announcements (`VoiceAssistantAnnounceRequest`).
    pub const ANNOUNCE: u32 = 16;
    /// HA may start a conversation after an announcement.
    pub const START_CONVERSATION: u32 = 32;
    /// Second, non-enhanced audio channel (not implemented; rejected).
    pub const MULTI_CHANNEL_AUDIO: u32 = 64;
    /// Bits this crate implements.
    pub const SUPPORTED: u32 =
        VOICE_ASSISTANT | SPEAKER | API_AUDIO | ANNOUNCE | TIMERS | START_CONVERSATION;
    /// Default: TTS streamed over the API (no URL fetch, which matters when
    /// HA's URLs are not reachable from the TV) plus announcements.
    pub const DEFAULT: u32 = VOICE_ASSISTANT | SPEAKER | API_AUDIO | ANNOUNCE;
    /// [`DEFAULT`] plus [`TIMERS`] (= 31): set `Config::voice_features` to this
    /// to receive timer events.
    pub const DEFAULT_WITH_TIMERS: u32 = DEFAULT | TIMERS;
}

/// Audio format announced to Home Assistant on the media player entity.
///
/// HA 2026.10.4 uses the first `ANNOUNCEMENT` format of any media player of the
/// device as the preferred TTS format (and for proxied announcement media).
/// Because of a bug in that release, `num_channels` and `sample_bytes` are only
/// applied when `sample_rate > 0`, so `sample_rate` must always be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormat {
    /// Container/codec name as HA/ffmpeg understands it: `flac`, `wav` or `mp3`.
    pub format: String,
    /// Sample rate in Hz (must be non-zero).
    pub sample_rate: u32,
    /// Channel count.
    pub num_channels: u32,
    /// Bytes per sample.
    pub sample_bytes: u32,
}

impl Default for AudioFormat {
    /// FLAC, 22050 Hz, mono, 16-bit: Piper's native rate (no resampling in HA),
    /// streamable (unlike WAV from a pipe, whose header has no length) and
    /// decodable by symphonia.
    fn default() -> Self {
        Self {
            format: "flac".to_owned(),
            sample_rate: 22_050,
            num_channels: 1,
            sample_bytes: 2,
        }
    }
}

/// A configuration `select` entity (for example a behaviour setting). The
/// application sets its state ([`crate::AppCommand::SetSelect`]) and is told
/// of HA's changes ([`crate::AppEvent::SelectChanged`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectEntity {
    /// Object id (the entity key is derived from it).
    pub object_id: String,
    /// Name shown in HA.
    pub name: String,
    /// Material Design icon, e.g. `mdi:power`.
    pub icon: String,
    /// The options, in display order. HA can only choose one of these.
    pub options: Vec<String>,
    /// None (primary), configuration or diagnostic. The behaviour selects
    /// use [`crate::proto::EntityCategory::Config`].
    pub entity_category: crate::proto::EntityCategory,
    /// `true`: a valid command is stored and confirmed to HA at once (a
    /// setting). `false`: the application owns the state, as for a
    /// [`NumberEntity`]: a valid command is reported as
    /// [`crate::AppEvent::SelectChanged`] and **not** echoed, HA shows the
    /// state the application reports ([`crate::AppCommand::SetSelect`],
    /// [`crate::AppCommand::ClearSelect`]), and the option is not written to
    /// the log at info (it may be an app title or an input label).
    pub command_echo: bool,
}

/// A `number` entity: a bounded float setting or control, such as a TV volume.
///
/// The application sets its state ([`crate::AppCommand::SetNumber`]) and is
/// told of HA's commands ([`crate::AppEvent::NumberChanged`]). The server does
/// **not** echo a command back as a state: HA shows the state the application
/// reports, so a control (a TV volume) only moves once the application has
/// acted on it and says so. Use [`crate::proto::EntityCategory`] and
/// [`crate::proto::NumberMode`] for `entity_category` and `mode`.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberEntity {
    /// Object id: `[a-z0-9_]`, at most 120 characters, stable (the entity key
    /// and HA's entity id derive from it) and unique among all entities.
    pub object_id: String,
    /// Name shown in HA.
    pub name: String,
    /// Material Design icon, e.g. `mdi:volume-high`.
    pub icon: String,
    /// Lowest accepted value.
    pub min_value: f32,
    /// Highest accepted value.
    pub max_value: f32,
    /// Step HA's slider and box use (greater than zero). Commands are not
    /// rounded to it.
    pub step: f32,
    /// Unit of measurement, e.g. `%`. Empty for none.
    pub unit: String,
    /// HA device class (for example `volume`); empty for none. Passed through.
    pub device_class: String,
    /// Slider or box (`Auto` lets HA choose).
    pub mode: crate::proto::NumberMode,
    /// None (primary), configuration or diagnostic.
    pub entity_category: crate::proto::EntityCategory,
}

/// A `button` entity: a stateless action HA can press.
///
/// A press arrives as [`crate::AppEvent::ButtonPressed`]. Buttons have no
/// state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonEntity {
    /// Object id (see [`NumberEntity::object_id`]).
    pub object_id: String,
    /// Name shown in HA.
    pub name: String,
    /// Material Design icon.
    pub icon: String,
    /// HA device class (`restart`, `update` or empty).
    pub device_class: String,
    /// None (primary), configuration or diagnostic.
    pub entity_category: crate::proto::EntityCategory,
}

/// A `binary_sensor` entity: a read-only on/off state.
///
/// The application sets its state ([`crate::AppCommand::SetBinarySensor`]),
/// `None` meaning "no valid state" (HA shows unknown). HA cannot change it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinarySensorEntity {
    /// Object id (see [`NumberEntity::object_id`]).
    pub object_id: String,
    /// Name shown in HA.
    pub name: String,
    /// Material Design icon.
    pub icon: String,
    /// HA binary sensor device class (`running`, `power`, `sound`, ...); empty
    /// for none. Passed through.
    pub device_class: String,
    /// None (primary), configuration or diagnostic.
    pub entity_category: crate::proto::EntityCategory,
}

/// A generic `switch` entity: an on/off control whose state the application
/// owns (the built-in "Mute microphone" switch is separate).
///
/// A command from HA arrives as [`crate::AppEvent::SwitchChanged`] and is
/// **not** echoed: HA shows the state the application reports
/// ([`crate::AppCommand::SetSwitch`], `None` meaning "no valid state", shown
/// as unknown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchEntity {
    /// Object id (see [`NumberEntity::object_id`]).
    pub object_id: String,
    /// Name shown in HA.
    pub name: String,
    /// Material Design icon.
    pub icon: String,
    /// HA switch device class (`switch`, `outlet`) or empty.
    pub device_class: String,
    /// None (primary), configuration or diagnostic.
    pub entity_category: crate::proto::EntityCategory,
}

/// A generic `text_sensor` entity: a read-only string state (the built-in
/// "TV power state" sensor is separate).
///
/// The application sets its state ([`crate::AppCommand::SetTextSensor`]),
/// `None` meaning "no valid state" (HA shows unknown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSensorEntity {
    /// Object id (see [`NumberEntity::object_id`]).
    pub object_id: String,
    /// Name shown in HA.
    pub name: String,
    /// Material Design icon.
    pub icon: String,
    /// HA sensor device class or empty.
    pub device_class: String,
    /// None (primary), configuration or diagnostic.
    pub entity_category: crate::proto::EntityCategory,
}

/// Music playback on the built-in `media_player` (design
/// `docs/design/music-playback.md`, ADR 0010). `Config::music = None` (the
/// default) keeps the entity exactly as it was: feature flags 5124, the
/// ANNOUNCEMENT format only, and a media URL from HA ignored.
///
/// When set, the entity also offers `PLAY_MEDIA`, `PAUSE`, `PLAY` and
/// `BROWSE_MEDIA` (flags 153093), plus `MEDIA_ANNOUNCE` when
/// [`MusicConfig::announce`] is set (1201669), and lists a second supported
/// format with purpose `DEFAULT` **after** the ANNOUNCEMENT one (HA takes the
/// TTS format from the first ANNOUNCEMENT entry, so TTS is unchanged).
///
/// The server only decodes and routes HA's commands
/// ([`crate::AppEvent::Music`]); fetching, decoding and playing are the
/// application's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicConfig {
    /// The `DEFAULT`-purpose format HA's ffmpeg proxy converts music to:
    /// `flac` or `wav` (MP3 is never advertised), 16-bit, one or two
    /// channels. The player follows the header of the stream it gets, not
    /// this value, which only steers HA's proxy and Music Assistant.
    pub format: AudioFormat,
    /// Offer `MEDIA_ANNOUNCE` (`play_media` with `announce: true`).
    pub announce: bool,
    /// Act on a media URL from a **plaintext** (unencrypted) API session.
    /// Default `false`: such a URL is refused, because the application
    /// fetches it (design decision D14).
    pub allow_plaintext: bool,
}

impl Default for MusicConfig {
    /// FLAC, 48000 Hz, stereo, 16-bit; announce offered; plaintext refused.
    fn default() -> Self {
        Self {
            format: AudioFormat {
                format: "flac".to_owned(),
                sample_rate: 48_000,
                num_channels: 2,
                sample_bytes: 2,
            },
            announce: true,
            allow_plaintext: false,
        }
    }
}

/// An application-owned `media_player` entity besides the built-in one (the
/// satellite's reply-volume and announcement player, whose object id and key
/// never change). Use it for a thing the application controls, such as the
/// TV itself, so HA's media player voice intents (volume, mute, turn on and
/// off) can target it.
///
/// It **plays nothing**: it advertises no audio format (so HA never picks it
/// for TTS or announcement media) and [`MediaPlayerEntity::feature_flags`]
/// may only hold the control bits of
/// [`crate::entities::APP_MEDIA_PLAYER_FEATURES`] (no `PLAY_MEDIA`, no
/// `MEDIA_ANNOUNCE`, no `BROWSE_MEDIA`; [`Config::validate`] rejects them). A
/// media URL sent to it anyway is ignored with a log line.
///
/// HA's commands arrive as [`crate::AppEvent::MediaPlayerCommand`], only for
/// advertised features, and are **not echoed**: HA shows the state the
/// application reports with [`crate::AppCommand::SetMediaPlayer`]. The
/// protocol has no "unknown" media player state: until the application sends
/// one, HA shows unknown; after that the last state stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPlayerEntity {
    /// Object id (see [`NumberEntity::object_id`]).
    pub object_id: String,
    /// Name shown in HA after the device name.
    pub name: String,
    /// Material Design icon.
    pub icon: String,
    /// `MediaPlayerEntityFeature` bits (aioesphomeapi 46.6.0, the same values
    /// as HA's); HA 2026.10.4 takes the entity's features from them.
    pub feature_flags: u32,
    /// None (primary), configuration or diagnostic.
    pub entity_category: crate::proto::EntityCategory,
}

/// Configuration of the ESPHome API server.
#[derive(Clone)]
pub struct Config {
    /// Node name: lowercase letters, digits and `-`, at most 31 characters
    /// (an ESPHome hostname). Shown by HA and used as the mDNS host name.
    pub name: String,
    /// Human-readable device name shown in HA.
    pub friendly_name: String,
    /// MAC-style identifier, e.g. `"02:AB:CD:EF:01:23"`. HA uses it as the
    /// device's unique id, so it must stay stable. If `None`, a locally
    /// administered address is derived from [`Config::mac_seed`].
    pub mac: Option<String>,
    /// Seed for the derived MAC when [`Config::mac`] is `None`; defaults to the
    /// node name. Use a stable per-device value (for example a random id stored
    /// in the private configuration).
    pub mac_seed: Option<String>,
    /// Model string reported in device info.
    pub model: String,
    /// Address to listen on.
    pub bind_addr: IpAddr,
    /// TCP port (HA default 6053).
    pub port: u16,
    /// Base64 32-byte Noise key (ESPHome `api: encryption: key:`). `None` means
    /// **plaintext**, intended for development only.
    pub noise_psk: Option<String>,
    /// HA-driven key provisioning ([`crate::pairing`]). Only with
    /// [`Config::noise_psk`] unset: the server then starts unpaired (or with
    /// the key stored in [`Provisioning::key_file`]). `None` and no key means
    /// **plaintext** (development only).
    pub provisioning: Option<Provisioning>,
    /// Advertise `_esphomelib._tcp` over mDNS (fails soft).
    pub mdns: bool,
    /// Announcement/TTS format announced on the media player entity.
    pub announcement_format: AudioFormat,
    /// Voice assistant feature bits ([`features`]); default [`features::DEFAULT`].
    pub voice_features: u32,
    /// List the diagnostic "TV power state" text sensor (the application
    /// feeds it with [`crate::AppCommand::SetPowerState`]).
    pub power_sensor: bool,
    /// Configuration select entities (entity category `config`).
    pub selects: Vec<SelectEntity>,
    /// `number` entities.
    pub numbers: Vec<NumberEntity>,
    /// `button` entities.
    pub buttons: Vec<ButtonEntity>,
    /// `binary_sensor` entities.
    pub binary_sensors: Vec<BinarySensorEntity>,
    /// Generic `switch` entities (besides the built-in mute switch).
    pub switches: Vec<SwitchEntity>,
    /// Generic `text_sensor` entities (besides the built-in power state).
    pub text_sensors: Vec<TextSensorEntity>,
    /// Application-owned media players (besides the built-in one).
    pub media_players: Vec<MediaPlayerEntity>,
    /// Music playback on the built-in media player; `None` (default) keeps it
    /// announcements-only ([`MusicConfig`]).
    pub music: Option<MusicConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: "webos-satellite".to_owned(),
            friendly_name: "TV Voice Satellite".to_owned(),
            mac: None,
            mac_seed: None,
            model: "LG webOS TV".to_owned(),
            bind_addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            noise_psk: None,
            provisioning: None,
            mdns: false,
            announcement_format: AudioFormat::default(),
            voice_features: features::DEFAULT,
            power_sensor: false,
            selects: Vec::new(),
            numbers: Vec::new(),
            buttons: Vec::new(),
            binary_sensors: Vec::new(),
            switches: Vec::new(),
            text_sensors: Vec::new(),
            media_players: Vec::new(),
            music: None,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("name", &self.name)
            .field("friendly_name", &self.friendly_name)
            .field("mac", &self.mac)
            .field("model", &self.model)
            .field("bind_addr", &self.bind_addr)
            .field("port", &self.port)
            .field("noise_psk", &self.noise_psk.as_ref().map(|_| "<redacted>"))
            .field("provisioning", &self.provisioning)
            .field("mdns", &self.mdns)
            .field("announcement_format", &self.announcement_format)
            .field("voice_features", &self.voice_features)
            .field("power_sensor", &self.power_sensor)
            .field("selects", &self.selects)
            .field("numbers", &self.numbers)
            .field("buttons", &self.buttons)
            .field("binary_sensors", &self.binary_sensors)
            .field("switches", &self.switches)
            .field("text_sensors", &self.text_sensors)
            .field("media_players", &self.media_players)
            .field("music", &self.music)
            .finish_non_exhaustive()
    }
}

/// Invalid configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// Node name is empty, too long or has characters other than `[a-z0-9-]`.
    InvalidName,
    /// `mac` is not six colon- or dash-separated hex octets (or 12 hex digits).
    InvalidMac,
    /// `noise_psk` is not base64 of exactly 32 bytes.
    InvalidPsk(&'static str),
    /// Both `noise_psk` and `provisioning` are set, or the pairing window is
    /// zero.
    InvalidProvisioning(&'static str),
    /// Announcement format is not usable (empty name or zero sample rate).
    InvalidFormat,
    /// `voice_features` lacks VOICE_ASSISTANT or API_AUDIO, or has
    /// unimplemented bits.
    InvalidFeatures,
    /// An entity definition is not usable (object id, duplicate id or key,
    /// number range); the text says which.
    InvalidEntity(String),
    /// The music format is not usable; the text says why.
    InvalidMusic(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("name must be 1-31 chars of [a-z0-9-]"),
            Self::InvalidMac => f.write_str("mac must look like 02:AB:CD:EF:01:23"),
            Self::InvalidPsk(why) | Self::InvalidProvisioning(why) => f.write_str(why),
            Self::InvalidFormat => f.write_str("announcement format needs a name and a sample rate"),
            Self::InvalidFeatures => f.write_str(
                "voice_features must include VOICE_ASSISTANT and API_AUDIO and only implemented bits",
            ),
            Self::InvalidEntity(why) => write!(f, "invalid entity: {why}"),
            Self::InvalidMusic(why) => write!(f, "invalid music config: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A 48-bit MAC-style device identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mac(pub [u8; 6]);

impl Mac {
    /// Parses `AA:BB:CC:DD:EE:FF`, `aa-bb-…` or `aabbccddeeff`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::InvalidMac`] on any other input.
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        let hex: String = s.chars().filter(|c| *c != ':' && *c != '-').collect();
        if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ConfigError::InvalidMac);
        }
        let mut out = [0_u8; 6];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
                .map_err(|_| ConfigError::InvalidMac)?;
        }
        Ok(Self(out))
    }

    /// Derives a stable, locally administered, unicast address from `seed`
    /// (64-bit FNV-1a; identification only, not a secret).
    #[must_use]
    pub fn derive(seed: &str) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for b in b"webos-voice-satellite:".iter().chain(seed.as_bytes()) {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        let b = hash.to_be_bytes();
        // Locally administered (bit 1 set), unicast (bit 0 clear).
        Self([(b[0] & 0xfc) | 0x02, b[1], b[2], b[3], b[4], b[5]])
    }

    /// `AA:BB:CC:DD:EE:FF`, the form ESPHome reports in device info.
    #[must_use]
    pub fn pretty(&self) -> String {
        let b = self.0;
        format!(
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }

    /// `aabbccddeeff`, the form ESPHome uses in the Noise hello and mDNS TXT.
    #[must_use]
    pub fn compact(&self) -> String {
        use std::fmt::Write as _;
        self.0.iter().fold(String::with_capacity(12), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }
}

/// Validated configuration with decoded secrets.
#[derive(Clone)]
pub(crate) struct Resolved {
    pub(crate) name: String,
    pub(crate) friendly_name: String,
    pub(crate) mac: Mac,
    pub(crate) model: String,
    pub(crate) psk: Option<[u8; 32]>,
    pub(crate) provisioning: Option<Provisioning>,
    pub(crate) mdns: bool,
    pub(crate) announcement_format: AudioFormat,
    pub(crate) voice_features: u32,
    pub(crate) power_sensor: bool,
    pub(crate) selects: Vec<SelectEntity>,
    pub(crate) numbers: Vec<NumberEntity>,
    pub(crate) buttons: Vec<ButtonEntity>,
    pub(crate) binary_sensors: Vec<BinarySensorEntity>,
    pub(crate) switches: Vec<SwitchEntity>,
    pub(crate) text_sensors: Vec<TextSensorEntity>,
    pub(crate) media_players: Vec<MediaPlayerEntity>,
    pub(crate) music: Option<MusicConfig>,
}

/// An ESPHome object id: `[a-z0-9_]`, 1 to 120 characters.
fn valid_object_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl Config {
    /// Checks the entity definitions: valid object ids, no duplicate ids or
    /// keys (including the built-in entities), sane number ranges.
    fn check_entities(&self) -> Result<(), ConfigError> {
        use crate::entities::{
            APP_MEDIA_PLAYER_FEATURES, MEDIA_PLAYER_OBJECT_ID, MUTE_OBJECT_ID,
            POWER_STATE_OBJECT_ID, entity_key,
        };
        let err = |why: String| Err(ConfigError::InvalidEntity(why));
        let ids = self
            .selects
            .iter()
            .map(|e| e.object_id.as_str())
            .chain(self.numbers.iter().map(|e| e.object_id.as_str()))
            .chain(self.buttons.iter().map(|e| e.object_id.as_str()))
            .chain(self.binary_sensors.iter().map(|e| e.object_id.as_str()))
            .chain(self.switches.iter().map(|e| e.object_id.as_str()))
            .chain(self.text_sensors.iter().map(|e| e.object_id.as_str()))
            .chain(self.media_players.iter().map(|e| e.object_id.as_str()));
        let mut seen: Vec<(&str, u32)> = vec![
            (MEDIA_PLAYER_OBJECT_ID, entity_key(MEDIA_PLAYER_OBJECT_ID)),
            (MUTE_OBJECT_ID, entity_key(MUTE_OBJECT_ID)),
        ];
        if self.power_sensor {
            seen.push((POWER_STATE_OBJECT_ID, entity_key(POWER_STATE_OBJECT_ID)));
        }
        for id in ids {
            if !valid_object_id(id) {
                return err(format!("object id {id:?} must be 1-120 chars of [a-z0-9_]"));
            }
            let key = entity_key(id);
            if let Some((other, _)) = seen.iter().find(|(o, k)| *o == id || *k == key) {
                return err(format!(
                    "object id {id:?} duplicates or collides with {other:?}"
                ));
            }
            seen.push((id, key));
        }
        for sel in &self.selects {
            if sel.options.is_empty() {
                return err(format!(
                    "select {:?}: needs at least one option",
                    sel.object_id
                ));
            }
        }
        for mp in &self.media_players {
            let extra = mp.feature_flags & !APP_MEDIA_PLAYER_FEATURES;
            if extra != 0 {
                return err(format!(
                    "media player {:?}: feature bits {extra:#x} are not control features (it plays no media)",
                    mp.object_id
                ));
            }
        }
        for n in &self.numbers {
            if !(n.min_value.is_finite() && n.max_value.is_finite() && n.min_value <= n.max_value) {
                return err(format!(
                    "number {:?}: min_value must be finite and <= max_value",
                    n.object_id
                ));
            }
            if !(n.step.is_finite() && n.step > 0.0) {
                return err(format!(
                    "number {:?}: step must be finite and > 0",
                    n.object_id
                ));
            }
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 31
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Config {
    /// Checks the configuration.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.resolve().map(|_| ())
    }

    /// The MAC the server will report.
    ///
    /// # Errors
    ///
    /// [`ConfigError::InvalidMac`] if [`Config::mac`] does not parse.
    pub fn effective_mac(&self) -> Result<Mac, ConfigError> {
        match &self.mac {
            Some(mac) => Mac::parse(mac),
            None => Ok(Mac::derive(self.mac_seed.as_deref().unwrap_or(&self.name))),
        }
    }

    pub(crate) fn resolve(&self) -> Result<Resolved, ConfigError> {
        if !valid_name(&self.name) {
            return Err(ConfigError::InvalidName);
        }
        let fmt = &self.announcement_format;
        if fmt.format.is_empty() || fmt.sample_rate == 0 {
            return Err(ConfigError::InvalidFormat);
        }
        let required = features::VOICE_ASSISTANT | features::API_AUDIO;
        if self.voice_features & required != required
            || self.voice_features & !features::SUPPORTED != 0
        {
            return Err(ConfigError::InvalidFeatures);
        }
        self.check_entities()?;
        if let Some(m) = &self.music {
            let f = &m.format;
            if f.format != "flac" && f.format != "wav" {
                return Err(ConfigError::InvalidMusic("format must be flac or wav"));
            }
            if f.sample_rate == 0 {
                return Err(ConfigError::InvalidMusic("sample_rate must be non-zero"));
            }
            if !(1..=2).contains(&f.num_channels) {
                return Err(ConfigError::InvalidMusic("channels must be 1 or 2"));
            }
            if f.sample_bytes != 2 {
                return Err(ConfigError::InvalidMusic(
                    "only 16-bit (2 bytes) is offered",
                ));
            }
        }
        let psk = self
            .noise_psk
            .as_deref()
            .map(crate::frame::decode_psk)
            .transpose()
            .map_err(ConfigError::InvalidPsk)?;
        if let Some(p) = &self.provisioning {
            if psk.is_some() {
                return Err(ConfigError::InvalidProvisioning(
                    "noise_psk and provisioning are mutually exclusive",
                ));
            }
            if p.window.is_zero() {
                return Err(ConfigError::InvalidProvisioning(
                    "the pairing window must be longer than zero",
                ));
            }
            if p.key_file.file_name().is_none() {
                return Err(ConfigError::InvalidProvisioning(
                    "the pairing key file needs a file name",
                ));
            }
        }
        Ok(Resolved {
            name: self.name.clone(),
            friendly_name: if self.friendly_name.is_empty() {
                self.name.clone()
            } else {
                self.friendly_name.clone()
            },
            mac: self.effective_mac()?,
            model: self.model.clone(),
            psk,
            provisioning: self.provisioning.clone(),
            mdns: self.mdns,
            announcement_format: fmt.clone(),
            voice_features: self.voice_features,
            power_sensor: self.power_sensor,
            selects: self.selects.clone(),
            numbers: self.numbers.clone(),
            buttons: self.buttons.clone(),
            binary_sensors: self.binary_sensors.clone(),
            switches: self.switches.clone(),
            text_sensors: self.text_sensors.clone(),
            media_players: self.media_players.clone(),
            music: self.music.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn names() {
        assert!(valid_name("lg-c3-tv"));
        assert!(!valid_name(""));
        assert!(!valid_name("LG"));
        assert!(!valid_name("a_b"));
        assert!(!valid_name(&"a".repeat(32)));
    }

    #[test]
    fn mac_parse_and_format() {
        let mac = Mac::parse("02:ab:CD:ef:01:23").unwrap();
        assert_eq!(mac.pretty(), "02:AB:CD:EF:01:23");
        assert_eq!(mac.compact(), "02abcdef0123");
        assert_eq!(Mac::parse("02abcdef0123").unwrap(), mac);
        assert_eq!(Mac::parse("02-ab-cd-ef-01-23").unwrap(), mac);
        assert!(Mac::parse("02:ab:cd").is_err());
        assert!(Mac::parse("zz:ab:cd:ef:01:23").is_err());
    }

    #[test]
    fn derived_mac_is_stable_and_local() {
        let a = Mac::derive("seed-1");
        assert_eq!(a, Mac::derive("seed-1"));
        assert_ne!(a, Mac::derive("seed-2"));
        assert_eq!(a.0[0] & 0x03, 0x02);
    }

    #[test]
    fn psk_validation_and_redaction() {
        let mut cfg = Config {
            noise_psk: Some("short".to_owned()),
            ..Config::default()
        };
        assert!(matches!(cfg.validate(), Err(ConfigError::InvalidPsk(_))));
        cfg.noise_psk = Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned());
        cfg.validate().unwrap();
        assert!(!format!("{cfg:?}").contains("AAAA"));
    }

    #[test]
    fn feature_validation() {
        assert_eq!(features::DEFAULT, 23);
        let mut cfg = Config::default();
        cfg.voice_features = features::VOICE_ASSISTANT | features::SPEAKER;
        assert_eq!(cfg.validate(), Err(ConfigError::InvalidFeatures));
        cfg.voice_features = features::DEFAULT | features::MULTI_CHANNEL_AUDIO;
        assert_eq!(cfg.validate(), Err(ConfigError::InvalidFeatures));
        assert_eq!(features::DEFAULT_WITH_TIMERS, 31);
        cfg.voice_features = features::DEFAULT_WITH_TIMERS;
        cfg.validate().unwrap();
        assert_eq!(Config::default().voice_features & features::TIMERS, 0);
        cfg.voice_features = features::SUPPORTED;
        cfg.validate().unwrap();
    }

    #[test]
    fn provisioning_excludes_a_fixed_key() {
        let mut cfg = Config {
            provisioning: Some(Provisioning::default()),
            ..Config::default()
        };
        cfg.validate().unwrap();
        cfg.noise_psk = Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned());
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidProvisioning(_))
        ));
        cfg.noise_psk = None;
        cfg.provisioning = Some(Provisioning {
            window: std::time::Duration::ZERO,
            ..Provisioning::default()
        });
        assert!(cfg.validate().is_err());
    }

    fn number(id: &str) -> NumberEntity {
        NumberEntity {
            object_id: id.to_owned(),
            name: "N".to_owned(),
            icon: String::new(),
            min_value: 0.0,
            max_value: 100.0,
            step: 1.0,
            unit: "%".to_owned(),
            device_class: String::new(),
            mode: crate::proto::NumberMode::Slider,
            entity_category: crate::proto::EntityCategory::None,
        }
    }

    fn binary(id: &str) -> BinarySensorEntity {
        BinarySensorEntity {
            object_id: id.to_owned(),
            name: "B".to_owned(),
            icon: String::new(),
            device_class: String::new(),
            entity_category: crate::proto::EntityCategory::None,
        }
    }

    #[test]
    fn entity_definitions_are_validated() {
        let mut cfg = Config::default();
        cfg.numbers.push(number("tv_volume"));
        cfg.binary_sensors.push(binary("listening"));
        cfg.validate().unwrap();
        let bad = |c: &Config| matches!(c.validate(), Err(ConfigError::InvalidEntity(_)));
        // Bad characters, empty, duplicates (also across types) and the
        // built-in ids.
        for id in ["", "Tv", "tv-volume", "tv volume"] {
            let mut c = Config::default();
            c.numbers.push(number(id));
            assert!(bad(&c), "{id:?}");
        }
        let mut c = cfg.clone();
        c.binary_sensors.push(binary("tv_volume"));
        assert!(bad(&c));
        let mut c = Config::default();
        c.numbers.push(number("media_player"));
        assert!(bad(&c));
        let mut c = Config::default();
        c.binary_sensors.push(binary("mute_microphone"));
        assert!(bad(&c));
        // The power sensor id is reserved only while the sensor exists.
        let mut c = Config::default();
        c.binary_sensors.push(binary("tv_power_state"));
        c.validate().unwrap();
        c.power_sensor = true;
        assert!(bad(&c));
        // Ranges.
        let mut c = Config::default();
        let mut n = number("a");
        n.min_value = 5.0;
        n.max_value = 1.0;
        c.numbers.push(n);
        assert!(bad(&c));
        let mut c = Config::default();
        let mut n = number("a");
        n.step = 0.0;
        c.numbers.push(n);
        assert!(bad(&c));
        let mut c = Config::default();
        let mut n = number("a");
        n.max_value = f32::NAN;
        c.numbers.push(n);
        assert!(bad(&c));
    }

    #[test]
    fn switch_text_sensor_and_select_definitions_are_validated() {
        use crate::proto::EntityCategory;
        let switch = |id: &str| SwitchEntity {
            object_id: id.to_owned(),
            name: "S".to_owned(),
            icon: String::new(),
            device_class: String::new(),
            entity_category: EntityCategory::None,
        };
        let text = |id: &str| TextSensorEntity {
            object_id: id.to_owned(),
            name: "T".to_owned(),
            icon: String::new(),
            device_class: String::new(),
            entity_category: EntityCategory::Diagnostic,
        };
        let mut cfg = Config::default();
        cfg.switches.push(switch("tv_screen"));
        cfg.text_sensors.push(text("tv_sound_output"));
        cfg.validate().unwrap();
        let bad = |c: &Config| matches!(c.validate(), Err(ConfigError::InvalidEntity(_)));
        let mut c = cfg.clone();
        c.text_sensors.push(text("tv_screen"));
        assert!(bad(&c), "duplicate across types");
        let mut c = cfg.clone();
        c.switches.push(switch("mute_microphone"));
        assert!(bad(&c), "the built-in mute switch id");
        let mut c = cfg.clone();
        c.switches.push(switch("Bad Id"));
        assert!(bad(&c));
        let mut c = cfg.clone();
        c.text_sensors.push(text("tv_power_state"));
        c.validate().unwrap();
        c.power_sensor = true;
        assert!(bad(&c), "the built-in power state id");
        let mut c = cfg.clone();
        c.selects.push(SelectEntity {
            object_id: "tv_input".to_owned(),
            name: "I".to_owned(),
            icon: String::new(),
            options: Vec::new(),
            entity_category: EntityCategory::None,
            command_echo: false,
        });
        assert!(bad(&c), "a select needs options");
        c.selects[0].options.push("Other".to_owned());
        c.validate().unwrap();
    }

    #[test]
    fn music_config_is_validated() {
        let with = |f: &dyn Fn(&mut MusicConfig)| {
            let mut m = MusicConfig::default();
            f(&mut m);
            Config {
                music: Some(m),
                ..Config::default()
            }
            .validate()
        };
        assert!(with(&|_| {}).is_ok());
        assert!(with(&|m| "wav".clone_into(&mut m.format.format)).is_ok());
        assert!(with(&|m| m.format.num_channels = 1).is_ok());
        // MP3 is never advertised; nor an unknown name, rate 0, 3 channels, 24-bit.
        for bad in [
            &(|m: &mut MusicConfig| "mp3".clone_into(&mut m.format.format))
                as &dyn Fn(&mut MusicConfig),
            &|m| "opus".clone_into(&mut m.format.format),
            &|m| m.format.sample_rate = 0,
            &|m| m.format.num_channels = 3,
            &|m| m.format.num_channels = 0,
            &|m| m.format.sample_bytes = 3,
        ] {
            assert!(matches!(with(bad), Err(ConfigError::InvalidMusic(_))));
        }
        // Defaults: FLAC, 48 kHz, stereo, 16-bit; announce on; plaintext refused.
        let d = MusicConfig::default();
        assert_eq!(
            (
                d.format.format.as_str(),
                d.format.sample_rate,
                d.format.num_channels,
                d.format.sample_bytes
            ),
            ("flac", 48_000, 2, 2)
        );
        assert!(d.announce && !d.allow_plaintext);
        assert!(Config::default().music.is_none());
    }

    #[test]
    fn media_player_definitions_are_validated() {
        use crate::entities::{
            FEATURE_MEDIA_ANNOUNCE, FEATURE_PLAY_MEDIA, FEATURE_TURN_OFF, FEATURE_TURN_ON,
            FEATURE_VOLUME_MUTE, FEATURE_VOLUME_SET, FEATURE_VOLUME_STEP,
        };
        use crate::proto::EntityCategory;
        let mp = |id: &str, flags: u32| MediaPlayerEntity {
            object_id: id.to_owned(),
            name: "TV".to_owned(),
            icon: String::new(),
            feature_flags: flags,
            entity_category: EntityCategory::None,
        };
        let control = FEATURE_VOLUME_SET
            | FEATURE_VOLUME_STEP
            | FEATURE_VOLUME_MUTE
            | FEATURE_TURN_ON
            | FEATURE_TURN_OFF;
        let mut cfg = Config::default();
        cfg.media_players.push(mp("tv_media_player", control));
        cfg.validate().unwrap();
        let bad = |c: &Config| matches!(c.validate(), Err(ConfigError::InvalidEntity(_)));
        // Play media, announce, browse media and select source (which HA's
        // ESPHome platform drops anyway) are refused.
        for flags in [FEATURE_PLAY_MEDIA, FEATURE_MEDIA_ANNOUNCE, 1 << 17, 1 << 11] {
            let mut c = Config::default();
            c.media_players.push(mp("tv_media_player", control | flags));
            assert!(bad(&c), "{flags:#x} must be refused");
        }
        let mut c = Config::default();
        c.media_players.push(mp("media_player", control));
        assert!(bad(&c), "the built-in media player id");
        let mut c = cfg.clone();
        c.numbers.push(number("tv_media_player"));
        assert!(bad(&c), "duplicate across types");
    }

    #[test]
    fn format_needs_sample_rate() {
        let mut cfg = Config::default();
        cfg.announcement_format.sample_rate = 0;
        assert_eq!(cfg.validate(), Err(ConfigError::InvalidFormat));
    }
}
