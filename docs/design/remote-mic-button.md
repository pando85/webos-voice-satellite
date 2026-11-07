# Remote mic button: hold to talk (daemon side)

- **Status:** built and tested on the PC only (unit tests and host integration tests with a fake input device, a fake `luna-send` and a fake LG socket). **Not run on a TV, not seen with a real Home Assistant.** Off by default (`[remote] mic_button = "off"`), and off is inert: nothing of the feature is started. Written 2026-11-07; revised the same day after an independent review of `785d25c` (section 11), and again for the LG hand-over (section 7), which is built and host-tested but has **never run on a TV**: its first use is the owner's first enable session ([EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) C1).
- **Owner request:** "holding the Magic Remote's mic button talks to our HA assistant"; the daemon side only, independent of how LG's assistant is kept away, off by default.
- **Coupled to LG's assistant (owner, 2026-11-07, [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md) accepted for this path):** "We cannot have our feature enabled without disabling the LG one. Option must be in the UI and in esphome." The feature runs **only** while LG's voice key is taken over by an in-memory configd override (section 7); if that fails, the feature stays off and says why. The `voiceconductor` pause (ADR 0013 decision 2(b)) is not built.
- **Not in this note (not built):** the on-screen overlay.
- **Related:** [research note](../research/remote-mic-button.md), [EXP-022](../experiments/EXP-022-remote-mic-button-observe.md) (the key and the stream, verified on c3-a and c3-b), [ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md), [ADR 0012](../decisions/0012-config-switches-in-ha.md) (the config switch pattern), [AEC front-end](aec-frontend.md), [ESPHome API crate](../development/esphome-api.md).

Labels follow [AGENTS.md](../../AGENTS.md): **verified-on-target** (EXP-022, LG C3, webOS TV 8.3), **verified-on-host** (the tests named in section 9), **hypothesis**.

## 1. What the user gets

With `mic_button = "ptt"`:

| Action | What happens |
| --- | --- |
| Hold the mic button | A run starts at speech-to-text (no wake word). The audio comes from the **remote's own microphone** while it is held (the TV microphone if the remote's stream is not there). |
| Release (held 300 ms or more) | After 250 ms more audio (what is still in flight from the remote and LG's buffer), the run's audio ends (`VoiceAssistantAudio{end: true}`): HA finalizes speech-to-text with what it has, then the intent and the spoken reply follow as usual. The remote's stream closes. |
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

- **Join:** speech-to-text audio is streaming now (a continued conversation after `continue_conversation`, an earlier push-to-talk run, or speech after the wake word): no new run is started; the remote's audio takes over from its first chunk, and the release ends that run's audio. The conversation id is kept by the voice layer either way. "Streaming speech-to-text" is tracked explicitly by the daemon: set by `StartStreaming` without the wake word and by `WakeWordDetected`, cleared by `StopStreaming`, `RunEnd` and a disconnect. (The first version used the daemon's `in_run`, which stays set when the voice layer aborts a run and swallows HA's `RUN_END`; a press then "joined" the idle wake word session and sent remote audio to openWakeWord.)
- **Start:** otherwise a push-to-talk run is requested (`AppCommand::StartPipeline { mode: PushToTalk }`, the existing path). The voice layer aborts the idle wake word session first and waits for its end ([voice layer](../development/esphome-api.md)); a reply that is playing is interrupted (barge-in, as with `SIGUSR1`); a ringing timer alarm stops (a run at speech-to-text stops it); music ducks as for any run.
- **Only that run:** the voice layer marks the run its push-to-talk request started (`StartStreaming { push_to_talk: true }`) and reports a request that will not stream (`PushToTalkFailed`: refused while muted, without live capture, during an announcement or unsubscribed; or its start failed, timed out, or was dropped by an announcement, mute, capture loss, a disconnect or `StopPipeline`). The button takes over only the marked run; on the failure report, or when nothing streams within 12 s (`START_WAIT`: the voice layer's 10 s start timeout plus an aborted run's 1.5 s end), the press is given up. So a later continued-conversation, `start_conversation` or announcement run is never fed stale audio or ended by an old press.

A second press without a release is ignored; a press within 50 ms of the last release is a bounce and ignored with its release. While the remote's mic run is active the satellite reports `busy: voice` in `run/voice.json` like any run, so a config change waits (ADR 0012).

## 3. The button

- **Source (verified-on-target, c3-a and c3-b, EXP-022):** `EV_KEY` code 428, value 1 at the press and 0 at the release, no autorepeat seen, on the input device named `LGE M-RCU - Builtin [0]` (`event2` on both TVs that day).
- **Reader (`crates/satellite/src/evdev.rs`):** a thread finds the node by its **exact name** in `/sys/class/input/eventN/device/name` (the lowest N first), never by index, opens `/dev/input/eventN` **read-only and without `EVIOCGRAB`** (every other reader, LG's compositor first, keeps all events), and keeps only `EV_KEY` events of the one code. Records follow the kernel layout for the process (16 bytes for the armv7 build, 24 for aarch64). No event content is logged.
- **Times:** each press and release carries the kernel's event time, and the state machine gets the hold length the kernel measured (release time minus press time) for the tap-or-hold decision, so an event handled late (a busy event loop, before or after the press) does not change it. Without both kernel times, or across a clock step (the release before the press, or a hold over 120 s), the daemon's own clock from the press's handling decides.
- **Lost events:** `SYN_DROPPED` (the kernel's buffer overflowed) discards events up to the next `SYN_REPORT` and reports a held key released; a press while the key is already down (a missed release) is reported as a lost release followed by the press. A lost release ends the hold like a release but does not start the 50 ms bounce window.
- **Device loss:** when the node goes away (read error or end of file) a held key is reported released (lost) and the device is looked up again after 1 s, doubling to 30 s for losses in a row, with a log line only at the 1st, 2nd, 4th, 8th ... loss; while it is missing, every 5 s, logged once. The uinput node `LGE M-RCU - Builtin [0]` survives the remote's Bluetooth disconnect (EXP-022 saw `hidraw0` vanish, not the uinput node), so a disconnect while held gives no release: the 15 s cap ends that hold.
- **Off is inert:** with `mic_button = "off"` the daemon starts nothing of the feature: no reader thread, no input device opened, no machine, no remote microphone. A change of `mic_button` restarts the daemon anyway (section 8). (The first version kept a passive reader running when off; the owner asked for off to be inert.)

## 4. The audio of a button run

### 4.1 The remote's microphone

- **Open (`crates/satellite/src/remote_mic.rs`):** at the press, `luna-send -i luna://com.webos.service.voiceinput/startStreaming '{"deviceType":"remote","subscribe":true}'` on a pseudo-terminal (the power watch's helper). EXP-022 B (c3-a): reply `{"subscribed":true,"returnValue":true,"socketPath":"/tmp/voiceinput_remote1.soc"}` 29 ms after the press. The socket path is **read from the reply** (an absolute path without `..` or control characters, at most 107 bytes), never assumed.
- **Refusals:** `{"errorCode":"2003","errorText":"audio data is already capturing",...}` was seen once on c3-a and cleared within 0.65 s. A refusal, no answer within 1.5 s or a `luna-send` that ends is retried twice, 300 ms apart (3 calls); then the run keeps the TV microphone.
- **Read:** a thread connects to the Unix socket (up to 1 s) and reads 16 kHz mono S16LE, the format that fits the measured 31.6 kB/s (EXP-022 B; the reply carries no format). Bytes are converted across read boundaries, passed through the remote's own front-end (section 4.3) and cut into 512-sample chunks, the size the TV microphone sends.
- **Format and liveness checks:** the stream must deliver its first chunk within 1 s of the connection; over the first second after that chunk its byte rate must lie between 0.6 and 1.5 times 32 000 bytes per second (this catches 8 kHz mono and 32 kHz mono or 16 kHz stereo; it cannot tell S16LE from another two-byte format or byte order at the same rate); and once it delivers, a gap of 500 ms without a chunk is a stall. Any of these closes it with a warning and the TV microphone takes over. The format is not adapted (no resampling): another format means another firmware, which needs a new experiment.
- **Close:** at the release (after the grace), the cap, the run's end, the mute switch or a gate, an HA disconnect and the daemon's shutdown, the `luna-send` child is killed and reaped (which ends the subscription and LG's stream, as in EXP-022 B4) and the reader thread stops within 200 ms. The `RemoteMic` closes its session when dropped, too; at shutdown the daemon waits (3 s at most) until every closed session's child is reaped. `stopStreaming` is not called: its parameters are not known.
- **Crash safety:** every `luna-send` subscription child (this one and the power watch's) is started with `PR_SET_PDEATHSIG = SIGKILL`, so it dies with the daemon also when the daemon cannot kill it (a panic abort, the watchdog's exit, SIGKILL); a leftover `voiceinput` subscription could otherwise hold LG's remote stream until a reboot. This needs a `pre_exec` hook, the satellite crate's only `unsafe` (lint `unsafe_code = "deny"` with one `#[allow]` in `power::start_child`, the pattern the capture crate already uses for its ALSA ioctls).

### 4.2 Switching between the two microphones

The TV microphone's capture thread keeps running all the time (the wake word needs it, and it is the fallback). A shared flag, `remote_feeds`, holds its chunks back while the remote feeds the run; the daemon then sends the remote's chunks itself, in order with `EndAudio` on the same command channel.

- The remote takes over **from its first chunk**, never earlier, so a stream that connects but stays silent costs no TV audio.
- Remote audio that arrives before the run streams (HA takes from tens of ms up to about 1.5 s after an aborted wake word session) is kept, at most 2 s, and sent first, so the first words are not lost. If the run already streams the TV microphone when the remote's first chunk arrives (a join, or a slow stream), the switch happens at that chunk without the kept audio (HA already has that time from the TV).
- A stream that fails, ends, stalls or fails the format check gives the run back to the TV microphone at once, also mid-run (a gap of up to one chunk, or the 500 ms of a stall).
- A hold released before HA started the run (a slow HA): the remote's audio kept so far is sent when the run starts, then the audio ends. Without kept audio (the stream never opened) the run is left to HA's end-of-speech detection.

### 4.3 Front-end and echo cancellation

The remote's audio goes through the same `webos_dsp::Frontend` type as the TV microphone, but its own instance: the high-pass filter of `dsp.hpf_hz` and a fixed gain of `[remote] gain_db` (default 0 dB; the TV microphone's +24 dB is for a far-field microphone and would clip: speech peaked at -4.9 dBFS in EXP-022 B). **No echo canceller**, for four reasons:

1. **Close talk:** speech was 24 dB above the floor on the remote (EXP-022 B), against 3 to 5 dB on the TV microphone at the D3 reference level; the TV's own sound is far quieter relative to the voice at the remote (hypothesis: not measured with TV content playing).
2. **Unknown, probably variable delay:** the remote's audio crosses Bluetooth and LG's `voiceinput` buffering, unlike the TV microphone whose reference is captured on the same card. AEC3 would have to find a new delay on every run of a few seconds (hypothesis).
3. **Structure:** the reference capture is opened together with the TV microphone's PCM in the capture thread; a second consumer would need its own reference stream.
4. **Little to cancel:** a press stops a reply that plays (section 2), so the satellite's own voice is rarely in the remote's audio; TV programme sound is.

Revisit if the on-TV check (section 10) shows TV sound reaching HA's speech-to-text through the remote.

## 5. The state machine

`crates/satellite/src/remote_ptt.rs`, `Hold`, pure (no I/O): inputs are the button, the voice layer's `StartStreaming { push_to_talk: true }`, `PushToTalkFailed`, `StopStreaming` and `RunEnd`, the stream's events and time; outputs are `StartRun`, `EndAudio`, `OpenStream`, `CloseStream`, `Audio(chunk)` and `RemoteFeeds(bool)`. Phases: `Idle`, `Held` (with "our run streams yet"), `Releasing` (the 250 ms grace after a hold's release), `EndPending` (released before the run streamed), `Tapped`, `WaitRelease`. Before our run's `StartStreaming` arrives, a `StopStreaming` belongs to the session the press replaced and is ignored. `Button` wraps it for the daemon: it turns the kernel event times into the release time, runs the stream actions, checks the session generation of each stream event (a late event of a closed session is dropped), logs, and calls the hand-over seam.

New in the ESPHome API crate:

- **`AppCommand::EndAudio`** ends the audio of the run that streams gracefully (`VoiceAssistantAudio{end: true}` to HA and `StopStreaming` to the application); the run is not aborted and the wake word stays armed. Ignored when nothing streams. HA's side (documented, research section 3.1): `handle_stop(False)` ends the audio stream and speech-to-text finalizes.
- **`StartStreaming { push_to_talk }`** is true only for the run of the last `StartPipeline(PushToTalk)`; **`AppEvent::PushToTalkFailed`** is sent once for a request that will not reach it (section 2). A newer request replaces a waiting one without a report.

## 6. Privacy

- The remote's microphone is open only from the press to the release (or the cap, the run's end, mute, a gate, an HA disconnect, shutdown); a tap closes it at once. While it is open its audio goes only to the Home Assistant the satellite is paired with, as any run's audio does.
- **No audio is stored**; the kept audio before the run streams is at most 2 s in memory and dropped when not sent. Logs carry press and release times, hold length, refusal reasons, the socket path (no identifier, EXP-022), luna error codes, byte counts and levels; never audio, speech or transcripts.
- The "Listening" indicator (F7) stays tied to the TV microphone's PCM, which is open during a button run anyway; with the remote feeding, the TV PCM is open but its chunks are not sent. Whether to show the remote's microphone separately is part of the overlay work (not built).
- **LG:** with the feature running, LG's assistant does not start from the mic button (the key filter consumes it, static reading; to be confirmed in EXP-023 C1), so a press sends nothing to LG's cloud. If the takeover fails the feature does not run at all, so there is no state where both assistants hear a press.

## 7. LG's voice key: taken over while the feature runs (ADR 0013)

The mechanism is LG's own (research section 9, static): the compositor's key filter (`KeyFilters/appLaunch.js`) forwards the mic key to LG's assistant only while the configd value `com.webos.keyaction.voice` has `"action":"launch"`; with any other action it shows "This function is not supported." and consumes the key. LG ships `"ignore"` for hotel mode. The satellite sets that value **in memory only**.

**Where:** the state file `run/lg_voice_override` and the lock `run/lg_voice.lock` are always under the install (`<prefix>/run`), whatever `[report] dir` says, because `ctl.sh`, the supervisor and `status` look there (review of `506cbe7`). Acquire, every release by the daemon and every `ctl.sh` restore hold the lock (`flock(2)`: the daemon through `rustix`, `ctl.sh` through `python3`, else `flock(1)`), so no restore interleaves with an acquire. The state file holds `configd <epoch> <token>`; a restore removes it only if it still holds what that restore read at its start.

**Acquire** (`crates/satellite/src/lg_voice.rs`, daemon start with `mic_button = "ptt"`, after every start-up step that can fail, so no early return leaves the key taken over):

1. The key device must be there (found by name), and `luna-send` must be found; otherwise the feature does not run.
2. A state file left by an earlier run (`run/lg_voice_override`, a crash) is restored first.
3. `getConfigs {"configNames":["com.webos.keyaction.voice"]}` must read `launch` (c3-a's base value); anything else is left alone and the feature does not run.
4. configd's persistent databases `/var/preferences/configd_factory_db.json` and `/var/preferences/configd_db.json` are looked at: existence, size, modification time (metadata only; never opened, written or removed).
5. The state file `run/lg_voice_override` (`configd <epoch> <token>`) is written **before** the change.
6. `setConfigs {"configs":{"com.webos.keyaction.voice":{"action":"ignore"}},"volatile":true}`.
7. Read back: the action must be `ignore`.
8. Both databases must be as in step 4. They are looked at again **5 s later** and at the release: a change then is logged as an error, the override is removed at once, the mic button stops, and the reason goes to `voice.json`, `ctl.sh status` and HA (switch off, "LG voice key" sensor with the reason).

If 6 to 8 fail, the override is removed again (`null`, volatile) and read back; the feature stays off. Only the one config name is ever read (never the `voiceframework` category, which returns LG's security keys) and nothing is written without `"volatile":true`. Calls go through `luna-send -n 1` on a pseudo-terminal (as all the daemon's calls). Each `setConfigs` also leaves a `/tmp/configd_<time>_before_setConfigs.json` and may add a timestamp to `configd_debug_db.json` (upstream source; EXP-023 records whether this firmware does).

**Release:** `setConfigs` with `null` (volatile), then a read-back that must say **`launch`**, LG's base value, the only value an acquire starts from. Anything else (still `ignore`, the key reported in `missingConfigs`, `null`, an unreadable reply) is no success: the state file stays for the next path and `lg_voice_key` reads `unknown` with the reason. The paths:

| When | Who |
| --- | --- |
| Daemon shutdown (SIGTERM: `ctl.sh stop`, a config change's restart, an update) | the daemon itself |
| Any daemon exit, also a crash, SIGKILL, the watchdog or a panic abort | the supervisor (`supervise.sh`), before any restart and before supervision ends: `ctl.sh restore-lg-voice` when the state file exists. A stop that has to SIGKILL the supervisor kills its whole process group (it leads one: `start.sh` uses `setsid`), so no orphaned restore can run on and remove a newer acquire's state file; the luna call it may have started is bounded by its own timeout (6 s) and removes nothing |
| `ctl.sh stop`, `restart`, `install` (update), `uninstall`, `purge`, `config` (its restart) | `ctl.sh`, after it stopped the daemon (a failure in `uninstall` or `purge` says "reboot the TV", since `ctl.sh` goes away) |
| By hand | `ctl.sh restore-lg-voice`: repeatable (nothing to do without the state file), one JSON line `{"schema":1,"restore":"done|nothing|failed|refused",...}`, refused while the daemon runs (it holds the key: turn the feature off instead) |
| The feature turned off | the next daemon start (with `"off"`) gives back an override left by an earlier run; without `luna-send` it reports `unknown` |
| A TV restart | configd itself: the in-memory value is gone; the state file then only makes the next start do a harmless restore |

Removing the app package runs no script of ours (`packaging/webos/tv/appinstall.sh`: the installer's remove runs nothing of the app), so `ctl.sh uninstall` or `purge` must come first, as for the boot hook; the TV app's Uninstall runs `ctl.sh uninstall`. A crashed daemon that is not supervised (started by hand without `--supervise`) leaves the override until `ctl.sh restore-lg-voice` or a TV restart.

**Reporting:** `run/voice.json` gets `"mic_button"` (`off`, `ptt` running, `failed`), `"lg_voice_key"` (`lg`, `ours (configd)`, `unknown`) and `"mic_button_error"` (its quotes and backslashes written as `'`, so `ctl.sh` reads it whole). HA's switch "Remote mic button" shows **what runs**: on only with the override in place; a failed enable comes back off (the same resend as a refused flip, ADR 0012), and a diagnostic text sensor **"LG voice key"** (`lg_voice_key`) says "LG assistant", "Satellite (LG ignores the key)" or "LG assistant; mic button off: <reason>". `ctl.sh status --json` adds `mic_button_active`, `mic_button_error` and `lg_voice_key`.

**Retry and off:** with `ptt` configured but the takeover failed, an "on" (HA, app, `ctl.sh config mic-button on`) is a retry: `ctl.sh` compares with what runs, not only with the config, so it restarts the daemon with the same config. An "off" compares with the config, so it writes `off`.

## 8. Configuration and Home Assistant

`[remote]` in `etc/config.toml` (all keys in [`config/example.toml`](../../config/example.toml)), read at start only:

| Key | Default | Meaning |
| --- | --- | --- |
| `mic_button` | `"off"` | `"off"`: nothing is started. `"ptt"`: hold to talk. |
| `device_name` | `"LGE M-RCU - Builtin [0]"` | input device, exact name (1 to 128 characters) |
| `key_code` | `428` | `EV_KEY` code (1 to 767) |
| `remote_mic` | `true` | audio from the remote's microphone; `false`: TV microphone only |
| `max_hold_s` | `15` | cap after the press (1 to 60) |
| `tap_ms` | `300` | shorter releases are taps (0 to 2000, below the cap; 0: no taps) |
| `gain_db` | `0.0` | gain on the remote's microphone (-20 to 30) |

- **Home Assistant:** a config switch **"Remote mic button"** (object id `remote_mic_button`, `mdi:remote`, entity category `config`), after "Echo cancellation" and "Music playback", following ADR 0012: a flip runs `ctl.sh config mic-button on|off` detached, which writes `mic_button = "ptt"` or `"off"` in `[remote]` (in place, or the table appended), checks the config, restarts the daemon and rolls back on failure, with the same cooldown and busy guards. Adding a third switch to ADR 0012's fixed table is the owner's request of 2026-11-07; ADR 0012 itself is not edited here.
- **ctl.sh:** `ctl.sh config mic-button on|off`; `ctl.sh status` prints "remote mic button: off|ptt", whether it runs and the reason, and "LG voice key: ..."; `status --json` has `"mic_button":"off"|"ptt"` (the config; a single-quoted TOML value, `mic_button = 'ptt'`, is read the same; a change replaces it in place), `mic_button_active`, `mic_button_error` and `lg_voice_key`. `ctl.sh restore-lg-voice` (section 7).
- **TV app:** Home, Sound, a third row **"Remote mic button → Home Assistant"** with the note "Replaces LG's voice assistant on the mic button while on. LG's toast "not supported" may appear." It runs the same `ctl.sh config mic-button on|off` (two presses, a restart) and shows On only when the running satellite reports it (`mic_button_active` true): "Checking..." right after enabling or while the satellite has not said, "Not running" when the satellite is stopped (OK twice turns it off), "Off (LG kept it)" with the reason after a failed takeover (OK twice retries); turning it off asks "Give the mic button back to LG?". The app is English only. The phone setup page does not offer it (a restart ends the phone session, review C7).
- `--check-config` prints `remote=[mic_button=off device="..." key=428]` or `remote=[mic_button=ptt ... mic=remote max_hold_s=15 tap_ms=300 gain_db=0]` (what `ctl.sh` reads).

## 9. Tests (verified-on-host)

- `remote_ptt::tests` (31, the hand-over seam test replaced by the daemon and `lg_voice` tests below): hold with kept audio, the release grace (audio still forwarded, HA ending it during the grace), tap, tap cap, `tap_ms = 0`, cap while held, the start limit (held, released, tapped), `PushToTalkFailed` in every waiting phase (no foreign run fed or ended), each refusal (HA, mute, announcement, TV microphone) and its swallowed release, join, the replaced run's stop, release before the run streams (with and without kept audio), TV-microphone-only holds, stream failure before and during the run, no data, a stall mid-run and before the run, wrong rate, the 2 s limit of the kept audio, halt, a late open, duplicates and bounces, a lost release (no bounce window), a new press over a tapped run, the kernel-measured hold (also with a press handled late), and through `Button`: a hold, the kernel-time tap decision, the fallback without `luna-send`, a failed push-to-talk.
- `evdev::tests`: record layout with the kernel time, lookup by exact name (not by index), key filtering with times, a missed release and `SYN_DROPPED`, the loss back-off and its log rate, a fake device file with a press, a release, another key and a held press lost at the end of the file, a missing device.
- `remote_mic::tests`: reply parsing (the c3-a lines), socket path checks, S16LE conversion across reads, and against a fake `luna-send` and a fake LG socket: a `2003` refusal retried then a stream of chunks at the right level, the subscription child reaped on shutdown, **reaped when shutdown follows a close** (review finding 3); three refusals reported lost; no `luna-send`; no listener; the end of the stream; a new open or a drop ending the old child.
- `power::tests::a_subscription_child_dies_with_the_thread_that_spawned_it` (`PR_SET_PDEATHSIG`, with `kill_on_drop` bypassed).
- `daemon::tests::the_mic_button_joins_only_while_speech_to_text_streams` (the stale `in_run` case), `daemon::tests::mic_button_off_starts_nothing`, `mic_button_on_takes_lgs_key_over_and_gives_it_back` and `a_refused_takeover_leaves_lg_as_it_was_and_off_gives_back_a_leftover` (a fake `luna-send` modelling configd, through the real pseudo-terminal call), `mic_button_on_without_a_takeover_does_not_run_and_says_why`.
- `lg_voice::tests` (a fake configd): success (state file first, set, read-back, release with `null`), a set failure, a read-back mismatch, a persistent database created by configd, a change of `configd_db.json` (at acquire and found by the later check), success only with `launch` read back (`missingConfigs` and an unreadable reply keep the state file), a value other than `launch` left alone, a crash leftover restored before acquiring, an unconfirmed restore keeping the state file, a state file rewritten during a restore kept (token), acquire waiting for the lock; every payload volatile and naming the one key; `missingConfigs` and other reply shapes parsed.
- `daemon::tests::the_state_file_lives_under_the_install_not_the_report_dir`, `off_without_luna_send_reports_a_leftover_as_unknown`, `a_persistent_change_after_the_takeover_stops_the_mic_button_loudly` (switch off and sensor reason to HA, override removed).
- `config_switch::tests::a_failed_mic_button_takeover_can_be_retried_and_turned_off`; `voice_status::tests::the_mic_button_fields_come_after_busy_and_before_music`.
- `packaging/webos/tests/tv_settings.sh`: `restore-lg-voice` (nothing, done, repeatable, unconfirmed and refused restores keep the state file, no `luna-send`, refused while the daemon runs, only volatile writes and the one name), `ctl.sh stop`, `uninstall` and `purge` giving the key back, `status --json` and text fields, the retry and the "off" of a failed takeover. `tv_settings.sh` also: `missingConfigs` after the restore is no success, the token keeps a newer state file, a restore waits for the lock. `run.sh`: a SIGKILLed daemon holding the key, the supervisor giving it back before the restart; a hung restore in the supervisor killed with its process group by a stop, the state file untouched. `app_matrix.js`: the mic row (off, running, failed with the reason and retry, "Not running" while stopped, "Checking..." without an answer, never On without `mic_button_active` true), its texts, two presses on and off, a failed takeover and the retry, focus order.
- `voice::tests` (ESPHome API crate): `end_audio_*`, `only_the_push_to_talk_run_is_marked_and_its_follow_up_is_not`, `a_refused_push_to_talk_is_reported`, `a_failed_or_dropped_push_to_talk_start_is_reported_once`, `a_second_push_to_talk_replaces_the_first_silently`.
- `capture::tests::the_tv_microphone_is_held_back_while_the_remote_feeds_the_run`; `config::tests::remote_section_*`; `config_switch` tests with the third switch; `packaging/webos/tests/tv_settings.sh` (`config mic-button`, single-quoted values, status fields).

## 10. What needs checking on a TV (not done)

On c3-a, with the owner's authorization and consent. The LG hand-over steps are EXP-023's [C1 through the satellite](../experiments/EXP-023-remote-mic-button-handover.md#c1-through-the-satellite-the-owners-first-enable-session): enable from the app, the log line `LG's voice key taken over`, no LG screen on a tap, the factory database absent, other keys normal, disable and LG's screen back, and optionally a SIGKILL with the supervisor's restore. Then:

1. The daemon finds `LGE M-RCU - Builtin [0]` and logs `watching ... passive read`; the remote, the pointer and LG's compositor behave as before.
2. Hold, speak, release: HA's debug view shows a run at speech-to-text with the transcript; the log shows `remote mic: stream open`, the release time, `subscription closed`, and no `luna-send` child is left (`ps`).
3. The format check passes (no "samples/s" warning); speech-to-text quality on the remote against the TV microphone, with TV sound playing (section 4.3, AEC).
4. Tap: the run ends by HA's end-of-speech detection; the stream closed at once.
5. A 2003 refusal or LG's "audio processing is unstable": how often, and whether the TV-microphone fallback carries the run.
6. Mute in HA, then press: refused and logged, no stream opened.
7. The 15 s cap; a press during a reply and during a continued conversation.
8. CPU of the reader threads (expected idle while nothing is pressed).

## 11. Review of `785d25c` and what changed

An independent review (2026-11-07) found the feature safe to deploy **off** and these problems **on**; all are fixed as described above:

1. A stale `in_run` let a press join the idle wake word session (section 2, "Join").
2. A refused or failed push-to-talk start left the machine waiting, so it could take over a later continued-conversation, `start_conversation` or `SIGUSR1` run (section 2, "Only that run"; voice crate events in section 5).
3. Shutdown did not wait for the `luna-send` child of a session the machine had already closed (section 4.1, "Close").
4. A `luna-send` subscription could outlive a crashed daemon (section 4.1, "Crash safety").
5. The release cut audio still in flight (the 250 ms grace).
6. A stream that stopped mid-run was not noticed (the 500 ms stall).
7. The rate check's comment claimed more than it checks (section 4.1).
8. `SYN_DROPPED` and a missed release could leave the key stuck (section 3).
9. The tap/hold decision used the read time, not the kernel's event time (section 3). A first fix rebuilt a local release instant from the kernel times and fell back to the read time when that instant lay after "now" (a press handled late), so a 300 ms hold could still count as a tap; the re-review asked for the kernel-measured duration to be passed to the machine directly, which it now is.
10. A device that kept failing was retried and logged every second (section 3).

Owner priority added at the same time: off is inert (no reader), and `ctl.sh` reads a single-quoted `mic_button`.

### Review of `506cbe7` (LG hand-over)

Fixed before the release: success of a restore requires `launch` (it accepted anything but `ignore`); the restore is serialised with the daemon through a `flock`ed lock file, the state file carries a token, and a stop kills the supervisor's process group (an orphaned restore could remove a newer acquire's state file); the state file and lock live under the install, not `[report] dir`; the app shows On only with `mic_button_active` true; the persistence check also compares `configd_db.json` (size, modification time) and runs again 5 s after the takeover and at the release; the takeover runs after every fallible start-up step; uninstall and purge failures say "reboot the TV"; a leftover state file without `luna-send` reports `unknown`; `mic_button_error` is written without quotes so `ctl.sh` reads it whole.

### Re-review of `456e9ec` (before 0.5.0)

- `stop.sh` and `uninstall.sh` now give LG's key back after their stop (through the installed `ctl.sh restore-lg-voice`; an uninstall failure says "reboot the TV"), and a stop waits up to 45 s (not 5 s) for a supervisor that is restoring before the process-group kill (`WEBOS_SAT_RESTORE_WAIT`, tests only).
- A restore checks again, under the lock, that no daemon runs: a daemon that started while the restore waited keeps its key and state file.
- The takeover (lock wait up to 30 s plus luna calls) and a stop of the mic button at runtime keep the watchdog's loop heartbeat going, so a slow configd at boot cannot trip `limits.loop_stall_s`.
- `ctl.sh status` (and `config mic-button`) believe `voice.json` only while the daemon process itself is alive, not its supervisor: after a crash the mic button is not reported On; the TV app also shows On only with the daemon running.
- `lg_lock`: the lock file is opened in a subshell first (a failed `exec` redirection would end the shell), `flock(1)` is used with a 30 s `-n` loop (busybox has no `-w`), and a TV with neither `python3` nor `flock` gets that reason.

### Open question: a persistent write despite `"volatile": true` (hypothesis, not seen)

The persistence check (section 7) exists for the case that this firmware's configd stores the override in a persistent database although `"volatile": true` was sent (upstream source: volatile values stay in memory; the shipped binary has the same symbols; **not observed on a TV**). If the check ever fires, the satellite removes its in-memory value with `null` (volatile) and stops the feature, but a `null` sent with `"volatile": true` **cannot be expected to remove a value from a persistent database**. The satellite then must not try more: any non-volatile `setConfigs` is outside [ADR 0013](../decisions/0013-remote-mic-button-coexistence.md) and is never sent.

What the owner should do then (hypothesis until evidence exists): leave the feature off, note the log line (`configd's persistent database ... changed`), and check whether LG's assistant still opens from the mic button after a TV restart. If it does, the change was not about this key (configd writes the file for other reasons) and nothing else is needed. If it does not, LG's key stays at `ignore` across reboots, and undoing it needs a separate, owner-approved experiment and an ADR amendment (for example a non-volatile `null` for this one key, which upstream says removes it from the factory database: EXP-023's C1 rollback note).

Evidence needed before anything is run: on the TV, read-only, the metadata (size, time) of `/var/preferences/configd_db.json` and `configd_factory_db.json` before and after one enable session (EXP-023 C1 step 3), and whether `getConfigs` of `com.webos.keyaction.voice` reads `launch` after a TV restart. No write is part of that check.

**Baseline (read-only, 2026-11-07, 0.5.0 install, verified-on-target):** `com.webos.keyaction.voice` reads `launch` on both TVs. c3-a: `configd_db.json` 144 305 bytes, mtime 2024-02-26T02:24:46Z, `configd_factory_db.json` absent. c3-b: `configd_db.json` 144 306 bytes, mtime 2024-02-14T01:38:23Z, and `configd_factory_db.json` **present** (168 bytes, mtime 2025-12-12T16:24:37Z, contents not read), written by something of LG's before the satellite ever called configd there ([c3-a P4](../discovery/c3-a/P4-permanent-install.md#update-to-050-2026-11-07-mic-button-and-overlay-off), [c3-b P4](../discovery/c3-b/P4-permanent-install.md#update-to-050-2026-11-07)).
