# Architecture decision records

Record decisions once supported by evidence or by an explicit owner decision. Use `NNNN-title.md`. Format: Status, Context (with links to evidence), Decision, Consequences, Alternatives considered, Revisit when. Do not create ADRs just to restate untested hypotheses.

## Index

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-esphome-native-api.md) | Integrate with Home Assistant through the ESPHome native API | Accepted (2026-11-01, owner) |
| [0002](0002-rust-implementation.md) | Implement the satellite in Rust | Accepted (2026-11-01, owner) |
| [0003](0003-server-side-wake-word-for-mvp.md) | Server-side wake word for the MVP | Accepted (2026-11-01, owner) |
| [0004](0004-mvp-capture-device-and-lg-handsfree-off.md) | MVP capture from hw:LGSIC,0 with LG hands-free off | Accepted (2026-11-01, owner) |
| [0005](0005-tv-control-entities.md) | TV control from Home Assistant through separate satellite entities | Accepted (2026-11-04, owner); decision 3 amended by 0007 |
| [0006](0006-soft-firmware-policy-and-compatibility-reports.md) | Soft firmware policy, self-checks and one-click compatibility reports | Accepted (2026-11-04, owner); not implemented |
| [0007](0007-tv-media-player.md) | The TV as a control-only media player, next to the separate TV entities | Accepted (2026-11-04, owner); implemented on the PC, not run on a TV |
| [0008](0008-user-initiated-lg-settings.md) | User-initiated changes of two LG settings (Always Ready, LG hands-free) from the TV app | Accepted (2026-11-05, owner); design only, writes `pending EXP-011` |
| [0009](0009-phone-setup-web-server.md) | A phone setup web server on the TV, started from the TV app (QR code, one-time URL, every change approved on the TV) | Accepted (2026-11-05, owner); design only, not implemented, TV network probe pending |
| [0010](0010-music-playback.md) | Music playback through the Home Assistant integration | Accepted for option B (2026-11-05, owner); AirPlay rejected; design only, not implemented, defaults to confirm |
| [0011](0011-aec-on-by-default.md) | Echo cancellation on by default (B + NS), as a user setting | Accepted (2026-11-05, owner); implemented in 0.4.3; benefit not significant in the live A/Bs |
| [0012](0012-config-switches-in-ha.md) | Echo cancellation and music as HA config switches that run `ctl.sh config` | Accepted (2026-11-06, owner); implemented on the host, not run on a TV |
| [0013](0013-remote-mic-button-coexistence.md) | Remote mic button: coexistence with LG's voice assistant (in-memory configd override of the voice key first, SIGSTOP of `voiceconductor` as fallback, restored on stop, crash and uninstall) | Accepted (2026-11-07, owner) for the configd override; SIGSTOP fallback not accepted (only if [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) C1 fails); in 0.5.0; used on c3-a by the owner 2026-11-07 (owner-reported pass, no logs), LG toast "Función no disponible" on every press |
| [0014](0014-assistant-overlay.md) | The on-screen assistant overlay as an `overlay`-window app in its own package (installed by `ctl.sh install`), fed by SAM launch parameters behind a screen gate | Accepted (2026-11-07, owner); **off by default**; window checked on c3-a ([EXP-024](../experiments/EXP-024-assistant-overlay-window.md)); owner's on-TV check passed on c3-a with 0.5.0 (2026-11-07, owner-reported, no logs), so default-on is possible; the default is the owner's decision |
| [0015](0015-silent-mic-key-filter.md) | A silent mic-key filter in LG's compositor through an in-memory override of `com.webos.surfacemanager.keyFilters`, to remove the toast of ADR 0013 | **Proposed (draft, 2026-11-07), not accepted**; untested ([EXP-025](../experiments/EXP-025-mic-button-toast.md) Part B not authorized) |
| [0016](0016-remote-key-grab.md) | Take the Magic Remote's mic key with an evdev grab of the remote's device and re-send every other event into an idle sibling (no toast, no LG assistant); ADR 0013's override kept as the safety net; four-layer dead-man | **Accepted (2026-11-07, owner)**; EXP-026 B2 and B3 passed on c3-a; implementation for 0.6.0, off by default |

These ADRs record **owner decisions** supported by discovery on c3-a (ADR 0005 and 0007 also by checks on c3-b). They do not claim that the chosen route has been verified end to end; open verification work is in the [roadmap](../project/roadmap.md).

## Related documents

- Plan: [roadmap](../project/roadmap.md) and [backlog](../project/backlog.md).
- Evidence: [discovery logs](../discovery/README.md).
- The [architecture overview](../architecture/overview.md) is a **provisional** sketch and is partly superseded by these ADRs (see the notes there).
