# EXP-018: Live echo-cancellation A/B on c3-a (JBL talker, TV content, Home Assistant's real pipeline)

**Status:** INCONCLUSIVE for the pass criterion (directional gains over content, none significant at 20 trials per cell); TARGET_VERIFIED for the delay lock, ERLE with content, CPU and memory, and for the music reaching the reference
**Date:** 2026-11-05 (12:33 to 13:50 CET; sound from 12:38 to 13:43, about 64 minutes)
**Device alias / model / firmware / webOS / architecture:** c3-a (entity `lg-c3-a`), LG C3 42 inch (OLED42C34LA), firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland, daemon `webos-satellite 0.4.2 (aarch64-unknown-linux-musl)` (the permanent ipk install), Home Assistant 2026.10.4, openWakeWord `alexa` (HA's Wyoming service, threshold 0.5, trigger level 1), faster-whisper `es`
**Power and playback state:** `Active`, screen on, HDMI 1 (the owner's PC display, owner present), TV speaker, Always Ready on, LG hands-free off (`settingStatus` false). TV volume 10 or 15 during the runs (the owner's own 89 before and after).
**Related research / requirements / issue:** [AEC front-end design](../design/aec-frontend.md) (section 9 steps 2 and 3, section 12), [P2 offline A/B](P2-aec-offline-ab.md), [P2 AEC tuning](P2-aec-tuning.md), [P2 corpus and metrics](P2-corpus.md), [P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md), [backlog item 1](../project/backlog.md#1-professional-audio-front-end)
**Risk:** sound in the room (TV content, a Bluetooth loudspeaker playing synthetic speech, the satellite's own music); configuration changes on the TV (`[aec]`, `[music]`, log level; restored byte for byte); a short mic + reference capture with the satellite stopped (kept local, never committed)
**Approval needed / obtained:** owner, 2026-11-05 about 12:30 CET: AEC un-parked, live tests on c3-a with TV content and the JBL as a simulated talker, daytime, TV volume at most 20, config A/B with restarts, restore AEC off and music off at the end. The owner was at the TV during the session and judged the JBL level by ear (below).

Labels follow [AGENTS.md](../../AGENTS.md). Everything measured here is **verified-on-target** for c3-a, firmware 03.30.74, `Active`, TV speaker, this room and this loudspeaker position only. The talker is a synthetic Piper voice through a Bluetooth loudspeaker, not a person.

## Question and falsifiable hypothesis

Does the canceller help a talker in the room while the TV plays, on the real TV, through Home Assistant's real pipeline, and what does it cost? This closes the open parts of section 9 step 2 of the [design](../design/aec-frontend.md#9-on-tv-verification-plan-pending) (delay lock and ERLE with a stimulus) and runs a simplified step 3 (live A/B).

- H1 (proposed pass criterion, for the owner to confirm): with content at TV volume 10, configuration "B" at least halves the misses of no AEC (or gives a clear wake-rate gain), false wakes are not higher, the quiet-room wake rate is no worse, STT is not worse, and the daemon's CPU stays acceptable.
- H2: the delay estimator locks between 11 and 19 ms on content and stays there across content changes and restarts.
- H3: the satellite's own music (`[music]`) reaches the `SoundEngineCapture` reference, so the canceller sees it too.

## Prerequisites and safe baseline

Read at 12:34 CET, before any change: TV `Active`, master volume 89, not muted, `tv_speaker`, foreground `com.webos.app.hdmi1`, `lifeOnScreenMode` `alwaysReady`, LG hands-free `settingStatus` false; daemon 0.4.2 aarch64 running from the ipk under its supervisor, HA connected, mic listening, reply volume 40, remote power-off `normal`, `[music]` off, `[aec]` absent (off); only `pcm0c` of the LGSIC card `RUNNING`. PC: the HDMI sink that feeds the TV at 22 %, the JBL (a JBL Go 3 on Bluetooth, the D3 rig loudspeaker) at 82 %. HA: the satellite's pipeline select is `preferred` (Kiwi: openWakeWord `alexa`, faster-whisper `es`, conversation `conversation.qwen2_5_3b` through Ollama, which was not running, so every run ended in `intent-failed` and nothing could act on a transcript). The config was copied to the PC (`devices/private/`, gitignored) and checked by md5.

**No actuation.** Only harmless positives were played: "alexa" alone and "alexa" followed by a time, temperature or weather question (no light, climate or timer command). The Piper MLS voices were left out (0 of 13 each through HA in [session 2](../discovery/c3-a/P2-session2-wake-accuracy.md)). No HA configuration was changed (no pipeline, agent or entity change).

## Commands / steps

All scripts are the committed [audio rig](../../tools/audio-rig/README.md) tools plus small wrappers in the session scratchpad. Raw captures, logs and the per-trial JSON stay in `devices/private/c3-a/EXP-018/` (gitignored).

1. **Talker calibration** (satellite stopped with `ctl.sh stop`, TV silent): `tv_dualcap.py --ref hw:LGSIC,3 --ref-channels 2 --ref-format S32_LE` captured mic and reference for 115 s while the PC played four corpus positives (two "alexa" alone, two with a question) on the JBL at playback gains +24, +6, +12, +18 and +24 dB (the `play_record.py` convention: +24 dB is the item at its corpus level, -6 dBFS peak) with the JBL sink at 82 %, then +24 and +20 dB with the sink at 100 %. The talker level at the mic is the item window's power minus the preceding noise power, against the played item's power (the "digital-to-mic gain" of D3), converted to the corpus' active-speech RMS, as P2 defined its reference level (about -61 dBFS).
2. **Delay lock, ERLE, CPU** (arm B below, `[log] level = "debug"`): the 10-minute content sampler from the PC over HDMI (PC sink 100 %, as in session 2), TV volume 10 for 2 minutes, 15 for 2 minutes, one `ctl.sh restart` with content playing, then content off and the satellite's own music: `[music] enabled = true`, `media_player.play_media` of a Radio Browser station through HA's proxy for 100 s at the stored satellite volume 40 %, TV volume 15, then `media_stop`. The daemon's own `aec:` and `capture:` lines (every 60 s) and `/proc/<pid>/stat` CPU time over 20 to 75 s windows.
3. **Live A/B.** Arms (one daemon restart each, same config otherwise, debug log): **OFF** (`[aec]` absent), **DEFAULT** (`[aec] enabled = true`), **B** (`enabled = true`, `render_channels = 2`, `[aec.aec3] "linear_mix" = 0.4`, `"ep_strength.default_len" = 0.0`). Conditions: **Q** quiet room (TV volume 10, nothing playing), talker at the owner-natural level; **C10** content at TV volume 10, talker at the owner-natural level; **C15** content at TV volume 15, talker at the loud level (below). 20 positives per cell (10 "alexa" alone, 10 with a question; sharvard M and F, davefx, ald, claude, lessac, ryan), split into two sets of 10 played in two passes; arm order DEFAULT, B, OFF in pass 1 and OFF, B, DEFAULT in pass 2, condition order drawn at random per arm (seed 18) and reversed in pass 2. `p2_runner.py` (a scratch copy that also accepts HA's `error` event as the end of a run) played one item at a time on the JBL after the satellite was idle and its next wake-word run armed, and recorded the satellite entity's `idle` to `listening` changes; a detection counts from 0.2 s before the item's start to 1.5 s after its end (Bluetooth latency assumed 0.2 s; the P2 window, widened to the whole item because the item's word timestamps were not used). `ha_assist_watch.py` logged every state change and pipeline run (wake word, STT text) for the whole session. Content started 15 s before the first item of a content cell and played the sampler continuously.
4. **False wakes:** every `idle` to `listening` change while content played that was not inside an item window (from the play start to 3 s after the item's end).
5. **B + NS** (`ns = true` added to B) was run last, at C10 only (both item sets), as time allowed. It is therefore not counterbalanced against the other arms.

**Room conditions.** One room, owner present at the desk the whole time, typing on the keyboard (not talking); the arm order was randomised and reversed so that this affects all arms alike. The JBL stood where the owner placed it, roughly where a seated person's head would be; its distance to the TV was **not measured** (assumed about 1 to 2 m, a flag for any comparison with a person). Home Assistant's pod restarted at about 13:19 CET (not caused by this experiment); see the observations for what that cost.

## Expected evidence and failure interpretation

- Pass (H1): at C10, B's misses at most half of OFF's (or a clear gain), no more false wakes, Q no worse, STT not worse, CPU acceptable. With 20 paired trials per cell, only a large difference can be called (exact McNemar test on the same items); anything smaller is reported as directional.
- H2 fails if a lock falls outside 11 to 19 ms or the delay keeps moving; H3 fails if no `aec: echo delay` line appears while only the satellite's music plays.
- A quiet-room difference between arms is measurement noise: with a silent reference the canceller is transparent (offline, [P2 A/B](P2-aec-offline-ab.md)). The Q cells therefore show how much the same condition varies from block to block.

## Observations and redacted evidence links

### 1. Talker level (calibration)

Talker level at the TV mic (channel 0 after a 120 Hz high-pass, active-speech RMS in dBFS of the mic's own full scale; median of four items; the room floor between items was -60 to -67 dBFS because of keyboard noise, so the low steps are unreliable):

| JBL sink | Playback gain | Talker at the mic | Label |
| --- | --- | --- | --- |
| 82 % | +24 dB (the item at corpus level) | **-48.7 dBFS** (items -47.6 to -50.7) | **owner-natural** (owner-confirmed by ear: "the initial sound level at JBL tests was the correct") |
| 82 % | +18 dB | -52.2 dBFS | quieter than the owner's voice (owner) |
| 82 % | +12 dB, +6 dB | not measurable over the typing noise (the +6 dB step gave about -66 dBFS) | quieter than the owner's voice |
| 100 % | +20 dB | -44.5 dBFS | louder (owner: "good to test") |
| 100 % | +24 dB | **-40.5 dBFS** | **loud** (the JBL's maximum with the corpus files, +8.2 dB over owner-natural) |

- The owner-natural level is about **12 dB above** P2's "reference level" (-61 dBFS, [P2 session 2](../discovery/c3-a/P2-session2-wake-accuracy.md)). So the offline "+10 dB talker" results of the [A/B](P2-aec-offline-ab.md) and the [tuning](P2-aec-tuning.md) are the ones that correspond to this owner's voice, not the reference-level ones.
- Digital-to-mic gain at 82 %: -26.7 dB (D3 measured -27.0 dB with the same loudspeaker). Going from 82 % to 100 % on the sink added 8.2 dB at the mic, not the 5.2 dB of the sink volume (the Bluetooth volume is not linear).
- The JBL cannot go higher with these files: +24 dB already puts the loudest item at -6 dBFS peak. The owner judged the loud level by ear as acceptable; distortion was not measured.
- Talker-to-content ratio at the mic (content level from the OFF arm's 60 s `capture:` lines, which include the talker trials): about **+5 to +6 dB** for owner-natural at volume 10 (content -54 to -55 dBFS), about **+4 to +5 dB** for the loud level at volume 15 (content -45 dBFS). P2's semi-synthetic talker was at -8 and -16 dB.

### 2. Delay lock, ERLE, CPU and memory (section 9 step 2)

| Event | Delay lock | Notes |
| --- | --- | --- |
| B start, content volume 10 | 16.5 ms after 9 s (confidence 11) | then changed to 15.2 and 16.8 ms within the first two minutes (content changes) and stayed at 16.5 to 16.8 ms |
| B restart with content playing, volume 15 | **9.8 ms** after 1.3 s (confidence 11, then 22) | outside the 11 to 19 ms of P2 session 1; ERLE that minute 11.5 dB, so AEC3 followed it |
| B + music, satellite music only | 18.8 ms after 5 s of music | see section 4 |
| DEFAULT (A/B pass 1) | 18.0 ms | first lock when content started; no change over 5 min |
| B (pass 1) | 15.1 ms | no change over 8 min |
| B (pass 2) | 17.2 ms | no change over 8 min |
| DEFAULT (pass 2) | 17.4 ms | no change over 7 min |
| B + NS | 16.6, then 17.9 ms after 32 s | no further change |

- **H2 partly supported:** 7 of 8 locks between 15.1 and 18.8 ms, one restart at 9.8 ms. The delay moves by up to 1.6 ms within the first minute and is then stable for the rest of a run (5 to 8 minutes); it differs per open, as P2 session 1 found. **verified-on-target**
- **ERLE** (the daemon's own figure, 60 s windows with an active reference): B 4.8 to 14.8 dB (median about 10 dB; two windows with under 700 active frames left out), DEFAULT 5.3 to 18.4 dB (median about 8 dB), B + NS 4.5 to 11.6 dB. These windows include the JBL talker trials, which the canceller must not remove, so they understate echo removal on content alone; offline, the same content gave about 10 dB at volume 10 ([P2 A/B](P2-aec-offline-ab.md)). Reference underruns: 0 in every window (21 windows with the canceller, 0 xruns).
- **Clipping of the +24 dB stream:** OFF during its content blocks: 8 to 109 clipped samples per minute; with the canceller 0 to 35, mostly 0 (the canceller removes echo before the gain, as designed).
- **CPU and memory** (aarch64 build, `/proc/<pid>/stat` over 20 to 75 s, share of one core):

  | Arm | Daemon CPU | RSS | sonora per 10 ms frame (mean / max) |
  | --- | --- | --- | --- |
  | OFF | 2.2 % | 1.9 MB | - |
  | DEFAULT | 5.3 % | 2.8 MB | 0.30 to 0.36 ms / up to 31 ms |
  | B | 5.8 to 6.9 % | 2.9 to 3.1 MB | 0.35 to 0.50 ms / up to 30 ms |
  | B + NS | 6.6 % | 3.3 MB | 0.41 ms / up to 52 ms |
  | B + music playing | 8.6 to 8.7 % | 4.6 MB | 0.34 to 0.41 ms / up to 41 ms |

  The mean matches the offline estimate (section 12 of the design: about 0.25 ms per frame for B on aarch64; the TV measured 0.35 to 0.50 ms). Single frames up to 30 to 52 ms happened a few times per minute without an xrun or a reference underrun (the 8192-frame buffers absorb them); the cause (scheduling on a loaded TV, load average about 12) is a **hypothesis**.

### 3. Live A/B through Home Assistant

Wake detections of HA's openWakeWord through the real ESPHome integration, 20 positives per cell, the same items in every arm (10 per pass). 95 % Wilson intervals. "Paired" is the exact McNemar test against OFF on the same items (arm-only detections / OFF-only detections, two-sided p).

| Arm | Q: quiet, owner-natural | C10: content vol. 10, owner-natural | C15: content vol. 15, loud talker | False wakes during content (minutes of content) |
| --- | --- | --- | --- | --- |
| OFF | 15/20 = 75 % [53, 89] | 8/20 = 40 % [22, 61] | 6/20 = 30 % [15, 52] | 0 (8.7 min) |
| DEFAULT | 11/20 = 55 % [34, 74]; paired 3/7, p 0.34 | 8/20 = 40 % [22, 61]; paired 4/4, p 1.0 | 12/20 = 60 % [39, 78]; paired 8/2, p 0.11 | 0 (9.7 min) |
| B | 14/20 = 70 % [48, 85]; paired 2/3, p 1.0 | 11/20 = 55 % [34, 74]; paired 5/2, p 0.45 | **13/20 = 65 % [43, 82]**; paired 11/4, p 0.12 | 0 (15.9 min, includes step 2) |
| B + NS (run last) | not run | **15/20 = 75 % [53, 89]**; paired 10/3, p 0.09; against B 7/3, p 0.34 | not run | 0 (5.7 min) |

- **Block-to-block noise is large.** The same quiet condition (where the canceller is transparent) gave 9/10 and 5/10 for B, 7/10 and 4/10 for DEFAULT, 8/10 and 7/10 for OFF in the two passes. So a cell of 20 can move by about 20 points without any real effect. The quiet-room "DEFAULT 55 % against OFF 75 %" is this noise, not the canceller. These blocks were not re-run (the sound budget was used); the owner's typing is the likely cause of part of the variance (**hypothesis**).
- **Direction:** over content every canceller arm except DEFAULT at volume 10 detected more than OFF, and the gains are largest where the echo is loudest (C15: +7 for B, +6 for DEFAULT of 20). None reaches p < 0.05 at this sample size.
- **Pass criterion (H1) at C10:** OFF missed 12 of 20; halving needs at most 6 misses. B missed 9: **not met**. B + NS missed 5: met numerically, but it ran last, in one block order, and its gain over B (p 0.34) is within the block noise above.
- **By item type:** "alexa" alone was detected 13/30 (OFF), 17/30 (DEFAULT), 24/30 (B) over all conditions; "alexa" + question 16/30, 14/30, 14/30. The canceller's gain is in the short items.
- **False wakes:** 0 in every arm during 40 minutes of content in total (`idle` to `listening` changes outside item windows, from HA's state history). This bounds the rate only: at most about 21 per hour for OFF and 11 per hour for B (rule of three, 95 %). Nothing here can show a difference; the owner's live use and a multi-hour soak settle it.
- **STT** (faster-whisper, detected question items with a transcript; transcripts after 13:18 were lost when HA restarted, see below): almost all word-perfect in every arm ("¿Qué hora es?", "¿Qué temperatura hay en el salón?", "¿Qué tiempo hace hoy?"; OFF had "¡Qué gola es!" once at C10 and "¡Gracias!" for a question at C15). With the talker at the owner-natural level and content at volume 10, STT is not the bottleneck; the wake word is. After "alexa" alone Whisper wrote "¡Gracias!" (a hallucination on silence) or transcribed the TV content (a sentence of the Spanish audiobook in OFF at C10); that is expected for a run without a command.
- **Home Assistant restart (about 13:19 CET):** HA's pod was recreated during the OFF C10 pass 2 cell (not by this experiment). The runner crashed on the closed WebSocket after its 9th item; the 10th item had already played and was detected at 13:18:30 (watcher log, STT "¿Qué hora es?"), so it is counted as detected, with an estimated play time (a reconstructed row in the private data). The run debug data of the old HA instance and the watcher were lost, so transcripts of the cells from 13:20 to 13:37 are missing; wake detections and false wakes come from the runner and from HA's recorded state history, which survived. The satellite reconnected by itself.

### 4. The satellite's own music in the reference (H3)

With content off and only the satellite's music playing (Radio Browser station through HA's proxy, `track 1 playing flac 48000 Hz 2 ch`, satellite volume 40 %, TV volume 15), the estimator locked at **18.8 ms (confidence 18) 5 s after the music started**. **H3 supported, verified-on-target:** the music played through the satellite's own `pacat` stream reaches `SoundEngineCapture`, so the canceller sees it like TV content. At that volume the music was quiet at the mic (mic -61 to -65 dBFS against a -67 dBFS floor; the reference was above the -70 dBFS activity threshold in only 11 to 26 % of the frames), so the 3.3 to 4.4 dB ERLE says nothing about how well loud music is cancelled. The music path raised the daemon to 8.7 % of one core and 4.6 MB RSS. Music was stopped (`media_stop`) and `[music]` turned off again.

## Conclusion and confidence

- **The canceller works on the TV as designed** (**verified-on-target**): the satellite opens and links `SoundEngineCapture`, locks the delay within seconds (7 of 8 opens at 15 to 19 ms, one at 9.8 ms), runs with 0 underruns and 0 xruns, removes echo before the gain (no clipping where OFF clipped), and costs about 3 to 5 points of one core on the aarch64 build (daemon 5 to 7 % against 2 %) and about 1 MB.
- **Benefit for wake word: positive in direction, not proven.** Over content B detected 55 % (volume 10) and 65 % (volume 15, louder talker) against 40 % and 30 % without; B + NS 75 % at volume 10. No paired difference is significant with 20 trials per cell, and the same quiet condition varied by up to 25 points between blocks. Confidence: low to moderate (one room, one loudspeaker position, synthetic voices, the owner typing, one session).
- **No harm seen:** quiet-room differences are within the block noise (the canceller is transparent there), 0 false wakes in every arm (a weak bound), STT unchanged.
- **Compared with offline:** the live results agree with the offline "+10 dB talker" figures (B 42/45/25 % quiet/vol 10/vol 15 offline at a talker 10 dB above the reference; the owner-natural level is 12 dB above it). The quiet-room rate is higher live (70 to 75 %) than offline because these seven voices exclude the MLS voices.

## Rollback and cleanup

- 13:43 CET: PC HDMI sink back to 22 % **before** the TV volume went back to 89; JBL sink back to 82 %; no `pw-play`, `content_play` or runner left on the PC.
- `config.toml` restored from the copy (md5 `cd688f93...`, equal to the original), the four `exp018-arm-*.toml` files and `/tmp/tv_dualcap.py` deleted, daemon restarted with `ctl.sh restart`: 0.4.2 aarch64, HA connected at 13:43:22, mic listening, `[music]` off, reply volume 40, remote power-off `normal`; `pcm0c` `RUNNING`, `pcm3c` and every other LGSIC capture PCM `closed`.
- TV: `Active`, volume 89, not muted, HDMI 1, Always Ready on, LG hands-free off (as before). HA entities: satellite media player `idle` at volume 0.4 with `supported_features` 5124 (music off), pipeline select `preferred`, mute switch off.
- HA: no configuration changed; the session's own refresh token was revoked by id (`/auth/revoke`, then 401 on the API) and its file deleted. No other token or session was touched.
- Private data kept locally (gitignored, never committed): `devices/private/c3-a/EXP-018/` (calibration capture, per-trial JSON, watcher log, HA state history, AEC log lines, arm configs, the original config).

## Follow-ups / unresolved questions

- **Decision input for the owner:** ship AEC as an **opt-in setting** with B (B + NS as the candidate default for that setting), not on by default yet. See the summary in the [design](../design/aec-frontend.md#12-tuning-results-and-parked-state).
- **Next experiment (to decide on the default):** OFF against B + NS only, at volume 10 and 15 with the owner-natural talker, 40 items per cell, alternating arms every 10 items (ABBA), the owner not typing; plus the mic + reference recorded with `tv_dualcap.py` in a block with the satellite stopped, so that more variants can be scored offline on the same JBL audio. Then a short live-person check and a multi-hour false-wake soak with B + NS over real TV use.
- Why the delay came out at 9.8 ms after one restart (a different open, or a different path) and whether AEC3 always follows such a jump without a loss.
- Loud music through the satellite: ERLE and false wakes with the music at a normal listening level (needs its own sound slot).
- The single-frame spikes of 30 to 52 ms: harmless here (no underruns), but worth a check under heavier load.
