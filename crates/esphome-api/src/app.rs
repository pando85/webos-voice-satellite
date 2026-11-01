//! Messages between the server and the application (capture, playback).

use std::net::SocketAddr;

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

/// Event from the server to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

/// Command from the application to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}
