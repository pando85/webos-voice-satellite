# Verified TVs

Which LG TV series and firmware the project has run the satellite on itself. **This is not a gate:** the microphone starts on every TV and firmware ([firmware policy and reports](../design/firmware-policy-and-reports.md), [ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md)). The list only decides what the daemon, Home Assistant and the TV app *say* (`compat_status`, `firmware_verified`). Everything not in the **Verified** table is "not verified", which means "nobody has reported or tested it yet", not "does not work".

Scope: one TV model per row, as tested. A finding on another LG model is not confirmation on this one ([AGENTS.md](../../AGENTS.md)). Per-device records: [c3-a](c3-a.md), [c3-b](c3-b.md).

## How matching works (mirror of `data/verified-models.toml`)

The data file [`data/verified-models.toml`](../../data/verified-models.toml) is compiled into the daemon. The match is by **series and firmware** (owner decision 2026-11-04):

| TV model and firmware | Status | `compat_status` |
| --- | --- | --- |
| The model matches the series pattern (for `LG OLED C3 (2023, webOS 8.x)`: `OLED??C3*`, any size or regional variant) **and** the webOS release starts with the series' major number **and** the firmware is listed | verified | `verified` |
| Same series, firmware **not** listed (another build, or another size or model of the series whose firmware nobody reported) | series verified, firmware not yet reported | `series verified (firmware not yet reported)` |
| Another series, another webOS major, or a model or version that cannot be read | not verified or unknown | `not verified` or `unknown` |

`firmware_verified` in Home Assistant is on only for the first row. `webos-satellite --check-firmware` does not read the model: it exits 0 when the firmware is in the built-in list or in `safety.validated_firmware`, 1 otherwise or when the firmware cannot be read. The model name comes from `luna-send` `com.webos.service.tv.systemproperty/getSystemInfo` with the key `modelName` only; the daemon never calls `apiadapter/system/getSystemInfo` (it returns the serial number).

## Verified

One row per firmware. A row is added only for a firmware on which the project itself ran the microphone path (`verified-on-target`, with the experiment). `tools/check_docs.py` fails when the series and firmware columns differ from the data file.

| Series | Model tested | webOS | Firmware | Unit | What passed | Date | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| LG OLED C3 (2023, webOS 8.x) | `OLED42C34LA` | 8.3.0 | `03.30.74` | c3-a | Built-in mic capture `hw:LGSIC,0` (S24_LE, 2 ch, 16 kHz, LG hands-free off), Noise API to Home Assistant, permanent install, daily use; remaining capabilities are NOT TESTED on this unit (see the record) | 2026-11-01 to 2026-11-03 | [c3-a](c3-a.md) |
| LG OLED C3 (2023, webOS 8.x) | `OLED55C36LC` | 8.3.0 | `03.30.73` | c3-b | Mic capture (aarch64 daemon, 81 min, no `MIC BUSY`), capture with the screen off and in `Active Standby`, Assist pipeline, reply playback, app package install and update, TV control setters (EXP-010 Part A); details and failures (standby playback, `Suspend`) in the record | 2026-11-03 to 2026-11-04 | [c3-b](c3-b.md) |

## Reported working (unconfirmed)

Reports from other users that passed every core check. They stay `observed-on-other-device` in the project's terms. None yet.

| Model | webOS | Firmware | Report | Date |
| --- | --- | --- | --- | --- |
| | | | | |

## Known problems

Failing reports and what is known about them. None yet.

| Model | webOS | Firmware | Issue | Finding |
| --- | --- | --- | --- | --- |
| | | | | |

## How a maintainer adds a row from a report

Reports arrive as GitHub issues from the [compatibility report form](../../.github/ISSUE_TEMPLATE/compatibility-report.yml) (label `compat-report`; the maintainer adds `works`, `broken` or `needs-info` while triaging). The report is allow-listed text: model, webOS, firmware, architecture, audio device ids, self-check results, level numbers, power states and TV control results. Read it before acting on it.

1. **One report, every core check passing,** from an author the maintainer does not already know: add a row to **Reported working (unconfirmed)** and label the issue `works`. The series status does not change.
2. **A second independent report** (a different GitHub author) for the same model and firmware, **or a maintainer run** on that model: add a row to **Verified** and the matching `[[series.firmware]]` entry to `data/verified-models.toml` (a new `[[series]]` when the report is for another series, with its `models` glob and `webos` major), in the same change; the daemon picks it up in the next release. Keep the two in step (`python3 tools/check_docs.py`).
3. **A failing report:** a row in **Known problems**, the issue kept open with the label `broken` (or `needs-info` when a number or setting is missing), and the finding recorded in a per-device record if someone with the TV can run an experiment.

Reports never become `verified-on-target` on their own; that label needs the project's own experiment on the exact unit and firmware.
