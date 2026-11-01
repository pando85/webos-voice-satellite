# P1: Satellite against the real Home Assistant (roadmap 1.4)

- **Date:** 2026-11-02
- **Device under test:** the `webos-satellite` binary on the dev PC (x86_64 host build), microphone = a **TV-mic recording from c3-a** (D3 run `tv-wov-speech-g12`, S24_LE 2 ch 16 kHz), reply sink = WAV files. Nothing ran on the TV in this step.
- **Home Assistant:** the owner's instance, HA core 2026.10.4, running in a Kubernetes pod; pipelines "Kiwi" (preferred) and "Kiwi Test" (see [integration notes](../../research/ha-esphome-integration-notes.md#1-pipelines-verified-on-target-via-ws-assist_pipelinepipelinelist)).
- **Related:** [ESPHome API crate](../../development/esphome-api.md), [roadmap 1.4](../../project/roadmap.md#phase-1-silent-build-and-integration-no-sound-played), tools `tools/esphome-fake-ha/ha_bridge.py` and `satellite_e2e.py`.
- **Privacy:** logs, run summaries and reply WAVs are in the gitignored `artifacts/p1-ha/`; the derived mic files are in `devices/private/c3-a/P1-ha/`. No new recording was made. No addresses, keys or tokens appear here.

Labels: **verified-on-target** = observed against the owner's HA (and, for audio, using a c3-a recording); **documented** = HA source at tag 2026.10.4; **hypothesis**; **blocked**.

## Summary

| Question | Result | Label |
| --- | --- | --- |
| Can HA's ESPHome integration open a connection to the satellite? | **No.** HA's pod gets a TCP reset for every LAN host and port tried (the dev PC on 6053 and 22, the TV on 22, which is known to be open); IPv6 is unreachable from the pod. The same PC port is reachable from the TV on the LAN. HA's cluster network cannot open connections into the LAN segment where the TV and the dev PC live. | verified-on-target |
| ESPHome config entry and `assist_satellite` entity | **Not created** (the config flow stops at `connection_error`; the flow was aborted, nothing was left in HA) | blocked (needs owner) |
| Satellite protocol with HA's client library | Noise handshake, hello (API 1.14), device info (voice flags 23), voice subscription with API audio, start/stop, events and TTS stream all work against aioesphomeapi 46.6.0 (the library HA 2026.10.4 uses), through the bridge below | verified (library), not through HA's integration |
| Wake word id that the pipeline requests | Kiwi stored `alexa_v0.1`; openWakeWord exposes `alexa`. With `alexa_v0.1` **no detection** in 19.6 s of audio that contains the wake word; after changing only Kiwi's `wake_word_id` to `alexa`, **detected** at 6.98 s (end of the word) on the same audio | verified-on-target |
| Wake word → STT from TV-mic audio, through the satellite | detected, then STT produced text (see runs) | verified-on-target |
| Push-to-talk (SIGUSR1) → STT | `¿Qué hora es?`, word-perfect | verified-on-target |
| Intent (conversation agent) | **Fails on HA's side for every input**: Kiwi's agent `conversation.qwen2_5_3b` returns `model 'qwen2.5:3b' not found` from Ollama (`intent-failed`); Kiwi Test's agent `conversation.chatgpt` does not exist (`intent-not-supported`) | verified-on-target (owner-side problem) |
| TTS reply streamed to the satellite and written by its sink | Piper reply (the pipeline's TTS) streamed as 16 kHz mono PCM, 2612 ms; HA STT transcribes the satellite's WAV back to the exact reply text | verified-on-target (intent substituted, see method) |

## Method

### Network check

The satellite listened on TCP 6053 of the dev PC with a Noise key. The ESPHome config flow was started through HA's REST API (`POST /api/config/config_entries/flow`, handler `esphome`, then the host and port). HA answered `connection_error`, and HA's log showed `[Errno 111] Connect call failed` (connection refused) for the PC on 6053. Probing other hosts and ports through the same flow step showed the same reset for the PC on 22 and the TV on 22, and `[Errno 101] Network unreachable` for the PC's IPv6 address. From the TV, the PC's port 6053 accepted a TCP connection (the satellite logged it). The PC's own firewall is therefore not the cause; the reset comes from the path between HA's cluster network and the LAN (router policy, **hypothesis** for the exact place). The flow was then aborted (`DELETE /api/config/config_entries/flow/<id>`), so HA has no entry and no leftover state from this step.

### Bridge (stands in for HA's ESPHome integration)

Because HA cannot reach the satellite, `tools/esphome-fake-ha/ha_bridge.py` runs on the dev PC and plays the role of HA's ESPHome integration:

- It connects to the satellite with aioesphomeapi 46.6.0 (Noise), subscribes to voice with API audio, and answers start requests.
- Each start request becomes an HA WebSocket `assist_pipeline/run` on the selected pipeline: `wake_word`→`tts` when the device sets `USE_WAKE_WORD`, else `stt`→`tts`. Mic audio goes to HA as binary frames.
- Pipeline events are mapped to voice assistant events as `esphome/assist_satellite.py` 2026.10.4 does (**documented**), including `wake_word-end` without output → `ERROR no_wake_word`.
- The TTS result is fetched from HA's `tts_proxy`, converted to 16 kHz mono s16 with ffmpeg and streamed between `TTS_STREAM_START`/`TTS_STREAM_END` in 512-sample chunks, paced 384 ms ahead of real time like HA.
- **Substitute intent (labelled):** with `--reply-on-intent-failure TEXT`, when HA's intent stage fails the bridge asks the same pipeline for a TTS-only run of TEXT and sends that as the reply (`intent_substituted: true` in the run summary). Used here because both pipelines' agents are broken; the reply text was "Esta es una respuesta de prueba del satélite."

Differences from HA's integration that matter when reading the numbers: HA streams the WAV from its TTS stream directly, while the bridge downloads the proxy file (mp3) and converts it (879 ms the first time, 67 ms cached); wake-word timeout handling, the pipeline/VAD selects and announcements are not exercised.

### Microphone input

The D3 recording contains TTS "alexa" followed by three Spanish phrases. Two of them switch lights ("enciende la luz del salón", "apaga la luz de la cocina"), so they were **cut out** to make sure the live test could not switch real devices. Derived files, spliced at noise-floor points of the same TV-mic recording:

- wake test: start of the recording up to just after "alexa", then "¿qué hora es?", then noise (19.9 s);
- push-to-talk test: "¿qué hora es?", then noise (12.3 s).

The satellite read them with the file source (real-time pacing, 250 ms startup skip for the wake file, `pause_when_idle`) through the normal front-end (ch0, 120 Hz high-pass, +24 dB).

## Runs

All runs: satellite release build with Noise, mic file as above, WAV sink, pipeline Kiwi (after the wake word fix where noted). Times are milliseconds from the start of the pipeline run (HA event timestamps as seen by the bridge) or, for the satellite's own latency line, from the wake word or the push-to-talk start.

| Run | Mode | Wake word | Transcript | Intent | Reply | Key timings |
| --- | --- | --- | --- | --- | --- | --- |
| pc-ptt-1 | ptt (SIGUSR1) | n/a | `¿Qué hora es?` | intent-failed (Ollama model missing) | none | VAD start 1385, VAD end 2551, STT end 3460 |
| pc-ptt-kiwitest-1 | ptt, pipeline Kiwi Test | n/a | n/a | HA refused the run: `intent-not-supported` (agent not found) | none | refused at start |
| pc-wake-kiwi-1 | wake, `wake_word_id=alexa_v0.1` | **not detected** in 19.6 s | n/a | n/a | n/a | run stayed at `wake_word-start` |
| pc-wake-kiwi-2 | wake, `wake_word_id=alexa` | detected at 6980 (audio time) | `¿Qué va eso?` | intent-failed | none | wake end 7046, STT end 10777 |
| pc-wake-kiwi-3 | wake, `alexa`, substitute reply | detected at 6980 | `¿Qué va eso?` | substituted | 2612 ms Piper WAV | satellite: STT end +3708 after wake, TTS stream start +4596, playback end +6837 |
| pc-ptt-kiwi-3 | ptt, substitute reply | n/a | `¿Qué hora es?` | substituted | 2612 ms Piper WAV (cached) | satellite: STT end 3424, TTS stream start 3514, playback end 5755 |

Notes:

- In wake mode the phrase was transcribed as `¿Qué va eso?` in both runs (the same input gives the same result), while push-to-talk on the same phrase was word-perfect. In the wake runs, HA's VAD started 0.3 s after the wake word, so the STT segment probably includes the tail of "alexa" (**hypothesis**). One input, no rate is claimed. Phase 2 measures this with real voices.
- The satellite logged one latency line per run, for example `run 1 latency ms (from wake word): stt_end=3708 intent_end=3716 tts_start=4596 playback_end=6837 wake_after_stream_start=7042 reply_audio=2612`. With the WAV sink, "playback end" is the end of the stream; on the TV with `pacat` it includes PulseAudio's drain.
- Reply WAVs: 16 kHz mono, 2612 ms, 41358 non-zero samples. HA's STT on Kiwi transcribes both as `Esta es una respuesta de prueba del satélite.`, so the streamed reply reached the satellite intact.
- Satellite logs at debug level contain the transcripts (by design); at info level they do not (checked by `satellite_e2e.py` S13).
- The satellite logged `HA subscribed without API_AUDIO` at each connect although the request had `flags=4`. Cause: `api.proto` defines `VOICE_ASSISTANT_SUBSCRIBE_API_AUDIO = 1`, but aioesphomeapi 46.6.0 sends `API_AUDIO = 1 << 2` (4). Audio flowed over the API either way. The crate now accepts either bit, so the warning no longer fires for HA's client.

## Change made in HA (rollback)

- **Kiwi pipeline:** `wake_word_id` changed from `alexa_v0.1` to `alexa` via WS `assist_pipeline/pipeline/update`, with every other field sent unchanged. Compared after the change: only `wake_word_id` differs; Kiwi Test and the preferred pipeline are unchanged.
  - Rollback: the same call with `wake_word_id: "alexa_v0.1"`, or Settings → Voice assistants → Kiwi → wake word.
  - The change was kept: without it the wake word never fires on Kiwi.
- **Not fixed (owner):** Kiwi Test still has `alexa_v0.1` and a missing agent; Kiwi's agent fails because the Ollama model `qwen2.5:3b` is not available.
- No ESPHome entry was created, so none had to be removed. The aborted config flow left no state.

## Residual unknowns and needs-owner

- **Needs owner (blocks the 1.4 and 1.5 exit signals):** allow HA (its pod's egress, as seen by the router) to open TCP 6053 to the TV, and to the dev PC if PC tests through HA are wanted. An alternative is to put the TV on a network segment HA can already reach. Then add the device through the config flow (the procedure is in the [integration notes](../../research/ha-esphome-integration-notes.md#4-adding-an-esphome-device-manually-documented-ha-2026104-esphomeconfig_flowpy)) and confirm that the `assist_satellite` entity appears and which pipeline its select uses.
- **Needs owner:** a working conversation agent for Kiwi (restore the Ollama model or pick another agent). Until then every voice run ends at `intent-failed`.
- Behaviour that only HA's integration exercises: the entity, the pipeline/VAD selects, reconnection after HA restarts, the 384 ms TTS pacing from HA itself, and announcements (which also need a reachable HA URL, see the integration notes).
- Wake-mode transcription quality (the `¿Qué va eso?` result) with real voices: Phase 2.
