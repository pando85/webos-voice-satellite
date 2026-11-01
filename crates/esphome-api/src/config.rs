//! Server configuration.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

use crate::DEFAULT_PORT;

/// `VoiceAssistantFeature` bits (aioesphomeapi 46.6.0 `model.py`), reported in
/// `DeviceInfoResponse.voice_assistant_feature_flags`.
pub mod features {
    /// Voice assistant present (required, else HA creates no satellite).
    pub const VOICE_ASSISTANT: u32 = 1;
    /// TTS is streamed to the device as audio instead of a URL.
    pub const SPEAKER: u32 = 2;
    /// Audio travels over the API connection, not UDP (required here).
    pub const API_AUDIO: u32 = 4;
    /// Timer events (not implemented; rejected).
    pub const TIMERS: u32 = 8;
    /// Announcements (`VoiceAssistantAnnounceRequest`).
    pub const ANNOUNCE: u32 = 16;
    /// HA may start a conversation after an announcement.
    pub const START_CONVERSATION: u32 = 32;
    /// Second, non-enhanced audio channel (not implemented; rejected).
    pub const MULTI_CHANNEL_AUDIO: u32 = 64;
    /// Bits this crate implements.
    pub const SUPPORTED: u32 =
        VOICE_ASSISTANT | SPEAKER | API_AUDIO | ANNOUNCE | START_CONVERSATION;
    /// Default: TTS streamed over the API (no URL fetch, which matters when
    /// HA's URLs are not reachable from the TV) plus announcements.
    pub const DEFAULT: u32 = VOICE_ASSISTANT | SPEAKER | API_AUDIO | ANNOUNCE;
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
    /// Advertise `_esphomelib._tcp` over mDNS (fails soft).
    pub mdns: bool,
    /// Announcement/TTS format announced on the media player entity.
    pub announcement_format: AudioFormat,
    /// Voice assistant feature bits ([`features`]); default [`features::DEFAULT`].
    pub voice_features: u32,
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
            mdns: false,
            announcement_format: AudioFormat::default(),
            voice_features: features::DEFAULT,
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
            .field("mdns", &self.mdns)
            .field("announcement_format", &self.announcement_format)
            .field("voice_features", &self.voice_features)
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
    /// Announcement format is not usable (empty name or zero sample rate).
    InvalidFormat,
    /// `voice_features` lacks VOICE_ASSISTANT or API_AUDIO, or has
    /// unimplemented bits.
    InvalidFeatures,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("name must be 1-31 chars of [a-z0-9-]"),
            Self::InvalidMac => f.write_str("mac must look like 02:AB:CD:EF:01:23"),
            Self::InvalidPsk(why) => f.write_str(why),
            Self::InvalidFormat => f.write_str("announcement format needs a name and a sample rate"),
            Self::InvalidFeatures => f.write_str(
                "voice_features must include VOICE_ASSISTANT and API_AUDIO and only implemented bits",
            ),
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
    pub(crate) mdns: bool,
    pub(crate) announcement_format: AudioFormat,
    pub(crate) voice_features: u32,
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
        let psk = self
            .noise_psk
            .as_deref()
            .map(crate::frame::decode_psk)
            .transpose()
            .map_err(ConfigError::InvalidPsk)?;
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
            mdns: self.mdns,
            announcement_format: fmt.clone(),
            voice_features: self.voice_features,
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
        cfg.voice_features = features::DEFAULT | features::TIMERS;
        assert_eq!(cfg.validate(), Err(ConfigError::InvalidFeatures));
        cfg.voice_features = features::SUPPORTED;
        cfg.validate().unwrap();
    }

    #[test]
    fn format_needs_sample_rate() {
        let mut cfg = Config::default();
        cfg.announcement_format.sample_rate = 0;
        assert_eq!(cfg.validate(), Err(ConfigError::InvalidFormat));
    }
}
