# ESPHome native API satellite in Rust: protocol, reuse and target research

**Status:** research note, desk research only. No device access was used. Consulted: 2026-11-01.

**Question.** What exactly must a Rust daemon on the rooted LG C3 implement so that Home Assistant (HA) 2026.10.4's ESPHome integration creates a native Assist satellite? The design uses server-side (HA) wake word and TTS playback on the TV. Which existing code, crates and target settings fit the TV userland?

**Why it matters.** The accepted direction is: ESPHome native API device, Rust, and wake word in HA for the MVP. This note turns that direction into concrete protocol obligations and build choices, and lists what must still be checked on the device.

**Evidence labels.** Each claim is marked:

- **[VS]** verified-from-source: read in the cited file at the cited tag or commit. In the repository's scheme this is `documented`. It is **not** `verified-on-target`.
- **[INF]** inferred: reasoning from sources, not stated by them. In the repository's scheme this is `hypothesis`.

Nothing in this note is verified on the TV.

**Pinned sources** (all read 2026-11-01):

| Repo | Ref read | Why this ref |
| --- | --- | --- |
| esphome/esphome | tag `2026.10.1` (latest stable). Dev `dddacfa` (2026.11.0-dev) was diffed: the voice proto section is identical. | Device-side reference behaviour |
| home-assistant/core | tag `2026.10.4` (`357cae5`), the user's version. Dev `99cc036` was used where noted. | Client (HA) behaviour |
| esphome/aioesphomeapi | `v46.6.0` (`26451b4`), pinned by HA 2026.10.4 [manifest](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/manifest.json). Main is v46.25.1. | Client library and test harness |
| OHF-Voice/linux-voice-assistant | `5b980d0` (v1.1.18, 2026-10-29) | Reference satellite |
| UbiHome/esphome-native-api | `125b541` (crate 3.0.0, 2026-07-15) | Rust implementation |

Short link names used below:

- `api.proto` = [esphome 2026.10.1 api.proto](https://github.com/esphome/esphome/blob/2026.10.1/esphome/components/api/api.proto)
- `va.cpp` = [esphome 2026.10.1 voice_assistant.cpp](https://github.com/esphome/esphome/blob/2026.10.1/esphome/components/voice_assistant/voice_assistant.cpp)
- `ha_sat` = [HA 2026.10.4 esphome/assist_satellite.py](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/assist_satellite.py)
- `ha_mgr` = [HA 2026.10.4 esphome/manager.py](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/manager.py)
- `aio_client` = [aioesphomeapi v46.6.0 client.py](https://github.com/esphome/aioesphomeapi/blob/v46.6.0/aioesphomeapi/client.py)

**Role naming.** In ESPHome terms the **device is the API "server"**: it listens on TCP. **HA is the "client"**: it dials in. In `api.proto`, `SOURCE_SERVER` means device→HA and `SOURCE_CLIENT` means HA→device. [VS `api.proto`]

## A. ESPHome native API for voice satellites

### A1. Connection basics

- **Port and roles.** HA's default port is 6053 [VS [HA const.py](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/const.py) `DEFAULT_PORT`]. HA connects to the device. ESPHome 2026.x also has an opt-in "outgoing connection" mode, where the device dials a listener in HA. HA 2026.10.4 ships it in `outgoing_connection.py`, and the proto has `HelloRequest.outgoing_connection_target` and `DeviceInfoResponse.api_outgoing_connection_supported` [VS `api.proto`, HA tree at 2026.10.4]. The listener port and setup were not researched. See the open questions.
- **Plaintext framing.** Each frame is: byte `0x00`, varint payload length, varint message type, protobuf payload [VS `api.proto` header comment]. Plaintext is still supported by ESPHome (`USE_API_PLAINTEXT`) and by HA [VS esphome `api/__init__.py`, HA `strings.json`].
- **Noise framing.** Each frame is: `0x01`, a 2-byte big-endian length, then the frame. The client hello is `01 00 00`, followed by a handshake frame that starts with `0x00`. The server hello is `0x01` (the chosen protocol, `Noise_NNpsk0_25519_ChaChaPoly_SHA256`), then the NUL-terminated node name and the NUL-terminated MAC. The prologue is `b"NoiseAPIInit\x00\x00"`. The PSK is the base64-decoded 32-byte key. After the handshake, the plaintext inside each encrypted frame is: 2-byte type (BE), 2-byte length (BE), payload. A handshake failure is reported as a frame starting with `0x01` plus an error string, for example "Handshake MAC failure". [VS [aioesphomeapi noise.py](https://github.com/esphome/aioesphomeapi/blob/v46.6.0/aioesphomeapi/_frame_helper/noise.py), [esphome api_frame_helper_noise.cpp](https://github.com/esphome/esphome/blob/2026.10.1/esphome/components/api/api_frame_helper_noise.cpp)]
- **Handshake.** HA sends `HelloRequest`. aioesphomeapi 46.6.0 sends API 1.18, and HA rejects devices with major > 2. The device answers `HelloResponse`; ESPHome 2026.10.1 reports 1.19 and its node name [VS [aioesphomeapi connection.py](https://github.com/esphome/aioesphomeapi/blob/v46.6.0/aioesphomeapi/connection.py), [esphome api_connection.cpp](https://github.com/esphome/esphome/blob/2026.10.1/esphome/components/api/api_connection.cpp)]. HA then sends `DeviceInfoRequest` and `ListEntitiesRequest` in one packet and waits for `DeviceInfoResponse` and `ListEntitiesDoneResponse`. Next it subscribes to states, HA states and services [VS `ha_mgr` `_on_connect`; aioesphomeapi `device_info_and_list_entities`].
- **Password auth.** Removed in ESPHome 2026.1.0. Message IDs 3/4 are reserved, and `DeviceInfoResponse.uses_password` is deprecated [VS `api.proto`]. aioesphomeapi still sends `AuthenticationRequest` only when HA has a stored password, and it never waits for the reply [VS `connection.py`]. **A new device must not implement passwords.**
- **Keepalive.** Either side may send `PingRequest`, and the device must answer `PingResponse`. ESPHome pings after 60 s of silence and drops the link after 2.5× that without traffic [VS `api_connection.cpp`/`.h`].
- **Automatic key provisioning.** HA can provision a Noise key. If `DeviceInfoResponse.api_encryption_supported` is true and HA has no key, HA generates one. HA sends it with `NoiseEncryptionSetKeyRequest` (id 124). It uses a zero-PSK Noise session if `api_encryption_provisionable` is set; otherwise the plaintext path, which is deprecated [VS `ha_mgr` `_handle_dynamic_encryption_key`]. **Do not set these flags unless key storage is implemented.**
- **Capabilities message.** `DeviceCapabilitiesRequest/Response` (ids 149/150, API ≥ 1.15) duplicates the voice feature flags. HA 2026.10.4 still reads `DeviceInfoResponse.voice_assistant_feature_flags` (field 17) [VS: `ha_mgr` and `ha_sat` use `voice_assistant_feature_flags_compat`; no `capabilit` reference in HA esphome at dev `99cc036`]. **[INF]** Reporting API 1.10–1.14 in `HelloResponse` keeps aioesphomeapi on the compat path that synthesizes capabilities from DeviceInfo. linux-voice-assistant reports 1.10.
- **Minimum for HA to create the satellite** [VS `ha_mgr`, `ha_sat`, [HA esphome select.py @dev](https://github.com/home-assistant/core/blob/99cc0366358a990b27b9582f937d92041ff597c4/homeassistant/components/esphome/select.py)]:
  1. Hello, Disconnect and Ping handling.
  2. `DeviceInfoResponse` with a stable `mac_address` (HA's unique_id), `name`, and `voice_assistant_feature_flags` ≠ 0. `friendly_name`, `manufacturer`, `model` and `project_name/version` are recommended.
  3. `ListEntitiesRequest` → `ListEntitiesDoneResponse`. Zero entities is allowed.
  4. Tolerate `SubscribeStatesRequest`, `SubscribeHomeAssistantStatesRequest`, `SubscribeHomeassistantServicesRequest` and other unknown messages. ESPHome devices ignore unknown messages [VS `api.proto` wizard comment].
  5. `SubscribeVoiceAssistantRequest` (id 89). HA sets `flags=API_AUDIO` when the device advertises API_AUDIO [VS `aio_client`].
  6. If `ANNOUNCE` is advertised: answer `VoiceAssistantConfigurationRequest` within **5 s**. Entity setup blocks on it, then falls back to an empty config [VS `ha_sat` `_CONFIG_TIMEOUT_SEC`].

  HA itself then creates the `assist_satellite` entity and HA-side selects: pipeline ×N, VAD sensitivity, wake word ×N. **The device does not list these** [VS select.py].

### A2. Voice messages

| ID | Message | Direction | Key fields |
| --- | --- | --- | --- |
| 89 | SubscribeVoiceAssistantRequest | HA→dev | `subscribe`, `flags` (1 = API_AUDIO) |
| 90 | VoiceAssistantRequest | dev→HA | `start`, `conversation_id`, `flags` (1 = USE_VAD, 2 = USE_WAKE_WORD), `audio_settings`, `wake_word_phrase` |
| 91 | VoiceAssistantResponse | HA→dev | `port` (UDP only, otherwise 0), `error` |
| 92 | VoiceAssistantEventResponse | HA→dev | `event_type`, repeated `{name,value}` |
| 106 | VoiceAssistantAudio | both | `data`, `end`, `data2` (second channel) |
| 115 | VoiceAssistantTimerEventResponse | HA→dev | started/updated/cancelled/finished, ids, seconds |
| 119 | VoiceAssistantAnnounceRequest | HA→dev | `media_id` (URL), `text`, `preannounce_media_id`, `start_conversation` |
| 120 | VoiceAssistantAnnounceFinished | dev→HA | `success` |
| 121/122 | VoiceAssistantConfigurationRequest/Response | HA→dev / dev→HA | external wake words / available and active wake words, `max_active_wake_words` |
| 123 | VoiceAssistantSetConfiguration | HA→dev | `active_wake_words` |

[VS `api.proto`]

- **Event types** [VS `api.proto`]: ERROR 0, RUN_START 1, RUN_END 2, STT_START 3, STT_END 4, INTENT_START 5, INTENT_END 6, TTS_START 7, TTS_END 8, WAKE_WORD_START 9, WAKE_WORD_END 10, STT_VAD_START 11, STT_VAD_END 12, TTS_STREAM_START 98, TTS_STREAM_END 99, INTENT_PROGRESS 100.
- **Feature flags (`VoiceAssistantFeature`)** [VS [aioesphomeapi model.py](https://github.com/esphome/aioesphomeapi/blob/v46.6.0/aioesphomeapi/model.py)]: VOICE_ASSISTANT 1, SPEAKER 2, API_AUDIO 4, TIMERS 8, ANNOUNCE 16, START_CONVERSATION 32, MULTI_CHANNEL_AUDIO 64. ESPHome always sets VOICE_ASSISTANT|API_AUDIO. It adds SPEAKER with a `speaker`, ANNOUNCE|START_CONVERSATION with a `media_player`, TIMERS when timers are used, and MULTI_CHANNEL with a second mic [VS [voice_assistant.h](https://github.com/esphome/esphome/blob/2026.10.1/esphome/components/voice_assistant/voice_assistant.h) `get_feature_flags`].
- **What each flag does in HA** [VS `ha_sat` `async_added_to_hass`]:
  - API_AUDIO: audio travels over the API.
  - TIMERS: HA registers a timer handler.
  - ANNOUNCE: adds the announce feature and fetches the config.
  - START_CONVERSATION: adds that feature.
  - MULTI_CHANNEL_AUDIO: HA may switch to `data2`, the non-enhanced channel, when the STT engine prefers no AGC/NS.
- **Audio transport.** Mic audio goes over the API (`VoiceAssistantAudio.data`) when API_AUDIO is set. Without API_AUDIO, HA opens a UDP server and returns its port [VS `ha_sat` `handle_pipeline_start`]. ESPHome comments mark UDP as "will eventually be deprecated" [VS `va.cpp`]. **Use API_AUDIO.**
- **Audio format.** 16 kHz, 16-bit, mono PCM: HA builds `SpeechMetadata(16 kHz, 16-bit, mono)` [VS [assist_satellite/entity.py](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/assist_satellite/entity.py); [assist_pipeline const.py @dev](https://github.com/home-assistant/core/blob/99cc0366358a990b27b9582f937d92041ff597c4/homeassistant/components/assist_pipeline/const.py)]. ESPHome sends 32 ms chunks (512 samples = 1024 B) [VS `va.cpp`]. HA re-chunks internally, so the chunk size is not a contract [INF].
- **Ending the stream.** `VoiceAssistantAudio{end=true}` means a graceful stop. `VoiceAssistantRequest{start=false}` means **abort** [VS `aio_client` `subscribe_voice_assistant`].

### A3. Server-side (HA) wake word

- **How to request it.** Send `VoiceAssistantRequest{start=true, flags=USE_WAKE_WORD, wake_word_phrase=""}` and stream audio. HA then starts the pipeline at `PipelineStage.WAKE_WORD`; without the flag it starts at STT [VS `ha_sat` `handle_pipeline_start`]. In ESPHome YAML this is `voice_assistant: use_wake_word: true` plus `voice_assistant.start_continuous` ("will start listening again after the response audio has finished playing") [VS `va.cpp` `START_PIPELINE`; [ESPHome docs](https://esphome.io/components/voice_assistant/)].
- **Which engine runs.** The pipeline's configured streaming wake word engine and wake word id (`pipeline.wake_word_entity`, `pipeline.wake_word_id`), for example openWakeWord over Wyoming. The device has no say in the model [VS [default_pipeline.py @dev](https://github.com/home-assistant/core/blob/99cc0366358a990b27b9582f937d92041ff597c4/homeassistant/components/assist_pipeline/default_pipeline.py)]. The pipeline is chosen through the HA-created pipeline select [VS `ha_sat`].
- **Continuous mode is current and supported in source.** One pipeline run equals one detection. After RUN_END, ESPHome re-requests immediately while it is in continuous mode. During a follow-up turn (`continue_conversation`) it omits USE_WAKE_WORD so the run starts at STT [VS `va.cpp`]. Official HA docs describe continuous streaming to HA for wake word detection [VS [HA wake word approach](https://home-assistant.io/voice_control/about_wake_word)]. No deprecation of the USE_WAKE_WORD flag was found in api.proto or HA code [VS, absence of evidence].
- **Caveats:**
  - **No timeout.** The ESPHome satellite passes no `WakeWordSettings`, so `timeout=None` and HA streams indefinitely. There is no VAD timeout in the wake word stage [VS `entity.py` and `default_pipeline.py`].
  - **One stream and one run per satellite.** A new start cancels the running pipeline and any TTS streaming task [VS `entity.py` `_cancel_running_pipeline`, `ha_sat`].
  - **Errors.**
    - `wake-engine-missing` and `wake-provider-missing`: no engine is configured, or Wyoming is down. ESPHome stops and does not retry [VS `va.cpp`]. **Our daemon should retry with backoff** [INF].
    - `wake-word-timeout`, `wake_word_detection_aborted`, and `no_wake_word` (WAKE_WORD_END with empty output, converted to ERROR by HA): not fatal. Wait for RUN_END [VS `va.cpp`, `ha_sat`].
    - `wake-stream-failed`: the engine raised an exception [VS default_pipeline.py].
  - **Duplicate wake-up.** HA rejects the same wake word phrase seen within 2 s across satellites (`WAKE_WORD_COOLDOWN`) [VS run.py/const.py @dev].
  - **Speech before detection.** Audio queued at detection (`queued_audio`) is forwarded to STT, so the user need not pause after the wake word [VS default_pipeline.py].
  - **Wizard limitation.** HA's wake word "intercept" path refuses stage WAKE_WORD with "Only on-device wake words currently supported". The HA wake word UI/test flow does not work with HA-side wake word. The device's wake word selects stay empty unless the device reports `available_wake_words` [VS `entity.py`, select.py].
  - **Ignored fields.** HA 2026.10.4 ignores `VoiceAssistantRequest.audio_settings` (noise suppression, auto gain, volume) and the `USE_VAD` flag. Only USE_WAKE_WORD is read. End-of-speech VAD is done by HA with the VAD sensitivity select (`AudioSettings(silence_seconds=…)`) [VS `ha_sat`, `entity.py`].
  - **Cost.** Each satellite is a permanent 256 kbit/s PCM stream plus framing, and one openWakeWord instance in HA [INF, from the format].
  - **Privacy.** Ambient audio leaves the TV all the time while armed. HA debug recording is opt-in (`debug_recording_dir`) [VS run.py @dev].
  - **No barge-in in the reference.** ESPHome stops the mic at STT_VAD_END/STT_END and does not stream while the response plays. HA-side wake word therefore cannot fire during TTS [VS `va.cpp`].

### A4. TTS delivery

- **URL path (no SPEAKER flag).** HA sends a URL in `TTS_END` (`data.url`). When HA supports streaming TTS, it also sends a URL in `RUN_START` (`data.url`) and later `INTENT_PROGRESS{tts_start_streaming="1"}`. The device may then start fetching the RUN_START URL early, which ESPHome does with its media player [VS `ha_sat` `on_pipeline_event`, `va.cpp`]. The URL is made absolute with HA's configured URL (`async_process_play_media_url`) and points to `/api/tts_proxy/{token}`, which has `requires_auth = False` [VS [HA tts/\_\_init\_\_.py](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/tts/__init__.py)]. **The device fetches it over HTTP or HTTPS.**
- **Streaming path (SPEAKER flag).** HA forces TTS to WAV, 16 kHz, mono, 16-bit. It streams raw PCM in `VoiceAssistantAudio` messages (512 samples per chunk) between `TTS_STREAM_START` and `TTS_STREAM_END`, over UDP if API_AUDIO is not set. It paces the stream to keep about 384 ms ahead, assuming a 512 ms device ring buffer. HA calls `tts_response_finished` itself [VS `ha_sat` `_stream_tts_audio`]. Announcements still arrive as a URL (A4 last bullet), so SPEAKER does not remove the HTTP client.
- **Format negotiation.** Without SPEAKER, HA takes the TTS format from the first `MediaPlayerSupportedFormat` with `purpose=ANNOUNCEMENT` on any device media player entity. The fields are `format`, `sample_rate`, `num_channels`, `sample_bytes` (plus `bitrate`) [VS `ha_sat` `_update_tts_format`; `api.proto` `ListEntitiesMediaPlayerResponse`].
  - Without such an entity, HA's TTS default is **mp3** [VS tts `_DEFAULT_FORMAT`]. HA converts with ffmpeg to the preferred format.
  - **Bug in 2026.10.4.** `num_channels` and `sample_bytes` are applied only if `sample_rate > 0` (fixed on dev) [VS diff 2026.10.4 vs dev]. **Always set `sample_rate`.**
  - "wav", "flac" and "mp3" are format strings already used in HA code [VS]. **[INF]** HA's ffmpeg proxy should accept any ffmpeg muxer name. Restrict ourselves to wav/flac/mp3.
- **Announcements.** These need the ANNOUNCE flag, **not** a media_player entity. HA sends `VoiceAssistantAnnounceRequest` and waits up to **5 min** for `VoiceAssistantAnnounceFinished` [VS `ha_sat` `_ANNOUNCEMENT_TIMEOUT_SEC`].
  - If an ANNOUNCEMENT format exists, non-TTS media and the preannounce chime are routed through `/api/esphome/ffmpeg_proxy/{device_id}/{file}` (no auth, at most 2 concurrent conversions per device) in that format. Otherwise the raw `media_id` URL is passed through in any format [VS `ha_sat` `_do_announce`, [ffmpeg_proxy.py](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/ffmpeg_proxy.py)].
  - **[INF]** Expose a minimal media_player entity with one ANNOUNCEMENT format. HA then delivers *all* voice audio in one format the daemon chose. A media_player entity also invites music `MediaPlayerCommandRequest`s, so Phase 1 must at least ignore them safely.
- **Finishing a response.** In URL mode the device must send `VoiceAssistantAnnounceFinished` when playback ends, **also for normal TTS responses**. HA uses it to call `tts_response_finished` [VS `ha_sat` `handle_announcement_finished`; ESPHome sends it after media-player FINISHED or a 2 s playback-start timeout, `va.cpp`].

### A5. Device-side state machine expectations

Typical full run, reconstructed from `ha_sat` and `va.cpp` [VS for individual events, INF for the ordering summary]:

1. **Request.** Device sends `VoiceAssistantRequest(start, flags)`. HA replies `VoiceAssistantResponse{port:0}` once the pipeline task is created.
2. **Wake word stage** (HA-side wake word only): `RUN_START` (may carry the streaming TTS `url`), then `WAKE_WORD_START`, then `WAKE_WORD_END`. The device keeps streaming.
3. **Speech to text.** `STT_START`, `STT_VAD_START`, `STT_VAD_END` (stop the mic here), `STT_END{text}`.
4. **Intent.** `INTENT_START`, `INTENT_PROGRESS{tts_start_streaming}` (optional), `INTENT_END{conversation_id, continue_conversation, speech}`.
5. **TTS.** `TTS_START{text}`, `TTS_END{url}`, `RUN_END`.
6. **Playback.** Device plays, then sends `AnnounceFinished`.
7. **Next turn.** If `continue_conversation=1`: new request **without** USE_WAKE_WORD, same `conversation_id`. Otherwise re-arm the wake word.

Other rules:

- **Errors.** `ERROR{code,message}` is followed by RUN_END. ESPHome stops the mic and signals stop, except for the non-fatal wake codes in A3. RUN_END while still streaming means stop the mic. RUN_END without TTS ("nevermind") means return to idle [VS `va.cpp`].
- **Timeouts.**
  - ESPHome resets `conversation_id` after `conversation_timeout` (default 300 s).
  - It treats TTS as finished if playback has not started within `tts_playback_start_timeout` (default 2 s).
  - It stops when one channel stalls for 2 s in multi-channel mode.
  - HA has **no** stream timeout [VS `va.cpp`, `voice_assistant.h`, ESPHome docs].
- **Barge-in and stop.** Sending a new `start` during TTS is accepted: HA cancels the TTS stream and the running pipeline [VS `ha_sat`, `entity.py`]. The device must also stop local playback. Detecting the user's barge-in needs on-device detection, for example a stop word as in LVA, or AEC plus streaming during playback. Neither is in the MVP [INF].
- **Timers.** Keep the timer list on the device. HA sends the full timer state on each event; the device ticks locally [VS `va.cpp` `on_timer_event`].

## B. Existing implementations

### B1. OHF-Voice/linux-voice-assistant (LVA)

- **What it is** [VS [repo @5b980d0](https://github.com/OHF-Voice/linux-voice-assistant/tree/5b980d0609e59a7baa0d3c8d05dde4c8698f3420), `pyproject.toml`, `api_server.py`, `satellite.py`]:
  - Python, Apache-2.0, "Development Status :: 3 - Alpha".
  - `requires-python >=3.11`. Depends on `aioesphomeapi==45.3.1`, `numpy>=2`, `soundcard`, `python-mpv`, `pymicro-wakeword`, `pyopen-wakeword`, `webrtc-noise-gain` and `zeroconf`.
  - Supported platforms are linux/amd64 and aarch64, with PulseAudio/PipeWire.
- **How it implements the device side.** An asyncio `Protocol` server. It reuses aioesphomeapi's generated protobufs and plaintext packet helper. It speaks **plaintext only** (no Noise) and reports API 1.10. Flags: VOICE_ASSISTANT|API_AUDIO|ANNOUNCE|START_CONVERSATION|TIMERS (+MULTI_CHANNEL). It exposes media_player, number, select and switch entities. TTS and announcements play by URL through mpv. It sends no `supported_formats`, so HA defaults to mp3 [VS].
- **Wake word.** **On-device only** (openWakeWord/microWakeWord, plus a stop word for barge-in). There is no `USE_WAKE_WORD` anywhere in the package [VS grep]. It does not cover our MVP mode as is.
- **On the TV's Python 3.8: not runnable** [VS for the version requirements, INF for the conclusion]:
  - It requires ≥ 3.11, and aioesphomeapi itself has `python_requires >=3.11` [VS [setup.py](https://github.com/esphome/aioesphomeapi/blob/main/setup.py)].
  - numpy 2 and the native wake word, mpv and soundcard dependencies have no 32-bit softfp glibc builds.
- **Best use.** As a behavioural reference, and as a reference satellite on a dev PC against the user's HA. Use it to capture HA-side debug logs and message sequences. Do not port it.

### B2. Rust building blocks

- **`esphome-native-api`** ([crate](https://crates.io/crates/esphome-native-api), [repo @125b541](https://github.com/UbiHome/esphome-native-api/tree/125b541eb0362f7f597e03729d641f05867eff60)).
  - What it is: MIT. Client and server roles, plaintext and Noise (`noise-protocol` crate). Used by UbiHome. tokio, prost 0.13 [VS].
  - Protocol pinning: the proto is pinned to one ESPHome version through a feature flag (default `version_2026_6_2`) [VS]. Message IDs come from a hand-written table [VS `src/parser.rs`].
  - Robustness concerns: an unknown message type returns `FrameError::UnknownMessageType` and `message_to_num` returns `u8` [VS]. **[INF]** Check that unknown HA messages do not tear down the connection before adopting it.
  - Verdict: worth evaluating, or mining for framing and Noise code. The protocol surface we need is small (about 25 messages).
- **Other crates.** `esphome-client` 0.2.1 is client-only and could serve as a Rust-side test client [VS crates.io description]. `esphome` 0.1.0 (2021) is stale [VS crates.io].
- **Noise.** `snow` 0.10.0's default resolver supports 25519, ChaChaPoly, SHA256 and `psk` modifiers on the NN pattern, so it can build `Noise_NNpsk0_25519_ChaChaPoly_SHA256` [VS [snow README / patterns.rs](https://github.com/mcginty/snow)].
- **Protobuf and `api.proto`.**
  - `api.proto` imports `api_options.proto`, which `extend`s `google.protobuf.{Method,Message,Field}Options` and imports `descriptor.proto` [VS].
  - `prost-build` compiles it with that file on the include path. esphome-native-api does exactly this [VS generator `compile_protos(api.proto)`].
  - prost **ignores custom options**, so the `(id)` message-type numbers must be extracted separately: a small build.rs parser or a hand table [VS that IDs are options; INF the approach].
  - Use `protox` 0.10 to avoid needing `protoc` on the build host [INF].
  - **Licence.** ESPHome's C/C++ runtime is **GPLv3**; Python and "all other parts" (including `.proto`) are **MIT** [VS [ESPHome LICENSE](https://github.com/esphome/esphome/blob/dev/LICENSE)]. Vendoring `api.proto` is fine. Do not translate `voice_assistant.cpp` line by line into a non-GPL codebase [INF].

### B3. aioesphomeapi as a fake-HA harness

`APIClient` is the exact library HA uses [VS `aio_client`]. A host-side Python ≥ 3.11 script can drive the whole satellite surface:

- connect with or without `noise_psk`;
- `device_info()`, `list_entities_services()`;
- `subscribe_voice_assistant(handle_start, handle_stop, handle_audio, handle_announcement_finished)`;
- `send_voice_assistant_event(type, data)`, `send_voice_assistant_audio`, `send_voice_assistant_announcement_await_response`, `get/set_voice_assistant_configuration`, `send_voice_assistant_timer_event`.

That gives scripted pipelines (fake STT text, a local WAV URL as TTS), fault injection (ERROR codes, abort, disconnect) and recording of mic audio without HA [INF design, VS API]. Pin it to **46.6.0** to match HA 2026.10.4. Also run the daemon against a real HA test instance before claiming compatibility.

## C. Rust for the target

### C1. Target triple, linking and audio

**Facts:**

- **TV userland.** 32-bit ARM, EABI5, `/lib/ld-linux.so.3`, glibc 2.31, no hard-float loader, no musl. The CPU is a Cortex-A76 and the kernel is aarch64 5.4 [target discovery: [D0-D1](../discovery/c3-a/D0-D1-runtime.md)]. The webOS NDK (buildroot-nc4 `webos_tv_defconfig`) uses `BR2_GCC_TARGET_FLOAT_ABI="softfp"`, ARMv7-A, VFPv3 and NEON, and GCC 11 [VS [defconfig](https://github.com/openlgtv/buildroot-nc4/blob/webos/configs/webos_tv_defconfig)]. **[INF]** The C3 libraries are softfp, which is call-compatible with soft-float. Still to check with `readelf -A`.
- **Rust ARM targets** [VS [rustc target specs](https://github.com/rust-lang/rust/tree/master/compiler/rustc_target/src/spec/targets), [platform support](https://doc.rust-lang.org/rustc/platform-support.html)]:
  - `armv7-unknown-linux-gnueabi`: tier 2 without host tools, glibc 2.27, `FloatAbi::Soft`, features `+soft-float,-neon`.
  - `armv7-unknown-linux-musleabi`: same float settings, static by default.
  - `armv7-unknown-linux-musleabihf`: hard-float with VFP.
  - `aarch64-unknown-linux-musl`: tier 2 with host tools, musl 1.2.5.
- **Softfp in rustc.** On ARM with a soft float ABI, rustc explicitly allows `-C target-feature=-soft-float,+vfp3,+neon` and calls it the `-mfloat-abi=softfp` equivalent. These ARM features are still "unstable" for `-Ctarget-feature`, so stable rustc warns [VS [target_features.rs](https://github.com/rust-lang/rust/blob/master/compiler/rustc_target/src/target_features.rs) `abi_required_features`; INF for the warning behaviour].
- **cargo-zigbuild** can pin the glibc version with `--target armv7-unknown-linux-gnueabi.2.31`. It cannot build static glibc; use musl for that [VS [README](https://github.com/rust-cross/cargo-zigbuild)].

**Options:**

| Option | Pros | Cons |
| --- | --- | --- |
| **1. `armv7-unknown-linux-gnueabi` + glibc ≤ 2.31, dynamic** (zigbuild `.2.31` or webosbrew GCC as linker), `RUSTFLAGS=-C target-feature=-soft-float,+vfp3,+neon` | Matches the userland ABI. Can link the TV's `libasound.so.2` (alsa crate) and `libpulse.so.0` if wanted. Smallest binary. | Needs a sysroot with alsa/pulse stubs at build time. Unstable-feature warning. Ties us to the system libs' ABI and versions. |
| **2. `armv7-unknown-linux-musleabi` static** (same RUSTFLAGS) | No loader or libc dependency. Survives firmware updates. Float ABI is irrelevant because nothing is shared. | Cannot dlopen the TV's glibc libasound or libpulse. Audio must be pure Rust, bundled C, or a subprocess. |
| `armv7-unknown-linux-musleabihf` static | VFP on by default | Works only because it is static; still no system libs. No real gain over option 2 with RUSTFLAGS. |
| `arm-unknown-linux-gnueabi` (ARMv6) | none | Slower baseline. Glibc story same as option 1. |
| **3. `aarch64-unknown-linux-musl` static** | Native 64-bit on the 64-bit kernel: NEON, ring/aws-lc and DSP all work. No 32-bit ABI questions. | Untested whether the C3 kernel runs 64-bit EL0 ELF ([D0-D1](../discovery/c3-a/D0-D1-runtime.md) lists it as untested). Can never load TV libs. |

**Recommendation [INF]:**

- **Default:** option 2, a static `armv7-unknown-linux-musleabi` binary with softfp RUSTFLAGS.
- **Experiment:** run a static aarch64 hello world on the C3. If it works, option 3 becomes an equal choice.
- **Fallback:** option 1 only if a library we must call cannot be replaced.

**Audio approach [INF]:**

- **Capture.** Talk to the kernel, not to alsa-lib.
  - Bundle **tinyalsa** (BSD-3-Clause, plain ioctls, no alsa.conf parsing [VS [NOTICE](https://github.com/tinyalsa/tinyalsa)]) with the `cc` crate, or write the roughly 6 needed ioctls in Rust.
  - Pure-Rust ALSA PCM crates are immature (`alsa_ioctl` 0.1.0, 2022) [VS crates.io].
  - A static alsa-lib would still read the TV's `/etc/asound.conf`, which maps `default` to the pulse plugin [target discovery: [D2](../discovery/c3-a/D2-audio-stack.md)], and could not load glibc plugins.
  - Resolve the card **by name** (`LGSIC`) from `/proc/asound/cards`, never by index. Open S24_LE, 2 ch, 16 kHz, period 256, buffer 8192. Use read-mode (`READI_FRAMES`) ioctls; mmap is not needed.
- **Playback.**
  - **Phase 1:** decode in-process and pipe s16le PCM to `pacat`/`paplay --raw` (present on the TV [D2]) on a chosen sink (`ptts`, `pdefaultapp`; to be tested). This is simplest and keeps PulseAudio policy in charge.
  - **Phase 2:** the pure-Rust `pulseaudio` crate (0.3.2, native protocol, MIT; zero-copy memfd/shm "not yet implemented") [VS [repo](https://github.com/colinmarc/pulseaudio-rs)] over `/var/run/pulse/native` (anonymous auth [D2]). Compatibility with PulseAudio 9's protocol version is unknown.
  - Avoid `libpulse-binding`: it needs the system libpulse, so option 1 only.
- **Build and test.**
  - Build: `cargo zigbuild` (or `cross`) in CI, then `file`/`readelf -h -A` checks on the artefact. For gnueabi builds, verify the maximum `GLIBC_` symbol version is ≤ 2.31.
  - Unit and protocol tests run on the host (x86_64) against aioesphomeapi.
  - qemu-user (`qemu-arm`) smoke test of the ARM binary.
  - Only then a manual copy to the TV under an experiment ID with rollback (delete the binary; no persistence).

### C2. Crates (versions from crates.io, 2026-11-01)

| Need | Crate | Notes |
| --- | --- | --- |
| Async runtime | `tokio` 1.54 | `rt` current-thread is enough for one connection and keeps RSS low [INF] |
| mDNS `_esphomelib._tcp` | `mdns-sd` 0.21.6 | Sets SO_REUSEADDR and SO_REUSEPORT on 5353 [VS [service_daemon.rs](https://github.com/keepsimple1/mdns-sd/blob/main/src/service_daemon.rs)]. HA needs TXT `mac`; it uses `friendly_name`, and `api_encryption` marks a Noise device [VS [HA config_flow.py @dev](https://github.com/home-assistant/core/blob/99cc0366358a990b27b9582f937d92041ff597c4/homeassistant/components/esphome/config_flow.py) `async_step_zeroconf`]. ESPHome also sends `version`, `platform`, `board`, `network` and `config_hash` [VS esphome `mdns_component.cpp` @dev]. |
| HTTP for TTS | `ureq` 3.5 (blocking, small) or `reqwest` 0.13 + rustls 0.23 | HA URLs are often plain `http://` on the LAN. For HTTPS, validate TLS with a configurable CA. ring/aws-lc on armv7 softfp is unverified; check at build time [INF]. |
| Decode | `symphonia` 0.6.1 | mp3/flac/wav. With an ANNOUNCEMENT `wav` format, only WAV parsing is needed. |
| Resample | `rubato` 5.0.1 | 22.05/24 kHz TTS to sink rate, if Pulse is not left to resample |
| Protobuf | `prost` 0.14.4 + `prost-build`/`protox` 0.10 | Custom `(id)` options must be mapped separately (B2) |
| Noise | `snow` 0.10.0 | NNpsk0, initiator is HA, responder is the device |
| Optional DSP | `webrtc-audio-processing` 2.1.0 | C++ build; post-MVP, AEC/NS investigation only |

## D. Risks and unknowns on this TV

| Risk | Status | Mitigation / experiment |
| --- | --- | --- |
| **mDNS port 5353 already owned by an LG responder** (the AirPlay app runs [D0-D1](../discovery/c3-a/D0-D1-runtime.md)) | hypothesis | Read-only check of UDP sockets on 5353 and their `SO_REUSEPORT` (`/proc/net/udp`, `ss -ulpn` if present). Fallback: add the device by IP in HA. The ESPHome config flow accepts host and port; mDNS only speeds up discovery and reconnect [VS HA config_flow/reconnect_logic]. |
| **Inbound TCP 6053 blocked** by iptables or LG policy | unknown | Read-only `iptables -S` if present; then a temporary `nc -l` listener test. Fallback: ESPHome outgoing-connection mode (open question). |
| **Mic exclusivity.** LG `voiceinput` opens `hw:LGSIC,0` when hands-free is enabled [D3](../discovery/c3-a/D3-mic-identification.md) | verified for that state | Detect EBUSY and report it. Do not touch LG services. Document the "hands-free off" requirement. |
| **Memory** (about 750 MB available, CMA tight [D0-D1]) | inferred OK | Target < 20 MB RSS [INF]. No large buffers; mic ring buffer about 1 s. |
| **Continuous upstream audio** (privacy and HA load) | by design | Mute switch entity that stops capture. No audio logging. An on-screen listening indicator is TBD. Wake word moves on-device later. |
| **Power states.** Daemon frozen or network down in standby or screen-off | unknown | HA reconnect logic handles drops. Test per power state (compatibility record). |
| **Stable identity.** `mac_address` is HA's unique_id | design | Use a locally generated, stored, locally administered MAC-like id. Do not commit it. Changing it creates a new HA device. |
| **TTS URL reachability.** HA builds absolute URLs from its configured URL (`get_url`) | inferred | The TV must reach HA's internal URL. With HTTPS and a private CA, ship the CA in the config. Never disable validation silently. |
| **No AEC.** TV speakers next to the mics | known | Never stream during our own playback in MVP. Barge-in is post-MVP. |
| **Protocol drift.** ESPHome and HA ship monthly; API 1.15+ moves flags to DeviceCapabilities | inferred | Report API 1.10–1.14 until DeviceCapabilities is implemented. Ignore unknown messages. Run CI against the aioesphomeapi version pinned by the user's HA. |
| **GPL contamination** from porting ESPHome C++ | inferred | Re-implement from proto plus observed behaviour. |

## Design implications for Phase 1

1. **Transport.**
   - Implement the API server with Noise (`snow`) from the start, using a user-supplied PSK in a local config file that is never committed. Allow plaintext only behind an explicit debug flag.
   - Do **not** advertise `api_encryption_supported` or `provisionable` until key storage exists.
   - Report API version 1.10–1.14 and answer Hello, DeviceInfo, ListEntities(Done), Ping and Disconnect. Ignore unknown messages.
2. **Feature flags:** `VOICE_ASSISTANT | API_AUDIO | ANNOUNCE | START_CONVERSATION`. Add `TIMERS` once a visible or audible timer exists. **No SPEAKER**: URL mode gives one code path for responses and announcements.
3. **Entities:**
   - One `media_player` with one `ANNOUNCEMENT` format (`wav` or `flac`, `sample_rate` always set, mono, 2 bytes). HA then delivers TTS and proxied announcements in our format.
   - One `switch` "Mute microphone" (privacy).
   - Answer `VoiceAssistantConfigurationRequest` immediately with no wake words and `max_active_wake_words=0`.
4. **Wake word in HA:**
   - After subscribe, send `VoiceAssistantRequest{start, flags=USE_WAKE_WORD}` and stream continuously. Re-arm on RUN_END.
   - Stop streaming at STT_VAD_END/STT_END and during playback. Restart after `AnnounceFinished`.
   - On `continue_conversation=1`, re-request without USE_WAKE_WORD and with the same `conversation_id`.
   - Back off and retry on `wake-engine-missing`/`wake-provider-missing`.
   - HA-side setup: the pipeline must have a streaming wake word engine (openWakeWord).
5. **Front-end:** S24_LE 2 ch at 16 kHz → sign-extend → mono s16 (channel select or average; to be decided by experiment) → 1024-byte chunks. Optional `MULTI_CHANNEL_AUDIO` with the raw second mic in `data2` (HA ≥ 2026.6).
6. **Playback:** HTTP GET the URL, then WAV/FLAC decode, then `pacat --raw` to a tested sink, then `AnnounceFinished{success}`. Cancel on a new pipeline start or a `MediaPlayerCommand STOP`.
7. **Build:** static `armv7-unknown-linux-musleabi` + softfp RUSTFLAGS via cargo-zigbuild. tinyalsa or raw ioctls for capture. Run an aarch64-static feasibility experiment in parallel.
8. **Tests:**
   - Host-side protocol tests with aioesphomeapi 46.6.0 as fake HA, including fault injection.
   - LVA on a dev PC as a reference satellite to compare HA logs and event order.
   - Then a real HA test instance.

## Open questions

1. Does the C3 kernel execute a static aarch64 ELF (option 3)? Needs a minimal experiment.
2. What exact FPU/NEON attributes do the TV's system libraries carry (`readelf -A` on a copied binary)? Relevant only for option 1.
3. Who binds UDP 5353 on the TV, and with which reuse flags? Is any inbound firewall active for 6053?
4. How does ESPHome/HA outgoing-connection mode work (HA listener port, how it is configured, which HA version introduced it)? Is it a fallback if inbound 6053 is blocked?
5. Which PulseAudio sink (`ptts`, `pdefaultapp`, other) plays TTS audibly over HDMI and Netflix playback and in screen-off, without breaking LG policy? (Ties to [03-playback-aec](03-playback-aec.md).)
6. Does `pulseaudio` 0.3.x interoperate with PulseAudio 9's native protocol without memfd/shm?
7. Is mono downmix (average) or a single mic better for openWakeWord and STT on `LGSIC,0`? Does `data2` help with HA's multi-channel selection?
8. Does HA's ffmpeg proxy accept all format strings we may choose, and does streaming TTS (RUN_START URL) work with `wav` from the user's TTS engine?
9. Does esphome-native-api tolerate unknown messages and newer protos? Adopt it, or write a thin layer over `prost` + `snow`?
10. ring or aws-lc-rs on armv7 softfp musl: does it build? If not, require an `http://` internal URL for HA, or use option 3.
11. Behaviour and listening indicator in standby, "Always Ready" and screen-off states. Needs compatibility records.
