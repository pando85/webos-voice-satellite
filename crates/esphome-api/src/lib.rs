//! ESPHome native API **device-side** server for the webOS voice satellite.
//!
//! Home Assistant's ESPHome integration connects to this server (TCP 6053) and
//! creates an Assist satellite for the TV. Wake word, STT, conversation and TTS
//! run in HA ([ADR 0001], [ADR 0003]); this crate only speaks the protocol and
//! hands audio and playback work to the application through channels.
//!
//! [ADR 0001]: ../../docs/decisions/0001-esphome-native-api.md
//! [ADR 0003]: ../../docs/decisions/0003-server-side-wake-word-for-mvp.md
//!
//! # Protocol level
//!
//! - Messages: ESPHome 2026.10.1 `api.proto` (vendored, MIT), checked against
//!   HA 2026.10.4 / aioesphomeapi 46.6.0.
//! - Hello reports API **1.14** ([`API_VERSION_MINOR`]): aioesphomeapi 46.6.0
//!   reads `voice_assistant_feature_flags` from device info for 1.10..1.14 and
//!   only expects `DeviceCapabilities` from 1.15 on (answered anyway).
//! - Framing: plaintext (no key configured) or Noise
//!   `Noise_NNpsk0_25519_ChaChaPoly_SHA256` (key configured; plaintext clients
//!   are then refused with the "requires encryption" signal HA understands).
//! - Pairing ([`pairing`], optional): with [`Config::provisioning`] the device
//!   starts without a key and HA 2026.9+ sets one over a zero-PSK Noise
//!   session (`NoiseEncryptionSetKeyRequest`) while a timed pairing window is
//!   open. Keyless sessions get hello, device info, an empty entity list,
//!   ping and `SetKey` only; voice messages are never served on them.
//! - Voice features ([`config::features`], default
//!   `VOICE_ASSISTANT | SPEAKER | API_AUDIO | ANNOUNCE`): microphone audio and,
//!   with SPEAKER, the TTS reply travel over the API connection. Without SPEAKER
//!   HA sends a TTS URL instead ([`AppEvent::TtsUrl`]).
//! - Timers: with [`features::TIMERS`] enabled (off by default), HA's timer events
//!   arrive as [`AppEvent::Timer`]; [`TimerTable`] keeps their state.
//! - Entities: a media player whose single ANNOUNCEMENT format (default
//!   flac/22050/mono/16-bit) is HA's TTS format in URL mode and the format of
//!   proxied announcement media, and a "Mute microphone" switch. The
//!   application can add `select`, `number`, `button` and `binary_sensor`
//!   entities ([`Config::selects`], [`Config::numbers`], [`Config::buttons`],
//!   [`Config::binary_sensors`]) with stable object ids and keys; states go
//!   out with [`AppCommand::SetSelect`], [`AppCommand::SetNumber`] and
//!   [`AppCommand::SetBinarySensor`], HA's commands arrive as
//!   [`AppEvent::SelectChanged`], [`AppEvent::NumberChanged`] and
//!   [`AppEvent::ButtonPressed`].
//!
//! # Usage
//!
//! ```no_run
//! use webos_esphome_api::{AppCommand, AppEvent, Config, PipelineMode, Server};
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let server = Server::bind(Config::default()).await?;
//! let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
//! let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<AppCommand>(64);
//! cmd_tx.send(AppCommand::StartPipeline { mode: PipelineMode::WakeWord }).await?;
//! tokio::spawn(server.run(ev_tx, cmd_rx));
//! while let Some(ev) = ev_rx.recv().await {
//!     // StartStreaming -> send AppCommand::Audio chunks until StopStreaming;
//!     // TtsStreamStart/TtsAudio/TtsStreamEnd or TtsUrl -> play, then
//!     // AppCommand::PlaybackFinished; Announce -> play, then AnnounceFinished.
//!     let _ = ev;
//! }
//! # Ok(()) }
//! ```
//!
//! # Connection handling
//!
//! One HA client at a time. Each new TCP connection performs its handshake
//! (Noise + `HelloRequest`) in a separate task; when it completes it
//! **replaces** the current connection (HA reconnecting after a network drop
//! must not be locked out by a half-dead socket; with Noise only key holders
//! get that far). Keyless pairing sessions never replace it: each runs in its
//! own restricted task (at most four) and is closed when a key is activated,
//! the pairing is reset or the window closes. Unknown messages are ignored. HA pings every 20 s; the server
//! pings after 60 s of silence and drops the link after 150 s.
//!
//! # Voice state machine
//!
//! ```text
//!            subscribe + armed (WakeWord)            HA: VoiceAssistantResponse
//!   Idle ──────────────────────────────▶ Starting ─────────────────────────▶ Running
//!    ▲  ▲  StartPipeline(PushToTalk)       │  ▲   (any pipeline event also     │ streaming
//!    │  └──────────────── (one-shot) ──────┘  │    counts as accepted)         │ audio ▶ HA
//!    │                                        │                                │
//!    │ not armed / muted        re-arm (wake) │                STT_VAD_END /   │
//!    │                     or follow-up (no   │                STT_END: stop   │
//!    │                     wake, same conv.)  │                audio (end=1)   ▼
//!    │                                        │       TTS_END(url) / TTS_STREAM_START..END
//!    │                                        │       → app plays → PlaybackFinished
//!    │                                        │       RUN_END ──┐
//!    │                                        └── success ◀─────┤ both done
//!    │       Waiting(backoff) ◀── error or no progress ◀────────┘
//!    │            │ timer
//!    └────────────┘ (re-arm)
//!
//!   Announce request (any state): abort run → Announcing → app plays →
//!   AnnounceFinished → start_conversation ? run without wake word : re-arm.
//! ```
//!
//! - `armed` is set by `StartPipeline(WakeWord)` and cleared by `StopPipeline`;
//!   it survives reconnects. Mute aborts the run and blocks starts until unmuted.
//! - A run "succeeds" when HA reported `WAKE_WORD_END` or `STT_END` and no
//!   `ERROR`. Otherwise the next start waits 250 ms × 2ⁿ (2 s × 2ⁿ for
//!   configuration errors such as `wake-engine-missing`), capped at 60 s, so a
//!   broken pipeline never hot-loops. HA's start reply must arrive within 10 s.
//! - The conversation id from `INTENT_END` is reused for 300 s; with
//!   `continue_conversation=1` the next run starts at STT (no wake word).
//! - Microphone audio is never streamed during playback (no AEC in the MVP).
//! - URL mode: the device sends `VoiceAssistantAnnounceFinished` after TTS
//!   playback (HA uses it to finish the response). SPEAKER mode: HA streams
//!   16 kHz mono s16le PCM between `TTS_STREAM_START`/`TTS_STREAM_END` and
//!   finishes the response itself; the device only waits for
//!   `PlaybackFinished` before re-arming. If no stream starts within 2 s of
//!   `TTS_END`, the run is finished anyway.

#![cfg_attr(
    test,
    allow(clippy::assert_is_empty, clippy::field_reassign_with_default)
)]

pub mod app;
pub mod config;
pub mod entities;
pub mod frame;
mod mdns;
pub mod pairing;
pub mod proto;
mod server;
pub mod timers;
pub mod varint;
mod voice;

pub use app::{AppCommand, AppEvent, PairingEvent, PipelineMode};
pub use config::{
    AudioFormat, BinarySensorEntity, ButtonEntity, Config, ConfigError, Mac, NumberEntity,
    SelectEntity, features,
};
pub use frame::{NOISE_PREAMBLE, PLAINTEXT_PREAMBLE};
pub use pairing::Provisioning;
pub use server::{Server, ServerError};
pub use timers::{Timer, TimerEvent, TimerTable};

/// Default TCP port of the ESPHome native API.
pub const DEFAULT_PORT: u16 = 6053;
/// API major version reported in `HelloResponse`.
pub const API_VERSION_MAJOR: u32 = 1;
/// API minor version reported in `HelloResponse` (see crate docs).
pub const API_VERSION_MINOR: u32 = 14;
/// `esphome_version` reported in device info and mDNS: the ESPHome release
/// whose `api.proto` this crate implements (not an ESPHome firmware).
pub const ESPHOME_VERSION: &str = "2026.10.1";
/// `manufacturer` reported in device info.
pub const MANUFACTURER: &str = "webos-voice-satellite";
