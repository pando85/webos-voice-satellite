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
  - `START_CONVERSATION` (32) is implemented but off by default (see [START_CONVERSATION status](#start_conversation-status)).
  - `TIMERS` (8) is implemented but off by default (see [Timers](#timers)).
  - `MULTI_CHANNEL_AUDIO` (64) is rejected by config validation because it is not implemented.
- **Request flags:** `USE_WAKE_WORD` (2) is set for wake word runs. `USE_VAD` is never set, because HA 2026.10.4 ignores it.
- **Entities:**
  - One `media_player`. Its single ANNOUNCEMENT format is `flac`, 22050 Hz, 1 channel, 2 bytes. That is Piper's native rate, it streams well and symphonia can decode it. `sample_rate` is always set, to work around the HA 2026.10.4 bug. The only media feature is STOP. In URL mode this format becomes HA's TTS format. In all modes it is the format of announcement media proxied through ffmpeg.
  - One "Mute microphone" switch, entity category config.
- **Wake words:** `VoiceAssistantConfigurationResponse` reports no available or active wake words and `max_active_wake_words = 0`. The wake word is whatever the selected HA pipeline configures.
- **Encryption:**
  - With `noise_psk` set, the device accepts only Noise. Plaintext clients get the Noise-framed rejection, which HA reports as "requires encryption" and answers by asking for the key.
  - With `provisioning` set (and no `noise_psk`), HA sets the key itself: see [Pairing](#pairing-ha-driven-key-provisioning).
  - With neither, the device serves plaintext only (development). `api_encryption_supported` is then false, so HA never tries to provision a key.
- **One HA client at a time.** A new connection that completes its handshake replaces the old one. Keyless pairing sessions are the exception: they never replace it (below).
- **Retry policy:**
  - A run that ends with `ERROR`, or without progress (no `WAKE_WORD_END` or `STT_END`), delays the next wake word start by 250 ms × 2ⁿ.
  - Configuration errors (`*-missing`, `*not-found*`) use 2 s × 2ⁿ instead.
  - The delay is capped at 60 s.
  - HA's start reply must arrive within 10 s.
- **mDNS:** optional (`Config::mdns`, cargo feature `mdns`, on by default). It advertises `_esphomelib._tcp` with TXT records `friendly_name`, `version`, `mac`, `platform`, `board`, `network` and `api_encryption`, using `mdns-sd`. If the advertisement fails, the server logs a warning and keeps running.

## Pairing (HA-driven key provisioning)

Status: **implemented (2026-11-02), tested on a PC against aioesphomeapi 46.6.0 acting as HA and once against a real HA 2026.10.4 on the PC; verified on c3-a with the owner's HA 2026.10.4** ([P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md#3-pairing-on-the-tv-with-the-owners-ha)), except two points changed afterwards and **not yet re-verified with a real HA**: the deferred `PROVISIONING_CLOSED` disconnect and the key clear by the key holder (both below). Design and threat model: [install and configuration UX](../design/install-and-config-ux.md#5-pairing-mode-design-option-c-detail). Code: [`pairing.rs`](../../crates/esphome-api/src/pairing.rs) (pure state machine and key file), [`server.rs`](../../crates/esphome-api/src/server.rs).

`Config::provisioning = Some(Provisioning { key_file, window, open_at_start })` with `noise_psk = None`:

- **Start.** The server reads `key_file`. A valid key means paired (Noise with that key, as a fixed key). No file means unpaired. A corrupt file (not base64 of 32 non-zero bytes) stops `Server::bind` with an error instead of silently reopening pairing. `Server::needs_pairing()` tells the application to keep the microphone closed.
- **Window.** Opened at `run()` when unpaired and `open_at_start`, or by `AppCommand::OpenPairingWindow`. Closes after `window` (in memory only, so a restart reopens it if `open_at_start`) or when a key is accepted. `AppCommand::ResetPairing` deletes the key file, closes every session and reopens it. After a clear by the key holder (below) the window stays closed and is not opened at start either, until `OpenPairingWindow` or `ResetPairing`. Changes arrive as `AppEvent::Pairing(PairingEvent::…)`: `WindowOpened`, `WindowClosed`, `Paired { client_info, peer }`, `KeyRotated`, `Reset`, `Cleared { client_info, peer }`.
- **Keyless sessions** (unpaired only). The first byte selects plaintext or Noise with the all-zero PSK, as ESPHome does. Each keyless session runs in its own task (at most four) and never becomes the main session, so the voice state machine, the entities and microphone audio are unreachable without a key. It answers `HelloRequest`, `DeviceInfoRequest` (with `api_encryption_supported` and `api_encryption_provisionable` set and `voice_assistant_feature_flags = 0`), `ListEntitiesRequest` with only `ListEntitiesDoneResponse` (HA lists entities before it provisions), ping, disconnect and `NoiseEncryptionSetKeyRequest`. Voice, switch and media player messages are logged and ignored. While the window is closed, a new connection gets its `HelloResponse` and then `DisconnectRequest(reason = PROVISIONING_CLOSED)`, which HA's config flow reports as "closed its setup window"; existing keyless sessions are closed when the window closes. The `DisconnectRequest` waits for the client's next message (HA's flow sends `DeviceInfoRequest` once `connect()` returned) or 2 s, then the server waits up to 1 s for the `DisconnectResponse`. Reason: aioesphomeapi 46.6.0 raises the `ConnectionClosedEvent` that carries the reason only if the connection was already `CONNECTED` (`was_connected` in `_cleanup`), and the state becomes `CONNECTED` only when `connect()` resumes after the `HelloResponse`. If both frames arrive in one read, the disconnect is handled first, `connect()` fails without the event and HA 2026.10.4's `_fetch_device_info` returns `connection_error`. That is what the owner's HA in its pod showed 4 of 4 on c3-a; the fake-HA scenario F reproduces it with a proxy that batches the device's frames (6 of 6 `connection_error` before the change, 6 of 6 `provisioning_closed` after).
- **`SetKey`.** The key field holds base64 text, as HA and ESPHome use it. Accepted only from a zero-PSK Noise session while the window is open, at most one attempt per second (any session): base64 of exactly 32 bytes, not all zero, not empty. The key is written to `key_file` atomically (exclusive temporary file with mode 0600 in the same directory, `fsync`, `rename`, directory `fsync`, base64 plus a newline, the `noise_psk_file` format). Then the reply `success = true` is sent, the key is activated, and every session opened before it gets a `DisconnectRequest` and is closed (ESPHome does this 100 ms after its reply): keyless sessions at once, the main session (a rotation or clear on it) after its client disconnected by itself or after 500 ms, answering only disconnect and ping meanwhile, because HA's `cli.disconnect()` follows the reply at once and would otherwise race the server's request. HA then reconnects with the key. A storage failure answers `success = false` and keeps the device unpaired.
- **After pairing.** Plaintext and zero-PSK clients are refused as with a fixed key ("requires encryption", "Handshake MAC failure", which HA reads as "already has an encryption key"). A session that proved the key may **rotate** it with a new valid key (ESPHome behaviour; HA 2026.10.4 does not do this). Setting the current key again answers success and changes nothing. A key fixed in the configuration is never changed or cleared (ESPHome: "Key set in YAML").
- **Clear by the key holder.** An empty key from a session that proved the stored key (HA 2026.10.4 `_async_clear_dynamic_encryption_key`, run when the entry is deleted) is accepted, as ESPHome does: the server writes the marker `<key_file>.cleared` (mode 0600), deletes the key file, answers `success = true` (HA then removes its stored key), closes the sessions as above and emits `PairingEvent::Cleared`. It is then unpaired with the window **closed**. The marker keeps it closed across restarts (`Server::bind` ignores `open_at_start` while the marker exists and no key is stored); `OpenPairingWindow`, `ResetPairing` or a new key remove it. If the marker cannot be written or the key cannot be deleted, the reply is `success = false` and the key stays. An empty key from a keyless session (unpaired: nothing to clear) is refused, and a clear counts against the one-attempt-per-second limit.
- **Logging.** The key is never logged or put in `Debug` output: `KeyState`, `Transport` and `Refusal` redact or omit it, and refusals name the reason only. Logs contain the peer address, the client info and the key file path.
- **mDNS** (when enabled) follows the runtime state: TXT without `api_encryption` while the window is open, with it once a key is active, and no advertisement while unpaired with the window closed. Unverified on a TV.

Divergences from ESPHome 2026.10.1 (all deliberate):

| ESPHome | This crate | Why |
| --- | --- | --- |
| A keyless device is a full plaintext device (entities, voice) | Keyless sessions get identity, an empty entity list and `SetKey` only | No microphone access or control without a key ([design rule 1](../design/install-and-config-ux.md#52-rules)) |
| `SetKey` accepted over plaintext (deprecated, "will be removed in 2027.2.0") | Refused over plaintext | The key would cross the LAN in clear; HA 2026.9+ uses the zero-PSK path when `api_encryption_provisionable` is set |
| Empty key clears the key from any session that reached `SetKey` | Only from a session that proved the stored key; never with a key from the configuration | Same effect for HA (entry removal clears both sides); nobody without the key can unpair the device |
| After a clear the device is keyless and, with the provisioning window already satisfied, accepts keyless sessions until the next reboot | Unpaired with the window closed, persisted in `<key_file>.cleared`, until a local open or reset | A removed device must not be claimable by the next LAN peer; HA sends the clear when its entry is deleted, not to re-pair |
| Closed window: `DisconnectRequest(PROVISIONING_CLOSED)` right behind the `HelloResponse` | After the client's next message, or 2 s | The back-to-back frames race in aioesphomeapi 46.6.0 and HA shows `connection_error` (c3-a P3) |
| Disconnects every client 100 ms after a key change | Keyless sessions at once (verified with HA on c3-a); the main session after the client's own disconnect or 500 ms | HA disconnects right after the `SetKey` reply; answering it avoids a race |
| `provisioning:` window is optional; without it a keyless device accepts a key forever | The window is mandatory (1 s to 1 day) | Bounded exposure |
| Multiple concurrent API clients | One main session plus up to four keyless sessions | Existing single-client design; HA needs two keyless sessions at once (main and zero-PSK) |
| `voice_assistant_feature_flags` as configured | 0 on keyless sessions | HA creates no satellite before a key exists |

## Timers

Status: **implemented in the API crate and wired into the satellite config (`[device] timers = true`), soundless (the chime is Phase 4), off by default.** Verified against aioesphomeapi 46.6.0 acting as HA, not against a real HA.

How HA behaves (HA 2026.10.4 `esphome/assist_satellite.py`, `intent/timers.py`; source reading):

- When the device advertises `TIMERS` (bit 8) in its voice feature flags, the ESPHome integration registers a timer handler for the device. It forwards each HA intent timer event as a `VoiceAssistantTimerEventResponse` with `timer_id`, `name`, `total_seconds` (the created seconds, which grow when time is added), `seconds_left` and `is_active`.
- The event types are `STARTED` (timer created), `UPDATED` (time added or removed, paused, resumed; a pause is `UPDATED` with `is_active = false`), `CANCELLED` and `FINISHED`.
- HA owns the timers and counts them down. The device never reports timers back. There is no message to list timers.
- HA does **not** replay existing timers when the device reconnects. After a reconnect the handler sees only later events. A device that wants to survive a reconnect must keep its own state.

What the crate does:

- Enable it with `Config::voice_features = features::DEFAULT_WITH_TIMERS` (31). The default stays `features::DEFAULT` (23). `fake_device` takes `--features 31`. In the satellite, set `[device] timers = true` (and optionally `start_conversation = true`, which needs `announce = true`); see the configuration reference in [development.md](../development.md#configuration-reference).
- With TIMERS advertised, each HA timer message becomes `AppEvent::Timer { event: TimerEvent, id, name, total_seconds, seconds_left, is_active }`. Unknown event types are dropped with a debug line. Without TIMERS the messages are ignored.
- `AppEvent`'s hand-written `Debug` prints event, id and seconds only. The timer name is user speech and stays out of logs (`name_bytes` in `Timer`'s `Debug`). `AppEvent` was already `#[non_exhaustive]`, so adding the variant breaks no match.
- `TimerTable` is an optional helper for the application: `apply(&AppEvent, Instant)` keeps the timers HA reported (at most 64), `Timer::remaining(now)` counts down locally while the timer is active, and `by_remaining` lists them soonest first. It lives in the application and so survives HA reconnects, which is the only state recovery available given that HA does not replay. The server itself keeps no timer state.
- The satellite keeps a `TimerTable` (survives HA reconnects) and logs each event at info level (`timer <event> id=... seconds_left=...`, so a finished timer logs `timer finished id=... seconds_left=0`; never the name). It plays no sound yet: the chime is Phase 4 work (see below). `satellite_e2e.py` checks this with a daemon started with `timers = true` and `start_conversation = true` (flags 63).

What the TV will do (Phase 4, not now): on `FINISHED` the satellite will play a repeating chime through the playback path until the user stops it (voice command to HA, which cancels the timer, or the media player STOP) or a time limit passes. The chime asset, the stop gesture, the volume and the interaction with a running pipeline run (duck or queue) are open Phase 4 and Phase 2 audio decisions. A disconnected HA cannot send `FINISHED`, so the application may sound a known timer from its local `remaining` time. HA's own `FINISHED` after a reconnect must not sound it twice, which `TimerTable` supports by dropping the timer on the first `FINISHED`.

## START_CONVERSATION status

Status: **works in the implementation, tested with aioesphomeapi 46.6.0, off by default in the advertised flags; the satellite behaves the same either way.** Not yet exercised against a real HA.

- HA side (`assist_satellite.py`): `assist_satellite.start_conversation` calls `async_start_conversation`, which is `_do_announce(..., run_pipeline_after=True)`. It sends `VoiceAssistantAnnounceRequest` with `start_conversation = true` and waits for `VoiceAssistantAnnounceFinished`. HA adds the `START_CONVERSATION` supported feature to the satellite entity only when the device advertises bit 32. Without the bit, HA does not offer the feature for this entity (the service gating itself is in the `assist_satellite` component, which was not read).
- Device side: `Voice::on_announce` aborts any run, plays the announcement (`AppEvent::Announce`, `start_conversation` flag), and after `AppCommand::AnnounceFinished` sends `AnnounceFinished` to HA and then starts a run without wake word (at STT) instead of re-arming. This happens regardless of the advertised bit, because the request itself carries the flag. Harness check A21 covers it (announce, then a run with `USE_WAKE_WORD` unset, then re-arm after `RUN_END`).
- So, to let `assist_satellite.start_conversation` be called for the TV, advertise bit 32 (`features::DEFAULT | features::START_CONVERSATION` = 55). `ANNOUNCE` (16) is also needed because HA waits for the announcement first. Announcements still arrive as URLs, which fail while HA's internal URL is unreachable from the TV (see the announcement note under the open points).
- Not verified: the real HA flow end to end (including HA's 5 minute announcement timeout, `_ANNOUNCEMENT_TIMEOUT_SEC`, when playback fails) and the TV audio path for the announcement.

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
- **Scenario A** also checks that a stray timer event is ignored while TIMERS is not advertised.
- **Scenario C** (plaintext, features 31) covers the TIMERS flag in device info and the app events for started, updated (time added, paused, resumed), finished and cancelled timers, a timer without a name, and that the device log (at debug level) never contains the timer name.
- **Scenario B** (Noise, URL mode, push-to-talk) covers:
  - refusal of a plaintext client and of a wrong key, with the exceptions HA maps to its UI;
  - an encrypted session;
  - a push-to-talk run through stdin;
  - `TtsUrl` followed by `AnnounceFinished`;
  - replacement of the first client by a second one.
- **Scenario D** (pairing) replays HA 2026.10.4's provisioning with aioesphomeapi: the keyless main session (provisionable, no voice features, no entities, no start or audio after subscribing, the app is not told HA connected), `SetKey` over plaintext refused, `_async_provision_key_over_noise` (zero-PSK client, `noise_encryption_set_key`) accepted, the keyless session closed, the key file (mode 0600, same key), a reconnect with the key that gives the full device and streams audio, refusal of no key, a wrong key and the zero PSK as HA's config flow classifies them, provisioning refused once paired, `reset-pairing`, and no key in the device log at trace level.
- **Scenario E** (pairing window) checks HA's config flow logic (`_fetch_device_info`) before and after a 1.5 s window: `provisioning_closed` on both transports, no key accepted, and the `pair` command reopening it.
- **Scenario F** (closed window with HA's network timing) runs the same config flow logic through `CoalescingProxy`, which hands the device's bytes to the client in batches (everything within 50 ms in one write), as HA in its pod received them on c3-a: `provisioning_closed` 6 of 6 on both transports. Against the build before the deferred disconnect it gives `connection_error` 6 of 6, the TV result.
- **Scenario G** (key clear) replays HA's `_async_clear_dynamic_encryption_key` after a provisioning: the empty key is accepted, the key file deleted and the marker written, HA's `disconnect()` right after the reply logs no aioesphomeapi error, keyless and old-key connections are refused, a restart keeps the window closed, the `pair` command reopens it and HA re-pairs with its stored key, and a key fixed in the configuration is not cleared.

`fake_device` takes the options `--port`, `--mode wake|ptt|none`, `--features N`, `--mac`, `--name`, `--mdns` and, for pairing, `--pair-key-file FILE`, `--pair-window SECS` and `--pair-closed`. It reads the key from `WEBOS_ESPHOME_PSK` and accepts the stdin commands `ptt`, `wake`, `stop`, `mute`, `unmute`, `pair`, `reset-pairing` and `quit`. Each app event is printed as an `EVENT …` line. The harness generates its keys at runtime.

`tests/pairing_io.rs` drives the same flows with a minimal Rust client over loopback (restricted keyless session, zero-PSK pairing with bad keys first, window timeout and reopen, a window closed at start, the timing of the closed-window disconnect, reset, rotation, a clear by the key holder and the closed window after it (also after a restart), a fixed key, a corrupt key file) and checks with a capturing logger at trace level that no key reaches the log.

Results on 2026-11-02 (host x86_64, Python 3.14, aioesphomeapi 46.6.0):

- 35 unit tests and 2 loopback tests passed.
- All 33 harness checks passed, against the host build and against the static armv7 build under `qemu-arm-static`.

Results on 2026-11-02 after the timers work (host x86_64 only): 42 unit tests and 4 loopback tests in the crate (the timer ones are new), 42 `fake_ha.py` checks and 13 satellite end-to-end checks passed. The armv7 and qemu run was not repeated.

Results on 2026-11-02 after the pairing work (host x86_64 only): 54 unit tests, 4 loopback tests and 7 pairing loopback tests in the crate, 58 `fake_ha.py` checks and 29 satellite end-to-end checks passed.

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
- **Announcements** still arrive as URLs, also in SPEAKER mode. They need an HA internal URL the TV can reach. Verified on 2026-11-02, with and without the pre-announce chime, after the owner set HA's internal URL to the LAN ingress ([P1 HA integration](../discovery/c3-a/P1-ha-integration.md#announcements-with-has-internal-url-2026-11-02)). Before that, the URL was the pod address: the fetch failed after 5 s and the device answered `AnnounceFinished(false)`.
- **Audio rate:** the satellite streams continuously at 256 kbit/s while the wake word is armed. Measure HA's CPU use for openWakeWord.
