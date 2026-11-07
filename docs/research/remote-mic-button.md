# Magic Remote mic button as a push-to-talk trigger for the satellite

- **Date:** 2026-11-07. **Scope:** desk research only: the repository's own findings, official LG and Home Assistant material, and open-source projects for rooted LG TVs. **Nothing was run on a TV when this note was written.** Update 2026-11-07: [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md) Part A (read-only) was then run on c3-b; the results are marked verified-on-target (c3-b) below. No binary was reverse engineered.
- **Owner request:** "Use the mic button from the remote to open a kind of assistant in the TV": the Magic Remote's mic (voice) button starts our Home Assistant assistant (push-to-talk), instead of or alongside LG's own voice assistant.
- **Target:** c3-a (LG C3 42 inch, webOS TV 8.3.0, firmware `03.30.74`) and c3-b (LG C3 55 inch `OLED55C36LC`, webOS TV 8.3.0, firmware `03.30.73`); see [c3-a](../compatibility/c3-a.md) and [c3-b](../compatibility/c3-b.md). The Magic Remote paired with c3-b is an **`LGE MR23`** (verified-on-target, EXP-022 A, 2026-11-07); the one with c3-a is still unrecorded. The original wording follows: the model of the Magic Remote paired with each TV is **not recorded** in this repository. A C3 is sold with an MR23-generation remote (MR23GA, or MR23GN with NFC), but this is a **hypothesis** for our two units ([LCD Compare](https://www.lcd-compare.com/definition-de-lg-magic-remote.htm)).
- **Check on the TV:** [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md). Part A was run read-only on c3-b on 2026-11-07 (owner authorized); Part A on c3-a and Part B (`startStreaming`) are not run.
- **Related:** [D2 audio stack](../discovery/c3-a/D2-audio-stack.md), [D3 step 5a](../discovery/c3-a/D3-mic-identification.md#2-step-5a-magic-remote-mic-button-2026-11-01), [always-on 10.1 and rank 3](../design/always-on.md), [D5 results on c3-b](../discovery/c3-b/D5-results.md), [voice and power screen](../design/voice-and-power-screen.md), [ESPHome API crate](../development/esphome-api.md), [ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md), [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md), [ADR 0008](../decisions/0008-user-initiated-lg-settings.md), backlog [#4 TV integration](../project/backlog.md#4-tv-integration).

Labels follow [AGENTS.md](../../AGENTS.md): **documented** (vendor or upstream documentation or source), **observed-on-other-device** (a report from another TV, named with its model and webOS version), **hypothesis**, **verified-on-target** (seen on c3-a or c3-b, with a link). Our TVs run webOS TV 8.3 (webOS 23). Most of the prior art below comes from **webOS 25** (SDK 10.x) and from other models. It is not evidence for our firmware.

## Summary

| # | Question | Short answer | Label |
| --- | --- | --- | --- |
| 1a | How does the press reach webOS? | The Magic Remote talks to the TV over 2.4 GHz Bluetooth HID. `lginput2` turns its buttons into a uinput event device (`LGE M-RCU - Builtin [0]`). There the mic button is key code **428**: LG calls it `KEY_VOICE`, while Linux's name for 428 is `KEY_VOICEMAIL`. The same press goes to the system log as `NL_BUTTON_CLICK … "button_type":"KEY_VOICE"`. LG's voice pipeline (`voiceinput_hidraw` → `voiceinput` → `voiceconductor`) reads the remote's HID stream on its own path. | **verified-on-target (c3-b, `03.30.73`, EXP-022 A):** the mic tap is `EV_KEY` **428** (press 1, release 0, no repeat) on **`LGE M-RCU - Builtin [0]`** (`event2`), four of four presses, and `lginput2` logs `NL_BUTTON_CLICK {"remote_type":"LGE MR23","button_type":"KEY_VOICE"}` within 1 ms. The remote reaches the TV as `hidraw0` (`LGE MR23`, via uhid), held by `voiceinput` and `lginput2`; there is **no `voiceinput_hidraw` process** on webOS 8.3. c3-a: not run (hypothesis) |
| 1b | Can a root process observe it? | **Yes on c3-b.** A passive `cat` of `event0` to `event8` (no grab, no write, nothing stored on the TV) saw every press of the mic button, Up, Back and the navigation keys on `event2`. A filtered read of the system log gives the same press as a second signal. | verified-on-target (c3-b, EXP-022 A); c3-a not run |
| 1c | Can it consume it so LG's UI does not open? | **LG's UI opens with the press:** with "Hi LG" off, each tap launched `com.webos.app.voice` (log, and the owner saw the assistant appear, say nothing and disappear), with LG's own parameters `recognitionSource {input voiceinput, key mrcu, type remote, keyType thinQtv}` (verified-on-target, c3-b). Not cleanly consumable with what is known today. Grabbing the event device hides the key from the compositor, but LG's voice pipeline reads the HID stream directly. CleanYourTV, which hooks the key inside `lginput2`, still has to pause `voiceconductor` (SIGSTOP) to keep LG's screen and cloud recognition away. | observed-on-other-device; whether a grab alone stops LG's UI on webOS 8.3 is **unknown** (not tested; the voice route here reads `hidraw0` through `voiceinput`, which a grab of the evdev node would not touch, inference) |
| 2 | Can we use the remote's own microphone? | Probably yes, through LG's service rather than raw HID. `luna://com.webos.service.voiceinput/startStreaming` with `{"deviceType":"remote","keyType":"thinQtv","subscribe":true}` answers with a Unix socket path. The socket carries 16 kHz mono 16-bit PCM while the button is held, and the call is refused with `errorCode` `10000` until then. On c3-a `voiceinput` declares `startStreaming`. The remote's voice audio is not an ALSA capture. | observed-on-other-device (OLED65G26LA and 65UR91006LB, webOS 25); declaration and "not ALSA" verified-on-target; **verified-on-target on c3-a (EXP-022 B, 2026-11-07): works on 8.3.** `{"deviceType":"remote","subscribe":true}` replied `{"subscribed":true,"returnValue":true,"socketPath":"/tmp/voiceinput_remote1.soc"}` 29 ms after the press; the socket gave 31.6 kB/s (16 kHz mono S16LE fits) with speech 24 dB over the floor; **the stream does not stop at the button release**; one `2003 audio data is already capturing` refusal; LG's UI said "audio processing is unstable" (owner observation, cause unknown, possibly contention). EXP-022 A on c3-b: the schema has `deviceType` (`remote`) but **no `keyType`** property, while LG's own launch uses `keyType thinQtv`; `getDevices` is empty; Part B not run |
| 3 | UX | Hold-to-talk fits the remote: press starts a run at STT, release ends the audio. The daemon already has a push-to-talk run (HA starts at STT when `USE_WAKE_WORD` is not set). A transparent `topui` window can show "listening / thinking / answer" over any app. | protocol documented and push-to-talk verified-on-target; overlay observed-on-other-device |
| 4 | Risks | LG's assistant reacting to the same press (UX conflict, and audio to LG's cloud), a grab locking the owner out of the remote, changing LG voice services (against current rules, needs an ADR), webOS-version-specific socket paths and parameters, LG updates (webOS upgrades are offered for the C3). | see section 5 |

**Recommendation (section 6):** run the read-only [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md) Part A first. It needs about 15 minutes of the owner's time and changes nothing. Do not build anything that consumes the key or touches LG's voice services before an ADR settles how we coexist with LG's assistant.

## 1. How the mic-button press reaches webOS

### 1.1 Facts already in this repository

| Fact | Label | Source |
| --- | --- | --- |
| Event devices on c3-a: `LGE RCU` (event1), `LGE M-RCU - Builtin [0..2]`, `LGE TONE+`, `LGE Simple Premium`, `Smart Remote RCU Input` (event9), `LGE Network Input` (event10), `IoT keypad`, `Bluetooth-audio-source`, `CHECK INPUT`. `surface-manager` is the only process holding input event nodes. `/dev/uinput` exists (root only) | verified-on-target (c3-a, read-only, 2026-11-03) | [always-on 10.1](../design/always-on.md) |
| A passive `cat` of event nodes (no grab, no write) recorded the IR POWER key as `KEY_POWER` on `LGE RCU` and the API key on `Smart Remote RCU Input`. `od` buffered and recorded nothing; raw `cat` worked. Records are 16 bytes on this 32-bit userland | verified-on-target (c3-b, `03.30.73`) | [D5 results](../discovery/c3-b/D5-results.md) |
| Which node the **Magic Remote** uses on c3-b was not established in D5 (it assumed `event9`, shared with the API key). **EXP-022 A: `event2` (`LGE M-RCU - Builtin [0]`).** On 2026-11-07 c3-b had **no** `Smart Remote RCU Input` or `LGE Network Input` node and `event9` to `event31` were stale files; the D5 `event9` finding (the API key) is not contradicted, the node simply did not exist at that time | verified-on-target (c3-b, EXP-022 A, 2026-11-07) | [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md) |
| During two mic-button presses on c3-a (LG hands-free off) **no ALSA capture PCM opened** on any card. Only `card0 pcm6p` playback (LG earcon and answer) ran | verified-on-target (c3-a, `03.30.74`) | [D3 step 5a](../discovery/c3-a/D3-mic-identification.md#2-step-5a-magic-remote-mic-button-2026-11-01) |
| Services declared on c3-a: `com.webos.service.voiceinput` (private group: `getDevices`, `startStreaming`, `startStreamingWithTrigger`, `stopStreaming`, adapter `setInputEvent`, …) and `com.webos.service.voiceconductor` (public: `recordVoice`, `getVoiceUiEvents`, …; private: `setInputEvent`, `getVoiceKey`, …). `voiceinput` links `libsbc`. Its `preDefinedDeviceList.json` names a `testRemote` (hidraw). Its `watchFolder.json` watches `/tmp/voiceinput_disable_set_input_event`. `audiod` lists `remote.monitor` as a sound input | verified-on-target (declarations and static files; behaviour unknown) | [D2](../discovery/c3-a/D2-audio-stack.md#lg-audio-and-voice-service-surface-luna-service2) |
| Root `luna-send` reaches `voiceinput` private methods (`setting/getActivation`, `setActivation`) | verified-on-target (c3-a) | [EXP-011](../experiments/EXP-011-lg-settings-toggle.md) |
| Running LG voice daemons on c3-a: `voiceconductor`, `voiceinput`, `lg.thinqai.adapter`, `amazon-alexa-adapter`. A `voiceinput_hidraw` process was **not** recorded (D0 to D2 did not look for it). On c3-b (EXP-022 A) it does not exist | verified-on-target (list); `voiceinput_hidraw` absent on c3-b, unknown on c3-a | [D0-D1](../discovery/c3-a/D0-D1-runtime.md) |

### 1.2 Prior art on other TVs

| Claim | Label | Source |
| --- | --- | --- |
| `lginput2` creates the Magic Remote's virtual input device `LGE M-RCU - Builtin [0]` through uinput. A daemon can take it with `EVIOCGRAB` ("so the TV no longer sees the raw events") and re-emit into the sibling `LGE M-RCU - Builtin [2]`, which the compositor already reads. It also has a `--dry-run` mode that "logs events without grabbing anything". Its key table, "Linux input event codes as emitted by lginput2 / micomservice", lists **428 `AI / Voice`**, 1086 Alexa and 1117 Google Assistant. Power, navigation, OK, Back, Home, Settings, volume and channel are deliberately not remappable. Tested on a C5 (webOS 10.3) with an MR25GA | observed-on-other-device | [LG Input Mapper](https://github.com/fivefold3/lginputmapper) (`README.md`, `shared/keys.js`, `daemon/lginputmapperd.c`, commit `fca03ae`) |
| On a TV with an MR23 remote on webOS 10.3, the hook in `lginput2` reported code **428** for the button that LG's system log names **`KEY_VOICE`**. The microphone key reached the hook, as did Netflix, Prime Video, Alexa and the colour keys. On a webOS 5.6 TV (55UN6950ZUA) no button reached the hook at all | observed-on-other-device | [CleanYourTV](https://github.com/lasharor/cleanyourtv-WebOS) `helper/keynames.js`, `docs/key-remap.md` (branch `dev`, commit `e524ac8`) |
| The hook is injected into `lginput2` with ezinject (`libinputhookpp`). Its keybinds can `exec` a script per key. The original LG Input Hook "stopped working on webOS 24/25", according to LG Input Mapper | observed-on-other-device | CleanYourTV `helper/inputhook.js`; [LG Input Hook](https://github.com/Simon34545/lginputhook) |
| "Magic Remote mic pipeline (`voiceinput_hidraw` → `voiceinput` → `voiceconductor`)". The mic audio is "a raw HID stream" on `/dev/hidraw0` (`HID_NAME=LGE MR25GA`), read only by `voiceinput_hidraw`, with no ALSA device involved. After those three processes were stopped, "the mic button press still registers as `NL_BUTTON_CLICK {"button_type":"KEY_VOICE"}`" from `lginput2` in `/tmp/var/log/messages`. On that TV, **voice transcripts were written in plain text to `/tmp/var/log/messages`** (finding F5) | observed-on-other-device (webOS 10.3.1) | [own-your-glass](https://github.com/JulioFerrero/own-your-glass) `docs/FINDINGS.md` F5, F37 (commit `f6fd6f2`) |
| LG manual for the MR22GN Magic Remote: "Speak while pressing and holding the button to use the voice recognition"; "Network connection is required to use the voice recognition function". Radio: 2.400 to 2.4835 GHz, 40 channels, 7 dBm | documented (MR22GN) | [LG MR22 manual (PDF)](https://lg.com/content/dam/channel/wcms/mx/download/mr22_manual.pdf) |
| MR23GA: 2.400 to 2.4835 GHz, 8 dBm EIRP, 40 channels | documented (MR23GA) | [manualslib MR23GA](https://www.manualslib.com/manual/3399539/Lg-Mr23ga.html) |

### 1.3 Answer to question 1

- **Path (hypothesis for c3-a and c3-b, built from the sources above).** The remote sends HID reports over Bluetooth LE. The 40-channel 2.4 GHz plan matches Bluetooth LE; own-your-glass calls the link "Bluetooth HID". From there the press takes two routes:
  - **Key route:** `lginput2` sends the press to an `LGE M-RCU - Builtin [n]` uinput device as `EV_KEY` code 428 (`KEY_VOICE` in LG's naming). `surface-manager` reads it, and `lginput2` writes `NL_BUTTON_CLICK` to the system log.
  - **Voice route:** on c3-b there is no `voiceinput_hidraw` process (verified-on-target, EXP-022 A); `voiceinput` (and `lginput2`) hold `/dev/hidraw0`, and on a short tap `voiceconductor` launches `com.webos.app.voice` with `keyType thinQtv` (verified-on-target). On this route LG starts listening and shows its UI; On this route LG starts listening, shows its UI and sends the audio to its cloud recognizer.
  - Both routes fire on a tap (the log shows `lginput2` first, then `voiceconductor` via `setInputEvent` from `com.webos.starfish.keyfilters`, then the voice app); which one a grab would stop is **unknown**. D2 found `setInputEvent` methods on both `voiceconductor` and the `voiceinput` adapter, which points to a separate event path between them (**hypothesis**).
- **Observe:** a passive reader of the event node (no `EVIOCGRAB`) receives every event that `surface-manager` receives. This is Linux evdev behaviour: every open handle gets the events unless one handle grabs the device. It does not disturb the remote. The method is verified-on-target (c3-b D5). That the mic button appears there as 428 is **verified-on-target on c3-b** (node `[0]`, EXP-022 A; also observed on webOS 10.3 with MR23 and MR25 remotes). A second read-only signal is the `NL_BUTTON_CLICK` line in the system log: on c3-b it is `{"remote_type":"LGE MR23","button_type":"KEY_VOICE"}`, one per press (verified-on-target). On webOS 10.3 on webOS 10.3 the same log may hold LG's voice transcripts, so a reader must filter and never store the rest.
- **Consume:** three known techniques, none proven on our firmware:
  1. `EVIOCGRAB` on `LGE M-RCU - Builtin [0]`, re-emitting every other key into `[2]` (LG Input Mapper). This hides the press from `surface-manager`. It is **unlikely to stop LG's voice route** if that route reads hidraw directly (inference from own-your-glass F37).
  2. An injected hook in `lginput2` (LG Input Hook, CleanYourTV). This is process injection into an LG service, which our rules exclude, and it is reported broken on webOS 24/25.
  3. Pausing `voiceconductor` with SIGSTOP (CleanYourTV's **Switch off LG's voice assistant**: "LG's own screen and its cloud recognition do not start on the same button"; a reboot or SIGCONT undoes it). CleanYourTV needs this **in addition to** its `lginput2` hook, which suggests the key hook alone does not stop LG's assistant (inference). This changes a native LG voice service, which [AGENTS.md](../../AGENTS.md) and the [discovery rules](../discovery/README.md#safety-rules-learned) currently forbid. It would need an owner decision and an ADR.
  4. Not found in any source: a supported LG setting that hands the mic button to a third party. The `voiceinput` watch file `/tmp/voiceinput_disable_set_input_event` and `voiceconductor`'s `setInputEvent`/`getVoiceKey` are candidates whose semantics are unknown (see section 7, REA).

## 2. The remote's own microphone

### 2.1 Prior art: LG's `voiceinput` remote stream (CleanYourTV)

CleanYourTV's voice feature captures "16 kHz mono from the microphone stream of LG's `voiceinput` service" (`docs/voice.md`). The code (`helper/voice.js`, `capture()`, branch `dev` at `e524ac8`) does the following (**observed-on-other-device**):

- It runs `luna-send -i luna://com.webos.service.voiceinput/startStreaming '{"deviceType":"remote","keyType":"thinQtv","subscribe":true}'`.
- Reply on a 65UR91006LB (webOS 25, SDK 10.3.2, firmware 33.31.75, 2023 board), quoted in [PR 184](https://github.com/lasharor/cleanyourtv-WebOS/pull/184): `{"subscribed":true,"returnValue":true,"id":"remote1","state":"voicestart","socketPath":"/var/voiceinput/voiceinput_remote1.soc"}`. On the maintainer's OLED65G26LA (webOS 25) the socket is `/tmp/voiceinput_remote1.soc`.
- **Gate:** "the service answers `{"errorCode":"10000","errorText":"unknown error","returnValue":false}` until the mic button is held, and starts streaming as soon as it is". CleanYourTV retries every 0.7 s for 8 s.
- It connects to the Unix socket and reads raw little-endian 16-bit samples at 16 kHz (`readInt16LE`, `RATE = 16000`), segmenting with a level-based endpoint. Moonshine recognised the result on both TVs, so the format reading is sound on those TVs.
- It runs alongside `voiceconductor` paused with SIGSTOP (section 1.3). Whether the stream also works with `voiceconductor` running, and what LG does with the same audio meanwhile, is not documented there.

### 2.2 Other routes

| Route | Assessment | Label |
| --- | --- | --- |
| ALSA capture | Not used for the remote's voice | verified-on-target (c3-a D3 5a) |
| `audiod` `remote.monitor` sound input | Listed, inactive. It may be how LG plays or monitors remote audio; no source describes it | hypothesis |
| Raw `/dev/hidraw*` | hidraw delivers each report to every open handle, so a second reader would not starve `voiceinput_hidraw`. But the audio is in a vendor HID report format, possibly SBC-coded (`voiceinput` links `libsbc`, D2), and decoding it needs reverse engineering. The HID device also carries the buttons; own-your-glass deliberately leaves it untouched | hypothesis; not recommended |
| `voiceconductor/recordVoice` (public) | Declared on c3-a; what it records (remote or far-field) and its parameters are unknown | hypothesis |

### 2.3 Answer to question 2

- **Transport:** 2.4 GHz with 40 channels, which is Bluetooth LE's channel plan (documented for MR22GN and MR23GA). The HID link to `voiceinput_hidraw` is observed-on-other-device (MR25GA). Bluetooth LE for our units' remote is a **hypothesis** with good support.
- **Format and exposure:** through LG's service, a Unix socket with 16 kHz mono S16LE PCM, gated on the button being held (observed-on-other-device, webOS 25). Not an ALSA device (verified-on-target). On webOS 8.3: `startStreaming` is declared (verified-on-target), but the accepted `deviceType`/`keyType` values, the socket path, the gate and the format are **unknown**. EXP-022 Part A reads the method's schema file, and Part B tests the stream with consent.
- **Value:** the remote is held near the mouth, so speech-to-noise should be far better than the TV's far-field mic. On c3-a speech was only about 3 to 5 dB above the floor at the D3 reference level, and wake accuracy depends strongly on the voice ([D3](../discovery/c3-a/D3-mic-identification.md), [P2 session 2](../discovery/c3-a/P2-session2-wake-accuracy.md)). TV-content echo would also be much lower. Hypothesis: no measurement exists.
- **Update (EXP-022 B, c3-a, verified-on-target):** on webOS 8.3 the stream was **not** ended by the button release (it stayed live for 7 s or more until the subscription was stopped), so "gated on the button being held" (observed on webOS 25) does not hold here; the satellite must close the subscription itself. One `2003 audio data is already capturing` refusal occurred, and LG's UI showed an "audio processing is unstable" message during the test (owner observation); a coexistence test is open ([EXP-022](../experiments/EXP-022-remote-mic-button-observe.md#part-b-on-c3-a-the-remotes-voice-stream-2026-11-07)).
- **No conflict with our capture:** the remote route does not open `hw:LGSIC,0` (verified-on-target, D3 5a, LG hands-free off), so the satellite can keep the far-field mic open for the wake word while a remote run streams.

## 3. UX: what "an assistant in the TV" could look like

### 3.1 Starting a run without a wake word (protocol)

| Fact | Label | Source |
| --- | --- | --- |
| A device-initiated `VoiceAssistantRequest{start: true}` without the `USE_WAKE_WORD` flag (2) makes HA start the pipeline at **STT**; with the flag it starts at the wake word | documented | HA core 2026.10.4 `homeassistant/components/esphome/assist_satellite.py` `handle_pipeline_start` (lines 555 to 558) |
| `VoiceAssistantAudio{end: true}` → `handle_stop(False)` → HA ends the audio stream and STT finalizes ("stop"). `VoiceAssistantRequest{start: false}` → `handle_stop(True)` → HA cancels the run ("abort") | documented | aioesphomeapi 46.6.0 `client.py` (`_on_voice_assistant_request`, `_on_voice_assistant_audio`); HA `assist_satellite.py` `handle_pipeline_stop`, `_stop_pipeline`, `_abort_pipeline` |
| HA sends the device the STT text (`STT_END` `text`) and the reply text (`TTS_START` `text`) | documented | HA `assist_satellite.py` (lines 368 and 393) |
| HA-initiated runs: `assist_satellite.start_conversation` (an announcement, then a run at STT) needs feature bit 32. It is implemented in our crate and off by default | documented, host-tested | [ESPHome API crate](../development/esphome-api.md#start_conversation-status) |
| Our daemon already has a push-to-talk run (`PipelineMode::PushToTalk`, triggered today by `SIGUSR1`, `--ptt-every` and `--ptt-on-connect`). It refuses while muted, during an announcement or with no live capture. Four consecutive live push-to-talk runs through HA's real integration ran on c3-a. The state machine can already send `AudioEnd`, and it receives the transcript as `AppEvent::Transcript` | verified-on-target (c3-a, Phase 1.5); code read | [P1 TV live](../discovery/c3-a/P1-tv-live.md), `crates/satellite/src/daemon.rs` `ptt()`, `crates/esphome-api/src/voice.rs` |

So the press-to-pipeline mapping needs no new protocol work. A button event calls the existing `ptt()`. For hold-to-talk, a release event needs a new `AppCommand` that ends the audio gracefully (`AudioEnd`) instead of aborting. That is a small change in `voice.rs` (**design, not built**).

### 3.2 Push-to-talk semantics

| Mode | Behaviour | Fits |
| --- | --- | --- |
| **Hold-to-talk** (press starts at STT, release sends `AudioEnd`; HA's VAD still ends it earlier on silence; cap about 15 s) | Matches the remote's printed instruction (hold and speak) and LG's own behaviour. With remote audio it is also forced: LG's stream exists only while the button is held (observed on webOS 25) | Remote-mic audio (preferred), or the far-field mic |
| **Tap-to-talk** (press starts at STT; HA's VAD ends it) | What `SIGUSR1` does today. Needs the far-field mic, because the remote's stream stops at release | Far-field mic only |
| Long press for LG, short press for us (or the reverse) | Needs the key consumed first (section 1.3), and LG's own behaviour already starts on press | Only after a coexistence decision |

Release detection: an evdev key normally gives value 1 (press) and 0 (release). LG Input Mapper notes that the remote "sends no repeat events" and detects long presses itself. A release event exists for 428 on c3-b (value 0 about 0.25 s after a tap; verified-on-target, EXP-022 A). A held press was not tried.

### 3.3 On-screen feedback

- **Today:** none during a run. The TV app (`packaging/webos/app/`) is a full-screen HBC app opened by the owner. It is not an overlay ([voice and power screen](../design/voice-and-power-screen.md)).
- **Overlay over any app (observed-on-other-device):** CleanYourTV shows "Listening" (moving bars), "Recognising" or "Thinking", and the answer text in a small app with `"defaultWindowType":"topui"`, `"transparent":true`, `"visible":false` and `"class":{"hidden":true}`. It starts the app with `luna://com.webos.applicationManager/launch` and passes the state as `params`. Its source says "`topui` windows sit above every app and, unlike `overlay` ones, leave the remote with the app underneath" (`helper/notify.js`, `notify/app/appinfo.json`). That window type on webOS 8.3 is **unverified**.
- **Text-only toast:** `com.webos.notification/createToast` (`message` required) is documented for **webOS OSE only** ([OSE reference](https://www.webosose.org/docs/reference/ls2-api/com-webos-notification/)). CleanYourTV calls the TV's toasts "text-only". It is not verified on our TVs.
- **Proposal (design sketch, not accepted):** a small `topui` overlay app in our package with three states, driven by the daemon's state file (`run/voice.json` already exists for the app):
  - **Listening** (shown only while capture for the run is live, to keep the indicator truthful; requirement F7);
  - **Thinking** (until `TTS_START`);
  - **Answer** (the reply text for a few seconds, optionally the transcript).
  The transcript and reply text are shown on screen only, never logged ([logging policy](../../AGENTS.md)). The audible earcon and TTS reply stay as today.

### 3.4 Coexisting with LG's assistant, and the way back

| Option | Effect | Restore |
| --- | --- | --- |
| A. Alongside, nothing consumed | Both LG's assistant and ours start on the same press. Two listeners, two answers, and the audio also goes to LG's cloud. **Not acceptable** as a product, but harmless as a measurement | Nothing to restore |
| B. Ours on a different key | A spare key, for example a streaming shortcut, triggers our run. LG still runs that key's own action unless it is grabbed and re-emitted (LG Input Mapper's technique) | Release the grab (closing the fd releases it); the key returns to LG |
| C. Ours on the mic key, LG's voice paused | `EVIOCGRAB` or hook plus SIGSTOP of `voiceconductor` (CleanYourTV) | SIGCONT, or any reboot. Persistent only through a boot hook, so the HBC failsafe also restores it |
| D. Ours on the mic key through a supported LG switch | Unknown whether one exists (section 1.3 item 4) | Depends |

Options C and D change LG's behaviour and need an ADR. ADR 0008 allows only user-initiated changes to two named LG settings.

## 4. Privacy notes specific to this feature

- **A press sends audio to LG today.** With LG's assistant active, pressing the mic button starts LG's cloud recognition (LG manual: "Network connection is required"). Any test that presses the button while LG's assistant is active must assume the speech reaches LG. Proposed experiments therefore press **without speaking**, or with a neutral consented phrase.
- **System-log transcripts:** on a webOS 10.3.1 TV, LG wrote voice transcripts in plain text to `/tmp/var/log/messages` (own-your-glass F5). On c3-b EXP-022 A the 202 filtered lines held no speech text (the owner did not speak); whether LG logs transcripts on webOS 8.3 stays a hypothesis. A watcher for `NL_BUTTON_CLICK` must filter with `grep` before anything is stored, and must never copy the log.
- **The HID uevent carries an identifier:** `/sys/class/hidraw/*/device/uevent` includes `HID_UNIQ` (the remote's Bluetooth address). Read `HID_NAME` only.
- **Privacy gain:** a push-to-talk-only mode (wake word off) would mean the satellite streams nothing to HA until a button is held. That is stricter than the current always-streaming wake-word mode (ADR 0003).
- **Indicators:** the remote has no documented "mic live" light. The on-screen state must follow the real stream (the run's capture), not the key alone.

## 5. Risks

| Risk | Severity | Mitigation |
| --- | --- | --- |
| A grab (`EVIOCGRAB`) on the remote's event device locks the owner out of the remote if the re-emit fails or the process hangs | High (TV unusable without another remote; the IR remote and the phone app remain) | Not in any experiment proposed now. If ever built: re-emit every key, never remap power or navigation (LG Input Mapper's list), release on watchdog, test with the IR remote at hand |
| Pausing or killing LG voice processes breaks other LG features (a luna call to a stopped service waits until it times out; Settings pages that query voice may hang) | Medium | Needs an ADR. SIGCONT on exit, no persistence first, owner present |
| Injecting into `lginput2` (hooks) | High (an LG input process; a hook in `micomservice` broke HDMI-CEC and eARC on another TV, CleanYourTV issue 23) | Excluded |
| Calling `voiceinput/startStreaming` disturbs LG's own voice state (a stuck "listening" session, the earcon, a refused next press) | Low to medium | Part B only, with the subscription ended by killing `luna-send`; check that LG's own voice still works afterwards |
| Firmware fragility: socket path (`/tmp` versus `/var/voiceinput`), `keyType` value, the `10000` gate, node names, key code. LG offers webOS upgrades for the C3 (an OLED48C3PUA on webOS 25 firmware 33.31.20 is reported by CleanYourTV `docs/tested-tvs.md`) | Medium | Read the socket path from the reply; discover the node by name and keys, never by index; self-check and report through the soft firmware policy ([ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md)) |
| Two assistants answering (option A) | UX | Feature off by default until a coexistence option is chosen |
| Device safety during observation | Low | Read-only: passive `cat`, named files, process fd lists. Never `dapm_widget` or debugfs ([D2 incident](../discovery/c3-a/D2-audio-stack.md#incident-during-this-step-read-first)) |

## 6. Recommended approach and rough effort

1. **EXP-022 Part A (read-only, needs owner authorization because TV access is currently install-only).** It answers: which node and code the mic button produces on c3-a, whether press and release are separate events, whether `voiceinput_hidraw` exists and which process holds the hidraw node, the `startStreaming` schema (allowed `deviceType`/`keyType`), and whether the `NL_BUTTON_CLICK` line exists on webOS 8.3. Effort: about 1 hour to prepare and analyse, about 15 minutes of the owner's time.
2. **EXP-022 Part B (separate consent).** Does `voiceinput/startStreaming` deliver 16 kHz mono PCM from the remote on webOS 8.3? It counts bytes and levels only, nothing is stored. Effort: about 1 hour plus 10 minutes of the owner's time.
3. **ADR "mic button and LG's assistant" (owner decision):** options B to D of section 3.4. Optionally first an REA pass (section 7) to find a supported switch (option D) before choosing the more invasive option C.
4. **Implementation, after the ADR:**
   - (a) a passive evdev reader thread in the daemon, finding the node by name and key bitmap and never grabbing, behind `[remote] mic_button = "off" | "ptt"` (default `off`), calling the existing `ptt()`;
   - (b) hold-to-talk with a graceful `AudioEnd` on release;
   - (c) a capture source "remote" that reads `voiceinput`'s socket for that run and falls back to the far-field mic;
   - (d) the `topui` overlay app;
   - (e) an HA diagnostic (last trigger source).
   Rough effort: (a) and (b) 2 to 3 days with tests; (c) 2 to 3 days plus a TV session; (d) 2 to 3 days; (e) under a day. Each step ships separately and is deployed to the TVs under the current install-only policy.

Until steps 1 to 3 exist, a mic-button trigger would always run **alongside** LG's assistant (option A). So nothing user-facing should ship first.

## 7. What REA analysis would add (not done)

Desk research cannot answer the coexistence question. If option D is wanted, an [REA](../tools/rea.md) pass on the shipped webOS 8.3 binaries should answer:

- `voiceinput` and `voiceinput_hidraw` (if present):
  - how the mic-button state gates `startStreaming`;
  - which `deviceType`/`keyType` values are accepted;
  - the socket path and format;
  - what `/tmp/voiceinput_disable_set_input_event` disables;
  - what the adapter's `setInputEvent` does.
- `voiceconductor`: what `setInputEvent` and `getVoiceKey` do, whether a client can claim the voice key, and what `getVoiceUiEvents` reports. A subscription to it might be a read-only press signal.
- `lginput2`: whether `KEY_VOICE` is also sent to `voiceconductor` over luna, besides the uinput device.

Static analysis only; any finding still needs an experiment on the TV.

## 8. Open questions

1. ~~Node and code of the mic button, release events~~ answered on c3-b (`event2`, 428, press and release; EXP-022 A). Still open: c3-a, and a held press.
2. ~~Does `voiceinput_hidraw` exist~~ no on c3-b; `voiceinput` and `lginput2` hold `hidraw0`. c3-a not checked.
3. `startStreaming` parameters (schema read on c3-b: no `keyType`), socket path, gate and format on webOS 8.3 (EXP-022 A schema; B live).
4. Does a grab of the event node alone stop LG's UI on webOS 8.3? Not proposed until an ADR.
5. Which remote model is paired with each TV (owner can read the label; the HID name shows it too)?
6. Does LG write voice transcripts to the system log on webOS 8.3 (privacy; filtered check only)?
