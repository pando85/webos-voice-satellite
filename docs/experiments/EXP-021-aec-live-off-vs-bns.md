# EXP-021: Live echo-cancellation A/B on c3-a, OFF against B + NS at TV volume 10 (counterbalanced, 40 items per arm)

**Status:** REJECTED for the pass criterion (B + NS did not halve the misses of OFF at volume 10: 13 of 33 against 14 of 34, paired p 1.0); TARGET_VERIFIED again for the delay lock, CPU, 0 reference underruns and 0 wake-word false wakes during content; STT comparison INCONCLUSIVE (Whisper was disturbed by a GPU benchmark during part of the run)
**Date:** 2026-11-05 (A/B 19:28 to 20:27 CET, mic + reference capture and echo-path sweep 20:27 to 20:33 CET; sound from 19:28 to 20:32)
**Device alias / model / firmware / webOS / architecture:** c3-a (entity `lg-c3-a`), LG C3 42 inch (OLED42C34LA), firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland, daemon `webos-satellite 0.4.2 (aarch64-unknown-linux-musl)` (the permanent ipk install), Home Assistant 2026.10.4, openWakeWord `alexa` (HA's Wyoming service, threshold 0.5, trigger level 1), faster-whisper `es`, conversation agent `conversation.fast` (the "Kiwi" pipeline)
**Power and playback state:** `Active`, screen on, HDMI 1 (the owner's PC display, owner present), TV speaker, Always Ready on, LG hands-free off. TV volume 10 during the A/B and the capture (the owner's own 89 before and after).
**Related research / requirements / issue:** [EXP-018](EXP-018-aec-live-c3-a.md) (the first live A/B), [AEC front-end design](../design/aec-frontend.md) (section 12), [P2 AEC tuning](P2-aec-tuning.md), [audio rig README](../../tools/audio-rig/README.md#live-aec-ab-on-a-tv-aec_live_abpy), [backlog item 1](../project/backlog.md#1-professional-audio-front-end)
**Risk:** sound in the room (TV content, a Bluetooth loudspeaker playing synthetic speech, the satellite's spoken replies); configuration changes on the TV (`[aec]`, log level; restored byte for byte); a mic + reference capture and an echo-path sweep with the satellite stopped (kept local, never committed)
**Approval needed / obtained:** owner, 2026-11-05: the run of plan `exp019-off-vs-bns.toml` approved for the evening (go-ahead about 18:45 CET, resumed at 19:23 CET after the loudspeaker was switched back on), no sound after 20:55 CET, the owner may type but not talk.

**Numbering.** The runner and the plan call this run "EXP-020" (the run id of the aborted attempts below). EXP-020 is already listed in the [index](README.md) for the moved Kivi conversation-agent evaluation, so this experiment is filed as **EXP-021**; the committed data uses run id `EXP-021`.

Labels follow [AGENTS.md](../../AGENTS.md). Everything measured here is **verified-on-target** for c3-a, firmware 03.30.74, `Active`, TV speaker, this room and this loudspeaker position only. The talker is a synthetic Piper voice through a Bluetooth loudspeaker, not a person.

## Question and falsifiable hypothesis

Does B + NS (the candidate default from EXP-018) beat no echo cancellation for the wake word over TV content at the owner's natural talker level, when the arms are counterbalanced and each arm gets 40 paired items? This is the follow-up EXP-018 asked for to decide between "on for everyone", "opt-in setting" and "keep parked".

- H1 (pass criterion, set by the owner): at TV volume 10 with content, B + NS has at most half the misses of OFF, no more false wakes, and speech-to-text is not worse.
- H2: the delay estimator locks within seconds of each restart and stays there; no reference underruns.

## Prerequisites and safe baseline

Read at 18:50 and again at 19:23 CET before any change: TV volume 89, not muted, TV speaker, foreground `com.webos.app.hdmi1`, LG hands-free off (the satellite's own sensor), satellite config md5 `cd688f93...` with no `[aec]` section, only `pcm0c` of the LGSIC card `RUNNING`. PC: the HDMI sink feeding the TV at 22 %, the JBL Go 3 (the D3 rig loudspeaker, where the owner placed it for EXP-018) at 82 %. A fresh short-lived HA token for this session only (mode 0600). No Home Assistant configuration was changed.

**No actuation.** Only harmless positives were played: "alexa" alone and "alexa" followed by a time, temperature or weather question. Unlike EXP-018, the pipeline's conversation agent now answered (the run summaries show `action_done` 36 times and `query_answer` 3 times, the generic response types of an LLM agent). HA's logbook for the run has no change of a light, switch, cover, climate, lock or media entity caused by the conversation agent (the only cover movement came from a scheduler, the switch and number changes from the satellite's own restarts and the energy automations). **verified-on-target** for this run only.

## Commands / steps

The committed runner [`aec_live_ab.py`](../../tools/audio-rig/aec_live_ab.py) (fixes of `a4c9550`) with the committed plan [`exp019-off-vs-bns.toml`](../../tools/audio-rig/aec_plans/exp019-off-vs-bns.toml), with two session-only changes kept in the scratchpad (not committed):

1. **Only the volume-10 condition.** In the first attempt (below) a detection took 45 to 95 s instead of the 13 s the plan assumed, because the agent now answers and keeps the conversation open. The full plan (volume 15 first, then 10) would not have reached volume 10 before the sound cut-off, so the plan was copied with `conditions = ["C10"]` (id `exp019-off-vs-bns-c10`). The schedule generator gives the same eight blocks as the plan's volume-10 half: OFF, B+NS, B+NS, OFF, B+NS, OFF, OFF, B+NS (ABBA per item set S1 to S4, item order shuffled from seed 19). 80 trials, 6 restarts.
2. **Only this satellite's pipeline runs.** The other TV (c3-b) uses the same HA pipeline and was in a continued-conversation loop the whole evening (a new speech-to-text run every 15 to 20 s; also earlier in the day, before this run). HA keeps only the last 10 debug runs per pipeline, so c3-a's armed wake-word run was pushed out and the runner's "armed" check (newest run of the pipeline) never passed. A wrapper filtered the debug list to runs whose `run-start` names c3-a's satellite entity, and treated an idle entity as armed when none of c3-a's runs was left in the list. c3-b was not touched.

Then, with the satellite stopped, `--stages dualcap,sweep` of the same plan: the 40 items over content at volume 10 with mic and reference recorded (`tv_dualcap.py`, `hw:LGSIC,0` and `hw:LGSIC,3`; section 5), and three 10 s sweeps through the TV (private, not analysed here). Arms: **OFF** (no `[aec]` section) and **B + NS** (`enabled = true`, `render_channels = 2`, `ns = true`, `[aec.aec3] "linear_mix" = 0.4`, `"ep_strength.default_len" = 0.0`), debug log level for the `aec:` lines.

Per-trial results (no transcripts, audio or identifiers): [`data/EXP-021-trials.csv`](data/EXP-021-trials.csv) (80 rows, schema in the [audio rig README](../../tools/audio-rig/README.md#live-aec-ab-on-a-tv-aec_live_abpy)); `tools/audio-rig/aec_live_report.py` gives the tables below from it and from `data/EXP-018-trials.csv`. Private (gitignored): `devices/private/c3-a/EXP-020/run4/` (the A/B), `run4-cap/` (capture and sweep), and the aborted attempts.

**Aborted attempts (not pooled, not committed).** 14:29 CET: stopped by the owner after 18 trials (`aborted-2026-11-05/`). 18:50 CET (`run2/`): stopped after 9 trials, 4 of them invalid, because of c3-b's runs (above); the JBL then switched itself off. 19:23 CET (`run3/`): the wrapper worked, but stopped after 9 trials to switch to the volume-10-only plan.

## Expected evidence and failure interpretation

- Pass (H1): B + NS misses at most half of OFF's at volume 10, wake-word false wakes not higher, STT not worse. With about 35 paired items per arm only a large difference reaches p < 0.05 (exact McNemar on the same items); smaller ones are reported as directional.
- A lock outside 10 to 19 ms or a delay that keeps moving, or reference underruns, would fail H2.

## Observations and redacted evidence links

### 1. Wake word through HA at volume 10 (H1)

80 trials, 13 invalid (6 OFF, 7 B + NS): the satellite was still in a continued conversation after the previous detection 120 s later, so the item was not played. The report skips them; pairs are items valid in both arms. 95 % Wilson intervals; "paired" is B + NS-only / OFF-only detections on the same item, exact two-sided McNemar.

| Arm | Wake k/n (rate, 95 % CI) | Paired vs OFF | "alexa" alone | "alexa" + question | Median wake latency |
| --- | --- | --- | --- | --- | --- |
| OFF | **20/34 = 59 %** [42, 74] | | 10/18 | 10/16 | 1330 ms |
| B + NS | **20/33 = 61 %** [44, 75] | 5/4, **p 1.0** (27 pairs) | 11/17 | 9/16 | 1370 ms |

- **Pass criterion not met:** OFF missed 14 of 34 (41 %), B + NS 13 of 33 (39 %). Halving needed at most about 7. **verified-on-target**
- **Pooled with EXP-018** (same cell: content at volume 10, owner-natural talker; `aec_live_report.py docs/experiments/data/EXP-018-trials.csv docs/experiments/data/EXP-021-trials.csv`): OFF 28/54 = 52 % [39, 65], B + NS 35/53 = 66 % [53, 77], paired 15/7, p 0.13 (47 pairs). The pooled gain comes from EXP-018's B + NS block (15/20, run last and not counterbalanced); this counterbalanced run shows none. Neither run alone nor the pool reaches p < 0.05, and the pooled B + NS misses (34 %) are not half of OFF's (48 %).
- OFF detected more here than in EXP-018 (59 % against 40 % in the same cell). Possible reasons (**hypothesis**, not separated): block-to-block noise (EXP-018 saw up to 25 points within one condition), the different TV content position, the owner typing, and the lower delay lock (below).

### 2. False wakes during content

The runner counts every `idle` to `listening` change outside the item windows: 49 (OFF) and 76 (B + NS). All but one were the satellite re-listening after its own spoken reply (HA's continued conversation: `responding`, `idle`, then `listening` within 0.4 to 0.5 s, no wake word), recounted from HA's recorded state history. The remaining one (B + NS, block 8) is the same pattern with a 1.9 s gap, right after a detected item's reply. So **wake-word false wakes: 0 in 26.0 minutes of content (OFF) and 0 in 31.3 minutes (B + NS)**. That bounds the rate only (at most about 7 and 6 per hour, rule of three, 95 %); it cannot show a difference. **verified-on-target**

### 3. Speech-to-text

A Whisper GPU benchmark ran on the same GPU in the cluster during this run (reported by the other session): 4 live STT requests failed (CUDA out of memory at 18:45:34 and 18:53:51 UTC), and 67 of about 190 requests took over 5 s (up to 47 s; normally about 0.9 s). Wake detection is not affected. In the CSV, every trial from 18:40 to 19:00 UTC (19:40 to 20:00 CET, 37 trials), every failed STT (1) and every detected trial without a transcript is marked `stt_contaminated=...` in `notes` (rows are kept). The state-history check (HA `processing` over 5 s after a detection) flagged no trial outside that window.

| Arm | Command items woken with a transcript (all) | Exact match, mean WER (all) | Uncontaminated only: exact match, mean WER |
| --- | --- | --- | --- |
| OFF | 9 | 6/9, 1.08 (one transcript of TV content, WER 8.5) | **4/6, 0.21** |
| B + NS | 9 | 6/9, 0.24 | **2/3, 0.22** |

STT is **inconclusive**: 6 and 3 clean transcripts cannot show "not worse". As in EXP-018, the errors are the TV's speech getting into the transcript after the question, in both arms.

### 4. Delay lock, ERLE, CPU (H2)

| Event | Lock | Notes |
| --- | --- | --- |
| B + NS restart, block 2 | 10.6 ms after 2.3 s (confidence 23) | jumped to 1.5 ms (confidence 11) for 35 s, then back to 10.6 ms |
| B + NS restart, block 5 | 12.7 ms after 1.3 s (confidence 23) | 1.5 ms for 31 s, then back to 12.7 ms |
| B + NS restart, block 8 | 10.0 ms (time to lock not logged, see below) | two 1.5 ms excursions of 31 to 35 s, each back to 10.0 ms |

- **H2 partly supported:** the two restarts with a logged lock locked within 1.3 and 2.3 s, all three at 10.0 to 12.7 ms, below EXP-018's 15 to 19 ms (where one restart gave 9.8 ms). Four times the estimator jumped to 1.5 ms for about half a minute at low confidence (10 to 13) and came back; 1 of the 33 valid B + NS trials fell in such a window. Whether AEC3 lost echo removal in those windows is not known; their ERLE (10.1 to 22.0 dB) is not lower than elsewhere. **verified-on-target**; cause **hypothesis** (a content passage with a weak or ambiguous correlation).
- **ERLE** (the daemon's own figure, 60 s windows with an active reference, 27 windows): 4.4 to 22.0 dB, median 10.8 dB, as in EXP-018 (B: median about 10 dB). The windows include the talker trials and the satellite's spoken replies. Reference underruns 0 in every window; capture xruns 0.
- **CPU** (`/proc/<pid>/stat` over each block, share of one core): OFF 1.2 to 1.3 %, B + NS 6.3 to 6.7 %; sonora 411 to 517 µs per 10 ms frame (mean), single frames up to 22.7 ms without an underrun. Same as EXP-018. **verified-on-target**
- **Log rotation:** the TV log lines from 19:11:56 to 19:15:45 UTC (block 8's restart and its first lock) were lost before the runner collected them (the debug log rotated more than once during the long block 7; the runner reads only the current and the previous file). With the last window of block 5, 3 valid B + NS trials have no ERLE or delay.

### 5. Offline re-scoring of the mic + reference capture

The same 40 items were played over the content at volume 10 with the satellite stopped and the mic (`hw:LGSIC,0`) and reference (`hw:LGSIC,3`) recorded together (290 s, 0 xruns on both PCMs). On the PC (**verified-on-host, device recordings**): `aec_eval/chain_eval.py` ran the daemon's exact front-end (`aec_offline`, rebuilt from the current tree) on the pair, and a replica of HA's Wyoming openWakeWord 2.1.0 (`aec_eval/oww_wyoming.py`, threshold 0.5, trigger level 1) scored each item as its own stream with the P2 window. 32 of the 40 items were located in the recording by template match (median offset 2.52 s, 10 to 90 % spread 138 ms); the rest use the median offset.

| Variant | Wake (offline replica) | Paired vs no AEC | In-band ERLE over the whole session | Delay lock |
| --- | --- | --- | --- | --- |
| No AEC | 16/40 = 40 % [26, 55] | | | |
| B + NS | 19/40 = 48 % [33, 63] | 3 more detected than lost (5/2), p 0.45 | 2.3 dB (10.2 dB in the first second) | 11.8 ms after 3.4 s (confidence 22) |
| B | not scored (time) | | 1.8 dB | 11.8 ms |
| AEC3 default | not scored (time) | | 2.0 dB | 11.8 ms |

- Offline on the same audio, B + NS is 3 detections ahead (not significant), the same direction and size as live. The whole-session ERLE is low because the talker items fill about half of the session's mic power; the daemon's 60 s figures above (median 10.8 dB) come from windows with more content than talker.
- The offline rates (40 to 48 %) are lower than live (59 to 61 %); the replica scores each item as a fresh stream with 8 s of silent warm-up, HA scores a continuous stream (**hypothesis** for the gap). The host was heavily loaded (load average above 30), so the host CPU figures of `chain_eval` are not meaningful.

## Conclusion and confidence

- **B + NS does not meet the pass criterion** at TV volume 10 with the owner-natural talker: 61 % against 59 % for OFF on 33 and 34 valid trials, paired 5/4. Together with EXP-018 the evidence is a gain of 0 to 14 points that is not significant (pooled p 0.13) and comes mostly from EXP-018's last, non-counterbalanced block. Confidence that B + NS does **not** halve the misses in this room: moderate. Confidence about a smaller gain either way: low.
- **Offline on the same 40 items** (mic + reference capture, the daemon's front-end, HA's detector replica): B + NS 48 % against 40 %, paired 5/2, p 0.45. Same direction and size as live, also not significant.
- **No harm seen:** 0 wake-word false wakes in both arms (a weak bound), wake latency unchanged, about 5 points of one core, 0 underruns. STT could not be compared (GPU contention).
- **Decision recommendation: opt-in setting, not on for everyone.** Keep `[aec] enabled = false` as the shipped default and offer B + NS as an experimental opt-in (for example for users with loud TV audio), since it costs little and did not hurt. Not "on for everyone": two live runs did not show the required gain. Not "keep parked" either: the feature works as designed on the TV and an opt-in lets real use and a false-wake soak collect data. Confidence: moderate.

## Rollback and cleanup

- 20:32:40 CET: the runner restored PC sinks first, then the TV volume, and the config, then `ctl.sh restart`. Checked at 20:32:55: config md5 `cd688f938f449c860a8a0dc25451d35f` (no `[aec]` section), only `pcm0c` `RUNNING` (`pcm1c` to `pcm4c` closed), satellite reconnected to HA and streaming the wake-word stage; TV volume 89, not muted, TV speaker, HDMI 1; PC HDMI sink 22 %, JBL sink 82 % (as found); no `pw-play`, content player or runner left.
- HA: no configuration changed; the session's own refresh token revoked (`/auth/revoke`, then 401 on the API) and its file deleted. No other token or session was touched. The other TV (c3-b) was not touched.
- Private data kept locally (gitignored, never committed): `devices/private/c3-a/EXP-020/` (per-trial JSON with transcripts, block data, AEC log lines, config backups, the capture and sweep).

## Follow-ups / unresolved questions

- **Clean STT data:** the STT comparison needs a re-run (or the mic + reference capture re-scored through Whisper offline) when the GPU is not shared.
- **Runner:** (a) filter HA's pipeline debug runs by the satellite (another satellite on the same pipeline breaks the arming check); (b) a detection now costs 45 to 95 s because the agent answers and keeps listening; the plan's 13 s per trial no longer holds, and continued-conversation re-listens must not be counted as false wakes (count only `idle` to `listening` after more than about 2 s of idle, or read the wake-word stage from the satellite log); (c) collect the TV log more often than once per block (log rotation); (d) after a restart, `wait_streaming` accepts a streaming line up to 1 s before the restart, which can be the old process's line.
- Why the delay locked at 10 to 13 ms in this session and why it flips to 1.5 ms for half a minute.
- Multi-hour false-wake soak with B + NS over real TV use, and a short check with a person talking.
