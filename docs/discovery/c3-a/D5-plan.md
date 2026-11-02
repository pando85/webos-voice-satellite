# D5 plan: screen-off and standby behaviour (owner procedure)

- **Status:** procedure only, **not run**. Every step that changes the TV's power or screen state is done by the owner, by hand, with the remote. Agents only observe over SSH (read-only commands listed below) and never change power states, settings or LG services.
- **Device:** c3-a (LG C3, webOS TV 8.3.0, firmware 03.30.74). Results apply to this model and firmware only, and to each state separately.
- **Question (from [EXP-004](../../experiments/EXP-004-power-lifecycle.md)):** in each of screen off, standby (power off with the remote) and Always Ready, does the satellite process survive, does the capture PCM keep delivering audio, does the network stay up, and does HA stay connected? What happens on the way back?
- **Why it matters:** the satellite is only useful if it listens while the screen is off; the autostart design ([autostart](../../design/autostart.md)) assumes nothing about standby.
- **Related:** [P3 robustness](P3-robustness.md) (watchdog, capture stall reopen, reconnection), [D0-D1](D0-D1-runtime.md), [rooted-tv-improvements](rooted-tv-improvements.md) (constraints, item 5 on reboots).

## Safety and privacy

- No sound: the satellite runs with `[sink] type = "null"`. LG hands-free stays off.
- The microphone is open during the test and audio goes to HA's wake word stage only (no recording). Everyone in the room is told; nobody needs to speak. The kill switch (`touch /tmp/webos-satellite.disable`) stops listening at any time.
- Everything runs from `/tmp` (lost at reboot). No persistence, no reboot by the agent. If the TV reboots by itself in standby, that is a result to record, not something to fix on the spot.
- Never read ASoC/DAPM sysfs nodes or `/sys/kernel/debug` (kernel oops hazard, see D2).
- Raw outputs go to `devices/private/c3-a/D5/` (gitignored), never into the repository.

## Prerequisites

1. The owner authorizes the session and has the remote at hand.
2. The satellite is installed under `/tmp/webos-satellite` (`make tv-install-tmp`) with the config used in [P3 robustness](P3-robustness.md) (`mode = "wake"`, ALSA mic, null sink, `level = "info"`).
3. On the PC: `ha_bridge.py` with a short-lived token (the bridge stands in for HA as long as HA cannot reach the TV), or HA itself once the network path is open. A second terminal keeps an SSH session to the TV.
4. Note the TV settings as they are (owner reads them in the menu, nothing is changed by the agent): Always Ready on/off, Quick Start+ on/off, screen saver timing, "Turn off screen" availability.

## Observation commands (read-only, run by the agent)

Start once, in the background, before the first state change:

```sh
# on the TV, one line every 5 s: time, pid, fds, threads, RSS kB, CPU ticks, PCM state, HA connections
sh /tmp/sampler.sh /tmp/d5-sampler.out 5 &
# PCM state transitions with millisecond timestamps
sh /tmp/pcmwatch.sh /tmp/d5-pcm.out &
```

([`sampler.sh`](../../../tools/lifecycle/tv/sampler.sh) and [`pcmwatch.sh`](../../../tools/lifecycle/tv/pcmwatch.sh) are the read-only loops used for P3, copied to `/tmp` on the TV; they read `/proc/<pid>/{fd,status,stat}`, `/proc/asound/LGSIC/pcm0c/sub0/status` and `netstat -tn`, and write only their output file.) Also note on the PC whether `ping` to the TV answers, and the bridge's log. Optional and only if already available: power draw from a plug meter (owner).

## Procedure

Run each state for 5 minutes, then return to normal viewing and wait 2 minutes before the next state. Record the time of each owner action.

| Step | Owner action | What to record |
| --- | --- | --- |
| 0 | TV on, normal picture, satellite running and connected (wake mode) for 2 min | Baseline: sampler lines, PCM `RUNNING`, HA connected, `ping` OK |
| 1 | Screen off (menu "Turn off screen" or the equivalent picture-off option; sound stays on) | Satellite alive? PCM still `RUNNING` and frames flowing (no `no audio from the device` warnings in the log)? HA still connected? Then screen on again: anything in the log (xrun, reopen)? |
| 2 | Standby with Always Ready **off** (power button on the remote) | Does SSH still answer? If yes: satellite alive, PCM state, HA connection, log lines. If SSH does not answer: note the time; after power-on check uptime (`cat /proc/uptime`) to see whether the TV rebooted, whether `/tmp/webos-satellite` still exists, and the log tail |
| 3 | Power on again | Time until SSH, satellite state, PCM reopened (log `microphone open`), HA reconnect and re-arm |
| 4 | Standby with Always Ready **on** (only if the owner already uses it, or agrees to toggle it and restore it afterwards) | As step 2; also whether LG's own wake-on-voice path takes the PCM (`MIC BUSY` in our log) |
| 5 | Power on again | As step 3 |

Expected signals and how to read them:

- PCM `RUNNING` and no stall warnings: capture works in that state.
- `capture: no audio from the device for 5000 ms; closed it, reopening`: the driver stopped delivering periods; the satellite keeps retrying with backoff (up to 30 s). Count the attempts.
- `MIC BUSY`: another process (LG's wake-on-voice) holds the mic in that state.
- `watchdog: the process did not run for a while (stopped or suspended)`: the system suspended the CPU; compare with the sampler gap.
- Sampler gap with the same pid afterwards: the process was frozen, not killed. A different pid: the supervisor restarted it (if run supervised). No pid and `/tmp` emptied: the TV rebooted.
- HA disconnected with `timeout` or `connection closed by peer`, then a reconnect and `requesting pipeline (wake_word=true)` after power-on: recovery works.

## Failure interpretation

- SSH gone in standby: network down in that state; the satellite cannot help in that state, whatever happens to the process. Mark **INCONCLUSIVE** for capture if nothing can be observed, not "fails".
- TV reboots on standby (uptime reset): record it; `/tmp` is gone, so nothing of the test remains. That is also the result for "what a non-persistent install does in standby".
- Kernel warnings in `dmesg` that mention the sound driver: copy them to the private folder, stop the test, tell the owner.

## Rollback

- Stop: `sh /tmp/webos-satellite/bin/stop.sh`; stop the loops (`touch /tmp/p3-sampler.stop /tmp/pcmwatch.stop`); `make tv-uninstall-tmp`; `rm -f /tmp/sampler.sh /tmp/pcmwatch.sh /tmp/d5-*.out`.
- Owner restores any TV setting changed for step 4 (Always Ready) to its noted value.
- Nothing persistent was written by the agent.

## Results template (to fill in)

| State | SSH | Process | PCM / frames | HA connection | Recovery on power-on | Power (W) | Label |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Screen off | | | | | | | |
| Standby, Always Ready off | | | | | | | |
| Standby, Always Ready on | | | | | | | |
