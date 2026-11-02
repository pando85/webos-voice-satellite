# P2 session 2: reply latency, wake and STT accuracy, false wakes with content, barge-in, AEC recordings (c3-a)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74, power state "Screen Saver" over the idle PC desktop on HDMI 1, TV speaker, sound mode `aiSoundPlus` (AI Sound Pro), `autoVolume` off; kernel taint 4097 before and after
- **Date:** 2026-11-02, 12:25 to 15:50 local time, unattended (nobody in the room, nobody spoke)
- **Roadmap:** [Phase 2](../../project/roadmap.md#phase-2-sound-tests) deliverables: wake accuracy and false triggers, barge-in, AEC evaluation input; metrics as defined in [P2 corpus and metrics](../../experiments/P2-corpus.md)
- **Starting point:** [P2 session 1](P2-session1-audible-and-echo.md)
- **Home Assistant:** core 2026.10.4, the permanent ESPHome entry `lg-c3-a`; Wyoming openWakeWord 2.1.0 (model `alexa`, default threshold), faster-whisper (large-v3-turbo, `es`), Piper `es_ES-sharvard-medium`
- **Material:** corpus `p2-v1` (`manifest_sha256` `ea7a3963…`), content set `p2-v1` (`manifest.csv` sha256 `52aa3de3…`)
- **Privacy:** synthetic Piper voices and CC or public-domain content only. Recordings, logs and transcripts are under the gitignored `devices/private/c3-a/P2/session2/`; none are committed. No ambient speech was logged.

Labels follow [AGENTS.md](../../../AGENTS.md): **verified-on-target** (measured on c3-a in this session), **observed-on-other-device** (digital or PC-side results without the TV's acoustic path), **documented**, **hypothesis**, **unknown**. "Semi-synthetic" marks results from corpus speech mixed digitally into real TV-mic recordings; they are observed-on-other-device, not measurements of a talker in the room.

## Summary

- **Blocker: the JBL Go 3 rig was not reachable** (Bluetooth page timeout, not found in a scan, several retries over the session; it was most likely switched off). No corpus item was played through the rig, so **no wake or STT rate for a talker in the room was measured**. The tasks that needed the rig were replaced by a digital baseline, a semi-synthetic substitute and code and log observations, all labelled as such. The live runner for the rig is written, unit-tested and ready.
- **Reply start latency (code change 359653b):** `sink.latency_msec` now defaults to 200 ms (0 = PulseAudio's default). With 200 ms, streamed replies started 85 to 332 ms (median 222 ms) after the TTS stream started, n = 6; with the old default 1632 to 1803 ms, n = 4. No gaps or underruns in either. **verified-on-target**
- **Digital baseline** (corpus streamed straight into HA's pipeline, no speaker or mic): wake 58/97 = 60 % [50, 69], strongly voice-dependent (0/13 and 1/13 for the two MLS voices, 13/13 for davefx and claude). Near-misses 3/14 woke it ("Alexandria", "Alex, ¿qué hora es?", "Alexia"), negatives 0/28. WER 15.6 % (11.9 % with the number-format fix), intent 23/40 = 58 % given the wake word. **observed-on-other-device**
- **False wakes with content** (68.4 min of content over HDMI, live satellite, HA's real integration): volume 10: **0 in 1.14 h** (rule-of-three bound 2.6 per hour); volume 15: **2 in 1.125 armed hours = 1.8 per hour [0.2, 6.4]**, one in a song with vocals, one at the start of an instrumental piece. Positive controls before and after the volume-15 run were detected. **verified-on-target**
- **The TV levels its program audio.** HDMI program at -20, -30 and -40 dBFS RMS came out at -47, -49 and -51 dBFS in `SoundEngineCapture` at volume 10 (20 dB in, about 4 dB out), with 2 to 3 dB/s gain recovery after a step down. `autoVolume` is off; the AI Sound Pro mode is the likely cause (**hypothesis**). **verified-on-target**
- Because of that levelling, corpus items played through the TV speaker are **not a valid stand-in for a talker**: the Spanish-voice "alexa" through the TV speaker was not detected even offline, the English voice was.
- **Barge-in:** by design and confirmed in 7 trials, the satellite **does not stream the mic while it plays** a reply or announcement: streaming stops at the announcement request or at the end of speech, and the next wake run starts 70 to 80 ms after the last reply sample. HA therefore cannot detect a wake word during a reply. Semi-synthetic: even if the mic were streamed without AEC, a reference-level "alexa" over the satellite's own reply was detected **0/41 times at volume 10 and 0/41 at volume 15**. At volume 15 the reply saturates 0.23 % of the samples after the +24 dB gain. **verified-on-target** (behaviour) and semi-synthetic (detection)
- **AEC recording set** (mic plus `hw:LGSIC,3`, linked, no xruns): 150 s quiet room, 600 s content at volume 10 and at volume 15, and 7 replies at volume 10 and 15, with a manifest. Offline openWakeWord on the raw mic, with the satellite's front-end emulated: **no detection** in 27 minutes of quiet, content, replies and noise.
- **Semi-synthetic conditions A/B/C** (reference-level corpus speech mixed into the TV-mic recordings, front-end emulated, streamed into HA): quiet room 42/97 = 43 % [34, 53], content at volume 10 11/97 = 11 % [6, 19], content at volume 15 **0/97** [0, 4]. With content at volume 10 Whisper transcribed the content rather than the command (WER about 98 % on the 7 detected commands). Without AEC, a talker at the rig reference level is not usable over TV content at the owner-approved test volumes (**semi-synthetic**, one run each).
- **Level matters:** with the +24 dB front-end, a reference-level talker reaches HA at about -37 dBFS active RMS. In the quiet room, HA detected 42/97 at that level and **55/97 = 57 % [47, 66] with the speech 10 dB louder** (as with a closer talker or more gain), close to the digital 58/97; WER fell from 12.3 % to 8.6 % (semi-synthetic). Offline the same trend: 37/97, 51/97 (+10 dB), 56/97 (+20 dB).

## Deviations from the brief and why

| Brief | What happened | Consequence |
| --- | --- | --- |
| Wake and STT accuracy with the JBL rig at the reference level, conditions A, B and C | The JBL was unreachable all session (`br-connection-page-timeout` on every `bluetoothctl connect`, not seen in a scan). Nobody was in the room to switch it on | **Blocked.** Replaced by the digital baseline, the semi-synthetic conditions and, for the runner, unit tests. Not a measurement of a talker |
| Conversation agent `conversation.home_assistant` | The built-in agent executes what it recognizes. Checked first with the recognition-only endpoint: "apaga todas las luces" would turn off three real, exposed lights and "enciende la luz del salón" one. With nobody home, no live run used a pipeline that could act. Live runs used a temporary pipeline with the built-in agent and conversation language `ru`, which matched none of 9 Spanish and English command phrases (checked). Intents were scored with `conversation/agent/homeassistant/debug` (recognition only, `es`) on the transcripts | No device was switched. Intent results are recognition results; the effect of a command was not checked |
| Corpus played item by item, live, waiting for re-arm | Not possible without the rig. A substitute through the TV's own speaker was tried and **rejected** (see [1.2](#12-corpus-through-the-tv-speaker-is-not-a-talker-stand-in)) | The runner (`p2_runner.py`) is ready but only unit-tested |
| 30 min of the sampler at volume 15 (if time allows) | The whole 68.4 min content set was played at volume 15, with positive controls before and after | More exposure, and the long wake run is shown to be live |
| AEC set: ≥ 40 rig positives over content | Blocked (rig). Content-only and reply recordings were made | No near-end speech in the AEC set |
| Barge-in with the rig speaking during a reply, 5 trials | 7 trials (5 at volume 10, 2 at volume 15) without near-end speech; detection over the reply was tested semi-synthetically | Behaviour verified; detection is semi-synthetic |

## TV state and restore point

Read at the start and verified at the end, all through luna (`ssh -tt`): master volume 6, not muted, scenario `mastervolume_tv_speaker`; power state `Screen Saver`; foreground `com.webos.app.hdmi1`; sound output `tv_speaker`; LG hands-free off (`settingStatus` and `activationState` false for every provider); all capture PCMs `closed`; kernel taint 4097. PC: the HDMI sink that feeds the TV at 22 % (`14492`), unmuted, default sink. The JBL was never connected, so its volume was not touched. Volume was only set with `com.webos.audio/setVolume` and read back each time (6, 10, 15). Mute and input were never changed.

## Setup

- **Binary:** `webos-satellite` with the code of commit 359653b (the latency change below), static armv7 musl, built in a clean worktree because another agent was editing other crates in the shared tree; sha256 checked before and after the copy. Installed with `packaging/webos/install.sh` under `/tmp/webos-satellite`, removed with `uninstall.sh`.
- **Configurations** (private): push-to-talk with a file mic (the P1 TV recording of "¿qué hora es?", looped) and `pacat` at gain 1.0, with `latency_msec` 200 or 0; wake mode with the live ALSA mic and gain 0 (false-wake runs); wake mode with the file mic and gain 1.0 (barge-in trials, so the ALSA mic stayed free for the recorder).
- **Pipelines** (temporary, deleted at the end): `p2s2-es` (es everywhere, built-in agent `es`; used only for the fixed "¿qué hora es?" file input of the latency runs and barge-in trials, and for the digital and semi-synthetic runs, which stop after STT) and `p2s2-live` (built-in agent with conversation language `ru`, see above; used for every live-mic run). Both used openWakeWord `alexa`, faster-whisper and Piper.
- **Tools** (committed, see [the audio rig README](../../../tools/audio-rig/README.md#phase-2-accuracy-and-false-wake-runs-through-home-assistant)): `ha_ws.py`, `p2_digital.py`, `p2_runner.py`, `ha_assist_watch.py`, `content_play.py`, `p2_score.py`, `oww_offline.py`, with unit tests. Captures used `tv_dualcap.py` with `hw:LGSIC,3` as the reference.
- **Clocks:** TV minus PC was 137 ms (one ssh round trip of 140 ms, so ±70 ms). False wakes were timed with the PC receive time of HA's state events; captures with the TV's monotonic clock plus the measured realtime offset.

## 0. Reply start latency

`[sink] latency_msec` (default 200, 0 = PulseAudio's default buffering, at most 10000, strictly validated) is passed to `pacat --latency-msec` for streamed replies and for decoded announcements (commit 359653b, with a unit test). Measured on the TV at volume 10 with push-to-talk runs through HA's real integration, the reply captured on `SoundEngineCapture`. "Start" is the first reply sample in the tap minus the satellite's `TTS stream start` log time; "span" is first to last reply sample.

| `latency_msec` | Runs | Start after `TTS stream start` | Span minus the reply's audio length | Longest zero run inside the reply | Satellite's `playback_end − tts_start − reply_audio` |
| --- | --- | --- | --- | --- | --- |
| 200 (new default) | 6 | 85, 188, 211, 233, 264, 332 ms (median 222) | 30 to 33 ms | 1 to 3 ms | 46 to 291 ms |
| 0 (PulseAudio default) | 4 | 1632, 1639, 1793, 1803 ms | 31 to 36 ms | 1 to 4 ms | 1592 to 1762 ms |

- The span equals the audio length plus about 31 ms (probably filter tails) in every run, and no reply has a gap longer than 4 ms, so there were **no underruns** with 200 ms, at the TV's usual load average of about 13. **verified-on-target**, n = 6 and n = 4.
- Announcements (decoded URL media through `PacatSink`) with 200 ms played completely in all 5 announcement trials of section 3 (11.9 s each). Their start was 0.52 to 0.55 s after the request reached the satellite, 1.22 s for the first one (fetch and decode included).
- Session 1's "about 0.1 s" (n = 1) is refined to 85 to 332 ms.

## 1. Wake and STT accuracy

### 1.1 Digital baseline

`p2_digital.py` streamed every corpus item (16 kHz mono, 2 s silence before, 3 s after, real-time pace) into HA's pipeline `p2s2-es` from `wake_word` to `stt`. Detections are scored with the P2-corpus window (wake start − 0.2 s to wake end + 1.5 s). The intent of each transcript, and of the reference text, comes from the built-in agent's recognition-only endpoint. **observed-on-other-device** (no speaker, no mic), one run.

| Group | Wake detected (95 % Wilson) |
| --- | --- |
| All positives | 58/97 = 60 % [50, 69] |
| "alexa" alone | 18/27 = 67 % [48, 81] |
| "alexa" + command | 40/70 = 57 % [45, 68] |
| English voices (lessac, ryan) | 6/6 |
| Spanish voices | 52/91 = 57 % [47, 67] |

| Voice | Alone | With command | All |
| --- | --- | --- | --- |
| `es_ES-davefx-medium` | 3/3 | 10/10 | 13/13 = 100 % [77, 100] |
| `es_MX-claude-high` | 3/3 | 10/10 | 13/13 = 100 % [77, 100] |
| `es_MX-ald-medium` | 2/3 | 9/10 | 11/13 = 85 % [58, 96] |
| `es_ES-sharvard-medium` M | 2/3 | 5/10 | 7/13 = 54 % [29, 77] |
| `es_ES-sharvard-medium` F | 2/3 | 5/10 | 7/13 = 54 % [29, 77] |
| `es_ES-mls_9972-low` | 0/3 | 1/10 | 1/13 = 8 % [1, 33] |
| `es_ES-mls_10246-low` | 0/3 | 0/10 | 0/13 = 0 % [0, 23] |
| `en_US-lessac-medium`, `en_US-ryan-medium` | 3/3 each | – | 6/6 |

- **Voice dominates.** With no acoustic path at all, two of the seven Spanish voices almost never trigger `alexa`, two always do. Pooled rates therefore depend on the voice mix more than on the condition. Real voices are needed before any rate is taken as a user-facing number.
- **False wakes:** negatives 0/28 items; near-misses **3/14**: "Alexandria", "Alex, ¿qué hora es?" and "Alexia no ha llegado todavía" (davefx and claude voices). No repeats.
- **Detection latency** (audio timeline, detection minus the end of "alexa"): median 0.02 s, 90th percentile 0.14 s.
- **WER** on the 40 detected commands, on the audio HA streamed to STT after the detection: **34/218 = 15.6 %** (P2-corpus v1 normalization); 25/40 items word-perfect; 13 numerals spelled out. Eight of the errors are number formats: Whisper writes "21 grados" for "veintiún grados" (v1 counts "veintiuno" vs "veintiun" as a substitution) and "50%" for "cincuenta por ciento" (two deletions). With those two fixes ("v1.1", reported separately, not a change of the definition) WER is **26/218 = 11.9 %**, 31/40 word-perfect. The rest are real errors, mostly `es_MX-ald` ("Enciéndelo el rostro del sabrón", "¡Gracias por ver el video!" for "¿qué hora es?") and one `mls_9972` item. One `claude` command was cut to "Apaga." (end of speech detected early, **hypothesis**).
- **Intent** (mapping `INTENT_MAP` in `p2_score.py`, written before the run): **23/40 = 58 % [42, 71]** given the wake word, 23/70 = 33 % [23, 44] end to end. The reference texts themselves reach only 28/40 = 70 %: in this HA, "cocina" and "dormitorio" are not areas (parsed as names), "baja la luz del salón al cincuenta por ciento" becomes `HassSetPosition` (cover), and "¿cuánto falta para que termine el temporizador?" and "¿va a llover mañana?" match no intent. STT changed the intent outcome on 5 of 40 items.

### 1.2 Corpus through the TV speaker is not a talker stand-in

Before the false-wake runs, corpus items were played over HDMI as positive controls. Two observations made this route unusable as a substitute for the rig:

- **Levelling.** The same item at -3 dB and -13 dB (digital) reached `SoundEngineCapture` at -53.2 and -54.3 dBFS active RMS and the mic at -51.4 and -51.5 dBFS. A pink-noise check confirmed it: -20, -30 and -40 dBFS RMS in gave -47.2, -49.0 and -51.2 dBFS in the tap (mic -49.3, -50.8, -52.2 dBFS). In a continuous signal stepping from -20 to -40 dBFS the tap dropped by about 14 dB, then gained 2 to 3 dB per second back (to -53 dBFS after 6 s); stepping back up overshot by 5 dB for about 0.5 s. The TV's settings read `soundMode: aiSoundPlus` and `autoVolume: off`. **verified-on-target** for the behaviour (volume 10, HDMI); the AI Sound Pro mode as the cause is a **hypothesis**. Session 1's "level-dependent dynamics" observation is consistent with this.
- **Detection.** Captured on the mic with the satellite stopped and checked offline (python openWakeWord with the satellite's front-end emulated), the English `lessac` "alexa" was detected at both levels, but `claude` and `davefx` were not, at either level, although both are detected 13/13 digitally. Live, a `claude` item was not detected by HA either (twice), while `lessac` was (2/2). **verified-on-target** (n = 6 offline, 4 live).

So speech through the TV speaker is levelled, processed and spatially identical to the content; it says little about a person in the room. Proxy runs were dropped. One consequence: content at any digital level reaches the mic at roughly the same level at a given master volume. The master volume, not the content's loudness, sets the echo level.

### 1.3 Semi-synthetic conditions (no talker in the room)

To still compare a quiet room with content at volume 10 and 15, each corpus item was mixed digitally, at the rig reference level, into real TV-mic recordings from this session. The sum was passed through the satellite's front-end emulation (ch0, 120 Hz high-pass, +24 dB, S16 saturation) and streamed into HA exactly like the digital baseline (`p2_digital.py --bed`). The reference level at the mic comes from D3: the rig path's digital-to-mic gain was -27.0 dB (+12 dB run; -27.4 dB in the +6 dB run). So a corpus item (-22 dBFS active RMS, -34 dBFS at the reference gain) lands at about **-61 dBFS active RMS** at the mic, about -37 dBFS after the front-end. The beds: the quiet room (150 s, -69 dBFS floor), the 10-minute sampler at volume 10 (mic -53 dBFS RMS overall) and at volume 15 (-45 dBFS), and the long announcements at volume 10 and 15. Each item gets a deterministic bed segment. **observed-on-other-device** (semi-synthetic): clean synthetic speech without room, distance or rig colouring, added to real echo.

| Condition (bed) | Wake detected, all positives | Alone / with command | Near-misses, negatives | WER v1 (v1.1), on detected commands | Intent given wake / end to end |
| --- | --- | --- | --- | --- | --- |
| Digital baseline (1.1, no bed, full level) | 58/97 = 60 % [50, 69] | 18/27, 40/70 | 3/14, 0/28 | 34/218 = 15.6 % (11.9 %), n = 40 | 23/40 = 58 % [42, 71] / 23/70 = 33 % |
| A: quiet room | 42/97 = 43 % [34, 53] | 14/27, 28/70 | 0/14, 0/28 | 18/146 = 12.3 % (8.9 %), n = 28 | 17/28 = 61 % [42, 76] / 17/70 = 24 % [16, 35] |
| A, speech +10 dB | 55/97 = 57 % [47, 66] | not split | not run | 18/209 = 8.6 % (4.8 %), n = 38 | 23/38 = 61 % [45, 74] / 23/70 = 33 % [23, 44] |
| B: content at volume 10 | 11/97 = 11 % [6, 19] | 4/27, 7/70 | not run | 41/42 = 98 % (95 %), n = 7 | 1/7 / 1/70 = 1 % [0, 8] |
| C: content at volume 15 | **0/97 = 0 % [0, 4]** | 0/27, 0/70 | not run | – | 0/70 |
| Own reply (announcement) at volume 10, stratified | **0/41 = 0 % [0, 9]** | 0/20, 0/21 | not run | – | 0/21 |
| Own reply (announcement) at volume 15, stratified | **0/41 = 0 % [0, 9]** | 0/20, 0/21 | not run | – | 0/21 |

- Per voice in A: the English voices 3/3 each, `ald` 11/13, `sharvard` 13/26, `davefx` 6/13, `claude` 6/13, both MLS voices 0/13. Compared with the digital baseline, the level and the room noise cost most for `davefx` and `claude` (13/13 → 6/13 each).
- In B, Whisper mostly transcribed the **content** instead of the command ("Ella se salió del camino y se fue a cortar flores.", English lyrics), so WER is close to 100 % on the 7 detected commands: at -8 dB SNR the reference-level talker loses to volume-10 content even after a correct wake.
- Detection latency (audio timeline, after the end of "alexa"): A median 0.07 s, 90th percentile 0.15 s; B 0.08 and 0.17 s.

- **Content SNR at the mic** (talker at -61 dBFS active RMS): about +8 dB above the floor in a quiet room; about -8 dB against the volume-10 content (-53 dBFS RMS); about -16 dB against volume 15 (-45 dBFS); about +3 dB against session 1's volume-6 content (-64 dBFS).
- **Offline cross-check** (python openWakeWord `alexa_v0.1`, ONNX, same mixtures, 97 positives each): quiet 37/97, content volume 10 12/97, volume 15 1/97, own reply at volume 10 0/97. The offline detector tracks HA's server closely enough to use it for the later offline AEC evaluation, with the caveat that it is not the same server (Wyoming openWakeWord 2.1.0).
- **Front-end level:** offline on the quiet bed, louder speech raised detection from 37/97 (reference level) to 51/97 (+10 dB) and 56/97 (+20 dB); the digital baseline at full level was 58/97. Through HA, +10 dB gave **55/97 = 57 % [47, 66]** (per voice: davefx and claude 12/13 each, ald 11/13, sharvard 14/26, English 3/3 each, MLS 0/13 each), WER 18/209 = 8.6 % (v1.1 4.8 %, 27/38 word-perfect), intent 23/38 given the wake word. The satellite's fixed +24 dB is therefore on the low side for a talker at the rig's level in a quiet room, while it already saturates on its own reply at volume 15 (section 3). That is an argument for gain after AEC and against simply raising the fixed gain (**hypothesis**, to be tested with real voices).

## 2. False wakes per hour with TV content

The 13 content clips (68.4 min, -23 LUFS, `content_play.py`) played from the PC over HDMI (PC sink at 0 dB), once at volume 10 and once at volume 15. The satellite ran live in wake mode (ALSA mic, replies at gain 0) on `p2s2-live`, through HA's real integration. `ha_assist_watch.py` logged every `idle` → `listening` change of the satellite entity (PC time) and the pipeline run behind it. A detection is counted for the clip that was playing; "armed" time subtracts the time from each detection until the wake word was listening again. **verified-on-target**, one run per volume.

| Category | Clips | Volume 10: wakes / min played | Volume 15: wakes / armed min | Volume 15 rate per hour [95 %] |
| --- | --- | --- | --- | --- |
| Spanish speech (LibriVox) | 2 | 0 / 18.0 (≤ 10.0/h) | 0 / 17.8 | 0 [0, 10.1] |
| English speech and film dialogue | 2 | 0 / 13.1 (≤ 13.7/h) | 0 / 13.1 | 0 [0, 13.7] |
| Music with vocals | 4 | 0 / 16.4 (≤ 11.0/h) | 1 / 16.0 | 3.8 [0.1, 21.0] |
| Music without vocals | 4 | 0 / 15.9 (≤ 11.3/h) | 1 / 15.6 | 3.8 [0.1, 21.4] |
| Action scene | 1 | 0 / 5.0 (≤ 36/h) | 0 / 5.0 | 0 [0, 36] |
| **Total** | 13 | **0 / 68.4 min: 0 per hour, ≤ 2.6/h** | **2 / 67.5 min** | **1.8 [0.2, 6.4]** |

- The two false wakes at volume 15: 21 s into "Cherubs" (vocals; STT: song lyrics, "The salty breeze was in our eyes…") and 1.1 s into "Canon in D", right after a song with vocals ended, so it may belong to the end of that song (STT: "¡Gracias!"). Both ended in `no_intent_match` (no actuation). The detection is reported about 0.3 s after the end of the word, so the second one is ambiguous between the two categories.
- **Liveness.** At volume 15, an English-voice "alexa" over the TV speaker (silence, no content) was detected before and after the run, and the two false wakes came from long-running wake runs, so the wake stage was live throughout. At volume 10 the same wake run (one `wake_word` run from start to end in HA's debug data; the TV sent about 280 kbit/s when checked at the end) got **no in-run positive control with a detectable voice**: the controls played after it used the `claude` voice, which the TV path does not let through (1.2). A fresh run detected the English control right afterwards. The volume-10 zero is therefore supported but not proven by a control in the same run.
- Volume 10 versus 15: 0/1.14 h and 2/1.125 h. The intervals overlap; a single run each is **inconclusive** for a difference. Pooled: 2 in 2.27 h, 0.9 per hour [0.1, 3.2].
- At volume 10 content reaches the mic at -53 dBFS RMS overall (category means -57 to -50 dBFS), at volume 15 at -45 dBFS (-49 to -41).

## 3. Barge-in

Seven trials with the satellite in wake mode (file mic, so the ALSA mic stayed free for the recorder; the voice state machine is the same as with the ALSA mic), replies audible at gain 1.0, pipeline `p2s2-es`: three 11.9 s announcements and two push-to-talk streamed replies (2.1 and 2.2 s, "¿qué hora es?") at volume 10, two announcements at volume 15. Mic and `SoundEngineCapture` were captured linked throughout. No near-end speech (no rig).

**Behaviour** (satellite log times against the reply in the tap; HA's pipeline runs). **verified-on-target**, 7 of 7 trials:

| Event | Announcement | Streamed reply |
| --- | --- | --- |
| Mic streaming stops | when the announcement request arrives (HA's wake run ends at that moment), 0.5 to 1.2 s before the first reply sample | at the end of speech (STT VAD end), 1.1 to 1.2 s before the reply starts |
| During the reply | nothing is streamed; no pipeline run is active in HA | nothing is streamed; HA's run has ended (`run-end` follows `tts-end`) |
| Next wake run | 70 to 80 ms after the last reply sample | 70 ms after the last reply sample |

- So **HA cannot detect a wake word during a reply or announcement**: the satellite does not send audio then. This is by design in `crates/esphome-api` (streaming stops at `STT_VAD_END` or on an announcement; the run finishes, and the wake word re-arms, only after playback reports finished).
- A user who says "alexa" while the TV answers is ignored; if they keep talking, the wake word re-arms within 80 ms of the reply's end, so a wake word said right after the reply works.

**What the stream would look like** (mic ch0 during the reply, the +24 dB front-end applied offline):

| Reply | Raw peak / RMS | After +24 dB: RMS, samples at full scale |
| --- | --- | --- |
| Announcement, volume 10 (3 trials) | -29.5 / -45.2 dBFS | -21.4 dBFS, 0 % |
| Streamed reply, volume 10 (2 trials) | -28.0 and -26.6 / -47.1 and -46.6 dBFS | -23.2 and -22.7 dBFS, 0 % |
| Announcement, volume 15 (2 trials) | -20.5 and -19.8 / -36.1 dBFS | -12.3 dBFS, **0.23 to 0.24 %** of samples clipped |

**Would HA detect "alexa" over the reply if it were streamed (no AEC)?** Semi-synthetic, 41 stratified positives (every voice) at the reference level mixed into the announcement segments: **0/41 at volume 10 and 0/41 at volume 15** through HA; offline 0/97 at volume 10. The reply is 16 dB (volume 10) to 25 dB (volume 15) louder at the mic than a reference-level talker.

**Design options** (not implemented, no decision):

1. **Stop the reply on wake (keep streaming during playback).** Requires streaming the mic during playback, which by the result above only helps with echo cancellation, or for a talker much louder than the rig reference. Without AEC it would also feed HA's openWakeWord the TV's own reply all the time.
2. **Lower the capture gain during playback** (for example +24 → +14 dB while the sink plays). Removes the saturation at volume 15 but not the 16 to 25 dB echo-to-talker ratio, so detection would stay near zero. Useful only together with option 3.
3. **AEC with `SoundEngineCapture` as the reference**, the canceller in front of the gain ([AEC front-end design](../../design/aec-frontend.md)). Session 1 measured about 14 dB of linear cancellation in 200 to 4000 Hz with this reference on test signals, and the AEC design reports 15 to 22 dB with the integrated canceller on session 1 recordings; that is the same order as the 16 to 25 dB echo-to-talker ratio, so barge-in at volume 10 is plausible and at volume 15 marginal (**hypothesis**). The AEC recordings below allow an offline estimate before any TV test.
4. **Local wake word on the satellite during playback only** (backlog), still needing 3.

## 4. AEC recording set and offline baseline

Recorded with the satellite stopped or on its file mic, with `tv_dualcap.py` (`hw:LGSIC,0` S24_LE 2 ch 16 kHz linked with `hw:LGSIC,3` S32_LE 2 ch 48 kHz, one trigger, no xruns), under `devices/private/c3-a/P2/session2/{aec,bargein,leveller,ctrl}/` with `aec/manifest.json`. The manifest gives the formats, the capture start in TV time, the content segments in capture time (from the reference onset, cross-checked with the clocks to 0.2 s) and the reply spans. **verified-on-target** recordings; no corpus speech (rig blocked).

| Recording | TV volume | Duration | Content |
| --- | --- | --- | --- |
| `aec/quiet` | (6, TV silent) | 150 s | room only (mic -69 dBFS) |
| `aec/content-v10` | 10 | 600 s | the 10-minute sampler (Spanish and English speech, vocals, instrumental, action), starting at 1.88 s (the last 1.9 s are not captured) |
| `aec/content-v15` | 15 | 600 s | the same |
| `bargein/v10` | 10 | 170 s | 3 announcements (11.9 s) and 2 streamed replies |
| `bargein/v15` | 15 | 60 s | 2 announcements |
| `leveller/v10` | 10 | 60 s | pink-noise bursts and level steps |
| `ctrl/mic-ctrl` | 10 | 32 s | six corpus items through the TV speaker (1.2) |

- **Delay** `SoundEngineCapture` → mic per open: 18.7 ms (content volume 10) and 15.0 ms (volume 15), in session 1's 12.9 to 19.1 ms range, so it changes per open again.
- **Coherence depends on the content's stereo image** (content volume 10, 60 s segments, reference L+R / L / R): Spanish audiobook (mono) 0.93 / 0.93 / 0.93 (coherence-bound ERLE 13.7 dB); film dialogue 0.65 / 0.51 / 0.63; music with vocals 0.81 / 0.72 / 0.78; action scene 0.39 / 0.49 / 0.64 (bound 2.2 / 3.0 / 5.0 dB). Session 1's test signals were dual mono. With stereo content the two speakers (and AI Sound Pro's processing) give the mic two different paths, which one summed reference cannot model: a **two-channel reference** for the canceller looks necessary (**hypothesis**; verified-on-target for the coherence numbers).
- **Offline openWakeWord baseline** (python `openwakeword` 0.6.0, `alexa_v0.1`, ONNX, threshold 0.5, the satellite's front-end emulated, raw mic ch0): no detection in the quiet room (max score 0.09), content at volume 10 (0.06) and 15 (0.14), the replies (0.02) and the pink noise (0.11): 0 false wakes in 27.3 min. Positives: 2/6 control items through the TV speaker (the two `lessac` ones). After the front-end, content at volume 15 saturates 0.01 % of the samples (peak +12.8 dBFS before clipping; volume 10: +5.3 dBFS, no sample clipped after the 120 Hz filter).

## Negative results

- No talker-in-the-room accuracy (rig blocked). Every wake, WER and intent number above is digital or semi-synthetic.
- The TV speaker is not a usable stand-in for a talker (levelling, processing, Spanish voices not detected).
- Two Piper voices (`mls_9972`, `mls_10246`) are almost never detected even digitally; three near-misses wake the word.
- No barge-in is possible with the current design, and none would be without AEC (0/41 semi-synthetic).
- With stereo content a single summed reference reaches poor coherence (as low as 0.39).

## Residual unknowns

- Real wake and STT rates with the rig and with real voices at a normal distance (needs the JBL and the owner); the runner is ready.
- Whether the volume-10 wake run was live for its whole 68 minutes (no in-run positive control; see section 2).
- Whether the levelling comes from AI Sound Pro (setting not changed; reading another sound mode needs the owner) and whether it also acts on `pacat` replies (session 1's ducking result suggests a stage on the mixed output).
- Whether `SoundEngineCapture` delay stays fixed beyond 10 minutes (it did within each 10-minute capture as far as the GCC estimates on 60 s segments agree).
- Wyoming openWakeWord 2.1.0 versus python openWakeWord 0.6.0: close on the semi-synthetic sets, not identical.
- Reply-start latency under heavier CPU load than the TV's normal load average of 13.

## Rollback and cleanup (verified at the end)

- TV: volume 6, not muted, `tv_speaker`, foreground `com.webos.app.hdmi1`, power state `Screen Saver`, hands-free off, all capture PCMs `closed`, no `webos-satellite`, `pacat` or recorder process, no sink-inputs, `/tmp/webos-satellite`, `/tmp/tv_dualcap.py` and the kill-switch file absent, kernel taint 4097. No ASoC/DAPM sysfs or debugfs path was read. Only `/tmp` was written.
- PC: HDMI sink back at `14492` (22 %), unmuted, still the default sink. The JBL was never connected.
- HA: the satellite's pipeline select back to `preferred` (the second select was never changed), both temporary pipelines deleted (HA has Kiwi and Kiwi Test again, Kiwi preferred), the access token revoked and its file deleted. The `lg-c3-a` entry stays, `unavailable` while the satellite is not running. HA's log showed two `APIConnectionError: Not connected` tracebacks, from restarting the satellite during an active wake run to change its configuration (known, see [P1 HA integration](P1-ha-integration.md)).
