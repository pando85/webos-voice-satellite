# c3-b: D6, remote power-off that looks off but still replies ("wake and darken")

- **Device:** c3-b (LG C3 55 inch, webOS TV 8.3.0, firmware `03.30.73`), the owner's hall TV. Results apply to this unit and firmware only. c3-a is not touched.
- **Status:** plan written in Phase 1; **run on c3-b on 2026-11-03 (Phase 2)**. The plan sections are kept as written; two things in them were wrong or incomplete and are corrected under [Corrections to the plan](#corrections-to-the-plan). Results are in [Results](#results). Raw logs: gitignored `devices/private/c3-b/D6/`.
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

### Answer

**Yes: the TV can be made to look off after the remote's power key while the satellite keeps listening and replies are audible, by waking it and turning the screen off ("wake and darken").** On c3-b it ended in `Screen Off` 5.3 to 5.9 s after the key in 7 of 7 wake-and-darken runs (A1 x3, A2 x3, A4 x1). Whether the panel is really dark, and the power draw, were **not observed** (nobody looking, no plug meter).

| Variant | Works | Panel lit (upper bound, indirect) | Latency | Reply audible | Second press |
| --- | --- | --- | --- | --- | --- |
| **A1** wake and darken, Always Ready `alwaysReady`, darken at settled `Active` | Yes, 3 runs: A1-1 used the 4 s toggle-key fallback; A1-2 started from `Screen Off` (so it was really a second press with a freshly started watcher); A1-3 is the clean run | 0.31, 0.31, 0.67 s | settled `Active Standby` at +3.0 to +3.3 s after the key; `Active` +5.1 to +5.6 s; `Screen Off` +5.4 to +5.9 s (+8.1 s in the first run) | **Yes, after the wake**: silent from the key (+0.0 s) until about 2.2 to 3.3 s after `Active`; the first audible beep started at +7.9 to +8.4 s after the key (+10.4 s in the first run) | see S below |
| **A2** darken earlier: at `Screen On`, `LastInput Ready` or `Prepare Power On` | Yes, 1 run each; the TV accepted `turnOffScreen` that early | 0.28, 0.38, 0.28 s | `Screen Off` +5.3 to +5.5 s | Yes, first audible beep +7.4 to +8.3 s | as A1 |
| **A3** wake with the toggle key instead of `powerOn` | Only as the fallback in A1-1 (it worked); not run on purpose | 0.31 s | `Prepare Power On` at +7.7 s (4 s fallback delay) | Yes | |
| **A4** wake and darken with **Always Ready off** (the unit's own setting) | **Yes**, 1 run: the wake at +4.9 s beat the `Request Suspend` at about +7 s; network stayed up | 0.63 s | `Screen Off` +5.6 s | Yes, first audible beep +8.3 s | not run |
| **B** stay in `Active Standby`, wake for a reply | Yes, 2 runs | 0.42 s (B-1) | `powerOn` accepted 0.13 to 0.16 s after the request (the standby had long settled), `Active` +0.30 to +0.36 s, `turnOffScreen` +0.55 to +0.61 s, gain restored +0.84 s; **first audible beep +2.75 s (B-1) and +3.77 s (B-2)**; back in `Active Standby` 3.1 s after the return key | Yes, from about 3 to 4 s after the request; muted again at the return key | not applicable |
| **C** why `Active Standby` is silent | see [Variant C](#variant-c-why-active-standby-is-silent) | | | | |

"Panel lit" is the time the panel controller's `getScreenMuteStatus` read `false` after the wake (it is `true` in `Active Standby` and `Screen Off`, `false` in `Active`), polled about every 0.1 s on the TV. It is an **upper bound from a getter whose physical meaning is not established**. **Not verified visually.** `isOledScreenVisible` stayed `true` in every state and is not informative.

### S: second press and the power key in `Screen Off`

| Test | Result |
| --- | --- |
| **S1**, watcher `darken`, TV in the `Screen Off` the watcher made, API key | `Request Power Off` (reason `remoteKey`) from `Screen Off`, settled `Active Standby` at +3.0 s, second-press rule: `powerOn` 1.7 s after the settle (the watcher waited out the 1.5 s key block), `Active` at **+5.06 s after the key**, no darkening, state `Active` (the panel unmuted). The TV came back on normally |
| **S2**, watcher `observe` (the TV by itself), TV in `Screen Off`, API key | The same sequence ends in `Active Standby` at +3.0 s and stays there. So **without the watcher a power press in `Screen Off` turns the TV off, and the user would need a second press to turn it on** |
| `turnOnScreen` 0.4 s after the key from `Screen Off` | Refused: `com.webos.service.tvpower/power is busy`. The sequence cannot be cut short |
| IR remote | **Not tested** (the API key matched the IR key from `Active` in D5; from `Screen Off` it is unverified) |

So the second press costs about 5 s instead of the normal 0.5 s. Audio after the second press was not checked.

### Variant C: why `Active Standby` is silent

Read-only snapshots (`d6-snapshot.sh`) in `Active`, `Active Standby` (+10 s) and `Screen Off`, compared:

| Item | `Active` | `Active Standby` | `Screen Off` |
| --- | --- | --- | --- |
| ALSA mixer `Gain Input` (card 0, numid 76, 12 channel/gain pairs) | all 12 at 8388608 (unity) | **8 of 12 at 3** (about zero) | all 12 at 8388608 |
| avmutemanager `getMuteInfo`: `audioMuteStatus` | false | false (a second context appears, also false) | false |
| `audio/master/getVolume`: `muteStatus`, `activeStatus` | false, true | read in the run before: unchanged | unchanged |
| `panelcontroller/getScreenMuteStatus` | false | true | true |
| `oledepl/getEplStatus` | false | not different | |
| PulseAudio and ALSA playback | idle | `pcm_output` and `pcm6p` ran in D5 and `pacat` finished | |

Timing from the audio watcher (`d6-audiowatch.sh`, polls `amixer cget` and the avmutemanager getter every 0.2 s): `Gain Input` goes to the low values **at the `Request Power Off` line (+0.00 s, in 3 runs, within the poll step)** and is restored **0.2 to 0.5 s after `Prepare Power On`**, just before `Active`. The tone is audible **about 2.1 to 3.0 s after that**. The cause of the extra 2 to 3 s is not isolated (**hypothesis**: the ARC link and soundbar). The audio mute flags of avmutemanager never changed, so avmutemanager is not the cause; the volume service did not report a mute either.

**Conclusion (`verified-on-target` for the observation, `hypothesis` for the cause):** in `Active Standby` the DSP input gain is set to about zero by the TV's power-off sequence. An audio path that works inside `Active Standby` would need that control written (`amixer cset` on numid 76 or the owner service that sets it). **Not tried**: it is a hardware control write, the platform has a kernel-oops history on audio sysfs, and the brief limited variant C to read-only inspection. It is the next experiment if the owner authorizes it.

### Can the watcher tell the cause of a power-off?

| Source | `Request Power Off` reason | Seen |
| --- | --- | --- |
| API key `networkinput/controls/power` | `remoteKey` | 9 times |
| IR remote (D5, uncontrolled) | `remoteKey` (input node `LGE RCU`) | D5 |
| `tvpower/power/powerOff {"reason":"offTimer"}` (simulated, programmatic) | `offTimer`, carried through the whole sequence | 1 run, watcher `observe`: `accepted=no` |
| `powerOff {}` or `{"reason":"d6probe"}` | refused (`'reason' is needed` / `Invalid reason`) | |
| Auto-off timers, no-signal, HDMI-CEC, HomeKit, compensation, TV's own button | **not triggerable here.** The valid names are in `powerOffReasons` of `/etc/tvpowerd/tvpowerd.json` (read on c3-a): `autoOff`, `offTimer`, `sleepTimer`, `nosignal`, `hdmiCec`, `homekit`, `localKey`, `compensationStart`, `compensationEnd`, `pnwashStart`, `oslbSaving`, and about 70 others | not seen live |

So a user's power key and an HA or API turn-off are **both** `remoteKey`; timers and CEC and HomeKit have their own reasons. The watcher's reason filter (act only on `remoteKey`) is enough to keep it from fighting an auto-off, and the stub test covers the unaccepted-reason path; a live skip with the new watcher is recorded below if run. An unrelated finding from another agent: after a reboot `qbd` powered the TV into `Active Standby` with reason `homekit`; the filter ignores it, and it was **not seen again** in this run.

### Corrections to the plan

1. `tvpower/power/powerOn {}` is refused (`'reason' is needed`); it takes a reason from `powerOnReasons` (`remoteKey` works from `Active Standby`). A `powerOn` in the first 1.5 s after the settle is refused with `Keyblock1.5second`. The watcher now waits and retries; the first runs used the 4 s toggle-key fallback (+4.4 s latency).
2. `panelcontroller/getScreenMuteStatus` is a usable (indirect) panel indicator and its subscription does not push changes, so it is polled; `isOledScreenVisible` is not informative; `oledepl` has pixel-refresher getters (below). None of these were in the plan.

### OLED and panel care (measured read-outs)

`oledepl/getPixelRefresherInfoList` and `isPixelRefresherCondition` (read-only getters; the meaning of the numbers is **not documented**): start of the session `offrsInterval 4`, `offrsLastTime 3480`, `offrsCount 641`, `jbInterval 500`, `jbLastTime 3225`, `jbCount 7`, `isCondition true`; `getPixelRefresherMode` `auto`; `getPanelUsageTime` 20908. After the session (about two hours later, 20 minutes of it in `Screen Off`): **all six values and `isCondition` unchanged**. The names suggest an "off refresher" every 4 hours of use (`offrs`) and a longer cycle every 500 (`jb`), which fits LG's four-hour and long-cycle text in the research section above, but that mapping is a **hypothesis**. No refresher was seen to run during the session (the session lasted about two hours and the TV was mostly awake).

### Dwell (D)

After the A4 run (Always Ready **off**, watcher-darkened `Screen Off`, 2026-11-03, 20 min 5 s):

- The state stayed `Screen Off` for the whole time; no timer ended it (`automaticStandBy 4` and `autoOff2HourOnTimer on` act on hours, so **a stay longer than 20 min is not known**).
- The satellite: same pid, mic PCM `RUNNING` in 239 of 239 samples, HA connection counted in 239 of 239, 0 ping losses (2,613 one-second pings over the whole session), `suspend_stats/success` **0 since boot** (so the A4 run and the dwell never entered a kernel suspend), no stall or watchdog line.
- Playback after the dwell: 28 of 28 beeps audible (band peak -31.7 dBFS, as in `Active`). An `assist_satellite.announce` of one short Spanish sentence (sink gain 0.2, TV volume 9, the mic paused through HA's mute switch so `arecord` could listen) was audible at the TV microphone: speech at -36 to -55 dBFS for about 2.4 s over a -84 dBFS floor. Volume restored to 12.
- One full wake word chain ran in the dwell (13:26 UTC, `reply_audio` 14.5 s, played at gain 0, so silent): room speech or a false wake. The pipeline's conversation language `ru` cannot match a command; no HA action ran. Transcripts were not read, and the temporary pipeline was deleted.
- `MIC BUSY` was logged 8 times, **all caused by my own `arecord` overlapping the satellite's re-open at the end of a measurement window** (the daemon retried and recovered each time); LG hands-free was off for the run.

### Restore (verified at the end)

**Live skip test of the reason filter** (watcher `darken`, Always Ready on to avoid a Suspend): `powerOff {"reason":"offTimer"}` gave `POWEROFF ... accepted=no` then `SKIP reason offTimer not accepted`; the TV stayed in `Active Standby` until I woke it with `powerOn {"reason":"remoteKey"}`.

| Item | Start of the session | End | Verified |
| --- | --- | --- | --- |
| `general/lifeOnScreenMode` | `off` | `off` (set `alwaysReady` for the runs, `off` for A4, `alwaysReady` for the skip test, `off` at the end) | read back |
| `network/wolwowlOnOff` | `false` | `false` (set `true` for the way back) | read back |
| `sound/soundOutput`, volume, mute | `external_arc`, 12, not muted | `external_arc`, 12, not muted (volume set to 9 for each check, restored each time) | read back |
| `option/quickStartMode`, `turnOnByVoice`, `standByLight`, `screenOff`, `lifeOnScreenEnergySaving`, `picture/energySaving`, `time/*` | as recorded | identical | read back |
| LG hands-free ("Hi LG", `setting/setActivation`) | **on** (`settingStatus true`, `activationState true`; it held the mic PCM `RUNNING` at the start, so the satellite would have got `MIC BUSY`) | **on** again: I set `settingStatus false` for the run and `true` at the end; `activationState true` read back | read back |
| Power state, app | `Active`, `com.webos.app.home` | `Active`, `com.webos.app.home`, screen mute `false`, `aod` status `none` | read back |
| TV `/tmp` | none of ours | no `d6`, no satellite, no loops, no `pacat` or `arecord`, no luna subscription; mic PCM `closed`; kernel taint `4097` (unchanged), 0 oops lines in `dmesg`, no reboot | listed |
| HA | 43 entries, 2 pipelines (Kiwi preferred) | 43 entries, 2 pipelines, Kiwi preferred, 0 `c3_b` entities; the temporary entry and the `ru` pipeline deleted; my two access tokens revoked by their own ids (the API answers 401); no other token touched | listed |

The coordinator's message said "Hi LG" was left off by the packaging agent; at the start it **read as on**, which is what I restored. The packaging agent's other values (Always Ready off, WoL false, Quick Start+ on, ARC, volume 12, not muted) matched what I read.
