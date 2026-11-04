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

Update 2026-11-04 (owner): the microphone starts on any firmware, and users report results through a compatibility report (TV app QR code or `ctl.sh report`, then the issue form [`compatibility-report.yml`](../../.github/ISSUE_TEMPLATE/compatibility-report.yml); [design](../design/firmware-policy-and-reports.md), [ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md); implemented in the repository, not yet run on a TV). **The list now exists: [verified-models.md](verified-models.md)** (Verified, Reported working (unconfirmed), Known problems), matched by series plus firmware and mirrored by `data/verified-models.toml`.


The public release will not hard-whitelist firmware in the daemon. Instead this directory holds the **verified list**: one row per tested model and firmware (matched by series) with what passed, the date and the evidence, and only those rows are announced as supported. Users send a sanitized compatibility report (model, webOS, firmware, architecture, audio PCM summary, capture and playback checks; never a serial, MAC, IP or hostname) through a GitHub issue template, and the app and Home Assistant show "verified / series verified / not verified" instead of blocking. Design and release prerequisites: [packaging section 9](../design/packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist), [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing). The daemon's hard firmware gate is gone from the default configuration (opt-in `strict` mode).

Per-device records: [c3-a](c3-a.md) (permanent install 2026-11-03, firmware `03.30.74`), [c3-b](c3-b.md) (live run 2026-11-03, permanent install 2026-11-04 with the microphone busy under LG hands-free, firmware `03.30.73`; no unit is marked supported).
