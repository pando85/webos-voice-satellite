# Remote mic button: hold to talk (daemon side)

- **Status:** built and tested on the PC only (unit tests and host integration tests with a fake input device, a fake `luna-send` and a fake LG socket). **Not run on a TV, not seen with a real Home Assistant.** Off by default (`[remote] mic_button = "off"`). Written 2026-11-07.
- **Owner request:** "holding the Magic Remote's mic button talks to our HA assistant"; the daemon side only, independent of how LG's assistant is kept away, off by default.
- **Not in this note (not built):** the on-screen overlay; any change to LG's voice services. How the satellite and LG's assistant share the key is [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md), which is **proposed, not accepted** ([EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) has not run). Until it is built, **LG's assistant still opens on every press** when the feature is on (section 7).
- **Related:** [research note](../research/remote-mic-button.md), [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md) (the key and the stream, verified on c3-a and c3-b), [ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md), [ADR 0012](../decisions/0012-config-switches-in-ha.md) (the config switch pattern), [AEC front-end](aec-frontend.md), [ESPHome API crate](../development/esphome-api.md).

Labels follow [AGENTS.md](../../AGENTS.md): **verified-on-target** (EXP-022, LG C3, webOS TV 8.3), **verified-on-host** (the tests named in section 9), **hypothesis**.

## 1. What the user gets

With `mic_button = "ptt"`:

| Action | What happens |
| --- | --- |
| Hold the mic button | A run starts at speech-to-text (no wake word). The audio comes from the **remote's own microphone** while it is held (the TV microphone if the remote's stream is not there). |
| Release (held 300 ms or more) | The run's audio ends (`VoiceAssistantAudio{end: true}`): HA finalizes speech-to-text with what it has, then the intent and the spoken reply follow as usual. The remote's stream closes. |
| Tap (released before 300 ms) | **Tap-to-talk** on the **TV microphone**: the run goes on and Home Assistant's end-of-speech detection ends it, as with `SIGUSR1` today. The remote's stream closes at once. |
| Hold longer than 15 s | The audio ends at 15 s; the release later does nothing. |

The three numbers are `tap_ms`, `max_hold_s` (and `remote_mic` chooses the source) in `[remote]` (section 8).

**Tap decision and why.** A tap could also be ignored. It is kept as tap-to-talk because (a) a short press that does nothing looks broken, (b) the TV microphone run with HA's end-of-speech detection is the push-to-talk path that already ran live on c3-a ([P1 TV live](../discovery/c3-a/P1-tv-live.md)), and (c) closing the remote's stream at the release keeps one simple privacy rule: **the remote's microphone is live only while the button is held** (the stream itself does not stop at the release on webOS 8.3, EXP-022 B, so the satellite closes it). The cost: an accidental tap starts a run that listens to the room until HA's end-of-speech detection (or the 15 s cap) ends it. Set `tap_ms = 0` to make every release end the audio.

## 2. Press rules

At a press the daemon checks, in this order (a refusal is logged at `warn` as `remote mic button: pressed, refused: <why>` and its release does nothing):

1. Home Assistant is connected.
2. The microphone is not muted or gated (HA's mute switch, the kill switch, the firmware guard, "not paired"). The remote's microphone is a microphone too: the same gates apply.
3. No announcement plays (the voice layer refuses a run then).
4. The TV microphone delivers audio (the voice layer refuses a run without a live capture, see [capture liveness](../development/esphome-api.md#capture-liveness-appcommandcapturelive)).

Then:

- **Join:** a run past the wake word is already streaming (a continued conversation after `continue_conversation`, an earlier push-to-talk run, or speech after the wake word): no new run is started; the remote's audio takes over from its first chunk, and the release ends that run's audio. The conversation id is kept by the voice layer either way.
- **Start:** otherwise a push-to-talk run is requested (`AppCommand::StartPipeline { mode: PushToTalk }`, the existing path). The voice layer aborts the idle wake word session first and waits for its end ([voice layer](../development/esphome-api.md)); a reply that is playing is interrupted (barge-in, as with `SIGUSR1`); a ringing timer alarm stops (a run at speech-to-text stops it); music ducks as for any run.

A second press without a release is ignored; a press within 50 ms of the last release is a bounce and ignored with its release. While the remote's mic run is active the satellite reports `busy: voice` in `run/voice.json` like any run, so a config change waits (ADR 0012).

## 3. The button

- **Source (verified-on-target, c3-a and c3-b, EXP-022):** `EV_KEY` code 428, value 1 at the press and 0 at the release, no autorepeat seen, on the input device named `LGE M-RCU - Builtin [0]` (`event2` on both TVs that day).
- **Reader (`crates/satellite/src/evdev.rs`):** a thread finds the node by its **exact name** in `/sys/class/input/eventN/device/name` (the lowest N first), never by index, opens `/dev/input/eventN` **read-only and without `EVIOCGRAB`** (every other reader, LG's compositor first, keeps all events), and keeps only `EV_KEY` events of the one code. Records follow the kernel layout for the process (16 bytes for the armv7 build, 24 for aarch64). When the node goes away (read error or end of file) a held key is reported released and the device is looked up again after 1 s; while it is missing, every 5 s. No event content is logged.
- **Always passive:** the reader runs whatever `mic_button` says. With `"off"` a press is only logged at debug ("observed only"), which is ADR 0013 decision 1's "only observes".
- **Hypothesis:** that the uinput node `LGE M-RCU - Builtin [0]` stays while the remote's Bluetooth link comes and goes (EXP-022 saw `hidraw0` vanish, not the uinput node). If it does vanish, the reader finds it again.

## 4. The audio of a button run

### 4.1 The remote's microphone

- **Open (`crates/satellite/src/remote_mic.rs`):** at the press, `luna-send -i luna://com.webos.service.voiceinput/startStreaming '{"deviceType":"remote","subscribe":true}'` on a pseudo-terminal (the power watch's helper). EXP-022 B (c3-a): reply `{"subscribed":true,"returnValue":true,"socketPath":"/tmp/voiceinput_remote1.soc"}` 29 ms after the press. The socket path is **read from the reply** (an absolute path without `..` or control characters, at most 107 bytes), never assumed.
- **Refusals:** `{"errorCode":"2003","errorText":"audio data is already capturing",...}` was seen once on c3-a and cleared within 0.65 s. A refusal, no answer within 1.5 s or a `luna-send` that ends is retried twice, 300 ms apart (3 calls); then the run keeps the TV microphone.
- **Read:** a thread connects to the Unix socket (up to 1 s) and reads 16 kHz mono S16LE, the format that fits the measured 31.6 kB/s (EXP-022 B; the reply carries no format). Bytes are converted across read boundaries, passed through the remote's own front-end (section 4.3) and cut into 512-sample chunks, the size the TV microphone sends.
- **Format check:** the stream must deliver its first chunk within 1 s of the connection, and over the first second after that chunk its rate must lie between 0.6 and 1.5 times 16 000 samples per second (8 kHz, 32 kHz or stereo fall outside). Otherwise it is closed with a warning and the TV microphone takes over. The format is not adapted (no resampling): another format means another firmware, which needs a new experiment.
- **Close:** at the release, the cap, the run's end, the mute switch or a gate, an HA disconnect and the daemon's shutdown, the `luna-send` child is killed and reaped (which ends the subscription and LG's stream, as in EXP-022 B4) and the reader thread stops within 200 ms. The `RemoteMic` closes its session when dropped, too. `stopStreaming` is not called: its parameters are not known.

### 4.2 Switching between the two microphones

The TV microphone's capture thread keeps running all the time (the wake word needs it, and it is the fallback). A shared flag, `remote_feeds`, holds its chunks back while the remote feeds the run; the daemon then sends the remote's chunks itself, in order with `EndAudio` on the same command channel.

- The remote takes over **from its first chunk**, never earlier, so a stream that connects but stays silent costs no TV audio.
- Remote audio that arrives before the run streams (HA takes from tens of ms up to about 1.5 s after an aborted wake word session) is kept, at most 2 s, and sent first, so the first words are not lost. If the run already streams the TV microphone when the remote's first chunk arrives (a join, or a slow stream), the switch happens at that chunk without the kept audio (HA already has that time from the TV).
- A stream that fails, ends or fails the format check gives the run back to the TV microphone at once, also mid-run (a gap of up to one chunk).
- A hold released before HA started the run (a slow HA): the remote's audio kept so far is sent when the run starts, then the audio ends. Without kept audio (the stream never opened) the run is left to HA's end-of-speech detection.

### 4.3 Front-end and echo cancellation

The remote's audio goes through the same `webos_dsp::Frontend` type as the TV microphone, but its own instance: the high-pass filter of `dsp.hpf_hz` and a fixed gain of `[remote] gain_db` (default 0 dB; the TV microphone's +24 dB is for a far-field microphone and would clip: speech peaked at -4.9 dBFS in EXP-022 B). **No echo canceller**, for four reasons:

1. **Close talk:** speech was 24 dB above the floor on the remote (EXP-022 B), against 3 to 5 dB on the TV microphone at the D3 reference level; the TV's own sound is far quieter relative to the voice at the remote (hypothesis: not measured with TV content playing).
2. **Unknown, probably variable delay:** the remote's audio crosses Bluetooth and LG's `voiceinput` buffering, unlike the TV microphone whose reference is captured on the same card. AEC3 would have to find a new delay on every run of a few seconds (hypothesis).
3. **Structure:** the reference capture is opened together with the TV microphone's PCM in the capture thread; a second consumer would need its own reference stream.
4. **Little to cancel:** a press stops a reply that plays (section 2), so the satellite's own voice is rarely in the remote's audio; TV programme sound is.

Revisit if the on-TV check (section 10) shows TV sound reaching HA's speech-to-text through the remote.

## 5. The state machine

`crates/satellite/src/remote_ptt.rs`, `Hold`, pure (no I/O): inputs are the button, the voice layer's `StartStreaming` (without wake word), `StopStreaming` and `RunEnd`, the stream's events and time; outputs are `StartRun`, `EndAudio`, `OpenStream`, `CloseStream`, `Audio(chunk)` and `RemoteFeeds(bool)`. Phases: `Idle`, `Held` (with "our run streams yet"), `EndPending` (released before the run streamed), `Tapped`, `WaitRelease`. Before our run's `StartStreaming` arrives, a `StopStreaming` belongs to the session the press replaced and is ignored. `Button` wraps it for the daemon: it runs the stream actions, checks the session generation of each stream event (a late event of a closed session is dropped), logs, and calls the hand-over seam.

New in the ESPHome API crate: **`AppCommand::EndAudio`** ends the audio of the run that streams gracefully (`VoiceAssistantAudio{end: true}` to HA and `StopStreaming` to the application); the run is not aborted and the wake word stays armed. Ignored when nothing streams. HA's side (documented, research section 3.1): `handle_stop(False)` ends the audio stream and speech-to-text finalizes.

## 6. Privacy

- The remote's microphone is open only from the press to the release (or the cap, the run's end, mute, a gate, an HA disconnect, shutdown); a tap closes it at once. While it is open its audio goes only to the Home Assistant the satellite is paired with, as any run's audio does.
- **No audio is stored**; the kept audio before the run streams is at most 2 s in memory and dropped when not sent. Logs carry press and release times, hold length, refusal reasons, the socket path (no identifier, EXP-022), luna error codes, byte counts and levels; never audio, speech or transcripts.
- The "Listening" indicator (F7) stays tied to the TV microphone's PCM, which is open during a button run anyway; with the remote feeding, the TV PCM is open but its chunks are not sent. Whether to show the remote's microphone separately is part of the overlay work (not built).
- **LG:** until ADR 0013 is built and accepted, a press with the feature on also starts LG's assistant, which may send that speech to LG's cloud (research section 4). This is why the feature is off by default.

## 7. Seam for LG's hand-over (ADR 0013, proposed)

`trait LgVoiceKey` in `remote_ptt.rs`: `acquire()` once when the key device is found with the feature on (ADR 0013 decision 3's "the key node is found by name"), `run_started()` and `run_ended()` around each accepted button run (for the decision 2(b) refinement, a pause limited to the run), `release()` at shutdown. The only implementation is `LeaveToLg`, which changes nothing and logs that LG's assistant keeps reacting. The volatile configd override, the SIGSTOP fallback, the state file, `ctl.sh restore-lg-voice` and the supervisor hooks are **not built**; they plug in here after the owner accepts ADR 0013 and EXP-023 has run. Implementations must be idempotent and must not block the event loop.

## 8. Configuration and Home Assistant

`[remote]` in `etc/config.toml` (all keys in [`config/example.toml`](../../config/example.toml)), read at start only:

| Key | Default | Meaning |
| --- | --- | --- |
| `mic_button` | `"off"` | `"off"`: only observe the key. `"ptt"`: hold to talk. |
| `device_name` | `"LGE M-RCU - Builtin [0]"` | input device, exact name (1 to 128 characters) |
| `key_code` | `428` | `EV_KEY` code (1 to 767) |
| `remote_mic` | `true` | audio from the remote's microphone; `false`: TV microphone only |
| `max_hold_s` | `15` | cap after the press (1 to 60) |
| `tap_ms` | `300` | shorter releases are taps (0 to 2000, below the cap; 0: no taps) |
| `gain_db` | `0.0` | gain on the remote's microphone (-20 to 30) |

- **Home Assistant:** a config switch **"Remote mic button"** (object id `remote_mic_button`, `mdi:remote`, entity category `config`), after "Echo cancellation" and "Music playback", following ADR 0012: a flip runs `ctl.sh config mic-button on|off` detached, which writes `mic_button = "ptt"` or `"off"` in `[remote]` (in place, or the table appended), checks the config, restarts the daemon and rolls back on failure, with the same cooldown and busy guards. Adding a third switch to ADR 0012's fixed table is the owner's request of 2026-11-07; ADR 0012 itself is not edited here.
- **ctl.sh:** `ctl.sh config mic-button on|off`; `ctl.sh status` prints "remote mic button: off|ptt" and `status --json` has `"mic_button":"off"|"ptt"`. The TV app does not show it yet.
- `--check-config` prints `remote=[mic_button=off device="..." key=428]` or `remote=[mic_button=ptt ... mic=remote max_hold_s=15 tap_ms=300 gain_db=0]` (what `ctl.sh` reads).

## 9. Tests (verified-on-host)

- `remote_ptt::tests` (24): hold with kept audio and release, tap, tap cap, `tap_ms = 0`, cap while held, a run that never starts, each refusal (HA, mute, announcement, TV microphone) and its swallowed release, join, the replaced run's stop, release before the run streams (with and without kept audio), TV-microphone-only holds, stream failure before and during the run, no data, wrong rate, the 2 s limit of the kept audio, halt, a late open, duplicates and bounces, a new press over a tapped run, the hand-over seam's call order, `"off"` touching nothing, and the fallback without `luna-send`.
- `evdev::tests`: record layout, lookup by exact name (not by index), key filtering, a fake device file with a press, a release, another key and a held press lost at the end of the file, a missing device.
- `remote_mic::tests`: reply parsing (the c3-a lines), socket path checks, S16LE conversion across reads, and against a fake `luna-send` and a fake LG socket: a `2003` refusal retried then a stream of chunks at the right level, the subscription child killed and reaped on close; three refusals reported lost; no `luna-send`; no listener; the end of the stream; a dropped `RemoteMic` ending its child.
- `capture::tests::the_tv_microphone_is_held_back_while_the_remote_feeds_the_run`; `voice::tests::end_audio_*` (ESPHome API crate); `config::tests::remote_section_*`; `config_switch` tests with the third switch; `packaging/webos/tests/tv_settings.sh` (`config mic-button`, status fields).

## 10. What needs checking on a TV (not done)

On c3-a, with the owner's authorization and consent (LG's assistant also hears each press until ADR 0013 is built):

1. The daemon finds `LGE M-RCU - Builtin [0]` and logs `watching ... passive read`; the remote, the pointer and LG's compositor behave as before.
2. Hold, speak, release: HA's debug view shows a run at speech-to-text with the transcript; the log shows `remote mic: stream open`, the release time, `subscription closed`, and no `luna-send` child is left (`ps`).
3. The format check passes (no "samples/s" warning); speech-to-text quality on the remote against the TV microphone, with TV sound playing (section 4.3, AEC).
4. Tap: the run ends by HA's end-of-speech detection; the stream closed at once.
5. A 2003 refusal or LG's "audio processing is unstable": how often, and whether the TV-microphone fallback carries the run.
6. Mute in HA, then press: refused and logged, no stream opened.
7. The 15 s cap; a press during a reply and during a continued conversation.
8. CPU of the reader threads (expected idle while nothing is pressed).
