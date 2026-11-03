# c3-b: D5 results, remote power-off and Always Ready

- **Device:** c3-b (LG C3 55 inch, webOS TV 8.3.0, firmware `03.30.73`, image `lib32-starfish-global-secured`), rooted with the Homebrew Channel. Facts and differences from c3-a: [D0 quick](D0-quick.md). Results apply to this unit and firmware only. c3-a was not touched.
- **Date:** 2026-11-03, 11:15 to 12:40 (CET). The TV is in the owner's hall and in daily use; a person was present part of the time (see [Observations that were not planned](#observations-that-were-not-planned)).
- **Procedure:** [D5 plan](../c3-a/D5-plan.md) (way-back gate, screen off, R1 to R5, rank 2 probes), with the changes listed under [Deviations](#deviations-from-the-plan).
- **Software:** `webos-satellite` 0.0.1, static `aarch64-unknown-linux-musl`, from the repository at commit `fdeb52c` (clean tree), `make tv-install-tmp TV_ARCH=aarch64`, under `/tmp/webos-satellite`. Config: `mode = "wake"`, ALSA mic (`LGSIC` / `WoV PDM Mic`), `[sink] type = "pacat"` with `gain = 0.0`, `level = "info"`, `validated_firmware = ["03.30.73"]` (**valid for this test only**, see [D0 quick](D0-quick.md#decision)), device name `c3-b-test`. LG hands-free was off at the start.
- **Evidence labels:** every result below is `verified-on-target` (c3-b, firmware `03.30.73`, 2026-11-03) unless marked `inferred`, `hypothesis` or `not done`. Raw logs are in the gitignored `devices/private/c3-b/`.

## Answer

**Does pressing power off keep the satellite listening and able to play a reply? Listening: yes, if Always Ready is on. Replying: no, not audibly, in that state.**

| State after the power-off | Satellite listening | A reply is audible |
| --- | --- | --- |
| Always Ready on (`alwaysReady` or `allEnabled`), Quick Start+ on | **Yes.** The TV sits in `Active Standby`, the process and the HA link stay up, the microphone PCM stays `RUNNING`, wake word runs. Seen for 59 min 18 s with no end by a timer | **No.** A quiet tone played by `pacat` was not heard by the TV's own microphone (5 checks, ARC and TV speaker outputs). The same tone was heard in `Active`, `Screen Saver` and `Screen Off` |
| Always Ready off (c3-b's own setting), Quick Start+ on | **No.** `Active Standby` for 7 to 15 s, then `Suspend`: network down, process frozen (not killed), kernel suspend. The satellite resumes by itself after a wake (the same pid, the HA link kept) | No (frozen) |
| Screen off on a running TV (`turnOffScreen`) | Yes | **Yes** |

The power-off here was the API key (`networkinput/controls/power`) **and** two real presses of the IR remote that a person made (see [R2](#r2-the-real-remote)): the TV did the same thing for both. Not measured: the panel (nobody was there to look), the power draw, a stay longer than 59 min.

## Way-back gate (before any standby step)

| Item | Result |
| --- | --- |
| SSH and luna | `getPowerState` through `ssh -tt` answered `Active`/`ScreenSaver`; `luna-send` through a plain pipe worked for `tvpower` but printed nothing for `audio` getters (a terminal was needed) |
| Screen restore without the remote | `turnOnScreen` answered `Active` (used after the screen-off step) |
| Wake path | **`network/wolwowlOnOff` was `false`.** It was set to `true` for the run (original recorded) and restored at the end. The TV is on Ethernet (the Wi-Fi interface is down); the wired MAC was read on the TV and kept in a private file. A WoL burst (packets to the subnet broadcast and the TV's address, from the PC on the same segment) woke the TV from `Suspend` twice: first ping reply **2 s** and **4 s** after the burst started, state `Active` 2 s later, reason `wakeOnLan` in the power log. **Not tested:** whether WoL works with the original `false` |
| HA turn-on | **No webostv entity exists** for this TV (or for c3-a), so there is no HA turn-on path; WoL from the PC was the only network wake |
| Remote | Not needed. A person's remote key also woke the TV (reason `remoteKey`) once, unprompted |

## Observation setup

TV-side loops (read-only, every 5 s): the power-state subscription (stamped), `getPowerState` and `alwaysready/aod/getStatus`, `suspend_stats/success`, the sampler (pid, fds, threads, RSS, CPU ticks, mic PCM state, HA connection count) and `pcmwatch`; raw `cat` copies of the input nodes `LGE RCU` (`event1`), `Smart Remote RCU Input` (`event9`) and `LGE Network Input` (`event10`), no grab, no write. PC side: a stamped `ping` every second, a reconnecting `tail` of the TV-side files, and a stamp for each action. A first `od`-based input trace was **buffered and recorded nothing** (it was replaced by raw `cat` before R4, so the key traces start at 12:28 and miss R1a, R1b and the first physical turn-on).

The wall clock of the TV stops during a kernel suspend and is corrected at the resume, so TV-side stamps after a suspend lag by the suspend duration until the next sync; the tables use the PC clock where the two differ.

## Table per state

"Tone" is the mic's view of one 1 kHz tone of 1.5 s played with `pacat` at software gain 0.2 with the TV master volume set to 9 for the check and restored to 12 (the kill switch pauses the satellite's capture for the check and the mic is read with `arecord`; the numbers are the microphone's RMS in dBFS, ch0, per 100 ms). In `Active` the tone is a steady **-36 dBFS** (1 kHz band -31 dBFS) over a floor of -84 dBFS.

| State | SSH / network | Process | PCM and frames | HA link | Tone (reply stand-in) | Wake / recovery | Power (W) | Label |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Screen saver (home app, start of the run, 3 checks) | up | pid 12922 from the first start | `RUNNING` | connected | **audible**, -36 dBFS steady | n/a | not measured | verified-on-target |
| Screen off (`turnOffScreen`, 5 min; state `Screen Off`) | up | same pid | `RUNNING`, no stall line | connected | **audible**, -36 dBFS steady | `turnOnScreen` answered `Active` at once; no log line, no xrun | not measured | verified-on-target |
| Screen off 30 min / soak | not done | | | | | | | not done (replaced by the Active Standby soak) |
| API key, **Always Ready off**, Quick Start+ on (R1a, 60 s) | up for 3 s (`Active Standby`), then **down 20 s after the key** | frozen (same pid, sampler gap 47 s, no restart) | `SUSPENDED` then `RUNNING` within about 20 ms of the resume (`pcmwatch`) | the TCP session survived (no disconnect line, sampler counts it established after the resume; HA's entity stayed `idle`) | n/a (frozen) | WoL burst at +60 s: ping up after 2 s, `Active` after 4 s; `suspend_stats/success` 9 to 10 | not measured | verified-on-target |
| API key, same, with a `sleepd` activity hold (2a) | down 10 s after the key | frozen | as above | as above | n/a | WoL at +44 s woke it in 4 s; `suspend_stats/success` 10 to 11; **the hold did not delay the suspend** (it came at +7 s instead of +15 s) | not measured | verified-on-target (negative for 2a) |
| API key, **Always Ready `alwaysReady`**, Quick Start+ on (R1b, soak) | **up throughout** (0 ping losses in 59 min) | same pid for the whole run (RSS 1252 kB, CPU about 3.3 % of one core, 12 fds, 3 threads) | `RUNNING` in 702 of 703 samples (the other one is my own kill-switch pause for a check) | connected the whole time | **not audible**: 1 kHz band at most -53 dBFS after the open transient and no plateau, in 4 checks at +45 s, +100 s, +30 min and with the TV speaker output (control in `Active`: -31 dBFS) | 6 wake word detections with the full chain (below). Ended after 59 min 18 s by a power-on whose reason was `remoteKey` (see below). `suspend_stats` unchanged at 10 | not measured | verified-on-target |
| API key, **Always Ready `allEnabled`** (R4, with wallpaper) | up | same pid | `RUNNING` | connected | **not audible** (max -63 dBFS) | Same state sequence as `alwaysReady`. One earlier attempt 90 s after a person's power-on showed no power-off at all: `Active`, `aod` status `default` for 11 s, then `none` (cause unknown, not reproduced) | not measured | verified-on-target |
| Standby, Quick Start+ off (cold start) | not done | | | | | | | not done |
| Reboot recovery | not done | | | | | | | not done |
| R1: API key `controls/power` | see R1a (Always Ready off) and R1b (on) | | | | | | | verified-on-target |
| R2: real remote (IR), uncontrolled | as R1b (press B) and R1a (press C) | alive (B) or frozen (C), same pid | as R1 | as R1 | not checked | B: `Active Standby`, no suspend; C: `Suspend`; see R2 | not measured | real-remote, uncontrolled (3 presses, 2 traced) |
| R3: soak after R2 | see the Always Ready row: 59 min 18 s | | | | | no timer ended it | | partial |
| R4: `allEnabled` | see the row above | | | | | | | verified-on-target |
| R5: `suspend_stats` counter | rose by one for each suspend (9, 10, 11, 12) and stayed at 10 for the whole Always Ready soak | | | | | LG's remote power-off with Always Ready off **is a kernel suspend**; Always Ready on **is not** | | verified-on-target |

## R1: state sequences (PC clock offsets from the key)

Power-state subscription lines with the transitional `processing` strings. The key was `luna-send ... controls/power '{}'` over SSH; it answered `{"returnValue":true}` each time.

| Offset | Always Ready **off** (R1a) | Always Ready **on** (R1b) |
| --- | --- | --- |
| +0 s | `Request Power Off`, `Request Power Off Logo` (reason `remoteKey`) | the same |
| +3 s | `Request Active Standby`, `Prepare Active Standby`, state `Active Standby` | the same (+2 s) |
| +7 s (R1b) / +15 s (R1a) | `Request Suspend` then `Prepare Suspend`, reason `oslbSaving` | `Prepare Active Standby` with reason `oslbSaving`, state `Active Standby` again; **no suspend** |
| +16 s | state `Suspend`; ping down at +20 s | stays `Active Standby`; `aod/getStatus` stays `none` (so `aod` status does not mark this state) |
| Wake | WoL: `Prepare Resume` (`wakeOnLan`), `LastInput Ready`, `Screen On`, `Active` | `controls/power` again: `Prepare Power On` (`remoteKey`), `LastInput Ready`, `Screen On`, `Active` 3 s later |

- **The API key puts the TV in the same sequence as a person's remote**, including the reason string `remoteKey`. This is the closest software equivalent to the real key, as predicted in [always-on 10.3](../../design/always-on.md#103-making-the-same-code-path-as-a-real-remote-press).
- `controls/power` is a **toggle**: from `Active Standby` it wakes the TV in 3 s (restore ladder step 1 works). It woke it three times.
- Whether the panel is dark in `Active Standby` was **not observed**; nobody looked. The `alwaysReady` and `allEnabled` runs gave the same states and the same silence.

## R2: the real remote (uncontrolled, from unplanned presses)

I cannot press the remote, and the physical-remote comparison was not scheduled. The owner confirmed afterwards that someone in the household turned the TV on and off with the **real remote** during the run. Those presses were at times I did not choose, with no command of mine near them (the action log has none), so they are **real-remote, uncontrolled** data. The input trace caught two of them.

| Press (PC clock) | Key seen on | TV before | State sequence | Process, PCM, network, HA link | How it came back |
| --- | --- | --- | --- | --- | --- |
| A. 12:25:59, power-on (not traced, reason `remoteKey`) | not traced | `Active Standby`, Always Ready `alwaysReady`, 59 min 18 s into the soak | `Prepare Power On`, `LastInput Ready`, `Screen On`, `Active` 3 s later | same pid 12922, PCM `RUNNING`, ping up, HA connection counted in every sample | it **is** the way back: a real key wakes `Active Standby` in 3 s, as `controls/power` does |
| B. 12:31:24, power-off | **`event1`, `LGE RCU`**, `KEY_POWER` press 0.28 s, then release | `Active` (12 s after I woke it), Always Ready `alwaysReady` | `Request Power Off` (reason `remoteKey`), `Request Power Off Logo`, `Active Standby` at +4 s, `Prepare Active Standby` (`oslbSaving`) at +7 s, `Active Standby` again; one loop sample read `processing: PrepareAlwaysReady` at +7 s. No suspend | pid unchanged, PCM `RUNNING` (the one `closed` sample is my own kill-switch pause), ping up, HA count 1, `suspend_stats` 10 | **my** `controls/power` 16 s later (`Active` at +19 s). Not timed longer, so how long this press would have held is unknown |
| C. 12:34:36, power-off | `event1`, `LGE RCU` | `Active` (about 37 s after my WoL wake), Always Ready `off` | `Request Power Off`, `Active Standby` at +4 s, `Request Suspend` (`oslbSaving`) at +7 s, `Suspend`, state check `susp=11` before and `12` after | pid unchanged after the wake (sampler gap 94 s, same pid 12922, 3 threads), PCM `RUNNING` again at the first sample, ping down at +10 s and up at the wake, HA count 1 after, `suspend_stats` 11 to 12 | **my** WoL burst about 90 s later: ping up at once, `Active` 4 s later |

- **The IR remote's POWER key is a normal `KEY_POWER` event on `LGE RCU` (`event1`)** while the TV is active. The API key arrived on **`Smart Remote RCU Input` (`event9`)**, six times, as a 0.05 s press; `LGE Network Input` (`event10`) saw nothing (`inferred`: the network-input service writes into the node named `Smart Remote RCU Input`). This answers the question rank 3 of the design asked: the physical POWER key is an input event on this unit. It is still not a way to keep the TV awake, see the design.
- For presses B and C the state sequence matched the API key with the same Always Ready setting (+4 s `Active Standby`, `Request Suspend` only when Always Ready is off), **including the reason `remoteKey`**. So the API key is a valid stand-in for the real key on this unit for the first 15 s, and the real key with Always Ready on gives the same listening state (press B, 16 s observed) and with it off the same suspend (press C).
- The people's presses came right after my wake-ups (12 s and 37 s), so they may be a reaction to the panel lighting up; see [Observations that were not planned](#observations-that-were-not-planned).
- **Not established:** a real-remote run of the long soak (the 59 min soak was entered with the API key and left by press A), the Magic Remote (its node is `event9`, which the API key also uses, so a trace cannot tell them apart), and which remote made press A (no trace was running).

## Always Ready soak (R3, API key, `alwaysReady`)

- Entered at 11:26:41 (UTC 10:26:41) from the PC; **left at 12:25:59 CET by a power-on with reason `remoteKey`**, 59 min 18 s later. No timer ended it: `aod30sec`, `autoOffByAoDTimer` and the no-signal/15-minute timers did not fire (the TV was on the home app, not on an HDMI input). Whether it would last hours is **not known**.
- 703 sampler samples: one pid, 702 `RUNNING` (the rest is my own pause for a check), HA connection count 1 in every sample, 0 ping losses, `suspend_stats/success` 10 throughout, state `Active Standby` in all 699 loop lines, `aod` status `none`.
- **No `MIC BUSY`, no stall line, no watchdog line** in the daemon log for the whole run (LG hands-free was off). RSS 992 kB to 1252 kB during the soak, 2.6 MB at the end; CPU about 3.3 % of one core with the aarch64 build (wake mode, mic streaming to HA).
- **Six wake word detections happened in Always Ready** (10:28, 10:37, 10:41, 10:59, 11:17 and 11:22 UTC), each with the whole chain: HA detected the wake word, Whisper produced a transcript (9 to 37 characters, read from the daemon's counts only), the intent stage answered (the test pipeline's conversation language `ru` cannot match a command), HA streamed 14.5 s of TTS to the satellite and `pacat` took it at gain 0 (silent). These were people talking in the hall near the mic or false wakes; this is not an accuracy measurement. Two more fired at 11:27 and 11:31 UTC.
- **Sound in that state:** see the table. All five checks in Always Ready were silent at the mic, with the sound output on ARC (volume 12 on a soundbar) and, once, on the TV's own speaker (it fell back to ARC on the next wake, see below). So the cause is **not** the ARC link alone. `pcm_output` and `pcm6p` were `RUNNING` and `pacat` finished normally: the audio reaches the PulseAudio and ALSA layers and is not audible. The TV or soundbar is probably muted or powered down in that state (**hypothesis**; the cause was not isolated, and `audiooutput/audio/getStatus` was empty).

## Rank 2 probes (run because Always Ready did not give audio)

| Probe | Result |
| --- | --- |
| 2a. `sleepd` `activityStart {"id":"d5-hold","duration_ms":60000}` then the API key, Always Ready off | The call answered `{"returnValue":true}`. The TV still went `Active Standby` at +3 s, `Request Suspend` at +7 s, `Suspend`. **No effect** (the suspend came earlier than without the hold). `activityEnd` answered true. Label: negative, verified-on-target |
| 2c. Call `tvpower/power/cancelPowerOff '{}'` in the same second as the first `Request Power Off` | The call answered `{"returnValue":true}`. The TV still went to `Active Standby`. **No effect with `{}`.** Parameters unknown; not tried further. Negative, verified-on-target |
| 2b. Register as a power-request client | Not run (the plan excludes it) |

## Audio and the soundbar (what this unit adds)

- `soundOutput` is `external_arc` here. `hw:LGSIC,1` SpeakerFeedback stayed digital silence (-200 dBFS on all channels) while a `pacat` tone was clearly heard by the mic, so on this unit SpeakerFeedback does not show playback that goes to ARC. The microphone was the measure.
- Setting `sound/soundOutput` to `tv_speaker` through `setSystemSettings` worked (volume 5, `externalDeviceControl false`). **The TV set it back to `external_arc` by itself on the next wake** (the soundbar reconnects over CEC), so a satellite that depends on the TV speaker cannot count on that setting surviving a power cycle (**verified-on-target**, one occurrence). The volume of ARC stayed 12.
- Every tone check restored the master volume to exactly the value read at its start (12 on ARC, 5 on the TV speaker) and left mute off; read back each time.

## aarch64 daemon: first real run on a TV

The package installed with `--arch aarch64` and `uname -m` `aarch64` (`webos-satellite 0.0.1 (aarch64-unknown-linux-musl)`). On c3-b, for 81 min including two kernel suspends:

- **Ran:** ALSA capture with the satellite's own ioctls (S24_LE, 2 ch, 16 kHz, period 256, buffer 8192, same as c3-a), the firmware guard (`03.30.73` validated through the config), the ESPHome API with Noise to HA 2026.10.4, mic streaming to HA's wake word stage, the full wake to STT to TTS chain with `pacat` as the sink (gain 0), an announcement (`assist_satellite.announce`, TLS fetch from HA's URL, 963 ms of audio, in `Active`), the kill switch (PCM `closed` within one poll in each check), and surviving suspend and resume (same pid, PCM `SUSPENDED` then `RUNNING`, HA link kept).
- **Not run:** the watchdog exits, the supervisor, pairing, the echo canceller (off), a reboot.
- Resource use: RSS 1 to 2.6 MB, 12 fds, 3 threads, about 3.3 % of a core when idle and streaming.
- `development.md` still says the aarch64 daemon is pending; that status can now move to "verified on c3-b (a different unit, firmware `03.30.73`)". Not edited in this change (the packaging docs are owned by another change).

## Observations that were not planned

1. **People use the hall TV.** The owner confirmed that someone in the household used the real remote during the run. Presses (not mine, from the input trace and the action log): the TV was turned on at 12:25:59 (reason `remoteKey`), turned off at 12:31:24 (12 s after I had woken it) and at 12:34:36 (about 37 s after I had woken it by WoL). Both off presses came right after my own wake-ups, so they may be a reaction to the TV lighting up. Each of my wake-ups of an Always Ready or suspended TV lights the panel. The tests stopped after that and were not resumed: the owner switched the TV back on to the home screen afterwards, the room is in use, and the daemon was already removed. A further run (a real-remote soak, the wake-and-darken idea) should wait for a quiet window agreed with the owner, with tones rare, short and quiet, and no power-off while content is playing. A voice detection at 12:31:23 came one second before the first off press (`inferred`: someone spoke at the mic and then used the remote).
2. **LG hands-free ("Hi LG", ThinQ) was ON at the end of the run.** It was off when I started (`settingStatus false`, `activationState false` at 11:15 and during all the checks: the satellite held the mic with no `MIC BUSY`). When I read it again at 12:37 it was `settingStatus true`, `activationState true`, `currentTriggerWord hiLG`. I did not change it; a person at the TV may have. I left it as found (the rules say to record it and ask before changing it). With hands-free on, `voiceinput` takes the mic and the satellite would see `MIC BUSY` ([D3](../c3-a/D3-mic-identification.md)).
3. **Wake word runs on room speech.** The satellite streamed the hall's microphone to HA's wake word stage for about 80 min. Eight detections ran the whole chain, so HA's Whisper transcribed room speech. The daemon log keeps only counts; HA keeps pipeline debug data in memory until the pipeline is deleted. **The temporary pipeline was deleted** at the end (it held the transcripts) and I did not read them.
4. **A short exposure on the preferred pipeline.** From 10:16:34 to 10:16:55 UTC (about 20 s), before I could select the test pipeline, the new device streamed to HA's preferred pipeline "Kiwi" (wake word `alexa`; its conversation agent is the Ollama one, down). Nothing was detected in that window. The test pipeline (conversation language `ru`, built-in agent) was selected on both pipeline selects, then the mic switch was cycled to restart the run on it.
5. **The HA pod could reach this TV's port 6053 directly.** The config flow created the entry at once, so no bridge (`ha_bridge.py`) was needed. The rule that opened c3-a evidently covers this address too (**inferred**, the router was not inspected).
6. **PCM `SUSPENDED` at suspend.** The capture PCM went to `SUSPENDED` when the TV suspended and to `RUNNING` within about 20 ms after the resume with no log line at `info`.

## Deviations from the plan

- The first input trace (`od`) was buffered; the key traces start at 12:28. R1a and R1b have no input trace.
- The screen saver and the 30-minute screen-off step were replaced by the screen-saver checks at the start (the TV was in that state) and the 59-minute Always Ready soak. The Quick Start+ off and reboot steps were not run (the TV is the owner's, and a person was using it).
- `alwaysready` 2b, the power draw and the panel observation were not done (no plug meter, nobody looking).
- A person's presses stood in for R2; the real-remote soak and the Magic Remote are still open.
- Tone checks used the TV microphone because SpeakerFeedback is blind to ARC. They paused the satellite for about 8 s each with the kill switch.

## Residual unknowns

1. Does the soak last hours, and does anything end it (OLED care, the `automaticStandBy = 4` timer, `autoOff2HourOnTimer`, the no-signal rules on an HDMI input)? Only 59 min were seen, on the home app.
2. Is the panel dark, and what does the unit draw in `Active Standby` and in `Suspend`? (owner and a plug meter)
3. Why is nothing audible in `Active Standby`: a mute in the amp path, `avmutemanager`, a CEC power-off of the soundbar, or the TV speaker path? **Answered in [D6](D6-poweroff-screenoff.md#variant-c-why-active-standby-is-silent):** the DSP input gain (`Gain Input`) is set to about zero at the power-off request; `avmutemanager` and the volume service report no mute.
4. Is the physical wake from `Active Standby` by WoL possible (`controls/power` and the remote work; WoL was only tried from `Suspend`)?
5. Does WoL wake the TV with `wolwowlOnOff = false`, the owner's original value?
6. Does the **Magic Remote** power key give the same sequence as the IR remote?
7. **Tested later in [D6](D6-poweroff-screenoff.md): it works** (wake with `powerOn` and a reason, then `turnOffScreen`; `Screen Off` about 5.3 to 5.9 s after the key). Original text: reply while looking off by waking and darkening in one step: from `Active Standby` call `controls/power` (3 s to `Active`), then `turnOffScreen` (`Screen Off` played the tone). It needs a test of the panel flash and of the delay before the first sound.

## Restore (verified at the end)

- **Settings:** `general/lifeOnScreenMode` back to `off`, `network/wolwowlOnOff` back to `false`, `sound/soundOutput` `external_arc`; all other recorded settings unchanged (compared with the start: no difference apart from the two items below). Volume 12, not muted. Power state `Active` (it was `ScreenSaver` on the home app at the start, `Active` now after the wake).
- **TV:** the satellite stopped and uninstalled, `/tmp` holds none of our files (`/tmp/webos-satellite`, loops, stop files, tone), no process of ours, no `pacat`, 0 sink-inputs, mic PCMs `closed`, kernel taint `4097` (unchanged), no reboot, no `dmesg` oops. The remaining `tail` on the TV belongs to another app.
- **HA:** the test config entry deleted (HA is back to 43 entries), the temporary pipeline deleted (2 pipelines, "Kiwi" preferred), no config flow left, 0 `c3_b` entities. The three access tokens I minted were revoked by their own refresh-token ids (the API answers 401); no other token was touched.
- **Re-check at 12:40 (after the owner switched the TV on):** power state `Screen Saver` on the home app (the start state), no reboot (uptime 37 369 s), `/tmp` still clean, no daemon or loop running, volume 12 on `external_arc`, not muted, no stream playing, `lifeOnScreenMode off`, `wolwowlOnOff false`, `quickStartMode on`, `turnOnByVoice off`, mic PCM `closed`, kernel taint 4097, `suspend_stats/success` 12.
- **Differences from the start that are not mine:** LG hands-free is on (above), and the `aod` getter now also returns `pre_status`.
