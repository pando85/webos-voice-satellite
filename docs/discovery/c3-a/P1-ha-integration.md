# P1: Satellite against the real Home Assistant (roadmap 1.4)

- **Date:** 2026-11-02
- **Device under test:** the `webos-satellite` binary on the dev PC (x86_64 host build), microphone = a **TV-mic recording from c3-a** (D3 run `tv-wov-speech-g12`, S24_LE 2 ch 16 kHz), reply sink = WAV files. Nothing ran on the TV in this step.
- **Home Assistant:** the owner's instance, HA core 2026.10.4, running in a Kubernetes pod; pipelines "Kiwi" (preferred) and "Kiwi Test" (see [integration notes](../../research/ha-esphome-integration-notes.md#1-pipelines-verified-on-target-via-ws-assist_pipelinepipelinelist)).
- **Related:** [ESPHome API crate](../../development/esphome-api.md), [roadmap 1.4](../../project/roadmap.md#phase-1-silent-build-and-integration-no-sound-played), tools `tools/esphome-fake-ha/ha_bridge.py` and `satellite_e2e.py`.
- **Privacy:** logs, run summaries and reply WAVs are in the gitignored `artifacts/p1-ha/`; the derived mic files are in `devices/private/c3-a/P1-ha/`. No new recording was made. No addresses, keys or tokens appear here.

Labels: **verified-on-target** = observed against the owner's HA (and, for audio, using a c3-a recording); **documented** = HA source at tag 2026.10.4; **hypothesis**; **blocked**.

## Announcements with HA's internal URL (2026-11-02)

The owner chose HA's LAN ingress address as HA's internal URL. Results are **verified-on-target** (permanent entry, satellite on c3-a from `/tmp`, sink `pacat` at in-process gain 0).

- **Pre-check from the TV:**
  - The TV resolves HA's internal URL (LAN ingress hostname) through its normal resolver.
  - The satellite's own HTTP stack (rustls with the bundled webpki roots, `play_url --fetch-only` from `/tmp`) fetched HA's front page with a valid certificate (200, 8677 bytes).
  - A self-signed control site was still rejected (`UnknownIssuer`).
- **Change in HA:** `internal_url` was set through WS `config/core/update`; HA's `network/url` now reports it.
  - `external_url` was left unset; it was not needed.
  - Rollback: set `internal_url` back to null, which was its previous value (as was `external_url`).
  - No warnings or errors appeared in HA's log after the change.
- **Reconnect:** HA reconnected to the permanent entry by itself 34 s after the satellite started.

| Check | Result |
| --- | --- |
| `assist_satellite.announce` (Spanish message, `preannounce: false`) | The URL now uses HA's internal URL, not the pod address. The satellite fetched over TLS 1.3: response headers 135 ms and body done 148 ms after the request arrived, 55 402 bytes, `audio/x-wav` (HA sent WAV, not the media player's flac format). Decode 1 ms, resample to 44.1 kHz stereo 59 ms. `pacat` finished 1987 ms after the request (1729 ms of audio). `AnnounceFinished(success=true)` was sent at once and the wake word re-armed. **HA's service call returned in 2169 ms** (the earlier failure took about 5 s) |
| Same with `preannounce: true` (HA's default chime) | First the chime through HA's ffmpeg proxy: `application/octet-stream`, which the decoder detected as FLAC; first byte after 15 ms; about 0.5 s of transcoding before the body ended. Then the message WAV (53 172 bytes, decode 0 ms, resample 41 ms). Total 4517 ms from the request to the end of playback; HA's call returned in 4711 ms; success |
| Silence | Decoded URL media goes through `PacatSink`, which multiplies every sample by the configured gain (0.0) before writing, covered by the test `pacat_sink_applies_software_gain`. The satellite used to print "0 non-zero samples" for URL media, but that number was not a measurement. The log now says the count is not taken for decoded URL media; streamed TTS keeps its real count |
| HA log during both announcements | no warnings or errors |
| TV after the test | satellite stopped, `/tmp` clean (including the fetch tool), 0 sink-inputs, mic PCM `closed`, kernel taint value unchanged |

## Permanent entry, direct connection (2026-11-02 morning)

The owner added a router rule so HA's network reaches the TV on TCP 6053. The satellite (build from commit 0bace3e) ran on c3-a from `/tmp`: wake mode, `lg-c3-a`, Noise, sink `pacat` at in-process gain 0, LG hands-free off. HA connected **directly to the TV**, with no relay or bridge. All results **verified-on-target**.

| Check | Result |
| --- | --- |
| Config flow (REST: host and port 6053, then the encryption key) | `user` → `encryption_key` (name `lg-c3-a`) → `create_entry`, title "LG C3 (c3-a) Voice Satellite", state `loaded`; the whole flow took 0.5 s. HA connected from its own network, and the satellite logged `Home Assistant 2026.10.4` |
| Device and entities | device `webos-voice-satellite` / `LG webOS TV` / 2026.10.1; `assist_satellite.*` (idle), `media_player.*` (idle), mute switch (off), two pipeline selects (preferred), finished speaking detection select (default), two wake word selects (unavailable, `no_wake_word`, expected) |
| Live mic, push-to-talk (SIGUSR1), temporary pipeline with `conversation.home_assistant` | One clean run: STT ended at HA's timeout after 15.9 s (silent room; Whisper hallucinated a phrase), the built-in reply (1811 ms) streamed by HA and played with 0 non-zero samples, wake re-armed |
| File mic (TV recording "alexa" + "¿qué hora es?"), wake mode | HA detected the wake word; STT `¿Qué va eso?`; built-in reply (3587 ms) streamed by HA and played with 0 non-zero samples; re-armed. From the wake word: STT end 3734 ms, intent end 3750 ms, TTS stream start 3751 ms, `pacat` drained 9183 ms. States idle → listening → processing → responding → idle. HA reconnected 1.5 s after the satellite was restarted |
| HA log | only the expected encryption probe warnings at setup, and one `APIConnectionError: Not connected` from the deliberate satellite restart during an active wake run (HA-side handling, seen before) |
| After the test | pipeline select set back to `preferred`, temporary pipeline deleted (HA has its 2 original pipelines). Satellite stopped and the TV's `/tmp` cleaned: no process, file or sink-input left, mic PCM `closed`, kernel taint value unchanged |

**The entry is kept** (HA now has 43 config entries). Its entities show `unavailable` while the satellite is not running. That is expected until autostart on the TV is approved (Phase 3); starting the satellite again from `/tmp` makes HA reconnect by itself.

## Update: HA's real ESPHome integration against the satellite on the TV (verified)

Later on 2026-11-02 HA's own ESPHome integration was connected to the satellite running on c3-a. The connection went through **a temporary TCP relay on a host the HA network may reach** (one port that demonstrably passes; the service normally on it was stopped for about 4 minutes and restored afterwards). Everything below is **verified-on-target** with HA core 2026.10.4. The satellite ran on the TV from `/tmp`: Noise, sink `pacat` with in-process gain 0, LG hands-free off.

| Check | Result |
| --- | --- |
| Config flow (REST) | `user` → `encryption_key` (HA asks for the key, `name: lg-c3-a`) → `create_entry`; the entry is `loaded`. HA's log showed the expected "requires encryption" and the zero-key probe ("Invalid encryption key: received_name=lg-c3-a") before the key was given |
| Device | name "LG C3 (c3-a) Voice Satellite", manufacturer `webos-voice-satellite`, model `LG webOS TV`, sw_version `2026.10.1` |
| Entities | `assist_satellite.*` (state `idle`, supported_features 1); `media_player.*` (`idle`); `switch.*mute_microphone` (`off`); two pipeline selects ("Assistant" and "Assistant 2", options preferred/Kiwi/Kiwi Test/test pipeline); "finished speaking detection" select; two wake word selects, `unavailable` with option `no_wake_word` (expected: the device reports no on-device wake words) |
| Live mic | right after the entry was created, HA subscribed and the satellite streamed the live mic into HA's wake word stage (silent room, no detection) |
| Temporary pipeline | "webos-satellite-test": es, faster-whisper, Piper `es_ES-sharvard-medium`, `conversation.home_assistant`, openWakeWord `alexa`; selected on the satellite's pipeline select |
| End to end through the real integration (file mic: TV recording "alexa" + "¿qué hora es?") | HA detected the wake word at 6.98 s of audio; STT `¿Qué va eso?`; the built-in intent answered (response type `error`: "Lo siento, no conozco ningún dispositivo llamado va eso"); **HA streamed the TTS over the API in SPEAKER mode** (WAV 16 kHz) with its own pacing. The satellite played 3587 ms through `pacat` with **0 non-zero samples** (stream `s16le 1ch 16000Hz` seen in `pactl` 32 times) and re-armed |
| Latency (satellite's line, from the wake word) | STT end 3775 ms, intent end 3894 ms, TTS stream start 3894 ms, `pacat` drained 9275 ms; HA reconnected 3.5 s after a satellite restart |
| `assist_satellite` states | `idle` → `listening` (wake word) → `processing` (STT end) → `responding` (TTS) → `idle` (when HA finished streaming), then the next wake run |
| Push-to-talk (SIGUSR1) with the live mic | **failed: abort cascade (bug, fixed afterwards).** After the satellite aborted the wake run, HA's `RUN_END` for the aborted run arrived after the new start request. The satellite took it for the end of the new run and restarted; each restart aborted HA's newest run again. The pipeline debug shows runs ending 250 to 550 ms after they started, retried with a growing backoff (no hot loop). Fix in `webos-esphome-api`: after an abort, the next start waits for the aborted run's `RUN_END` (at most 1.5 s), and events of the aborted run are ignored. Unit tests reproduce the sequence. **Re-verified, see below** |
| HA error on every reply | `KeyError: <MediaPlayerState.ANNOUNCING: 4>` in `esphome/entry_data.py`: HA 2026.10.4 maps only IDLE/PLAYING/PAUSED/OFF/ON for ESPHome media players. Fix: the satellite reports PLAYING while a reply plays. **Re-verified, see below** |
| `assist_satellite.announce` (message, no pre-announce), first window | **no result:** the service call did not return within 60 s, and the satellite received no announce request. Probably related to the abort cascade or the media player error above (**hypothesis**). Retest after the fixes. The pod-IP media URL question therefore remains open |
| HA log | besides the `KeyError` above: one `APIConnectionError: Not connected` traceback from a wake word run cancelled when the satellite was restarted on purpose |

Cleanup: the temporary pipeline and the ESPHome entry were deleted, the relay was stopped, and the service on the relay port was restarted and listened on all its original ports again. The TV's `/tmp` was cleaned, with no process or sink-input left. HA is back to its original 42 config entries and 2 pipelines (Kiwi keeps `wake_word_id` `alexa`). The permanent TV entry still needs the owner's network rule (HA → TV TCP 6053).

### Re-verification with the fixes (build from commit 3e2af15, second relay window)

Same procedure: temporary relay, temporary pipeline with `conversation.home_assistant`, satellite on c3-a from `/tmp`, `pacat` at gain 0. The service on the relay port was down for about 4 minutes and restored on all its original ports. All results **verified-on-target**.

| Check | Result |
| --- | --- |
| Live-mic push-to-talk, 4 consecutive SIGUSR1 triggers, each while a wake run was armed and streaming | **One clean run per trigger**, no `run failed`, no backoff. The new start went out 2 to 6 ms after the trigger, once HA had ended the aborted wake run. STT ended at HA's timeout after about 15.9 s (silent room); Whisper produced `Gracias por ver el video.` (a hallucination); the built-in agent replied "Lo siento, no he entendido"; HA streamed 1811 ms of TTS, played with 0 non-zero samples; the satellite re-armed the wake word afterwards. Pipeline debug: each run went `stt-start` … `run-end` without an error event |
| Wake run from the file mic (alexa + "¿qué hora es?") | Detected; STT `¿Qué va eso?`; built-in reply streamed (3587 ms, 0 non-zero samples); STT end 3731 ms, TTS start 3745 ms, `pacat` drained 8925 ms after the wake word; re-armed |
| Media player state | now `playing` during a reply and `idle` afterwards; **no `KeyError`** and no other esphome or assist_satellite error in HA's log during or after the replies |
| `assist_satellite.announce` ("Prueba de anuncio", no pre-announce) | The request **reaches the satellite** (it aborts the armed wake run, as designed). The media URL HA sends points at HA's pod-internal address, which the TV cannot reach: the fetch failed after the 5 s connect timeout, and the satellite sent `AnnounceFinished(success=false)` at once and re-armed. HA's service call returned after 5.2 s, and the entity went `responding` → `idle` (media player `playing` → `idle`). So announcements need an HA URL the TV can reach (Internal/External URL, see section 6 of the [integration notes](../../research/ha-esphome-integration-notes.md#6-network-reachability-and-advertised-urls)). No change needed in the satellite: the 5 s bound comes from `FetchConfig::connect_timeout` |
| HA log | only one `APIConnectionError: Not connected` traceback, at the moment the satellite was deliberately restarted during an active wake run (how HA handles a device disconnect mid-run) |

Cleanup as before: pipeline and entry deleted, relay stopped, the port's service restored (same ports), TV `/tmp` clean, token deleted; HA back to 42 entries and 2 pipelines.

## Summary (first session, before the relay)

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

### Relay attempt (2026-11-02, later the same night)

A temporary TCP relay forwarded to the TV's 6053, to let HA's real integration reach the satellite. It ran on a host on the LAN that HA's network was expected to reach on certain ports. It accepted connections from the LAN (the TV connected through it). From HA, the config flow still got `[Errno 111]` on both relay ports tried. A service already listening on that host on another allowed port was reachable from HA (HA connected and got that service's own reply). So pass rules exist, but they did not cover the relay ports as configured. Whether a router rule or a filter on the relay host caused the reset could not be told without administrator access (**unknown**). The relay was stopped, the flow aborted, and the satellite on the TV stopped and removed from `/tmp`. Nothing was left in HA.

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
