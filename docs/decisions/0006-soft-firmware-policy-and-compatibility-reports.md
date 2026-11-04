# ADR 0006: Soft firmware policy, self-checks and one-click compatibility reports

- **Status:** Accepted (2026-11-04, owner), amended the same day (no Home Assistant actions path; verified list matched by series plus firmware). Implemented in the repository (host tests only); **not yet deployed to or verified on a TV**. The design and the step plan are in [firmware policy and reports](../design/firmware-policy-and-reports.md).

## Context

- The daemon keeps the microphone off on any firmware that is not in `safety.validated_firmware` (c3-a `03.30.74`); on the TV this was verified for a matching and a non-matching list ([P3 robustness](../discovery/c3-a/P3-robustness.md#firmware-guard)).
- The gate guards against an unseen audio stack after a silent LG update. No failure of the capture path on another firmware has been observed: c3-b (firmware `03.30.73`) captured for 81 minutes ([c3-b compatibility](../compatibility/c3-b.md)). The only kernel fault seen came from reading ASoC DAPM sysfs nodes in a discovery probe, which the daemon never does.
- Owner decision of 2026-11-03 (public release without a hard whitelist, verified-models list) kept the gate until the release. Owner decision of 2026-11-04: "make the mic work. Why do we stop it? It is better to start it, and if it doesn't work we have to have an easy way for users to report that it doesn't work for that firmware version (like creating an issue in GitHub through one button click or something easy)". TV control is also not gated ([ADR 0005](0005-tv-control-entities.md)).

## Decision

1. **No hard firmware gate by default.** The microphone starts on any firmware. `[safety] firmware_policy = "warn"` is the default; `"strict"` keeps the old gate for owners who want it.
2. **Visible status instead of a block:** a log warning, HA diagnostic entities (`firmware_verified`, `tv_firmware`, `tv_model`, `compat_status`, `firmware_changed`), a status-app banner, and a firmware-changed marker kept until acknowledged.
3. **Measured self-checks** on the normal capture stream (PCM found by card id and capabilities, open, frames, stuck, silent, saturated levels over about 10 seconds); no ASoC sysfs or debugfs reads, no recording, no extra PCM open. A measured failure closes the mic with bounded retries and sets `capture_problem`; a warning leaves it open. The existing kill switch stays.
4. **Compatibility report through the TV, not through Home Assistant.** The report is built once in the daemon from an allow-list and shown by the TV status app ("Report compatibility": a QR code of the prefilled GitHub issue link plus the report text) and by `ctl.sh report`. Home Assistant gets diagnostic entities only (status, self-check, `capture_problem`, `firmware_changed`, buttons to re-run the check and acknowledge a firmware change). The issue is created on GitHub with a form template; the user logs in and presses Submit. Nothing is sent by the daemon on its own. The Home Assistant action route (a button that creates a notification, which needs the device option "Allow the device to perform Home Assistant actions") was **rejected by the owner**.
5. **Verified list by series and firmware:** the series (a model glob plus the webOS major) and the tested firmware, in a data file compiled into the daemon and mirrored in `docs/compatibility/verified-models.md`. The same series with an unlisted firmware is "series verified, firmware not yet reported". A report can add a row to "Reported working (unconfirmed)"; a second independent report or a maintainer run moves it to "Verified".

## Consequences

- A kernel fault on an unknown audio driver is **not** prevented by user-space checks. The risk is accepted by the owner and stays documented; the strict mode, the kill switch and the status make it visible and reversible.
- The daemon needs generic `text_sensor` entities (added for [TV control](0005-tv-control-entities.md)). It sends no Home Assistant action and gets no extra privilege in Home Assistant.
- `autostart.sh`, `ctl.sh status`, the status app and the shipped config defaults must stop treating an unlisted firmware as "do not start".
- A public GitHub issue form, a label set and a maintainer routine for the verified list are new project obligations.
- Report content is allow-listed and tested; a report is never `verified-on-target` by itself.

## Alternatives considered

- **Keep the hard gate until the release (packaging section 9, 2026-11-03):** rejected by the owner on 2026-11-04.
- **Start the mic only after an "I understand" confirmation (install UX question 5):** rejected: a blocking prompt again.
- **Only a "not verified" label:** rejected: it drops the cheap measurable checks.
- **Post the issue from the TV or HA with a token:** rejected: credentials on the TV, no benefit over a prefilled link.
- **HA button that creates a notification with the link (device option "Allow the device to perform Home Assistant actions"):** rejected by the owner (2026-11-04): it gives the TV the ability to call any HA action; the QR code is preferred.
- **Exact model string in the verified list:** replaced by series plus firmware (owner, 2026-11-04), so other sizes and regional variants of a tested series are not all "not verified".
- **`text_sensor` with the whole report or URL:** rejected: HA states are limited to 255 characters and are not clickable.

## Revisit when

- The first reports arrive from other models or firmwares.
- A report shows a failure that a check should have caught, or a check that fails on a working TV (thresholds are hypotheses until run on c3-a and c3-b).
- A kernel fault is seen on a capture path (then the default may need to change).
