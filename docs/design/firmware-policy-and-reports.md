# Soft firmware policy, self-checks and one-click compatibility reports

- **Status:** design, accepted direction ([ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md)); **the daemon is not changed yet; only the pure libraries of steps S2 and S3 exist** (`crates/compat`, host unit tests only, not wired, not run on a TV). Until step S1 of [section 9](#9-implementation-plan) lands, the daemon behaves as before: it keeps the microphone off on a firmware that is not in `safety.validated_firmware`.
- **Date:** 2026-11-04. **Owner decision (2026-11-04):** "make the mic work. Why do we stop it? It is better to start it, and if it doesn't work we have to have an easy way for users to report that it doesn't work for that firmware version (like creating an issue in GitHub through one button click or something easy)." TV control is not gated either ([ADR 0005](../decisions/0005-tv-control-entities.md), already in the code as `tv_control.follow_firmware_guard = false`).
- **Replaces:** the hard firmware gate. It extends [packaging section 9](packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist) (owner decision 2026-11-03, which still kept the gate until the release) and answers [install UX open question 5](install-and-config-ux.md#6-open-questions).
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
| Model and firmware are in the verified list ([section 6.3](#63-verified-list-in-the-binary-and-in-the-docs)) | Mic starts. Self-check runs as usual | `compat_status` = `verified` |
| Not in the list (any other firmware or model) | **Mic starts.** One `WARN` line at start (firmware, model, "not verified, report if it does not work") and the self-check runs | `compat_status` = `not verified`; `firmware_verified` off |
| Firmware or model cannot be read | Mic starts; warning | `compat_status` = `unknown`; `firmware_verified` unknown |
| Firmware differs from the one the daemon last ran on (stored in `etc/firmware_seen`; written at first start and after an acknowledgement) | Mic keeps running; `WARN firmware changed: <old> -> <new>`; the self-check reruns right away | `compat_status` = `firmware changed`; `firmware_changed` on until the acknowledge button or `ctl.sh ack-firmware`. The status app shows a banner |
| Self-check **fails** ([section 3.3](#33-what-the-daemon-does-on-a-failure)) | Mic closed; bounded retries; no crash loop | `capture_problem` on; `compat_status` = `capture problem`; the report button gets a hint |
| Self-check **warns** | Mic stays open | `selfcheck` text says why |
| `safety.firmware_policy = "strict"` and the version is not in `safety.validated_firmware` | The old gate: mic closed (opt-in) | `microphone_muted` on, reason "firmware guard" |
| Kill switch (HA mute, `ctl.sh mic off`, `/tmp/webos-satellite.disable`, app "Microphone off") | Unchanged: PCM closed, works even if the event loop hangs | `microphone_muted` on |

TV control is never gated by the firmware status (ADR 0005). Each control already has its own startup capability check and a failing control is switched off with a log line ([TV control design](tv-control.md)).

### 2.2 Configuration changes

- New `[safety] firmware_policy = "warn" | "strict"`, default `"warn"`.
- `validated_firmware` stays as the strict-mode list and as an **owner extension** of the built-in verified list. In `"warn"` mode an empty or missing list is normal. The shipped default (`packaging/webos/config.default.toml`, `config/example.toml`) becomes empty with a comment; installs that already hold the old default behave like `"warn"` (the new default applies unless they set `"strict"`).
- New `[safety] self_check = true`. `false` skips the checks (the status is then `not checked`); nothing else changes.
- New `[report] repo = "pando85/webos-voice-satellite"`: the GitHub repository the report link points at, so a fork can point to itself. Nothing is sent anywhere by the daemon.
- `--check-firmware` stays for scripts, with exit codes 0 (verified), 1 (not verified or unreadable), 3 (strict list empty). `packaging/webos/autostart.sh` must stop treating a non-zero exit as "do not start" unless strict mode is on (today it refuses to start: `autostart.sh` line 81 onwards); `ctl.sh status` shows the policy result instead of `firmware_ok`.

## 3. Self-checks

### 3.1 Rules

- **No new risky access.** The checks use only `/proc/asound/cards`, `/proc/asound/pcm` (read-only), the normal PCM open the daemon already does, and level statistics the capture thread already computes (`in rms`, `in peak`, `clipped`, `xruns`, chunks sent: `crates/satellite/src/capture.rs`). **Never** `/sys/devices/platform/soc-audio/*`, `dapm_widget`, `/sys/kernel/debug`, or a recursive read of `/sys` ([hazard](../discovery/c3-a/rooted-tv-improvements.md)).
- **No recording.** The checks observe the stream the daemon would anyway capture; no sample is stored or logged. Only numbers leave the capture thread.
- **No extra PCM open.** The "levels over N seconds" check observes the first seconds of the normal open, so it adds no second open on a driver that was not seen before.
- Runs at the first open after start, after a firmware change, and on the HA button "Re-run microphone check". If the mic is closed (muted, kill switch, not paired) the result is `not run (microphone closed)` and it runs at the next open.

### 3.2 Checks

| ID | Check | PASS | WARN | FAIL | Source of the signal |
| --- | --- | --- | --- | --- | --- |
| C1 | Firmware and model readable | both read | one missing | - (status `unknown`, never a failure) | `/var/run/nyx/os_info.json` `webos_manufacturing_version` (verified-on-target c3-a); model from `luna-send` `getSystemInfo` asking only for `modelName`, `firmwareVersion`, `sdkVersion` as D0-D1 did (never the serial; verified-on-target c3-a) |
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

- **FAIL (C2, C3, C4, C5): measured, nothing useful can come out.** The daemon closes the PCM, sets `capture_problem` on and `compat_status` to `capture problem`, writes the result to `run/selfcheck.json`, logs one line with the reason and the hint "press Create compatibility report", and keeps running (HA connection, entities, TV control and the kill switch stay up). `microphone_muted` turns on with the reason "capture problem" and `listening` off, so the [listening indicator](../development.md#listening-indicator) stays truthful.
- **Bounded retries, never a crash loop.** The open is retried with backoff 5 s, 10 s, 20 s, 40 s, 80 s (the existing open-retry path, capped). After the fifth consecutive failure the daemon stops retrying on its own. It tries again only on the HA button "Re-run microphone check", on `ctl.sh selfcheck`, on a daemon restart, or after the firmware changed. The supervisor's crash-loop marker is **not** involved: a failed self-check is a state, not an exit. A hung capture thread still ends the process through the existing stall watchdog (exit 70 or 71) and the supervisor's existing limits apply.
- **WARN (C6, C7, a lower C4):** the mic stays open. A quiet room or a loud TV is a normal environment, not a failure. The `selfcheck` text names the reason.
- The result is **sticky across HA reconnects** (the states are resent) and cleared by the next run.
- A self-check on a **verified** model and firmware behaves the same. One policy, no special case.

### 3.4 Firmware-changed marker

`etc/firmware_seen` holds the last firmware string the daemon ran on (one line, no other data). Written at the first start after install and on acknowledgement; read at every start. It is not written automatically when the version differs, so a change stays visible until acknowledged.

## 4. Home Assistant entities

All on the existing satellite device. Object ids are stable ([entity keys are hashes of the object id](../development/esphome-api.md)). All diagnostic, none enabled-by-default beyond this list. The ESPHome API crate already has generic `binary_sensor`, `button`, `number` and `select`; it needs a generic `text_sensor` (ADR 0005 plans it for TV control; this design reuses it, [section 9 S4](#9-implementation-plan)). **The ESPHome native API has no entity attributes**, so model and firmware cannot be attributes (the earlier sketch in [packaging 9.4](packaging.md#94-runtime-behaviour-at-the-release-replaces-the-block) assumed they could): each is its own entity.

| Entity | Type | Object id | Values | Category |
| --- | --- | --- | --- | --- |
| Firmware verified | `binary_sensor` | `firmware_verified` | on = model and firmware are in the verified list; unknown if unreadable | diagnostic |
| TV firmware | `text_sensor` | `tv_firmware` | `03.30.74` | diagnostic |
| TV model | `text_sensor` | `tv_model` | the model name the TV reports | diagnostic |
| Compatibility | `text_sensor` | `compat_status` | `verified`, `not verified`, `firmware changed`, `capture problem`, `unknown`, `checking` | diagnostic |
| Microphone self-check | `text_sensor` | `selfcheck` | short text, for example `passed (peak -54 dBFS)`, `warning: very quiet`, `failed: no audio frames`, `not run (microphone closed)` | diagnostic |
| Capture problem | `binary_sensor` | `capture_problem` | on after a FAIL; device class `problem` | diagnostic |
| Firmware changed | `binary_sensor` | `firmware_changed` | on until acknowledged | diagnostic |
| Report status | `text_sensor` | `report_status` | `idle`, `sent to Home Assistant`, `Home Assistant actions are not allowed: see docs`, `no answer from Home Assistant` | diagnostic |
| Create compatibility report | `button` | `report_compat` | press to build the report | config |
| Re-run microphone check | `button` | `selfcheck_run` | press to rerun [section 3](#3-self-checks) | config |
| Acknowledge firmware change | `button` | `firmware_ack` | writes `etc/firmware_seen`, clears the marker | config |

States are plain short strings (at most 255 characters; HA's state limit, `documented`, home-assistant.io). The list is eleven entities; all but the first four can be hidden with `[safety] self_check = false` or `[report] enabled = false` if a user wants a quieter device.

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
| An ESPHome device can call any HA action (`HomeassistantActionRequest`, api.proto message 35, fields `service`, `data`, `data_template`, `variables`, `is_event`, `call_id`) only when the user enables "Allow the device to perform Home Assistant actions" in the device options. It is off by default; HA warns "Only enable this if you trust the device" | [HA ESPHome integration](https://www.home-assistant.io/integrations/esphome/), [ESPHome API docs](https://esphome.io/components/api/), `crates/esphome-api/proto/api.proto` | documented |
| In HA's handler (`components/esphome/manager.py`, `dev` branch read 2026-11-04) the service string is split at the first dot, `data_template` is rendered with Jinja **on the HA side**, an event is accepted only in the `esphome.` domain and is **not** gated by the option, a service call is gated by the option, and with it off HA creates a repair issue and runs nothing. With a `call_id` HA sends back a success or failure message | source reading | documented (source reading, not run against the owner's HA) |
| `persistent_notification.create` takes `message`, `title`, `notification_id` and is a normal HA action | [HA docs, persistent notification](https://www.home-assistant.io/integrations/persistent_notification/) | documented (not re-fetched in this session; confirm in S5) |
| A state is limited to 255 characters, so a `text_sensor` cannot carry the full report or a long URL | HA core behaviour | documented |
| The satellite does **not** yet send `HomeassistantActionRequest` or handle `SubscribeHomeassistantServicesRequest` (only the proto is vendored) | `crates/esphome-api` read 2026-11-04 | documented (code) |

### 5.2 Options

| | Option | User effort | Needs | Weak points |
| --- | --- | --- | --- | --- |
| a | **Prefilled GitHub issue URL** (the link itself) | open link, log in if needed, tick the privacy box, Submit | a GitHub account; URL under about 2000 characters (the limit is unknown, [5.1](#51-evidence-collected-read-2026-11-04)) | Not a channel by itself: the link must be shown somewhere |
| b | **HA button** "Create compatibility report" to a notification with the link | press the button in HA, tap the link in the notification, then as (a) | the HA option "Allow the device to perform Home Assistant actions", which gives the device the ability to call **any** HA action | The option is off by default and is a privilege increase: the user must trust the device, which here is a TV running our daemon. The daemon would send one fixed action (`persistent_notification.create`) and a test pins that. When it is off the button does nothing in HA; the daemon only learns this from a missing answer |
| b2 | A `text_sensor` "Report link" with a compact URL under 255 characters | copy the text from the entity dialog, paste in a browser | nothing in HA | Plain text, not clickable; only the essentials fit |
| c | **TV status app** "Report compatibility" with a QR code of the URL, and the report text below | open the app on the TV, scan with a phone, then as (a) | a QR generator in the app (about 700 lines of MIT-licensed JavaScript if vendored, for example Project Nayuki's QR Code generator, **not yet license-checked or tried on the TV**); TV browsers are poor, so the phone does the work | A dense QR on a TV screen: keep the URL short enough (about 1000 characters, [7.4](#74-length-budget)); not verified on the TV's browser engine |
| d | **`ctl.sh report`** | run it over SSH, copy the text or open the printed URL | nothing | Technical users only, but it is the base the others call and the fallback that always works |

### 5.3 Recommendation

Build the report once in the daemon (`webos-satellite --report`) and offer it through **b, c and d** with (a) as the target they all show:

1. **For HA users (the main path): b.** Press **Create compatibility report** in HA. A notification appears with the report and one link; tap it; GitHub opens the form already filled; tick "I reviewed the report"; press **Submit new issue**. Two clicks in HA and GitHub plus a login.
2. **Without the HA option (and for people at the TV): c.** Open the app, press **Report compatibility**, scan the QR with a phone. Same form, same link.
3. **Always: d** (`ctl.sh report`), and the app's text view of the same report.
4. **b2 is optional** (stretch step), for HA users who refuse the privilege and do not want to walk to the TV. Decide after the owner's answer on the HA option.

If the HA option is off when the button is pressed, `report_status` says so and names the other two paths; nothing is hidden. The owner decides whether the one-time HA option is acceptable ([question 1 in section 11](#11-questions-for-the-owner)).

**What does not work or is rejected:** a `text_sensor` carrying the URL as the only path (not clickable, 255 limit); posting the issue from the TV (needs a GitHub token on the device, rejected: no credentials on the TV); `homeassistant.event` as the transport (works without the option, but a user would need an automation to turn it into a notification, so it is no easier).

## 6. GitHub side

### 6.1 Issue form

[`.github/ISSUE_TEMPLATE/compatibility-report.yml`](../../.github/ISSUE_TEMPLATE/compatibility-report.yml) (added with this design). Fields:

| id | Type | Prefilled by the link | Notes |
| --- | --- | --- | --- |
| `report` | textarea, `render: text` | yes (the short report, [7.4](#74-length-budget)) | required |
| `details` | textarea | no (the user may paste the full report from the app or `ctl.sh report --full`) | optional |
| `comments` | textarea | no | optional: what did or did not work, how the TV is used (LG hands-free off?) |
| `confirm` | checkboxes | no | required: "I reviewed the report: it has no serial, address or other private data, and I know the issue is public". One tick, chosen over a silent submit because the issue is public |

The form has a top note: what the report contains, what it never contains, that the daemon sends nothing by itself, and that the issue is public. The title is prefilled as `[Compat] <model> fw <firmware>: <verdict>` where the verdict is `works`, `partly works` or `does not work`, computed from the self-check (a positive report uses the same form). The label `compat-report` comes from the form file; it must exist in the repository (a maintainer creates it once; whether a missing label is dropped silently is `hypothesis`). Maintainers add `works`, `broken` or `needs-info` when triaging.

### 6.2 Link format

`https://github.com/<repo>/issues/new?template=compatibility-report.yml&title=<encoded>&report=<encoded>`. No `body`, no `labels`. Percent-encoded UTF-8; the daemon builds it, with a test that decodes it back and compares.

### 6.3 Verified list in the binary and in the docs

- **Data file** `data/verified-models.toml` in the repository, compiled into the daemon (`include_str!`): rows of exact `model` (the string the TV reports) and `firmware`. A model or firmware not in the list is "not verified", never an error. Updating the list ships with a release, so a user does not edit it.
- **Docs** `docs/compatibility/verified-models.md` has two tables: **Verified** (the same rows as the data file, plus webOS, architecture, what passed, date, evidence) and **Reported working (unconfirmed)**. A test (`tools/check_docs.py`, S7) fails if the Verified table and the data file differ.
- **How a row is added from a report.** A maintainer reads the issue (the report is allow-listed text), then:
  - one report with every core check passing and an author the maintainer does not already know: a row in **Reported working (unconfirmed)**, label `works`. It stays `observed-on-other-device` in the project's terms;
  - a second independent report (a different GitHub author) for the same model and firmware, or a maintainer run, moves it to **Verified** and into the data file in the next release;
  - a failing report: a row in a **Known problems** list in the same file, an issue kept open with the label `broken`, and the finding recorded in a per-device record if someone with the TV can run an experiment.
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
- A report is shown to the user (the HA notification, the app, `ctl.sh report`) before it goes anywhere; the issue form shows it again.

### 7.4 Length budget

- **Short report** (in the URL, QR): at most about 600 bytes before encoding, so the whole link stays near 1000 characters (about 1300 with the title in a worst case; the cap is 1500). Priority order if it does not fit: verdict, model, firmware, webOS, app version, architecture, check results, levels, PCM list (shortened to the chosen device), power, TV control.
- **Full report:** no hard limit (app text, `ctl.sh report --full`, the HA notification body); the user may paste it into `details`.
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

- **HA:** the notification title "Compatibility report for the TV satellite"; body: one line of what to do, the link, then the report in a code block; `notification_id` fixed so a new press replaces the old one. The daemon sends `persistent_notification.create` with `data` only (no `data_template`, no variables), a `call_id`, and waits 5 s for the answer; no answer sets `report_status` to the "not allowed or no answer" text.
- **TV app** (`packaging/webos/app`): a "Report compatibility" button; a screen with the QR (large, high contrast), the short URL text for typing as a fallback, and the report text; a "Back" key. The app calls `ctl.sh report --json` through the existing `exec` path.
- **`ctl.sh`:** `ctl.sh report` prints the short report and the link; `--full` prints the full report; `--json` is for the app; `ctl.sh selfcheck` reruns the check; `ctl.sh ack-firmware` writes the marker. When the daemon is running it asks it (a control file the daemon watches, like the existing kill switch), because a second process must not open the PCM; when it is not running the command reports the last saved `run/selfcheck.json` and says it is old.

## 9. Implementation plan

Small steps, each for one agent, in order unless marked parallel. Another agent is editing `crates/` today; steps that touch `daemon.rs`, `config.rs` and `esphome-api` wait until that work is committed. Each step ends with the repository gates: `cargo test`, `cargo clippy` as the project already runs them, `sh -n tools/probe/probe.sh`, `python3 tools/check_docs.py`, and for packaging `packaging/webos/tests/run.sh`.

| Step | Scope | Tests |
| --- | --- | --- |
| **S1** Policy core | `firmware.rs`: `Verdict` becomes a status (verified, not verified, unknown, changed) independent of capture; `[safety] firmware_policy` and `self_check` in `config.rs`; `firmware_seen` read and write; `daemon.rs` gate only in strict mode; `--check-firmware` exit codes kept; `autostart.sh` stops blocking in warn mode; defaults in `packaging/webos/config.default.toml` and `config/example.toml` | table tests for each row of [2.1](#21-behaviour-by-situation); existing `firmware_block...` test adapted to strict; `packaging/webos/tests/run.sh` autostart case on an unverified version |
| **S2** Self-check engine (pure, parallel with S1). **Done 2026-11-04** (library only, not wired) | `crates/compat` (`webos-compat`), no dependencies: `window.rs` (statistics, thresholds as `Thresholds` defaults, the single verdict function `grade_window` for C4 to C7), `pcm.rs` (parsers for `/proc/asound/cards` and `/proc/asound/pcm` text, C2 by card id), `check.rs` (C1 and C8 helpers, grades, fixed reason codes), `engine.rs` (state machine with injected time: window, no-frame fail at 3 s, one first try plus five retries at 5, 10, 20, 40 and 80 s, then stop, rerun). Thresholds are `hypothesis`; open points: below 50 % of the frames is still a WARN (the table defines FAIL only as "none"), and a stuck signal is peak-to-peak of at most 1 LSB | table tests with c3-a numbers (-67.6 dBFS pass, -92.6 peak warn, all-equal samples fail, clipped warn); engine tests with a fake clock (silent warns, constant fails and closes, retries bounded) |
| **S3** Report builder (pure, parallel with S1). **Done 2026-11-04** (library only, not wired; `--report` flags belong to S6) | `crates/compat`: `report.rs` (typed `ReportInput`, short and full text, JSON, truncation by priority), `sanitize.rs` (token sanitizer that drops identifier-looking values whole, fail-closed scan), `link.rs` (prefilled link with `template`, `title` and `report`, encoding, parser). The text format differs from the illustration in [7.5](#75-report-text-schema-v1-illustrative): lines are `key: label=value, ...` and card and device are separate fields. The verified-list lookup is left to S7 (the data file does not exist yet) | the three tests of [7.3](#73-how-the-allow-list-is-enforced) (seeded identifiers in every input, pinned field list, scan rules); link length (about 930 characters for a c3-a sized report, at most 1500 in the worst case) and round trip; positive and negative reports |
| **S4** ESPHome API | generic `text_sensor` (state command, missing state, tests as the others); `SubscribeHomeassistantServicesRequest` handled; `AppCommand::CallHaAction { service, data, call_id }` sends message 35 and routes the answer (message 130) to `AppEvent::ActionResult`; harness scenario in `tools/esphome-fake-ha/fake_ha.py` where the fake HA answers, and one where it does not | `server_io` tests; fake-HA scenario; a test that only the allowed service string can be sent (the crate stays generic, the allow rule lives in the satellite) |
| **S5** Daemon wiring | entities of [section 4](#4-home-assistant-entities); capture thread feeds S2; failure handling of [3.3](#33-what-the-daemon-does-on-a-failure) (close, backoff 5 to 80 s, stop, manual rerun); `microphone_muted` reason "capture problem"; the report button sends the HA action (S3, S4); `report_status` | host end-to-end with a file mic: silent file gives warn, constant file gives fail and `capture_problem`, then button rerun; fake HA sees one `persistent_notification.create` with no other call; the retry count is bounded (a test with a fake clock) |
| **S6** `ctl.sh`, status app | `ctl.sh report`, `selfcheck`, `ack-firmware`; status JSON gets the policy fields; app "Report compatibility" with a vendored QR generator (license check and the third-party notice, [packaging section 8](packaging.md#8-license-decision-and-dependency-audit)); app shows the firmware-changed banner | `packaging/webos/tests/run.sh` cases; `app_smoke.js` for the screen and for the QR payload equal to the link; QR decode check on the host (a decoder in the test only) |
| **S7** GitHub and docs | confirm the issue form (added with this design); create the label; `docs/compatibility/verified-models.md` (tables of [6.3](#63-verified-list-in-the-binary-and-in-the-docs)); README "Verified TVs" section; `data/verified-models.toml` with the rows the owner approves; `tools/check_docs.py` checks the two lists agree and that the issue form parses; CONTRIBUTING note on reports; update [development.md](../development.md) configuration reference, packaging section 3 and 4, and the install UX text that still says "microphone off" | `python3 tools/check_docs.py`; YAML parse test of the form |
| **S8** (optional) b2 | `text_sensor` "Report link" under 255 characters | length test |

## 10. What to verify after deployment

**Silently on c3-a through HA** (c3-a runs the mic today and no sound is involved; each TV run needs the owner's authorization per [AGENTS.md](../../AGENTS.md)):

1. Deploy the build over the running daemon by the ipk update path ([EXP-009](../experiments/EXP-009-ipk-install-update.md)). In HA the new entities appear on the existing device: `tv_firmware` = `03.30.74`, `tv_model`, `compat_status`, `selfcheck`, `firmware_verified`, `capture_problem` off, `firmware_changed` off, `report_status` idle.
2. `selfcheck` shows `passed` and a peak near the c3-a quiet-room reference (about -54 dBFS peak, -67.6 dBFS RMS); record the real numbers and fix the C6 and C7 thresholds (hypothesis until then). Compare `listening` and `microphone_muted` before and after.
3. `ctl.sh report`: read the output against the TV's real serial, MAC and address (looked up privately, not recorded) and confirm none appears; run it with the daemon stopped and running.
4. Press **Create compatibility report** in HA **with the HA option off first**: `report_status` must say so and HA should show the repair issue the source reading predicts. Then, **only with the owner's consent**, turn the option on for the c3-a entry, press again, and check the notification, that it is the only action sent, and that the link opens the form **filled** (`report` prefilled in the textarea, title set). Do not press Submit; close the tab. Repeat with the app's QR from a phone.
5. Firmware-changed path: with the owner's authorization, edit `etc/firmware_seen` to another value, restart the daemon, and see `compat_status` = `firmware changed`, the app banner, the rerun check, then the acknowledge button.
6. Unverified path: a config with an empty verified list and a model string not in the data file (or a test build) shows `not verified` with the mic open.
7. Kill switch while a check is running; stop and start the daemon repeatedly (5 cycles) to see no retry storm.

**On the host (file mic, fake HA):** the failure paths (stuck file, silent file, saturated file, no PCM), the bounded retries, and that no crash loop marker is written.

**c3-b** (second firmware): the same list without the HA option, when the owner agrees; this is the first sample for an unverified firmware in the list.

**Report prefill check (no TV):** open the generated link in a browser and confirm the form is filled. This settles the `hypothesis` rows in [5.1](#51-evidence-collected-read-2026-11-04). If the textarea is not filled, fall back to `body` for the first line and to the copy button.

## 11. Questions for the owner

1. **HA option.** Is it acceptable that the one-click HA path needs "Allow the device to perform Home Assistant actions" on the satellite's entry (off by default, gives the device any action)? If not, only the TV app, `ctl.sh` and (optionally) the 255-character link remain.
2. **Repository.** The link targets `pando85/webos-voice-satellite` (the `origin` remote and the app id); confirm, and the label names `compat-report`, `works`, `broken`, `needs-info`.
3. **GitHub login.** A user must be logged in to GitHub to submit. Acceptable, or should an anonymous channel (a form service) be considered? Not recommended: it would add a third party and a place for private data.
4. **Verified rows.** Which rows go into the first `verified-models.toml`: c3-a `03.30.74` (the mic works, daily use) and c3-b `03.30.73` (81 minutes of capture)? The existing rule says no model is marked supported without the project's own evidence.
5. **Model matching.** Exact model name (for example `OLED42C34LA` versus the other regional variants), or series (`C3`)? This design uses the exact string.
6. **Strict mode default.** `"warn"` is the default as decided; confirm the old installs keep the shipped list as the strict list only when they also set `"strict"`.

## Sources

Read 2026-11-04: [GitHub, creating an issue (URL query)](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue); [GitHub form schema syntax](https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/syntax-for-githubs-form-schema); [HA ESPHome integration](https://www.home-assistant.io/integrations/esphome/); [ESPHome API component](https://esphome.io/components/api/); Home Assistant core `homeassistant/components/esphome/manager.py` (`async_on_service_call`); `crates/esphome-api/proto/api.proto` (messages 35 and 130). Repository: [P3 robustness](../discovery/c3-a/P3-robustness.md), [c3-b compatibility](../compatibility/c3-b.md), [ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md), [ADR 0005](../decisions/0005-tv-control-entities.md), [packaging section 9](packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist).
