# D2: Audio stack and LG voice services inventory, c3-a (LG C3)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74, kernel 5.4.96 aarch64, SoC reported as "LG1213"
- **Date:** 2026-11-01
- **Method:** read-only shell inspection over SSH as root, plus read-only `luna-send -n 1` getters. No `/dev/snd` node was opened. Nothing was recorded or played, and no tracing was done. Raw outputs are kept in the gitignored `devices/private/c3-a/D2/`.
- **Related:** [EXP-002 microphone mapping](../../experiments/EXP-002-microphone-mapping.md), [EXP-006 AEC reference](../../experiments/EXP-006-aec-reference.md), [02-audio-capture](../../research/02-audio-capture.md), [03-playback-aec](../../research/03-playback-aec.md)

Confidence labels:

- **verified**: read directly on c3-a in this session (verified-on-target, read-only).
- **inferred**: derived from on-device names, configs or static strings, but not exercised.
- **unknown**: not determined.

## Incident during this step (read first)

Reading `/sys/devices/platform/soc-audio/lg115x.6/dapm_widget` and `.../lg115x.8/dapm_widget` with `cat` caused **two kernel oopses** (`Unable to handle kernel paging request` / `NULL pointer dereference`, `pc: mutex_lock`, `lr: dapm_widget_show`). Only the `cat` process was killed. The TV stayed in power state `Active` with the HDMI1 app in the foreground, and pulseaudio, audiod, audiooutputd and voiceinput kept the same PIDs. The kernel taint value went from P|O to P|O|D. Confidence: verified.

- Do **not** read `dapm_widget` files under `soc-audio` or `lgsic-audio`, and do not read `/sys/kernel/debug/asoc/*`, on this kernel. This is a vendor driver bug in a read-only path.
- Recommendation: restart the TV fully when convenient so the kernel no longer runs in a post-oops state. This agent did not reboot it.
- Recommendation: add these paths to the probe tooling denylist (`tools/probe/probe.sh` does not touch them today).

## TV state at snapshot time

| Item | Value | Confidence |
| --- | --- | --- |
| Power state | `Active` (screen on) | verified |
| Foreground app | `com.webos.app.hdmi1` (HDMI input) | verified |
| Audio stream | one stream, `umilivetv`, source `ADEC1` to sink `MAIN`, pipeline source type HDMI | verified |
| Sound output | `tv_speaker`, master volume 100, not muted. The volume scale was not cross-checked against the on-screen value. | verified |
| Built-in mic status | `internalMicrophone.connectStatus = "TV"`, `externalMicrophone = NotConnected` (`com.webos.service.audio/tv/getHeadphoneStatus`) | verified |
| LG hands-free voice ("Hi LG"/Alexa/other) | `voiceinput/setting/getActivation`: `settingStatus=false`, `activationState=false` for ThinQ, Amazon and a third provider. `supportPlatform=true`. | verified |
| Always Ready | `alwaysready/aod/getStatus` returns `status: none` | verified |
| PulseAudio | every sink and source `SUSPENDED`. The only clients are audiod and the probe. | verified |

All findings below describe this state: screen on, HDMI input playing, hands-free voice off, idle voice. Behavior while a voice session is active, while hands-free is on, or in standby/Always Ready is **unknown**.

## ALSA cards and PCMs

ALSA driver version `k5.4.96-307`. There are two cards. **Card indexes are not universal**, so match on card id and PCM id strings.

| Card id | Platform device | Driver | Confidence |
| --- | --- | --- | --- |
| `lg115x` (index 0 here) | `/devices/platform/soc-audio` | `soc-audio` | verified |
| `LGSIC` (index 1 here) | `/devices/platform/lgsic-audio` | `lgsic-audio` | verified |

| PCM (this boot) | ALSA id | Dir | Subdevs | State now | Holder now | Notes | Confidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `hw:LGSIC,0` | `WoV PDM Mic snd-soc-dummy-dai-0` | capture | 1 | closed | none | "Wake-on-Voice PDM microphone". sysfs node `PDMMic`. | verified (name), inferred (built-in mic) |
| `hw:LGSIC,1` | `SpeakerFeedback snd-soc-dummy-dai-1` | capture | 1 | closed | none | likely speaker or amp output tap, an AEC reference candidate | verified (name), inferred (role) |
| `hw:LGSIC,2` | `MixerCapture snd-soc-dummy-dai-2` | capture | 1 | closed | none | likely mixer output tap | verified (name), inferred (role) |
| `hw:LGSIC,3` | `SoundEngineCapture snd-soc-dummy-dai-3` | capture | 1 | closed | none | likely sound-engine (post-processing) output tap | verified (name), inferred (role) |
| `hw:LGSIC,4` | `BluetoothCapture snd-soc-dummy-dai-4` | capture | 1 | closed | none | likely the BT audio-out path tap | verified (name), inferred (role) |
| `hw:lg115x,0..5`, `8`, `9` | `lg115x.N lg115x-codec-dai.N-N` | playback | 1 each | closed | none | purpose unknown | verified (closed), unknown (role) |
| `hw:lg115x,6` | `lg115x.6 ...` | playback | 1 | closed | none | PulseAudio sink `pcm_output` (default sink) | verified |
| `hw:lg115x,7` | `lg115x.7 ...` | playback | 1 | closed | none | PulseAudio sink `ptts` (TTS role sink) | verified |
| `hw:lg115x,10..14` | `lg115x.N ...` | capture | 1 each | closed | none | purpose unknown. Possibly TV-audio capture taps used by audiod `tvCaptureManager` or music recognition. | verified (closed), unknown (role) |

- **Every PCM substream was `closed`** (`/proc/asound/card*/pcm*/sub0/status`). No process held any `/dev/snd/pcm*` node. HDMI audio is decoded and played through the vendor hardware pipeline (ADEC to MAIN via `/dev/lg/*` and the UMI mixer), **not** through ALSA. Confidence: verified.
- Channel count, sample format and rate for each PCM are **unknown**, because `hw_params` is only visible while a PCM is open. Confidence: unknown.
- **Inferred** from the voiceinput tuning table `/usr/share/voiceinput/preDefinedSetup_v0.6.json`: other 2020 to 2023 LG SoCs list 4 or 6 mic-stream channels with left/right mix masks such as `[[1,0,1,0,1,0,0,0],[0,1,0,1,0,1,0,0]]`. The voiceinput strings also mention left and right mic inspection. This suggests a multi-channel interleaved PDM stream from a 2-side mic arrangement. **No entry explicitly names C3**, so this is not confirmed for c3-a.
- `/dev/snd/*` nodes are `root:audio` with mode `0770` (`crwxrwx---`). There are no `by-path` or `by-id` links. Confidence: verified.

### ALSA userland configuration

- `/etc/asound.conf` makes `default` a **pulse** PCM (`device pdefaultapp`) and defines pulse-backed aliases: `feedback`, `media`, `defaultapp`, `ringtones`, `alerts`, `tts`, `ndk`, `voicerecognition`, `powersound`, `wowsound`, `remote`, `record` (to `precord`) and `voipsource` (to `pvoipsource`). No `dsnoop`, `dmix`, `plug`, AEC or loopback alias exists for the LGSIC card. Confidence: verified.
- `/etc/alsa/conf.d` holds only the stock `50-pulseaudio.conf` and `99-pulseaudio-default.conf`. There are no LG-specific ALSA plugins. `alsa-lib` plugins present: pulse pcm, ctl and conf. Confidence: verified.
- **Correction to earlier notes:** `arecord` and `aplay` (alsa-utils), plus `amixer`, `alsaloop`, `speaker-test`, `pactl`, `pacmd`, `pacat`/`parec`/`pamon` and `paplay`, **are present in `/usr/bin` and on the SSH PATH** for this session. Confidence: verified.
- The shipped userland binaries examined are **32-bit ARM ELF** (EI_CLASS=1, e_machine=ARM), and the debug paths show `lib32-audiod`, even though the kernel is aarch64. The runtime/ABI step should confirm. Confidence: verified for voiceinput, inferred for the rest.

## Sound server architecture

PulseAudio 9.0 (`9.0-28-gb2d6-dirty`) runs in **system mode** as uid `pulse`. Its command line is `/usr/bin/pulseaudio --log-target=syslog --log-level=debug --system=1 --resample-method=palm`. The native socket is `/var/run/pulse/native` (mode 0777, `auth-anonymous=1`). There is also an abstract unix socket `@palmaudio` that connects `module-palm-policy` to audiod. Default sample spec: s16le 2ch 44100 Hz. Confidence: verified.

Static configuration (`/etc/pulse/system.pa`), confirmed by `pactl list short modules` (confidence: verified):

- `module-alsa-sink device=hw:0,6 sink_name=pcm_output` (default sink) and `module-alsa-sink device=hw:0,7 sink_name=ptts`.
- Null sinks for each policy role: `palerts`, `pfeedback`, `pringtones`, `pmedia`, `pdefaultapp`, `peffects`, `pvoicerecognition`, `ppowersound`, `pwowsound`, `pndk`, `pnetflix`, `rtp` (8 kHz A-law), `remote`.
- `precord` and `pvoipsource` are **null sources**, so they produce silence today. `set-default-source pcm_input` names a source that is not loaded statically.
- `module-palm-policy` loads at startup. There is **no** `module-echo-cancel`, `module-loopback`, `module-alsa-source` or `module-udev-detect`.

Dynamic mic path (**inferred** from static strings in `module-palm-policy.so`): on a "mic recording cmd from Audiod", the policy module runs `load-module module-alsa-source device=hw:%d,%d mmap=0 source_name=pcm_input fragment_size=4096 tsched=0`. It then redirects virtual sources such as `precord` and `pvoipsource` to that physical source, and unloads it afterwards. audiod's `getSoundInputList` reports `pcm_input`, `usb_input` and `remote.monitor`, all with `Active:false, Connected:false`. Confidence: verified (list). Which card and device audiod would pass for `pcm_input` is **unknown**.

```text
                       luna-bus (Luna API)
                              |
     voiceconductor --- voiceinput ------------- audiod (com.webos.service.audio / com.webos.audio)
     (session/NLP)     (wake word, AEC,          |  @palmaudio (abstract socket)
                        BT-remote audio)         v
                              |            PulseAudio 9 (system mode, /var/run/pulse/native)
       direct ALSA            |              role null sinks (pmedia, pdefaultapp, ptts, remote, ...)
       (libaec-fe:            |              pcm_output -> hw:lg115x,6     ptts -> hw:lg115x,7
        hw:1,0..1,4)          |              precord/pvoipsource = null sources
                              v              [pcm_input = module-alsa-source hw:X,Y, loaded on demand]
     LGSIC card:  WoV PDM Mic | SpeakerFeedback | MixerCapture | SoundEngineCapture | BluetoothCapture
     lg115x card: playback 0-9, capture 10-14 (all closed now)

     Main TV audio (HDMI/broadcast/apps via starfish/UMI):  ADEC -> AMIXER -> sound engine -> amp
       controlled by audiooutputd (controlC0, i2c) through /dev/lg/*; bypasses PulseAudio and ALSA PCMs
```

Consequences:

- `pcm_output.monitor` and `ptts.monitor` only carry audio routed through PulseAudio (UI and system sounds, TTS, some apps). They **do not carry HDMI or broadcast audio**, which goes through ADEC and UMI. That makes them a weak general AEC reference. Confidence: inferred (HDMI playing while pulse is SUSPENDED is verified).
- `remote.monitor` is very likely the Magic Remote voice path: voiceinput links `libsbc` and audiod lists `remote.monitor` as a sound input. It is **not** the built-in far-field mic. Confidence: inferred.

## Who holds which sound device (snapshot)

| Process (exe) | uid | `/dev/snd` nodes held | Other audio-relevant fds or sockets | Confidence |
| --- | --- | --- | --- | --- |
| pulseaudio (`/usr/bin/pulseaudio`) | pulse | **none** (sinks suspended) | `/var/run/pulse/native`, `@palmaudio` | verified |
| audiod (`/usr/sbin/audiod`, `com.webos.service.audio` + `com.webos.audio`) | root | none | 35 sockets, a pulse memfd. Links libasound and libpulse. Contains a `tvCaptureManager` and scans `/dev/snd/pcmC*`. | verified (fds), inferred (role) |
| audiooutputd (`/usr/sbin/audiooutputd`) | root | `controlC0` x4 | `/dev/i2c-0..9`, `/dev/lg/{sys0,pm0,ucom0}` (amp and sound-engine control) | verified |
| wowplay (`/usr/sbin/wowplay`) | root | `controlC0` x1 | sockets | verified |
| extinput-integration | root | `controlC0` x2 | | verified |
| legacy-broadcast-dvb | root | `controlC0` x1 | `/dev/lg/*`, `/dev/lgmem` | verified |
| audpera (`/usr/bin/audpera -d`) | root | none | `/dev/lg/audio_uimc`, `/dev/aud_blind` | verified |
| voiceinput (`/usr/sbin/voiceinput`, `com.webos.service.voiceinput` + `voiceinputadapter`) | root | **none now** | 16 sockets, `/dev/lg/*`. Links `libasound`, `libaec-fe`, `libsnsr` (Sensory, "Hi LG"), `libpryon_lite` (Alexa wake word), `libawe`/`libaweWrapper` (DSP Concepts Audio Weaver AEC/beamforming), `libsbc`, `libsamplerate`. | verified (fds/libs) |
| voiceconductor, alwaysready, tts, nlpmanager, user-intent-manager, performer (jailed), lg.thinqai.adapter (jailed), amazon-alexa-adapter (jailed), musicrecognition, btaudiosrc, btaudiosnk, airplay, umediaserver, starfish-media-pipeline, asm, asc, avmutemanager, captureservice | various | none | sockets only (captureservice and avmutemanager hold video nodes) | verified |

**Owner of the capture PCMs:** nobody right now. The only shipped code found that opens the LGSIC capture PCMs directly is **voiceinput through `libaec-fe.so.1`**, which hard-codes the device strings `hw:1,0` to `hw:1,4`. It configures a "ref_dev" (the default when unknown is "SPK") and uses `snd_pcm_link` and `snd_pcm_htimestamp`, which points to linked, synchronously started mic plus reference capture for AEC. Confidence: verified (strings), inferred (runtime behavior). audiod can also open capture PCMs for its `tvCaptureManager` and can ask PulseAudio to open one as `pcm_input`. Confidence: inferred.

Exclusivity: every capture PCM has `subdevices_count: 1` and there is no `dsnoop` in the ALSA config. Whichever process opens `hw:LGSIC,0` first therefore gets it exclusively. Expect a conflict if LG hands-free voice is enabled while our client holds it. Confidence: inferred, not tested.

## LG audio and voice service surface (luna-service2)

Declared in `/usr/share/luna-service2/{services.d,roles.d,api-permissions.d,client-permissions.d,manifests.d}`. Confidence: verified (declarations).

| Service name(s) | Executable | Method groups (summary) |
| --- | --- | --- |
| `com.webos.service.audio`, `com.webos.audio` | audiod | `media` group: volume and mute, sound output, `getSoundInput`/`getSoundInputList`/`setSoundInput`/`connectSoundInput`, `getStreamStatus`, `playSound`, `tv/*` (amp, `switchMicrophone`, `ctrMicSwitch`, `getSoundCardsState`, `getHeadphoneStatus`, I2C read/write to amp, `setSoundEngineStatus`, ...), `UMI/*`, `master/*`. Strings also reveal `/capture`, `/capture/getStatus`. |
| `com.webos.service.audiooutput` | audiooutputd | `media` group: `audio/{connect,disconnect,getStatus,mute,setSoundOut,volume/*}`, `quality/*` |
| `com.webos.service.voiceinput`, `com.webos.service.voiceinputadapter` | voiceinput | `private` group: `getDevices`, `startStreaming`, `startStreamingWithTrigger` (schema only), `stopStreaming`, `enroll/*`, `setting/{get,set}{Activation,Sensitivity}`, `util/{startWebBSI,stopWebBSI,runByFile,subscribeFileOutput}`, `factory/{testMicSignal,getDataVersion}`, `tas/*`, adapter `getMessage`/`runTest`/`sendCommand`/`setInputEvent` |
| `com.webos.service.voiceconductor` | voiceconductor | `public`: `recordVoice`, `stopRecordingVoice`, `recognizeIntentByText`, `getVoiceUiEvents`, `interactor/*`, ... `private`: `recognizeVoice*`, `recognizeIntentByVoice`, `setInputEvent`, `getVoiceKey`, `debug/*`, ... |
| `com.webos.service.alwaysready` | alwaysready | `system`/`private`: `aod/{getStatus,setStatus}`, `timer/*`, `monitor/{getGPIOState,getDeviceState}`. Strings: `onAoDByWov` (Always-On-Display triggered by wake-on-voice). |
| `com.webos.service.tts` | tts (dynamic) | role only |
| `com.webos.service.nlpmanager`, `com.webos.service.userintentmanager`, `com.webos.service.voice.performer.plugin`, `lg.thinqai.adapter`, `amazon.alexa.adapter.*` | NLP and assistant plugins | private recognition, intent and TTS-data methods |
| `com.webos.service.wowplay` | wowplay | WOWCAST and soundbar |
| `com.webos.service.micomservice` | micomservice | includes `setWOVMode`/`updateWOVModeEnable` (wake-on-voice in the standby MCU) and `requestAlwaysReadyControlCommand` |
| `com.webos.service.soundshare`, `btaudiosnk`, `btaudiosrc`, `tsrecord`, `miracasttx`, `btgattwowcast` | | not voice-related |

Read-only getters called (confidence: verified). Identifiers are omitted.

- `com.webos.service.audio/getSoundInputList` returns `pcm_input`, `usb_input` and `remote.monitor`, all inactive and disconnected.
- `com.webos.service.audio/getSoundInput` returns `soundInput: ""`.
- `com.webos.service.audio/tv/getSoundCardsState` returns `soundCards: []`.
- `com.webos.service.audio/tv/getHeadphoneStatus` returns internal mic `TV` and external mic `NotConnected`.
- `com.webos.service.audio/getStreamStatus` and `UMI/getStatus` return one `umilivetv` HDMI stream on ADEC1 to MAIN.
- `com.webos.service.audiooutput/audio/getStatus` returns ADEC1 to MAIN, unmuted.
- `com.webos.service.voiceinput/getDevices` returns `devices: []`.
- `com.webos.service.voiceinput/factory/getDataVersion` returns a version string.
- `com.webos.service.voiceinput/setting/getActivation` reports everything off.
- `com.webos.service.alwaysready/monitor/getGPIOState` includes a `WOV_RL_ON` GPIO (value 255, meaning unknown).
- `getInputVolume` and `tv/getGainAsInput` need parameters, so they were not retried.

voiceinput static data (confidence: verified):

- `preDefinedDeviceList.json` lists capture devices named `LGSIC` (type `sound`, role `mix`), `O20 Builtin` (type `internal`), `Mars` (role `mixRTK`) and `testRemote` (hidraw).
- Sensory model `trigger_lge_en-US_v1.2_hi_lg_*.snsr` and an Audio Weaver tuning file `tunning_BM_V_1_0_10.awb`.
- `watchFolder.json` watches `/tmp/voiceinput_dump_setup` and `/tmp/voiceinput_disable_set_input_event`, which is probably a vendor debug-dump hook.

## Kernel side

- No loadable sound modules. The ASoC drivers (`soc-audio`, `lgsic-audio`) are built in. The only audio-adjacent modules are USB WLAN and BT. Confidence: verified.
- The `lgsic-audio` platform device exposes sub-nodes `PDMMic`, `SpeakerFeedback`, `MixerCapture`, `SoundEngineCapture` and `BluetoothCapture`. Confidence: verified.
- The dmesg ring buffer had rotated (WLAN log spam), so **no boot-time audio, DSP or firmware lines were available**. Confidence: verified (absence), unknown (DSP firmware).

## Candidate built-in mic sources (ranked)

1. **`hw:LGSIC,0` "WoV PDM Mic"**: strongest candidate. Its name means "wake-on-voice PDM mic", it has its own `PDMMic` platform node, and it sits on a separate card from the TV playback SoC card. voiceinput (Sensory, Pryon and Audio Weaver AEC) links a library that hard-codes `hw:1,0`. It is closed now, so it is available while LG hands-free is off. Likely raw multi-channel PDM-derived PCM, pre-AEC. Confidence: inferred.
2. **PulseAudio `pcm_input`, created on demand by audiod and module-palm-policy**: a sanctioned, shareable path. It is only useful if audiod points it at the LGSIC mic, and how a client triggers it is unknown. Confidence: inferred.
3. **`hw:lg115x,10..14`**: SoC-card captures of unknown purpose. They are more likely TV-audio or ADEC taps (useful as AEC references) than microphones. Confidence: unknown.
4. Not built-in mic sources: `remote.monitor` (Magic Remote audio over BT), `precord`/`pvoipsource` (null sources today), and `usb_input` (external USB mic).

AEC reference candidates, best first:

1. `hw:LGSIC,1` SpeakerFeedback. Plausibly the true post-processing speaker signal, including HDMI/ADEC audio, and the `ref_dev` default "SPK" in libaec-fe.
2. `hw:LGSIC,3` SoundEngineCapture.
3. `hw:LGSIC,2` MixerCapture.
4. PulseAudio `pcm_output.monitor` / `ptts.monitor`, which only carry pulse-routed audio.

All of these are inferred. None is measured yet.

## Residual unknowns

- Channel count, format and rate of every LGSIC PCM, and whether opening one disturbs voiceinput or the standby wake-on-voice MCU path.
- Whether the Magic Remote mic button uses the far-field mic at all, or only the BT `remote` path.
- What audiod passes to `pcm_input`, and which client action triggers it.
- Behavior in screen-off / Always Ready (`onAoDByWov`, micom `setWOVMode`) states.
- Whether the PDM mic delivers raw or vendor-processed audio.

## Proposed D3/D4 actions

Ordered from least to most invasive. Every step needs explicit user authorization. Never read `dapm_widget` or debugfs ASoC files. Wrap every command in `timeout`.

1. **D3-a: passive observation during native voice use.** No binaries or writes; the user presses the Magic Remote mic button and speaks.
   - Every 0.5 s for about 15 s, snapshot `/proc/asound/card*/pcm*/sub0/{status,hw_params}`, `ls -l /proc/$(pidof voiceinput)/fd`, and `pactl list short modules`, `sources` and `source-outputs`. Repeat once with the screen-saver/idle state.
   - Reveals which PCM opens (LGSIC,0 or not), its live channels, format and rate, its owner, and whether `pcm_input` or `module-alsa-source` appears.
   - Risk: low. Only `/proc` files that were already read safely are polled. Speech goes to LG's cloud as in normal use.
2. **D3-b: same observation with LG hands-free ("Hi LG") enabled.** The user toggles it in the Settings UI; that is reversible, so turn it off afterwards. This step needs the voice-recognition EULA/consent the user already manages in LG settings.
   - Shows whether voiceinput then holds `hw:LGSIC,0` (plus a reference PCM) **continuously and in RUNNING state**, which would make raw access exclusive and conflicting.
   - Risk: low. It changes an LG setting that the user can revert.
3. **D3-c: read ALSA mixer controls read-only:** `amixer -c LGSIC contents` and `amixer -c lg115x contents` (shipped binary).
   - Opens `controlC*`, which was not allowed in D2. Read-only ioctls change nothing, but this is the first time `/dev/snd` would be opened.
   - Look for mic gain, PDM enable, channel-select or reference-select controls. Do **not** run `amixer set`/`cset`.
   - Risk: low.
4. **D3-d: hardware capability probe without keeping audio:** `arecord -D hw:LGSIC,0 --dump-hw-params -d 1 -f S16_LE /dev/null`, then repeat for `hw:LGSIC,1..3` (shipped `arecord`; output discarded to `/dev/null`).
   - Run only while LG hands-free is OFF and no voice session is active.
   - Shows the supported channels, rates and formats.
   - Technically opens the mic for about 1 s. Risk: low to medium, with possible brief contention with voiceinput.
5. **D4-a: consented short mic recording.** The user and housemates consent and the room is quiet. Example parameters, to be set from D3-d: `arecord -D hw:LGSIC,0 -c <N> -r <rate> -f <fmt> -d 5 /tmp/d4a.raw`.
   - The user speaks a fixed phrase at about 2 m. Copy the file off, analyze per-channel energy, correlation and clipping, then **delete it on the TV and the host**. Do not commit it.
   - Answers which channels are real mics and whether the signal is raw or processed.
   - Risk: medium. It is a privacy-sensitive recording and needs `/tmp` space.
6. **D4-b: AEC reference identification.** HDMI audio plays at a known volume, with no speech.
   - Capture `hw:LGSIC,1`, `,2` and `,3` (5 s each, separately), and `pcm_output.monitor` via `parec` while a UI sound plays. Optionally capture `hw:LGSIC,0` at the same time to check delay and coherence.
   - Tells you which tap carries the HDMI/ADEC signal and its latency relative to the mic.
   - Risk: medium. TV program audio is recorded, which is not personal speech.
7. **D4-c: classify `hw:lg115x,10..14`** with a 2 s capture each during HDMI playback. Risk: low to medium.
8. **D4-d: syscall trace of voiceinput** during the remote mic button press and "Hi LG".
   - Needs a static aarch64 (compat-capable) or armhf `strace` copied to `/tmp`, inspected and checksummed first, and removed afterwards.
   - Run `strace -p <voiceinput> -e trace=openat,ioctl,read -tt` for about 20 s.
   - Confirms the device strings, the `hw_params` ioctls, and whether mic and reference are linked.
   - Risk: medium. ptrace slows voiceinput, a crash could restart voice services, and it writes to `/tmp`.
   - Prefer D3-a/D3-b first, since they may make this unnecessary.
9. **D4-e (optional, static analysis first):** use REA on `libaec-fe.so.1`, `module-palm-policy.so` and audiod to learn (a) how `ref_dev` maps to `hw:1,1..4`, (b) what triggers audiod to load `pcm_input`, and (c) what the `/tmp/voiceinput_dump_setup` hook dumps.
   - This step makes no device change. Creating the watch file later counts as a device write and needs separate authorization.
