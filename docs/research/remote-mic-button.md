# Magic Remote mic button as a push-to-talk trigger for the satellite

- **Date:** 2026-11-07. **Scope:** desk research only: the repository's own findings, official LG and Home Assistant material, and open-source projects for rooted LG TVs. **Nothing was run on a TV when this note was written.** Update 2026-11-07: [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md) Part A (read-only) was then run on c3-b; the results are marked verified-on-target (c3-b) below. No binary was reverse engineered. Update 2026-11-07 (later): EXP-022 Part B ran on c3-a. A read-only inspection of c3-a's voice configuration, key-filter scripts and ACG files was then done, plus a static analysis of the shipped `voiceinput`, `voiceconductor` and `lginput2` binaries, to look for a supported way to hand the button to the satellite. Results in [section 9](#9-hand-over-candidates-configuration-and-static-analysis-2026-11-07); the resulting experiment is [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) (not run), and the proposal is [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md) (proposed). Update 2026-11-07 (later): EXP-025 Part A (read-only) ran on c3-a; results and the one candidate in [section 10.7](#107-part-a-on-c3-a-the-key-filter-list-is-a-candidate-2026-11-07). Update 2026-11-07 (evening): the owner used 0.5.0 on c3-a (EXP-023 C1 through the satellite, owner-reported pass); every press shows LG's toast "Función no disponible". [Section 10](#10-removing-the-mic-button-toast-2026-11-07) traces the toast and the ways to avoid it (desk work only); the read-only follow-up is [EXP-025](../experiments/EXP-025-mic-button-toast.md).
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
| 1c | Can it consume it so LG's UI does not open? | **LG's UI opens with the press:** with "Hi LG" off, each tap launched `com.webos.app.voice` (log, and the owner saw the assistant appear, say nothing and disappear), with LG's own parameters `recognitionSource {input voiceinput, key mrcu, type remote, keyType thinQtv}` (verified-on-target, c3-b). Not cleanly consumable with what is known today. Grabbing the event device hides the key from the compositor, but LG's voice pipeline reads the HID stream directly. CleanYourTV, which hooks the key inside `lginput2`, still has to pause `voiceconductor` (SIGSTOP) to keep LG's screen and cloud recognition away. | observed-on-other-device. **Update (section 9; static, read from c3-a's shipped files):** the press reaches LG's assistant through the compositor's key filter (`KeyFilters/appLaunch.js`). The filter calls `voiceconductor/setInputEvent` only while the configd value `com.webos.keyaction.voice` has `"action":"launch"`. With any other action it shows the toast "This function is not supported." and consumes the key. LG ships `"ignore"` in its hotel-mode and other layers. An in-memory (`volatile`) configd override is the best hand-over candidate (hypothesis until [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) C1). Whether a grab alone stops LG's UI on webOS 8.3 is **unknown** (not tested; the voice route here reads `hidraw0` through `voiceinput`, which a grab of the evdev node would not touch, inference) |
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
  4. Not found in any source: a supported LG setting that hands the mic button to a third party. The `voiceinput` watch file `/tmp/voiceinput_disable_set_input_event` and `voiceconductor`'s `setInputEvent`/`getVoiceKey` are candidates whose semantics are unknown (see section 7, REA). **Update (section 9):**
     - the watch file only stops the far-field "Hi LG" trigger event;
     - `getVoiceKey` returns LG's security voice keys;
     - `setInputEvent` is the trigger input itself;
     - none of them routes the key.
     - The routing lives in the key filter and the configd key `com.webos.keyaction.voice` (technique 5 below).
  5. **configd override of `com.webos.keyaction.voice`** (new, section 9): `{"action":"ignore"}` set in memory with `setConfigs … "volatile":true`, which a reboot clears. Static evidence only. It needs [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) C1 and the proposed [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md).

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

> **Update (2026-11-07):** built as the [assistant overlay](../design/assistant-overlay.md) ([ADR 0014](../decisions/0014-assistant-overlay.md), accepted, off by default until the owner's check). [EXP-024](../experiments/EXP-024-assistant-overlay-window.md) on c3-a (`verified-on-target`, firmware `03.30.74`): **`topui` is refused** on webOS 8.3 ("Unsupported Application Type"); an **`overlay`** window works, as CleanYourTV uses on SDK 5 to 9; a second app inside one package is not registered without a SAM restart, so the overlay app is its own package. The states come through the launch parameters (not `run/voice.json`, which the web runtime cannot read). The sketch below is kept as written.

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
- **LG voice keys in settings:** `settingsservice/getSystemSettings {"category":"voiceframework"}` returns LG's `staticVoiceKey` and `dynamicVoiceKey` (security keys used by `voiceconductor`), together with harmless flags. Do not read that whole category in scripts or diagnostics; ask for named keys only. One read on c3-a on 2026-11-07 returned them. The private copy was redacted, and they appear in no document.
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

**Update 2026-11-07:** done for `voiceinput`, `voiceconductor` and `lginput2` (and configd), see [section 9](#9-hand-over-candidates-configuration-and-static-analysis-2026-11-07). The REA MCP service is not installed on the workstation. The same read-only static method was used: `readelf`, `nm -D` (the binaries export C++ symbols), `strings` and `llvm-objdump` Thumb disassembly.

## 8. Open questions

1. ~~Node and code of the mic button, release events~~ answered on c3-b (`event2`, 428, press and release; EXP-022 A). Still open: c3-a, and a held press.
2. ~~Does `voiceinput_hidraw` exist~~ no on c3-b; `voiceinput` and `lginput2` hold `hidraw0`. c3-a not checked.
3. `startStreaming` parameters (schema read on c3-b: no `keyType`), socket path, gate and format on webOS 8.3 (EXP-022 A schema; B live).
4. Does a grab of the event node alone stop LG's UI on webOS 8.3? Not proposed until an ADR.
5. Which remote model is paired with each TV (owner can read the label; the HID name shows it too)?
6. Does LG write voice transcripts to the system log on webOS 8.3 (privacy; filtered check only)?
7. Does the volatile configd override of `com.webos.keyaction.voice` reach the key filter live, stop LG's assistant, leave nothing persistent, and restore at runtime? ([EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) C1.) Does the "not supported" toast appear? **Owner-reported 2026-11-07 (0.5.0, c3-a, no logs):** the feature works; the toast appears on every press ("Función no disponible"). Persistence and restore not reported in detail.
9. Can the toast be avoided? Not with any value of the key ([section 10](#10-removing-the-mic-button-toast-2026-11-07)); possibly with an in-memory override of the key-filter list ([10.7](#107-part-a-on-c3-a-the-key-filter-list-is-a-candidate-2026-11-07), untested; [EXP-025](../experiments/EXP-025-mic-button-toast.md) Part B, not authorized; draft ADR 0015).
8. Was LG's "audio processing is unstable" message caused by our subscription? (EXP-023 C0.)

## 9. Hand-over candidates: configuration and static analysis (2026-11-07)

**Question:** is there a supported way to hand the mic button to the satellite so LG's assistant does not start? If not, what does pausing LG's voice service involve?

**Method:**
- **On the TV (c3-a, `03.30.74`, owner authorized, read-only):** read the voice configuration, schemas, ACG/role files and the compositor's key-filter scripts. Called only these getters: `config/getConfigs` and `settingsservice/getSystemSettings` for `voiceframework`, the latter not to be repeated (privacy section 4). Listed files and mtimes. Copied `voiceinput`, `voiceconductor`, `lginput2`, `performer` and `configd` to the gitignored `devices/private/c3-a/EXP-023/` (SHA256 recorded there; nothing committed). No setter, no signal, no flag file, no debugfs.
- **On the PC:** static analysis of those binaries (32-bit ARM Thumb, stripped, exported C++ symbols): strings, `nm -D`, `llvm-objdump` disassembly of the named functions; and the upstream configd source (`webosose/configd`, commit `99873ce`). The REA MCP service is not installed; the same static, read-only method was used.

**Labels:**
- "static (c3-a)" means read from files shipped on c3-a: the scripts are plain source, while for the binaries it is disassembly. It says what the code does, not that the runtime behaves so. Every runtime claim below is a **hypothesis** until [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md).
- "documented" means upstream source or documentation.

### 9.1 How a mic press reaches LG's assistant on webOS 8.3

| Step | Evidence | Label |
| --- | --- | --- |
| `lginput2` (also the `com.webos.service.mrcu` service) parses the remote's HID reports and emits `Key_webOS_Voice` (evdev 428) on its uinput device | `lginput2` strings `KEY_VOICE`, `interpret_ble_voice_packet`; EXP-022 | verified-on-target (key), static (parsing) |
| The compositor (`surface-manager`) runs the key filter `/usr/lib/qml/KeyFilters/appLaunch.js` (package `starfish-systemui-keyfilters`, lunaCall `appId` `com.webos.starfish.keyfilters`) | file content | static (c3-a) |
| For `Key_webOS_Voice`, if `access.keyAction.activateXinput` and not first-use boot: when `voiceAction.action == "launch"` it calls `voiceconductor/setInputEvent {"inputType":"hold","event":"down"/"up","deviceParam":{"keyType":"thinQtv"}}`; **otherwise it sends the toast "This function is not supported."** on the press. Either way it returns `KeyPolicy.Accepted` (apps do not get the key) | `appLaunch.js` lines 1856 to 1868 | static (c3-a) |
| `voiceAction` defaults to `launch` and is overwritten from configd `com.webos.keyaction.voice` in `cb_getConfigs`. A falsy value sets the action to `null` | `appLaunch.js` lines 507, 561, 873 to 882 | static (c3-a) |
| The key filter's `lunaCall.subscribe()` always adds `"subscribe": true`, so it is subscribed to `com.webos.keyaction.*` and gets changes at runtime. configd posts changed values to `getConfigs` subscribers after `setConfigs` | `StarfishKeyFilter.qml` `subscribe()`; upstream `Manager::updateUnifiedDatabase` → `postGetConfigs` | static (c3-a) + documented; live delivery is a hypothesis |
| Current value on c3-a: `{"action":"launch","id":"com.webos.app.voice","params":{"activateType":"mrcu"}}`, from `/etc/configd/layers/base/com.webos.keyaction.json` | `getConfigs` | verified-on-target (c3-a) |
| **LG's own layers set `"voice": {"action": "ignore"}`** in `hotelmode/on`, `tvoverlay` and `distro/hd1080-mini` (not active on c3-a) | layer files | verified-on-target (files) |
| `voiceconductor` then runs its press detection (`TVInputEventHandler`: `DetectHold`, `ShortPressOnly`, `LongPressOnly`; the 500 ms "long press" timer), and the performer launches `com.webos.app.voice` (EXP-022 A log) | strings; EXP-022 A | static + verified-on-target (log, c3-b) |
| The same `voiceAction` check also gates `Key_webOS_TonePlusVoice` (TONE Free earbuds). The Alexa, Google and Search keys have separate branches that call `voiceconductor` directly | `appLaunch.js` | static (c3-a) |

### 9.2 What each lead turned out to be

| Lead | Finding | Label | Usable as a switch? |
| --- | --- | --- | --- |
| `/tmp/voiceinput_disable_set_input_event` | `watchFolder.json` maps create to `eventCode` 3 and delete to 4. `CPreprocessingManager::handleDisableSetInputEventSetup` reads the task's `enable` property and calls `CSoundEngine::setRunningMode(4)` (enable) or `setRunningMode(0)`. The `RUNNING_MODE_NOT_SEND_EVENT` log string is used in `CSoundEngine::verified(CVerifierResult, CTriggerInfo)`, the far-field wake-word verifier. So the file stops the **far-field "Hi LG" trigger event** from being sent; it does not touch the remote key path. Its sibling `voiceinput_dump_setup` is a dump hook. The delete path sets mode 0, which need not equal the mode before the file was created | static (c3-a); mode 4 = `NOT_SEND_EVENT` is an inference from the string use | **No** (wrong path; unsafe delete) |
| `voiceconductor/setInputEvent` (private) | The trigger input. Parameters seen in strings and callers: `inputType`, `event`, `deviceId`, `deviceParam` (`keyType`, `key`), `detectShortPress`, `detectLongPress`, `autoEndDetection`, `voiceModel`, `voiceUiParam`, `inAppControl`, `triggerType`. Callers: the key filter, and `voiceinput` itself (`CVoiceConductor::sendSetInputEvent`, for far-field triggers through its adapter `voiceinputadapter/setInputEvent {"triggerType"}`). No "claim the key" semantics found | static (c3-a) | No (it starts LG's flow) |
| `voiceconductor/getVoiceKey` (private) | `VoiceFWLunaAdapter::GetVoiceKey` returns `SecurityKeyManager::StaticVoiceKey()` and `DynamicVoiceKey()`: security keys, also stored in the `voiceframework` settings category. Not a key binding. **Not called** (a credential) | static (c3-a) | No |
| LG settings | `voiceframework` category: `supportStt`, `supportNlp`, `audioFeedback`, `thinQtvTTSBlock`, `*LongDistanceGuided`, `voiceModel`, … and the two voice keys. `/etc/palm/voice-framework.conf` binds `webOSVoice` to `thinQtv`/`dangbeiVoice` (read-only rootfs). None disables or redirects the mic key | verified-on-target (values), static | No |
| `com.webos.service.mrcu` (in `lginput2`) | About 80 private methods (pairing, cursor, IR, sensor, test). None routes the voice key | verified-on-target (ACG file) | No |
| **configd `com.webos.keyaction.voice`** | See 9.1. `setConfigs` (ACG `configd.internal`, `configPermissions.json` allows every config to private clients) with `"volatile": true` stores the value in configd's in-memory "Volatile Database", which `reconfigure` and a reboot clear. Without `volatile` (and not from `com.webos.app.factorywin`), it writes the **persistent** `/var/preferences/configd_factory_db.json` (absent on c3-a today). Each call also writes `/tmp/configd_<time>_before_setConfigs.json` and appends a timestamp to `configd_debug_db.json`. The OSE API page says values are kept "until rebooting", which the source confirms only for `volatile`. The shipped `configd` has the same symbols (`Configd::setConfigs`, `splitVolatileConfigs`, `JsonDB::getVolatileInstance`, the `volatile` and `factory_db` strings) | documented (upstream source and [OSE API page](https://www.webosose.org/docs/reference/ls2-api/com-webos-service-config/)) + static (c3-a); untested on the TV | **Yes, best candidate** ([EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) C1). Costs a "not supported" toast per press (static reading) |
| CleanYourTV: SIGSTOP `voiceconductor` | `voiceconductor` is a systemd unit (`Restart=on-failure`, `OOMScoreAdjust=-1000`, no watchdog setting), and SIGSTOP is not a failure for systemd. While stopped, the key filter's `setInputEvent` (sent without a callback) and every other client's call wait in the bus, and are delivered at SIGCONT (inference) | observed-on-other-device (webOS 25) + static (unit file) | Fallback (EXP-023 C2) |
| 500 ms long-hold timer | Inside `voiceconductor` (`DetectShortPress`, modes `DetectHold`, `ShortPressOnly`, `LongPressOnly`), chosen by `setInputEvent` parameters from the caller. Not a hand-over point | static (c3-a) | No |

### 9.3 `voiceinput` and the remote's audio

- **Capture path:**
  - `voiceinput` reads the remote's BLE voice reports itself (`CCaptureHidRaw`, `CSbcInterpreter::interpretBleRemoconData`, in-band markers logged as `BUTTON_VOICE_START 0a` and `BUTTON_VOICE_STOP 0d`).
  - `lginput2` also parses voice packets (`interpret_ble_voice_packet`, `LGINPUT_COMMAND_VOICE_START/STOP`).
  - Who tells the remote to start sending audio is not established (hypothesis: `voiceinput` when a stream is requested).
  - Static (c3-a).
- **Client model:**
  - Each device (`CDeviceInfo`) has an owner and a client key.
  - A second `startStreaming` for a device that is already capturing gets `2003 audio data is already capturing`.
  - No priority or pre-emption API was found: the schema has only `id`, `deviceType`, `key`, `socketType` and `subscribe`.
  - So it is first come, first served. Our subscription can own the stream only if LG's flow does not start it first. The configd override would keep LG's flow from starting at all.
  - Static (c3-a) + EXP-022 B; hypothesis for the runtime rule.
- **If `voiceconductor` is stopped:**
  - `voiceinput` keeps running.
  - Its own calls to `voiceconductor` (far-field triggers only, and "Hi LG" is off) would wait.
  - Our `startStreaming` goes to `voiceinput` directly. CleanYourTV streams this way with `voiceconductor` paused on webOS 25.
  - Hypothesis for webOS 8.3 (EXP-023 C2).

### 9.4 Result

- **Best candidate:** an in-memory configd override, `com.webos.keyaction.voice = {"action":"ignore"}` with `"volatile": true`, applied only while the satellite runs with the feature on, and restored with `null`. This is LG's own mechanism (LG uses the value for hotel mode), it changes no LG binary or process, and a reboot clears it.
- **Costs and unknowns:** a toast per press (static); live delivery to the key filter; configd's debug bookkeeping writes.
- **Fallback:** SIGSTOP of `voiceconductor`.
- Both are specified in [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) (not run) and the proposed [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md).

## 10. Removing the mic-button toast (2026-11-07)

**Question:** while the satellite owns the mic key (ADR 0013, `com.webos.keyaction.voice = {"action":"ignore"}`, volatile), every press shows LG's toast "Función no disponible" (owner-reported on c3-a with 0.5.0, [EXP-023 observations](../experiments/EXP-023-remote-mic-button-handover.md#observations-and-redacted-evidence-links)). Which component shows it, and can it be stopped narrowly? **Why it matters:** the toast appears on every use of the feature, next to our own overlay. **Evidence needed:** the code path that sends it, the values the key action accepts, and any scoped suppression.

**Method:** desk work on the PC only, nothing run on a TV.
- The key-filter files copied read-only from c3-a (`03.30.74`) on 2026-11-07 for section 9 (gitignored `devices/private/c3-a/EXP-023/fs/`): `KeyFilters/appLaunch.js`, `easytv.js`, `preProcess.js`, `systemUi.js`, `WebOSCompositor/controllers/StarfishKeyFilter.qml`. Plain source, so "static (c3-a)" below means "read in the shipped file", not "seen at runtime".
- `strings` of the copied `voiceconductor`, `voiceinput`, `lginput2` and `performer` (section 9 method).
- Upstream webOS OSE source: [webosose/notificationmgr](https://github.com/webosose/notificationmgr) (commit `c50c4d7`), [webosose/configd](https://github.com/webosose/configd) (`99873ce`), [webosose/luna-surfacemanager](https://github.com/webosose/luna-surfacemanager) (`b2fef6f`), and the [OSE `com.webos.notification` API page](https://www.webosose.org/docs/reference/ls2-api/com-webos-notification/). OSE is **not** proof for the commercial TV build.
- Community: CleanYourTV (`dev`, `0945050`) and LG Input Mapper (`fca03ae`) contain no `keyaction`, `setConfigs` or toast handling for the mic key; a web search for `com.webos.keyaction.voice` found nothing public.
- REA was not used: every question here is answered by plain-source files or by upstream source; what is left needs read-only inspection on the TV ([EXP-025](../experiments/EXP-025-mic-button-toast.md)), not decompilation.

### 10.1 Which component shows the toast (question 1)

| Claim | Label | Source |
| --- | --- | --- |
| The toast is sent by the compositor's key filter, `appLaunch.js` `handleHotkeys()`, case `WebOS.Key_webOS_Voice`: when `voiceAction.action != "launch"` and the key is **pressed**, it calls `com.webos.notification/createToast` with `sourceId` = the key filter's luna app id (`com.webos.starfish.keyfilters`) and `message` = `qsTranslate("appLaunch", "This function is not supported.")`; it then returns `KeyPolicy.Accepted` (the key goes nowhere else). One toast per press, none on release | static (c3-a) | `appLaunch.js` lines 1856 to 1868 |
| Not `voiceconductor`, `voiceinput`, `lginput2` or SAM: in the non-`launch` branch the filter calls nothing but `createToast`. `voiceconductor` gets no `setInputEvent` | static (c3-a); runtime not logged | same lines |
| The owner's toast text "Función no disponible" is this message in the TV's Spanish UI | **hypothesis** (strong: it is the only toast on that branch). **Correction (10.7, EXP-025 A5):** the TV's Spanish for this string is "Esta función no es compatible."; "Función no disponible" is in no Spanish catalogue, so the quote is most likely a paraphrase. The Spanish catalogue was not read. Note the back-translation fits "This function is not available." better, which is a **different** source string the filter uses for the sleep, aspect-ratio and picture-mode keys, not for the voice key; LG may translate both alike. [EXP-025](../experiments/EXP-025-mic-button-toast.md) A5 reads the `.qm` catalogue, A6/B1 the toast's caller in the log | owner report + static |
| **Not tied to `ignore`.** The test is `voiceAction.action != "launch"`, so every value other than the exact string `launch` (any string, a missing `action`, or a falsy config value, which sets `action = null`) takes the toast branch | static (c3-a) | `appLaunch.js` lines 873 to 882, 1858 |
| The same branch, with the same toast, handles `Key_webOS_TonePlusVoice` (TONE Free earbuds) | static (c3-a) | `appLaunch.js` lines 1870 to 1880 |
| Silent ways out of the branch exist only through the guard before it: `access.keyAction.activateXinput` false (also false while a modal LG alert is active), `bootManager.bootFirstUse`, or the rolling-screen (`aplus`, LG OLED R) "zero view" state. None of them is a setting a third party can scope to this key; where `access.keyAction` comes from is not in the copied files | static (c3-a); source of `access` unknown | `appLaunch.js` lines 634 to 641, 1857; `StarfishKeyFilter.qml` lines 611 to 623 |

### 10.2 What `com.webos.keyaction.*` accepts (question 2)

| Claim | Label | Source |
| --- | --- | --- |
| configd does not validate values: `setConfigs` only requires `configs` to be an object | documented (upstream) | `configd/files/schema/ConfigdService.setConfigs.schema` |
| Values LG uses across `com.webos.keyaction.*` on c3-a: `action` `launch` (with `id`, `params`), `ignore`, `showQuickSettings` (with `params`); `accept: false` (colour keys, read by some branches to pass the key on); `fgAppIdsForByPass` (guide key) | verified-on-target (getConfigs, 2026-11-07, section 9) + static | `get1.txt` (private), `appLaunch.js` |
| **For the voice key only `action` is used, and only as "is it `launch`".** The filter copies `id` into `voiceAction.appId` but never reads it; `params` is not read; `accept` is not read in the voice branch; there is no "silent", "showToast", "none" or luna-call action | static (c3-a) | `appLaunch.js` (all 8 uses of `voiceAction`) |
| `launch` always means `voiceconductor/setInputEvent` (hold, down/up, `keyType thinQtv`), never an app launch by the filter. The base value's `id` (`com.webos.app.voice`) and `params` (`activateType mrcu`) are not read by `voiceconductor`, `voiceinput`, `lginput2` or `performer` either (no `keyaction` string in any of them; `performer` has `com.webos.app.voice` built in) | static (c3-a) | `strings` of the four binaries |
| **Conclusion:** no value of `com.webos.keyaction.voice` both keeps LG's assistant away and avoids the toast, on this firmware | static (c3-a); confirmed at runtime only for `ignore` (toast, owner-reported) | above |

### 10.3 Could the key launch our overlay instead? (question 3)

- **Not through this key action.** `launch` is hard-wired to `voiceconductor`, and the `id` is ignored (10.2). An override such as `{"action":"launch","id":"<app id>.overlay"}` would hand the press to LG's assistant again (and the audio to LG's cloud), so it is not proposed, not even as a trial. static (c3-a).
- **Not through LG's voice framework either:** `voiceconductor`'s voice models (`/etc/palm/voice-framework.conf`, `voiceKeyBindings` `webOSVoice` → `thinQtv`, required packages `lg.thinqai`, `com.webos.service.voice.performer`, `com.webos.app.voice`) live on the read-only rootfs, and registering a model would still start LG's capture and recognition flow. Rejected. static (c3-a).
- **Not needed for the panel:** the satellite already opens its own overlay on the 428 press (ADR 0014), and the owner reports it works. Focus and the `overlay` window type are therefore unchanged by this question: SAM launches the overlay as today, the foreground app stays in front (EXP-024). The only thing the key action could add, silence, it cannot give (10.2).

### 10.4 Notification-side suppression (question 4)

| Mechanism | Finding | Label | Verdict |
| --- | --- | --- | --- |
| `com.webos.notification/disable` and `enable` | Turn off toasts, alerts, input alerts and PIN prompts for **every** source; privileged callers only | documented (OSE API page; `notificationmgr` `cb_disable`, `UiStatus`) | **Rejected:** global, hides important system messages |
| Store-mode silence (`option` settings `storeMode` = `store` with `enableToastPopup` = `off`) | Silences all toasts, and store mode changes much more of the TV | documented (OSE `Settings.cpp`) | **Rejected:** global, and an LG setting outside ADR 0008 |
| Per-app toast disable | `Settings::disableToastNotificationForApp` exists but is an empty stub in OSE; no luna method reaches it | documented (OSE `Settings.cpp`) | Not available |
| `closeToast {"sourceId":…}` after the press | In OSE it only deletes the stored item from the notification history (`History::deleteMessage`); privileged callers only. Whether the commercial build also takes a toast off the screen is unknown | documented (OSE) + hypothesis (TV) | **Not proposed:** would act on LG's notification service at runtime (outside ADR 0013), and in OSE does not hide the visible toast |
| A "do not disturb" setting | None found in OSE or in the TV's settings categories read so far | documented (absence in OSE) | Not available |
| The compositor closing a visible toast on a key press | Only in the Easy TV profile (`easytv.js` `handleNotification`) | static (c3-a) | Not applicable |

The commercial TV's notification service may differ from OSE (**hypothesis**); [EXP-025](../experiments/EXP-025-mic-button-toast.md) A6 reads its ACG declaration (method list) read-only, without calling it.

### 10.5 Other paths in the compositor (found while answering 1 and 2)

| Path | What it would do | Label | Verdict |
| --- | --- | --- | --- |
| **Key-filter chain from configd** (`com.webos.surfacemanager.keyFilters`, a list of `{file, handler}` loaded by `StarfishKeyFilter.qml`). **Update 10.7: the loader takes any absolute path (verified-on-target); now the candidate** | A volatile override that puts a handler in front of `appLaunch.js` could accept `Key_webOS_Voice` silently. Every change of the list tears down and reloads **all** key filters (timers cancelled, luna subscriptions dropped); a handler of ours would run inside `surface-manager`, and a mistake can leave the remote without Home, Back or volume | static (c3-a) for the loading; whether `file` may point outside `/usr/lib/qml/KeyFilters/` is unknown ([EXP-025](../experiments/EXP-025-mic-button-toast.md) A1, A3) | **Not recommended.** Code in LG's compositor is the same risk class as the `lginput2` hooks ADR 0013 rejects. It is a different configd key, so it would need a **new ADR**, not only an amendment |
| LG's own silent blockers reordered into the chain | `easytv.js` `handleBlockKeys` accepts the voice key without a toast, but also Netflix, Amazon, Search, Guide, Help and 7 more keys; the hotel-mode block needs hotel mode on | static (c3-a) | **Rejected:** not narrow |
| App-scoped key-filter bypass (`preProcess.js` `handleKeyBlockByApp` → `isKeyFilterBlocked`) | While an app listed in `viewState.blockKeyFilterNonCardTypeApps` is in front (and, with an `allowedKeyMap` entry, has focus), keys skip the key filter and go to that app: no toast, no LG. Where the list comes from (configd, appinfo, SAM) is not in the copied files. It would only help while our app is the foreground app; the overlay is not (EXP-024), and the first press comes before any window of ours exists | static (c3-a); source unknown ([EXP-025](../experiments/EXP-025-mic-button-toast.md) A4) | **Low value;** recorded so the read-only check settles it |
| Keep `launch` and pause `voiceconductor` (ADR 0013 decision 2(b)) | No toast (the filter takes the `launch` branch), LG's assistant held back by the SIGSTOP. Queued `setInputEvent` calls are delivered at SIGCONT (inference, section 9.2), which could open LG's screen late; every other `voiceconductor` client waits | static + observed-on-other-device (webOS 25) | Not proposed here. Not covered by AGENTS.md's exception; would need the owner's decision and an ADR 0013 amendment, plus EXP-023 C2 |
| A different key for push-to-talk (section 3.4 B) | No toast, but the owner asked for the mic button, and a grab is excluded | — | Out of scope |

### 10.6 Result

- **Component:** LG's compositor key filter (`appLaunch.js`), on the press, for **any** `com.webos.keyaction.voice` value other than `launch`. Not `voiceconductor`, `lginput2` or the notification service's own logic. static (c3-a); text on the TV owner-reported.
- **Within ADR 0013's scope (this key, volatile values only) the toast cannot be removed** on firmware `03.30.74`: the voice branch knows only "launch, or toast and consume". static (c3-a).
- **No credible narrowly scoped candidate** was found by desk work alone. **Update (10.7):** after EXP-025 Part A the key-filter list became a credible, invasive candidate. Every remaining path either suppresses all system toasts (rejected), runs our code inside LG's compositor (not recommended; new ADR), needs our app in front (low value), or pauses `voiceconductor` (ADR 0013 (b); amendment).
- **What is cheap and safe next** (as written before Part A ran; Part A is done and Part B was rewritten, see 10.7): the read-only [EXP-025](../experiments/EXP-025-mic-button-toast.md) Part A (about 15 to 20 minutes, no press needed) confirms that the running filter is the file read here, finds the Spanish string, and settles the two open hypotheses of 10.5 (where the filter list may load files from, and where the app-scoped bypass list comes from). Part B (one press, about 5 minutes with the owner) attributes the toast to the key filter in the log, under exactly ADR 0013's override.
- **Mitigation without any LG change (design, not built):** keep the toast secondary. Ask the owner where the toast sits relative to our panel (bottom centre); if they overlap, move or delay the panel's first frame. The TV app's note already warns that the toast "may appear"; it could say that it does.
- **ADR:** nothing here changes ADR 0013. Any path that removes the toast needs an amendment (voiceconductor pause) or a new ADR (key-filter chain or another configd key).

### 10.7 Part A on c3-a: the key-filter list is a candidate (2026-11-07)

[EXP-025](../experiments/EXP-025-mic-button-toast.md#part-a-results-2026-11-07) Part A ran read-only on c3-a (`03.30.74`) the same evening, owner-authorized for one run. It corrects two points above and turns one "not recommended" path into the only candidate.

| Claim | Label | Source |
| --- | --- | --- |
| The running key-filter files are the ones analysed in 10.1 (same SHA-256), and the active chain `com.webos.surfacemanager.keyFilters` has 18 `{handler, file}` entries with absolute paths; `appLaunch.js handleHotkeys`, which holds the voice-key branch, is 13th; nothing before it handles the voice key | verified-on-target (c3-a) + static | EXP-025 A1, A2 |
| The voice key has **no entry of its own** in the list; it is one case of `handleHotkeys`, which also serves Settings, Guide, inputs and the app keys. Removing that entry is not narrow | verified-on-target + static | A1 |
| The loader includes **any absolute path**: `StarfishKeyFilterShell.js` `load()` = `Qt.include("file://" + file)`, no directory check (10.5 assumed this was unknown) | verified-on-target (file) | A3 |
| On the owner's three presses with 0.5.0, LG's toast window (`com.webos.app.toast`) appeared 12 to 58 ms after each `KEY_VOICE` and closed after 5.0 s; no LG voice app opened (a press before the feature was on opened `voiceagent` and `com.webos.app.voice`); our overlay appeared 0.6 to 2.0 s after the press | verified-on-target (filtered log of the owner's session) | A7 |
| **Correction to 10.1, the text:** the key filter's Spanish string on this TV is **"Esta función no es compatible."** (`es_ES` and `es`), source "This function is not supported."; "Función no disponible" is in none of the 48 Spanish catalogues. The owner's quote is most likely a paraphrase of the key filter's toast (hypothesis; the timing and the absence of LG's voice apps point to the key filter); the owner can confirm by reading the toast | verified-on-target (catalogues) + hypothesis (owner's wording) | A5 |
| The TV's notification service declares no per-source or per-app suppression method (10.4 holds on target) | verified-on-target (ACG file) | A6 |
| The app-scoped bypass list is the configd key `com.webos.surfacemanager.blockKeyFilterNonCardTypeApps`; `allowedKeyMap` is filled by an app's `registerAllowedKey` event; `access.keyAction.activateXinput` comes from the **fullscreen** app's window property `_WEBOS_ACCESS_POLICY_KEYACTION_ACTIVATE_XINPUT`. All three act only while a given app is in front (10.5 row confirmed: low value) | verified-on-target (files) + static | A4 |
| `surface-manager` runs as root; the app directories under `/media/developer/apps/usr/palm/applications/` (ours included) and their parents are **world-writable (0777)** | verified-on-target | EXP-025 Part A |

**Candidate (hypothesis, untested):** an in-memory override of `com.webos.surfacemanager.keyFilters` equal to LG's current list plus **one** entry, placed just before `handleHotkeys`, whose file (ours, 3 lines, root-owned in a private `mktemp -d` directory under `/tmp`) returns `KeyPolicy.Accepted` for `Key_webOS_Voice` and `KeyPolicy.NextPolicy` for everything else.
- **Expected:** the mic key is consumed silently by our handler: no toast, no LG assistant; every other key passes on to LG's handlers as before. The satellite still sees the press through its passive evdev reader.
- **Basis:** configd puts any key with `"volatile":true` in memory (upstream `Configd::setConfigs`, `documented`; used live for `com.webos.keyaction.voice` on c3-a, `verified-on-target`); the compositor subscribes to the list and rebuilds the chain on every change (static); in OSE's `WebOSKeyFilter::handleKeyEvent` a handler that is not callable, throws or returns `undefined` is skipped (`documented`, OSE `luna-surfacemanager` `b2fef6f`; the TV's C++ may differ).
- **Restore:** `setConfigs {"configs":{"com.webos.surfacemanager.keyFilters":null},"volatile":true}`: configd removes the in-memory entry (`null` → `remove`, upstream) and serves LG's base list; read it back and compare with the list saved before. A TV restart clears both the override and `/tmp`.
- **Keep ADR 0013's override as the fallback:** if our handler fails to load, the press reaches `appLaunch.js` with `ignore` and shows today's toast, never LG's assistant.

**Risks (each a reason the decision belongs to the owner):**
1. **Our code runs inside LG's compositor as root.** A wrong file can only be "not callable" or throw (skipped, per OSE), but the commercial build is not verified. The file must never live in a world-writable place: on this TV that rules out our own install directory.
2. **Every apply and every restore rebuilds the whole chain:** `StarfishKeyFilter.qml` cancels all key-filter timers, drops all their luna subscriptions, destroys and re-creates each filter object and calls its `init()` again (`appLaunch.js` `init()` re-subscribes to about 15 services and pre-launches Quick Settings and the input picker). A key pressed during the rebuild may be lost; a long press in progress is cancelled. In the rebuild loop, a filter object that fails to be created makes the loop throw on `obj.shell`, which would leave every handler after it out of the chain until the next change (static; LG's loader component is the one created, so this needs LG's own QML to fail).
3. **The override pins the list:** while it is active, a change LG makes to `com.webos.surfacemanager.keyFilters` (a layer switch, a mode such as Easy TV, if it uses this key) is masked by our in-memory value (hypothesis). Restoring with `null`, never by writing back a saved list, always returns to LG's current value.
4. **A dangling file:** if the directory disappears while the override is active, nothing changes until the next rebuild, which then skips our handler (fallback: the toast).
5. **Scope:** a different configd key from ADR 0013's and code in the compositor. It needs its own ADR and a change to the AGENTS.md exception; draft [ADR 0015](../decisions/0015-silent-mic-key-filter.md) (proposed, not accepted).

**Result after Part A:** 10.6 stands for `com.webos.keyaction.voice` (no value avoids the toast) and for notifications (no scoped switch). The key-filter list is now a **credible but invasive** candidate. The next step is the owner's decision on running EXP-025 Part B (rewritten for this candidate, not authorized), and on draft ADR 0015.
