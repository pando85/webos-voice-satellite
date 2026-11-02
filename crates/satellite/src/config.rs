//! Configuration file (TOML) and command-line overrides.
//!
//! Every section and key is optional; the defaults are the verified values for
//! the LG C3 (c3-a). Unknown keys are rejected so typos do not silently fall
//! back to defaults. Relative paths in the file are resolved against the
//! directory of the file; relative paths given on the command line against the
//! current directory. See `config/example.toml`.

use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

/// Default `sink.latency_msec`: P2 session 1 on c3-a measured the reply start
/// about 0.1 s after the TTS stream started with 200 ms, against about 1.5 s
/// with PulseAudio's default buffering.
pub(crate) const DEFAULT_LATENCY_MSEC: u32 = 200;
/// Upper bound accepted for `sink.latency_msec`.
pub(crate) const MAX_LATENCY_MSEC: u32 = 10_000;

/// Whole configuration.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    /// Pipeline trigger mode.
    pub(crate) mode: Mode,
    /// ESPHome device identity and API server.
    pub(crate) device: DeviceSection,
    /// Microphone source.
    pub(crate) mic: MicSection,
    /// Microphone front-end.
    pub(crate) dsp: DspSection,
    /// Reply audio output.
    pub(crate) sink: SinkSection,
    /// Logging.
    pub(crate) log: LogSection,
    /// Kill switch, mute persistence and firmware guard.
    pub(crate) safety: SafetySection,
    /// Resource limits and watchdog.
    pub(crate) limits: LimitsSection,
    /// HA-driven key provisioning (pairing without typing a key).
    pub(crate) pairing: PairingSection,
    /// Acoustic echo cancellation against the TV's own output.
    pub(crate) aec: AecSection,
}

/// How pipeline runs start.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    /// Continuous streaming; Home Assistant runs the wake word (ADR 0003).
    #[default]
    Wake,
    /// One run per trigger (SIGUSR1), starting at speech-to-text.
    Ptt,
}

/// `[device]`.
#[derive(Clone, Deserialize, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // one bool per TOML switch
#[serde(default, deny_unknown_fields)]
pub(crate) struct DeviceSection {
    /// ESPHome node name (`[a-z0-9-]`, at most 31 characters).
    pub(crate) name: String,
    /// Name shown in Home Assistant.
    pub(crate) friendly_name: String,
    /// Fixed MAC-style id (HA's unique id for the device).
    pub(crate) mac: Option<String>,
    /// Seed for a derived, stable MAC-style id when `mac` is not set.
    pub(crate) mac_seed: Option<String>,
    /// Model string shown in HA.
    pub(crate) model: String,
    /// Listen address.
    pub(crate) bind: IpAddr,
    /// TCP port.
    pub(crate) port: u16,
    /// Base64 Noise key inline (prefer `noise_psk_file`).
    pub(crate) noise_psk: Option<String>,
    /// File holding the base64 Noise key.
    pub(crate) noise_psk_file: Option<PathBuf>,
    /// Allow running without encryption (development only).
    pub(crate) allow_plaintext: bool,
    /// Advertise `_esphomelib._tcp` over mDNS.
    pub(crate) mdns: bool,
    /// Advertise ANNOUNCE (bit 16): HA may send announcements.
    pub(crate) announce: bool,
    /// Advertise TIMERS (bit 8): HA forwards its timers to the device.
    pub(crate) timers: bool,
    /// Advertise START_CONVERSATION (bit 32): HA may start a conversation
    /// after an announcement. Needs `announce`.
    pub(crate) start_conversation: bool,
}

impl Default for DeviceSection {
    fn default() -> Self {
        Self {
            name: "webos-satellite".to_owned(),
            friendly_name: "TV Voice Satellite".to_owned(),
            mac: None,
            mac_seed: None,
            model: "LG webOS TV".to_owned(),
            bind: IpAddr::from([0, 0, 0, 0]),
            port: webos_esphome_api::DEFAULT_PORT,
            noise_psk: None,
            noise_psk_file: None,
            allow_plaintext: false,
            mdns: false,
            announce: true,
            timers: false,
            start_conversation: false,
        }
    }
}

impl fmt::Debug for DeviceSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceSection")
            .field("name", &self.name)
            .field("friendly_name", &self.friendly_name)
            .field("mac", &self.mac)
            .field("mac_seed", &self.mac_seed.as_ref().map(|_| "<set>"))
            .field("model", &self.model)
            .field("bind", &self.bind)
            .field("port", &self.port)
            .field("noise_psk", &self.noise_psk.as_ref().map(|_| "<redacted>"))
            .field("noise_psk_file", &self.noise_psk_file)
            .field("allow_plaintext", &self.allow_plaintext)
            .field("mdns", &self.mdns)
            .field("announce", &self.announce)
            .field("timers", &self.timers)
            .field("start_conversation", &self.start_conversation)
            .finish()
    }
}

/// Microphone source kind.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MicSource {
    /// ALSA capture PCM (the TV's built-in mic).
    #[default]
    Alsa,
    /// Raw S24_LE interleaved file (tests on a PC).
    File,
}

/// `[mic]`. ALSA keys apply to `source = "alsa"`, file keys to `"file"`;
/// `rate` and `channels` apply to both.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MicSection {
    /// Source kind.
    pub(crate) source: MicSource,
    /// ALSA card id (`/proc/asound/cards`), not an index.
    pub(crate) card_id: String,
    /// Substring of the PCM name or id.
    pub(crate) pcm_name: Option<String>,
    /// Explicit PCM device number (overrides `pcm_name`).
    pub(crate) device: Option<u32>,
    /// Sample rate in Hz (must be 16000: the front-end does not resample).
    pub(crate) rate: u32,
    /// Interleaved channels.
    pub(crate) channels: u16,
    /// ALSA period size in frames.
    pub(crate) period_frames: u32,
    /// ALSA ring buffer size in frames.
    pub(crate) buffer_frames: u32,
    /// Audio dropped after every ALSA start and overrun recovery.
    pub(crate) discard_ms: u32,
    /// File path (`source = "file"`).
    pub(crate) path: Option<PathBuf>,
    /// Pace file reads at the sample rate, like a live mic.
    pub(crate) realtime: bool,
    /// Restart the file at its end.
    #[serde(rename = "loop")]
    pub(crate) looping: bool,
    /// Skip this much audio at the start of the file (startup burst).
    pub(crate) skip_ms: u32,
    /// Advance the file only while audio is streamed to HA, so each run sees
    /// the next part of the recording instead of what passed while idle.
    pub(crate) pause_when_idle: bool,
}

impl Default for MicSection {
    fn default() -> Self {
        Self {
            source: MicSource::Alsa,
            card_id: "LGSIC".to_owned(),
            pcm_name: Some("WoV PDM Mic".to_owned()),
            device: None,
            rate: 16_000,
            channels: 2,
            period_frames: 256,
            buffer_frames: 8192,
            discard_ms: 250,
            path: None,
            realtime: true,
            looping: false,
            skip_ms: 250,
            pause_when_idle: true,
        }
    }
}

/// Channel selection of the front-end.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Channel {
    /// Channel 0 (ADR 0004).
    #[default]
    Left,
    /// Channel 1.
    Right,
    /// Mean of all channels.
    Mix,
}

/// `[dsp]`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DspSection {
    /// Channel selection.
    pub(crate) channel: Channel,
    /// Fixed gain in dB.
    pub(crate) gain_db: f32,
    /// High-pass cutoff in Hz (0 disables).
    pub(crate) hpf_hz: f32,
}

impl Default for DspSection {
    fn default() -> Self {
        Self {
            channel: Channel::Left,
            gain_db: 24.0,
            hpf_hz: 120.0,
        }
    }
}

/// Reply sink kind.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SinkKind {
    /// PulseAudio through `pacat` (the TV).
    #[default]
    Pacat,
    /// One WAV file per reply in `dir` (PC tests, silent TV tests).
    Wav,
    /// Discard.
    Null,
}

/// `[sink]`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SinkSection {
    /// Sink kind.
    #[serde(rename = "type")]
    pub(crate) kind: SinkKind,
    /// Software gain applied to every sample written to `pacat` (1.0 unity,
    /// 0.0 digital silence). The reliable level control on c3-a, where
    /// `pacat --volume` is ignored.
    pub(crate) gain: f32,
    /// `pacat` binary.
    pub(crate) binary: PathBuf,
    /// PulseAudio sink name; unset uses the default sink.
    pub(crate) device: Option<String>,
    /// Rate for decoded URL/announcement audio sent to `pacat`. Streamed TTS
    /// is sent at its native 16 kHz mono and PulseAudio converts it.
    pub(crate) rate: u32,
    /// Channels for decoded URL/announcement audio sent to `pacat`.
    pub(crate) channels: u16,
    /// `pacat --latency-msec` for replies and announcements, in ms. 0 omits
    /// the flag (PulseAudio's default buffering, about 2 s, which delayed the
    /// start of short streamed replies by about 1.5 s on c3-a).
    pub(crate) latency_msec: u32,
    /// Output directory of the WAV sink.
    pub(crate) dir: PathBuf,
}

impl Default for SinkSection {
    fn default() -> Self {
        Self {
            kind: SinkKind::Pacat,
            gain: 1.0,
            binary: PathBuf::from("pacat"),
            device: None,
            rate: 44_100,
            channels: 2,
            latency_msec: DEFAULT_LATENCY_MSEC,
            dir: PathBuf::from("."),
        }
    }
}

/// `[log]`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LogSection {
    /// `error`, `warn`, `info`, `debug` or `trace`.
    pub(crate) level: String,
}

impl Default for LogSection {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
        }
    }
}

/// `[safety]`: privacy and firmware gates.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SafetySection {
    /// While this file exists the microphone PCM is closed, nothing is
    /// streamed and HA sees the mute switch on; HA cannot unmute. Checked at
    /// startup and every 250 ms.
    pub(crate) kill_switch: PathBuf,
    /// Remembers HA's mute switch across daemon restarts: created on mute,
    /// removed on unmute, read at startup.
    pub(crate) mute_file: PathBuf,
    /// Firmware versions (`webos_manufacturing_version`) the ALSA capture has
    /// been validated on. On any other version, or if `firmware_info` cannot
    /// be read, the satellite starts with the microphone disabled. An empty
    /// list turns the guard off (with a warning).
    pub(crate) validated_firmware: Vec<String>,
    /// Read-only JSON file with the TV's OS information (nyx).
    pub(crate) firmware_info: PathBuf,
}

impl Default for SafetySection {
    fn default() -> Self {
        Self {
            kill_switch: PathBuf::from("/tmp/webos-satellite.disable"),
            mute_file: PathBuf::from("/tmp/webos-satellite.muted"),
            validated_firmware: vec!["03.30.74".to_owned()],
            firmware_info: PathBuf::from("/var/run/nyx/os_info.json"),
        }
    }
}

/// `[limits]`: resource limits and watchdog thresholds.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LimitsSection {
    /// Scheduling niceness of the daemon (and its `pacat` child), -20 to 19.
    pub(crate) nice: i32,
    /// Soft `RLIMIT_NOFILE` (0 keeps the inherited limit).
    pub(crate) max_open_files: u32,
    /// Resident memory ceiling in MiB; above it the daemon exits so a
    /// supervisor restarts it (0 disables).
    pub(crate) max_rss_mb: u32,
    /// An ALSA read that sees no audio for this long closes and reopens the
    /// device (0 disables).
    pub(crate) capture_stall_s: u32,
    /// The daemon exits (code 70/71) when its event loop or capture thread
    /// shows no progress for this long (0 disables).
    pub(crate) loop_stall_s: u32,
}

impl Default for LimitsSection {
    fn default() -> Self {
        Self {
            nice: 10,
            max_open_files: 256,
            max_rss_mb: 64,
            capture_stall_s: 5,
            loop_stall_s: 30,
        }
    }
}

/// `[pairing]`: Home Assistant sets the Noise key itself (HA 2026.9 or newer).
/// Only used when `device.noise_psk` and `device.noise_psk_file` are unset.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // TOML switches and two CLI actions
#[serde(default, deny_unknown_fields)]
pub(crate) struct PairingSection {
    /// Start without a key and let HA provision one while the window is open.
    pub(crate) enabled: bool,
    /// Directory (created with mode 0700) for the stored key `noise_psk` and
    /// the trigger files `pair` and `reset-pairing`.
    pub(crate) state_dir: PathBuf,
    /// How long the pairing window stays open, in seconds.
    pub(crate) window_s: u32,
    /// Open the window when the daemon starts without a stored key.
    pub(crate) open_on_start: bool,
    /// `--pair`: open the window at startup whatever `open_on_start` says.
    #[serde(skip)]
    pub(crate) open_now: bool,
    /// `--reset-pairing`: delete the stored key before starting.
    #[serde(skip)]
    pub(crate) reset_now: bool,
}

/// Longest accepted `pairing.window_s` (one day).
pub(crate) const MAX_PAIRING_WINDOW_S: u32 = 86_400;

impl Default for PairingSection {
    fn default() -> Self {
        Self {
            enabled: false,
            state_dir: PathBuf::from("pairing"),
            window_s: 600,
            open_on_start: true,
            open_now: false,
            reset_now: false,
        }
    }
}

impl PairingSection {
    /// The stored key (written by the daemon when HA sets it).
    pub(crate) fn key_file(&self) -> PathBuf {
        self.state_dir.join("noise_psk")
    }

    /// Creating this file opens the pairing window (the daemon deletes it).
    pub(crate) fn pair_trigger(&self) -> PathBuf {
        self.state_dir.join("pair")
    }

    /// Creating this file deletes the stored key and opens the window.
    pub(crate) fn reset_trigger(&self) -> PathBuf {
        self.state_dir.join("reset-pairing")
    }
}

/// `aec.delay_ms`: `"auto"` or a fixed delay in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) enum AecDelay {
    /// Estimate it (GCC-PHAT) at start and every 30 s.
    #[default]
    Auto,
    /// Mic behind the reference by this many ms (the hint given to AEC3).
    Fixed(f32),
}

impl<'de> Deserialize<'de> for AecDelay {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(i64),
            Float(f64),
            Text(String),
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        match Raw::deserialize(d)? {
            Raw::Int(v) => Ok(Self::Fixed(v as f32)),
            Raw::Float(v) => Ok(Self::Fixed(v as f32)),
            Raw::Text(t) if t == "auto" => Ok(Self::Auto),
            Raw::Text(t) => Err(serde::de::Error::custom(format!(
                "expected \"auto\" or a number of milliseconds, got {t:?}"
            ))),
        }
    }
}

/// Sample format of the echo reference.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefFormat {
    /// 32-bit samples (SoundEngineCapture on c3-a).
    #[default]
    S32Le,
    /// 24-bit samples in 32-bit containers.
    S24Le,
}

/// `[aec]`: echo cancellation with the TV's output as the reference
/// (docs/design/aec-frontend.md). The ALSA reference is opened on the mic's
/// card (`mic.card_id`), matched by PCM id or name.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // one bool per TOML switch
#[serde(default, deny_unknown_fields)]
pub(crate) struct AecSection {
    /// Run the echo canceller (needs a reference).
    pub(crate) enabled: bool,
    /// Substring of the reference PCM's id or name on the mic's card.
    pub(crate) reference_pcm_id: String,
    /// Reference sample format.
    pub(crate) reference_format: RefFormat,
    /// Reference channels (downmixed to mono).
    pub(crate) reference_channels: u16,
    /// Reference rate: 48000 (decimated to 16 kHz) or 16000.
    pub(crate) reference_rate: u32,
    /// Reference ALSA period size in frames.
    pub(crate) reference_period_frames: u32,
    /// Reference ALSA ring buffer size in frames.
    pub(crate) reference_buffer_frames: u32,
    /// Link mic and reference so one trigger starts both.
    pub(crate) link: bool,
    /// `"auto"` or a fixed mic-behind-reference delay in ms (0 to 500).
    pub(crate) delay_ms: AecDelay,
    /// Noise suppression after the echo canceller.
    pub(crate) ns: bool,
    /// Skip the canceller while the reference is silent (saves CPU).
    pub(crate) bypass_when_silent: bool,
    /// Digital gain on the reference in dB (-20 to 40).
    pub(crate) reference_gain_db: f32,
    /// Raw reference file for `mic.source = "file"` (read in lockstep with
    /// `mic.path`).
    pub(crate) reference_path: Option<PathBuf>,
}

impl Default for AecSection {
    fn default() -> Self {
        Self {
            enabled: false,
            reference_pcm_id: "SoundEngineCapture".to_owned(),
            reference_format: RefFormat::S32Le,
            reference_channels: 2,
            reference_rate: 48_000,
            reference_period_frames: 256,
            reference_buffer_frames: 8192,
            link: true,
            delay_ms: AecDelay::Auto,
            ns: false,
            bypass_when_silent: false,
            reference_gain_db: 0.0,
            reference_path: None,
        }
    }
}

/// Largest accepted fixed `aec.delay_ms` (sonora clamps its hint there).
pub(crate) const MAX_AEC_DELAY_MS: f32 = 500.0;

/// The permission bits of `path` if group or other have any access
/// (unreadable or non-Unix: `None`).
#[cfg(unix)]
fn loose_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
fn loose_mode(_path: &Path) -> Option<u32> {
    None
}

/// Error loading or validating the configuration.
#[derive(Debug)]
pub(crate) struct ConfigError(pub(crate) String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn err(msg: impl Into<String>) -> ConfigError {
    ConfigError(msg.into())
}

fn resolve(base: &Path, p: &mut PathBuf) {
    if p.is_relative() {
        *p = base.join(&*p);
    }
}

impl Settings {
    /// Parses TOML text. Relative paths are resolved against `base`.
    pub(crate) fn from_toml(text: &str, base: &Path) -> Result<Self, ConfigError> {
        let mut s: Self = toml::from_str(text).map_err(|e| err(format!("config: {e}")))?;
        if let Some(p) = s.device.noise_psk_file.as_mut() {
            resolve(base, p);
        }
        if let Some(p) = s.mic.path.as_mut() {
            resolve(base, p);
        }
        resolve(base, &mut s.sink.dir);
        resolve(base, &mut s.safety.kill_switch);
        resolve(base, &mut s.safety.mute_file);
        resolve(base, &mut s.safety.firmware_info);
        resolve(base, &mut s.pairing.state_dir);
        if let Some(p) = s.aec.reference_path.as_mut() {
            resolve(base, p);
        }
        Ok(s)
    }

    /// Reads and parses a TOML file.
    pub(crate) fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| err(format!("cannot read {}: {e}", path.display())))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Self::from_toml(&text, base)
    }

    /// Log level filter.
    pub(crate) fn log_level(&self) -> Result<log::LevelFilter, ConfigError> {
        self.log
            .level
            .parse()
            .map_err(|_| err(format!("log.level: unknown level {:?}", self.log.level)))
    }

    /// Checks values that the libraries would only reject later.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        self.log_level()?;
        if self.mic.rate != 16_000 {
            return Err(err(
                "mic.rate must be 16000 (Home Assistant expects 16 kHz; the front-end does not resample)",
            ));
        }
        if self.mic.channels == 0 {
            return Err(err("mic.channels must be at least 1"));
        }
        if self.mic.source == MicSource::File && self.mic.path.is_none() {
            return Err(err("mic.source = \"file\" needs mic.path"));
        }
        if !self.dsp.gain_db.is_finite() || !(-20.0..=60.0).contains(&self.dsp.gain_db) {
            return Err(err("dsp.gain_db must be between -20 and 60"));
        }
        if !self.dsp.hpf_hz.is_finite() || self.dsp.hpf_hz < 0.0 {
            return Err(err("dsp.hpf_hz must be >= 0"));
        }
        if !self.sink.gain.is_finite() || !(0.0..=4.0).contains(&self.sink.gain) {
            return Err(err("sink.gain must be between 0.0 and 4.0"));
        }
        if self.sink.rate == 0 || self.sink.channels == 0 {
            return Err(err("sink.rate and sink.channels must be non-zero"));
        }
        if self.sink.latency_msec > MAX_LATENCY_MSEC {
            return Err(err(format!(
                "sink.latency_msec must be between 0 (PulseAudio default) and {MAX_LATENCY_MSEC}"
            )));
        }
        if !(-20..=19).contains(&self.limits.nice) {
            return Err(err("limits.nice must be between -20 and 19"));
        }
        if self.limits.max_open_files != 0 && self.limits.max_open_files < 32 {
            return Err(err(
                "limits.max_open_files must be 0 (inherit) or at least 32",
            ));
        }
        if self.limits.max_rss_mb != 0 && self.limits.max_rss_mb < 8 {
            return Err(err("limits.max_rss_mb must be 0 (off) or at least 8"));
        }
        if self.limits.loop_stall_s != 0 && self.limits.loop_stall_s < 5 {
            return Err(err("limits.loop_stall_s must be 0 (off) or at least 5"));
        }
        if self.safety.kill_switch.as_os_str().is_empty()
            || self.safety.mute_file.as_os_str().is_empty()
        {
            return Err(err("safety.kill_switch and safety.mute_file must be set"));
        }
        if self.safety.kill_switch == self.safety.mute_file {
            return Err(err("safety.kill_switch and safety.mute_file must differ"));
        }
        if self.device.start_conversation && !self.device.announce {
            return Err(err(
                "device.start_conversation needs device.announce = true (HA announces first)",
            ));
        }
        if self.device.port == 0 {
            return Err(err("device.port must be between 1 and 65535"));
        }
        if self.mic.period_frames == 0 || self.mic.buffer_frames < self.mic.period_frames {
            return Err(err(
                "mic.period_frames must be non-zero and mic.buffer_frames at least as large",
            ));
        }
        if self.dsp.channel == Channel::Right && self.mic.channels < 2 {
            return Err(err("dsp.channel = \"right\" needs mic.channels >= 2"));
        }
        if self.device.noise_psk.is_some() && self.device.noise_psk_file.is_some() {
            return Err(err(
                "set only one of device.noise_psk and device.noise_psk_file",
            ));
        }
        if !self.has_fixed_key() && !self.pairing_active() && !self.device.allow_plaintext {
            return Err(err(
                "no Noise key: set device.noise_psk_file, or pairing.enabled = true to let Home Assistant set one (or device.allow_plaintext = true for development)",
            ));
        }
        if self.pairing_active() && self.device.allow_plaintext {
            return Err(err(
                "pairing.enabled and device.allow_plaintext exclude each other",
            ));
        }
        if !(1..=MAX_PAIRING_WINDOW_S).contains(&self.pairing.window_s) {
            return Err(err(format!(
                "pairing.window_s must be between 1 and {MAX_PAIRING_WINDOW_S}"
            )));
        }
        if self.pairing.state_dir.as_os_str().is_empty() {
            return Err(err("pairing.state_dir must be set"));
        }
        if (self.pairing.open_now || self.pairing.reset_now) && !self.pairing_active() {
            return Err(err(
                "--pair and --reset-pairing need pairing.enabled = true and no device.noise_psk or noise_psk_file",
            ));
        }
        self.validate_aec()?;
        self.esphome_config()?
            .validate()
            .map_err(|e| err(format!("device: {e}")))?;
        Ok(())
    }

    fn validate_aec(&self) -> Result<(), ConfigError> {
        let a = &self.aec;
        if a.reference_pcm_id.trim().is_empty() {
            return Err(err("aec.reference_pcm_id must not be empty"));
        }
        if a.reference_rate != 16_000 && a.reference_rate != 48_000 {
            return Err(err(
                "aec.reference_rate must be 48000 (decimated to 16 kHz) or 16000",
            ));
        }
        if !(1..=8).contains(&a.reference_channels) {
            return Err(err("aec.reference_channels must be between 1 and 8"));
        }
        if a.reference_period_frames == 0 || a.reference_buffer_frames < a.reference_period_frames {
            return Err(err(
                "aec.reference_period_frames must be non-zero and aec.reference_buffer_frames at least as large",
            ));
        }
        if let AecDelay::Fixed(ms) = a.delay_ms {
            if !ms.is_finite() || !(0.0..=MAX_AEC_DELAY_MS).contains(&ms) {
                return Err(err(format!(
                    "aec.delay_ms must be \"auto\" or between 0 and {MAX_AEC_DELAY_MS}"
                )));
            }
        }
        if !a.reference_gain_db.is_finite() || !(-20.0..=40.0).contains(&a.reference_gain_db) {
            return Err(err("aec.reference_gain_db must be between -20 and 40"));
        }
        if !a.enabled && (a.ns || a.bypass_when_silent) {
            return Err(err(
                "aec.ns and aec.bypass_when_silent need aec.enabled = true",
            ));
        }
        if a.enabled && self.mic.source == MicSource::File && a.reference_path.is_none() {
            return Err(err(
                "aec.enabled with mic.source = \"file\" needs aec.reference_path",
            ));
        }
        Ok(())
    }

    /// Non-fatal findings about the configuration, for the startup log and
    /// `--check-config`. Never includes secret values.
    pub(crate) fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.device.allow_plaintext && !self.has_fixed_key() && !self.pairing_active() {
            w.push("device.allow_plaintext: the API is unencrypted (development only)".to_owned());
        }
        if self.device.noise_psk.is_some() {
            w.push(
                "device.noise_psk is set inline: the config file now holds a secret; prefer device.noise_psk_file"
                    .to_owned(),
            );
        }
        if let Some(path) = &self.device.noise_psk_file {
            if let Some(mode) = loose_mode(path) {
                w.push(format!(
                    "device.noise_psk_file {} has mode {mode:04o}: group/other can read the key; run chmod 600",
                    path.display()
                ));
            }
        }
        if self.pairing.enabled && self.has_fixed_key() {
            w.push(
                "pairing.enabled is ignored: a key is set in [device] (an explicit key disables pairing)"
                    .to_owned(),
            );
        }
        if self.pairing_active() {
            let key = self.pairing.key_file();
            if let Some(mode) = loose_mode(&key) {
                w.push(format!(
                    "the stored pairing key {} has mode {mode:04o}: group/other can read it; run chmod 600",
                    key.display()
                ));
            }
        }
        if self.safety.validated_firmware.is_empty() {
            w.push("safety.validated_firmware is empty: the firmware guard is off".to_owned());
        }
        if self.mic.source == MicSource::File {
            if let Some(p) = self.mic.path.as_ref().filter(|p| !p.exists()) {
                w.push(format!("mic.path {} does not exist", p.display()));
            }
        }
        if let Some(p) = &self.aec.reference_path {
            if self.mic.source == MicSource::Alsa {
                w.push(
                    "aec.reference_path is ignored with mic.source = \"alsa\" (the ALSA reference is used)"
                        .to_owned(),
                );
            } else if self.aec.enabled && !p.exists() {
                w.push(format!("aec.reference_path {} does not exist", p.display()));
            }
        }
        if self.aec.enabled && self.aec.ns {
            w.push(
                "aec.ns is on: noise suppression has not been evaluated for wake word or STT yet"
                    .to_owned(),
            );
        }
        w
    }

    /// `device.noise_psk` or `device.noise_psk_file` is set: that key is used
    /// and pairing is off.
    pub(crate) fn has_fixed_key(&self) -> bool {
        self.device.noise_psk.is_some() || self.device.noise_psk_file.is_some()
    }

    /// Pairing is enabled and not overridden by an explicit key.
    pub(crate) fn pairing_active(&self) -> bool {
        self.pairing.enabled && !self.has_fixed_key()
    }

    /// The Noise key, read from the file if configured. Never logged.
    fn noise_psk(&self) -> Result<Option<String>, ConfigError> {
        if let Some(k) = &self.device.noise_psk {
            return Ok(Some(k.trim().to_owned()));
        }
        let Some(path) = &self.device.noise_psk_file else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(path).map_err(|e| {
            err(format!(
                "cannot read device.noise_psk_file {}: {e}",
                path.display()
            ))
        })?;
        let key = text.trim();
        if key.is_empty() {
            return Err(err(format!("{} is empty", path.display())));
        }
        Ok(Some(key.to_owned()))
    }

    /// Voice feature bits advertised to HA (`[device]` `announce`, `timers`,
    /// `start_conversation`; the default is 23).
    pub(crate) fn voice_features(&self) -> u32 {
        use webos_esphome_api::features as f;
        let d = &self.device;
        let mut bits = f::VOICE_ASSISTANT | f::SPEAKER | f::API_AUDIO;
        if d.announce {
            bits |= f::ANNOUNCE;
        }
        if d.timers {
            bits |= f::TIMERS;
        }
        if d.start_conversation {
            bits |= f::START_CONVERSATION;
        }
        bits
    }

    /// Configuration of the ESPHome API server.
    pub(crate) fn esphome_config(&self) -> Result<webos_esphome_api::Config, ConfigError> {
        let d = &self.device;
        Ok(webos_esphome_api::Config {
            name: d.name.clone(),
            friendly_name: d.friendly_name.clone(),
            mac: d.mac.clone(),
            mac_seed: d.mac_seed.clone(),
            model: d.model.clone(),
            bind_addr: d.bind,
            port: d.port,
            noise_psk: self.noise_psk()?,
            provisioning: self
                .pairing_active()
                .then(|| webos_esphome_api::Provisioning {
                    key_file: self.pairing.key_file(),
                    window: Duration::from_secs(u64::from(self.pairing.window_s)),
                    open_at_start: self.pairing.open_on_start || self.pairing.open_now,
                }),
            mdns: d.mdns,
            voice_features: self.voice_features(),
            ..webos_esphome_api::Config::default()
        })
    }

    /// Front-end configuration.
    pub(crate) fn frontend_config(&self) -> webos_dsp::FrontendConfig {
        webos_dsp::FrontendConfig {
            channel: match self.dsp.channel {
                Channel::Left => webos_dsp::ChannelMode::Left,
                Channel::Right => webos_dsp::ChannelMode::Right,
                Channel::Mix => webos_dsp::ChannelMode::Mix,
            },
            gain_db: self.dsp.gain_db,
            hpf_hz: self.dsp.hpf_hz,
        }
    }

    /// Echo canceller settings (used only when `aec.enabled`).
    pub(crate) fn aec_config(&self) -> webos_dsp::AecConfig {
        let a = &self.aec;
        webos_dsp::AecConfig {
            reference_rate: a.reference_rate,
            reference_channels: a.reference_channels,
            reference_gain_db: a.reference_gain_db,
            delay: match a.delay_ms {
                AecDelay::Auto => webos_dsp::DelayMode::Auto,
                AecDelay::Fixed(ms) => webos_dsp::DelayMode::FixedMs(ms),
            },
            apply: webos_dsp::DelayApply::Hint,
            ns: a.ns,
            bypass_when_silent: a.bypass_when_silent,
        }
    }

    /// Reference sample format for the capture crate.
    pub(crate) fn reference_format(&self) -> webos_capture::SampleFormat {
        match self.aec.reference_format {
            RefFormat::S32Le => webos_capture::SampleFormat::S32Le,
            RefFormat::S24Le => webos_capture::SampleFormat::S24Le,
        }
    }

    /// ALSA echo-reference configuration (on the mic's card).
    #[cfg(target_os = "linux")]
    pub(crate) fn reference_config(&self) -> webos_capture::ReferenceConfig {
        let a = &self.aec;
        webos_capture::ReferenceConfig {
            pcm_name: a.reference_pcm_id.clone(),
            format: self.reference_format(),
            channels: a.reference_channels,
            rate: a.reference_rate,
            period_frames: a.reference_period_frames,
            buffer_frames: a.reference_buffer_frames,
            link: a.link,
        }
    }

    /// ALSA capture configuration.
    #[cfg(target_os = "linux")]
    pub(crate) fn capture_config(&self) -> webos_capture::CaptureConfig {
        let m = &self.mic;
        webos_capture::CaptureConfig {
            card_id: m.card_id.clone(),
            pcm_name: m.pcm_name.clone(),
            device: m.device,
            rate: m.rate,
            channels: m.channels,
            period_frames: m.period_frames,
            buffer_frames: m.buffer_frames,
            discard_ms: m.discard_ms,
            read_timeout_ms: self.limits.capture_stall_s.saturating_mul(1000),
        }
    }

    /// One-line summary for the startup log (no secrets).
    pub(crate) fn summary(&self) -> String {
        let mic = match self.mic.source {
            MicSource::Alsa => format!(
                "alsa card={} pcm={}",
                self.mic.card_id,
                self.mic
                    .device
                    .map_or_else(|| format!("{:?}", self.mic.pcm_name), |d| d.to_string())
            ),
            MicSource::File => format!(
                "file realtime={} loop={} skip_ms={} pause_when_idle={}",
                self.mic.realtime, self.mic.looping, self.mic.skip_ms, self.mic.pause_when_idle
            ),
        };
        let sink = match self.sink.kind {
            SinkKind::Pacat => format!(
                "pacat gain={} latency_msec={} device={}",
                self.sink.gain,
                self.sink.latency_msec,
                self.sink.device.as_deref().unwrap_or("default")
            ),
            SinkKind::Wav => format!("wav dir={}", self.sink.dir.display()),
            SinkKind::Null => "null".to_owned(),
        };
        let l = &self.limits;
        let fw = if self.safety.validated_firmware.is_empty() {
            "off".to_owned()
        } else {
            self.safety.validated_firmware.join(",")
        };
        let encryption = if self.has_fixed_key() {
            "noise".to_owned()
        } else if self.pairing_active() {
            format!(
                "pairing({}) pairing=[state_dir={} window_s={} open_on_start={}]",
                if self.pairing.key_file().exists() {
                    "paired"
                } else {
                    "not paired"
                },
                self.pairing.state_dir.display(),
                self.pairing.window_s,
                self.pairing.open_on_start
            )
        } else {
            "PLAINTEXT".to_owned()
        };
        let a = &self.aec;
        let aec = if a.enabled {
            format!(
                "on ref={} {}/{}ch/{}Hz link={} delay_ms={} ns={} bypass={} ref_gain_db={}{}",
                a.reference_pcm_id,
                match a.reference_format {
                    RefFormat::S32Le => "s32_le",
                    RefFormat::S24Le => "s24_le",
                },
                a.reference_channels,
                a.reference_rate,
                a.link,
                match a.delay_ms {
                    AecDelay::Auto => "auto".to_owned(),
                    AecDelay::Fixed(ms) => ms.to_string(),
                },
                a.ns,
                a.bypass_when_silent,
                a.reference_gain_db,
                if self.mic.source == MicSource::File {
                    " (file reference)"
                } else {
                    ""
                }
            )
        } else {
            "off".to_owned()
        };
        format!(
            "name={} port={} voice_features={} mode={:?} encryption={encryption} mic=[{mic}] dsp=[{:?} gain_db={} hpf_hz={}] aec=[{aec}] sink=[{sink}] safety=[kill_switch={} mute_file={} firmware={fw}] limits=[nice={} nofile={} rss_mb={} capture_stall_s={} loop_stall_s={}]",
            self.device.name,
            self.device.port,
            self.voice_features(),
            self.mode,
            self.dsp.channel,
            self.dsp.gain_db,
            self.dsp.hpf_hz,
            self.safety.kill_switch.display(),
            self.safety.mute_file.display(),
            l.nice,
            l.max_open_files,
            l.max_rss_mb,
            l.capture_stall_s,
            l.loop_stall_s,
        )
    }
}

/// Which thread `--test-hang` freezes.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Hang {
    /// The tokio event loop.
    Loop,
    /// The capture thread.
    Capture,
}

/// Test aids (not for production use).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TestTriggers {
    /// Start one push-to-talk run shortly after HA connects.
    pub(crate) ptt_on_connect: bool,
    /// Start a push-to-talk run at this interval.
    pub(crate) ptt_every: Option<Duration>,
    /// Freeze a thread 5 s after startup, to exercise the watchdog.
    pub(crate) hang: Option<Hang>,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Command {
    /// Print the version.
    Version,
    /// Print usage.
    Help,
    /// Runtime self-test.
    SelfTest,
    /// Validate the configuration and print a summary.
    CheckConfig(Box<Settings>),
    /// Read the firmware version and compare it with the allow-list.
    CheckFirmware(Box<Settings>),
    /// Run the daemon.
    Run(Box<Settings>, TestTriggers),
}

pub(crate) const USAGE: &str = "\
usage: webos-satellite [--config FILE] [options]
       webos-satellite --version | --help | --self-test

Runs the Home Assistant voice satellite (ESPHome native API on TCP 6053).

options (override the config file):
  --config FILE          TOML configuration (see config/example.toml)
  --check-config         validate the configuration, print a summary, exit
  --check-firmware       compare the TV firmware with safety.validated_firmware, exit
                         (0 validated, 1 not validated or unreadable, 3 guard off)
  --mode wake|ptt        wake: continuous server-side wake word; ptt: SIGUSR1 starts a run
  --name NAME            ESPHome node name
  --friendly-name NAME   name shown in Home Assistant
  --port PORT            API port (default 6053)
  --bind ADDR            listen address (default 0.0.0.0)
  --noise-psk-file FILE  file with the base64 Noise key
  --mic alsa|file        microphone source
  --mic-file FILE        raw S24_LE file as the microphone (implies --mic file)
  --mic-skip-ms MS       skip the start of the mic file
  --gain-db DB           front-end gain
  --sink pacat|wav|null  reply sink
  --sink-gain G          software gain for pacat (0 = digital silence)
  --wav-dir DIR          output directory of the wav sink
  --log-level LEVEL      error|warn|info|debug|trace
  --kill-switch FILE     while FILE exists the mic is closed and reported muted
  --pair                 open the pairing window at startup (needs pairing.enabled)
  --reset-pairing        delete the stored pairing key, then open the window
test aids:
  --ptt-on-connect       start one push-to-talk run after HA connects
  --ptt-every SECS       start a push-to-talk run every SECS seconds
  --test-hang loop|capture  freeze that thread 5 s after start (watchdog test)

signals: SIGUSR1 starts a push-to-talk run; SIGTERM/SIGINT stop.
pairing files (in pairing.state_dir, checked every second, deleted when acted on):
  pair opens the pairing window; reset-pairing deletes the stored key and opens it.
exit codes: 0 stopped by a signal, 1 error, 2 bad arguments or config,
  70 event loop stalled, 71 capture thread stalled, 72 memory ceiling exceeded.";

fn parse_num<T: std::str::FromStr>(flag: &str, v: &str) -> Result<T, ConfigError> {
    v.parse()
        .map_err(|_| err(format!("{flag}: invalid value {v:?}")))
}

fn parse_enum<T: for<'de> Deserialize<'de>>(flag: &str, v: &str) -> Result<T, ConfigError> {
    use serde::de::value::{Error, StrDeserializer};
    T::deserialize(StrDeserializer::<Error>::new(v))
        .map_err(|_| err(format!("{flag}: invalid value {v:?}")))
}

/// Parses the command line (without the program name).
pub(crate) fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Command, ConfigError> {
    let args: Vec<String> = args.into_iter().collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => return Ok(Command::Version),
        Some("--help" | "-h") => return Ok(Command::Help),
        Some("--self-test") => return Ok(Command::SelfTest),
        _ => {}
    }

    // First pass: the config file, so overrides apply on top of it.
    let mut settings = Settings::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--config" {
            let path = it.next().ok_or_else(|| err("--config needs a value"))?;
            settings = Settings::load(Path::new(path))?;
        }
    }

    let mut check = false;
    let mut check_fw = false;
    let mut triggers = TestTriggers::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let flag = flag.as_str();
        let mut value = || {
            it.next()
                .map(String::as_str)
                .ok_or_else(|| err(format!("{flag} needs a value")))
        };
        match flag {
            "--config" => {
                value()?;
            }
            "--check-config" => check = true,
            "--check-firmware" => check_fw = true,
            "--kill-switch" => settings.safety.kill_switch = PathBuf::from(value()?),
            "--pair" => settings.pairing.open_now = true,
            "--reset-pairing" => settings.pairing.reset_now = true,
            "--mode" => settings.mode = parse_enum(flag, value()?)?,
            "--name" => value()?.clone_into(&mut settings.device.name),
            "--friendly-name" => value()?.clone_into(&mut settings.device.friendly_name),
            "--port" => settings.device.port = parse_num(flag, value()?)?,
            "--bind" => settings.device.bind = parse_num(flag, value()?)?,
            "--noise-psk-file" => {
                settings.device.noise_psk = None;
                settings.device.noise_psk_file = Some(PathBuf::from(value()?));
            }
            "--mic" => settings.mic.source = parse_enum(flag, value()?)?,
            "--mic-file" => {
                settings.mic.source = MicSource::File;
                settings.mic.path = Some(PathBuf::from(value()?));
            }
            "--mic-skip-ms" => settings.mic.skip_ms = parse_num(flag, value()?)?,
            "--gain-db" => settings.dsp.gain_db = parse_num(flag, value()?)?,
            "--sink" => settings.sink.kind = parse_enum(flag, value()?)?,
            "--sink-gain" => settings.sink.gain = parse_num(flag, value()?)?,
            "--wav-dir" => settings.sink.dir = PathBuf::from(value()?),
            "--log-level" => value()?.clone_into(&mut settings.log.level),
            "--ptt-on-connect" => triggers.ptt_on_connect = true,
            "--test-hang" => triggers.hang = Some(parse_enum(flag, value()?)?),
            "--ptt-every" => {
                let secs: f64 = parse_num(flag, value()?)?;
                if !(secs.is_finite() && secs >= 1.0) {
                    return Err(err("--ptt-every must be at least 1 second"));
                }
                triggers.ptt_every = Some(Duration::from_secs_f64(secs));
            }
            other => return Err(err(format!("unknown argument: {other}"))),
        }
    }
    settings.validate()?;
    let settings = Box::new(settings);
    Ok(if check_fw {
        Command::CheckFirmware(settings)
    } else if check {
        Command::CheckConfig(settings)
    } else {
        Command::Run(settings, triggers)
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    type Mutate = fn(&mut Settings);

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn defaults_match_c3_capture() {
        let s = Settings::default();
        assert_eq!(s.mode, Mode::Wake);
        assert_eq!(s.mic.card_id, "LGSIC");
        assert_eq!(s.mic.period_frames, 256);
        assert_eq!(s.mic.buffer_frames, 8192);
        assert_eq!(s.dsp.gain_db, 24.0);
        assert_eq!(s.sink.kind, SinkKind::Pacat);
        assert_eq!(s.device.port, 6053);
    }

    #[test]
    fn parses_full_file_and_resolves_paths() {
        let text = r#"
mode = "ptt"
[device]
name = "lg-c3-a"
friendly_name = "Living room TV"
mac_seed = "abc"
port = 6054
noise_psk_file = "private/psk"
[mic]
source = "file"
path = "rec.raw"
loop = true
skip_ms = 500
[dsp]
channel = "mix"
gain_db = 20
hpf_hz = 100
[sink]
type = "wav"
dir = "out"
[log]
level = "debug"
"#;
        let s = Settings::from_toml(text, Path::new("/etc/sat")).unwrap();
        assert_eq!(s.mode, Mode::Ptt);
        assert_eq!(s.device.name, "lg-c3-a");
        assert_eq!(s.device.port, 6054);
        assert_eq!(
            s.device.noise_psk_file,
            Some(PathBuf::from("/etc/sat/private/psk"))
        );
        assert_eq!(s.mic.source, MicSource::File);
        assert_eq!(s.mic.path, Some(PathBuf::from("/etc/sat/rec.raw")));
        assert!(s.mic.looping);
        assert_eq!(s.mic.skip_ms, 500);
        assert_eq!(s.dsp.channel, Channel::Mix);
        assert_eq!(s.sink.kind, SinkKind::Wav);
        assert_eq!(s.sink.dir, PathBuf::from("/etc/sat/out"));
        assert_eq!(s.log_level().unwrap(), log::LevelFilter::Debug);
    }

    #[test]
    fn example_file_documents_the_defaults() {
        let base = Path::new("/cfg");
        let s = Settings::from_toml(include_str!("../../../config/example.toml"), base).unwrap();
        let mut expected = Settings::default();
        expected.device.noise_psk_file = Some(base.join("noise_psk"));
        expected.sink.dir = base.join(".");
        expected.pairing.state_dir = base.join("pairing");
        assert_eq!(s, expected);
    }

    #[test]
    fn aec_section_defaults_parse_and_validate() {
        let base = Path::new("/cfg");
        let s = Settings::default();
        assert!(!s.aec.enabled);
        assert_eq!(s.aec.reference_pcm_id, "SoundEngineCapture");
        assert_eq!(s.aec.delay_ms, AecDelay::Auto);
        let c = s.aec_config();
        assert_eq!(c.apply, webos_dsp::DelayApply::Hint);
        assert_eq!(
            (
                c.reference_rate,
                c.reference_channels,
                c.ns,
                c.bypass_when_silent
            ),
            (48_000, 2, false, false)
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            s.reference_config(),
            webos_capture::ReferenceConfig::default()
        );

        let s = Settings::from_toml(
            "[mic]\nsource = \"file\"\npath = \"m.raw\"\n[aec]\nenabled = true\ndelay_ms = 17.5\nreference_path = \"r.raw\"\nreference_format = \"s24_le\"\nreference_rate = 16000\nns = true\n",
            base,
        )
        .unwrap();
        assert_eq!(s.aec.delay_ms, AecDelay::Fixed(17.5));
        assert_eq!(s.aec.reference_path, Some(base.join("r.raw")));
        assert_eq!(s.reference_format(), webos_capture::SampleFormat::S24Le);
        assert_eq!(s.aec_config().delay, webos_dsp::DelayMode::FixedMs(17.5));
        assert!(
            s.summary()
                .contains("aec=[on ref=SoundEngineCapture s24_le/2ch/16000Hz"),
            "{}",
            s.summary()
        );
        assert!(s.warnings().iter().any(|w| w.contains("aec.ns")));
        let s = Settings::from_toml("[aec]\ndelay_ms = 20\n", base).unwrap();
        assert_eq!(s.aec.delay_ms, AecDelay::Fixed(20.0));
        assert!(Settings::default().summary().contains("aec=[off]"));
        // Unknown words and keys are rejected with the key named.
        let e = Settings::from_toml("[aec]\ndelay_ms = \"soon\"\n", base).unwrap_err();
        assert!(e.0.contains("auto"), "{e}");
        let e = Settings::from_toml("[aec]\nenable = true\n", base).unwrap_err();
        assert!(e.0.contains("enable"), "{e}");
        let e = Settings::from_toml("[aec]\nreference_format = \"s16_le\"\n", base).unwrap_err();
        assert!(e.0.contains("s16_le"), "{e}");
    }

    #[test]
    fn aec_validation_names_the_key() {
        let mut s = Settings::default();
        s.device.allow_plaintext = true;
        s.validate().unwrap();
        let cases: Vec<(&str, Mutate)> = vec![
            ("aec.reference_pcm_id", |s| {
                s.aec.reference_pcm_id = " ".into();
            }),
            ("aec.reference_rate", |s| s.aec.reference_rate = 44_100),
            ("aec.reference_channels", |s| s.aec.reference_channels = 0),
            ("aec.reference_channels", |s| s.aec.reference_channels = 9),
            ("aec.reference_period_frames", |s| {
                s.aec.reference_buffer_frames = 128;
            }),
            ("aec.delay_ms", |s| s.aec.delay_ms = AecDelay::Fixed(600.0)),
            ("aec.delay_ms", |s| s.aec.delay_ms = AecDelay::Fixed(-1.0)),
            ("aec.reference_gain_db", |s| s.aec.reference_gain_db = 41.0),
            ("aec.ns", |s| s.aec.ns = true),
            ("aec.bypass_when_silent", |s| {
                s.aec.bypass_when_silent = true;
            }),
            ("aec.reference_path", |s| {
                s.aec.enabled = true;
                s.mic.source = MicSource::File;
                s.mic.path = Some(PathBuf::from("m.raw"));
            }),
        ];
        for (key, f) in cases {
            let mut bad = s.clone();
            f(&mut bad);
            let e = bad.validate().unwrap_err();
            assert!(e.0.contains(key), "{key}: {e}");
        }
        let mut ok = s.clone();
        ok.aec.enabled = true;
        ok.aec.ns = true;
        ok.aec.bypass_when_silent = true;
        ok.aec.delay_ms = AecDelay::Fixed(0.0);
        ok.validate().unwrap();
        // A file reference with the ALSA mic is ignored (warned).
        ok.aec.reference_path = Some(PathBuf::from("r.raw"));
        assert!(
            ok.warnings()
                .iter()
                .any(|w| w.contains("aec.reference_path is ignored"))
        );
    }

    #[test]
    fn sink_latency_defaults_to_200_and_0_means_pulseaudio_default() {
        let base = Path::new(".");
        assert_eq!(Settings::default().sink.latency_msec, 200);
        let mut s = Settings::from_toml("[sink]\nlatency_msec = 0\n", base).unwrap();
        s.device.allow_plaintext = true;
        assert_eq!(s.sink.latency_msec, 0);
        assert!(s.validate().is_ok(), "0 is accepted");
        assert!(s.summary().contains("latency_msec=0"));
        s.sink.latency_msec = MAX_LATENCY_MSEC;
        assert!(s.validate().is_ok());
        s.sink.latency_msec = MAX_LATENCY_MSEC + 1;
        let e = s.validate().unwrap_err().to_string();
        assert!(e.contains("sink.latency_msec"), "{e}");
        for bad in ["-1", "1.5", "\"200\""] {
            let toml = format!("[sink]\nlatency_msec = {bad}\n");
            assert!(
                Settings::from_toml(&toml, base).is_err(),
                "latency_msec = {bad} must be rejected"
            );
        }
    }

    #[test]
    fn voice_feature_keys_map_onto_the_flags() {
        use webos_esphome_api::features as f;
        assert_eq!(Settings::default().voice_features(), 23);
        assert_eq!(Settings::default().voice_features(), f::DEFAULT);
        let s = Settings::from_toml("[device]\ntimers = true\n", Path::new(".")).unwrap();
        assert_eq!(s.voice_features(), 31);
        let s = Settings::from_toml(
            "[device]\ntimers = true\nstart_conversation = true\n",
            Path::new("."),
        )
        .unwrap();
        assert_eq!(s.voice_features(), 63);
        let s =
            Settings::from_toml("[device]\nstart_conversation = true\n", Path::new(".")).unwrap();
        assert_eq!(s.voice_features(), 55);
        let s = Settings::from_toml("[device]\nannounce = false\n", Path::new(".")).unwrap();
        assert_eq!(s.voice_features(), 7);
        // The flags reach the API config, which accepts every combination.
        let mut s = s;
        s.device.allow_plaintext = true;
        s.device.timers = true;
        assert_eq!(s.esphome_config().unwrap().voice_features, 15);
        assert!(s.summary().contains("voice_features=15"));
    }

    #[test]
    fn start_conversation_requires_announce() {
        let mut s = Settings::default();
        s.device.allow_plaintext = true;
        s.device.start_conversation = true;
        s.device.announce = false;
        let e = s.validate().unwrap_err();
        assert!(e.0.contains("device.start_conversation"), "{e}");
        s.device.announce = true;
        s.validate().unwrap();
        // Typos in the new keys are still rejected.
        assert!(Settings::from_toml("[device]\ntimer = true\n", Path::new(".")).is_err());
        assert!(Settings::from_toml("[device]\ntimers = \"yes\"\n", Path::new(".")).is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        let e = Settings::from_toml("[mic]\nsorce = \"file\"\n", Path::new(".")).unwrap_err();
        assert!(e.0.contains("sorce"), "{e}");
        assert!(Settings::from_toml("modee = \"wake\"\n", Path::new(".")).is_err());
        assert!(Settings::from_toml("mode = \"always\"\n", Path::new(".")).is_err());
    }

    #[test]
    fn validation() {
        let mut s = Settings::default();
        // No key and plaintext not allowed.
        assert!(s.validate().unwrap_err().0.contains("Noise key"));
        s.device.allow_plaintext = true;
        s.validate().unwrap();

        let mut bad = s.clone();
        bad.mic.rate = 48_000;
        assert!(bad.validate().is_err());
        let mut bad = s.clone();
        bad.mic.source = MicSource::File;
        assert!(bad.validate().is_err());
        let mut bad = s.clone();
        bad.sink.gain = -1.0;
        assert!(bad.validate().is_err());
        let mut bad = s.clone();
        bad.device.name = "Bad Name".to_owned();
        assert!(bad.validate().is_err());
        let mut bad = s.clone();
        bad.log.level = "loud".to_owned();
        assert!(bad.validate().is_err());
        let mut bad = s;
        bad.device.noise_psk = Some("AAAA".to_owned());
        assert!(bad.validate().is_err(), "short key must be rejected");
    }

    #[test]
    fn validation_messages_name_the_key() {
        let mut s = Settings::default();
        s.device.allow_plaintext = true;
        let cases: [(Mutate, &str); 11] = [
            (|s| s.mic.rate = 44_100, "mic.rate"),
            (|s| s.sink.latency_msec = 10_001, "sink.latency_msec"),
            (|s| s.device.port = 0, "device.port"),
            (|s| s.mic.buffer_frames = 16, "mic.buffer_frames"),
            (
                |s| {
                    s.dsp.channel = Channel::Right;
                    s.mic.channels = 1;
                },
                "dsp.channel",
            ),
            (|s| s.dsp.hpf_hz = -1.0, "dsp.hpf_hz"),
            (|s| s.limits.nice = 20, "limits.nice"),
            (|s| s.limits.max_open_files = 8, "limits.max_open_files"),
            (|s| s.limits.max_rss_mb = 1, "limits.max_rss_mb"),
            (|s| s.limits.loop_stall_s = 1, "limits.loop_stall_s"),
            (
                |s| s.safety.mute_file = s.safety.kill_switch.clone(),
                "safety.mute_file",
            ),
        ];
        for (mutate, key) in cases {
            let mut bad = s.clone();
            mutate(&mut bad);
            let e = bad.validate().unwrap_err();
            assert!(e.0.contains(key), "{key}: {e}");
        }
        // TOML syntax and type errors carry the position and key.
        let e = Settings::from_toml("[mic]\nrate = \"fast\"\n", Path::new(".")).unwrap_err();
        assert!(e.0.contains("rate") && e.0.contains("line 2"), "{e}");
    }

    #[test]
    fn pairing_section_and_key_precedence() {
        let base = Path::new("/etc/sat");
        let s = Settings::from_toml(
            "[pairing]\nenabled = true\nstate_dir = \"state\"\nwindow_s = 120\nopen_on_start = false\n",
            base,
        )
        .unwrap();
        s.validate().unwrap();
        assert!(s.pairing_active() && !s.has_fixed_key());
        assert_eq!(
            s.pairing.key_file(),
            PathBuf::from("/etc/sat/state/noise_psk")
        );
        assert_eq!(
            s.pairing.pair_trigger(),
            PathBuf::from("/etc/sat/state/pair")
        );
        assert_eq!(
            s.pairing.reset_trigger(),
            PathBuf::from("/etc/sat/state/reset-pairing")
        );
        let esp = s.esphome_config().unwrap();
        assert!(esp.noise_psk.is_none());
        let prov = esp.provisioning.unwrap();
        assert_eq!(prov.key_file, PathBuf::from("/etc/sat/state/noise_psk"));
        assert_eq!(prov.window, Duration::from_secs(120));
        assert!(!prov.open_at_start);
        assert!(
            s.summary().contains("encryption=pairing(not paired)"),
            "{}",
            s.summary()
        );
        assert!(s.warnings().is_empty(), "{:?}", s.warnings());
        // --pair opens the window at startup even with open_on_start = false.
        let mut pair_now = s.clone();
        pair_now.pairing.open_now = true;
        assert!(
            pair_now
                .esphome_config()
                .unwrap()
                .provisioning
                .unwrap()
                .open_at_start
        );

        // An explicit key wins: pairing off, with a warning; nothing secret.
        let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let mut keyed = s.clone();
        keyed.device.noise_psk = Some(key.to_owned());
        keyed.validate().unwrap();
        assert!(!keyed.pairing_active());
        assert!(keyed.esphome_config().unwrap().provisioning.is_none());
        assert!(keyed.summary().contains("encryption=noise"));
        let w = keyed.warnings();
        assert!(
            w.iter().any(|m| m.contains("pairing.enabled is ignored")),
            "{w:?}"
        );
        assert!(!w.iter().any(|m| m.contains(key)));
        // --pair/--reset-pairing need active pairing.
        keyed.pairing.open_now = true;
        assert!(keyed.validate().unwrap_err().0.contains("--pair"));

        // Plaintext and pairing exclude each other; window bounds.
        let cases: [(Mutate, &str); 4] = [
            (|s| s.device.allow_plaintext = true, "allow_plaintext"),
            (|s| s.pairing.window_s = 0, "pairing.window_s"),
            (
                |s| s.pairing.window_s = MAX_PAIRING_WINDOW_S + 1,
                "pairing.window_s",
            ),
            (
                |s| s.pairing.state_dir = PathBuf::new(),
                "pairing.state_dir",
            ),
        ];
        for (mutate, key) in cases {
            let mut bad = s.clone();
            mutate(&mut bad);
            let e = bad.validate().unwrap_err();
            assert!(e.0.contains(key), "{key}: {e}");
        }
        // Without a key, pairing or plaintext the message names pairing.
        let e = Settings::default().validate().unwrap_err();
        assert!(e.0.contains("pairing.enabled"), "{e}");
        assert!(Settings::from_toml("[pairing]\nenabeld = true\n", base).is_err());
    }

    #[test]
    fn pairing_cli_flags() {
        let dir = std::env::temp_dir().join(format!("webos-sat-pair-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("c.toml");
        std::fs::write(&cfg, "[pairing]\nenabled = true\n").unwrap();
        let cfg = cfg.to_string_lossy().into_owned();
        let Command::Run(s, _) =
            parse_args(args(&["--config", &cfg, "--pair", "--reset-pairing"])).unwrap()
        else {
            panic!("expected Run");
        };
        assert!(s.pairing.open_now && s.pairing.reset_now);
        assert_eq!(s.pairing.state_dir, dir.join("pairing"));
        // Without pairing enabled the flags are an error.
        let plain = dir.join("p.toml");
        std::fs::write(&plain, "[device]\nallow_plaintext = true\n").unwrap();
        let plain = plain.to_string_lossy().into_owned();
        assert!(parse_args(args(&["--config", &plain, "--pair"])).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn warns_when_the_psk_file_is_readable_by_others() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("webos-sat-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let path = dir.join("psk");
        std::fs::write(&path, key).unwrap();
        let s = Settings::from_toml("[device]\nnoise_psk_file = \"psk\"\n", &dir).unwrap();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let w = s.warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("0644") && w[0].contains("chmod 600"), "{w:?}");
        assert!(!w[0].contains(key));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(s.warnings().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warns_about_plaintext_inline_key_and_missing_mic_file() {
        let mut s = Settings::default();
        s.device.allow_plaintext = true;
        assert!(s.warnings()[0].contains("unencrypted"));
        s.device.allow_plaintext = false;
        s.device.noise_psk = Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned());
        let w = s.warnings();
        assert!(w[0].contains("inline"), "{w:?}");
        assert!(!w[0].contains("AAAA"));
        s.mic.source = MicSource::File;
        s.mic.path = Some(PathBuf::from("/nonexistent/webos-sat.raw"));
        assert!(s.warnings().iter().any(|m| m.contains("does not exist")));
        s.safety.validated_firmware.clear();
        assert!(
            s.warnings()
                .iter()
                .any(|m| m.contains("firmware guard is off"))
        );
    }

    #[test]
    fn psk_file_is_read_and_trimmed() {
        let dir = std::env::temp_dir().join(format!("webos-sat-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        std::fs::write(dir.join("psk"), format!("{key}\n")).unwrap();
        let s = Settings::from_toml("[device]\nnoise_psk_file = \"psk\"\n", &dir).unwrap();
        s.validate().unwrap();
        assert_eq!(s.esphome_config().unwrap().noise_psk.as_deref(), Some(key));
        // The summary and Debug output never contain the key.
        assert!(!s.summary().contains(key));
        let mut inline = s.clone();
        inline.device.noise_psk_file = None;
        inline.device.noise_psk = Some(key.to_owned());
        assert!(!format!("{inline:?}").contains(key));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_overrides_and_triggers() {
        let cmd = parse_args(args(&[
            "--mode",
            "ptt",
            "--name",
            "webos-satellite-dev",
            "--mic-file",
            "x.raw",
            "--sink",
            "null",
            "--sink-gain",
            "0",
            "--ptt-on-connect",
            "--ptt-every",
            "5",
            "--port",
            "16053",
            "--log-level",
            "debug",
        ]));
        // No key: rejected.
        assert!(cmd.is_err());

        let dir = std::env::temp_dir().join(format!("webos-sat-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("c.toml");
        std::fs::write(&cfg, "[device]\nallow_plaintext = true\nport = 1\n").unwrap();
        let cfg = cfg.to_string_lossy().into_owned();
        let Command::Run(s, t) = parse_args(args(&[
            "--port",
            "16053",
            "--config",
            &cfg,
            "--mode",
            "ptt",
            "--mic-file",
            "x.raw",
            "--sink",
            "null",
            "--sink-gain",
            "0",
            "--ptt-on-connect",
            "--ptt-every",
            "5",
        ]))
        .unwrap() else {
            panic!("expected Run");
        };
        // Overrides win over the file regardless of their position.
        assert_eq!(s.device.port, 16053);
        assert_eq!(s.mode, Mode::Ptt);
        assert_eq!(s.mic.source, MicSource::File);
        assert_eq!(s.sink.kind, SinkKind::Null);
        assert_eq!(s.sink.gain, 0.0);
        assert!(t.ptt_on_connect);
        assert_eq!(t.ptt_every, Some(Duration::from_secs(5)));

        assert!(matches!(
            parse_args(args(&["--config", &cfg, "--check-config"])).unwrap(),
            Command::CheckConfig(_)
        ));
        let Command::CheckFirmware(s) = parse_args(args(&[
            "--config",
            &cfg,
            "--check-firmware",
            "--kill-switch",
            "/run/x.disable",
        ]))
        .unwrap() else {
            panic!("expected CheckFirmware");
        };
        assert_eq!(s.safety.kill_switch, PathBuf::from("/run/x.disable"));
        assert!(parse_args(args(&["--config", &cfg, "--bogus"])).is_err());
        assert!(parse_args(args(&["--config", &cfg, "--mode", "loud"])).is_err());
        assert!(parse_args(args(&["--config", &cfg, "--ptt-every", "0"])).is_err());
        assert!(parse_args(args(&["--config", &cfg, "--port"])).is_err());
        assert_eq!(parse_args(args(&["--version"])).unwrap(), Command::Version);
        assert_eq!(
            parse_args(args(&["--self-test"])).unwrap(),
            Command::SelfTest
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
