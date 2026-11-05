//! End-to-end server tests over loopback TCP with a minimal Rust client
//! (plaintext and Noise). The aioesphomeapi harness in tools/esphome-fake-ha
//! covers the HA behaviour in depth.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use webos_esphome_api::frame::{self, Codec, NOISE_PROLOGUE, NOISE_PROTOCOL, RawMessage};
use webos_esphome_api::proto::{self as pb, ApiMessage, VoiceAssistantTimerEvent as E};
use webos_esphome_api::{
    AppCommand, AppEvent, Config, MusicCommand, MusicConfig, MusicState, PipelineMode, Server,
    TimerEvent,
};

const PSK_B64: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=";

async fn start(
    psk: Option<&str>,
) -> (
    u16,
    mpsc::UnboundedReceiver<AppEvent>,
    mpsc::Sender<AppCommand>,
) {
    start_with(psk, webos_esphome_api::features::DEFAULT).await
}

async fn start_with(
    psk: Option<&str>,
    voice_features: u32,
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
        voice_features,
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

fn timer_msg(
    event: pb::VoiceAssistantTimerEvent,
    left: u32,
) -> pb::VoiceAssistantTimerEventResponse {
    pb::VoiceAssistantTimerEventResponse {
        event_type: event as i32,
        timer_id: "t1".into(),
        name: "tea".into(),
        total_seconds: 120,
        seconds_left: left,
        is_active: true,
    }
}

async fn connect_plain(port: u16) -> Client {
    let mut c = Client {
        stream: TcpStream::connect(("127.0.0.1", port)).await.unwrap(),
        codec: Codec::Plaintext,
        buf: Vec::new(),
    };
    c.hello().await;
    c
}

#[tokio::test]
async fn timer_events_reach_the_app_when_timers_are_enabled() {
    let features = webos_esphome_api::features::DEFAULT_WITH_TIMERS;
    let (port, mut events, _cmds) = start_with(None, features).await;
    let mut c = connect_plain(port).await;
    c.send(&pb::DeviceInfoRequest {}).await;
    let info: pb::DeviceInfoResponse = c.recv_as().await;
    assert_eq!(info.voice_assistant_feature_flags, features);

    c.send(&timer_msg(E::VoiceAssistantTimerStarted, 120)).await;
    c.send(&timer_msg(E::VoiceAssistantTimerFinished, 0)).await;
    loop {
        match events.recv().await.expect("events closed") {
            AppEvent::Timer {
                event: TimerEvent::Started,
                id,
                name,
                total_seconds: 120,
                seconds_left: 120,
                is_active: true,
            } => {
                assert_eq!((id.as_str(), name.as_str()), ("t1", "tea"));
                break;
            }
            AppEvent::HaConnected { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(matches!(
        events.recv().await,
        Some(AppEvent::Timer {
            event: TimerEvent::Finished,
            seconds_left: 0,
            ..
        })
    ));
}

#[tokio::test]
async fn timer_events_are_ignored_when_timers_are_not_advertised() {
    let (port, mut events, _cmds) = start(None).await;
    let mut c = connect_plain(port).await;
    c.send(&pb::DeviceInfoRequest {}).await;
    let info: pb::DeviceInfoResponse = c.recv_as().await;
    assert_eq!(info.voice_assistant_feature_flags & 8, 0);
    c.send(&timer_msg(
        pb::VoiceAssistantTimerEvent::VoiceAssistantTimerStarted,
        5,
    ))
    .await;
    // The ping answer proves the timer message was processed before it.
    c.send(&pb::PingRequest {}).await;
    let _: pb::PingResponse = c.recv_as().await;
    assert!(matches!(
        events.recv().await,
        Some(AppEvent::HaConnected { .. })
    ));
    assert!(events.try_recv().is_err(), "no timer event expected");
}

async fn start_cfg(
    cfg: Config,
) -> (
    u16,
    mpsc::UnboundedReceiver<AppEvent>,
    mpsc::Sender<AppCommand>,
) {
    let server = Server::bind(cfg).await.unwrap();
    let port = server.local_addr().unwrap().port();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    tokio::spawn(server.run(ev_tx, cmd_rx));
    (port, ev_rx, cmd_tx)
}

fn plain_cfg() -> Config {
    Config {
        name: "test-tv".into(),
        bind_addr: "127.0.0.1".parse().unwrap(),
        port: 0,
        mac: Some("02:11:22:33:44:55".into()),
        ..Config::default()
    }
}

async fn next_event(events: &mut mpsc::UnboundedReceiver<AppEvent>) -> AppEvent {
    loop {
        match timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timeout")
            .expect("events closed")
        {
            AppEvent::HaConnected { .. } | AppEvent::MuteChanged(_) => {}
            other => return other,
        }
    }
}

#[tokio::test]
async fn reply_volume_on_the_media_player() {
    let (port, mut events, cmds) = start_cfg(plain_cfg()).await;
    cmds.send(AppCommand::SetVolume(40)).await.unwrap();
    let mut c = connect_plain(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let mp: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
    assert_eq!(
        mp.feature_flags,
        webos_esphome_api::entities::MEDIA_PLAYER_FEATURES
    );
    c.send(&pb::SubscribeStatesRequest {}).await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert!((st.volume - 0.4).abs() < 1e-6, "{}", st.volume);

    // HA's slider.
    c.send(&pb::MediaPlayerCommandRequest {
        key: mp.key,
        has_volume: true,
        volume: 0.25,
        ..Default::default()
    })
    .await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert!((st.volume - 0.25).abs() < 1e-6);
    assert_eq!(next_event(&mut events).await, AppEvent::VolumeChanged(25));

    // Volume up and down steps of 5 %.
    for (cmd, want) in [
        (pb::MediaPlayerCommand::VolumeUp, 30),
        (pb::MediaPlayerCommand::VolumeDown, 25),
    ] {
        c.send(&pb::MediaPlayerCommandRequest {
            key: mp.key,
            has_command: true,
            command: cmd as i32,
            ..Default::default()
        })
        .await;
        let st: pb::MediaPlayerStateResponse = c.recv_as().await;
        assert!((st.volume - f32::from(want) / 100.0).abs() < 1e-6);
        assert_eq!(next_event(&mut events).await, AppEvent::VolumeChanged(want));
    }
    // An out-of-range value is clamped; the same value again is not an event.
    c.send(&pb::MediaPlayerCommandRequest {
        key: mp.key,
        has_volume: true,
        volume: 0.25,
        ..Default::default()
    })
    .await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    c.send(&pb::PingRequest {}).await;
    let _: pb::PingResponse = c.recv_as().await;
    assert!(events.try_recv().is_err(), "unchanged volume: no event");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn two_media_players_are_listed_and_routed_by_key() {
    use webos_esphome_api::entities::{
        self as ents, FEATURE_TURN_OFF, FEATURE_TURN_ON, FEATURE_VOLUME_MUTE, FEATURE_VOLUME_SET,
        FEATURE_VOLUME_STEP,
    };
    use webos_esphome_api::{MediaPlayerEntity, PlayerCommand, PlayerState, PlayerStatus};
    let flags = FEATURE_VOLUME_SET
        | FEATURE_VOLUME_STEP
        | FEATURE_VOLUME_MUTE
        | FEATURE_TURN_ON
        | FEATURE_TURN_OFF;
    let mut cfg = plain_cfg();
    cfg.media_players.push(MediaPlayerEntity {
        object_id: "tv_media_player".into(),
        name: "TV".into(),
        icon: "mdi:television".into(),
        feature_flags: flags,
        entity_category: pb::EntityCategory::None,
    });
    let (port, mut events, cmds) = start_cfg(cfg).await;
    cmds.send(AppCommand::SetVolume(40)).await.unwrap();
    let mut c = connect_plain(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let sat: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
    let tv: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
    // The satellite player is unchanged: same object id, key, format, features.
    assert_eq!(sat.object_id, "media_player");
    assert_eq!(sat.key, ents::entity_key("media_player"));
    assert_eq!(sat.feature_flags, ents::MEDIA_PLAYER_FEATURES);
    assert_eq!(sat.supported_formats.len(), 1);
    // The TV player: own key, no format, controls only.
    assert_eq!(
        (tv.object_id.as_str(), tv.name.as_str()),
        ("tv_media_player", "TV")
    );
    assert_eq!(tv.key, ents::entity_key("tv_media_player"));
    assert_ne!(tv.key, sat.key);
    assert_eq!(tv.supported_formats.len(), 0, "no audio format");
    assert_eq!(tv.feature_flags, flags);

    // States: the TV player has none until the application sends one.
    c.send(&pb::SubscribeStatesRequest {}).await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.key, sat.key);
    c.send(&pb::PingRequest {}).await;
    loop {
        let m = c.recv().await;
        assert_ne!(
            m.type_id,
            pb::MediaPlayerStateResponse::ID,
            "no TV player state before the application sent one"
        );
        if m.type_id == pb::PingResponse::ID {
            break;
        }
    }
    let on = PlayerStatus {
        state: PlayerState::On,
        volume: Some(0.2),
        muted: false,
    };
    for status in [on, on] {
        cmds.send(AppCommand::SetMediaPlayer {
            object_id: "tv_media_player".into(),
            status,
        })
        .await
        .unwrap();
    }
    cmds.send(AppCommand::SetMediaPlayer {
        object_id: "tv_media_player".into(),
        status: PlayerStatus {
            state: PlayerState::Off,
            volume: None,
            muted: true,
        },
    })
    .await
    .unwrap();
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(
        (st.key, st.state),
        (tv.key, pb::MediaPlayerState::On as i32)
    );
    assert!((st.volume - 0.2).abs() < 1e-6 && !st.muted);
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(
        (st.key, st.state),
        (tv.key, pb::MediaPlayerState::Off as i32),
        "no duplicate of an unchanged state"
    );
    assert!(st.volume.is_nan() && st.muted);

    // Commands to the TV player become events and are not echoed.
    let send = |key: u32, cmd: Option<pb::MediaPlayerCommand>, vol: Option<f32>| {
        pb::MediaPlayerCommandRequest {
            key,
            has_command: cmd.is_some(),
            command: cmd.map_or(0, |c| c as i32),
            has_volume: vol.is_some(),
            volume: vol.unwrap_or_default(),
            ..Default::default()
        }
    };
    for (cmd, vol, want) in [
        (None, Some(0.5), PlayerCommand::SetVolume(0.5)),
        (
            Some(pb::MediaPlayerCommand::VolumeUp),
            None,
            PlayerCommand::VolumeUp,
        ),
        (
            Some(pb::MediaPlayerCommand::Mute),
            None,
            PlayerCommand::Mute(true),
        ),
        (
            Some(pb::MediaPlayerCommand::Unmute),
            None,
            PlayerCommand::Mute(false),
        ),
        (
            Some(pb::MediaPlayerCommand::TurnOff),
            None,
            PlayerCommand::TurnOff,
        ),
        (
            Some(pb::MediaPlayerCommand::TurnOn),
            None,
            PlayerCommand::TurnOn,
        ),
    ] {
        c.send(&send(tv.key, cmd, vol)).await;
        assert_eq!(
            next_event(&mut events).await,
            AppEvent::MediaPlayerCommand {
                object_id: "tv_media_player".into(),
                command: want
            }
        );
    }
    // Play media / an announcement to the TV player and a stop (not
    // advertised): no event, the last state is sent again.
    c.send(&pb::MediaPlayerCommandRequest {
        key: tv.key,
        has_media_url: true,
        media_url: "http://h/a.flac".into(),
        has_announcement: true,
        announcement: true,
        ..Default::default()
    })
    .await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.key, tv.key);
    c.send(&send(tv.key, Some(pb::MediaPlayerCommand::Stop), None))
        .await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.key, tv.key);

    // The satellite player's volume still works and only it moves; its stop
    // is MediaStop, never a TV command.
    c.send(&send(sat.key, None, Some(0.3))).await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.key, sat.key);
    assert!((st.volume - 0.3).abs() < 1e-6);
    assert_eq!(next_event(&mut events).await, AppEvent::VolumeChanged(30));
    c.send(&send(sat.key, Some(pb::MediaPlayerCommand::Stop), None))
        .await;
    assert_eq!(next_event(&mut events).await, AppEvent::MediaStop);
    // An unknown key does nothing.
    c.send(&send(777, Some(pb::MediaPlayerCommand::TurnOff), None))
        .await;
    c.send(&pb::PingRequest {}).await;
    let _: pb::PingResponse = c.recv_as().await;
    assert!(events.try_recv().is_err(), "unknown key: no event");

    // A reconnect gets both states again.
    drop(c);
    let mut c = connect_plain(port).await;
    c.send(&pb::SubscribeStatesRequest {}).await;
    let a: pb::MediaPlayerStateResponse = c.recv_as().await;
    let b: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!((a.key, b.key), (sat.key, tv.key));
    assert_eq!(b.state, pb::MediaPlayerState::Off as i32);
}

#[tokio::test]
async fn power_sensor_and_select_entities() {
    let mut cfg = plain_cfg();
    cfg.power_sensor = true;
    cfg.selects.push(webos_esphome_api::SelectEntity {
        object_id: "remote_power_off".into(),
        name: "When the TV is turned off with the remote".into(),
        icon: "mdi:power-standby".into(),
        options: vec![
            "Turn off normally".into(),
            "Keep listening (screen off)".into(),
        ],
        entity_category: pb::EntityCategory::Config,
        command_echo: true,
    });
    let (port, mut events, cmds) = start_cfg(cfg).await;
    cmds.send(AppCommand::SetSelect {
        object_id: "remote_power_off".into(),
        option: "Turn off normally".into(),
    })
    .await
    .unwrap();
    let mut c = connect_plain(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let ts: pb::ListEntitiesTextSensorResponse = c.recv_as().await;
    assert_eq!(ts.entity_category, pb::EntityCategory::Diagnostic as i32);
    let sel: pb::ListEntitiesSelectResponse = c.recv_as().await;
    assert_eq!(sel.options.len(), 2);
    assert_eq!(sel.entity_category, pb::EntityCategory::Config as i32);
    let _: pb::ListEntitiesDoneResponse = c.recv_as().await;

    c.send(&pb::SubscribeStatesRequest {}).await;
    let p: pb::TextSensorStateResponse = c.recv_as().await;
    assert!(p.missing_state, "no power state yet");
    let s: pb::SelectStateResponse = c.recv_as().await;
    assert_eq!(s.state, "Turn off normally");

    cmds.send(AppCommand::SetPowerState("Active".into()))
        .await
        .unwrap();
    let p: pb::TextSensorStateResponse = c.recv_as().await;
    assert_eq!((p.state.as_str(), p.missing_state), ("Active", false));

    // An invalid option is answered with the current state, no event.
    c.send(&pb::SelectCommandRequest {
        key: sel.key,
        state: "Darken".into(),
        ..Default::default()
    })
    .await;
    let s: pb::SelectStateResponse = c.recv_as().await;
    assert_eq!(s.state, "Turn off normally");
    c.send(&pb::SelectCommandRequest {
        key: sel.key,
        state: "Keep listening (screen off)".into(),
        ..Default::default()
    })
    .await;
    let s: pb::SelectStateResponse = c.recv_as().await;
    assert_eq!(s.state, "Keep listening (screen off)");
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::SelectChanged {
            object_id: "remote_power_off".into(),
            option: "Keep listening (screen off)".into(),
        }
    );
}

fn tv_entities_cfg() -> Config {
    use webos_esphome_api::config::{BinarySensorEntity, ButtonEntity, NumberEntity};
    let mut cfg = plain_cfg();
    cfg.numbers.push(NumberEntity {
        object_id: "tv_volume".into(),
        name: "TV volume".into(),
        icon: "mdi:volume-high".into(),
        min_value: 0.0,
        max_value: 100.0,
        step: 1.0,
        unit: "%".into(),
        device_class: "volume".into(),
        mode: pb::NumberMode::Slider,
        entity_category: pb::EntityCategory::None,
    });
    cfg.buttons.push(ButtonEntity {
        object_id: "tv_mute".into(),
        name: "TV mute".into(),
        icon: "mdi:volume-off".into(),
        device_class: String::new(),
        entity_category: pb::EntityCategory::Config,
    });
    cfg.binary_sensors.push(BinarySensorEntity {
        object_id: "listening".into(),
        name: "Listening".into(),
        icon: "mdi:ear-hearing".into(),
        device_class: "running".into(),
        entity_category: pb::EntityCategory::None,
    });
    cfg
}

#[tokio::test]
#[allow(clippy::too_many_lines, clippy::float_cmp)]
async fn number_button_and_binary_sensor_entities() {
    let (port, mut events, cmds) = start_cfg(tv_entities_cfg()).await;
    let mut c = connect_plain(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let num: pb::ListEntitiesNumberResponse = c.recv_as().await;
    assert_eq!(num.object_id, "tv_volume");
    assert_eq!((num.min_value, num.max_value, num.step), (0.0, 100.0, 1.0));
    assert_eq!(num.unit_of_measurement, "%");
    assert_eq!(num.device_class, "volume");
    assert_eq!(num.mode, pb::NumberMode::Slider as i32);
    let btn: pb::ListEntitiesButtonResponse = c.recv_as().await;
    assert_eq!(btn.object_id, "tv_mute");
    assert_eq!(btn.entity_category, pb::EntityCategory::Config as i32);
    let bin: pb::ListEntitiesBinarySensorResponse = c.recv_as().await;
    assert_eq!(bin.object_id, "listening");
    assert_eq!(bin.device_class, "running");
    let _: pb::ListEntitiesDoneResponse = c.recv_as().await;
    // Keys are the stable FNV-1 hashes of the object ids.
    assert_eq!(
        num.key,
        webos_esphome_api::entities::entity_key("tv_volume")
    );
    assert_eq!(
        bin.key,
        webos_esphome_api::entities::entity_key("listening")
    );

    // Before any state: "missing state" (HA shows unknown), not off.
    c.send(&pb::SubscribeStatesRequest {}).await;
    let n: pb::NumberStateResponse = c.recv_as().await;
    assert!(n.missing_state);
    let b: pb::BinarySensorStateResponse = c.recv_as().await;
    assert!(b.missing_state);

    // Changes are pushed; an unchanged state is not repeated.
    cmds.send(AppCommand::SetBinarySensor {
        object_id: "listening".into(),
        state: Some(true),
    })
    .await
    .unwrap();
    let b: pb::BinarySensorStateResponse = c.recv_as().await;
    assert_eq!((b.state, b.missing_state), (true, false));
    cmds.send(AppCommand::SetBinarySensor {
        object_id: "listening".into(),
        state: Some(true),
    })
    .await
    .unwrap();
    cmds.send(AppCommand::SetNumber {
        object_id: "tv_volume".into(),
        value: Some(30.5),
    })
    .await
    .unwrap();
    // The duplicate binary sensor state was dropped: the next frame is the number.
    let n: pb::NumberStateResponse = c.recv_as().await;
    assert_eq!((n.state, n.missing_state), (30.5, false));
    // Unknown ids and out-of-range values are ignored (no state frame).
    cmds.send(AppCommand::SetNumber {
        object_id: "tv_volume".into(),
        value: Some(101.0),
    })
    .await
    .unwrap();
    cmds.send(AppCommand::SetNumber {
        object_id: "nope".into(),
        value: Some(1.0),
    })
    .await
    .unwrap();
    cmds.send(AppCommand::SetBinarySensor {
        object_id: "listening".into(),
        state: Some(false),
    })
    .await
    .unwrap();
    let b: pb::BinarySensorStateResponse = c.recv_as().await;
    assert_eq!((b.state, b.missing_state), (false, false));

    // HA commands: in-range value -> event, no echo; out of range -> the
    // current state is re-sent and no event; button press -> event.
    c.send(&pb::NumberCommandRequest {
        key: num.key,
        state: 55.0,
        ..Default::default()
    })
    .await;
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::NumberChanged {
            object_id: "tv_volume".into(),
            value: 55.0
        }
    );
    c.send(&pb::NumberCommandRequest {
        key: num.key,
        state: f32::NAN,
        ..Default::default()
    })
    .await;
    let n: pb::NumberStateResponse = c.recv_as().await;
    assert_eq!(n.state, 30.5, "the current state, not the rejected value");
    c.send(&pb::NumberCommandRequest {
        key: num.key,
        state: -1.0,
        ..Default::default()
    })
    .await;
    let n: pb::NumberStateResponse = c.recv_as().await;
    assert_eq!(n.state, 30.5);
    c.send(&pb::NumberCommandRequest {
        key: 12345,
        state: 5.0,
        ..Default::default()
    })
    .await;
    c.send(&pb::ButtonCommandRequest {
        key: 12345,
        ..Default::default()
    })
    .await;
    c.send(&pb::ButtonCommandRequest {
        key: btn.key,
        ..Default::default()
    })
    .await;
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::ButtonPressed {
            object_id: "tv_mute".into()
        }
    );
    // A command for the binary sensor's key changes nothing.
    c.send(&pb::SwitchCommandRequest {
        key: bin.key,
        state: true,
        ..Default::default()
    })
    .await;

    // A reconnect gets the stored states again, current values included.
    let mut c2 = connect_plain(port).await;
    c2.send(&pb::SubscribeStatesRequest {}).await;
    let n: pb::NumberStateResponse = c2.recv_as().await;
    assert_eq!((n.state, n.missing_state), (30.5, false));
    let b: pb::BinarySensorStateResponse = c2.recv_as().await;
    assert_eq!((b.state, b.missing_state), (false, false));
    // None puts a sensor back to unknown.
    cmds.send(AppCommand::SetBinarySensor {
        object_id: "listening".into(),
        state: None,
    })
    .await
    .unwrap();
    let b: pb::BinarySensorStateResponse = c2.recv_as().await;
    assert!(b.missing_state);
}

fn tv_switch_cfg() -> Config {
    use webos_esphome_api::config::{SwitchEntity, TextSensorEntity};
    let mut cfg = plain_cfg();
    cfg.switches.push(SwitchEntity {
        object_id: "tv_screen".into(),
        name: "TV screen".into(),
        icon: "mdi:television".into(),
        device_class: "switch".into(),
        entity_category: pb::EntityCategory::None,
    });
    cfg.text_sensors.push(TextSensorEntity {
        object_id: "tv_sound_output".into(),
        name: "TV sound output".into(),
        icon: String::new(),
        device_class: String::new(),
        entity_category: pb::EntityCategory::Diagnostic,
    });
    cfg.selects.push(webos_esphome_api::SelectEntity {
        object_id: "tv_input".into(),
        name: "TV input".into(),
        icon: "mdi:video-input-hdmi".into(),
        options: vec!["HDMI 1".into(), "Other".into()],
        entity_category: pb::EntityCategory::None,
        command_echo: false,
    });
    cfg
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn switch_text_sensor_and_app_owned_select_entities() {
    let (port, mut events, cmds) = start_cfg(tv_switch_cfg()).await;
    let mut c = connect_plain(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    // Order: media player, the mute switch, generic switches, text sensors, selects.
    let mute: pb::ListEntitiesSwitchResponse = c.recv_as().await;
    assert_eq!(mute.object_id, "mute_microphone");
    let sw: pb::ListEntitiesSwitchResponse = c.recv_as().await;
    assert_eq!(sw.object_id, "tv_screen");
    assert_eq!(sw.device_class, "switch");
    assert_eq!(sw.entity_category, pb::EntityCategory::None as i32);
    assert_eq!(sw.key, webos_esphome_api::entities::entity_key("tv_screen"));
    let ts: pb::ListEntitiesTextSensorResponse = c.recv_as().await;
    assert_eq!(ts.object_id, "tv_sound_output");
    assert_eq!(ts.entity_category, pb::EntityCategory::Diagnostic as i32);
    let sel: pb::ListEntitiesSelectResponse = c.recv_as().await;
    assert_eq!(sel.options, ["HDMI 1", "Other"]);
    assert_eq!(sel.entity_category, pb::EntityCategory::None as i32);
    let _: pb::ListEntitiesDoneResponse = c.recv_as().await;

    // Missing states first.
    c.send(&pb::SubscribeStatesRequest {}).await;
    let _: pb::SwitchStateResponse = c.recv_as().await; // the mute switch
    let st: pb::SelectStateResponse = c.recv_as().await;
    assert!(st.missing_state);
    let s: pb::SwitchStateResponse = c.recv_as().await;
    assert!(s.missing_state && s.key == sw.key);
    let t: pb::TextSensorStateResponse = c.recv_as().await;
    assert!(t.missing_state);

    // Pushed states; an unchanged one is not repeated.
    for state in [Some(true), Some(true), Some(false)] {
        cmds.send(AppCommand::SetSwitch {
            object_id: "tv_screen".into(),
            state,
        })
        .await
        .unwrap();
    }
    let s: pb::SwitchStateResponse = c.recv_as().await;
    assert_eq!((s.state, s.missing_state), (true, false));
    let s: pb::SwitchStateResponse = c.recv_as().await;
    assert_eq!((s.state, s.missing_state), (false, false), "no duplicate");
    cmds.send(AppCommand::SetTextSensor {
        object_id: "tv_sound_output".into(),
        state: Some("external_arc".into()),
    })
    .await
    .unwrap();
    let t: pb::TextSensorStateResponse = c.recv_as().await;
    assert_eq!((t.state.as_str(), t.missing_state), ("external_arc", false));
    cmds.send(AppCommand::SetTextSensor {
        object_id: "tv_sound_output".into(),
        state: None,
    })
    .await
    .unwrap();
    let t: pb::TextSensorStateResponse = c.recv_as().await;
    assert!(t.missing_state);
    // Unknown ids are ignored.
    cmds.send(AppCommand::SetSwitch {
        object_id: "nope".into(),
        state: Some(true),
    })
    .await
    .unwrap();

    // A switch command is an event and no echo; the mute switch still works
    // as before (no SwitchChanged for it); an unknown key does nothing.
    c.send(&pb::SwitchCommandRequest {
        key: sw.key,
        state: true,
        ..Default::default()
    })
    .await;
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::SwitchChanged {
            object_id: "tv_screen".into(),
            state: true
        }
    );
    c.send(&pb::SwitchCommandRequest {
        key: 777,
        state: true,
        ..Default::default()
    })
    .await;

    // An app-owned select: a valid option is an event and no state frame
    // follows; the state is what the application reports; an invalid option
    // is answered with the current state.
    cmds.send(AppCommand::SetSelect {
        object_id: "tv_input".into(),
        option: "Other".into(),
    })
    .await
    .unwrap();
    let st: pb::SelectStateResponse = c.recv_as().await;
    assert_eq!(st.state, "Other");
    c.send(&pb::SelectCommandRequest {
        key: sel.key,
        state: "HDMI 1".into(),
        ..Default::default()
    })
    .await;
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::SelectChanged {
            object_id: "tv_input".into(),
            option: "HDMI 1".into()
        }
    );
    c.send(&pb::SelectCommandRequest {
        key: sel.key,
        state: "HDMI 9".into(),
        ..Default::default()
    })
    .await;
    let st: pb::SelectStateResponse = c.recv_as().await;
    assert_eq!(
        st.state, "Other",
        "the application's state, not the command"
    );
    cmds.send(AppCommand::SetSelect {
        object_id: "tv_input".into(),
        option: "HDMI 1".into(),
    })
    .await
    .unwrap();
    let st: pb::SelectStateResponse = c.recv_as().await;
    assert_eq!(st.state, "HDMI 1");
    cmds.send(AppCommand::ClearSelect {
        object_id: "tv_input".into(),
    })
    .await
    .unwrap();
    let st: pb::SelectStateResponse = c.recv_as().await;
    assert!(st.missing_state);

    // New options: the HA connection is closed so HA lists again; the new
    // list is what the next connection sees, with no state until reported.
    cmds.send(AppCommand::SetSelectOptions {
        object_id: "tv_input".into(),
        options: vec!["HDMI 1".into(), "HDMI 2".into(), "Other".into()],
    })
    .await
    .unwrap();
    assert_eq!(next_event(&mut events).await, AppEvent::HaDisconnected);
    let mut c2 = connect_plain(port).await;
    c2.send(&pb::ListEntitiesRequest {}).await;
    let sel2: pb::ListEntitiesSelectResponse = c2.recv_as().await;
    assert_eq!(sel2.options, ["HDMI 1", "HDMI 2", "Other"]);
    assert_eq!(sel2.key, sel.key, "the key is stable");
    c2.send(&pb::SubscribeStatesRequest {}).await;
    let _: pb::SwitchStateResponse = c2.recv_as().await;
    let st: pb::SelectStateResponse = c2.recv_as().await;
    assert!(st.missing_state);
    let s: pb::SwitchStateResponse = c2.recv_as().await;
    assert_eq!(
        (s.state, s.missing_state),
        (false, false),
        "kept across reconnects"
    );
    // The same list again does not close the connection.
    cmds.send(AppCommand::SetSelectOptions {
        object_id: "tv_input".into(),
        options: vec!["HDMI 1".into(), "HDMI 2".into(), "Other".into()],
    })
    .await
    .unwrap();
    cmds.send(AppCommand::SetSelect {
        object_id: "tv_input".into(),
        option: "HDMI 2".into(),
    })
    .await
    .unwrap();
    let st: pb::SelectStateResponse = c2.recv_as().await;
    assert_eq!(st.state, "HDMI 2");
}

#[tokio::test]
async fn resume_probe_keeps_a_live_connection_and_drops_a_silent_one() {
    let (port, mut events, cmds) = start_cfg(plain_cfg()).await;
    let mut c = connect_plain(port).await;
    cmds.send(AppCommand::Resumed).await.unwrap();
    // A live HA answers the probe ping and stays connected.
    let _: pb::PingRequest = c.recv_as().await;
    c.send(&pb::PingResponse {}).await;
    c.send(&pb::PingRequest {}).await;
    let _: pb::PingResponse = c.recv_as().await;

    // A client that does not answer is dropped after the probe timeout.
    cmds.send(AppCommand::Resumed).await.unwrap();
    let _: pb::PingRequest = c.recv_as().await;
    let ev = timeout(Duration::from_secs(10), async {
        loop {
            if events.recv().await.expect("events closed") == AppEvent::HaDisconnected {
                return true;
            }
        }
    })
    .await
    .expect("not dropped");
    assert!(ev);
}

// ---- Music on the built-in media player (design M2) ----

fn music_cfg(psk: Option<&str>, music: Option<MusicConfig>) -> Config {
    Config {
        noise_psk: psk.map(str::to_owned),
        music,
        ..plain_cfg()
    }
}

/// A Noise client (the same handshake as `noise_handshake_and_wrong_key`).
async fn connect_noise(port: u16) -> Client {
    let psk = frame::decode_psk(PSK_B64).unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut hs = snow::Builder::new(NOISE_PROTOCOL.parse().unwrap())
        .psk(0, &psk)
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
    let _server_hello = read_noise_frame(&mut stream, &mut buf).await;
    let reply = read_noise_frame(&mut stream, &mut buf).await;
    assert_eq!(reply[0], 0x00);
    let mut scratch = vec![0_u8; 128];
    hs.read_message(&reply[1..], &mut scratch).unwrap();
    let mut c = Client {
        stream,
        codec: Codec::Noise(Box::new(test_transport(hs.into_transport_mode().unwrap()))),
        buf,
    };
    c.hello().await;
    c
}

fn mp_cmd(key: u32, command: pb::MediaPlayerCommand) -> pb::MediaPlayerCommandRequest {
    pb::MediaPlayerCommandRequest {
        key,
        has_command: true,
        command: command as i32,
        ..Default::default()
    }
}

fn mp_url(key: u32, url: &str, announce: Option<bool>) -> pb::MediaPlayerCommandRequest {
    pb::MediaPlayerCommandRequest {
        key,
        has_media_url: true,
        media_url: url.into(),
        has_announcement: announce.is_some(),
        announcement: announce.unwrap_or(false),
        ..Default::default()
    }
}

fn mp_enqueue(key: u32, url: &str) -> pb::MediaPlayerCommandRequest {
    pb::MediaPlayerCommandRequest {
        has_command: true,
        command: pb::MediaPlayerCommand::Enqueue as i32,
        ..mp_url(key, url, None)
    }
}

/// A ping round trip: everything the server did for earlier requests (events
/// included) has happened when it returns.
async fn sync(c: &mut Client) {
    c.send(&pb::PingRequest {}).await;
    let _: pb::PingResponse = c.recv_as().await;
}

fn no_event(events: &mut mpsc::UnboundedReceiver<AppEvent>) {
    loop {
        match events.try_recv() {
            Ok(AppEvent::HaConnected { .. } | AppEvent::MuteChanged(_)) => {}
            Ok(other) => panic!("unexpected event {other:?}"),
            Err(_) => return,
        }
    }
}

#[tokio::test]
async fn music_off_keeps_flags_formats_and_ignores_media_commands() {
    let (port, mut events, cmds) = start_cfg(music_cfg(None, None)).await;
    let mut c = connect_plain(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let mp: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
    assert_eq!(mp.feature_flags, 5124);
    assert_eq!(mp.supported_formats.len(), 1);
    assert_eq!(
        mp.supported_formats[0].purpose,
        pb::MediaPlayerFormatPurpose::Announcement as i32
    );
    c.send(&mp_url(mp.key, "http://h/a.flac", None)).await;
    c.send(&mp_url(mp.key, "http://h/a.flac", Some(true))).await;
    for cmd in [
        pb::MediaPlayerCommand::Pause,
        pb::MediaPlayerCommand::Play,
        pb::MediaPlayerCommand::Enqueue,
    ] {
        c.send(&mp_cmd(mp.key, cmd)).await;
    }
    c.send(&mp_enqueue(mp.key, "http://h/b.flac")).await;
    sync(&mut c).await;
    no_event(&mut events);
    // SetMusicState is ignored: the state stays idle.
    cmds.send(AppCommand::SetMusicState(MusicState::Playing))
        .await
        .unwrap();
    c.send(&pb::SubscribeStatesRequest {}).await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.state, pb::MediaPlayerState::Idle as i32);
}

#[tokio::test]
async fn music_on_lists_flags_and_the_default_format_after_the_announcement_one() {
    for (announce, flags) in [(true, 1_201_669), (false, 153_093)] {
        let music = MusicConfig {
            announce,
            ..MusicConfig::default()
        };
        let (port, _events, _cmds) = start_cfg(music_cfg(None, Some(music))).await;
        let mut c = connect_plain(port).await;
        c.send(&pb::ListEntitiesRequest {}).await;
        let mp: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
        assert_eq!(mp.feature_flags, flags);
        assert_eq!(mp.supported_formats.len(), 2);
        let (a, d) = (&mp.supported_formats[0], &mp.supported_formats[1]);
        assert_eq!(a.purpose, pb::MediaPlayerFormatPurpose::Announcement as i32);
        assert_eq!((a.sample_rate, a.num_channels), (22_050, 1));
        assert_eq!(d.purpose, pb::MediaPlayerFormatPurpose::Default as i32);
        assert_eq!(
            (
                d.format.as_str(),
                d.sample_rate,
                d.num_channels,
                d.sample_bytes
            ),
            ("flac", 48_000, 2, 2)
        );
    }
    // WAV is selectable; MP3 is never advertised.
    let mut wav = MusicConfig::default();
    "wav".clone_into(&mut wav.format.format);
    assert!(music_cfg(None, Some(wav)).validate().is_ok());
    let mut mp3 = MusicConfig::default();
    "mp3".clone_into(&mut mp3.format.format);
    assert!(music_cfg(None, Some(mp3)).validate().is_err());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn music_commands_become_events_on_an_encrypted_session() {
    let (port, mut events, _cmds) =
        start_cfg(music_cfg(Some(PSK_B64), Some(MusicConfig::default()))).await;
    let mut c = connect_noise(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let mp: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
    c.send(&pb::SubscribeStatesRequest {}).await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    let url = "http://ma.lan:8097/flow/abc.flac";
    let play = |announce| {
        AppEvent::Music(MusicCommand::Play {
            url: url.into(),
            announce,
        })
    };

    c.send(&mp_url(mp.key, url, None)).await;
    assert_eq!(next_event(&mut events).await, play(false));
    c.send(&mp_url(mp.key, url, Some(true))).await;
    assert_eq!(next_event(&mut events).await, play(true));
    // An explicit announcement: false is a plain play.
    c.send(&mp_url(mp.key, url, Some(false))).await;
    assert_eq!(next_event(&mut events).await, play(false));
    c.send(&mp_cmd(mp.key, pb::MediaPlayerCommand::Pause)).await;
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::Music(MusicCommand::Pause)
    );
    c.send(&mp_cmd(mp.key, pb::MediaPlayerCommand::Play)).await;
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::Music(MusicCommand::Resume)
    );
    c.send(&mp_cmd(mp.key, pb::MediaPlayerCommand::Stop)).await;
    assert_eq!(next_event(&mut events).await, AppEvent::MediaStop);
    c.send(&pb::MediaPlayerCommandRequest {
        key: mp.key,
        has_volume: true,
        volume: 0.3,
        ..Default::default()
    })
    .await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert!((st.volume - 0.3).abs() < 1e-6);
    assert_eq!(next_event(&mut events).await, AppEvent::VolumeChanged(30));

    // ENQUEUE with a URL: refused, the URL is handed up, the state echoed.
    c.send(&mp_enqueue(mp.key, url)).await;
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.state, pb::MediaPlayerState::Idle as i32);
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::Music(MusicCommand::EnqueueRefused {
            url: Some(url.into())
        })
    );
    // ENQUEUE without a URL: refused, nothing handed up.
    c.send(&mp_cmd(mp.key, pb::MediaPlayerCommand::Enqueue))
        .await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;

    // Mute, unmute, toggle and an empty URL do nothing.
    for cmd in [
        pb::MediaPlayerCommand::Mute,
        pb::MediaPlayerCommand::Unmute,
        pb::MediaPlayerCommand::Toggle,
    ] {
        c.send(&mp_cmd(mp.key, cmd)).await;
    }
    c.send(&mp_url(mp.key, "", None)).await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    sync(&mut c).await;
    no_event(&mut events);
}

#[tokio::test]
async fn announce_is_refused_when_it_is_not_offered() {
    let music = MusicConfig {
        announce: false,
        ..MusicConfig::default()
    };
    let (port, mut events, _cmds) = start_cfg(music_cfg(Some(PSK_B64), Some(music))).await;
    let mut c = connect_noise(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let mp: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
    c.send(&pb::SubscribeStatesRequest {}).await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    c.send(&mp_url(mp.key, "http://h/a.flac", Some(true))).await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    sync(&mut c).await;
    no_event(&mut events);
    c.send(&mp_url(mp.key, "http://h/a.flac", None)).await;
    assert!(matches!(
        next_event(&mut events).await,
        AppEvent::Music(MusicCommand::Play {
            announce: false,
            ..
        })
    ));
}

#[tokio::test]
async fn media_urls_are_refused_on_a_plaintext_session_unless_allowed() {
    let (port, mut events, _cmds) = start_cfg(music_cfg(None, Some(MusicConfig::default()))).await;
    let mut c = connect_plain(port).await;
    c.send(&pb::ListEntitiesRequest {}).await;
    let mp: pb::ListEntitiesMediaPlayerResponse = c.recv_as().await;
    c.send(&pb::SubscribeStatesRequest {}).await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    c.send(&mp_url(mp.key, "http://h/a.flac", None)).await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    c.send(&mp_enqueue(mp.key, "http://h/a.flac")).await;
    let _: pb::MediaPlayerStateResponse = c.recv_as().await;
    sync(&mut c).await;
    no_event(&mut events);
    // Controls are not media URLs: pause still arrives.
    c.send(&mp_cmd(mp.key, pb::MediaPlayerCommand::Pause)).await;
    assert_eq!(
        next_event(&mut events).await,
        AppEvent::Music(MusicCommand::Pause)
    );

    let music = MusicConfig {
        allow_plaintext: true,
        ..MusicConfig::default()
    };
    let (port, mut events, _cmds) = start_cfg(music_cfg(None, Some(music))).await;
    let mut c = connect_plain(port).await;
    c.send(&mp_url(mp.key, "http://h/a.flac", None)).await;
    assert!(matches!(
        next_event(&mut events).await,
        AppEvent::Music(MusicCommand::Play { .. })
    ));
}

#[tokio::test]
async fn music_state_is_reported_merged_with_the_voice_state() {
    let (port, _events, cmds) = start_cfg(music_cfg(None, Some(MusicConfig::default()))).await;
    cmds.send(AppCommand::SetVolume(40)).await.unwrap();
    cmds.send(AppCommand::SetMusicState(MusicState::Paused))
        .await
        .unwrap();
    let mut c = connect_plain(port).await;
    c.send(&pb::SubscribeStatesRequest {}).await;
    // The state held at connect time (set before HA connected).
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.state, pb::MediaPlayerState::Paused as i32);
    assert!((st.volume - 0.4).abs() < 1e-6);

    for (state, want) in [
        (MusicState::Playing, pb::MediaPlayerState::Playing),
        (MusicState::Idle, pb::MediaPlayerState::Idle),
    ] {
        cmds.send(AppCommand::SetMusicState(state)).await.unwrap();
        let st: pb::MediaPlayerStateResponse = c.recv_as().await;
        assert_eq!(st.state, want as i32);
        assert!((st.volume - 0.4).abs() < 1e-6);
    }
    // An unchanged state is not repeated: the next message is the volume's.
    cmds.send(AppCommand::SetMusicState(MusicState::Idle))
        .await
        .unwrap();
    cmds.send(AppCommand::SetVolume(55)).await.unwrap();
    let st: pb::MediaPlayerStateResponse = c.recv_as().await;
    assert_eq!(st.state, pb::MediaPlayerState::Idle as i32);
    assert!((st.volume - 0.55).abs() < 1e-6);
}
