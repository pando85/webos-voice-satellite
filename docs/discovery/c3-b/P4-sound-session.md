# P4: Sound session on c3-b (0.2.1): update, announcement hang, Assist phrasings

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
- **Proposed fix (daemon, not made here):** request a wake-word pipeline only while the capture actually delivers audio (Listening on), and end it (stop request or end of audio) when the PCM closes or cannot be opened. Workaround for a test: the HA mute switch should end the pipeline (on c3-a mute stops the stream, [c3-a P4](../c3-a/P4-permanent-install.md#update-to-011-2026-11-04)); not tried on c3-b, the TV was gone by then.

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

- **Changed:** the alias "sonido de la tele del hall" was **removed from c3-b's TV mute switch** (it made "apaga el sonido" un-mute). After the change "apaga el sonido de la tele del hall" no longer resolves to an entity. c3-a has the same alias pattern ("sonido de la tele del despacho"); it was **not** changed (outside this session's scope); the owner should remove it there too.
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
