# ESPHome native API server (`webos-esphome-api`)

Roadmap step 1.3. The crate [`crates/esphome-api`](../../crates/esphome-api) implements the device side of the ESPHome native API. Home Assistant's ESPHome integration connects to it and creates an Assist satellite ([ADR 0001](../decisions/0001-esphome-native-api.md)). Wake word, STT, conversation and TTS run in HA ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)). The state machine and the public API are documented in the crate docs (`src/lib.rs`, `cargo doc -p webos-esphome-api`).

**Status (2026-11-02):** tested against aioesphomeapi 46.6.0 acting as HA (on the host, under qemu-arm, and on c3-a inside `webos-satellite`). It also carried full pipeline runs with the owner's real HA pipeline through the HA bridge. HA's own ESPHome integration has **not** connected to it yet, because HA cannot reach the TV's network ([P1 HA integration](../discovery/c3-a/P1-ha-integration.md), [P1 TV live](../discovery/c3-a/P1-tv-live.md)).

## Pinned protocol sources

| Item | Version | Use |
| --- | --- | --- |
| `api.proto`, `api_options.proto` | ESPHome 2026.10.1, vendored in `crates/esphome-api/proto/` (MIT, see `LICENSE-ESPHOME-MIT`) | Message definitions. No ESPHome C++ (GPLv3) was ported. |
| aioesphomeapi | 46.6.0 (the version HA 2026.10.4 pins) | Behaviour reference and the fake-HA test harness |
| HA | 2026.10.4 `components/esphome` | Behaviour reference (source reading only) |

Code generation: `build.rs` compiles the proto with `protox` (pure Rust, so no `protoc` is needed on the build host) and `prost-build`. It also extracts the `option (id)` message type ids from the proto text into `ApiMessage` impls. `prost` drops custom options, so this step is needed. Noise uses `snow` with only the ChaChaPoly, SHA256, 25519 and getrandom features. Its `std` feature would pull in `ring`, a C library, so it is left off.

## Decisions taken in this crate

- **API version 1.14** in `HelloResponse`. HA accepts device major version 1 or 2. aioesphomeapi 46.6.0 reads `voice_assistant_feature_flags` from `DeviceInfoResponse` for 1.10 to 1.14. From 1.15 on it expects `DeviceCapabilitiesResponse`, which the crate answers anyway. From 1.14, a missing `object_id` is tolerated, but the crate sets it regardless.
- **Voice feature flags** are configurable (`Config::voice_features`). The default is `VOICE_ASSISTANT | SPEAKER | API_AUDIO | ANNOUNCE` = 23:
  - With SPEAKER, HA streams the TTS reply as 16 kHz mono s16le PCM over the API, between the `TTS_STREAM_START` and `TTS_STREAM_END` events. The PCM chunks are 512 samples and carry no WAV header. The owner's HA builds media URLs from the pod IP, which the TV cannot reach (orchestrator note, [integration notes](../research/ha-esphome-integration-notes.md)).
  - Without SPEAKER, HA sends `TTS_END{url}` and the app fetches it (`AppEvent::TtsUrl`). After playback the device sends `VoiceAssistantAnnounceFinished`.
  - `START_CONVERSATION` (32) is implemented but off by default.
  - `TIMERS` and `MULTI_CHANNEL_AUDIO` are rejected by config validation because they are not implemented.
- **Request flags:** `USE_WAKE_WORD` (2) is set for wake word runs. `USE_VAD` is never set, because HA 2026.10.4 ignores it.
- **Entities:**
  - One `media_player`. Its single ANNOUNCEMENT format is `flac`, 22050 Hz, 1 channel, 2 bytes. That is Piper's native rate, it streams well and symphonia can decode it. `sample_rate` is always set, to work around the HA 2026.10.4 bug. The only media feature is STOP. In URL mode this format becomes HA's TTS format. In all modes it is the format of announcement media proxied through ffmpeg.
  - One "Mute microphone" switch, entity category config.
- **Wake words:** `VoiceAssistantConfigurationResponse` reports no available or active wake words and `max_active_wake_words = 0`. The wake word is whatever the selected HA pipeline configures.
- **Encryption:**
  - With `noise_psk` set, the device accepts only Noise. Plaintext clients get the Noise-framed rejection, which HA reports as "requires encryption" and answers by asking for the key.
  - Without `noise_psk`, the device serves plaintext only (development).
  - `api_encryption_supported` is true only when a key is configured, so HA never tries to provision a key. Key storage is not implemented.
- **One HA client at a time.** A new connection that completes its handshake replaces the old one.
- **Retry policy:**
  - A run that ends with `ERROR`, or without progress (no `WAKE_WORD_END` or `STT_END`), delays the next wake word start by 250 ms × 2ⁿ.
  - Configuration errors (`*-missing`, `*not-found*`) use 2 s × 2ⁿ instead.
  - The delay is capped at 60 s.
  - HA's start reply must arrive within 10 s.
- **mDNS:** optional (`Config::mdns`, cargo feature `mdns`, on by default). It advertises `_esphomelib._tcp` with TXT records `friendly_name`, `version`, `mac`, `platform`, `board`, `network` and `api_encryption`, using `mdns-sd`. If the advertisement fails, the server logs a warning and keeps running.

## Existing crate vs own implementation

`esphome-native-api` 3.0.0 (UbiHome, 2026-07) was evaluated:

- **For it:** MIT licence, maintained, client and server roles, Noise via `noise-protocol`. Unknown message types are now skipped.
- **Against it:**
  - It ships generated protos only up to ESPHome 2026.6.2, not 2026.10.x.
  - It uses prost 0.13 and enables tokio `full`, which adds a multi-thread runtime to a static TV binary.
  - It provides message types but no voice-assistant flow: no pipeline state machine, retry, announce or TTS streaming handling.
  - Its message type table is hand-written.

**Choice:** a thin implementation of our own, about 2.5 k lines including doc comments, plus tests, on the pinned proto, `prost` 0.14, `snow` and tokio `current_thread`. The protocol surface is small. The voice state machine is the part that matters, and it has to be ours either way.

## Running the tests

Unit tests and Rust end-to-end tests over loopback (plaintext and Noise):

```sh
CARGO_TARGET_DIR=target/w-esphome cargo test -p webos-esphome-api
```

The fake-HA harness needs Python 3.11 or newer. One command builds the `fake_device` example, creates `tools/esphome-fake-ha/.venv` (gitignored) with `aioesphomeapi==46.6.0`, and runs both scenarios against 127.0.0.1:

```sh
tools/esphome-fake-ha/run.sh
```

The harness plays HA:

- **Scenario A** (plaintext, SPEAKER, wake word mode) covers:
  - hello, device info, entities and formats, states, and the voice configuration;
  - subscribing with API audio and the device's `USE_WAKE_WORD` start;
  - mic audio checks;
  - a full pipeline with the graceful end at `STT_VAD_END`;
  - the TTS stream forwarded to the app, with no URL fetch and no AnnounceFinished;
  - re-arming with the conversation id;
  - `wake-engine-missing` backoff, `continue_conversation`, a run without TTS;
  - an announcement that aborts the wake word run, and `start_conversation`;
  - the mute switch, and disconnect.
- **Scenario B** (Noise, URL mode, push-to-talk) covers:
  - refusal of a plaintext client and of a wrong key, with the exceptions HA maps to its UI;
  - an encrypted session;
  - a push-to-talk run through stdin;
  - `TtsUrl` followed by `AnnounceFinished`;
  - replacement of the first client by a second one.

`fake_device` takes the options `--port`, `--mode wake|ptt|none`, `--features N`, `--mac`, `--name` and `--mdns`. It reads the key from `WEBOS_ESPHOME_PSK` and accepts the stdin commands `ptt`, `wake`, `stop`, `mute`, `unmute` and `quit`. Each app event is printed as an `EVENT …` line. The harness generates its keys at runtime.

Results on 2026-11-02 (host x86_64, Python 3.14, aioesphomeapi 46.6.0):

- 35 unit tests and 2 loopback tests passed.
- All 33 harness checks passed, against the host build and against the static armv7 build under `qemu-arm-static`.

Cross-build: the armv7 binary is static EABI5 with VFPv3, about 0.9 MB.

```sh
CARGO_TARGET_DIR=target/w-esphome cargo zigbuild --release --target armv7-unknown-linux-musleabi --example fake_device -p webos-esphome-api
```

## Open points for step 1.4 (live HA)

Results so far, from the bridge runs in [P1 HA integration](../discovery/c3-a/P1-ha-integration.md):

- The pipeline asked for `alexa_v0.1`, which never fired. Kiwi was changed to `alexa`.
- The Piper reply reaches the device in SPEAKER mode.
- The real order of events is `RUN_END` before `TTS_STREAM_START`.
- aioesphomeapi sends the API-audio subscribe flag as `1 << 2`, not as api.proto's `1`; both are accepted.
- On disconnect, a reply that has fully arrived keeps playing.

The original list follows.

- **Adding the device:** Settings → Devices & services → Add integration → ESPHome, then enter the TV's IP and port 6053. HA tries plaintext first. When the device requires Noise, HA asks for the encryption key, which is the same base64 key as `noise_psk`. With mDNS working, HA may offer the device as discovered instead.
- **Things that need a real HA instance:**
  - the wake word name the pipeline actually runs (`alexa` or `alexa_v0.1`, see [ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md));
  - that the Piper WAV stream reaches the device in SPEAKER mode with the Spanish voice;
  - that HA's pacing (about 384 ms ahead of real time) suits the playback buffer;
  - whether HA sends `TTS_STREAM_START` before or after `RUN_END` in practice. Both orders are handled, with a 2 s timeout if no stream comes.
- **Announcements** still arrive as URLs, also in SPEAKER mode. They fail while HA's internal URL is unset or points at the pod IP.
- **Audio rate:** the satellite streams continuously at 256 kbit/s while the wake word is armed. Measure HA's CPU use for openWakeWord.
