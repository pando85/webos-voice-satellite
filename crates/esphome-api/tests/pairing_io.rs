//! Pairing (HA-driven key provisioning) over loopback TCP with a minimal Rust
//! client playing HA's side: keyless plaintext and zero-PSK Noise sessions,
//! `SetKey`, the window, reset and rotation. A capturing logger checks that
//! no key ever reaches the log. The aioesphomeapi harness in
//! tools/esphome-fake-ha repeats the main flow with HA's client library.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep, timeout};
use webos_esphome_api::frame::{self, Codec, NOISE_PROLOGUE, NOISE_PROTOCOL, RawMessage};
use webos_esphome_api::proto::{self as pb, ApiMessage};
use webos_esphome_api::{
    AppCommand, AppEvent, Config, PairingEvent, PipelineMode, Provisioning, Server, features,
};

// ---------------------------------------------------------------- logging

struct Capture(Mutex<Vec<String>>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, r: &log::Record<'_>) {
        self.0.lock().unwrap().push(format!("{}", r.args()));
    }
    fn flush(&self) {}
}

fn logs() -> &'static Capture {
    static LOGGER: OnceLock<&'static Capture> = OnceLock::new();
    LOGGER.get_or_init(|| {
        let l: &'static Capture = Box::leak(Box::new(Capture(Mutex::new(Vec::new()))));
        log::set_logger(l).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
        l
    })
}

fn assert_not_logged(secret: &str) {
    let lines = logs().0.lock().unwrap();
    assert!(!lines.is_empty(), "logger captured nothing");
    for l in lines.iter() {
        assert!(!l.contains(secret), "secret in log line: {l}");
    }
}

// ---------------------------------------------------------------- setup

fn b64(key: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(key)
}

fn random_key() -> [u8; 32] {
    let mut k = [0_u8; 32];
    getrandom(&mut k);
    k
}

/// Non-cryptographic test randomness (distinct keys per test).
fn getrandom(buf: &mut [u8]) {
    use std::hash::{BuildHasher as _, Hasher as _};
    for chunk in buf.chunks_mut(8) {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(u64::from(std::process::id()));
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("webos-pairing-io-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Dev {
    port: u16,
    events: mpsc::UnboundedReceiver<AppEvent>,
    cmds: mpsc::Sender<AppCommand>,
}

async fn start(cfg: Config) -> (Dev, bool) {
    logs();
    let server = Server::bind(cfg).await.unwrap();
    let needs = server.needs_pairing();
    let port = server.local_addr().unwrap().port();
    let (ev_tx, events) = mpsc::unbounded_channel();
    let (cmds, cmd_rx) = mpsc::channel(16);
    tokio::spawn(server.run(ev_tx, cmd_rx));
    (Dev { port, events, cmds }, needs)
}

fn config(key_file: &Path, window: Duration, open_at_start: bool) -> Config {
    Config {
        name: "pair-tv".into(),
        bind_addr: "127.0.0.1".parse().unwrap(),
        port: 0,
        mac: Some("02:11:22:33:44:66".into()),
        provisioning: Some(Provisioning {
            key_file: key_file.to_path_buf(),
            window,
            open_at_start,
        }),
        ..Config::default()
    }
}

impl Dev {
    /// Next event matching `pred`, skipping others.
    async fn event(&mut self, pred: impl Fn(&AppEvent) -> bool) -> AppEvent {
        timeout(Duration::from_secs(5), async {
            loop {
                let ev = self.events.recv().await.expect("events closed");
                if pred(&ev) {
                    return ev;
                }
            }
        })
        .await
        .expect("no matching event")
    }

    fn pending_events(&mut self) -> Vec<AppEvent> {
        let mut v = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            v.push(ev);
        }
        v
    }
}

// ---------------------------------------------------------------- client

struct Client {
    stream: TcpStream,
    codec: Codec,
    buf: Vec<u8>,
}

impl Client {
    async fn send<M: ApiMessage>(&mut self, msg: &M) {
        let mut out = Vec::new();
        self.codec
            .encode(M::ID, &msg.encode_to_vec(), &mut out)
            .unwrap();
        self.stream.write_all(&out).await.unwrap();
    }

    /// Next message, or `None` when the server closed the connection.
    async fn try_recv(&mut self) -> Option<RawMessage> {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(m) = self.codec.decode(&mut self.buf).unwrap() {
                    return Some(m);
                }
                let mut tmp = [0_u8; 4096];
                match self.stream.read(&mut tmp).await {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                }
            }
        })
        .await
        .expect("timeout")
    }

    async fn recv(&mut self) -> RawMessage {
        self.try_recv().await.expect("server closed")
    }

    async fn recv_as<M: ApiMessage>(&mut self) -> M {
        let m = self.recv().await;
        assert_eq!(
            m.type_id,
            M::ID,
            "expected {}, got {:?}",
            M::NAME,
            pb::message_name(m.type_id)
        );
        M::decode(m.payload.as_slice()).unwrap()
    }

    async fn hello(&mut self) -> pb::HelloResponse {
        self.send(&pb::HelloRequest {
            client_info: "rust-test".into(),
            api_version_major: 1,
            api_version_minor: 14,
            ..Default::default()
        })
        .await;
        self.recv_as().await
    }

    async fn device_info(&mut self) -> pb::DeviceInfoResponse {
        self.send(&pb::DeviceInfoRequest {}).await;
        self.recv_as().await
    }

    async fn set_key(&mut self, key: &[u8]) -> bool {
        self.send(&pb::NoiseEncryptionSetKeyRequest { key: key.to_vec() })
            .await;
        let r: pb::NoiseEncryptionSetKeyResponse = self.recv_as().await;
        r.success
    }

    /// Expects a `DisconnectRequest` (skipping nothing) and returns its reason.
    async fn expect_disconnect(&mut self) -> i32 {
        let r: pb::DisconnectRequest = self.recv_as().await;
        r.reason
    }

    async fn expect_closed(&mut self) {
        loop {
            if self.try_recv().await.is_none() {
                return;
            }
        }
    }
}

async fn connect_plain(port: u16) -> Client {
    Client {
        stream: TcpStream::connect(("127.0.0.1", port)).await.unwrap(),
        codec: Codec::Plaintext,
        buf: Vec::new(),
    }
}

async fn read_noise_frame(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    loop {
        if let Some((body, used)) = frame::decode_noise_frame(buf).unwrap() {
            let body = body.to_vec();
            buf.drain(..used);
            return Some(body);
        }
        let mut tmp = [0_u8; 1024];
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Noise handshake as aioesphomeapi does it. `Err(reject body)` on failure.
async fn connect_noise(port: u16, psk: &[u8; 32]) -> Result<Client, Vec<u8>> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut hs = snow::Builder::new(NOISE_PROTOCOL.parse().unwrap())
        .psk(0, psk)
        .unwrap()
        .prologue(NOISE_PROLOGUE)
        .unwrap()
        .build_initiator()
        .unwrap();
    let mut msg = [0_u8; 128];
    let n = hs.write_message(&[], &mut msg[1..]).unwrap();
    let mut out = vec![0x01, 0x00, 0x00];
    frame::encode_noise_frame(&msg[..=n], &mut out).unwrap();
    stream.write_all(&out).await.unwrap();
    let mut buf = Vec::new();
    let hello = read_noise_frame(&mut stream, &mut buf).await.unwrap();
    assert_eq!(hello, b"\x01pair-tv\x00021122334466\x00");
    let reply = read_noise_frame(&mut stream, &mut buf).await.unwrap();
    if reply[0] != 0x00 {
        return Err(reply);
    }
    let mut scratch = [0_u8; 128];
    hs.read_message(&reply[1..], &mut scratch).unwrap();
    Ok(Client {
        stream,
        codec: Codec::Noise(Box::new(frame::NoiseTransport::from_state(
            hs.into_transport_mode().unwrap(),
        ))),
        buf,
    })
}

const ZERO: [u8; 32] = [0; 32];

fn is_mac_failure(reject: &[u8]) -> bool {
    reject == frame::handshake_reject_body(frame::HANDSHAKE_MAC_FAILURE)
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ---------------------------------------------------------------- tests

/// Keyless plaintext session: device info with the provisioning flags, no
/// entities, no voice, no audio, no `SetKey`, and the app never sees HA.
#[tokio::test]
async fn unpaired_plaintext_session_is_restricted() {
    let dir = tmpdir("restricted");
    let key_file = dir.join("noise_psk");
    let (mut dev, needs) = start(config(&key_file, Duration::from_secs(600), true)).await;
    assert!(needs);
    assert_eq!(
        dev.event(|_| true).await,
        AppEvent::Pairing(PairingEvent::WindowOpened { seconds: 600 })
    );
    // The app arms the wake word as the daemon does; it must not start a run.
    dev.cmds
        .send(AppCommand::StartPipeline {
            mode: PipelineMode::WakeWord,
        })
        .await
        .unwrap();

    let mut c = connect_plain(dev.port).await;
    assert_eq!(c.hello().await.name, "pair-tv");
    let info = c.device_info().await;
    assert!(info.api_encryption_supported && info.api_encryption_provisionable);
    assert_eq!(
        info.voice_assistant_feature_flags, 0,
        "no satellite unpaired"
    );
    assert_eq!(info.mac_address, "02:11:22:33:44:66");

    c.send(&pb::ListEntitiesRequest {}).await;
    let _: pb::ListEntitiesDoneResponse = c.recv_as().await;

    c.send(&pb::SubscribeStatesRequest {}).await;
    c.send(&pb::SubscribeVoiceAssistantRequest {
        subscribe: true,
        flags: 4,
    })
    .await;
    c.send(&pb::SwitchCommandRequest {
        key: 1,
        state: false,
        ..Default::default()
    })
    .await;
    dev.cmds
        .send(AppCommand::StartPipeline {
            mode: PipelineMode::PushToTalk,
        })
        .await
        .unwrap();
    dev.cmds
        .send(AppCommand::Audio(vec![7; 512]))
        .await
        .unwrap();
    sleep(Duration::from_millis(300)).await;
    // The next message is the ping reply: no state, no start request, no audio.
    c.send(&pb::PingRequest {}).await;
    let _: pb::PingResponse = c.recv_as().await;
    let evs = dev.pending_events();
    assert!(
        !evs.iter().any(|e| matches!(
            e,
            AppEvent::HaConnected { .. } | AppEvent::StartStreaming { .. }
        )),
        "{evs:?}"
    );

    // SetKey over plaintext is refused (the key would cross the LAN in clear).
    let key = random_key();
    assert!(!c.set_key(b64(&key).as_bytes()).await);
    assert!(!key_file.exists());
    assert_not_logged(&b64(&key));
    let _ = std::fs::remove_dir_all(&dir);
}

/// HA's flow: plaintext main session + zero-PSK session that sets the key.
#[tokio::test]
async fn zero_psk_set_key_pairs_and_then_requires_the_key() {
    let dir = tmpdir("pair");
    let key_file = dir.join("noise_psk");
    let (mut dev, _) = start(config(&key_file, Duration::from_secs(600), true)).await;

    let mut main = connect_plain(dev.port).await;
    main.hello().await;
    assert!(main.device_info().await.api_encryption_provisionable);

    let mut z = connect_noise(dev.port, &ZERO).await.unwrap();
    z.hello().await;
    let info = z.device_info().await;
    assert!(info.api_encryption_provisionable);
    // Bad keys first (one attempt per second is allowed).
    assert!(!z.set_key(b"AAAA").await, "short key");
    sleep(Duration::from_millis(1050)).await;
    assert!(!z.set_key(b64(&ZERO).as_bytes()).await, "all-zero key");
    sleep(Duration::from_millis(1050)).await;
    assert!(!z.set_key(b"").await, "empty key");
    sleep(Duration::from_millis(1050)).await;
    assert!(!key_file.exists());

    let key = random_key();
    let key_b64 = b64(&key);
    assert!(z.set_key(key_b64.as_bytes()).await);
    // Both keyless sessions are told to disconnect and closed.
    assert_eq!(z.expect_disconnect().await, 0);
    z.expect_closed().await;
    assert_eq!(main.expect_disconnect().await, 0);
    main.expect_closed().await;
    match dev
        .event(|e| matches!(e, AppEvent::Pairing(PairingEvent::Paired { .. })))
        .await
    {
        AppEvent::Pairing(PairingEvent::Paired { client_info, .. }) => {
            assert_eq!(client_info, "rust-test");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&key_file).unwrap(),
        format!("{key_b64}\n")
    );
    #[cfg(unix)]
    assert_eq!(mode(&key_file), 0o600);

    // Plaintext and zero-PSK are refused now; the wrong key too.
    let mut p = connect_plain(dev.port).await;
    p.send(&pb::HelloRequest::default()).await;
    let mut first = [0_u8; 1];
    p.stream.read_exact(&mut first).await.unwrap();
    assert_eq!(first[0], frame::NOISE_PREAMBLE, "requires encryption");
    assert!(is_mac_failure(
        &connect_noise(dev.port, &ZERO).await.err().unwrap()
    ));
    assert!(is_mac_failure(
        &connect_noise(dev.port, &[5; 32]).await.err().unwrap()
    ));

    // The new key works and gives the full device.
    let mut c = connect_noise(dev.port, &key).await.unwrap();
    c.hello().await;
    dev.event(|e| matches!(e, AppEvent::HaConnected { .. }))
        .await;
    let info = c.device_info().await;
    assert!(info.api_encryption_supported && !info.api_encryption_provisionable);
    assert_eq!(info.voice_assistant_feature_flags, features::DEFAULT);
    // A keyless SetKey is impossible now, and so is a remote clear.
    sleep(Duration::from_millis(1050)).await;
    assert!(!c.set_key(b"").await);
    drop(c);

    // A restarted server is paired from the file.
    let (_dev2, needs) = start(config(&key_file, Duration::from_secs(600), true)).await;
    assert!(!needs);
    assert_not_logged(&key_b64);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Window timeout: hello, then `DisconnectRequest(PROVISIONING_CLOSED)` on
/// both transports; an explicit open reopens it.
#[tokio::test]
async fn closed_window_refuses_and_reopens_on_request() {
    let dir = tmpdir("window");
    let key_file = dir.join("noise_psk");
    let (mut dev, _) = start(config(&key_file, Duration::from_millis(600), true)).await;
    // A keyless session open when the window closes is closed too.
    let mut early = connect_plain(dev.port).await;
    early.hello().await;
    dev.event(|e| *e == AppEvent::Pairing(PairingEvent::WindowClosed))
        .await;
    assert_eq!(early.expect_disconnect().await, 0);
    early.expect_closed().await;

    let closed = pb::DisconnectReason::ProvisioningClosed as i32;
    let mut c = connect_plain(dev.port).await;
    assert_eq!(c.hello().await.name, "pair-tv");
    assert_eq!(c.expect_disconnect().await, closed);
    c.expect_closed().await;
    let mut z = connect_noise(dev.port, &ZERO).await.unwrap();
    z.hello().await;
    assert_eq!(z.expect_disconnect().await, closed);

    dev.cmds.send(AppCommand::OpenPairingWindow).await.unwrap();
    dev.event(|e| matches!(e, AppEvent::Pairing(PairingEvent::WindowOpened { .. })))
        .await;
    let mut c = connect_plain(dev.port).await;
    c.hello().await;
    assert!(c.device_info().await.api_encryption_provisionable);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `open_at_start = false`: no window until it is opened explicitly.
#[tokio::test]
async fn window_can_start_closed() {
    let dir = tmpdir("closed-start");
    let (mut dev, needs) = start(config(
        &dir.join("noise_psk"),
        Duration::from_secs(600),
        false,
    ))
    .await;
    assert!(needs);
    let mut c = connect_plain(dev.port).await;
    c.hello().await;
    assert_eq!(
        c.expect_disconnect().await,
        pb::DisconnectReason::ProvisioningClosed as i32
    );
    dev.cmds.send(AppCommand::OpenPairingWindow).await.unwrap();
    assert_eq!(
        dev.event(|_| true).await,
        AppEvent::Pairing(PairingEvent::WindowOpened { seconds: 600 })
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reset: the key file is deleted, the keyed session dropped, the window
/// reopened, and the old key no longer works.
#[tokio::test]
async fn reset_pairing_deletes_the_key_and_reopens_the_window() {
    let dir = tmpdir("reset");
    let key_file = dir.join("noise_psk");
    let key = random_key();
    webos_esphome_api::pairing::store_key(&key_file, &key).unwrap();
    let (mut dev, needs) = start(config(&key_file, Duration::from_secs(60), true)).await;
    assert!(!needs);

    let mut c = connect_noise(dev.port, &key).await.unwrap();
    c.hello().await;
    dev.event(|e| matches!(e, AppEvent::HaConnected { .. }))
        .await;
    // The window cannot be opened while paired.
    dev.cmds.send(AppCommand::OpenPairingWindow).await.unwrap();
    dev.cmds.send(AppCommand::ResetPairing).await.unwrap();
    assert_eq!(
        dev.event(|e| matches!(e, AppEvent::Pairing(_))).await,
        AppEvent::Pairing(PairingEvent::Reset)
    );
    assert_eq!(
        dev.event(|e| matches!(e, AppEvent::Pairing(_))).await,
        AppEvent::Pairing(PairingEvent::WindowOpened { seconds: 60 })
    );
    assert_eq!(c.expect_disconnect().await, 0);
    assert!(!key_file.exists());
    assert!(is_mac_failure(
        &connect_noise(dev.port, &key).await.err().unwrap()
    ));
    let mut z = connect_noise(dev.port, &ZERO).await.unwrap();
    z.hello().await;
    assert!(z.device_info().await.api_encryption_provisionable);
    assert_not_logged(&b64(&key));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A session that proved the paired key may rotate it (ESPHome behaviour);
/// a key fixed in the configuration cannot change.
#[tokio::test]
async fn authenticated_rotation_and_fixed_key() {
    let dir = tmpdir("rotate");
    let key_file = dir.join("noise_psk");
    let old = random_key();
    webos_esphome_api::pairing::store_key(&key_file, &old).unwrap();
    let (mut dev, _) = start(config(&key_file, Duration::from_secs(60), true)).await;
    let mut c = connect_noise(dev.port, &old).await.unwrap();
    c.hello().await;
    let mut new = random_key();
    new[0] ^= 0x55;
    assert!(c.set_key(b64(&new).as_bytes()).await);
    assert_eq!(c.expect_disconnect().await, 0);
    dev.event(|e| *e == AppEvent::Pairing(PairingEvent::KeyRotated))
        .await;
    assert_eq!(
        webos_esphome_api::pairing::load_key(&key_file).unwrap(),
        Some(new)
    );
    assert!(is_mac_failure(
        &connect_noise(dev.port, &old).await.err().unwrap()
    ));
    let mut c = connect_noise(dev.port, &new).await.unwrap();
    c.hello().await;

    // Fixed key from the configuration: SetKey refused, no file written.
    let fixed_file = dir.join("never");
    let (fixed_dev, needs) = start(Config {
        name: "pair-tv".into(),
        bind_addr: "127.0.0.1".parse().unwrap(),
        port: 0,
        mac: Some("02:11:22:33:44:66".into()),
        noise_psk: Some(b64(&old)),
        ..Config::default()
    })
    .await;
    assert!(!needs);
    let mut c = connect_noise(fixed_dev.port, &old).await.unwrap();
    c.hello().await;
    assert!(!c.set_key(b64(&new).as_bytes()).await);
    assert!(!fixed_file.exists());
    assert_not_logged(&b64(&old));
    assert_not_logged(&b64(&new));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A corrupt stored key stops the server instead of reopening pairing.
#[tokio::test]
async fn corrupt_key_file_refuses_to_start() {
    let dir = tmpdir("corrupt");
    let key_file = dir.join("noise_psk");
    std::fs::write(&key_file, "garbage-SECRET\n").unwrap();
    let err = Server::bind(config(&key_file, Duration::from_secs(1), true))
        .await
        .err()
        .unwrap();
    let msg = err.to_string();
    assert!(
        msg.contains("reset the pairing") && !msg.contains("SECRET"),
        "{msg}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
