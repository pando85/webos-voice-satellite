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
| [0013](0013-remote-mic-button-coexistence.md) | Remote mic button: coexistence with LG's voice assistant (in-memory configd override of the voice key first, SIGSTOP of `voiceconductor` as fallback, restored on stop, crash and uninstall) | **Proposed** (2026-11-07); not accepted, not implemented; [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md) not run |

These ADRs record **owner decisions** supported by discovery on c3-a (ADR 0005 and 0007 also by checks on c3-b). They do not claim that the chosen route has been verified end to end; open verification work is in the [roadmap](../project/roadmap.md).

## Related documents

- Plan: [roadmap](../project/roadmap.md) and [backlog](../project/backlog.md).
- Evidence: [discovery logs](../discovery/README.md).
- The [architecture overview](../architecture/overview.md) is a **provisional** sketch and is partly superseded by these ADRs (see the notes there).
