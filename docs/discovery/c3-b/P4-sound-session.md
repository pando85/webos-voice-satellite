# P4: Sound session on c3-b (0.2.1): update, announcement hang, Assist phrasings

> **Session 2 (0.2.2, same evening):** the audible checks were run with the microphone free; see [Session 2](#session-2-022-with-the-microphone-free-2026-11-04-evening). Everything above that section is session 1.

- **Device:** c3-b (LG C3 55", webOS TV 8.3.0, firmware `03.30.73`, aarch64 kernel with 32-bit userland, sound out through ARC). The TV is in a household room. LG hands-free ("Hi LG") is on and holds the microphone, as in [P4 permanent install](P4-permanent-install.md); it was not touched.
- **Date:** 2026-11-04 (UTC times below).
- **Approval:** the owner (2026-11-04, about 15:50 CET): sound allowed on c3-b until 22:00 CET at moderate levels (TV volume at most the value found, reply volume default), short sounds; no sound on c3-a; HA pod restarts allowed. Rules kept: no ASoC or debugfs reads, no `apiadapter/system/getSystemInfo`, no LG setting changed, Kiwi's conversation agent not fixed, no pipeline changed.
- **Failsafe:** before any sound, a transient user timer on the PC (22:00 CET) and a persistent backup timer (22:00:30 CET, skipped on any other date) were scheduled to kill `pacat` children of the daemon for 330 s and restore TV volume 3 and mute off over SSH. Both were cancelled when the session ended early (nothing was playing and nothing had been changed on the TV).
- **Related:** [timer alarm checklist](../../development.md#timer-alarm), [TV control design](../../design/tv-control.md), [always-on 13](../../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b), [P2 session 1](../c3-a/P2-session1-audible-and-echo.md) (the playback taps).
- **Privacy:** no address, hostname, MAC, key or HA entry id appears here. Level logs (numbers only, no audio) are in the gitignored `devices/private/c3-b/P4-sound/`. No audio was recorded or kept.

Labels: **verified-on-target** (c3-b, this date, firmware `03.30.73`) unless marked.

## Outcome in one paragraph

0.2.1 was deployed to c3-b as an update (state and key kept). **No sound was heard:** the first announcement never reached the TV because Home Assistant blocked before sending it (finding 1), and 2 minutes later the TV powered itself off and suspended (finding 2), which ended the session; it was not woken. The Spanish Assist phrasings were checked with the built-in agent in recognition-only mode (no action), which found one alias with an inverted meaning (removed on c3-b) and that the volume and mute phrasings in the natural form do not reach the TV entities (finding 3). The audible checks (announcement level and latency, timer chime, stop, cap, queueing, TV commands executed) are **still open**.

## 1. Deploy 0.2.1

- `main` had new commits after 0.2.0 (timer alarm `37ec349`, TV control fixes `34d39ff`), so the version was bumped (`5407477`, "version 0.2.1", `Cargo.toml` and `Cargo.lock` only). `make ipk` from that commit (check passed), `make tv-install-ipk`, then `ctl.sh install` as in [c3-a P4](../c3-a/P4-permanent-install.md#update-to-011-2026-11-04).
- `ctl.sh status`: package, installed and running build `0.2.1 (aarch64-unknown-linux-musl)`; start at boot on; firmware validated; `etc/config.toml` and the stored pairing key have the same sha256 prefixes before and after. HA reconnected 3.7 s after the start with the stored key (no re-authentication).
- Startup: `voice_features=31` (timers now advertised; 0.2.0 logged 23), `tv_control=[... volume_limit=tv-max ...]`, "ready (power state Active; volume=ok mute=ok input=ok apps=ok)", the launch-declaration warning of 0.2.0 is gone, "the volume reply has no subscribed flag; keeping the subscription and polling every 10 s until an update is pushed" (the `34d39ff` behaviour), `MIC BUSY` as before.
- HA: 25 entities of the device, among them the new **Timers** sensor (`none`), **Timer alarm** binary sensor (`off`) and **Stop timer alarm** button; no duplicates; both assistant selects `preferred`.

## 2. Findings

### Finding 1: an announcement blocks in HA while the satellite holds an idle wake-word pipeline (mic busy)

- **Observed:** `assist_satellite.announce` (a 44-character Spanish message, `preannounce: false`) on the c3-b satellite did not return within 120 s. The daemon logged nothing (it logs `announcement ...` at info when the request arrives), the satellite state never left `idle` (HA sets `responding` just before it sends the request), and the PulseAudio monitor of the output sink stayed at digital silence. HA's log had no error. **verified-on-target**
- **Cause (documented, from HA 2026.10.4's source in the pod, not a guess about LG):** `assist_satellite` `async_internal_announce` first awaits `_cancel_running_pipeline()`, which cancels and then awaits the satellite's pipeline task. The daemon had requested a wake-word pipeline at connect (`requesting pipeline (wake_word=true)`, logged as "streaming mic audio to HA (wake word stage)") although the microphone is busy, so it sends no audio. The Wyoming wake-word client in that pipeline, on cancellation, runs `finally: await audio_task`, which waits for the **next audio chunk** ("It's critical that we don't cancel the audio task"). With no chunk ever coming, the cancel never completes and the announcement is never sent. On c3-a the microphone streams continuously, so the next chunk arrives within milliseconds and this does not show. The HA debug list of the preferred pipeline shows the runs from each connect stuck at `wake_word-start`. Label: **hypothesis** for the full chain (the HA side is read from source; the cancel itself was not instrumented), consistent with every observation above.
- **Consequence:** with the microphone busy (LG hands-free on), announcements, and probably timer-finished replies and `start_conversation`, cannot play on c3-b until the pipeline ends. The same would happen on any TV whenever the capture fails while HA is connected.
- **Fix (daemon, PC tests only, not yet on the TV):** the wake-word session is requested only while the capture actually delivers audio and is aborted (`VoiceAssistantRequest{start: false}`) when the PCM closes, cannot be opened or stops delivering, see [capture liveness](../../development/esphome-api.md#capture-liveness-appcommandcapturelive). Label: **hypothesis** until an announcement is heard on c3-b with hands-free on.
- **Original proposal:** request a wake-word pipeline only while the capture actually delivers audio (Listening on), and end it (stop request or end of audio) when the PCM closes or cannot be opened. Workaround for a test: the HA mute switch should end the pipeline (on c3-a mute stops the stream, [c3-a P4](../c3-a/P4-permanent-install.md#update-to-011-2026-11-04)); not tried on c3-b, the TV was gone by then.

### Finding 2: the TV powered itself off 15 minutes after it was left on a silent input

- HA history of the "TV power state" sensor: the TV was `Active` on `HDMI 1` (where the reboot of the [P4 install](P4-permanent-install.md) left it, about 15 min earlier) with a screen-saver request every 3 minutes (no source signal, **hypothesis**), then `Request Power Off` and `Request Power Off Logo` (15:20:24), `Active Standby` (15:20:28), `Active Standby (Prepare Suspend)` (15:20:40), and the HA entities `unavailable` at 15:22:33. The TV stopped answering ping and SSH. **verified-on-target** for the sequence.
- The 15 minutes from the install's reboot match LG's no-signal automatic power-off (**hypothesis**; the power-off reason is in the daemon log on the TV, not read yet because the TV is off). No action of this session targets the power state.
- So the TV's normal state in the household is off, and it was on only because of the reboot test. It was **not woken** (no authorization to power the TV on). This is also the first **real suspend** with the installed daemon: the `RESUMED` path ([always-on 13, G2](../../design/always-on.md#13-implemented-in-the-daemon-010-and-checked-on-c3-b)) can be read from the log when the household next turns the TV on.

### Finding 3: Spanish phrasings with HA's built-in agent (recognition only, nothing executed)

`conversation/agent/homeassistant/debug`, language `es`, with the c3-b device id as context. "Match" means the sentence resolved to one of the satellite's TV entities. **verified-on-target** (HA 2026.10.4) for the recognition; nothing was executed.

| Phrasing | Intent | Target | Result |
| --- | --- | --- | --- |
| "baja / sube el volumen de la tele del hall", "baja volumen de ...", "bajar volumen de ..." | `HassSetVolumeRelative` | wants a **media player** named "tele del hall"; none | no match (would fail) |
| "pon / establece el volumen de la tele del hall a 5" | `HassSetVolume` | media player "tele del hall"; none | no match |
| "pon el volumen de la tele del hall al 5" | `HassLightSet` (brightness) | none | wrong intent, no match |
| "pon el volumen de la tele del hall en 5" | `HassMediaSearchAndPlay` | none | wrong intent |
| "silencia la tele del hall" (and "silencia la tele") | `HassMediaPlayerMute` | media player; none | no match |
| "quita el silencio de la tele del hall" | `HassMediaPlayerUnmute` | media player; none | no match |
| "activa / enciende el silencio de la tele del hall" | `HassTurnOn` | TV mute switch | **match** (mute on) |
| "desactiva / apaga el silencio de la tele del hall" | `HassTurnOff` | TV mute switch | **match** (mute off) |
| "apaga / enciende la pantalla de la tele del hall" | `HassTurnOff` / `HassTurnOn` | TV screen switch | **match** |
| "apaga / enciende la tele del hall" | `HassTurnOff` / `HassTurnOn` | TV screen switch (alias "tele del hall") | **match** (screen, not power) |
| "activa bajar volumen de la tele del hall", "pulsa (el botón) bajar volumen ...", "presiona ..." | `HassVacuumCleanArea` or none | none | no match: the volume buttons cannot be pressed by voice in Spanish |
| "apaga el sonido de la tele del hall" (before the change below) | `HassTurnOff` | TV mute switch, by the alias "sonido de la tele del hall" | **inverted**: turning the mute switch off un-mutes |
| "pon un temporizador de un minuto", "... de 1 minuto", "temporizador de 20 segundos" | `HassStartTimer` | the device context | match |
| "cancela el temporizador", "cuánto queda del temporizador", "para el temporizador" | `HassCancelTimer`, `HassTimerStatus`, `HassPauseTimer` | | match |
| "para la alarma" | `HassMediaPause` | a media player | not a timer intent; whether pause reaches the alarm stop is untested |
| "pon un temporizador de 1 minuto en la tele del hall" | `HassMediaSearchAndPlay` | | wrong intent |

- **Changed:** the alias "sonido de la tele del hall" was **removed from c3-b's TV mute switch** (it made "apaga el sonido" un-mute). After the change "apaga el sonido de la tele del hall" no longer resolves to an entity. c3-a has the same alias pattern ("sonido de la tele del despacho"); it was **not** changed in this session; it was removed on c3-a afterwards ([c3-a P4, 0.2.2](../c3-a/P4-permanent-install.md#update-to-022-2026-11-04-firmware-policy-self-check-timers)).
- **Why the natural volume and mute sentences fail:** HA's Spanish volume and mute intents target `media_player` entities only. The TV's volume is a `number` and its mute a `switch` on the satellite device, and HA has no Spanish intent that sets a `number` or presses a `button` by name. The satellite's own media player is the **reply** volume and must not get the alias "tele del hall" (then "baja el volumen de la tele" would lower the satellite's replies, not the TV).
- **Proposals (not made):** (a) expose the TV volume and mute as a second ESPHome `media_player` entity of the device (volume set and step, mute) named for the TV, so the built-in intents reach it; or (b) custom sentences in HA (`conversation` custom sentences or an automation with a sentence trigger) for "baja/sube el volumen de la tele del hall", "silencia la tele del hall", "pon el volumen de la tele del hall a N" that call the button, switch and number. (b) needs no code and is per-household; (a) is the general fix.

## 3. Measurement setup (for the rerun)

- **PulseAudio monitor works:** `parec -d pcm_output.monitor` (48 kHz, s16le, stereo) streamed to the PC and reduced to 100 ms RMS and peak values (no audio kept) gave digital silence (`-150 dBFS`) while nothing played. The satellite's `pacat` plays to the default sink `pcm_output`, so this measures our stream after mixing and **before** the TV volume.
- **SoundEngineCapture (`hw:LGSIC,3`) gave no frames on c3-b** with sound out through ARC and nothing playing: the PCM opened and went `RUNNING` but `hw_ptr` stayed 0 for 2 minutes; the reader was killed and the PCM closed. Whether it clocks while something plays (as on c3-a with the TV speaker) is untested. `hw:LGSIC,2` (MixerCapture) was `RUNNING`, held by another process (LG's voice input); it was not opened.
- Sink inputs (`pactl list short sink-inputs`) polled every 250 ms work as an on/off signal.

## Left changed

On the TV: 0.2.1 instead of 0.2.0 (rollback: install the 0.2.0 ipk and `ctl.sh install`), and a copy of `tools/audio-rig/tv_dualcap.py` in `/tmp` (tmpfs; it could not be deleted because the TV went off; it is lost at the next boot). In HA: the alias "sonido de la tele del hall" removed from c3-b's TV mute switch. The TV volume (3), mute (off), sound output, input and app were never written. The HA token of the session was deleted.

## Residual unknowns

- Everything audible: announcement start latency and level at reply volume 40 %, the timer chime (start on FINISHED, level, the Timer alarm sensor, stop by the button, the cap), queueing of an announcement behind a ringing alarm, and the executed TV commands with their luna read-back. They need the TV on (the household or an authorized wake) and finding 1 out of the way (fix the daemon, or mute the microphone in HA during the test).
- The power-off reason (daemon log) and the first `RESUMED` after a real suspend.
- Whether SoundEngineCapture clocks on c3-b while sound plays through ARC.

## Session 2: 0.2.2 with the microphone free (2026-11-04, evening)

- **Approval:** the owner (2026-11-04): "Turn Hi LG off" on c3-b so the satellite can listen (record the original first, keep it off afterwards); power states, reboots and settings on c3-b allowed (restore originals where sensible); sound until 22:00 CET at the TV volume found and the default reply volume, short sounds; no sound on c3-a and c3-a not touched; HA pod restarts allowed (none needed); the c3-b device assigned to the existing HA area "Living Room". Rules kept: no ASoC or debugfs reads, no `apiadapter/system/getSystemInfo`, no pipeline changed or created.
- **Failsafe:** the same two PC timers as session 1 (22:00 and 22:00:30 CET), changed to restore the volume found in this session (12); cancelled at 20:03 CET after everything was restored. They never ran.
- **Measurement:** as in [section 3](#3-measurement-setup-for-the-rerun): `parec` on `pcm_output.monitor` reduced on the PC to 100 ms RMS and peak values (our stream after mixing, **before** the TV volume; nothing kept but the numbers), `pactl list short sink-inputs` every 200 ms, the daemon log, and HA `state_changed` events, all stamped with the PC clock (the TV clock is about 1 s behind; daemon log lines arrive with up to about 1 s of delay, so latencies below are taken from the monitor, the sink inputs and HA). A running `parec` keeps the sink awake, which may make the start latencies slightly shorter than in normal use. Raw numbers are in the gitignored `devices/private/c3-b/P4-sound/s2-*`.

Labels: **verified-on-target** (c3-b, firmware `03.30.73`, HA 2026.10.4) unless marked.

### State found and the "Hi LG" change

- At the start (19:50 CET) the TV was **on** (`Active`, home app; the household had turned it on with the remote about 19:48 CET after two power-offs), volume **12** (not the 3 of session 1: changed by the household), not muted, `external_arc`. It moved to `Screen Saver` by itself a minute later.
- **LG hands-free was already off:** `voiceinput/setting/getActivation` read `settingStatus false`, `activationState false` for ThinQ, Amazon and the third provider (`currentTriggerWord hiLG`); `option/turnOnByVoice off`, `wakeUpword LGTV`. Earlier the same day it was **on** (`settingStatus true`, `activationState true` for ThinQ: [P4 install](P4-permanent-install.md), [D6 restore table](D6-poweroff-screenoff.md)). **Who or what turned it off is unknown**: the TV's `messages` log rotates within minutes and kept nothing older than the session. The satellite log shows the microphone came free at 15:20:40 UTC, while the TV went to `Active Standby` before the no-signal power-off of session 1, and the satellite has held it since then (through two suspends and the household's power-on). Whether LG's voice service turns its setting off when it cannot reopen a busy microphone, or a person changed it, is a **hypothesis** either way.
- **Nothing was written** (the owner wanted it off and it read off). `hw:LGSIC,0` (`pcm0c`) is `RUNNING`, held by `webos-satellite` only; `pcm1c` (SpeakerFeedback) closed; `voiceinput` holds no capture PCM. HA's **Listening** is `on`, **Microphone muted** `off`. Kept **off** at the end (owner decision).
- **To switch "Hi LG" back on:** Settings, General, AI Service, Voice recognition settings, "Hi LG" (the household's way), or as root `luna-send -n 1 luna://com.webos.service.voiceinput/setting/setActivation '{"settingStatus":true}'` (needs a pty, as in [D6](D6-poweroff-screenoff.md)); read it back with `setting/getActivation`. With it on, the satellite reports `MIC BUSY` again and session 1's finding 1 applies.

### Deploy 0.2.2

- `afd6b02` "version 0.2.2" (`Cargo.toml` and `Cargo.lock` only; `main` has the soft firmware policy and self-check `76c3f8c` since 0.2.1). `make ipk` (check passed, sha256 `b5009971...`), `make tv-install-ipk`, `ctl.sh install`: aarch64 build, `etc/config.toml` and the stored pairing key with the same sha256 prefixes before and after, `etc/firmware_seen` written at the first start. HA reconnected 3.7 s after the start with the stored key.
- Startup: "firmware 03.30.73 ... verified (in safety.validated_firmware)", `tv_control: ready (... volume=ok mute=ok input=ok apps=ok)`, **no launch-declaration warning**, the volume subscription kept with polling until a push, microphone open, `self-check: passed`.
- HA: **35 entities** (25 before; the new ones are Firmware verified `on`, Capture problem `off`, Firmware changed `off`, Compatibility `verified`, Microphone self-check `passed (peak -40 dBFS)`, TV firmware `03.30.73`, TV model, Compatibility report "ready: TV app > Report compatibility, or ctl.sh report", and the buttons Re-run microphone check and Acknowledge firmware change). The ten new entities got entity ids that start with `living_room_` because the device had just been put in that area (HA's naming of new entities, **hypothesis** for the rule; cosmetic, the older ids were kept).
- **HA area:** the c3-b device is now in the existing area **Living Room** (no area was created).

### Self-check with the real microphone and the report

| Item | c3-b | Threshold ([design C6, C7](../../design/firmware-policy-and-reports.md#3-self-checks)) | Margin |
| --- | --- | --- | --- |
| Checks | C1 to C7 pass, C8 skipped | | |
| 10 s window at the first open (TV in `Screen Saver`, room quiet) | RMS -67.5 dBFS, peak -39.6 dBFS, clipped 0, xruns 0, 204 chunks, 0 dropped | C6: peak above -80 dBFS; C7: clipped below 0.5 % and RMS below -10 dBFS | C6 about 40 dB; C7 about 57 dB |
| Two later restarts | passed, peak -20 and -37 dBFS | | |

c3-b's RMS equals c3-a's quiet-room reference (-67.6 dBFS); the peak depends on what happens in the room in those 10 s. No threshold change is suggested by this unit.

**Report privacy:** `ctl.sh report`, `--full`, `--json`, `report.url` and the report files were compared on the PC with 13 identifiers read on the TV for the purpose (MAC of every interface, every IPv4 and IPv6 address, the hostname, the device ids of `nyx-cmd DeviceInfo`): **0 matches**, and no MAC or IPv4 pattern in the text. The identifiers were held in a temporary file that was deleted; none is written anywhere. The report carries the model (`OLED55C36LC`), webOS, firmware, kernel version, PCM parameters, levels and the TV-control results, by design.

### TV control re-checks

- **Refresh TV inputs and apps:** one summary line, `tv_refresh_lists: done: 4 inputs, 30 apps`.
- **Volume push:** the first mute change produced `an update was pushed on the volume subscription; it streams without the flag, polling stopped` (the `34d39ff` behaviour confirmed).
- **Spanish commands executed** (`conversation/process`, agent `conversation.home_assistant`, language `es`, the c3-b device as context), each read back with luna: "apaga la pantalla de la tele del hall" (`Screen Saver` to `Screen Off`), "enciende la pantalla de la tele del hall" (`Active`; the screen saver came back by itself later), "activa el silencio de la tele del hall" (`muteStatus true`, volume 12 kept), "desactiva el silencio de la tele del hall" (`muteStatus false`). All `action_done` on the right switch, HA answered in 7 to 16 ms.

### Sound results

Reply volume 40 % (default), TV volume 12 on ARC (as found). Levels are at the PulseAudio monitor, before the TV volume.

| Check | Result |
| --- | --- |
| **Announcement, microphone streaming** (`assist_satellite.announce`, `preannounce: false`) | **No hang** (session 1's finding 1 needs a busy microphone). 44 characters: audio at the monitor 1.08 s after the call, 2.68 s of audio, the call returned after 3.71 s. 27 characters: 0.33 s, 1.58 s, 1.84 s. Level: mean RMS -45 to -46 dBFS, peak -30.0 dBFS |
| **Timer by text** ("pon un temporizador de un minuto") | `action_done`; **Timers** `1 timer, next in 1 min` at once |
| **Finished: chime** | The chime was at the monitor within 0.1 s of **Timer alarm** turning `on` in HA (the HA event and the first audio frame arrive together); mean RMS -45.7 dBFS, loudest 100 ms -39.9 dBFS, peak -32.9 dBFS (about the announcement level) |
| **Announcement while it rings** | The alarm paused (`timer alarm paused: a reply or announcement is playing`), the announcement played alone, the alarm resumed within about 0.15 s of its end ("ringing for at most 292 s", the cap still counted from the finish). One stream at a time |
| **Timer finishing during an announcement** (10 s timer, 101-character announcement) | The alarm waited (Timer alarm `on` while queued) and started right after the announcement (no gap at 100 ms resolution, no overlap) |
| **Stop timer alarm button** | Sink input gone 0.18 to 0.20 s after the press; audio silent by the next 100 ms frame (2 runs); Timer alarm `off` with the press; Timers `none` |
| **Media player stop** | Audio silent within 0.13 s, sink input gone within 0.05 s; log `timer alarm stopped (media player stop; was Ringing)` |
| **Cap** (one run with `timer_alarm_max_s = 20` in the TV's config, the original restored after with the same sha256 prefix, two daemon restarts) | `stopped at the time cap (nobody stopped it)`: Timer alarm `on` for 20.2 s, audio 20.2 s |
| Stop by the wake word | not tried (needs a person to say it; HA's Kiwi conversation agent still fails, backlog #12) |

**False wakes:** HA detected the wake word **once** in the session (from the first sound to the end, about 8 minutes with about 50 s of our audio: 4 announcements and 4 alarms), about 4 s **before** a chime with nothing playing (room sound; the conversation agent failed, nothing was played). **0 detections during or right after our own sound.** While an announcement plays the wake-word pipeline is closed anyway (see the next finding), so only the chime and the gaps were exposed. One evening, one room, one level: not a rate.

### Findings

1. **Announcements interrupting the idle wake-word run count as failed runs, and the retry backoff grows** (daemon, **verified-on-target** from the log, cause read from `crates/esphome-api/src/voice.rs`). Every announcement cancels the waiting wake-word pipeline; the daemon logs `voice: run failed (N in a row); retry in M ms` and the counter is reset only by a wake word or a speech-to-text result. After three announcements it read "4 in a row; retry in 2000 ms"; the delay doubles from 250 ms up to 60 s. Here the announcement playback outlasted the delay, so streaming resumed at once, but after about eight announcements or alarms without a wake word the satellite would stop streaming for up to a minute after each. **Fix (PC tests only, not yet on the TV):** a run HA ended without an error is no longer a failure, see [an announcement is not a failed run](../../development/esphome-api.md#an-announcement-is-not-a-failed-run).
2. Session 1's announcement hang did not occur with the microphone streaming (4 of 4 announcements returned in 1.8 to 3.7 s). The hang with a **busy** microphone is still unfixed and was not re-tested.
3. The TV volume and the "Hi LG" setting were changed by the household (or the TV) between the sessions; the TV is in daily use in the evening.

### Restored and left changed

- TV at the end: `Screen Saver` on the home app (how it settled after the household turned it on; not a no-signal input), volume 12, not muted, `external_arc`, no sink input, mic PCM held by the satellite, kernel taint 4097, no oops. `/tmp`: `tv_dualcap.py` from session 1 deleted, the config backup deleted, nothing of ours left. The TV's config is the original (sha256 prefix compared).
- HA: both assistant selects `preferred`, 2 pipelines (none created), reply volume 0.4, Timer alarm `off`, Timers `none`, TV mute `off`, screen `on`. The session's refresh tokens (two) were deleted by their own ids; a refresh then fails. No other token was touched.
- **Left changed:** 0.2.2 instead of 0.2.1 (rollback: install the 0.2.1 ipk and `ctl.sh install`); the c3-b device in the HA area Living Room; "Hi LG" **off** (found off, kept off by the owner's decision; how to revert is above).

### Residual unknowns

- Stop by the wake word over the chime; how loud the chime is in the room at other TV volumes (only the level before the TV volume was measured); a long-term false-wake rate with the chime.
- Why "Hi LG" turned off; whether it comes back after a reboot (the setting reads off).
- The announcement hang with a busy microphone (session 1, finding 1) and the backoff growth (finding 1 above) are fixed in the daemon (PC tests only; to be verified on c3-b with the next update).
