//! TCP server: accepts HA, runs the handshake and dispatches messages.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use prost::Message as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{Duration, Instant, sleep_until, timeout};

use crate::app::{AppCommand, AppEvent};
use crate::config::{Config, ConfigError, Resolved, features};
use crate::entities;
use crate::frame::{
    self, Codec, FrameError, HANDSHAKE_MAC_FAILURE, NoiseResponder, RawMessage, decode_noise_frame,
    encode_noise_frame,
};
use crate::proto::{self as pb, ApiMessage};
use crate::voice::{FLAG_USE_WAKE_WORD, Out, ToHa, Voice};
use crate::{API_VERSION_MAJOR, API_VERSION_MINOR, ESPHOME_VERSION, MANUFACTURER};

/// Time allowed for the transport handshake plus `HelloRequest`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A blocked socket write for this long drops the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Send a ping after this much silence from HA (HA itself pings every 20 s).
const PING_AFTER: Duration = Duration::from_secs(60);
/// Drop the connection after this much silence (2.5 x 60 s, as ESPHome).
const DEAD_AFTER: Duration = Duration::from_secs(150);
/// Concurrent connections still in the handshake.
const MAX_PENDING: usize = 4;
const READ_CHUNK: usize = 8192;

/// Error starting the server.
#[derive(Debug)]
pub enum ServerError {
    /// Invalid configuration.
    Config(ConfigError),
    /// Socket error.
    Io(io::Error),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "invalid configuration: {err}"),
            Self::Io(err) => write!(f, "i/o error: {err}"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<io::Error> for ServerError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// ESPHome native API server for one HA client at a time.
pub struct Server {
    listener: TcpListener,
    cfg: Arc<Resolved>,
}

impl Server {
    /// Validates `config` and binds the TCP listener.
    ///
    /// # Errors
    ///
    /// Invalid configuration or a socket error (e.g. port in use).
    pub async fn bind(config: Config) -> Result<Self, ServerError> {
        let cfg = config.resolve().map_err(ServerError::Config)?;
        let listener = TcpListener::bind((config.bind_addr, config.port)).await?;
        if cfg.psk.is_none() {
            log::warn!(
                "esphome-api: no noise_psk configured; serving PLAINTEXT (development only)"
            );
        }
        Ok(Self {
            listener,
            cfg: Arc::new(cfg),
        })
    }

    /// Address the server listens on.
    ///
    /// # Errors
    ///
    /// If the socket address cannot be read.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serves HA until `commands` is closed (all senders dropped).
    ///
    /// Events go to `events`; it is unbounded so the server never blocks on a
    /// slow application. `commands` should be bounded so microphone audio gets
    /// back-pressure.
    ///
    /// # Errors
    ///
    /// Currently never; per-connection errors only drop that connection.
    pub async fn run(
        self,
        events: mpsc::UnboundedSender<AppEvent>,
        mut commands: mpsc::Receiver<AppCommand>,
    ) -> io::Result<()> {
        let bind = self.listener.local_addr()?;
        let _mdns = if self.cfg.mdns {
            crate::mdns::advertise(&self.cfg, bind.ip(), bind.port())
        } else {
            None
        };
        let mut ctx = Ctx {
            voice: Voice::new(self.cfg.voice_features & features::SPEAKER != 0),
            cfg: Arc::clone(&self.cfg),
            events,
            session: None,
        };
        let mut pending: JoinSet<Result<Session, (SocketAddr, FrameError)>> = JoinSet::new();
        log::info!("esphome-api: listening on {bind}");

        loop {
            ctx.drain_buffered().await;
            ctx.flush().await;
            let deadline = ctx.next_deadline();
            tokio::select! {
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, peer)) if pending.len() < MAX_PENDING => {
                        log::debug!("esphome-api: connection from {peer}");
                        pending.spawn(handshake(stream, peer, Arc::clone(&self.cfg)));
                    }
                    Ok((_, peer)) => log::warn!("esphome-api: too many pending handshakes; dropping {peer}"),
                    Err(err) => {
                        log::warn!("esphome-api: accept failed: {err}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                Some(joined) = pending.join_next(), if !pending.is_empty() => match joined {
                    Ok(Ok(session)) => ctx.adopt(session),
                    Ok(Err((peer, err))) => log::info!("esphome-api: handshake with {peer} failed: {err}"),
                    Err(err) => log::warn!("esphome-api: handshake task failed: {err}"),
                },
                read = fill_opt(ctx.session.as_mut()) => {
                    if let Err(err) = read {
                        ctx.close(&err.to_string());
                    }
                }
                cmd = commands.recv() => {
                    let Some(cmd) = cmd else {
                        if let Some(s) = ctx.session.as_mut() {
                            let _ = s.send(&pb::DisconnectRequest::default()).await;
                        }
                        ctx.close("server shutting down");
                        ctx.flush().await;
                        return Ok(());
                    };
                    ctx.command(cmd).await;
                }
                () = sleep_until(deadline) => ctx.tick().await,
            }
        }
    }
}

async fn fill_opt(session: Option<&mut Session>) -> Result<(), FrameError> {
    match session {
        Some(s) => s.fill().await,
        None => std::future::pending().await,
    }
}

/// One authenticated HA connection.
struct Session {
    rd: OwnedReadHalf,
    wr: OwnedWriteHalf,
    rbuf: Vec<u8>,
    tmp: Box<[u8]>,
    wbuf: Vec<u8>,
    codec: Codec,
    peer: SocketAddr,
    client_info: String,
    last_rx: Instant,
    ping_sent: bool,
    states_subscribed: bool,
}

impl Session {
    fn new(stream: TcpStream, peer: SocketAddr) -> Self {
        let (rd, wr) = stream.into_split();
        Self {
            rd,
            wr,
            rbuf: Vec::new(),
            tmp: vec![0_u8; READ_CHUNK].into_boxed_slice(),
            wbuf: Vec::new(),
            codec: Codec::Plaintext,
            peer,
            client_info: String::new(),
            last_rx: Instant::now(),
            ping_sent: false,
            states_subscribed: false,
        }
    }

    /// Reads more bytes into the buffer (cancel safe).
    async fn fill(&mut self) -> Result<(), FrameError> {
        let n = self.rd.read(&mut self.tmp).await?;
        if n == 0 {
            return Err(FrameError::Closed);
        }
        self.rbuf.extend_from_slice(&self.tmp[..n]);
        self.last_rx = Instant::now();
        self.ping_sent = false;
        Ok(())
    }

    async fn read_noise_frame(&mut self) -> Result<Vec<u8>, FrameError> {
        loop {
            if let Some((body, used)) = decode_noise_frame(&self.rbuf)? {
                let body = body.to_vec();
                self.rbuf.drain(..used);
                return Ok(body);
            }
            self.fill().await?;
        }
    }

    async fn read_message(&mut self) -> Result<RawMessage, FrameError> {
        loop {
            if let Some(msg) = self.codec.decode(&mut self.rbuf)? {
                return Ok(msg);
            }
            self.fill().await?;
        }
    }

    async fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), FrameError> {
        match timeout(WRITE_TIMEOUT, self.wr.write_all(bytes)).await {
            Ok(res) => res.map_err(FrameError::Io),
            Err(_) => Err(FrameError::Timeout),
        }
    }

    async fn send_raw(&mut self, type_id: u16, payload: &[u8]) -> Result<(), FrameError> {
        let mut wbuf = std::mem::take(&mut self.wbuf);
        wbuf.clear();
        let res = match self.codec.encode(type_id, payload, &mut wbuf) {
            Ok(()) => self.write_bytes(&wbuf).await,
            Err(err) => Err(err),
        };
        self.wbuf = wbuf;
        res
    }

    async fn send<M: ApiMessage>(&mut self, msg: &M) -> Result<(), FrameError> {
        log::trace!("esphome-api: -> {}", M::NAME);
        self.send_raw(M::ID, &msg.encode_to_vec()).await
    }
}

/// Transport handshake and hello, run in its own task per connection.
async fn handshake(
    stream: TcpStream,
    peer: SocketAddr,
    cfg: Arc<Resolved>,
) -> Result<Session, (SocketAddr, FrameError)> {
    match timeout(HANDSHAKE_TIMEOUT, handshake_inner(stream, peer, &cfg)).await {
        Ok(res) => res.map_err(|e| (peer, e)),
        Err(_) => Err((peer, FrameError::Timeout)),
    }
}

async fn handshake_inner(
    stream: TcpStream,
    peer: SocketAddr,
    cfg: &Resolved,
) -> Result<Session, FrameError> {
    stream.set_nodelay(true)?;
    let mut s = Session::new(stream, peer);
    if let Some(psk) = cfg.psk {
        noise_handshake(&mut s, &psk, cfg).await?;
    }
    let msg = match s.read_message().await {
        Err(FrameError::BadPreamble(frame::NOISE_PREAMBLE)) => {
            // A Noise client against a plaintext device: answer with a
            // plaintext preamble so aioesphomeapi reports "device is using
            // plaintext" instead of timing out.
            s.write_bytes(b"\x00Bad indicator byte").await?;
            return Err(FrameError::BadPreamble(frame::NOISE_PREAMBLE));
        }
        other => other?,
    };
    if msg.type_id != pb::HelloRequest::ID {
        return Err(FrameError::Protocol("first message is not HelloRequest"));
    }
    let hello = pb::HelloRequest::decode(msg.payload.as_slice())
        .map_err(|_| FrameError::Protocol("bad HelloRequest"))?;
    log::info!(
        "esphome-api: hello from {peer}: {:?} (API {}.{})",
        hello.client_info,
        hello.api_version_major,
        hello.api_version_minor
    );
    s.send(&pb::HelloResponse {
        api_version_major: API_VERSION_MAJOR,
        api_version_minor: API_VERSION_MINOR,
        server_info: format!("webos-voice-satellite {}", env!("CARGO_PKG_VERSION")),
        name: cfg.name.clone(),
    })
    .await?;
    s.client_info = hello.client_info;
    Ok(s)
}

async fn noise_handshake(
    s: &mut Session,
    psk: &[u8; 32],
    cfg: &Resolved,
) -> Result<(), FrameError> {
    let mut out = Vec::new();
    // Client hello: an (empty) frame. A plaintext client is told to encrypt.
    match s.read_noise_frame().await {
        Err(FrameError::BadPreamble(frame::PLAINTEXT_PREAMBLE)) => {
            encode_noise_frame(
                &frame::handshake_reject_body("Bad indicator byte"),
                &mut out,
            )?;
            s.write_bytes(&out).await?;
            return Err(FrameError::BadPreamble(frame::PLAINTEXT_PREAMBLE));
        }
        other => other?,
    };
    encode_noise_frame(
        &frame::server_hello_body(&cfg.name, &cfg.mac.compact()),
        &mut out,
    )?;
    s.write_bytes(&out).await?;
    out.clear();
    let body = s.read_noise_frame().await?;
    let mut responder = NoiseResponder::new(psk)?;
    match responder.respond(&body) {
        Ok(reply) => {
            encode_noise_frame(&reply, &mut out)?;
            s.write_bytes(&out).await?;
        }
        Err(err) => {
            encode_noise_frame(
                &frame::handshake_reject_body(HANDSHAKE_MAC_FAILURE),
                &mut out,
            )?;
            s.write_bytes(&out).await?;
            return Err(err);
        }
    }
    s.codec = Codec::Noise(Box::new(responder.into_transport()?));
    Ok(())
}

/// Server state shared by all handlers.
struct Ctx {
    cfg: Arc<Resolved>,
    voice: Voice,
    events: mpsc::UnboundedSender<AppEvent>,
    session: Option<Session>,
}

enum Flow {
    Continue,
    Close,
}

fn decode<M: ApiMessage>(msg: &RawMessage) -> Option<M> {
    match M::decode(msg.payload.as_slice()) {
        Ok(m) => Some(m),
        Err(err) => {
            log::warn!("esphome-api: cannot decode {}: {err}", M::NAME);
            None
        }
    }
}

impl Ctx {
    fn emit(&self, ev: AppEvent) {
        log::debug!("esphome-api: event {ev:?}");
        let _ = self.events.send(ev);
    }

    fn adopt(&mut self, session: Session) {
        if self.session.is_some() {
            self.close("replaced by a new connection");
        }
        log::info!("esphome-api: HA connected from {}", session.peer);
        self.emit(AppEvent::HaConnected {
            client_info: session.client_info.clone(),
            peer: session.peer,
        });
        self.session = Some(session);
    }

    fn close(&mut self, why: &str) {
        if let Some(s) = self.session.take() {
            log::info!("esphome-api: HA connection {} closed: {why}", s.peer);
            self.voice.on_disconnect();
            self.emit(AppEvent::HaDisconnected);
        }
    }

    fn next_deadline(&self) -> Instant {
        let far = Instant::now() + Duration::from_secs(3600);
        let mut deadline = self.voice.next_deadline().unwrap_or(far);
        if let Some(s) = &self.session {
            let keepalive = if s.ping_sent {
                s.last_rx + DEAD_AFTER
            } else {
                s.last_rx + PING_AFTER
            };
            deadline = deadline.min(keepalive);
        }
        deadline
    }

    async fn tick(&mut self) {
        let now = Instant::now();
        self.voice.on_tick(now);
        let mut dead = false;
        if let Some(s) = self.session.as_mut() {
            let idle = now.saturating_duration_since(s.last_rx);
            if idle >= DEAD_AFTER {
                dead = true;
            } else if idle >= PING_AFTER && !s.ping_sent {
                s.ping_sent = true;
                dead = s.send(&pb::PingRequest::default()).await.is_err();
            }
        }
        if dead {
            self.close("keepalive timeout");
        }
    }

    /// Handles complete messages already in the read buffer.
    async fn drain_buffered(&mut self) {
        loop {
            let Some(s) = self.session.as_mut() else {
                return;
            };
            let msg = match s.codec.decode(&mut s.rbuf) {
                Ok(Some(msg)) => msg,
                Ok(None) => return,
                Err(err) => {
                    self.close(&err.to_string());
                    return;
                }
            };
            match self.handle(&msg).await {
                Ok(Flow::Continue) => {}
                Ok(Flow::Close) => {
                    self.close("disconnect requested");
                    return;
                }
                Err(err) => {
                    self.close(&err.to_string());
                    return;
                }
            }
            self.flush().await;
        }
    }

    /// Sends the voice state machine's outputs.
    async fn flush(&mut self) {
        loop {
            let outs = self.voice.take();
            if outs.is_empty() {
                return;
            }
            for out in outs {
                match out {
                    Out::App(ev) => self.emit(ev),
                    Out::Ha(msg) => {
                        if let Err(err) = self.send_to_ha(msg).await {
                            self.close(&err.to_string());
                        }
                    }
                }
            }
        }
    }

    async fn send_to_ha(&mut self, msg: ToHa) -> Result<(), FrameError> {
        let Some(s) = self.session.as_mut() else {
            return Ok(());
        };
        match msg {
            ToHa::Start {
                wake_word,
                conversation_id,
            } => {
                log::info!("esphome-api: requesting pipeline (wake_word={wake_word})");
                s.send(&pb::VoiceAssistantRequest {
                    start: true,
                    conversation_id,
                    flags: if wake_word { FLAG_USE_WAKE_WORD } else { 0 },
                    audio_settings: Some(pb::VoiceAssistantAudioSettings {
                        noise_suppression_level: 0,
                        auto_gain: 0,
                        volume_multiplier: 1.0,
                    }),
                    wake_word_phrase: String::new(),
                })
                .await
            }
            ToHa::Abort => {
                s.send(&pb::VoiceAssistantRequest {
                    start: false,
                    ..Default::default()
                })
                .await
            }
            ToHa::AudioEnd => {
                s.send(&pb::VoiceAssistantAudio {
                    end: true,
                    ..Default::default()
                })
                .await
            }
            ToHa::AnnounceFinished(success) => {
                s.send(&pb::VoiceAssistantAnnounceFinished { success })
                    .await
            }
            ToHa::MediaState(state) if s.states_subscribed => {
                s.send(&entities::media_player_state(state)).await
            }
            ToHa::MuteState(muted) if s.states_subscribed => {
                s.send(&entities::mute_state(muted)).await
            }
            ToHa::MediaState(_) | ToHa::MuteState(_) => Ok(()),
        }
    }
}

fn device_info(cfg: &Resolved) -> pb::DeviceInfoResponse {
    pb::DeviceInfoResponse {
        name: cfg.name.clone(),
        mac_address: cfg.mac.pretty(),
        esphome_version: ESPHOME_VERSION.to_owned(),
        model: cfg.model.clone(),
        manufacturer: MANUFACTURER.to_owned(),
        friendly_name: cfg.friendly_name.clone(),
        voice_assistant_feature_flags: cfg.voice_features,
        // True only when a key is configured: HA then already holds the key
        // and never tries to provision one (no key storage here).
        api_encryption_supported: cfg.psk.is_some(),
        ..Default::default()
    }
}

impl Ctx {
    #[allow(clippy::too_many_lines)] // one flat dispatch table
    async fn handle(&mut self, msg: &RawMessage) -> Result<Flow, FrameError> {
        let now = Instant::now();
        let Some(s) = self.session.as_mut() else {
            return Ok(Flow::Close);
        };
        log::trace!(
            "esphome-api: <- {}",
            pb::message_name(msg.type_id).unwrap_or("unknown")
        );
        match msg.type_id {
            pb::DisconnectRequest::ID => {
                let _ = s.send(&pb::DisconnectResponse::default()).await;
                return Ok(Flow::Close);
            }
            pb::DisconnectResponse::ID => return Ok(Flow::Close),
            pb::PingRequest::ID => s.send(&pb::PingResponse::default()).await?,
            pb::DeviceInfoRequest::ID => {
                s.send(&device_info(&self.cfg)).await?;
            }
            pb::DeviceCapabilitiesRequest::ID => {
                s.send(&pb::DeviceCapabilitiesResponse {
                    voice_assistant: Some(pb::VoiceAssistantCapabilities {
                        feature_flags: self.cfg.voice_features,
                    }),
                    ..Default::default()
                })
                .await?;
            }
            pb::ListEntitiesRequest::ID => {
                s.send(&entities::list_media_player(&self.cfg)).await?;
                s.send(&entities::list_mute_switch()).await?;
                s.send(&pb::ListEntitiesDoneResponse::default()).await?;
            }
            pb::SubscribeStatesRequest::ID => {
                s.states_subscribed = true;
                s.send(&entities::media_player_state(self.voice.media_state()))
                    .await?;
                s.send(&entities::mute_state(self.voice.is_muted())).await?;
            }
            pb::SubscribeVoiceAssistantRequest::ID => {
                if let Some(req) = decode::<pb::SubscribeVoiceAssistantRequest>(msg) {
                    if req.subscribe && req.flags & 1 == 0 {
                        log::warn!(
                            "esphome-api: HA subscribed without API_AUDIO; audio needs the API path"
                        );
                    }
                    log::info!(
                        "esphome-api: voice subscribe={} flags={}",
                        req.subscribe,
                        req.flags
                    );
                    self.voice.on_subscribe(now, req.subscribe);
                }
            }
            pb::VoiceAssistantResponse::ID => {
                if let Some(r) = decode::<pb::VoiceAssistantResponse>(msg) {
                    self.voice.on_response(now, r.port, r.error);
                }
            }
            pb::VoiceAssistantEventResponse::ID => {
                if let Some(e) = decode::<pb::VoiceAssistantEventResponse>(msg) {
                    let data: Vec<(String, String)> =
                        e.data.into_iter().map(|d| (d.name, d.value)).collect();
                    self.voice.on_event(now, e.event_type, &data);
                }
            }
            pb::VoiceAssistantAudio::ID => {
                if let Some(a) = decode::<pb::VoiceAssistantAudio>(msg) {
                    self.voice.on_tts_audio(a.data);
                }
            }
            pb::VoiceAssistantAnnounceRequest::ID => {
                if let Some(a) = decode::<pb::VoiceAssistantAnnounceRequest>(msg) {
                    self.voice.on_announce(
                        a.media_id,
                        a.preannounce_media_id,
                        a.text,
                        a.start_conversation,
                    );
                }
            }
            pb::VoiceAssistantConfigurationRequest::ID => {
                // Wake word runs in HA (ADR 0003): no on-device wake words.
                s.send(&pb::VoiceAssistantConfigurationResponse {
                    available_wake_words: Vec::new(),
                    active_wake_words: Vec::new(),
                    max_active_wake_words: 0,
                })
                .await?;
            }
            pb::VoiceAssistantSetConfiguration::ID => {
                if let Some(c) = decode::<pb::VoiceAssistantSetConfiguration>(msg) {
                    log::info!(
                        "esphome-api: HA set active wake words {:?} (ignored)",
                        c.active_wake_words
                    );
                }
            }
            pb::SwitchCommandRequest::ID => {
                if let Some(c) = decode::<pb::SwitchCommandRequest>(msg) {
                    if c.key == entities::mute_key() {
                        self.voice.on_mute(now, c.state);
                    }
                }
            }
            pb::MediaPlayerCommandRequest::ID => {
                if let Some(c) = decode::<pb::MediaPlayerCommandRequest>(msg) {
                    if c.has_command && c.command == pb::MediaPlayerCommand::Stop as i32 {
                        self.voice.on_media_stop();
                    } else {
                        log::info!("esphome-api: unsupported media player command ignored");
                    }
                }
            }
            pb::NoiseEncryptionSetKeyRequest::ID => {
                log::warn!("esphome-api: HA tried to set an encryption key; not supported");
                s.send(&pb::NoiseEncryptionSetKeyResponse { success: false })
                    .await?;
            }
            pb::HelloRequest::ID
            | pb::PingResponse::ID
            | pb::SubscribeLogsRequest::ID
            | pb::SubscribeHomeassistantServicesRequest::ID
            | pb::SubscribeHomeAssistantStatesRequest::ID
            | pb::VoiceAssistantTimerEventResponse::ID
            | pb::GetTimeResponse::ID => {}
            other => log::debug!(
                "esphome-api: ignoring message {other} ({})",
                pb::message_name(other).unwrap_or("unknown")
            ),
        }
        Ok(Flow::Continue)
    }

    async fn command(&mut self, cmd: AppCommand) {
        let now = Instant::now();
        match cmd {
            AppCommand::Audio(samples) => {
                if !self.voice.is_streaming() {
                    return;
                }
                let Some(s) = self.session.as_mut() else {
                    return;
                };
                let data: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
                let msg = pb::VoiceAssistantAudio {
                    data,
                    ..Default::default()
                };
                if let Err(err) = s.send(&msg).await {
                    self.close(&err.to_string());
                }
            }
            AppCommand::StartPipeline { mode } => self.voice.on_start_pipeline(now, mode),
            AppCommand::StopPipeline => self.voice.on_stop_pipeline(),
            AppCommand::PlaybackFinished { success } => {
                self.voice.on_playback_finished(now, success);
            }
            AppCommand::AnnounceFinished { success } => {
                self.voice.on_announce_finished(now, success);
            }
            AppCommand::SetMuted(muted) => self.voice.on_mute(now, muted),
        }
    }
}
