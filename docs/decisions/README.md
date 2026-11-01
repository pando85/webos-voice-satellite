# Architecture decision records

Record decisions once supported by evidence or by an explicit owner decision. Use `NNNN-title.md`. Format: Status, Context (with links to evidence), Decision, Consequences, Alternatives considered, Revisit when. Do not create ADRs just to restate untested hypotheses.

## Index

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-esphome-native-api.md) | Integrate with Home Assistant through the ESPHome native API | Accepted (2026-11-01, owner) |
| [0002](0002-rust-implementation.md) | Implement the satellite in Rust | Accepted (2026-11-01, owner) |
| [0003](0003-server-side-wake-word-for-mvp.md) | Server-side wake word for the MVP | Accepted (2026-11-01, owner) |
| [0004](0004-mvp-capture-device-and-lg-handsfree-off.md) | MVP capture from hw:LGSIC,0 with LG hands-free off | Accepted (2026-11-01, owner) |

These ADRs record **owner decisions** supported by discovery on c3-a. They do not claim that the chosen route has been verified end to end; open verification work is in the [roadmap](../project/roadmap.md).

## Related documents

- Plan: [roadmap](../project/roadmap.md) and [backlog](../project/backlog.md).
- Evidence: [discovery logs](../discovery/README.md).
- The [architecture overview](../architecture/overview.md) is a **provisional** sketch and is partly superseded by these ADRs (see the notes there).
