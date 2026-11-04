//! Messages between the server and the application (capture, playback).

use std::fmt;
use std::net::SocketAddr;

use crate::timers::TimerEvent;

/// How a pipeline run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineMode {
    /// Continuous server-side wake word (ADR 0003): stream audio to HA, which
    /// runs openWakeWord. One run per detection; the server re-arms after each
    /// run (and after reconnects) until [`AppCommand::StopPipeline`].
    WakeWord,
    /// One run that starts at speech-to-text (no wake word), e.g. a button.
    PushToTalk,
}

/// Pairing (HA-driven key provisioning, [`crate::pairing`]) changes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PairingEvent {
    /// The pairing window opened (or restarted); it closes after `seconds`.
    WindowOpened {
        /// Window length in seconds.
        seconds: u64,
    },
    /// The window closed without a key; keyless connections are now refused
    /// until it is opened again.
    WindowClosed,
    /// A key was stored and activated. Every keyless session was closed; HA
    /// reconnects with the key.
    Paired {
        /// `client_info` of the session that set the key (e.g. "Home
        /// Assistant 2026.10.4").
        client_info: String,
        /// Its address.
        peer: SocketAddr,
    },
    /// A session that proved the old key replaced it with a new one.
    KeyRotated,
    /// The stored key was deleted ([`AppCommand::ResetPairing`]); the device
    /// is unpaired again.
    Reset,
    /// The key holder cleared the key remotely (an empty `SetKey` over a
    /// session keyed with the stored key; HA sends it when its entry is
    /// deleted). The key file is deleted, the device is unpaired, and the
    /// pairing window stays **closed** (also across restarts) until it is
    /// opened locally ([`AppCommand::OpenPairingWindow`] or a reset).
    Cleared {
        /// `client_info` of the session that cleared the key.
        client_info: String,
        /// Its address.
        peer: SocketAddr,
    },
}

/// Event from the server to the application.
///
/// `Debug` is written by hand and never prints payloads (audio bytes, URLs,
/// transcripts, reply text): it shows variant names and sizes only, so
/// `{ev:?}` in a log line cannot leak them.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum AppEvent {
    /// HA completed the hello exchange.
    HaConnected {
        /// HA's `client_info` (e.g. "Home Assistant 2026.10.4").
        client_info: String,
        /// Remote address.
        peer: SocketAddr,
    },
    /// The HA connection ended (or was replaced by a newer one).
    HaDisconnected,
    /// HA accepted a pipeline run: start sending [`AppCommand::Audio`].
    StartStreaming {
        /// The run starts at the wake word stage.
        wake_word: bool,
        /// Conversation id sent with the request, if continuing one.
        conversation_id: Option<String>,
    },
    /// Stop sending audio (end of speech, error, run end, abort, mute).
    StopStreaming,
    /// HA's wake word engine detected the wake word.
    WakeWordDetected,
    /// Speech-to-text result.
    Transcript(String),
    /// Intent processing finished.
    IntentEnd {
        /// Plain-text response.
        speech: String,
        /// HA expects a follow-up utterance (handled automatically).
        continue_conversation: bool,
    },
    /// URL mode (SPEAKER feature off): play the TTS response at this URL, then
    /// send [`AppCommand::PlaybackFinished`].
    TtsUrl(String),
    /// SPEAKER mode: HA starts streaming the TTS response over the API. Play
    /// the following [`AppEvent::TtsAudio`] chunks and, after
    /// [`AppEvent::TtsStreamEnd`] and draining the buffer, send
    /// [`AppCommand::PlaybackFinished`].
    TtsStreamStart {
        /// Sample rate in Hz (16000 in HA 2026.10.4).
        sample_rate: u32,
        /// Channel count (1).
        channels: u16,
        /// Bytes per sample (2, signed little-endian).
        sample_width: u16,
    },
    /// SPEAKER mode: raw PCM chunk of the TTS response (no WAV header; HA sends
    /// 512 samples per chunk, paced about 384 ms ahead of real time).
    TtsAudio(Vec<u8>),
    /// SPEAKER mode: HA finished streaming the TTS response.
    TtsStreamEnd,
    /// Play an announcement, then send [`AppCommand::AnnounceFinished`].
    Announce {
        /// Media URL (already absolute; may be HA's ffmpeg proxy).
        url: String,
        /// Optional chime to play first.
        preannounce_url: Option<String>,
        /// Announcement text (informational).
        text: String,
        /// HA wants a pipeline run after the announcement (handled automatically).
        start_conversation: bool,
    },
    /// Stop any playback now (media player STOP, barge-in, stop command).
    StopPlayback,
    /// HA pressed stop on the media player. Sent for every stop command,
    /// even when the voice layer has no reply or announcement to stop (it
    /// then sends no [`AppEvent::StopPlayback`]); an application that plays
    /// its own sounds (a timer alarm) stops them on this.
    MediaStop,
    /// The pipeline run ended.
    RunEnd,
    /// HA reported a pipeline error.
    Error {
        /// HA error code, e.g. `wake-engine-missing`, `stt-no-text-recognized`.
        code: String,
        /// Human-readable message.
        message: String,
    },
    /// Microphone mute changed (from HA's switch or [`AppCommand::SetMuted`]).
    MuteChanged(bool),
    /// Pairing state changed. Never carries key material.
    Pairing(PairingEvent),
    /// HA changed the reply volume on the media player entity (set, or one
    /// volume step up or down), in percent (0 to 100). The server already
    /// reported the new value back to HA; the application applies and
    /// stores it.
    VolumeChanged(u8),
    /// HA chose an option of a select entity ([`crate::Config::selects`]).
    /// Only valid options are reported; the server already reported the new
    /// state back to HA.
    SelectChanged {
        /// The select's object id.
        object_id: String,
        /// The option chosen.
        option: String,
    },
    /// HA set a number entity ([`crate::Config::numbers`]). Only finite values
    /// inside the entity's range are reported (others are answered with the
    /// current state). The server does not echo the value: the application
    /// applies it and reports the resulting state with
    /// [`AppCommand::SetNumber`].
    NumberChanged {
        /// The number's object id.
        object_id: String,
        /// The value HA asked for.
        value: f32,
    },
    /// HA pressed a button entity ([`crate::Config::buttons`]).
    ButtonPressed {
        /// The button's object id.
        object_id: String,
    },
    /// HA switched a generic switch entity ([`crate::Config::switches`]). The
    /// server does not echo the value: the application acts on it and reports
    /// the resulting state with [`AppCommand::SetSwitch`].
    SwitchChanged {
        /// The switch's object id.
        object_id: String,
        /// The state HA asked for.
        state: bool,
    },
    /// A timer event from HA. Only sent when the `TIMERS` feature
    /// ([`crate::features::TIMERS`]) is enabled. HA owns and counts the timers;
    /// the device shows or sounds them (see [`crate::TimerTable`]).
    Timer {
        /// What happened.
        event: TimerEvent,
        /// HA's timer id.
        id: String,
        /// User-chosen timer name (may be empty).
        name: String,
        /// Seconds on the timer when it was created.
        total_seconds: u32,
        /// Seconds left when HA sent the event.
        seconds_left: u32,
        /// Ticking (false while paused).
        is_active: bool,
    },
}

/// Command from the application to the server.
///
/// `Debug` never prints the audio samples (sizes only).
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum AppCommand {
    /// Microphone audio: 16 kHz, signed 16-bit, mono. Forwarded only while
    /// streaming (between `StartStreaming` and `StopStreaming`), otherwise
    /// dropped. 512 samples (32 ms) per chunk is what ESPHome sends.
    Audio(Vec<i16>),
    /// Start a pipeline (see [`PipelineMode`]).
    StartPipeline {
        /// Wake word (armed, continuous) or push-to-talk (one run).
        mode: PipelineMode,
    },
    /// Disarm the wake word and abort the current run.
    StopPipeline,
    /// Playback of the TTS response ([`AppEvent::TtsUrl`] or the streamed
    /// [`AppEvent::TtsStreamStart`]..[`AppEvent::TtsStreamEnd`]) finished.
    PlaybackFinished {
        /// Playback succeeded.
        success: bool,
    },
    /// Playback of the last [`AppEvent::Announce`] finished.
    AnnounceFinished {
        /// Playback succeeded.
        success: bool,
    },
    /// Mute or unmute the microphone (reported to HA via the switch entity).
    SetMuted(bool),
    /// Whether the application's microphone capture is delivering audio right
    /// now. A host that never sends this is assumed to be capturing. While it
    /// is `false` no pipeline run is open: HA's Assist satellite cancels a
    /// running wake word pipeline before an announcement and that cancel
    /// waits for one more audio chunk, which never comes from a closed or
    /// busy microphone, so the announcement would hang. Going `false` ends
    /// the run (HA is told to abort, streaming stops) and `true` re-arms the
    /// wake word. A push-to-talk request while `false` is ignored. Send it in
    /// order with the [`AppCommand::Audio`] chunks (`true` before the first
    /// chunk, `false` when capture stops).
    CaptureLive(bool),
    /// Open (or restart) the pairing window. Ignored, with a log line, when a
    /// key is active or pairing is not enabled. A local action: it also
    /// removes the marker of a remote clear ([`PairingEvent::Cleared`]).
    OpenPairingWindow,
    /// Delete the stored key, close every session and open the pairing
    /// window. A local action. (The key holder can also clear the key
    /// remotely, but that leaves the window closed: [`PairingEvent::Cleared`].)
    ResetPairing,
    /// The reply volume, in percent (0 to 100, larger values are clamped),
    /// reported as the media player's volume. Sent by the application at
    /// startup (the stored value); not echoed as [`AppEvent::VolumeChanged`].
    SetVolume(u8),
    /// The TV power state text for the diagnostic text sensor (only listed
    /// when [`crate::Config::power_sensor`] is set).
    SetPowerState(String),
    /// The system resumed from a suspend (the process was frozen). The server
    /// probes the HA connection with a ping and drops it when HA does not
    /// answer within a few seconds, re-announces mDNS (when enabled) so HA
    /// reconnects at once, and re-arms the wake word without waiting out a
    /// retry backoff.
    Resumed,
    /// The state of a select entity ([`crate::Config::selects`]): the stored
    /// value at startup, or a local change. Not echoed as an event. An
    /// option that is not in the list is ignored with a log line.
    SetSelect {
        /// The select's object id.
        object_id: String,
        /// One of its options.
        option: String,
    },
    /// The state of a number entity ([`crate::Config::numbers`]): the actual
    /// value, at startup, after a [`AppEvent::NumberChanged`] or on a local
    /// change. `None` means no valid state (HA shows "unknown"). A value
    /// outside the entity's range is ignored with a log line.
    SetNumber {
        /// The number's object id.
        object_id: String,
        /// The value, or `None` for unknown.
        value: Option<f32>,
    },
    /// The state of a generic switch entity ([`crate::Config::switches`]).
    /// `None` means no valid state (HA shows "unknown"). Sent to HA on every
    /// change and again when HA connects or resubscribes; an unchanged state
    /// is not repeated.
    SetSwitch {
        /// The switch's object id.
        object_id: String,
        /// The state, or `None` for unknown.
        state: Option<bool>,
    },
    /// The state of a text sensor entity ([`crate::Config::text_sensors`]).
    /// `None` means no valid state (HA shows "unknown"). An unchanged state is
    /// not repeated.
    SetTextSensor {
        /// The sensor's object id.
        object_id: String,
        /// The text, or `None` for unknown.
        state: Option<String>,
    },
    /// Puts a select entity ([`crate::Config::selects`]) back to "no valid
    /// state" (HA shows "unknown").
    ClearSelect {
        /// The select's object id.
        object_id: String,
    },
    /// Replaces the options of a select entity (an app list that changed).
    /// HA reads the options when it lists the entities, so when the list
    /// differs the server **closes the HA connection** (HA reconnects at once
    /// and lists the entities again) and the select has no state until the
    /// application sends [`AppCommand::SetSelect`] again. The application
    /// decides when it is safe to interrupt (no conversation in progress).
    /// An empty list or an unknown id is ignored with a log line.
    SetSelectOptions {
        /// The select's object id.
        object_id: String,
        /// The new options, in display order.
        options: Vec<String>,
    },
    /// The state of a binary sensor entity ([`crate::Config::binary_sensors`]).
    /// `None` means no valid state (HA shows "unknown"). Sent to HA on every
    /// change and again when HA connects or resubscribes.
    SetBinarySensor {
        /// The sensor's object id.
        object_id: String,
        /// The state, or `None` for unknown.
        state: Option<bool>,
    },
}

impl fmt::Debug for AppEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HaConnected { client_info, peer } => f
                .debug_struct("HaConnected")
                .field("client_info", client_info)
                .field("peer", peer)
                .finish(),
            Self::HaDisconnected => f.write_str("HaDisconnected"),
            Self::StartStreaming {
                wake_word,
                conversation_id,
            } => f
                .debug_struct("StartStreaming")
                .field("wake_word", wake_word)
                .field(
                    "conversation_id",
                    &conversation_id.as_ref().map(|_| "<set>"),
                )
                .finish(),
            Self::StopStreaming => f.write_str("StopStreaming"),
            Self::WakeWordDetected => f.write_str("WakeWordDetected"),
            Self::Transcript(t) => write!(f, "Transcript(<{} bytes>)", t.len()),
            Self::IntentEnd {
                speech,
                continue_conversation,
            } => f
                .debug_struct("IntentEnd")
                .field("speech_bytes", &speech.len())
                .field("continue_conversation", continue_conversation)
                .finish(),
            Self::TtsUrl(u) => write!(f, "TtsUrl(<url, {} bytes>)", u.len()),
            Self::TtsStreamStart {
                sample_rate,
                channels,
                sample_width,
            } => f
                .debug_struct("TtsStreamStart")
                .field("sample_rate", sample_rate)
                .field("channels", channels)
                .field("sample_width", sample_width)
                .finish(),
            Self::TtsAudio(d) => write!(f, "TtsAudio(<{} bytes>)", d.len()),
            Self::TtsStreamEnd => f.write_str("TtsStreamEnd"),
            Self::Announce {
                url,
                preannounce_url,
                text,
                start_conversation,
            } => f
                .debug_struct("Announce")
                .field("url_bytes", &url.len())
                .field("preannounce", &preannounce_url.is_some())
                .field("text_bytes", &text.len())
                .field("start_conversation", start_conversation)
                .finish(),
            Self::StopPlayback => f.write_str("StopPlayback"),
            Self::MediaStop => f.write_str("MediaStop"),
            Self::RunEnd => f.write_str("RunEnd"),
            Self::Error { code, message } => f
                .debug_struct("Error")
                .field("code", code)
                .field("message_bytes", &message.len())
                .finish(),
            Self::MuteChanged(m) => write!(f, "MuteChanged({m})"),
            Self::Pairing(p) => f.debug_tuple("Pairing").field(p).finish(),
            Self::VolumeChanged(v) => write!(f, "VolumeChanged({v})"),
            Self::SelectChanged { object_id, option } => f
                .debug_struct("SelectChanged")
                .field("object_id", object_id)
                .field("option", option)
                .finish(),
            Self::NumberChanged { object_id, value } => f
                .debug_struct("NumberChanged")
                .field("object_id", object_id)
                .field("value", value)
                .finish(),
            Self::ButtonPressed { object_id } => f
                .debug_struct("ButtonPressed")
                .field("object_id", object_id)
                .finish(),
            Self::SwitchChanged { object_id, state } => f
                .debug_struct("SwitchChanged")
                .field("object_id", object_id)
                .field("state", state)
                .finish(),
            // The timer name is user speech: id, event and seconds only.
            Self::Timer {
                event,
                id,
                name: _,
                total_seconds,
                seconds_left,
                is_active,
            } => f
                .debug_struct("Timer")
                .field("event", event)
                .field("id", id)
                .field("total_seconds", total_seconds)
                .field("seconds_left", seconds_left)
                .field("is_active", is_active)
                .finish(),
        }
    }
}

impl fmt::Debug for AppCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio(a) => write!(f, "Audio(<{} samples>)", a.len()),
            Self::StartPipeline { mode } => {
                f.debug_struct("StartPipeline").field("mode", mode).finish()
            }
            Self::StopPipeline => f.write_str("StopPipeline"),
            Self::PlaybackFinished { success } => f
                .debug_struct("PlaybackFinished")
                .field("success", success)
                .finish(),
            Self::AnnounceFinished { success } => f
                .debug_struct("AnnounceFinished")
                .field("success", success)
                .finish(),
            Self::SetMuted(m) => write!(f, "SetMuted({m})"),
            Self::CaptureLive(l) => write!(f, "CaptureLive({l})"),
            Self::OpenPairingWindow => f.write_str("OpenPairingWindow"),
            Self::ResetPairing => f.write_str("ResetPairing"),
            Self::SetVolume(v) => write!(f, "SetVolume({v})"),
            Self::SetPowerState(s) => f.debug_tuple("SetPowerState").field(s).finish(),
            Self::Resumed => f.write_str("Resumed"),
            Self::SetSelect { object_id, option } => f
                .debug_struct("SetSelect")
                .field("object_id", object_id)
                .field("option", option)
                .finish(),
            Self::SetNumber { object_id, value } => f
                .debug_struct("SetNumber")
                .field("object_id", object_id)
                .field("value", value)
                .finish(),
            Self::SetBinarySensor { object_id, state } => f
                .debug_struct("SetBinarySensor")
                .field("object_id", object_id)
                .field("state", state)
                .finish(),
            Self::SetSwitch { object_id, state } => f
                .debug_struct("SetSwitch")
                .field("object_id", object_id)
                .field("state", state)
                .finish(),
            // A text sensor or an option may hold an app title or an input
            // label: sizes only.
            Self::SetTextSensor { object_id, state } => f
                .debug_struct("SetTextSensor")
                .field("object_id", object_id)
                .field("state_bytes", &state.as_ref().map(String::len))
                .finish(),
            Self::ClearSelect { object_id } => f
                .debug_struct("ClearSelect")
                .field("object_id", object_id)
                .finish(),
            Self::SetSelectOptions { object_id, options } => f
                .debug_struct("SetSelectOptions")
                .field("object_id", object_id)
                .field("options", &options.len())
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_contains_payloads() {
        let events = [
            AppEvent::Transcript("turn on the secret lamp".to_owned()),
            AppEvent::TtsUrl("http://h/api/tts_proxy/SECRET.flac?authSig=SECRET".to_owned()),
            AppEvent::TtsAudio(vec![0x53, 0x45, 0x43, 0x52, 0x45, 0x54]),
            AppEvent::IntentEnd {
                speech: "SECRET reply".to_owned(),
                continue_conversation: false,
            },
            AppEvent::Announce {
                url: "http://h/a?authSig=SECRET".to_owned(),
                preannounce_url: Some("http://h/b?authSig=SECRET".to_owned()),
                text: "SECRET text".to_owned(),
                start_conversation: false,
            },
            AppEvent::StartStreaming {
                wake_word: true,
                conversation_id: Some("SECRET".to_owned()),
            },
            AppEvent::Error {
                code: "stt-failed".to_owned(),
                message: "SECRET".to_owned(),
            },
            AppEvent::Timer {
                event: TimerEvent::Started,
                id: "t1".to_owned(),
                name: "SECRET".to_owned(),
                total_seconds: 60,
                seconds_left: 60,
                is_active: true,
            },
        ];
        for e in &events {
            let d = format!("{e:?}");
            assert!(!d.contains("SECRET") && !d.contains("secret"), "{d}");
            assert!(!d.contains("83, 69"), "{d}");
        }
        let d = format!("{:?}", AppCommand::Audio(vec![1234, -4321]));
        assert!(!d.contains("1234"), "{d}");
        assert!(format!("{:?}", AppEvent::TtsAudio(vec![0; 10])).contains("10 bytes"));
    }
}
