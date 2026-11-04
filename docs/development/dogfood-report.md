# Dogfood report tool

`tools/dogfood_report.py` turns a satellite's own logs (and, optionally, Home Assistant's Assist debug runs) into numbers for real-use decisions: echo-cancellation work (misses while the TV or PC plays sound), microphone front-end work (misses in quiet) and false-wake tuning. Python 3 standard library only; the websocket client is a small stdlib implementation, so no `aiohttp` or `websockets` is needed.

Status: written on 2026-11-04 and run against c3-a (firmware validated for the satellite, daemon 0.1.0). Tests use synthetic log lines only.

## Usage

```sh
# Daemon health and pipeline outcomes of the last 3 local days (read-only ssh)
DOGFOOD_TV_HOST=<tv host> tools/dogfood_report.py --days 3 --no-ha

# With HA's debug runs, accumulating across invocations in a gitignored directory
tools/dogfood_report.py --host <tv host> --days 7 \
    --ha-url https://<ha host> --ha-token-file "$TOKEN_FILE" --ha-pipeline <name> \
    --archive-dir devices/private/dogfood

tools/dogfood_report.py --host <tv host> --json > report.json     # machine-readable
```

| Option | Meaning |
| --- | --- |
| `--host`, `--user` | TV address and ssh user (default `root`). No default host: flag, then env `DOGFOOD_TV_HOST` / `DOGFOOD_TV_USER`, then the gitignored `devices/private/dogfood/config.json` (`host`, `user`, `ha_url`, `ha_token_file`, `ha_pipeline`). |
| `--days N`, `--since`, `--until` | Window. `--days N` is the last N local days including today (default 7); `--since` takes `YYYY-MM-DD` (local midnight) or an ISO time. `--utc` makes days and hours UTC; otherwise they use the PC's timezone. |
| `--no-ha` | Skip the Home Assistant part. |
| `--ha-url`, `--ha-token-file`, `--ha-pipeline` | HA base URL, a file holding an access token (bare or the JSON `/auth/token` returns), and the pipeline name or id (default: HA's preferred one). Env `HA_URL`, `HA_TOKEN_FILE`, `DOGFOOD_HA_PIPELINE`. TLS certificates are always verified (`--ha-cafile` for a private CA). |
| `--archive-dir DIR` | Append the new sanitized log lines and HA run summaries to files here, so a window longer than the TV's log (below) can be reported. Refused unless the path is gitignored or outside the repository. |
| `--bundle`, `--save-bundle` | Analyse a saved raw TV bundle offline, or save one (gitignored path only; it holds addresses). |
| `--json` | JSON instead of markdown. |

The ssh call runs one `sh -s` script that only does `cat`, `date`, `getconf`, `ls`-style tests and `ctl.sh status`; nothing is written on the TV, nothing is restarted, and no sysfs or debugfs node is read.

## What it reports

Per local day and for the whole window:

- **Daemon health.** Running state, pid, version, daemon and TV uptime and RSS (live, from `ctl.sh status` and `/proc`); daemon sessions and uptime percentage; starts with their cause (boot through autostart, manual or `ctl`, crash restart with the exit status, mapped for the watchdog codes 70 to 72); supervisor restarts and crash-loop give-ups; watchdog trips, panics, capture stalls, suspend resumes; autostart refusals; logged RSS.
- **HA connection.** Connects, disconnects with a coarse reason (requested, reset, timeout), connected and down time.
- **Microphone and power.** Time per microphone state (open, muted, kill switch, busy, error), HA mutes, busy events; time per TV power state, transitions, standby entries, remote power-off requests.
- **Pipeline.** Wakes detected by HA, attempts, outcomes (`completed`, `completed_no_reply`, `intent_error`, `stt_empty`, `timeout`, `stt_timeout_or_no_speech_end`, `disconnect`, `shutdown`, `tts_error`, others), HA error codes, latency percentiles (`stt_end`, `intent_end`, `tts_start`, `playback_end`, from the wake word), attempts per local hour and per TV power state at the wake, failures followed by a retry within 30 s, push-to-talk runs, and wake-stage cycles that ended without a wake (HA ended the idle stage; not a user attempt).
- **HA debug runs** (optional): counts of wake detected, STT text present or empty, intent type handled or not, TTS done, error codes, and a join with the TV's attempts by start time.

Outcomes come from the daemon's one-line-per-run latency record (`run N latency ms (from wake word): ...`, see `RunTracker` in `crates/satellite/src/daemon.rs`) and the lines around it. Unknown lines are counted and ignored.

## Privacy

The report holds counts, booleans, lengths, timings and fixed error codes only. Debug-level log lines (the only ones that may carry transcripts) are dropped unread. HA event data is reduced to a whitelist at parse time; transcript, intent input, reply text and error messages are never copied, printed or cached, and the tests assert it with a canary string. The report prints no address, hostname or token; the archive redacts IPv4 addresses and drops debug lines. Files written (archive, cache, bundle) are mode `0600` and must live under `devices/private/` (gitignored).

## HA token

The tool never mints, refreshes or stores a token. Give it a file. A short-lived token (about 30 minutes) comes from HA's normal login flow (`/auth/login_flow`, then `/auth/token` with the authorization code); write it to a `0600` file outside the repository and delete it afterwards. Revoke only a token you created yourself. A long-lived token created in the HA profile also works; keep it out of the repository.

## What HA retains

Verified in HA 2026.10.4 (`homeassistant/components/assist_pipeline/run.py`, `runtime.py`, `websocket_api.py`): debug runs live in memory in a `LimitedSizeDict` of **10 runs per pipeline** (`STORED_PIPELINE_RUNS = 10`), oldest evicted first, lost when HA restarts, shared by every device on that pipeline. Each wake-stage cycle is a run (several minutes when idle on c3-a), so ten runs cover roughly an hour. HA is a window on the recent past only; for a day or more, run the tool regularly with `--archive-dir` (the summaries are merged by run id) or rely on the TV log. The first real run (HA restarted minutes earlier) saw 3 runs.

## Limits

- The TV log is size-rotated (256 KiB, then copied to one `.1` file and truncated). On c3-a it held about 72 KB after 20 hours of light use, so a file lasts roughly three days at that rate and less with heavy use. Older days are not available without `--archive-dir` and a regular run.
- Misses cannot be seen: a wake word HA never detected leaves nothing in the log or in HA. Only failed or immediately repeated attempts show.
- Whether the TV or PC was playing sound is not logged; the power state is screen/standby only. So "misses with sound" and "misses in quiet" cannot be told apart yet.

## Proposed daemon log additions (not implemented)

To split misses with sound from misses in quiet, the daemon would need to log, at info level and as numbers only (never audio):

1. **Per wake-stage cycle**, at its end: duration, mic level (RMS and peak in dBFS after the front-end, plus the 10th-percentile 20 ms RMS as a noise floor), share of frames with speech-like energy, and the same for the echo-reference tap (TV output level) with a `tv_audio_active` flag. Speech-like frames with no wake in a cycle that ended idle are a proxy for misses, split by whether the TV was loud.
2. **Per run**, next to the latency line: the same levels over the second before the wake, the reference level, and with AEC on the delay lock and ERLE. This ties each outcome (empty STT, early cut-off) to loud or quiet.
3. **Capture counters per run**: chunks dropped and xruns since the previous run (the counters exist; they are logged at shutdown only).
4. **A miss marker** the owner can trigger (an HA button entity or a file/CLI trigger on the TV) that writes one line with the current context. It is the only source of ground truth for undetected wake words; the tool would count markers per context.
5. **TV volume, mute and foreground app** if a public interface for them is confirmed on the target firmware (unverified; needs a read-only experiment first).

Items 1 to 3 need the reference tap open, which costs a little CPU; they can ride on the existing 60 s debug metrics code.

## Tests

```sh
python3 -m unittest discover -s tools -p 'test_dogfood*.py'
```

They cover log parsing and outcome classification, sessions and start causes, per-day clipping, percentiles, the archive merge and its sanitising, the HA event whitelist (canary string), the websocket client against an in-process fake server, and the markdown and JSON output end to end.
