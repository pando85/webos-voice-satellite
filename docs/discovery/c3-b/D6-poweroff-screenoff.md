# c3-b: D6, remote power-off that looks off but still replies ("wake and darken")

- **Device:** c3-b (LG C3 55 inch, webOS TV 8.3.0, firmware `03.30.73`), the owner's hall TV. Results apply to this unit and firmware only. c3-a is not touched.
- **Status:** **plan only (Phase 1, 2026-11-03).** Nothing in this file has been run on the TV yet. The results sections are filled in after the live run and each row there carries its own label.
- **Owner goal:** pressing power off on the remote makes the TV look off, the satellite keeps listening **and** replies are audible. Owner approval to test on this TV: 2026-11-03 (power states, settings and reboot allowed, every original recorded and restored; c3-a never touched).
- **Starting point ([D5 results](D5-results.md)):** with Always Ready on, a power-off ends in `Active Standby`; listening works there (59 min seen) but 5 tone checks were silent. In `Screen Off` (reached by `turnOffScreen` on a running TV) the same tone was audible.
- **Evidence labels:** `documented`, `observed-on-other-device`, `hypothesis`, `verified-on-target`. Anything below that is not `verified-on-target` is not a result.

## Questions

1. **Variant A, wake and darken.** When the TV settles in `Active Standby` after a power-off, a watcher brings it back to `Active` and calls `turnOffScreen`. Does it work, how long does the panel possibly light up, how soon is a reply audible, does a second power press turn the TV on normally?
2. **Variant B, wake only for a reply.** Stay in `Active Standby`; when a reply must play, wake with the screen off, play, and return to `Active Standby`. Same measures, plus the time to the first sound.
3. **Variant C, why is `Active Standby` silent, and is there a path that works there?** Read-only inspection of the audio and mute state in each power state.
4. **Can the watcher tell a user's power-off from other transitions** (auto-off timers, a programmatic power-off)?
5. **OLED panel care and power draw** of keeping the TV in `Screen Off` instead of standby (sources, below).

## What is already known (inputs to the plan)

| Fact | Label | Source |
| --- | --- | --- |
| After the power key (IR remote or API key) the sequence is `Request Power Off` (reason `remoteKey`), `Request Power Off Logo`, `Request Active Standby`, `Prepare Active Standby`, settled `Active Standby` at +3 to +4 s. Always Ready on: stays there (a second `Prepare Active Standby`, reason `oslbSaving`, at +7 s). Always Ready off: `Request Suspend` at +7 to +15 s | verified-on-target (c3-b) | [D5 results, R1](D5-results.md#r1-state-sequences-pc-clock-offsets-from-the-key) |
| `networkinput/controls/power` is a toggle and wakes `Active Standby` in 3 s (`Prepare Power On`, `LastInput Ready`, `Screen On`, `Active`) | verified-on-target (c3-b) | D5 R1 |
| `turnOffScreen` on a running TV gives `Screen Off`; a tone was audible there, and `turnOnScreen` answered `Active` at once | verified-on-target (c3-b) | D5 table per state |
| `cancelPowerOff {}` in the first second of the request and a `sleepd` activity hold did not stop the transition | verified-on-target (c3-b), negative | D5 rank 2 probes |
| `tvpower/power/getClientList` (`/power2`): **`com.webos.service.audio`, `com.webos.service.arccontroller`, `com.webos.service.panelcontroller`, `com.webos.service.oledepl`, `videooutputd`, `com.webos.service.pqcontroller`** register for **PrepareActiveStandby**, and `audio`, `panelcontroller`, `oledepl`, `legacybroadcast`, `BAM`, `asc`, `btaudiosnk` for **ActiveStandbyRequests**. `avmutemanager` registers for PreparePowerOn and PrepareResume only. `oledepl` is also in PowerOffRequests and AlwaysOnDisplayRequests | verified-on-target (c3-a, firmware `03.30.74`; not read on c3-b) | c3-a D5 `getClientList` capture, summarised in [always-on 2](../../design/always-on.md#2-what-was-observed-on-c3-a-read-only-2026-11-03) |
| `tvpower/power/powerOn` and `powerOff` are declared methods; their parameters are not documented anywhere found | declared-on-target (c3-a), parameters unknown | c3-a D5 method list |
| `com.webos.service.avmutemanager` declares `getMuteStatus`, `getMuteOwnerList`, `getMuteInfo`, `getRegisterDB`, `setAvMute`, `updateAvResource`; `com.webos.service.panelcontroller` declares `setScreenOnOff` (in the same permission group as `turnOffScreen`) | declared-on-target (c3-a) | c3-a permission files |
| The TV sets `soundOutput` back to `external_arc` after a power cycle | verified-on-target (c3-b) | D5 |

**Hypothesis behind variant C (not a result):** on `Prepare Active Standby` the audio service (and the ARC controller) mute or power down the output path; `Screen Off` is not a standby, so nothing mutes there. The soundbar is probably told to standby over CEC. The TV-speaker check in `Active Standby` was also silent, so ARC alone is not the cause.

## Research: OLED panel care and power draw

Searched on 2026-11-03 (public pages, owner manuals, forums). **No source found says anything about Always Ready, `Screen Off` or webOS "turnOffScreen" and the pixel refresher.** What is documented:

| Claim | Label | Source |
| --- | --- | --- |
| The Pixel Refresher runs automatically **when the TV is turned off** after more than four cumulative hours of viewing, "for a few minutes" (LG US owner manual); LG's Hong Kong reliability page gives a longer run, about an hour, after 2,000 hours | documented | [LG OLED reliability (HK)](https://www.lg.com/hk_en/tv/oled-tv/oled-reliability/); LG owner manual text quoted at [ManualsLib, LG GX6 p. 19](https://www.manualslib.com/manual/3390863/Lg-Gx6.html?page=19) |
| "The Pixel Refresher function will stop if the TV is turned on while it is in operation"; a short run that is cut is retried at the next turn-off | documented (LG HK page; the retry sentence is from a forum quote of the LG text) | LG HK page above; [LG OLED manual excerpts via forum](https://forum.tomshw.it/threads/lg-oled-funzione-pixel-refresher-help.812194/latest) (snippet-level) |
| It does not start if the TV is unplugged or the main power is off | documented | LG HK page, ManualsLib above |
| A manual run exists in the menu (OLED care, Pixel Cleaning / Pixel Refresher, "operate once when TV is off" or now) | documented, menu names vary by model and webOS version | LG support pages (search result titles only; the US page text was not retrievable) |
| Some TVs start the refresher some hours after a remote power-off, not at once | anecdote, one poster | [hifivision thread](https://hifivision.com/threads/oled-tv-pixel-refresher-run-cycle-analysis-process-that-helps-with-avoiding-burn-in.94427) |
| `Screen Off` (Energy Saving): "Screen is turned off and only sound is played. Press any button except Power button on the remote control to turn the screen back on" | documented (LG Energy Saving help; for the Energy Saving menu entry, not `turnOffScreen`) | [LG Australia, Energy Saving](https://www.lg.com/au/support/product-help/CT20088015-1437132262154) |
| Always Ready: the TV enters it after a remote power-off; "energy consumption may increase" in that mode; "Enable Always Ready Mode without Background Screen" is the dark variant | documented | [LG US, Always Ready (webOS 22)](https://www.lg.com/us/support/help-library/lg-tv-how-to-use-the-always-ready-feature-webos-22--20153280995321) and LG regional Always Ready pages |
| LG's own spec sheets list C3 standby under 0.5 W and "operational" about 95 W for 55 inch (115 W for 65 inch); Always Ready has no separate figure | documented (spec sheets) | retailer and LG listings, for example [LG C3 65 inch listing](https://usm.channelonline.com/arlingtoncp/tech/Products/overview/M024276888) |
| **Measured by owners, one 42 inch C2 on a smart plug (non-professional):** about 8 W in standby with Quick Start and Always Ready on, about 12 W with both off (still 12 W after days), 20 to 30 W for about 5 minutes after a power-on (a cleaning phase), 0.5 W right after plug-in; another poster saw 12 W falling to 0.5 W within 10 s after entering standby. Meter accuracy at that level was disputed in the thread | observed-on-other-device (C2, not C3, forum, uncontrolled) | [choice.community, LG C2 standby power](https://choice.community/t/lg-c2-oled-high-power-consumption-in-standby/31045) |

**Reasoning (hypothesis, not tested):**

1. A watcher that wakes the TV 3 to 6 s after each power-off means the TV never rests in standby after viewing. If the refresher is tied to the power-off sequence (LG: "when you turn off the TV"; `oledepl` sits in `PowerOffRequests`), it would be cut by the "turned on while in operation" rule every time, and the four-hour counter would keep growing with no completed cycle. `Screen Off` is a running state, so no turn-off event happens while the screen is dark.
2. Panel emission is off in `Screen Off` (`turnOffScreen` is documented in the webOS tooling as "Turn off Tv screen (inverter off)", from the secondscreen interface text read on c3-a), so the wear of an OLED in use is not accumulated while dark. The **compensation** side (panel noise, long cycle) is what could be missed.
3. Power: the TV is "on" (SoC, network, mic DSP, soundbar link) in `Screen Off`. No source gives its draw. The nearest numbers are a spec of under 0.5 W for standby and an owner's 8 W for Always Ready on a C2, so expect the dark-and-running state to draw clearly more than standby; **no plug meter is available, so this stays unmeasured**.
4. Mitigations that need no private API (design input): leave a real standby window (for example the watcher is off 03:00 to 05:00 so the refresher can run), tell the owner to run Pixel Cleaning from the menu now and then, and make the setting off by default.

## Measures for every run

| Measure | How | Limits |
| --- | --- | --- |
| Panel lights up, and for how long | **Not observable by any sensor here (nobody is looking).** Infer from the watcher's stamped power-state lines: the interval from the first `Screen On` line (or `Prepare Power On`) to the settled `Screen Off`, which is an **upper bound** of the lit window, plus the interval between settled `Active Standby` and the first `Request Power Off Logo`. Also read `picture/backlight`, `picture/oledLight` and `energySaving` per state in the snapshot. **Say in the results: not verified visually.** The owner can look once at the end | Inferred only |
| Latency | Watcher log (TV clock, ms): `Request Power Off`, settled `Active Standby`, wake call, settled `Active`, `turnOffScreen` call and reply, settled `Screen Off`. PC clock stamp at the key | 10 ms resolution on the TV |
| Audible reply | The TV's own microphone, as in D5 (kill switch pauses capture, `arecord` on `hw:LGSIC,0`): beeps (0.15 s, 1 kHz, 0.2 full scale, one every 0.5 s for 12 s, TV volume about 9, original restored) started at the key, analysed with `tools/lifecycle/d6_tools.py beeps`. Gives the **first audible beep** after each event. Plus one `assist_satellite.announce` (short text) in the final state | The mic hears the room; threshold -60 dBFS in the 1 kHz band (D5: audible -31 to -36, silent below -53) |
| Watcher can tell the cause | `reason` of each `Request Power Off`; the input node that saw the key (IR `LGE RCU` `event1`, API and Magic Remote `event9`, passive `cat`, no grab); the reason of a programmatic `tvpower/power/powerOff {}` call | An auto-off timer cannot be triggered in a test session; its reason stays unknown |
| Second press | From the watcher's `Screen Off`: `controls/power` with the watcher in `darken` mode (second-press rule) and in `observe` mode (what the TV does by itself). Also `turnOnScreen` | The agent cannot press the IR remote; the API key matched the IR key in D5 for the power-off from `Active`. Whether it matches from `Screen Off` is **unverified** |

## Variants and what is run

All with the satellite running (wake mode, mic open), `soundOutput = external_arc`, hands-free left as the household set it.

| ID | Setting | Trigger and action | Runs |
| --- | --- | --- | --- |
| A1 | Always Ready `alwaysReady` | key power-off, watcher `darken`, wake `powerOn`, darken at settled `Active` | 3 |
| A2 | same | darken earlier: at `Prepare Power On`, `LastInput Ready` or `Screen On` (shortens the lit window if the TV accepts it) | 1 each |
| A3 | same | wake with the key instead of `powerOn` (if `powerOn {}` is refused) | 1 |
| A4 | **Always Ready off** (the unit's own setting) | same as A1; does the wake win the race against `Request Suspend` at +7 s? A failed race means Suspend: wake by WoL (set to true for the run) | 1 |
| B | Always Ready `alwaysReady` | stay in `Active Standby`; at T0 start beeps and capture, wake (`powerOn`), `turnOffScreen`, then power-off again by key from `Screen Off`; time to first audible beep, time to `Active Standby` again | 2 |
| C | all of the above | read-only snapshots in `Active`, `Active Standby`, `Screen Off`, and during the wake | 1 per state |
| S1 | A1 end state | second press in `darken` mode | 2 |
| S2 | A1 end state, watcher `observe` | second press: what the TV does by itself | 1 |
| P1 | observe | key power-off (reason), `tvpower/power/powerOff {}` (reason, if accepted) | 1 each |
| D | A1 end state | dwell 20 min with the satellite listening; sampler, PCM, HA link; then one tone and one announcement | 1 |

## Procedure

**Gate (every power-off):** the TV is not being watched. Read `getPowerState` (`Active` or `Screen Saver`), the foreground app (home or screen saver), `pactl list short sink-inputs` (0), and count input bytes for 60 s (`timeout 60 cat /dev/input/event1 | wc -c` and `event9`, expect 0, no grab, not kept). If anything suggests use, wait 10 minutes and repeat. If a person walks in during a run, stop, wake the TV with `turnOnScreen` or `controls/power`, and report.

**Phase 1 (done on the PC only):** the tools below, a stub test of the watcher (`sh tools/lifecycle/tv/test_d6_watcher.sh`, 6 cases: observe makes no call; darken makes `powerOn` then `turnOffScreen`; an unaccepted reason makes no call; a second press from our own `Screen Off` wakes without darkening; `powerOn` that does nothing falls back to the key only while the state is still `Active Standby`; the hold file blocks), `d6_tools.py selftest`, `sh -n` and `shellcheck`.

| Tool | Where it runs | What it does |
| --- | --- | --- |
| `tools/lifecycle/tv/d6-watcher.sh` | TV, from `/tmp/d6` | the watcher: `observe` or `darken`, safety limits (max actions, cooldown, hold file, stop file, state-checked key) |
| `tools/lifecycle/tv/d6-snapshot.sh LABEL` | TV, via `ssh -tt` | read-only getters (power, volume and mute, sound output, `avmutemanager` getters, `pactl list sinks`, `amixer -c 0 contents`, PCM 6 status) into `/tmp/d6/snap-LABEL.txt` |
| `tools/lifecycle/tv/d6-beeper.sh` | TV | the beeps with a stamped log |
| `tools/lifecycle/d6_tools.py` | PC | `tone` makes the tone file, `beeps` rates each beep from the mic capture |
| `tools/lifecycle/tv/sampler.sh`, `pcmwatch.sh` | TV | existing read-only loops (process, PCM, HA link) |

**Phase 2 (after the orchestrator's GO):**

| Step | Action | Record |
| --- | --- | --- |
| 0 | Gate. Record originals into the private folder: `lifeOnScreenMode` (off), `wolwowlOnOff` (false), `soundOutput` (`external_arc`), volume and mute, hands-free state (on, leave it; if `MIC BUSY` blocks capture, turn it off for the run and restore on), `quickStartMode`, `picture/energySaving`, power-on reason, kernel taint | private originals file |
| 1 | Way back: set `wolwowlOnOff` true (read back); the wake MAC file from D5 checked against the TV's wired MAC; `controls/power` and `turnOnScreen` known to work. Ladder if the TV does not answer: key, WoL burst from the PC, then the owner | |
| 2 | Read-only discovery on the TV: `ls /usr/share/luna-service2/api-permissions.d` filtered for `audio`, `avmute`, `panel`, `oled`; read `com.webos.service.avmutemanager.api.json`; `ls /sys/class/backlight` (one level, names only). No other `/sys` or `/proc` listing | method names |
| 3 | HA: mint a token (`pass web/hass.grigri.cloud`, 0600 scratchpad file, never printed). **Create the temporary pipeline first** (language `ru`, conversation agent `conversation.home_assistant`, the same engines as the D5 test pipeline). Install the satellite with `make tv-install-tmp TV_ARCH=aarch64` and a private config with `validated_firmware = ["03.30.73"]` (valid for this test only). **Start it muted:** create `/tmp/webos-satellite.disable` before the first start so no audio reaches HA, add the config entry, select the `ru` pipeline on both pipeline selects, check them, then remove the kill switch and unmute from HA | HA entry id, pipeline id |
| 4 | Loops: sampler, `pcmwatch`, passive `cat` of `event1`/`event9`, a PC `ping` and a stream of the TV files to the private folder (as in D5) | |
| 5 | P1/C: watcher in `observe` (`ssh -tt`; 15 s dry run first to see lines arrive). Snapshot `active`; API key power-off; snapshot `standby` at +10 s; `controls/power`; `tvpower/power/powerOff {}` once (reason); `turnOffScreen`, snapshot `screenoff`, `turnOnScreen`. Compare snapshots (mute flags, sink state, volume `activeStatus`, controls) | reasons, diff of snapshots |
| 6 | A1 x3, then A2, A3: watcher `darken`; beeps and capture started at the key; `controls/power` as the key; 90 s between runs; return with `turnOnScreen` | latencies, first audible beep, calls and replies |
| 7 | S1 x2, S2: second press | state sequence, time to `Active`, whether the screen state is normal |
| 8 | B x2 | time to first audible beep, to `Active Standby` again |
| 9 | A4 (Always Ready off) x1 | race result; recovery by WoL if needed |
| 10 | D: A1 end state for 20 min, then a tone check and one `assist_satellite.announce` (short text) | PCM state, HA link, timers, audible |
| 11 | Restore every original (Always Ready, WoL, sound output, volume), read each back, state `Active` on the home screen. Stop and remove the satellite and `/tmp/d6`, `/tmp` check. HA: delete the config entry, the pipeline, the select states; revoke **only my own** refresh tokens by id | restore table |

**Rollback / stop conditions:** a person needs the TV; the TV does not answer ping after 60 s (ladder above); a kernel oops (`dmesg` tail shows one) stops all audio experiments and asks the owner for a reboot; the watcher goes wrong (a loop of power-offs): `touch /tmp/d6/stop` and `touch /tmp/d6/hold`, then `sh /tmp/d6/d6-watcher.sh stop`.

**What each outcome means:** a `powerOn` reply of `returnValue: false` or no state change is a result (the call or its parameters are wrong), not a failure of the idea; fall back to the key (A3). A wake that lands before `Request Suspend` (A4) says Always Ready is not needed for the idea. A beep audible before settled `Active` shows the audio path comes back early and the reply could start sooner than the full wake.

## Results

Not run yet. To be filled in: per variant (works, lit-window upper bound, latency, audible, second press), the snapshot diff (variant C), the reasons table, the dwell, the restore table, the commit list. Panel state remains **unverified visually** unless the owner looks.
