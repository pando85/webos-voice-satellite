# D5 plan: screen-off, standby and Always Ready (agent-runnable procedure)

- **Status:** procedure only, **not run**. Revised 2026-11-03: the owner allows agents to change the TV's power state (screen off/on, standby, power-on), to change TV settings and to reboot the TV whenever a test needs it. The orchestrator schedules the run, because the TV is the owner's daily PC display. The design this plan feeds is [always-on](../../design/always-on.md).
- **Device:** c3-a (LG C3, webOS TV 8.3.0, firmware 03.30.74). Results apply to this model and firmware only, and to each state separately.
- **Questions (from [EXP-004](../../experiments/EXP-004-power-lifecycle.md) and [always-on section 9](../../design/always-on.md#9-open-questions-top-three-need-the-d5-session)):** in each state, does the satellite process survive, does the capture PCM keep delivering audio, does the network stay up, does HA stay connected, and **does a short reply sound**? What happens on the way back? Can the TV be woken without the remote?
- **Related:** [P3 robustness](P3-robustness.md) (watchdog, capture stall reopen, reconnection), [D0-D1](D0-D1-runtime.md), [rooted-tv-improvements](rooted-tv-improvements.md), [autostart](../../design/autostart.md).

## Safety and privacy

- **The TV is in daily use.** Run only in a window the orchestrator has agreed with the owner, with the owner told in advance. Stop at once if the owner needs the TV.
- **Way back before any standby (hard gate, see [below](#way-back-gate-before-any-standby-step)).** The remote is the last fallback, not the plan.
- **Reboots and settings changes are allowed when a test needs them** (owner, 2026-11-03). Every setting that is changed (Always Ready, Quick Start+, Turn on via Wi-Fi/WoL, screen-off options, auto-off timers, and so on) is **read first, recorded in the private folder, and restored to the exact original value** at the end of the session and on any abort, and read back after each write. A reboot is done through a normal path (the TV's restart or HBC's reboot), never by cutting power, and only for a step that needs it (step 9). If the TV reboots by itself, record it and do not fix it on the spot.
- **Still forbidden:** firmware writes and updates (the update block stays as it is), stopping or disabling LG services, reading ASoC/DAPM sysfs or debugfs, persistence of anything of ours (the autostart hook is a separate approval under [autostart](../../design/autostart.md)). **LG hands-free stays off** unless a step names it, and is then restored to off.
- **`/tmp` is wiped by a reboot**, and so are the satellite, the sampler loops and their output. Keep the logs on the PC: stream the loops' output over SSH to the private folder instead of leaving them only in `/tmp`, and re-install after each reboot with `make tv-install-tmp` (it is the same non-persistent install).
- **Sound:** none during steps 0 to 7 except the playback checks. Playback checks are short (at most 2 s), quiet, announced to the owner, and follow the [volume protocol](#playback-check-short-and-quiet). The satellite otherwise runs with `[sink] type = "null"`.
- The microphone is open and audio goes to HA's wake word stage only (no recording). Nobody needs to speak. The kill switch (`touch /tmp/webos-satellite.disable`) stops listening at any time.
- Everything runs from `/tmp` (lost at reboot). No persistence by the agent.
- Never read ASoC/DAPM sysfs nodes or `/sys/kernel/debug` (kernel oops hazard, see [D2](D2-audio-stack.md)).
- Raw outputs go to `devices/private/c3-a/D5/` (gitignored). The TV's MAC, IP, hostname, the HA token and the webostv client key stay in private files and are never written into the repository, logs, commits or issue text.

## Way-back gate (before any standby step)

Standby steps (4 onward, and the cold-boot step 9) are not started until every item is true and written in the private log. Network-up states (screen off, screen saver) need only the first two.

1. **SSH and luna work now** (`ssh -tt` getter of `getPowerState` returns `Active`).
2. **A restore for the screen exists without the remote:** `power/turnOnScreen` over SSH (network stays up in screen off, expected), and any remote key as the owner's fallback.
3. **A wake path from standby is prepared, from the PC:**
   - the TV's wake MAC (the Wi-Fi MAC if the TV is on Wi-Fi, the wired MAC if on Ethernet; both kept in a private file, and the right one confirmed from the TV's active interface) and a WoL sender on the PC (tool checked, packet sent to the subnet broadcast address and also to the TV's address; several packets in a burst, as users report a single packet can be missed);
   - the HA webostv entity for this TV: confirm it exists and which turn-on action it uses (`kubectl` context in the owner's homelab note). **HA's turn-on on webOS is WoL as well**, so WoL and HA are one path, not two;
   - read on the TV: `network/wolwowlOnOff` (true on 2026-11-03) and `getPowerOnReason` (was `wakeOnWiFi`), recorded.
4. **The owner is reachable** and knows the planned time of each standby step, and the remote is within reach. The owner's remote is the last fallback.
5. **Rehearsal:** the first standby entry is short (60 s), by the safest method (below), with a wake attempt at 60 s. The wake ladder: WoL burst at 60 s; if the TV does not answer ping/SSH by 120 s, a second burst and the HA turn-on; at 180 s ask the owner to press the remote. **If the first wake by WoL fails, no other standby step is run**; the result is recorded as "no network wake from this state" and the plan continues only with network-up states.
6. If the TV does not come back and the owner is not available, stop and report; do not retry in a loop.

## Entering and leaving states (methods and what is uncertain)

| Goal | Method for the agent | Notes |
| --- | --- | --- |
| Read state | `ssh -tt`, `luna-send -n 1 -w 4000 -f luna://com.webos.service.tvpower/power/getPowerState '{}'` | Needs a terminal on c3-a. Getter |
| Log every state change | `luna-send -i luna://com.webos.service.tvpower/power/getPowerState '{"subscribe":true}'` from a background SSH session, each line stamped with `date +%s`, written to `/tmp/d5-power.out` | Whether the subscription prints over a pipe or needs a terminal is unknown; if silent, run it under `ssh -tt`. Whether it **survives suspend** is itself a result |
| Screen off / on | `luna://com.webos.service.tvpower/power/turnOffScreen` / `turnOnScreen` with `{}` | Declared on c3-a, not yet called. Alternative: the HA webostv "Screen" switch (disabled by default) |
| Standby (like the remote) | 1) HA `media_player.turn_off` on the webostv entity, or 2) `ssap://system/turnOff` through aiowebostv from the PC with the private client key. Not `power/powerOff` unless its parameters are first read from a source (hypothesis that it differs from the key) | LG says Always Ready's Background Screen is triggered only when the TV is turned off by the **remote**; an API turn-off may land in Suspend instead. Record what `getPowerState` reports |
| Always Ready by the remote key | Only the owner (remote) unless a button command is shown to work | Needed to test "Always Ready entered by the real key" |
| Toggle a setting | `luna://com.webos.settingsservice/setSystemSettings` with the category and key from the key map (for example `{"category":"option","settings":{"quickStartMode":"off"}}`), after the getter read and the private record; read back | `setSystemSettings` is a write: allowed for the listed settings only, always restored. The TV's own menu is the alternative. Some settings may need the menu (a rejected write is a result) |
| Reboot | The TV's restart, or HBC's reboot, with the owner told; not a power cut | After a reboot, wait for SSH, re-install the satellite (`make tv-install-tmp`), restart the loops |
| Wake | WoL burst; HA turn-on; remote as last fallback | |

## Prerequisites

1. The orchestrator confirms the time window with the owner. The way-back gate holds (items 1 to 3 at least).
2. The satellite is installed under `/tmp/webos-satellite` (`make tv-install-tmp`) with the config used in [P3 robustness](P3-robustness.md) (`mode = "wake"`, ALSA mic, `[sink] type = "null"` for the observation steps; the playback checks switch to `pacat` with a gain, see below), `level = "info"`.
3. On the PC: `ha_bridge.py` with a short-lived token, or HA itself (the permanent entry `lg-c3-a` connects directly). A second terminal keeps an SSH session to the TV.
4. Record the settings **before touching anything** (getters, private file): `option/quickStartMode`, `general/lifeOnScreenMode`, `network/wolwowlOnOff`, `option/screenOff`, `option/screenOffTime`, `option/turnOnByVoice`, `general/screenSaverEnabled`, `time/{noSignalAutoOff,autoOff15Min,noOperationAutoOff,autoOff2HourOnTimer,automaticStandBy}`, `picture/energySaving`; `com.webos.service.audio/master/getVolume` (the owner reported 89 on 2026-11-03, very loud) and the mute state; the foreground app and input (the owner's PC on HDMI).
5. Note which input the TV is on. Screen-off on an HDMI input with no signal may trigger the no-signal auto-off, so the soak runs on an input that has signal or on an app (hypothesis, see [always-on](../../design/always-on.md#4-what-always-on-realistically-means-and-recommended-modes)).

## Observation commands (read-only)

Start once, before the first state change:

```sh
sh /tmp/sampler.sh /tmp/d5-sampler.out 5 &      # time, pid, fds, threads, RSS, CPU ticks, PCM state, HA connections
sh /tmp/pcmwatch.sh /tmp/d5-pcm.out &            # PCM state transitions, millisecond stamps
# power-state log (subscription), stamped, from a background SSH session:
luna-send -i luna://com.webos.service.tvpower/power/getPowerState '{"subscribe":true}' | while IFS= read -r l; do echo "$(date +%s) $l"; done >> /tmp/d5-power.out &
```

([`sampler.sh`](../../../tools/lifecycle/tv/sampler.sh) and [`pcmwatch.sh`](../../../tools/lifecycle/tv/pcmwatch.sh) are the read-only loops used for P3.) On the PC: a stamped `ping` of the TV, the bridge or HA satellite state, and the stamps of every agent action. Optional: power draw from a plug meter (owner or homelab) in each state.

## Procedure

Each state runs for 5 minutes (step 8 is the long soak), then the TV returns to Active with the original screen state, then 2 minutes pass before the next step. Stamp each action.

| Step | Action | Record |
| --- | --- | --- |
| 0 | Baseline: TV on, satellite connected in wake mode for 2 min; settings, volume and input recorded; subscription started | Sampler lines, PCM `RUNNING`, HA connected, `ping` OK, subscription output format |
| 1 | **Screen off** with `turnOffScreen`. Watch the state | `getPowerState` value (`Screen Off`?); process alive; PCM `RUNNING`, frames flowing (no `no audio from the device` warnings); HA connected; SSH answers. Then **playback check A**. Then `turnOnScreen`: log lines on return (xrun, reopen) |
| 2 | **Screen saver** (`turnOnScreenSaver` or wait for it): same observations as step 1 | State string; compare with step 1. Playback check B |
| 3 | **Screen off for 30 min** (watch timers) | How long the state lasts; whether the screen comes back by itself; auto-off or screen-saver transitions; power draw |
| 4 | **Way-back gate complete.** Always Ready as it is now (on): standby by the API method (HA turn-off or `ssap`), 60 s rehearsal, then wake by WoL | State sequence (`Active Standby`? `Suspend`? something else); SSH during the state; process pid after wake (same pid = frozen, new pid = restarted, no `/tmp` = rebooted); PCM, HA reconnect and re-arm times; what the log says (`watchdog: the process did not run for a while`, `microphone open`, reconnect lines); `getPowerOnReason` after wake |
| 5 | **Longer standby (5 min)** by the same method, only if step 4 woke cleanly. Wake by WoL | As step 4 plus: does the network answer during the state (ping, SSH, TCP 6053)? Does it vanish after a few minutes? |
| 6 | **Always Ready off** (read the value; set `general/lifeOnScreenMode` to its own `none` value; read back), standby by the API method, 5 min, wake | As step 5, and the contrast with step 5. **Restore `lifeOnScreenMode` to the original value and read it back** |
| 6b | **Quick Start+ off** (read `option/quickStartMode`, set `off`, read back; with Always Ready off as in step 6): standby by the API method, 5 min, wake by WoL | Does the process die (cold start: pid gone, `/tmp` empty, uptime reset), how long to SSH, is the network up in standby, does WoL work (the HA reports say it does with Quick Start+ off). Re-install after the boot. **Restore `quickStartMode` and `lifeOnScreenMode`** |
| 7 | **Always Ready entered by the remote key** (owner, only if the orchestrator schedules it): LG shows the Background Screen; observe 5 min, including whether LG's voice daemon takes the PCM (`MIC BUSY` in our log). Playback check C. Wake with the remote or WoL | State value in this mode; process, network, PCM; any `MIC BUSY`; sound or not |
| 8 | **All-day screen-off soak** (below) | |
| 9 | **Reboot recovery** (only after the others; a normal restart, then, if the autostart hook has been installed separately, whether the satellite returns without action; otherwise a manual re-install) | Time to SSH, `/tmp` wiped, `getPowerOnReason`, HBC `startup.sh` ran again, failsafe flag absent; with the hook installed: the satellite's return time and HA state |

If a step is inconclusive (nothing observable because SSH is gone), mark it **INCONCLUSIVE**, not "fails".

### Playback check (short and quiet)

Done in steps 1, 2 and 7, and in step 4 or 5 only on return to Active (the process is frozen in standby, nothing can play there). Purpose: does a reply-equivalent sound reach the speaker in that state?

1. Read the master volume and mute with `com.webos.service.audio/master/getVolume` and record them. The expected original is **89**; if it differs, record the value read and use that as the original.
2. Tell the owner. Set the volume to **8 to 10** with `luna://com.webos.audio/setVolume {"volume": N}` (the method used in [P2 session 1](P2-session1-audible-and-echo.md)), then read it back.
3. Play one 1 kHz tone, at most 2 s, at low level, with `pacat` (software gain 0.1 to 0.3; `pacat --volume` is ignored on c3-a), and capture the mic and SpeakerFeedback levels the same way P2 session 1 did, so that "sound present" is measured, not guessed. The owner confirms whether it was audible.
4. **Restore the exact original volume** with `setVolume` and read it back. Verify mute is unchanged. Do this also on every abort path (a `trap` on the PC side and a check at the end of the session).
5. Never raise the volume above 10 for these checks without a new approval.

### All-day screen-off soak (step 8)

- **Purpose:** the evidence for "always on" in mode A: survival over hours, idle drift, timers, reconnects, power draw.
- **Conditions:** needs an owner-approved window (the TV's display is the owner's); screen off by `turnOffScreen`; TV on an input or app with signal; satellite in wake mode with the null sink (or the real sink if the owner agrees to occasional quiet replies); sampler every 30 s; subscription log; an hourly read-only check of `getPowerState`, `/proc/uptime`, satellite RSS and fds; a stamped HA satellite state; plug meter if available. Target 8 hours (overnight or a work day), at least 2 hours for a first pass.
- **Record:** state changes and what caused them (timer, screen saver, remote); HA reconnects and their causes; capture stalls; RSS and fd trend; false wakes (HA's count); power draw average; any suspend (`watchdog` line, sampler gap).
- **Stop:** the owner needs the TV (turn the screen on, end the run), a kernel warning, or the satellite leaves its limits.
- A single reply-equivalent playback check is allowed at the middle of the soak (same volume protocol) to test "first sound after a long idle".

## Expected signals and how to read them

- PCM `RUNNING` and no stall warnings: capture works in that state.
- `capture: no audio from the device for 5000 ms; closed it, reopening`: the driver stopped delivering periods; the satellite retries with backoff (up to 30 s). Count the attempts.
- `MIC BUSY`: another process (LG's wake-on-voice) holds the mic in that state.
- `watchdog: the process did not run for a while (stopped or suspended)`: the system suspended the CPU; compare with the sampler gap.
- Sampler gap with the same pid afterwards: the process was frozen, not killed. A different pid: the supervisor restarted it (if run supervised). No pid and `/tmp` emptied: the TV rebooted.
- HA disconnected with `timeout` or `connection closed by peer`, then a reconnect and `requesting pipeline (wake_word=true)` after the wake: recovery works.
- Power-state log: the sequence of states and the time between them (compare with `Active Standby` 9 s before `Suspend` on a C4).

## Failure interpretation and abort rules

- SSH gone in standby: network down in that state; the satellite cannot help in that state, whatever happens to the process. Mark **INCONCLUSIVE** for capture, not "fails". Wake by the ladder.
- TV does not wake by WoL: record "no network wake", ask the owner for the remote, run no more standby steps.
- TV reboots on standby (uptime reset): record it; `/tmp` is gone; that is also the result for "what a non-persistent install does in standby".
- Kernel warnings in `dmesg` that mention the sound driver: copy them to the private folder, stop, tell the owner.
- Any unexpected owner-visible effect (sound, picture stuck, an LG error dialog): stop and report.

## Rollback

- Screen: `turnOnScreen`; settings: restore each changed value from the private record and read it back; volume: restore the exact original and read it back.
- After a reboot step: re-run `make tv-install-tmp` for the next steps; restore the original values of every setting changed (Quick Start+, Always Ready, anything else) and read them back before ending the session.
- Stop: `sh /tmp/webos-satellite/bin/stop.sh`; stop the loops (`touch /tmp/p3-sampler.stop /tmp/pcmwatch.stop`), end the subscription; `make tv-uninstall-tmp`; `rm -f /tmp/sampler.sh /tmp/pcmwatch.sh /tmp/d5-*.out`.
- Nothing persistent was written by the agent.

## Results template (to fill in)

| State | SSH | Process | PCM / frames | HA connection | Playback | Wake / recovery | Power (W) | Label |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Screen off | | | | | | | | |
| Screen saver | | | | | | | | |
| Screen off 30 min / soak | | | | | | | | |
| Standby, Always Ready on (API) | | | | | n/a | | | |
| Standby, Always Ready off (API) | | | | | n/a | | | |
| Standby, Quick Start+ off (cold start) | | | | | n/a | | | |
| Reboot recovery | | | | | n/a | | | |
| Always Ready by remote | | | | | | | | |

Known on c3-a before this run (read-only, 2026-11-03, see [always-on section 2](../../design/always-on.md#2-what-was-observed-on-c3-a-read-only-2026-11-03)): Quick Start+ on, Always Ready on, `wolwowlOnOff` true, last power-on reason `wakeOnWiFi`, state `Active`. Earlier docs that wrote "Always Ready is `none`" meant the `alwaysready/aod/getStatus` status, not the setting.
