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
use crate::pairing::{self, KeyChange, KeyState, OpenResult, Origin, Pairing, Transport, ZERO_PSK};
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
/// Closed pairing window: how long to wait after the `HelloResponse` for the
/// client's next message before the `DisconnectRequest(PROVISIONING_CLOSED)`
/// is sent anyway. HA's config flow sends `DeviceInfoRequest` at once.
const CLOSED_WINDOW_WAIT: Duration = Duration::from_secs(2);
/// After our own `DisconnectRequest`: how long to wait for the client's
/// `DisconnectResponse` (or its close) before the socket is dropped.
const DISCONNECT_LINGER: Duration = Duration::from_secs(1);
/// After a key change or clear on the main session: how long its client may
/// still disconnect by itself (HA does right after the `SetKey` reply) before
/// the server sends `DisconnectRequest`. ESPHome waits 100 ms.
const KEY_CHANGE_GRACE: Duration = Duration::from_millis(500);
/// After [`AppCommand::Resumed`]: the HA connection is dropped when HA does
/// not answer the probe ping within this time (HA answers a ping at once; a
/// connection that HA closed during the suspend gets no answer or a reset).
const RESUME_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// Unpaired because the key holder cleared the key
    /// ([`pairing::cleared_marker`]): the window is not opened at start.
    cleared: bool,
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
        let cleared = matches!(state, KeyState::Unpaired { .. })
            && cfg
                .provisioning
                .as_ref()
                .is_some_and(|p| pairing::was_cleared(&p.key_file));
        let pairing = Pairing::new(state, window, cfg.provisioning.is_some());
        Ok(Self {
            listener,
            cfg: Arc::new(cfg),
            pairing,
            cleared,
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
            volume: 100,
            power_state: None,
            selects: vec![None; self.cfg.selects.len()],
            numbers: vec![None; self.cfg.numbers.len()],
            binary_sensors: vec![None; self.cfg.binary_sensors.len()],
            switches: vec![None; self.cfg.switches.len()],
            text_sensors: vec![None; self.cfg.text_sensors.len()],
        };
        let mut pending: JoinSet<Result<Session, (SocketAddr, FrameError)>> = JoinSet::new();
        let mut keyless: JoinSet<()> = JoinSet::new();
        log::info!("esphome-api: listening on {bind}");
        ctx.window_at_start(self.cleared);
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
    /// A probe ping after a resume must be answered by then.
    probe_until: Option<Instant>,
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
            probe_until: None,
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
        self.probe_until = None;
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
        log::warn!("esphome-api: {peer} refused: the pairing window is closed");
        refuse_closed_window(&mut s).await?;
        return Err(FrameError::Protocol("pairing window closed"));
    }
    Ok(s)
}

/// As ESPHome's `provisioning:` after its window: the hello was acknowledged
/// (the client learns the name); now disconnect with the reason HA shows as
/// "closed its setup window".
///
/// ESPHome sends `DisconnectRequest` right behind the `HelloResponse`. That
/// races in aioesphomeapi 46.6.0 (HA 2026.10.4): it reports the reason
/// (`ConnectionClosedEvent`, which HA's config flow checks) only if the
/// connection is already `CONNECTED`, which happens when `connect()` resumes
/// after the `HelloResponse`. When both frames arrive in one read (HA in a
/// container behind a router, c3-a P3), the disconnect is handled first and
/// HA shows `connection_error`. So the request waits for the client's next
/// message (HA's flow sends `DeviceInfoRequest` once `connect()` returned) or
/// [`CLOSED_WINDOW_WAIT`] for a client that sends nothing.
async fn refuse_closed_window(s: &mut Session) -> Result<(), FrameError> {
    match timeout(CLOSED_WINDOW_WAIT, s.read_message()).await {
        Ok(Ok(msg)) if msg.type_id == pb::DisconnectRequest::ID => {
            let _ = s.send(&pb::DisconnectResponse::default()).await;
            return Ok(());
        }
        Ok(Ok(msg)) => log::debug!(
            "esphome-api: {} sent {} on a closed window; disconnecting",
            s.peer,
            pb::message_name(msg.type_id).unwrap_or("unknown")
        ),
        Ok(Err(err)) => return Err(err),
        Err(_) => log::debug!("esphome-api: {} sent nothing after hello", s.peer),
    }
    s.send(&pb::DisconnectRequest {
        reason: pb::DisconnectReason::ProvisioningClosed as i32,
    })
    .await?;
    linger(s).await;
    Ok(())
}

/// After our `DisconnectRequest`: waits up to [`DISCONNECT_LINGER`] for the
/// client's `DisconnectResponse` or close, so the socket is not closed with
/// unread data (which would reset the connection).
async fn linger(s: &mut Session) {
    let _ = timeout(DISCONNECT_LINGER, async {
        loop {
            match s.read_message().await {
                Ok(msg) if msg.type_id == pb::DisconnectResponse::ID => return,
                Ok(_) => {}
                Err(_) => return,
            }
        }
    })
    .await;
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
    /// Reply volume in percent, reported as the media player's volume.
    volume: u8,
    /// Text of the TV power state sensor (`None` until the first state).
    power_state: Option<String>,
    /// State of each select in `cfg.selects` (same order).
    selects: Vec<Option<String>>,
    /// State of each number in `cfg.numbers` (same order); `None` = unknown.
    numbers: Vec<Option<f32>>,
    /// State of each binary sensor in `cfg.binary_sensors` (same order).
    binary_sensors: Vec<Option<bool>>,
    /// State of each generic switch in `cfg.switches` (same order).
    switches: Vec<Option<bool>>,
    /// State of each text sensor in `cfg.text_sensors` (same order).
    text_sensors: Vec<Option<String>>,
}

enum Flow {
    Continue,
    Close,
    /// The key was replaced or cleared on this session: reply sent; retire
    /// this session ([`retire`]), close the others and report the event.
    KeyChanged(PairingEvent),
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
                Ok(Flow::Continue | Flow::KeyChanged(_)) => {}
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

/// Ends the main session after its client changed or cleared the key. HA
/// disconnects by itself right after the `SetKey` reply (`cli.disconnect()`
/// sends `DisconnectRequest` and waits for the response); answering that,
/// instead of racing it with our own request, keeps HA's log free of
/// "disconnect request failed". Only disconnect and ping are answered; after
/// [`KEY_CHANGE_GRACE`] the server sends `DisconnectRequest` itself. Nothing
/// else is served: the session is no longer the main session.
async fn retire(mut s: Session) {
    let until = Instant::now() + KEY_CHANGE_GRACE;
    let reason: String = loop {
        match s.codec.decode(&mut s.rbuf) {
            Ok(Some(msg)) => {
                match msg.type_id {
                    pb::DisconnectRequest::ID => {
                        let _ = s.send(&pb::DisconnectResponse::default()).await;
                        break "client disconnected".to_owned();
                    }
                    pb::DisconnectResponse::ID => break "client disconnected".to_owned(),
                    pb::PingRequest::ID => {
                        if let Err(err) = s.send(&pb::PingResponse::default()).await {
                            break err.to_string();
                        }
                    }
                    _ => {}
                }
                continue;
            }
            Ok(None) => {}
            Err(err) => break err.to_string(),
        }
        tokio::select! {
            read = s.fill() => if let Err(err) = read {
                break err.to_string();
            },
            () = sleep_until(until) => {
                if s.send(&pb::DisconnectRequest::default()).await.is_ok() {
                    linger(&mut s).await;
                }
                break "disconnected after the key change".to_owned();
            }
        }
    };
    log::info!(
        "esphome-api: HA connection {} ended after the key change: {reason}",
        s.peer
    );
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
            if let Some(probe) = s.probe_until {
                deadline = deadline.min(probe);
            }
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
        let mut dead = None;
        if let Some(s) = self.session.as_mut() {
            let idle = now.saturating_duration_since(s.last_rx);
            if s.probe_until.is_some_and(|t| now >= t) {
                dead = Some("no answer to the ping after a resume");
            } else if idle >= DEAD_AFTER {
                dead = Some("keepalive timeout");
            } else if idle >= PING_AFTER && !s.ping_sent {
                s.ping_sent = true;
                if s.send(&pb::PingRequest::default()).await.is_err() {
                    dead = Some("keepalive timeout");
                }
            }
        }
        if let Some(why) = dead {
            self.close(why);
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
                Ok(Flow::KeyChanged(ev)) => {
                    if let Some(s) = self.session.take() {
                        self.voice.on_disconnect();
                        self.emit(AppEvent::HaDisconnected);
                        tokio::spawn(retire(s));
                    }
                    self.after_key_change(ev).await;
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
                s.send(&entities::media_player_state(state, self.volume))
                    .await
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
                for sw in &self.cfg.switches {
                    s.send(&entities::list_switch(sw)).await?;
                }
                if self.cfg.power_sensor {
                    s.send(&entities::list_power_sensor()).await?;
                }
                for t in &self.cfg.text_sensors {
                    s.send(&entities::list_text_sensor(t)).await?;
                }
                for sel in &self.cfg.selects {
                    s.send(&entities::list_select(sel)).await?;
                }
                for n in &self.cfg.numbers {
                    s.send(&entities::list_number(n)).await?;
                }
                for b in &self.cfg.buttons {
                    s.send(&entities::list_button(b)).await?;
                }
                for b in &self.cfg.binary_sensors {
                    s.send(&entities::list_binary_sensor(b)).await?;
                }
                s.send(&pb::ListEntitiesDoneResponse::default()).await?;
            }
            pb::SubscribeStatesRequest::ID => {
                s.states_subscribed = true;
                s.send(&entities::media_player_state(
                    self.voice.media_state(),
                    self.volume,
                ))
                .await?;
                s.send(&entities::mute_state(self.voice.is_muted())).await?;
                if self.cfg.power_sensor {
                    s.send(&entities::power_state(self.power_state.as_deref()))
                        .await?;
                }
                for (sel, state) in self.cfg.selects.iter().zip(&self.selects) {
                    s.send(&entities::select_state(sel, state.as_deref()))
                        .await?;
                }
                for (n, state) in self.cfg.numbers.iter().zip(&self.numbers) {
                    s.send(&entities::number_state(n, *state)).await?;
                }
                for (b, state) in self.cfg.binary_sensors.iter().zip(&self.binary_sensors) {
                    s.send(&entities::binary_sensor_state(b, *state)).await?;
                }
                for (sw, state) in self.cfg.switches.iter().zip(&self.switches) {
                    s.send(&entities::switch_state(sw, *state)).await?;
                }
                for (t, state) in self.cfg.text_sensors.iter().zip(&self.text_sensors) {
                    s.send(&entities::text_sensor_state(t, state.as_deref()))
                        .await?;
                }
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
                    } else {
                        self.switch_command(c.key, c.state);
                    }
                }
            }
            pb::SelectCommandRequest::ID => {
                if let Some(c) = decode::<pb::SelectCommandRequest>(msg) {
                    self.select_command(c.key, c.state).await?;
                }
            }
            pb::NumberCommandRequest::ID => {
                if let Some(c) = decode::<pb::NumberCommandRequest>(msg) {
                    self.number_command(c.key, c.state).await?;
                }
            }
            pb::ButtonCommandRequest::ID => {
                if let Some(c) = decode::<pb::ButtonCommandRequest>(msg) {
                    self.button_command(c.key);
                }
            }
            pb::MediaPlayerCommandRequest::ID => {
                if let Some(c) = decode::<pb::MediaPlayerCommandRequest>(msg) {
                    self.media_player_command(&c).await?;
                }
            }
            pb::NoiseEncryptionSetKeyRequest::ID => {
                let key = decode::<pb::NoiseEncryptionSetKeyRequest>(msg)
                    .map(|r| r.key)
                    .unwrap_or_default();
                let (origin, peer) = (s.kind.origin(), s.peer);
                let client_info = s.client_info.clone();
                let (success, applied) = self.apply_set_key(&key, origin, peer, now);
                let Some(s) = self.session.as_mut() else {
                    return Ok(Flow::Close);
                };
                let sent = s.send(&pb::NoiseEncryptionSetKeyResponse { success }).await;
                // The change is done even if the reply could not be sent.
                match applied {
                    Some(Applied::Set) => return Ok(Flow::KeyChanged(PairingEvent::KeyRotated)),
                    Some(Applied::Cleared) => {
                        return Ok(Flow::KeyChanged(PairingEvent::Cleared {
                            client_info,
                            peer,
                        }));
                    }
                    None => sent?,
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
            AppCommand::CaptureLive(live) => self.voice.on_capture(now, live),
            AppCommand::OpenPairingWindow => self.open_window(now),
            AppCommand::ResetPairing => self.reset_pairing(now).await,
            AppCommand::SetVolume(percent) => {
                self.volume = percent.min(100);
                if let Err(err) = self.send_media_state().await {
                    self.close(&err.to_string());
                }
            }
            AppCommand::SetPowerState(text) => {
                self.power_state = Some(text);
                let Some(s) = self.session.as_mut() else {
                    return;
                };
                if self.cfg.power_sensor && s.states_subscribed {
                    if let Err(err) = s
                        .send(&entities::power_state(self.power_state.as_deref()))
                        .await
                    {
                        self.close(&err.to_string());
                    }
                }
            }
            AppCommand::Resumed => self.resumed(now).await,
            AppCommand::SetNumber { object_id, value } => {
                self.set_number(&object_id, value).await;
            }
            AppCommand::SetBinarySensor { object_id, state } => {
                self.set_binary_sensor(&object_id, state).await;
            }
            AppCommand::SetSwitch { object_id, state } => {
                self.set_switch(&object_id, state).await;
            }
            AppCommand::SetTextSensor { object_id, state } => {
                self.set_text_sensor(&object_id, state).await;
            }
            AppCommand::ClearSelect { object_id } => self.clear_select(&object_id).await,
            AppCommand::SetSelectOptions { object_id, options } => {
                self.set_select_options(&object_id, options);
            }
            AppCommand::SetSelect { object_id, option } => {
                let Some(i) = self
                    .cfg
                    .selects
                    .iter()
                    .position(|s| s.object_id == object_id)
                else {
                    log::warn!("esphome-api: SetSelect for unknown select {object_id:?} ignored");
                    return;
                };
                if !self.cfg.selects[i].options.contains(&option) {
                    log::warn!("esphome-api: SetSelect {object_id:?}: not an option; ignored");
                    log::debug!(
                        "esphome-api: SetSelect {object_id:?}: {option:?} is not an option"
                    );
                    return;
                }
                self.selects[i] = Some(option);
                if let Err(err) = self.send_select_state(i).await {
                    self.close(&err.to_string());
                }
            }
        }
    }

    async fn set_number(&mut self, object_id: &str, value: Option<f32>) {
        let Some(i) = self
            .cfg
            .numbers
            .iter()
            .position(|n| n.object_id == object_id)
        else {
            log::warn!("esphome-api: SetNumber for unknown number {object_id:?} ignored");
            return;
        };
        if value.is_some_and(|v| !entities::number_accepts(&self.cfg.numbers[i], v)) {
            log::warn!("esphome-api: SetNumber {object_id:?}: value out of range; ignored");
            return;
        }
        if self.numbers[i] == value {
            return;
        }
        self.numbers[i] = value;
        if let Err(err) = self.send_number_state(i).await {
            self.close(&err.to_string());
        }
    }

    async fn set_binary_sensor(&mut self, object_id: &str, state: Option<bool>) {
        let Some(i) = self
            .cfg
            .binary_sensors
            .iter()
            .position(|b| b.object_id == object_id)
        else {
            log::warn!("esphome-api: SetBinarySensor for unknown sensor {object_id:?} ignored");
            return;
        };
        if self.binary_sensors[i] == state {
            return;
        }
        self.binary_sensors[i] = state;
        if let Err(err) = self.send_binary_sensor_state(i).await {
            self.close(&err.to_string());
        }
    }

    async fn set_switch(&mut self, object_id: &str, state: Option<bool>) {
        let Some(i) = self
            .cfg
            .switches
            .iter()
            .position(|sw| sw.object_id == object_id)
        else {
            log::warn!("esphome-api: SetSwitch for unknown switch {object_id:?} ignored");
            return;
        };
        if self.switches[i] == state {
            return;
        }
        self.switches[i] = state;
        if let Err(err) = self.send_switch_state(i).await {
            self.close(&err.to_string());
        }
    }

    async fn set_text_sensor(&mut self, object_id: &str, state: Option<String>) {
        let Some(i) = self
            .cfg
            .text_sensors
            .iter()
            .position(|t| t.object_id == object_id)
        else {
            log::warn!("esphome-api: SetTextSensor for unknown sensor {object_id:?} ignored");
            return;
        };
        if self.text_sensors[i] == state {
            return;
        }
        self.text_sensors[i] = state;
        if let Err(err) = self.send_text_sensor_state(i).await {
            self.close(&err.to_string());
        }
    }

    async fn clear_select(&mut self, object_id: &str) {
        let Some(i) = self
            .cfg
            .selects
            .iter()
            .position(|s| s.object_id == object_id)
        else {
            log::warn!("esphome-api: ClearSelect for unknown select {object_id:?} ignored");
            return;
        };
        self.selects[i] = None;
        if let Err(err) = self.send_select_state(i).await {
            self.close(&err.to_string());
        }
    }

    /// Replaces a select's options. HA only reads them when it lists the
    /// entities, so a changed list closes the connection: HA reconnects at
    /// once and lists again.
    fn set_select_options(&mut self, object_id: &str, options: Vec<String>) {
        let Some(i) = self
            .cfg
            .selects
            .iter()
            .position(|s| s.object_id == object_id)
        else {
            log::warn!("esphome-api: SetSelectOptions for unknown select {object_id:?} ignored");
            return;
        };
        if options.is_empty() {
            log::warn!("esphome-api: SetSelectOptions {object_id:?}: no options; ignored");
            return;
        }
        if self.cfg.selects[i].options == options {
            return;
        }
        Arc::make_mut(&mut self.cfg).selects[i].options = options;
        self.selects[i] = None;
        log::info!(
            "esphome-api: the options of {object_id} changed; closing the HA connection so HA lists the entities again"
        );
        self.close("the options of a select changed");
    }

    async fn send_switch_state(&mut self, i: usize) -> Result<(), FrameError> {
        let msg = entities::switch_state(&self.cfg.switches[i], self.switches[i]);
        match self.session.as_mut() {
            Some(s) if s.states_subscribed => s.send(&msg).await,
            _ => Ok(()),
        }
    }

    async fn send_text_sensor_state(&mut self, i: usize) -> Result<(), FrameError> {
        let msg =
            entities::text_sensor_state(&self.cfg.text_sensors[i], self.text_sensors[i].as_deref());
        match self.session.as_mut() {
            Some(s) if s.states_subscribed => s.send(&msg).await,
            _ => Ok(()),
        }
    }

    /// `SwitchCommandRequest` for a generic switch: reported to the
    /// application, which answers with the actual state
    /// ([`AppCommand::SetSwitch`]); the server does not echo it. A key that
    /// is no switch is ignored.
    fn switch_command(&mut self, key: u32, state: bool) {
        let Some(sw) = self
            .cfg
            .switches
            .iter()
            .find(|sw| entities::entity_key(&sw.object_id) == key)
        else {
            log::debug!("esphome-api: switch command for an unknown entity ignored");
            return;
        };
        log::info!(
            "esphome-api: {} set to {} by HA",
            sw.object_id,
            if state { "on" } else { "off" }
        );
        let object_id = sw.object_id.clone();
        self.emit(AppEvent::SwitchChanged { object_id, state });
    }

    async fn send_number_state(&mut self, i: usize) -> Result<(), FrameError> {
        let msg = entities::number_state(&self.cfg.numbers[i], self.numbers[i]);
        match self.session.as_mut() {
            Some(s) if s.states_subscribed => s.send(&msg).await,
            _ => Ok(()),
        }
    }

    async fn send_binary_sensor_state(&mut self, i: usize) -> Result<(), FrameError> {
        let msg =
            entities::binary_sensor_state(&self.cfg.binary_sensors[i], self.binary_sensors[i]);
        match self.session.as_mut() {
            Some(s) if s.states_subscribed => s.send(&msg).await,
            _ => Ok(()),
        }
    }

    /// `NumberCommandRequest`: a finite value inside the range is reported to
    /// the application, which answers with the actual state
    /// ([`AppCommand::SetNumber`]); the server does not echo it. Anything else
    /// is answered with the current state.
    async fn number_command(&mut self, key: u32, value: f32) -> Result<(), FrameError> {
        let Some(i) = self
            .cfg
            .numbers
            .iter()
            .position(|n| entities::entity_key(&n.object_id) == key)
        else {
            log::info!("esphome-api: number command for an unknown entity ignored");
            return Ok(());
        };
        let n = &self.cfg.numbers[i];
        if entities::number_accepts(n, value) {
            log::info!("esphome-api: {} set to {value} by HA", n.object_id);
            let object_id = n.object_id.clone();
            self.emit(AppEvent::NumberChanged { object_id, value });
            Ok(())
        } else {
            log::warn!(
                "esphome-api: {}: {value} is outside {}..={}; ignored",
                n.object_id,
                n.min_value,
                n.max_value
            );
            self.send_number_state(i).await
        }
    }

    /// `ButtonCommandRequest`: reported to the application.
    fn button_command(&mut self, key: u32) {
        let Some(b) = self
            .cfg
            .buttons
            .iter()
            .find(|b| entities::entity_key(&b.object_id) == key)
        else {
            log::info!("esphome-api: button command for an unknown entity ignored");
            return;
        };
        log::info!("esphome-api: {} pressed by HA", b.object_id);
        let object_id = b.object_id.clone();
        self.emit(AppEvent::ButtonPressed { object_id });
    }

    async fn send_select_state(&mut self, i: usize) -> Result<(), FrameError> {
        let msg = entities::select_state(&self.cfg.selects[i], self.selects[i].as_deref());
        match self.session.as_mut() {
            Some(s) if s.states_subscribed => s.send(&msg).await,
            _ => Ok(()),
        }
    }

    /// `SelectCommandRequest`: a valid option is stored, reported to the
    /// application and confirmed to HA; anything else is answered with the
    /// current state.
    async fn select_command(&mut self, key: u32, option: String) -> Result<(), FrameError> {
        let Some(i) = self
            .cfg
            .selects
            .iter()
            .position(|s| entities::entity_key(&s.object_id) == key)
        else {
            log::info!("esphome-api: select command for an unknown entity ignored");
            return Ok(());
        };
        let sel = &self.cfg.selects[i];
        if !sel.options.contains(&option) {
            log::warn!(
                "esphome-api: {}: a value that is not an option; ignored",
                sel.object_id
            );
            log::debug!(
                "esphome-api: {}: {option:?} is not an option",
                sel.object_id
            );
        } else if !sel.command_echo {
            // The application owns the state; the option may be an app title
            // or an input label, so it is not logged at info.
            log::info!("esphome-api: {} chosen by HA", sel.object_id);
            log::debug!("esphome-api: {} chosen: {option:?}", sel.object_id);
            let object_id = sel.object_id.clone();
            self.emit(AppEvent::SelectChanged { object_id, option });
            return Ok(());
        } else if self.selects[i].as_deref() != Some(option.as_str()) {
            log::info!("esphome-api: {} set to {option:?} by HA", sel.object_id);
            let object_id = sel.object_id.clone();
            self.selects[i] = Some(option.clone());
            self.emit(AppEvent::SelectChanged { object_id, option });
        }
        self.send_select_state(i).await
    }

    /// Sends the media player state (with the reply volume) if HA subscribed.
    async fn send_media_state(&mut self) -> Result<(), FrameError> {
        let state = entities::media_player_state(self.voice.media_state(), self.volume);
        match self.session.as_mut() {
            Some(s) if s.states_subscribed => s.send(&state).await,
            _ => Ok(()),
        }
    }

    /// `MediaPlayerCommandRequest`: stop, and the reply volume (set, step).
    async fn media_player_command(
        &mut self,
        c: &pb::MediaPlayerCommandRequest,
    ) -> Result<(), FrameError> {
        let mut volume = None;
        if c.has_volume {
            volume = Some(entities::volume_to_percent(c.volume));
        }
        if c.has_command {
            match pb::MediaPlayerCommand::try_from(c.command) {
                Ok(pb::MediaPlayerCommand::Stop) => {
                    self.voice.on_media_stop();
                    self.emit(AppEvent::MediaStop);
                }
                Ok(pb::MediaPlayerCommand::VolumeUp) => {
                    volume = Some(entities::volume_step(self.volume, true));
                }
                Ok(pb::MediaPlayerCommand::VolumeDown) => {
                    volume = Some(entities::volume_step(self.volume, false));
                }
                _ => log::info!(
                    "esphome-api: unsupported media player command {} ignored",
                    c.command
                ),
            }
        } else if !c.has_volume {
            log::info!("esphome-api: empty media player command ignored");
        }
        if let Some(v) = volume {
            if v != self.volume {
                log::info!("esphome-api: reply volume {} -> {v} % by HA", self.volume);
                self.volume = v;
                self.emit(AppEvent::VolumeChanged(v));
            }
            // Always answer, so HA's slider shows the value in effect.
            self.send_media_state().await?;
        }
        Ok(())
    }

    /// After a system suspend: probe the HA connection, re-announce mDNS and
    /// re-arm the wake word at once.
    async fn resumed(&mut self, now: Instant) {
        let mut failed = false;
        if let Some(s) = self.session.as_mut() {
            log::info!(
                "esphome-api: resumed; probing the HA connection {} (dropped without an answer within {} s)",
                s.peer,
                RESUME_PROBE_TIMEOUT.as_secs()
            );
            s.ping_sent = true;
            s.probe_until = Some(now + RESUME_PROBE_TIMEOUT);
            failed = s.send(&pb::PingRequest::default()).await.is_err();
        } else {
            log::info!("esphome-api: resumed; no HA connection (HA reconnects by itself)");
        }
        if failed {
            self.close("the connection did not survive the suspend");
        }
        if let Some(m) = &self.mdns {
            m.reannounce();
        }
        self.voice.on_resume(now);
    }
}

/// What an accepted `SetKey` changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Applied {
    /// A new key was stored and activated.
    Set,
    /// The key was deleted (unpaired, window closed).
    Cleared,
}

/// Pairing: window, `SetKey`, activation and reset.
impl Ctx {
    fn key_file(&self) -> Option<&std::path::Path> {
        self.cfg.provisioning.as_ref().map(|p| p.key_file.as_path())
    }

    /// At start: opens the window if unpaired and configured, unless the key
    /// holder cleared the key (then only a local action opens it).
    fn window_at_start(&mut self, cleared: bool) {
        if !self.pairing.needs_pairing() {
            return;
        }
        if cleared {
            log::warn!(
                "esphome-api: the key holder cleared the pairing key; the pairing window stays closed until it is opened locally (keyless connections are refused)"
            );
        } else if self
            .cfg
            .provisioning
            .as_ref()
            .is_some_and(|p| p.open_at_start)
        {
            self.open_window(Instant::now());
        } else {
            log::warn!(
                "esphome-api: pairing window closed; open it to pair (keyless connections are refused)"
            );
        }
    }

    fn open_window(&mut self, now: Instant) {
        match self.pairing.open_window(now) {
            OpenResult::Opened(d) => {
                self.forget_cleared();
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

    /// Removes the marker of a remote clear (a local open, a reset or a new
    /// key ends that state). Best effort, logged.
    fn forget_cleared(&self) {
        if let Some(path) = self.key_file() {
            if let Err(err) = pairing::forget_cleared(path) {
                log::error!(
                    "esphome-api: cannot remove {}: {err}",
                    pairing::cleared_marker(path).display()
                );
            }
        }
    }

    /// Checks a `SetKey` and stores or deletes the key. Returns the reply's
    /// `success` and what changed. Never logs the key.
    fn apply_set_key(
        &mut self,
        key: &[u8],
        origin: Origin,
        peer: SocketAddr,
        now: Instant,
    ) -> (bool, Option<Applied>) {
        let change = match self.pairing.check_set_key(key, origin, now) {
            Err(why) => {
                log::warn!("esphome-api: SetKey from {peer} refused: {why}");
                return (false, None);
            }
            Ok(KeyChange::Unchanged) => {
                log::info!("esphome-api: SetKey from {peer}: that key is already active");
                return (true, None);
            }
            Ok(change) => change,
        };
        let Some(path) = self.key_file().map(std::path::Path::to_path_buf) else {
            return (false, None);
        };
        match change {
            KeyChange::Set(key) => {
                if let Err(err) = pairing::store_key(&path, &key) {
                    log::error!(
                        "esphome-api: SetKey from {peer}: cannot store the key in {}: {err}",
                        path.display()
                    );
                    return (false, None);
                }
                log::warn!(
                    "esphome-api: key set by {peer} and stored in {} (mode 0600); closing sessions opened before it",
                    path.display()
                );
                self.forget_cleared();
                self.pairing.activate(key);
                (true, Some(Applied::Set))
            }
            KeyChange::Clear => {
                // The marker first: if it cannot be written, the key stays.
                if let Err(err) = pairing::mark_cleared(&path) {
                    log::error!(
                        "esphome-api: key clear from {peer} refused: cannot write {}: {err}",
                        pairing::cleared_marker(&path).display()
                    );
                    return (false, None);
                }
                if let Err(err) = pairing::remove_key(&path) {
                    log::error!(
                        "esphome-api: key clear from {peer} failed: cannot delete {}: {err}; the key stays active",
                        path.display()
                    );
                    self.forget_cleared();
                    return (false, None);
                }
                self.pairing.clear();
                log::warn!(
                    "esphome-api: PAIRING CLEARED by the key holder {peer}: stored key deleted ({}); unpaired, and the pairing window stays closed until it is opened locally",
                    path.display()
                );
                (true, Some(Applied::Cleared))
            }
            KeyChange::Unchanged => (true, None),
        }
    }

    async fn on_key_request(&mut self, req: KeyRequest) {
        let (success, applied) = self.apply_set_key(&req.key, req.origin, req.peer, Instant::now());
        // The keyless task sends the reply before it sees the new epoch.
        let _ = req.reply.send(success);
        // Keyless sessions can only set a first key (never clear one).
        if applied == Some(Applied::Set) {
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
        self.forget_cleared();
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
