# Home Assistant ESPHome integration notes (Phase 1 step 1.4 reconnaissance)

Date: 2026-11-02. Scope: what the owner's Home Assistant (see ~/homelab) will do when the webOS satellite is added as an ESPHome-API device. Read-only reconnaissance; nothing was created or changed in HA or the cluster. No tokens, passwords or IP addresses are recorded here.

Evidence labels: `verified-on-target` here means observed on the owner's HA instance (HA core 2026.10.4), not on the TV. `documented` means read from HA core source at tag 2026.10.4 (`homeassistant/components/esphome/`, `assist_satellite/`, `assist_pipeline/`, `wyoming/`, `helpers/network.py`) and aioesphomeapi 46.6.0 (the version pinned by that tag). Source reading is not a live test of the satellite.

## 1. Pipelines (verified-on-target, via WS `assist_pipeline/pipeline/list`)

| Name | Language | STT | TTS engine / voice | Conversation | Wake word entity | wake_word_id |
|---|---|---|---|---|---|---|
| Kiwi (preferred) | es | `stt.faster_whisper` (es) | `tts.piper` / `es_ES-sharvard-medium` (tts language `es_ES`) | `conversation.qwen2_5_3b` | `wake_word.openwakeword` | `alexa_v0.1` |
| Kiwi Test | es | `stt.faster_whisper` (es) | `tts.piper` / `es_ES-sharvard-medium` | `conversation.chatgpt` | `wake_word.openwakeword` | `alexa_v0.1` |

`preferred_pipeline` is the Kiwi pipeline. `prefer_local_intents` is false.

## 2. Wake word, STT, TTS entities

- WS `wake_word/info` with `{"entity_id":"wake_word.openwakeword"}` (verified-on-target) returns these ids: `okay_nabu`, `hey_jarvis`, `hey_mycroft`, `alexa`, `hey_rhasspy`. The id is `alexa`, with no `_v0.1` suffix.
- **Mismatch (risk, hypothesis from source):** the Kiwi pipeline stores `wake_word_id = "alexa_v0.1"`, which is not in the list above. The wake stage passes `pipeline.wake_word_id` to the Wyoming entity (`assist_pipeline/default_pipeline.py`). The Wyoming entity sends `Detect(names=["alexa_v0.1"])` (`wyoming/wake_word.py`). In wyoming-openwakeword 2.1.0 (the image deployed in the cluster, started with `--preload-model alexa`), names that are not valid model ids are skipped and it falls back to its default model. HA then logs "Expected wake word ... but got ..." and skips detections. So server-side wake word on the Kiwi pipeline probably never fires until the pipeline is re-saved with `alexa`. Not tested live. The fix is an owner-side change: edit the pipeline's wake word to "Alexa" in the HA UI, or use a dedicated pipeline for the satellite.
- Entities in the entity registry: `stt.faster_whisper` (wyoming), `stt.openai_stt`, `tts.piper` (wyoming), `tts.openai_tts`, `tts.google_en_com`, `wake_word.openwakeword` (wyoming), `conversation.qwen2_5`, `conversation.qwen2_5_3b`, `conversation.chatgpt`, `conversation.openwebui_millaguie`.
- Piper voices for `es_ES`: `es_ES-carlfm-x_low`, `es_ES-davefx-medium`, `es_ES-mls_10246-low`, `es_ES-mls_9972-low`, `es_ES-sharvard-medium`.

## 3. Existing ESPHome entries and satellites (verified-on-target)

- 8 config entries for domain `esphome`, all `source: user`, all `loaded`, all with an options flow (`supports_options: true`). They are not voice devices: a rack controller, a mini switch, two plant sensors, two smart plugs and two irrigation controllers.
- No `assist_satellite.*` entities exist. No ESPHome device exposes the voice-assistant feature today.
- So the HA pod can already open outbound API connections to ESPHome devices (indirect evidence for section 5).

## 4. Adding an ESPHome device manually (documented, HA 2026.10.4 `esphome/config_flow.py`)

REST sequence (all with `Authorization: Bearer <token>`):

1. `POST /api/config/config_entries/flow` with `{"handler":"esphome","show_advanced_options":false}`. Response: form `step_id: "user"`, fields `host` (required, str) and `port` (optional, int, default 6053).
2. `POST /api/config/config_entries/flow/<flow_id>` with `{"host":"<tv host>","port":6053}`. HA, from the pod, opens a TCP connection to the device and does `Hello`, then `DeviceInfoRequest`. HA first tries without encryption. Outcomes:
   - Device reachable, no encryption, no password: the flow returns `create_entry` immediately. The entry has title = `friendly_name` or `name`, data `{host, port, password:"", noise_psk:"", device_name}`, and options `{allow_service_calls: false}`.
   - Device demands encryption (`RequiresEncryptionAPIError`): HA probes with a zero PSK to learn name and MAC, tries stored keys and the ESPHome dashboard key, then returns form `step_id: "encryption_key"` with required field `noise_psk` (base64 32-byte key). `POST /flow/<id>` with `{"noise_psk":"<key>"}`. A wrong key returns the form again with error `invalid_psk`.
   - `DeviceInfo.uses_password` is true: form `step_id: "authenticate"` with required field `password`. This legacy path should be avoided: the satellite should not set a password.
   - Errors re-show the `user` form with `errors.base`: `resolve_error`, `connection_error`, `requires_encryption_key`, `invalid_auth`, or provisioning-closed.
3. Another entry with the same `device_name` but a different MAC: the flow shows step `name_conflict`, a menu with `name_conflict_migrate` and `name_conflict_overwrite`. Avoid this by choosing a unique device name.
4. Options (post-creation): `POST /api/config/config_entries/options/flow` with `{"handler":"<entry_id>"}` returns form `init` with `allow_service_calls` (bool, default true in the form but the new entry starts `false`) and `subscribe_logs` (bool). Submit with `POST /api/config/config_entries/options/flow/<flow_id>` and `{"allow_service_calls":true,"subscribe_logs":false}`. `allow_service_calls` only lets the device call HA actions; it is not needed for the voice pipeline, and the options flow reloads the entry.

What the device must answer for the flow to succeed (documented):

- `HelloResponse` with API major version 1 or 2 (aioesphomeapi 46.6.0 sends 1.18 and rejects a server major above 2).
- `DeviceInfoResponse` with `mac_address` (becomes the unique_id after `format_mac`; must be stable and unique), `name` (device name, stored as `device_name`), `friendly_name` (becomes the entry title and device name), `uses_password` false, and `esphome_version` (free-form).
- Duplicate MAC or unique_id: the flow aborts. If the stored host and port match the new ones, the reason is `already_configured`. If the existing entry is loaded and connected, the abort is `already_configured` without updating the host. Otherwise the abort is `already_configured_updates` (host/port/PSK updated on the existing entry) or `already_configured_detailed`. An entry that was ignored aborts with `already_configured`.

Version gating (documented):

- HA does not require a minimum `esphome_version` string for voice features. `esphome_version` is parsed with AwesomeVersion only for the Bluetooth-proxy "firmware outdated" repair (`STABLE_BLE_VERSION` 2026.5.1), and only if the device advertises Bluetooth-proxy flags. It is otherwise just stored as the device `sw_version`. Use any sensible string, for example the project version.
- Voice features are gated on the API version reported in `HelloResponse`: `DeviceInfo.voice_assistant_feature_flags_compat(api_version)` in aioesphomeapi 46.6.0 uses the legacy `legacy_voice_assistant_version` field if `api_version < 1.10` (1 means voice assistant, 2 adds speaker). For `api_version >= 1.10` it uses `voice_assistant_feature_flags`. The satellite should report API version at least 1.10 and set `voice_assistant_feature_flags`.
- If the flags are non-zero, HA creates the `assist_satellite` platform entity on first connect (`esphome/manager.py`). With flags 0, no satellite entity is created.
- `VoiceAssistantFeature` bits: `VOICE_ASSISTANT` 1, `SPEAKER` 2, `API_AUDIO` 4, `TIMERS` 8, `ANNOUNCE` 16, `START_CONVERSATION` 32, `MULTI_CHANNEL_AUDIO` 64.

Entities created for a voice device (documented, via unique ids `<mac>-pipeline`, `<mac>-vad_sensitivity`, `<mac>-wake_word`): `assist_satellite.<name>`, a pipeline `select` (options: preferred or a pipeline name; this picks the pipeline), a VAD sensitivity `select`, and (if wake words are configured) a wake word `select`.

## 5. Voice flow behaviour that the satellite must match (documented)

- Audio in: with `API_AUDIO` set, HA subscribes to audio over the API (TCP; handler receives `bytes` chunks, plus a second non-enhanced channel only if `MULTI_CHANNEL_AUDIO`). Without it HA uses UDP and returns a UDP port from `handle_pipeline_start`. Plan: `API_AUDIO`.
- `VoiceAssistantRequest` flags: `USE_WAKE_WORD` (bit 1) makes HA start the pipeline at the wake-word stage (server-side wake word on the streamed audio); otherwise it starts at STT. `USE_VAD` is bit 0. HA runs wake word with `pipeline.wake_word_entity` and `pipeline.wake_word_id` from the pipeline picked by the pipeline select (not from the device). The device's `wake_word_phrase` is only used to pick among up to several wake-word selects.
- Pipeline ends at TTS. Events sent to the device: `RUN_START` (may carry a `url` for pre-announce), `STT_START`, `STT_END` (`text`), `INTENT_PROGRESS` (`tts_start_streaming`), `INTENT_END` (`conversation_id`, `continue_conversation`, `speech`), `TTS_START` (`text`), `TTS_END` (`url`), `RUN_END`, `ERROR` (`code`, `message`; HA emits `no_wake_word` when nothing was detected), and `TTS_STREAM_START` / `TTS_STREAM_END` when streaming.
- TTS format:
  - If `SPEAKER` is set, HA requests WAV 16 kHz, 1 channel, 16-bit and streams the audio to the device as `voice_assistant_audio` API messages. The device does not need to fetch a URL, but it still receives `TTS_END.url`. With `SPEAKER` and without `API_AUDIO`, HA streams over UDP instead. Only WAV is streamed; other formats log an error. HA paces about 384 ms ahead of real time; the device buffer is expected to be about 512 ms.
  - If `SPEAKER` is not set, HA takes the ANNOUNCEMENT-purpose format from the device's `media_player` entity `supported_formats` (format, sample rate, channels, sample bytes) as the preferred TTS format, and the device must download `TTS_END.url`.
- Announcements (`ANNOUNCE`): HA calls the device with a media id (a signed URL via the ffmpeg proxy for non-TTS media). The device must answer with `VoiceAssistantAnnounceFinished`. With `ANNOUNCE` set, HA also requests `VoiceAssistantConfigurationRequest` (5 s timeout; a timeout is tolerated and a placeholder config used).
- `TTS_END.url`: `/api/tts_proxy/` paths are not signed. Other HA paths get a signed URL (`async_process_play_media_url`).

## 6. Network reachability and advertised URLs

- Pod network (verified-on-target, kubectl read-only): the HA pod and its Wyoming pods (openWakeWord, Piper, Whisper) use a normal pod network (`hostNetwork` unset, DNS `ClusterFirst`). Services are ClusterIP only. HA is exposed through an nginx ingress (`nginx-external`) on the owner's HA hostname. A pod on the pod network does reach the LAN through NAT, as the 8 existing ESPHome entries show (inferred). mDNS/zeroconf discovery from a pod does not see LAN devices, so add the satellite manually by host and port, and use a stable address (static DHCP lease or resolvable hostname).
- NetworkPolicies (verified-on-target): none in the Home Assistant namespace. The only policies cluster-wide are in unrelated namespaces (keycloak, mintpsicologia, nextcloud coturn, plus one Cilium policy for CI runners). So no egress restriction applies to TCP 6053 from HA. This was not tested by connecting to a device.
- Advertised URLs (verified-on-target): `internal_url` and `external_url` are both unset (`null` in `/api/config` and `get_config`). WS `network/url` reports `internal` = `http://<pod IP>:8123` (the pod's cluster-internal 10.x address; confirmed equal to the pod IP), `external` null, `cloud` null. HA serves plain HTTP behind the ingress, so `hass.config.api.use_ssl` is false and `get_url` falls back to the detected local (pod) IP (`helpers/network.py`).
- Consequence (hypothesis from source, not tested end to end): any media URL HA sends to the satellite (`TTS_END.url`, announcement URLs, RUN_START URL) would point at `http://<pod IP>:8123/...`, which a TV on the LAN cannot reach. Two ways around it:
  1. The satellite advertises `SPEAKER` + `API_AUDIO` and plays the streamed WAV from the API, so it never fetches a URL for conversational TTS (preferred for the MVP; it also avoids relying on TLS or DNS on the TV).
  2. The owner sets Internal URL and/or External URL in HA general settings to an address the TV can reach (an owner-side configuration change, not made here). The TV must then be able to resolve and reach that address and validate its TLS certificate.
  Announcements and `start_conversation` with non-TTS media also need a reachable URL even in case 1.

## Residual unknowns

- Whether the existing ESPHome entries are reached by LAN IP or hostname, and whether the TV's LAN segment is routable from the pod network (a live test is needed when the daemon exists).
- Whether the stale `alexa_v0.1` wake word id breaks detection in practice (needs a live pipeline run).
- Exact behaviour of the real satellite against aioesphomeapi 46.6.0 (handshake, Noise, voice request fields); to be covered by the Phase 1 fake-HA and integration tests.
