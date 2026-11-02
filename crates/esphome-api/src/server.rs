//! TCP server: accepts HA, runs the handshake and dispatches messages.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use prost::Message as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::{Duration, Instant, sleep_until, timeout};

use crate::app::{AppCommand, AppEvent, PairingEvent};
use crate::config::{Config, ConfigError, Resolved, features};
use crate::entities;
use crate::frame::{
    self, Codec, FrameError, HANDSHAKE_MAC_FAILURE, NoiseResponder, RawMessage, decode_noise_frame,
    encode_noise_frame,
};
use crate::pairing::{self, KeyState, OpenResult, Origin, Pairing, Transport, ZERO_PSK};
use crate::proto::{self as pb, ApiMessage};
use crate::timers;
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
/// Concurrent keyless (pairing) sessions. HA uses two: its plaintext main
/// connection and the short zero-PSK session that sets the key.
const MAX_KEYLESS: usize = 4;
const READ_CHUNK: usize = 8192;

/// Error starting the server.
#[derive(Debug)]
pub enum ServerError {
    /// Invalid configuration.
    Config(ConfigError),
    /// Socket error.
    Io(io::Error),
    /// The stored pairing key cannot be read or is corrupt. The daemon does
    /// not fall back to an open pairing window: reset the pairing explicitly.
    KeyFile(PathBuf, io::Error),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "invalid configuration: {err}"),
            Self::Io(err) => write!(f, "i/o error: {err}"),
            Self::KeyFile(path, err) => write!(
                f,
                "cannot use the stored pairing key {}: {err} (reset the pairing to start over)",
                path.display()
            ),
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
    pairing: Pairing,
}

impl Server {
    /// Validates `config` and binds the TCP listener.
    ///
    /// # Errors
    ///
    /// Invalid configuration or a socket error (e.g. port in use).
    /// With [`Config::provisioning`], the stored key is read here: a missing
    /// file means unpaired.
    ///
    /// # Errors
    ///
    /// Invalid configuration, an unreadable or corrupt stored key, or a
    /// socket error (e.g. port in use).
    pub async fn bind(config: Config) -> Result<Self, ServerError> {
        let cfg = config.resolve().map_err(ServerError::Config)?;
        let (state, window) = match (&cfg.psk, &cfg.provisioning) {
            (Some(psk), _) => (KeyState::Fixed(*psk), Duration::ZERO),
            (None, Some(p)) => match pairing::load_key(&p.key_file) {
                Ok(Some(key)) => (KeyState::Paired(key), p.window),
                Ok(None) => (KeyState::Unpaired { window_until: None }, p.window),
                Err(err) => return Err(ServerError::KeyFile(p.key_file.clone(), err)),
            },
            (None, None) => (KeyState::Plaintext, Duration::ZERO),
        };
        let listener = TcpListener::bind((config.bind_addr, config.port)).await?;
        match &state {
            KeyState::Plaintext => log::warn!(
                "esphome-api: no noise_psk configured; serving PLAINTEXT (development only)"
            ),
            KeyState::Paired(_) => log::info!("esphome-api: paired (stored key)"),
            KeyState::Unpaired { .. } => log::warn!(
                "esphome-api: NOT PAIRED: no key yet; keyless sessions get device info and SetKey only"
            ),
            KeyState::Fixed(_) => {}
        }
        let pairing = Pairing::new(state, window, cfg.provisioning.is_some());
        Ok(Self {
            listener,
            cfg: Arc::new(cfg),
            pairing,
        })
    }

    /// Pairing is enabled and no key is active yet: the application must keep
    /// the microphone closed until [`PairingEvent::Paired`].
    #[must_use]
    pub fn needs_pairing(&self) -> bool {
        self.pairing.needs_pairing()
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
        let (key_tx, mut key_rx) = mpsc::channel::<KeyRequest>(MAX_KEYLESS);
        let mut ctx = Ctx {
            voice: Voice::new(self.cfg.voice_features & features::SPEAKER != 0),
            cfg: Arc::clone(&self.cfg),
            events,
            session: None,
            epoch: watch::channel(self.pairing.epoch()).0,
            pairing: self.pairing,
            mdns: None,
            bind,
        };
        let mut pending: JoinSet<Result<Session, (SocketAddr, FrameError)>> = JoinSet::new();
        let mut keyless: JoinSet<()> = JoinSet::new();
        log::info!("esphome-api: listening on {bind}");
        if ctx.pairing.needs_pairing()
            && ctx
                .cfg
                .provisioning
                .as_ref()
                .is_some_and(|p| p.open_at_start)
        {
            ctx.open_window(Instant::now());
        } else if ctx.pairing.needs_pairing() {
            log::warn!(
                "esphome-api: pairing window closed; open it to pair (keyless connections are refused)"
            );
        }
        ctx.refresh_mdns();

        loop {
            ctx.drain_buffered().await;
            ctx.flush().await;
            let deadline = ctx.next_deadline();
            tokio::select! {
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, peer)) if pending.len() < MAX_PENDING => {
                        log::debug!("esphome-api: connection from {peer}");
                        pending.spawn(handshake(
                            stream,
                            peer,
                            Arc::clone(&self.cfg),
                            ctx.pairing.transport(),
                            ctx.pairing.epoch(),
                        ));
                    }
                    Ok((_, peer)) => log::warn!("esphome-api: too many pending handshakes; dropping {peer}"),
                    Err(err) => {
                        log::warn!("esphome-api: accept failed: {err}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                Some(joined) = pending.join_next(), if !pending.is_empty() => match joined {
                    Ok(Ok(session)) if session.epoch != ctx.pairing.epoch() => log::info!(
                        "esphome-api: dropping {}: the pairing state changed during its handshake",
                        session.peer
                    ),
                    Ok(Ok(session)) if session.kind.keyless() => {
                        if keyless.len() < MAX_KEYLESS {
                            keyless.spawn(serve_keyless(
                                session,
                                keyless_device_info(&self.cfg),
                                ctx.epoch.subscribe(),
                                key_tx.clone(),
                            ));
                        } else {
                            log::warn!(
                                "esphome-api: too many keyless sessions; dropping {}",
                                session.peer
                            );
                        }
                    }
                    Ok(Ok(session)) => ctx.adopt(session),
                    Ok(Err((peer, err))) => log::info!("esphome-api: handshake with {peer} failed: {err}"),
                    Err(err) => log::warn!("esphome-api: handshake task failed: {err}"),
                },
                Some(_) = keyless.join_next(), if !keyless.is_empty() => {}
                Some(req) = key_rx.recv() => ctx.on_key_request(req).await,
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

/// How a session was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    /// Noise with the active key.
    Authenticated,
    /// Plaintext development mode (no key, no pairing): full access.
    DevPlaintext,
    /// Unpaired, plaintext: hello, device info and an empty entity list.
    KeylessPlaintext,
    /// Unpaired, zero-PSK Noise: as plaintext, plus `SetKey`.
    KeylessNoise,
}

impl SessionKind {
    fn keyless(self) -> bool {
        matches!(self, Self::KeylessPlaintext | Self::KeylessNoise)
    }

    fn origin(self) -> Origin {
        match self {
            Self::Authenticated => Origin::Authenticated,
            Self::DevPlaintext => Origin::DevPlaintext,
            Self::KeylessPlaintext => Origin::KeylessPlaintext,
            Self::KeylessNoise => Origin::ZeroPsk,
        }
    }
}

/// One HA connection after the handshake. Only `Authenticated` and
/// `DevPlaintext` sessions become the server's main session (voice, entities);
/// keyless ones are served by [`serve_keyless`].
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
    kind: SessionKind,
    /// [`Pairing::epoch`] when the connection was accepted.
    epoch: u64,
}

impl Session {
    fn new(stream: TcpStream, peer: SocketAddr, epoch: u64) -> Self {
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
            kind: SessionKind::DevPlaintext,
            epoch,
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
    transport: Transport,
    epoch: u64,
) -> Result<Session, (SocketAddr, FrameError)> {
    match timeout(
        HANDSHAKE_TIMEOUT,
        handshake_inner(stream, peer, &cfg, transport, epoch),
    )
    .await
    {
        Ok(res) => res.map_err(|e| (peer, e)),
        Err(_) => Err((peer, FrameError::Timeout)),
    }
}

async fn handshake_inner(
    stream: TcpStream,
    peer: SocketAddr,
    cfg: &Resolved,
    transport: Transport,
    epoch: u64,
) -> Result<Session, FrameError> {
    stream.set_nodelay(true)?;
    let mut s = Session::new(stream, peer, epoch);
    match transport {
        Transport::Noise(psk) => {
            noise_handshake(&mut s, &psk, cfg).await?;
            s.kind = SessionKind::Authenticated;
        }
        Transport::Plaintext => s.kind = SessionKind::DevPlaintext,
        Transport::Keyless { .. } => {
            // Unpaired: plaintext or a Noise hello, told apart by the first
            // byte as ESPHome does. Noise runs with the all-zero PSK.
            while s.rbuf.is_empty() {
                s.fill().await?;
            }
            if s.rbuf[0] == frame::NOISE_PREAMBLE {
                noise_handshake(&mut s, &ZERO_PSK, cfg).await?;
                s.kind = SessionKind::KeylessNoise;
            } else {
                s.kind = SessionKind::KeylessPlaintext;
            }
        }
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
        "esphome-api: hello from {peer}: {:?} (API {}.{}){}",
        hello.client_info,
        hello.api_version_major,
        hello.api_version_minor,
        match s.kind {
            SessionKind::KeylessPlaintext => " [keyless plaintext, pairing]",
            SessionKind::KeylessNoise => " [keyless zero-PSK Noise, pairing]",
            _ => "",
        }
    );
    s.send(&pb::HelloResponse {
        api_version_major: API_VERSION_MAJOR,
        api_version_minor: API_VERSION_MINOR,
        server_info: format!("webos-voice-satellite {}", env!("CARGO_PKG_VERSION")),
        name: cfg.name.clone(),
    })
    .await?;
    s.client_info = hello.client_info;
    if transport == (Transport::Keyless { open: false }) {
        // As ESPHome's `provisioning:` after its window: acknowledge the hello
        // (the client learns the name), then disconnect with the reason HA
        // shows as "closed its setup window".
        log::warn!("esphome-api: {peer} refused: the pairing window is closed");
        s.send(&pb::DisconnectRequest {
            reason: pb::DisconnectReason::ProvisioningClosed as i32,
        })
        .await?;
        return Err(FrameError::Protocol("pairing window closed"));
    }
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
    pairing: Pairing,
    /// Publishes [`Pairing::epoch`]; keyless sessions close when it changes.
    epoch: watch::Sender<u64>,
    mdns: Option<crate::mdns::Advertiser>,
    bind: SocketAddr,
}

enum Flow {
    Continue,
    Close,
    /// A key was activated on this session: reply sent, now close everything.
    KeyChanged,
}

/// A `SetKey` from a keyless session, decided by the server loop (which owns
/// the key state). Deliberately not `Debug`, so it cannot be logged.
struct KeyRequest {
    key: Vec<u8>,
    origin: Origin,
    peer: SocketAddr,
    client_info: String,
    reply: oneshot::Sender<bool>,
}

/// Serves one keyless (pairing) session: hello is done; answers device info,
/// an empty entity list, ping, disconnect and `SetKey`. Everything else,
/// voice in particular, is ignored. Ends when the client leaves, after
/// [`DEAD_AFTER`] of silence, or when the pairing epoch changes (key
/// activated, reset, window closed), with a `DisconnectRequest`.
async fn serve_keyless(
    mut s: Session,
    info: pb::DeviceInfoResponse,
    mut epoch: watch::Receiver<u64>,
    keys: mpsc::Sender<KeyRequest>,
) {
    let reason: String = 'session: loop {
        loop {
            let msg = match s.codec.decode(&mut s.rbuf) {
                Ok(Some(msg)) => msg,
                Ok(None) => break,
                Err(err) => break 'session err.to_string(),
            };
            match keyless_message(&mut s, &msg, &info, &keys).await {
                Ok(Flow::Continue | Flow::KeyChanged) => {}
                Ok(Flow::Close) => break 'session "disconnect requested".to_owned(),
                Err(err) => break 'session err.to_string(),
            }
        }
        if *epoch.borrow_and_update() != s.epoch {
            let _ = s.send(&pb::DisconnectRequest::default()).await;
            break "pairing state changed".to_owned();
        }
        let dead_at = s.last_rx + DEAD_AFTER;
        tokio::select! {
            read = s.fill() => if let Err(err) = read {
                break err.to_string();
            },
            changed = epoch.changed() => if changed.is_err() {
                break "server stopped".to_owned();
            },
            () = sleep_until(dead_at) => break "idle timeout".to_owned(),
        }
    };
    log::info!("esphome-api: keyless session {} closed: {reason}", s.peer);
}

async fn keyless_message(
    s: &mut Session,
    msg: &RawMessage,
    info: &pb::DeviceInfoResponse,
    keys: &mpsc::Sender<KeyRequest>,
) -> Result<Flow, FrameError> {
    let name = pb::message_name(msg.type_id).unwrap_or("unknown");
    log::trace!("esphome-api: <- {name} (keyless)");
    match msg.type_id {
        pb::DisconnectRequest::ID => {
            let _ = s.send(&pb::DisconnectResponse::default()).await;
            return Ok(Flow::Close);
        }
        pb::DisconnectResponse::ID => return Ok(Flow::Close),
        pb::PingRequest::ID => s.send(&pb::PingResponse::default()).await?,
        pb::DeviceInfoRequest::ID => s.send(info).await?,
        // HA lists entities before it provisions the key; the list is empty
        // until a key is active.
        pb::ListEntitiesRequest::ID => s.send(&pb::ListEntitiesDoneResponse::default()).await?,
        pb::NoiseEncryptionSetKeyRequest::ID => {
            let key = decode::<pb::NoiseEncryptionSetKeyRequest>(msg)
                .map(|r| r.key)
                .unwrap_or_default();
            let (reply, answer) = oneshot::channel();
            let req = KeyRequest {
                key,
                origin: s.kind.origin(),
                peer: s.peer,
                client_info: s.client_info.clone(),
                reply,
            };
            let success = keys.send(req).await.is_ok() && answer.await.unwrap_or(false);
            s.send(&pb::NoiseEncryptionSetKeyResponse { success })
                .await?;
        }
        pb::SubscribeVoiceAssistantRequest::ID
        | pb::VoiceAssistantResponse::ID
        | pb::VoiceAssistantEventResponse::ID
        | pb::VoiceAssistantAudio::ID
        | pb::VoiceAssistantAnnounceRequest::ID
        | pb::VoiceAssistantConfigurationRequest::ID
        | pb::VoiceAssistantSetConfiguration::ID
        | pb::VoiceAssistantTimerEventResponse::ID
        | pb::SwitchCommandRequest::ID
        | pb::MediaPlayerCommandRequest::ID => {
            log::info!("esphome-api: {name} from {} ignored: not paired", s.peer);
        }
        _ => log::debug!("esphome-api: {name} ignored on a keyless session"),
    }
    Ok(Flow::Continue)
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
        // Invariant: a keyless session never becomes the main session (no
        // voice, entities or audio without a key).
        if session.kind.keyless() || self.pairing.needs_pairing() {
            log::error!(
                "esphome-api: refusing to adopt an unauthenticated session from {}",
                session.peer
            );
            return;
        }
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
        if let Some(window) = self.pairing.deadline() {
            deadline = deadline.min(window);
        }
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
        if self.pairing.on_tick(now) {
            log::warn!(
                "esphome-api: pairing window closed without a key; keyless connections are refused until it is opened again"
            );
            self.epoch.send_replace(self.pairing.epoch());
            self.refresh_mdns();
            self.emit(AppEvent::Pairing(PairingEvent::WindowClosed));
        }
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
                Ok(Flow::KeyChanged) => {
                    self.after_key_change(PairingEvent::KeyRotated).await;
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

/// Device info for keyless (pairing) sessions: identity only, no voice
/// features (no satellite before a key is active), and the provisioning
/// flags HA 2026.9+ looks for.
fn keyless_device_info(cfg: &Resolved) -> pb::DeviceInfoResponse {
    pb::DeviceInfoResponse {
        voice_assistant_feature_flags: 0,
        api_encryption_supported: true,
        api_encryption_provisionable: true,
        ..device_info(cfg, false)
    }
}

/// Device info for the main session. `has_key`: a key is active (fixed or
/// paired), so HA already holds it and never tries to provision one.
fn device_info(cfg: &Resolved, has_key: bool) -> pb::DeviceInfoResponse {
    pb::DeviceInfoResponse {
        name: cfg.name.clone(),
        mac_address: cfg.mac.pretty(),
        esphome_version: ESPHOME_VERSION.to_owned(),
        model: cfg.model.clone(),
        manufacturer: MANUFACTURER.to_owned(),
        friendly_name: cfg.friendly_name.clone(),
        voice_assistant_feature_flags: cfg.voice_features,
        // False in plaintext development mode, so HA never tries to provision
        // a key there (no key storage without pairing).
        api_encryption_supported: has_key,
        api_encryption_provisionable: false,
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
                s.send(&device_info(&self.cfg, self.pairing.has_key()))
                    .await?;
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
                    // api.proto defines VOICE_ASSISTANT_SUBSCRIBE_API_AUDIO = 1,
                    // but aioesphomeapi 46.6.0 (HA 2026.10.4) sends
                    // `VoiceAssistantSubscriptionFlag.API_AUDIO = 1 << 2`.
                    if req.subscribe && req.flags & (1 | 4) == 0 {
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
                let key = decode::<pb::NoiseEncryptionSetKeyRequest>(msg)
                    .map(|r| r.key)
                    .unwrap_or_default();
                let (origin, peer) = (s.kind.origin(), s.peer);
                let (success, activated) = self.apply_set_key(&key, origin, peer, now);
                let Some(s) = self.session.as_mut() else {
                    return Ok(Flow::Close);
                };
                s.send(&pb::NoiseEncryptionSetKeyResponse { success })
                    .await?;
                if activated {
                    return Ok(Flow::KeyChanged);
                }
            }
            pb::HelloRequest::ID
            | pb::PingResponse::ID
            | pb::SubscribeLogsRequest::ID
            | pb::SubscribeHomeassistantServicesRequest::ID
            | pb::SubscribeHomeAssistantStatesRequest::ID
            | pb::GetTimeResponse::ID => {}
            pb::VoiceAssistantTimerEventResponse::ID => {
                // HA only registers a timer handler when TIMERS is advertised.
                if self.cfg.voice_features & features::TIMERS == 0 {
                    log::debug!("esphome-api: timer event ignored (TIMERS not advertised)");
                } else if let Some(ev) = decode::<pb::VoiceAssistantTimerEventResponse>(msg)
                    .and_then(timers::to_app_event)
                {
                    self.emit(ev);
                } else {
                    log::debug!("esphome-api: undecodable timer event ignored");
                }
            }
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
                // Never any microphone audio while unpaired (belt and braces:
                // without a main session the voice layer cannot stream).
                if !self.voice.is_streaming() || self.pairing.needs_pairing() {
                    return;
                }
                let Some(s) = self.session.as_mut() else {
                    return;
                };
                // Exact capacity: `flat_map().collect()` has no size hint and
                // would regrow the buffer about eight times per chunk.
                let mut data = Vec::with_capacity(samples.len() * 2);
                data.extend(samples.iter().flat_map(|v| v.to_le_bytes()));
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
            AppCommand::OpenPairingWindow => self.open_window(now),
            AppCommand::ResetPairing => self.reset_pairing(now).await,
        }
    }
}

/// Pairing: window, `SetKey`, activation and reset.
impl Ctx {
    fn key_file(&self) -> Option<&std::path::Path> {
        self.cfg.provisioning.as_ref().map(|p| p.key_file.as_path())
    }

    fn open_window(&mut self, now: Instant) {
        match self.pairing.open_window(now) {
            OpenResult::Opened(d) => {
                log::warn!(
                    "esphome-api: PAIRING WINDOW OPEN for {} s: add this device in Home Assistant (2026.9 or newer sets the key)",
                    d.as_secs()
                );
                self.refresh_mdns();
                self.emit(AppEvent::Pairing(PairingEvent::WindowOpened {
                    seconds: d.as_secs(),
                }));
            }
            OpenResult::AlreadyPaired => log::warn!(
                "esphome-api: pairing window not opened: already paired (reset the pairing first)"
            ),
            OpenResult::Disabled => log::warn!(
                "esphome-api: pairing window not opened: pairing is not enabled (fixed key or plaintext mode)"
            ),
        }
    }

    /// Checks and stores a key. Returns (reply success, key activated). Never
    /// logs the key.
    fn apply_set_key(
        &mut self,
        key: &[u8],
        origin: Origin,
        peer: SocketAddr,
        now: Instant,
    ) -> (bool, bool) {
        let key = match self.pairing.check_set_key(key, origin, now) {
            Err(why) => {
                log::warn!("esphome-api: SetKey from {peer} refused: {why}");
                return (false, false);
            }
            Ok(None) => {
                log::info!("esphome-api: SetKey from {peer}: that key is already active");
                return (true, false);
            }
            Ok(Some(key)) => key,
        };
        let Some(path) = self.key_file() else {
            return (false, false);
        };
        if let Err(err) = pairing::store_key(path, &key) {
            log::error!(
                "esphome-api: SetKey from {peer}: cannot store the key in {}: {err}",
                path.display()
            );
            return (false, false);
        }
        log::warn!(
            "esphome-api: key set by {peer} and stored in {} (mode 0600); closing sessions opened before it",
            path.display()
        );
        self.pairing.activate(key);
        (true, true)
    }

    async fn on_key_request(&mut self, req: KeyRequest) {
        let (success, activated) =
            self.apply_set_key(&req.key, req.origin, req.peer, Instant::now());
        // The keyless task sends the reply before it sees the new epoch.
        let _ = req.reply.send(success);
        if activated {
            self.after_key_change(PairingEvent::Paired {
                client_info: req.client_info,
                peer: req.peer,
            })
            .await;
        }
    }

    /// After a key change or reset: every session opened before it is closed
    /// (keyless ones through the epoch, the main one here) and mDNS updated.
    async fn after_key_change(&mut self, ev: PairingEvent) {
        self.epoch.send_replace(self.pairing.epoch());
        if let Some(s) = self.session.as_mut() {
            let _ = s.send(&pb::DisconnectRequest::default()).await;
        }
        self.close("the key changed");
        self.refresh_mdns();
        self.emit(AppEvent::Pairing(ev));
    }

    async fn reset_pairing(&mut self, now: Instant) {
        let Some(path) = self.key_file().map(std::path::Path::to_path_buf) else {
            log::warn!("esphome-api: reset-pairing ignored: pairing is not enabled");
            return;
        };
        match pairing::remove_key(&path) {
            Ok(existed) => log::warn!(
                "esphome-api: PAIRING RESET: stored key {} ({})",
                if existed { "deleted" } else { "was absent" },
                path.display()
            ),
            Err(err) => {
                log::error!(
                    "esphome-api: reset-pairing failed: cannot delete {}: {err}; the key stays active",
                    path.display()
                );
                return;
            }
        }
        if !self.pairing.reset(now) {
            return;
        }
        self.after_key_change(PairingEvent::Reset).await;
        let seconds = self
            .pairing
            .deadline()
            .map_or(0, |d| d.saturating_duration_since(now).as_secs());
        log::warn!(
            "esphome-api: PAIRING WINDOW OPEN for {seconds} s (remove the old device in Home Assistant before re-adding it)"
        );
        self.emit(AppEvent::Pairing(PairingEvent::WindowOpened { seconds }));
    }

    /// (Re)starts the mDNS advertisement for the current key state: with
    /// `api_encryption` when a key is active, without it while the window is
    /// open, none while unpaired with the window closed.
    fn refresh_mdns(&mut self) {
        if !self.cfg.mdns {
            return;
        }
        self.mdns = None;
        let advertise = !self.pairing.needs_pairing() || self.pairing.window_open();
        if advertise {
            self.mdns = crate::mdns::advertise(
                &self.cfg,
                self.bind.ip(),
                self.bind.port(),
                self.pairing.has_key(),
            );
        }
    }
}
