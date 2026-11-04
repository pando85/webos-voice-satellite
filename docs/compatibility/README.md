# Device compatibility and evidence

No model or firmware is currently verified for public announcement. **Do not mark LG C3 supported based on other LG TVs.**

Each result must identify:
- Alias (not IP/serial/MAC), exact model, webOS version, firmware, kernel and userland architecture.
- Power state (normal, screen off, Always Ready, standby) and media source (HDMI, web app, tuner, AirPlay).
- Capture/playback endpoint and permission behavior.
- Experiment IDs and redacted evidence, date, pass/fail/inconclusive.
- Known regressions and recovery method.

Use [DEVICE-TEMPLATE.md](DEVICE-TEMPLATE.md). Store private raw output outside Git.

## Verified TVs list (release plan)

Update 2026-11-04 (owner): the microphone is to start on any firmware, and users report results through a one-click compatibility report ([design](../design/firmware-policy-and-reports.md), [ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md); not implemented yet, and the issue form [`compatibility-report.yml`](../../.github/ISSUE_TEMPLATE/compatibility-report.yml) is its target). The list will hold two tables, Verified and Reported working (unconfirmed), as described there.


The public release will not hard-whitelist firmware in the daemon. Instead this directory will hold the **verified list**: one row per exact model and firmware with what passed, the date and the evidence, and only those rows are announced as supported. Users will send a sanitized compatibility report (model, webOS, firmware, architecture, audio PCM summary, capture and playback checks; never a serial, MAC, IP or hostname) through a GitHub issue template, and the app and Home Assistant will show "verified / not verified" instead of blocking. Design and release prerequisites: [packaging section 9](../design/packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist), [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing). Not implemented yet; the daemon's firmware gate is unchanged until the release.

Per-device records: [c3-a](c3-a.md) (permanent install 2026-11-03, firmware `03.30.74`), [c3-b](c3-b.md) (live run 2026-11-03, firmware `03.30.73`; no unit is marked supported).
