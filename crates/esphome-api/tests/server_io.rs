//! End-to-end server tests over loopback TCP with a minimal Rust client
//! (plaintext and Noise). The aioesphomeapi harness in tools/esphome-fake-ha
//! covers the HA behaviour in depth.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use webos_esphome_api::frame::{self, Codec, NOISE_PROLOGUE, NOISE_PROTOCOL, RawMessage};
use webos_esphome_api::proto::{self as pb, ApiMessage};
use webos_esphome_api::{AppCommand, AppEvent, Config, PipelineMode, Server};

const PSK_B64: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=";

async fn start(
    psk: Option<&str>,
) -> (
    u16,
    mpsc::UnboundedReceiver<AppEvent>,
    mpsc::Sender<AppCommand>,
) {
    let cfg = Config {
        name: "test-tv".into(),
        bind_addr: "127.0.0.1".parse().unwrap(),
        port: 0,
        mac: Some("02:11:22:33:44:55".into()),
        noise_psk: psk.map(str::to_owned),
        ..Config::default()
    };
    let server = Server::bind(cfg).await.unwrap();
    let port = server.local_addr().unwrap().port();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    tokio::spawn(server.run(ev_tx, cmd_rx));
    (port, ev_rx, cmd_tx)
}

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

    async fn recv(&mut self) -> RawMessage {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(m) = self.codec.decode(&mut self.buf).unwrap() {
                    return m;
                }
                let mut tmp = [0_u8; 4096];
                let n = self.stream.read(&mut tmp).await.unwrap();
                assert!(n > 0, "server closed");
                self.buf.extend_from_slice(&tmp[..n]);
            }
        })
        .await
        .expect("timeout")
    }

    async fn recv_as<M: ApiMessage>(&mut self) -> M {
        loop {
            let m = self.recv().await;
            if m.type_id == M::ID {
                return M::decode(m.payload.as_slice()).unwrap();
            }
        }
    }

    async fn hello(&mut self) -> pb::HelloResponse {
        self.send(&pb::HelloRequest {
            client_info: "rust-test".into(),
            api_version_major: 1,
            api_version_minor: 18,
            ..Default::default()
        })
        .await;
        self.recv_as().await
    }
}

async fn read_noise_frame(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Vec<u8> {
    loop {
        if let Some((body, used)) = frame::decode_noise_frame(buf).unwrap() {
            let body = body.to_vec();
            buf.drain(..used);
            return body;
        }
        let mut tmp = [0_u8; 1024];
        let n = stream.read(&mut tmp).await.unwrap();
        assert!(n > 0, "server closed");
        buf.extend_from_slice(&tmp[..n]);
    }
}

#[tokio::test]
async fn plaintext_hello_device_info_and_voice_start() {
    let (port, mut events, cmds) = start(None).await;
    cmds.send(AppCommand::StartPipeline {
        mode: PipelineMode::WakeWord,
    })
    .await
    .unwrap();
    let mut c = Client {
        stream: TcpStream::connect(("127.0.0.1", port)).await.unwrap(),
        codec: Codec::Plaintext,
        buf: Vec::new(),
    };
    let hello = c.hello().await;
    assert_eq!((hello.api_version_major, hello.api_version_minor), (1, 14));
    assert_eq!(hello.name, "test-tv");
    assert!(matches!(
        events.recv().await,
        Some(AppEvent::HaConnected { .. })
    ));

    c.send(&pb::DeviceInfoRequest {}).await;
    let info: pb::DeviceInfoResponse = c.recv_as().await;
    assert_eq!(info.mac_address, "02:11:22:33:44:55");
    assert_eq!(
        info.voice_assistant_feature_flags,
        webos_esphome_api::features::DEFAULT
    );

    // Unknown message ids are ignored; ping still works afterwards.
    c.stream.write_all(&[0x00, 0x00, 0xe7, 0x07]).await.unwrap();
    c.send(&pb::PingRequest {}).await;
    let _: pb::PingResponse = c.recv_as().await;

    c.send(&pb::SubscribeVoiceAssistantRequest {
        subscribe: true,
        flags: 1,
    })
    .await;
    let req: pb::VoiceAssistantRequest = c.recv_as().await;
    assert!(req.start);
    assert_eq!(req.flags, 2);
    c.send(&pb::VoiceAssistantResponse {
        port: 0,
        error: false,
    })
    .await;
    assert!(matches!(
        events.recv().await,
        Some(AppEvent::StartStreaming {
            wake_word: true,
            ..
        })
    ));
    cmds.send(AppCommand::Audio(vec![1, -2, 3])).await.unwrap();
    let audio: pb::VoiceAssistantAudio = c.recv_as().await;
    assert_eq!(audio.data, [1, 0, 0xfe, 0xff, 3, 0]);

    c.send(&pb::DisconnectRequest::default()).await;
    let _: pb::DisconnectResponse = c.recv_as().await;
    loop {
        match events.recv().await {
            Some(AppEvent::HaDisconnected) => break,
            Some(_) => {}
            None => panic!("events closed"),
        }
    }
}

#[tokio::test]
async fn noise_handshake_and_wrong_key() {
    let (port, _events, _cmds) = start(Some(PSK_B64)).await;
    let psk = frame::decode_psk(PSK_B64).unwrap();

    for (key, should_work) in [(psk, true), ([9_u8; 32], false)] {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut hs = snow::Builder::new(NOISE_PROTOCOL.parse().unwrap())
            .psk(0, &key)
            .unwrap()
            .prologue(NOISE_PROLOGUE)
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut msg = [0_u8; 128];
        let n = hs.write_message(&[], &mut msg[1..]).unwrap();
        let mut out = vec![0x01, 0x00, 0x00]; // client hello
        frame::encode_noise_frame(&msg[..=n], &mut out).unwrap();
        stream.write_all(&out).await.unwrap();

        let mut buf = Vec::new();
        let server_hello = read_noise_frame(&mut stream, &mut buf).await;
        assert_eq!(server_hello, b"\x01test-tv\x00021122334455\x00");
        let reply = read_noise_frame(&mut stream, &mut buf).await;
        if !should_work {
            assert_eq!(
                reply,
                frame::handshake_reject_body(frame::HANDSHAKE_MAC_FAILURE)
            );
            continue;
        }
        assert_eq!(reply[0], 0x00);
        let mut scratch = vec![0_u8; 128];
        hs.read_message(&reply[1..], &mut scratch).unwrap();
        let transport = hs.into_transport_mode().unwrap();
        let mut c = Client {
            stream,
            codec: Codec::Noise(Box::new(test_transport(transport))),
            buf,
        };
        let hello = c.hello().await;
        assert_eq!(hello.name, "test-tv");
    }

    // A plaintext client gets a Noise-framed rejection (HA: "requires encryption").
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut out = Vec::new();
    frame::encode_plaintext(pb::HelloRequest::ID, &[], &mut out).unwrap();
    stream.write_all(&out).await.unwrap();
    let mut first = [0_u8; 1];
    stream.read_exact(&mut first).await.unwrap();
    assert_eq!(first[0], frame::NOISE_PREAMBLE);
}

/// Wraps a client-side snow transport in the crate's codec (symmetric API).
fn test_transport(state: snow::TransportState) -> frame::NoiseTransport {
    frame::NoiseTransport::from_state(state)
}
