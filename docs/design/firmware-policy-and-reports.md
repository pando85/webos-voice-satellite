# Soft firmware policy, self-checks and one-click compatibility reports

- **Status:** design, accepted direction ([ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md)); **implemented in the repository (steps S1 to S7, host tests, fake Home Assistant and a file microphone only), NOT yet deployed to or verified on a TV.** Everything in this document about the TV's behaviour is `hypothesis` until section 10 has run on c3-a and c3-b; the self-check thresholds in particular are hypotheses. Owner decisions of 2026-11-04 that changed the first draft: no Home Assistant actions path (the report goes through the TV app's QR code, `ctl.sh report` and diagnostic entities, [section 5](#5-one-click-compatibility-report-options)), matching of the verified list by series plus firmware ([section 6.3](#63-verified-list-in-the-binary-and-in-the-docs)), and no hard gate for the microphone or TV control on unvalidated firmware.
- **Date:** 2026-11-04. **Owner decision (2026-11-04):** "make the mic work. Why do we stop it? It is better to start it, and if it doesn't work we have to have an easy way for users to report that it doesn't work for that firmware version (like creating an issue in GitHub through one button click or something easy)." TV control is not gated either ([ADR 0005](../decisions/0005-tv-control-entities.md), already in the code as `tv_control.follow_firmware_guard = false`).
- **Replaces:** the hard firmware gate. It extends [packaging section 9](packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist) (owner decision 2026-11-03, which still kept the gate until the release; the gate is now gone from the default configuration) and answers [install UX open question 5](install-and-config-ux.md#6-open-questions).
- **Devices in scope for TV facts:** c3-a (LG C3, webOS TV 8.3.0, firmware `03.30.74`) and c3-b (LG C3, webOS TV 8.3.0, firmware `03.30.73`). Nothing else has been seen.

Labels follow [AGENTS.md](../../AGENTS.md): `documented`, `observed-on-other-device`, `hypothesis`, `verified-on-target` (with unit and firmware). "Owner decision" is not evidence about the TV.

## 1. What the old hard guard covered, and why a soft policy is acceptable

**What it did.** On a firmware version missing from `safety.validated_firmware`, or when the version could not be read, the daemon never opened the capture PCM (verified-on-target, c3-a `03.30.74`, [P3 robustness](../discovery/c3-a/P3-robustness.md#firmware-guard)).

**The risk it was meant to cover** (an inference from the discovery, not a measured failure on any firmware):

1. LG can update the firmware silently and with it the audio stack: ALSA card names, the capture device, formats, and driver behaviour. A capture path that was tested only on `03.30.74` could open a different device, or decode a different format into noise (hypothesis).
2. The kernel oops seen on c3-a on 2026-11-01 came from **reading ASoC DAPM sysfs nodes** during a discovery probe (verified-on-target, c3-a, [D2](../discovery/c3-a/D2-audio-stack.md)). The daemon never reads those nodes (it reads `/proc/asound` and opens the PCM). Whether a PCM open or capture ioctl could hurt on an unseen audio driver is **unknown (hypothesis, no evidence either way)**. The gate could only turn that unknown into "the mic stays off".

**Evidence that points the other way.** The same capture path ran 81 minutes on c3-b, firmware `03.30.73`, which is not in the shipped default allow-list (the run's own config is not recorded here), with no `MIC BUSY` and no kernel problem recorded ([c3-b compatibility](../compatibility/c3-b.md), EXP-002, verified-on-target c3-b). One extra firmware is a sample of two, not a proof.

**Why the soft policy is acceptable (owner decision 2026-11-04).** The cost of the gate was a mic that never started for everyone outside a list of two firmwares, with no way to learn whether it would have worked. The remaining risk is a driver misbehaving on an unseen firmware; the policy below limits it with measurable checks, a visible status, bounded retries, a kill switch that works without the daemon's cooperation, and a fast way to report. It does **not** remove the risk of a kernel fault on an unknown audio driver: nothing in user space can. The strict mode stays available for owners who want the old gate.

## 2. Policy

### 2.1 Behaviour by situation

| Situation | Behaviour | Visible as |
| --- | --- | --- |
| Model and firmware are in the verified list ([section 6.3](#63-verified-list-in-the-binary-and-in-the-docs)) | Mic starts. Self-check runs as usual | `compat_status` = `verified`; `firmware_verified` on |
| Same series as a verified TV, firmware not listed | **Mic starts.** One `INFO` line (series verified, firmware not yet reported) and the self-check runs | `compat_status` = `series verified (firmware not yet reported)`; `firmware_verified` off |
| Any other model, series or webOS major | **Mic starts.** One `WARN` line at start (firmware, model, "not verified, report if it does not work") and the self-check runs | `compat_status` = `not verified`; `firmware_verified` off |
| Firmware or model cannot be read | Mic starts; warning | `compat_status` = `unknown`; `firmware_verified` unknown (no state) |
| Firmware differs from the one the daemon last ran on (stored in `etc/firmware_seen`; written at first start and after an acknowledgement) | Mic keeps running; `WARN firmware changed: <old> -> <new>`; the self-check reruns right away | `compat_status` = `firmware changed`; `firmware_changed` on until the acknowledge button or `ctl.sh ack-firmware`. The status app shows a banner |
| Self-check **fails** ([section 3.3](#33-what-the-daemon-does-on-a-failure)) | Mic closed; bounded retries; no crash loop | `capture_problem` on; `compat_status` = `capture problem`; the app's report screen and `ctl.sh report` carry the failing check |
| Self-check **warns** | Mic stays open | `selfcheck` text says why |
| `safety.firmware_policy = "strict"` and the version is not in `safety.validated_firmware` | The old gate: mic closed (opt-in; an empty list turns it off with a warning, as before) | `microphone_muted` on, reason "firmware guard" |
| Kill switch (HA mute, `ctl.sh mic off`, `/tmp/webos-satellite.disable`, app "Microphone off") | Unchanged: PCM closed, works even if the event loop hangs | `microphone_muted` on |

TV control is never gated by the firmware status (ADR 0005). Each control already has its own startup capability check and a failing control is switched off with a log line ([TV control design](tv-control.md)).

### 2.2 Configuration changes

- `[safety] firmware_policy = "warn" | "strict"`, default `"warn"`.
- `validated_firmware` is empty by default. In `"strict"` mode it is the list of the old gate; in both modes a firmware listed there also counts as verified (an **owner extension** of the built-in list: a firmware match without a model match). In `"warn"` mode an empty or missing list is normal. Installs whose kept config still holds the old default `["03.30.74"]` behave like `"warn"` (the new default applies unless they set `"strict"`).
- `[safety] self_check = true`; `false` skips the checks (the status is then `not checked`) and nothing else changes. `self_check_window_s = 10` is the measurement window (a test aid and a tuning point while the thresholds are hypotheses).
- `[safety] firmware_seen_file = "firmware_seen"` (relative to the config directory, so `etc/firmware_seen`): the last firmware string the daemon ran on, [section 3.4](#34-firmware-changed-marker).
- `[report] enabled = true`, `repo = "pando85/webos-voice-satellite"` (the repository the link points at, so a fork can point to itself), `dir` (where the daemon keeps the report files and the trigger files of [section 8](#8-presentation-by-surface); default `<config dir>/../run` when the config directory is named `etc`, otherwise `/tmp/webos-satellite`). Nothing is sent anywhere by the daemon.
- `--check-firmware` stays for scripts and does **not** read the model: exit 0 when the firmware is in the built-in list or in `validated_firmware`, 1 when it is not or cannot be read, 3 in strict mode with an empty list. `packaging/webos/autostart.sh` blocks the start on a non-zero exit only in strict mode; `ctl.sh status` shows the policy result and `firmware_ok` means "may start".
- `webos-satellite --report [--full|--json]` builds a static report (no self-check result) from the TV facts, for `ctl.sh report` when the daemon has not run.

## 3. Self-checks

Implementation choices (2026-11-04, `crates/compat` plus the capture thread; host tests only): the window is measured on the **normal first open** (the engine is fed by the capture thread, there is no second open); C2 is read from `/proc/asound` before the open and skipped when those files cannot be read; a PCM that is **busy** (LG hands-free) is reported as a capture problem with the reason busy but **retried forever** with the existing slow backoff (it is an environment, not an incompatibility, and the user may turn hands-free off later); every other open failure uses the bounded 5, 10, 20, 40 and 80 s retries and then stops until the button, `ctl.sh selfcheck`, a restart or a firmware change; a file microphone with `pause_when_idle` is not checked (`not run`).

### 3.1 Rules

- **No new risky access.** The checks use only `/proc/asound/cards`, `/proc/asound/pcm` (read-only), the normal PCM open the daemon already does, and level statistics the capture thread already computes (`in rms`, `in peak`, `clipped`, `xruns`, chunks sent: `crates/satellite/src/capture.rs`). **Never** `/sys/devices/platform/soc-audio/*`, `dapm_widget`, `/sys/kernel/debug`, or a recursive read of `/sys` ([hazard](../discovery/c3-a/rooted-tv-improvements.md)).
- **No recording.** The checks observe the stream the daemon would anyway capture; no sample is stored or logged. Only numbers leave the capture thread.
- **No extra PCM open.** The "levels over N seconds" check observes the first seconds of the normal open, so it adds no second open on a driver that was not seen before.
- Runs at the first open after start, after a firmware change, and on the HA button "Re-run microphone check" or `ctl.sh selfcheck`. If the mic is closed (muted, kill switch, not paired) the result is `not run (microphone closed)` and it runs at the next open.

### 3.2 Checks

| ID | Check | PASS | WARN | FAIL | Source of the signal |
| --- | --- | --- | --- | --- | --- |
| C1 | Firmware and model readable | both read | one missing | - (status `unknown`, never a failure) | `/var/run/nyx/os_info.json` `webos_manufacturing_version` (verified-on-target c3-a); model from `luna-send` `com.webos.service.tv.systemproperty/getSystemInfo` with the key `modelName` only (verified-on-target c3-a, D0-D1; never `apiadapter/system/getSystemInfo`, which returns the serial) |
| C2 | Capture PCM found **by card id and capabilities**, not by index | a card with a capture PCM that offers the configured rate and a supported format | present but needs a different format or rate than configured | no capture PCM, or only the configured device is missing | `/proc/asound/cards`, `/proc/asound/pcm`. Today the config names `hw:LGSIC,0`; the runtime discovery of Phase 5 replaces the name. [AGENTS.md](../../AGENTS.md) forbids treating `hw:1,0` or one name as universal |
| C3 | Open succeeds | opened | opened after retries | fails 5 times in a row (bounded, [3.3](#33-what-the-daemon-does-on-a-failure)). "Busy" (LG hands-free on) is reported as busy, not as incompatible | the daemon's existing open and `MIC BUSY` path |
| C4 | Frames flow | at least 90 % of the expected chunks in the window | 50 to 90 % | none in 3 s (the existing stall watchdog is 5 s) | `chunks_sent`, `xruns` |
| C5 | Level not stuck | peak-to-peak above 1 LSB at some point in the window | - | every input sample identical for the whole window ("stuck") | capture stats |
| C6 | Level not silent | input peak above -80 dBFS | peak at or below -80 dBFS (probably a wrong format or a muted mic, but a very quiet room is possible) | - | `in_peak_dbfs`. Reference: c3-a in a quiet room measured -67.6 dBFS RMS and -53.6 dBFS peak; a wrong 32-bit decode of the same stream gave a peak of -92.6 dBFS (verified-on-target c3-a, [D3](../discovery/c3-a/D3-mic-identification.md), [P1 capture](../discovery/c3-a/P1-capture-validation.md)) |
| C7 | Level not saturated | clipped fraction below 0.5 % and RMS below -10 dBFS | above either | - | `clipped`, `in_rms_dbfs`. The first 250 ms after open is discarded because of the startup burst (verified-on-target c3-a) |
| C8 | Reference/playback levels (only when the AEC reference is configured) | reference stream present | reference silent while the TV plays | - | AEC metrics. Optional, skipped without a reference |
| C9 | Power states supported (from the daemon's own state subscription) | reported, not judged | - | - | what the power module saw since start; goes into the report only |
| C10 | TV control capability results | per control `ok`, `off` or `failed` | - | - | the startup checks of `tv_control` |

**Window.** N = 10 seconds of audio after the 250 ms discard (about the 10 s used in the c3-a levels measurement). **Thresholds are `hypothesis`** until run on c3-a and c3-b (section 10). The pure function that turns statistics into a verdict has a table-driven test with those numbers and is the single place to change them.

### 3.3 What the daemon does on a failure

- **FAIL (C2, C3, C4, C5): measured, nothing useful can come out.** The daemon closes the PCM, sets `capture_problem` on and `compat_status` to `capture problem`, writes the result to `run/selfcheck.json`, logs one line with the reason and the hint "open the TV app, Report compatibility", and keeps running (HA connection, entities, TV control and the kill switch stay up). `microphone_muted` (the indicator only; **HA's mute switch is not changed**) turns on and `listening` off, so the [listening indicator](../development.md#listening-indicator) stays truthful.
- **Bounded retries, never a crash loop.** The open is retried with backoff 5 s, 10 s, 20 s, 40 s, 80 s (the existing open-retry path, capped). After the fifth consecutive failure the daemon stops retrying on its own. It tries again only on the HA button "Re-run microphone check", on `ctl.sh selfcheck`, on a daemon restart, or after the firmware changed. The supervisor's crash-loop marker is **not** involved: a failed self-check is a state, not an exit. A hung capture thread still ends the process through the existing stall watchdog (exit 70 or 71) and the supervisor's existing limits apply.
- **WARN (C6, C7, a lower C4):** the mic stays open. A quiet room or a loud TV is a normal environment, not a failure. The `selfcheck` text names the reason.
- The result is **sticky across HA reconnects** (the states are resent) and cleared by the next run.
- A self-check on a **verified** model and firmware behaves the same. One policy, no special case.

### 3.4 Firmware-changed marker

`etc/firmware_seen` holds the last firmware string the daemon ran on (one line, no other data). Written at the first start after install and on acknowledgement; read at every start. It is not written automatically when the version differs, so a change stays visible until acknowledged (HA button, `ctl.sh ack-firmware`, or the app).

## 4. Home Assistant entities

All on the existing satellite device. Object ids are stable ([entity keys are hashes of the object id](../development/esphome-api.md)). All diagnostic except the two buttons (config category). The ESPHome API crate has generic `binary_sensor`, `button`, `number`, `select`, `switch` and `text_sensor` (the last two were added for TV control, [ADR 0005](../decisions/0005-tv-control-entities.md)). **The ESPHome native API has no entity attributes**, so model and firmware cannot be attributes (the earlier sketch in [packaging 9.4](packaging.md#94-runtime-behaviour-at-the-release-replaces-the-block) assumed they could): each is its own entity.

| Entity | Type | Object id | Values | Category |
| --- | --- | --- | --- | --- |
| Firmware verified | `binary_sensor` | `firmware_verified` | on only when model and firmware are both in the verified list; off for a series match with another firmware and for not verified; no state (unknown) when unreadable | diagnostic |
| TV firmware | `text_sensor` | `tv_firmware` | `03.30.74` | diagnostic |
| TV model | `text_sensor` | `tv_model` | the model name the TV reports | diagnostic |
| Compatibility | `text_sensor` | `compat_status` | `verified`, `series verified (firmware not yet reported)`, `not verified`, `firmware changed`, `capture problem`, `unknown`, `checking` | diagnostic |
| Microphone self-check | `text_sensor` | `selfcheck` | short text, for example `passed (peak -54 dBFS)`, `warning: very quiet`, `failed: no audio frames`, `not run (microphone closed)` | diagnostic |
| Capture problem | `binary_sensor` | `capture_problem` | on after a FAIL; device class `problem` | diagnostic |
| Firmware changed | `binary_sensor` | `firmware_changed` | on until acknowledged | diagnostic |
| Compatibility report | `text_sensor` | `report_status` | says where the report is: for example `ready: TV app, Report compatibility, or ctl.sh report`, or `withheld by the privacy scan` | diagnostic |
| Re-run microphone check | `button` | `selfcheck_run` | press to rerun [section 3](#3-self-checks) | config |
| Acknowledge firmware change | `button` | `firmware_ack` | writes `etc/firmware_seen`, clears the marker | config |

There is **no** "Create compatibility report" button: the report is not delivered through Home Assistant (owner decision 2026-11-04, [section 5](#5-one-click-compatibility-report-options)). The daemon keeps the report files fresh instead and the TV app and `ctl.sh report` show them. States are plain short strings (at most 255 characters; HA's state limit, `documented`, home-assistant.io). Ten entities; the self-check ones (`selfcheck`, `capture_problem`, `selfcheck_run`) are not listed with `[safety] self_check = false`, and `report_status` is not listed with `[report] enabled = false`.

## 5. One-click compatibility report: options

The report content and link are built **once**, in the daemon, so the allow-list lives in one place and every front end shows the same text ([section 7](#7-report-content-and-the-allow-list)).

### 5.1 Evidence collected (read 2026-11-04)

| Claim | Evidence | Label |
| --- | --- | --- |
| New-issue URL query parameters: `title`, `body`, `labels`, `milestone`, `assignees`, `projects`, `template` (the template file under `.github/ISSUE_TEMPLATE/`) | [GitHub docs, Creating an issue, "from a URL query"](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue) | documented |
| "You can also use URL query parameters to fill custom text fields that you have defined in issue form templates"; the form field `id` "is the canonical identifier for the field in URL query parameter prefills" | same page; [form schema syntax](https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/syntax-for-githubs-form-schema) | documented |
| A URL over the server limit returns `414 URI Too Long`; the limit is **not stated** | same page | documented (limit unknown) |
| The `labels` parameter needs permission to label; without it the page says the URL can return `404` | same page | documented, so the link **does not use `labels`**; the form file sets the label |
| Whether a `render: text` textarea accepts the prefill, and whether `body` is honoured for issue forms | not stated | **hypothesis**: the link uses the field id and the plan checks it ([section 10](#10-what-to-verify-after-deployment)) |
| The user must be logged in to GitHub and press Submit; nothing is posted by the link | GitHub behaviour of `issues/new` | documented |
| An ESPHome device can call any HA action (`HomeassistantActionRequest`, api.proto message 35, fields `service`, `data`, `data_template`, `variables`, `is_event`, `call_id`) only when the user enables "Allow the device to perform Home Assistant actions" in the device options. It is off by default; HA warns "Only enable this if you trust the device" | [HA ESPHome integration](https://www.home-assistant.io/integrations/esphome/), [ESPHome API docs](https://esphome.io/components/api/), `crates/esphome-api/proto/api.proto` | documented, for the rejected option b only |
| In HA's handler (`components/esphome/manager.py`, `dev` branch read 2026-11-04) the service string is split at the first dot, `data_template` is rendered with Jinja **on the HA side**, an event is accepted only in the `esphome.` domain and is **not** gated by the option, a service call is gated by the option, and with it off HA creates a repair issue and runs nothing. With a `call_id` HA sends back a success or failure message | source reading | documented (source reading), for the rejected option b only |
| `persistent_notification.create` takes `message`, `title`, `notification_id` and is a normal HA action | [HA docs, persistent notification](https://www.home-assistant.io/integrations/persistent_notification/) | documented, for the rejected option b only |
| A state is limited to 255 characters, so a `text_sensor` cannot carry the full report or a long URL | HA core behaviour | documented |

### 5.2 Options

| | Option | User effort | Needs | Weak points |
| --- | --- | --- | --- | --- |
| a | **Prefilled GitHub issue URL** (the link itself) | open link, log in if needed, tick the privacy box, Submit | a GitHub account; URL under about 2000 characters (the limit is unknown, [5.1](#51-evidence-collected-read-2026-11-04)) | Not a channel by itself: the link must be shown somewhere |
| b | **HA button** "Create compatibility report" to a notification with the link (`persistent_notification.create` through `HomeassistantActionRequest`) | press the button in HA, tap the link in the notification, then as (a) | the HA option "Allow the device to perform Home Assistant actions", which gives the device the ability to call **any** HA action | **Rejected by the owner (2026-11-04):** the privilege increase for a device that is a TV running our daemon is not worth it, and the owner prefers the QR code. Not implemented; the satellite sends no Home Assistant action and the ESPHome API crate keeps only the vendored proto |
| b2 | A `text_sensor` "Report link" with a compact URL under 255 characters | copy the text from the entity dialog, paste in a browser | nothing in HA | Plain text, not clickable; only the essentials fit. Optional, **not built** (step S8) |
| c | **TV status app** "Report compatibility" with a QR code of the URL, and the report text below | open the app on the TV, scan with a phone, then as (a) | a QR generator in the app, vendored and permissively licensed, no network at runtime | A dense QR on a TV screen: the URL is kept short enough (about 1000 characters, [7.4](#74-length-budget)); not verified on the TV's browser engine |
| d | **`ctl.sh report`** | run it over SSH, copy the text or open the printed URL | nothing | Technical users only, but it is the base the others call and the fallback that always works |

### 5.3 Recommendation (as decided)

Build the report once in the daemon and offer it through **c and d**, with (a) as the target both show, plus the diagnostic entities of [section 4](#4-home-assistant-entities):

1. **At the TV (main path): c.** Open the app, press **Report compatibility**, scan the QR with a phone. GitHub opens the form already filled in; tick "I read the report"; press **Submit new issue**. The screen also shows the report text and the short link for typing.
2. **Always: d** (`ctl.sh report`, `--full`, `--json`): the same text and link on the command line.
3. **Home Assistant** shows the status only: `compat_status`, `selfcheck`, `capture_problem`, `firmware_changed`, the buttons to re-run the check and acknowledge a firmware change, and `report_status`, which says where the report is. HA never receives the report.
4. **b** is rejected (owner decision) and **b2** is optional and not built.

**What does not work or is rejected:** a `text_sensor` carrying the URL as the only path (not clickable, 255 limit); posting the issue from the TV (needs a GitHub token on the device, rejected: no credentials on the TV); `homeassistant.event` as the transport (works without the option, but a user would need an automation to turn it into a notification, so it is no easier).

## 6. GitHub side

### 6.1 Issue form

[`.github/ISSUE_TEMPLATE/compatibility-report.yml`](../../.github/ISSUE_TEMPLATE/compatibility-report.yml) (added with this design; checked against GitHub's issue-forms schema by `tools/check_docs.py`: ids, `required` validations, the single label). Fields:

| id | Type | Prefilled by the link | Notes |
| --- | --- | --- | --- |
| `report` | textarea, `render: text` | yes (the short report, [7.4](#74-length-budget)) | required |
| `details` | textarea | no (the user may paste the full report from the app or `ctl.sh report --full`) | optional |
| `comments` | textarea | no | optional: what did or did not work, how the TV is used (LG hands-free off?) |
| `confirm` | checkboxes | no | required: "I reviewed the report: it has no serial, address or other private data, and I know the issue is public". One tick, chosen over a silent submit because the issue is public |

The form has a top note: what the report contains, what it never contains, that the daemon sends nothing by itself, and that the issue is public. The title is prefilled as `[Compat] <model> fw <firmware>: <verdict>` where the verdict is `works`, `partly works` or `does not work`, computed from the self-check (a positive report uses the same form). The label `compat-report` comes from the form file; it must exist in the repository (created on 2026-11-04 together with `works`, `broken` and `needs-info`; whether a missing label is dropped silently is `hypothesis`). Maintainers add `works`, `broken` or `needs-info` when triaging.

### 6.2 Link format

`https://github.com/<repo>/issues/new?template=compatibility-report.yml&title=<encoded>&report=<encoded>`. No `body`, no `labels`. Percent-encoded UTF-8; the daemon builds it, with a test that decodes it back and compares.

### 6.3 Verified list in the binary and in the docs

- **Matching by series and firmware (owner decision 2026-11-04).** A TV is "verified" when its model matches a series pattern (`OLED??C3*` for `LG OLED C3 (2023, webOS 8.x)`, case-insensitive glob), its webOS release starts with the series' major number and its firmware is listed; the same series with another firmware is `series verified (firmware not yet reported)`; anything else is `not verified`. Other sizes and regional variants of a tested series count as the series, which is what the owner wants; the firmware column is what stays exact.
- **Data file** `data/verified-models.toml` in the repository, compiled into the daemon (`include_str!`): `[[series]]` tables (`id`, `name`, `models`, `webos`) with `[[series.firmware]]` rows (`version`, `unit`, `model` tested, `evidence`). A model or firmware not in the list is "not verified", never an error. Updating the list ships with a release, so a user does not edit it (an owner can add a firmware to `safety.validated_firmware`). Rows today: c3-a `03.30.74` and c3-b `03.30.73`.
- **Docs** [`docs/compatibility/verified-models.md`](../compatibility/verified-models.md) has three tables: **Verified** (the same rows as the data file, plus webOS, what passed, date, evidence), **Reported working (unconfirmed)** and **Known problems**. `tools/check_docs.py` fails when the series and firmware columns of the Verified table differ from the data file.
- **How a row is added from a report.** A maintainer reads the issue (the report is allow-listed text), then:
  - one report with every core check passing and an author the maintainer does not already know: a row in **Reported working (unconfirmed)**, label `works`. It stays `observed-on-other-device` in the project's terms;
  - a second independent report (a different GitHub author) for the same model and firmware, or a maintainer run, moves it to **Verified** and into the data file (a firmware row of the series, or a new series) in the next release;
  - a failing report: a row in **Known problems**, an issue kept open with the label `broken`, and the finding recorded in a per-device record if someone with the TV can run an experiment.
  Reports never become `verified-on-target` on their own; that label needs the project's own experiment.

## 7. Report content and the allow-list

### 7.1 Allowed fields

| Group | Fields |
| --- | --- |
| Software | satellite version, build architecture (`armv7`, `aarch64`), report schema version |
| TV | model name, webOS release (`webos_release`), firmware (`webos_manufacturing_version`), kernel architecture and kernel numeric version (`5.4.96`, no build string), policy result (`verified`, `not verified`, `firmware changed`) |
| Audio | PCM summary: card **ids** with their capture and playback devices, and for the chosen capture device its channels, format and rate; the capture device chosen |
| Self-check | C1 to C8 results (`pass`, `warn`, `fail`, `skipped`) and the reason codes |
| Capture levels | over the window: input RMS and peak in dBFS, clipped count, xruns, chunks sent and dropped |
| Power | the power states the daemon saw since start, and whether screen-off capture was seen |
| TV control | per control `ok`, `off` or `failed` (C10) |
| Verdict | `works`, `partly works`, `does not work` |

### 7.2 Never in a report

Serial number, MAC, IP, hostname, device name, TV name, HA URL or address, Wi-Fi name, pairing or API keys, tokens, any audio or transcript, log lines, the `etc/` contents, file paths other than fixed ones, user names.

### 7.3 How the allow-list is enforced

- The report is built from a **typed struct**. Each field has its own renderer. Free strings (model, card ids, kernel version, reason codes) pass a **token sanitizer** that keeps `[A-Za-z0-9._+-]` and spaces up to 48 characters and drops the rest. The builder never receives a raw dump (no `os_info.json` map, no `getSystemInfo` reply, no `/proc/asound` text): the inputs are parsed into the typed fields first.
- **Test 1 (seeded identifiers):** feed a fake `os_info.json`, a fake `getSystemInfo` reply with serial, a fake `/proc/asound`, a config with a device name, a HA URL, a pairing key, a MAC and an IP, and a hostname; assert none of the seeded strings appears in the short report, the full report or the URL.
- **Test 2 (schema):** the list of rendered field names equals a constant in the test; adding a field fails the test until the allow-list is updated on purpose.
- **Test 3 (fail closed):** a final scan rejects any output matching a MAC, an IPv4 or IPv6 address, a 32-character or longer hex or base64 run, `http` outside `github.com/<repo>`, or a line over 200 characters. A hit withholds the report and logs the rule that fired, not the text.
- A report is shown to the user (the app, `ctl.sh report`) before it goes anywhere; the issue form shows it again.

### 7.4 Length budget

- **Short report** (in the URL, QR): at most about 600 bytes before encoding, so the whole link stays near 1000 characters (about 1300 with the title in a worst case; the cap is 1500). Priority order if it does not fit: verdict, model, firmware, webOS, app version, architecture, check results, levels, PCM list (shortened to the chosen device), power, TV control.
- **Full report:** no hard limit (app text, `ctl.sh report --full`); the user may paste it into `details`.
- Test: the generated link of a worst-case fake TV is at most 1500 characters and decodes to the same text.

### 7.5 Report text (schema v1, illustrative)

```
webos-satellite compatibility report v1
result: works
app: 0.1.1 armv7
tv: OLED42C34LA webOS 8.3.0 fw 03.30.74 kernel armv7l 5.4.96
policy: verified
pcm: LGSIC cap0 2ch S24_LE 16000; chosen LGSIC/0
checks: C1 pass C2 pass C3 pass C4 pass C5 pass C6 pass C7 pass C8 skipped
levels(10s): in rms -67.6 peak -53.6 clipped 0 xruns 0 chunks 312/0
power: Active; Screen Off not seen
tv-control: screen ok mute ok volume ok input ok app ok
```

The values above are the c3-a measurements already in the repository; the text is an illustration of the format, not a run of the report.

## 8. Presentation by surface

- **HA:** diagnostic entities only ([section 4](#4-home-assistant-entities)); no report and no action goes through Home Assistant (owner decision 2026-11-04).
- **TV app** (`packaging/webos/app`): a "Report compatibility" button; a screen with the QR (large, high contrast; generated offline in the app by a vendored library, no network), the short URL text for typing as a fallback, and the report text; a "Back" key. The app calls `ctl.sh report --json` through the existing `exec` path. It also shows a banner for `firmware changed`, `capture problem` and `not verified`, with the buttons to acknowledge and to re-run the check.
- **`ctl.sh`:** `ctl.sh report` prints the short report and the link; `--full` prints the full report; `--json` is for the app; `ctl.sh selfcheck` reruns the check; `ctl.sh ack-firmware` acknowledges a firmware change. The daemon keeps the files `report.json`, `report.short.txt`, `report.full.txt`, `report.url` and `compat.json` fresh in `[report] dir`, and polls the trigger files `selfcheck-run` and `firmware-ack` there (a second process must not open the PCM, so `ctl.sh` asks the running daemon through them). When the daemon is not running, `ctl.sh report` shows the saved files and says they are old, or, with none, calls `webos-satellite --report` for a static report; `ctl.sh ack-firmware` then writes `etc/firmware_seen` itself.

## 9. Implementation plan

Small steps, each for one agent. **Status 2026-11-04: S1 to S7 are implemented in the repository (host tests, fake Home Assistant, file microphone); none of it has run on a TV.** S4 and S8 changed with the owner's decisions (below). Each step ended with the repository gates: `cargo test`, `cargo clippy`, `sh -n tools/probe/probe.sh`, `python3 tools/check_docs.py`, and for packaging `packaging/webos/tests/run.sh`.

| Step | Scope | Tests |
| --- | --- | --- |
| **S1** Policy core. **Implemented 2026-11-04** | `firmware.rs`: the verdict becomes a status (verified, series verified, not verified, unknown) independent of capture, with the series lookup in `data/verified-models.toml`; `[safety] firmware_policy`, `self_check`, `self_check_window_s`, `firmware_seen_file` in `config.rs`; `firmware_seen` read and write; `daemon.rs` gate only in strict mode; `--check-firmware` exit codes as in [2.2](#22-configuration-changes); `autostart.sh` stops blocking in warn mode; defaults in `packaging/webos/config.default.toml` and `config/example.toml` | table tests for each row of [2.1](#21-behaviour-by-situation); the `firmware_block...` test adapted to strict; a test that unvalidated firmware no longer keeps the mic off; `packaging/webos/tests/run.sh` autostart case on an unverified version |
| **S2** Self-check engine (pure, parallel with S1). **Done 2026-11-04** (library only, not wired) | `crates/compat` (`webos-compat`), no dependencies: `window.rs` (statistics, thresholds as `Thresholds` defaults, the single verdict function `grade_window` for C4 to C7), `pcm.rs` (parsers for `/proc/asound/cards` and `/proc/asound/pcm` text, C2 by card id), `check.rs` (C1 and C8 helpers, grades, fixed reason codes), `engine.rs` (state machine with injected time: window, no-frame fail at 3 s, one first try plus five retries at 5, 10, 20, 40 and 80 s, then stop, rerun). Thresholds are `hypothesis`; open points: below 50 % of the frames is still a WARN (the table defines FAIL only as "none"), and a stuck signal is peak-to-peak of at most 1 LSB | table tests with c3-a numbers (-67.6 dBFS pass, -92.6 peak warn, all-equal samples fail, clipped warn); engine tests with a fake clock (silent warns, constant fails and closes, retries bounded) |
| **S3** Report builder (pure, parallel with S1). **Done 2026-11-04** (library only, not wired; `--report` flags belong to S6) | `crates/compat`: `report.rs` (typed `ReportInput`, short and full text, JSON, truncation by priority), `sanitize.rs` (token sanitizer that drops identifier-looking values whole, fail-closed scan), `link.rs` (prefilled link with `template`, `title` and `report`, encoding, parser). The text format differs from the illustration in [7.5](#75-report-text-schema-v1-illustrative): lines are `key: label=value, ...` and card and device are separate fields. The verified-list lookup is left to S7 (the data file does not exist yet) | the three tests of [7.3](#73-how-the-allow-list-is-enforced) (seeded identifiers in every input, pinned field list, scan rules); link length (about 930 characters for a c3-a sized report, at most 1500 in the worst case) and round trip; positive and negative reports |
| **S4** ESPHome API. **Reduced** | generic `text_sensor` (done with TV control). The Home Assistant action part (`SubscribeHomeassistantServicesRequest`, `AppCommand::CallHaAction`, message 35) is **dropped**: the owner rejected the HA actions path (2026-11-04) | none for the dropped part |
| **S5** Daemon wiring. **Implemented 2026-11-04** | entities of [section 4](#4-home-assistant-entities); the capture thread feeds S2 on the normal stream (no extra PCM open, no sysfs or debugfs reads, no recording); failure handling of [3.3](#33-what-the-daemon-does-on-a-failure) (close, backoff 5 to 80 s, stop, manual rerun; busy retried forever); the `microphone_muted` indicator while closed by a failed check; the report files and trigger files of [section 8](#8-presentation-by-surface); `report_status` | host end-to-end with a file mic: silent file gives warn, constant file gives fail and `capture_problem`, then rerun; fake-HA scenario for the new entities; the retry count is bounded (a test with a fake clock) |
| **S6** `ctl.sh`, status app. **Implemented 2026-11-04** | `ctl.sh report`, `selfcheck`, `ack-firmware`; status JSON gets the policy fields; app "Report compatibility" with a vendored QR generator (license check and the third-party notice, [packaging section 8](packaging.md#8-license-decision-and-dependency-audit)); app shows the firmware-changed banner; `webos-satellite --report` | `packaging/webos/tests/run.sh` cases; `app_smoke.js` for the screen and for the QR payload equal to the link; QR decode check on the host (a decoder in the test only) |
| **S7** GitHub and docs. **Done 2026-11-04** | the issue form checked against the issue-forms schema; the labels `compat-report`, `works`, `broken`, `needs-info` created; `docs/compatibility/verified-models.md` (tables of [6.3](#63-verified-list-in-the-binary-and-in-the-docs)); README "Verified TVs" section; `data/verified-models.toml` with the two rows the owner approved; `tools/check_docs.py` checks the two lists agree and that the issue form has the expected ids and required fields; CONTRIBUTING note on reports; development.md, packaging and install UX text updated | `python3 tools/check_docs.py`; `tools/test_check_docs.py` |
| **S8** (optional) b2. **Not built** | `text_sensor` "Report link" under 255 characters | length test |

## 10. What to verify after deployment

Nothing in this document has run on a TV yet. All of it is **silent** (no sound, no power-state change) and goes through Home Assistant entities, `ctl.sh` and the app; each TV run needs the owner's authorization per [AGENTS.md](../../AGENTS.md).

**c3-a** (runs the mic today):

1. Deploy the build over the running daemon by the ipk update path ([EXP-009](../experiments/EXP-009-ipk-install-update.md)). In HA the new entities appear on the existing device: `tv_firmware` = `03.30.74`, `tv_model` (should read `OLED42C34LA`), `compat_status` = `verified`, `selfcheck`, `firmware_verified` on, `capture_problem` off, `firmware_changed` off, `report_status` saying where the report is. The first start writes `etc/firmware_seen`.
2. `selfcheck` shows `passed` and a peak near the c3-a quiet-room reference (about -54 dBFS peak, -67.6 dBFS RMS); record the real numbers and fix the C6 and C7 thresholds (hypothesis until then). Compare `listening` and `microphone_muted` before and after: the check must not add an open or a visible change.
3. `ctl.sh report`: read the output against the TV's real serial, MAC and address (looked up privately, not recorded) and confirm none appears; run it with the daemon stopped and running.
4. App: open "Report compatibility"; the QR scanned with a phone opens the form **filled** (`report` in the textarea, title set). Do not press Submit; close the tab. This also settles the prefill `hypothesis` rows in [5.1](#51-evidence-collected-read-2026-11-04). A QR that is too dense for the TV's browser is a finding to record.
5. Firmware-changed path: with the owner's authorization, edit `etc/firmware_seen` to another value, restart the daemon, and see `compat_status` = `firmware changed`, the app banner, the rerun check, then the acknowledge button and `ctl.sh ack-firmware`.
6. Kill switch while a check is running; stop and start the daemon repeatedly (5 cycles) to see no retry storm.

**c3-b** (second firmware, `03.30.73`): the same list, when the owner agrees. It is a verified firmware of the same series, so `compat_status` = `verified`; a config with the series pattern changed in a test build shows `series verified (firmware not yet reported)` and `not verified` with the mic open.

**On the host (file mic, fake HA):** the failure paths (stuck file, silent file, saturated file, no PCM), the bounded retries, busy retried forever, and that no crash loop marker is written.

**Report prefill check (no TV):** open the generated link in a browser and confirm the form is filled. If the textarea is not filled, fall back to `body` for the first line and to the copy button.

## 11. Questions for the owner (answered 2026-11-04)

1. **HA option.** Rejected: no Home Assistant actions path; the owner prefers the QR code ([section 5](#5-one-click-compatibility-report-options)).
2. **Repository and labels.** `pando85/webos-voice-satellite`; `compat-report`, `works`, `broken`, `needs-info`.
3. **GitHub login.** A user must be logged in to GitHub to submit; accepted (an anonymous channel would add a third party and a place for private data).
4. **Verified rows.** c3-a `03.30.74` and c3-b `03.30.73`, as one series.
5. **Model matching.** By series plus firmware, not the exact model string ([6.3](#63-verified-list-in-the-binary-and-in-the-docs)).
6. **Strict mode default.** `"warn"` is the default; old installs behave like `"warn"` unless they set `"strict"`.

## Sources

Read 2026-11-04: [GitHub, creating an issue (URL query)](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue); [GitHub form schema syntax](https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/syntax-for-githubs-form-schema); [HA ESPHome integration](https://www.home-assistant.io/integrations/esphome/); [ESPHome API component](https://esphome.io/components/api/); Home Assistant core `homeassistant/components/esphome/manager.py` (`async_on_service_call`); `crates/esphome-api/proto/api.proto` (messages 35 and 130). Repository: [P3 robustness](../discovery/c3-a/P3-robustness.md), [c3-b compatibility](../compatibility/c3-b.md), [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md), [ADR 0005](../decisions/0005-tv-control-entities.md), [packaging section 9](packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist).
