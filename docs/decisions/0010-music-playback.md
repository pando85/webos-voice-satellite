# ADR 0010: Music playback through the Home Assistant integration

- **Status:** Proposed — owner decision pending. Nothing here is accepted, built or run on a TV.
- **Scope:** how music reaches the TV speaker under Home Assistant control. Design and evidence: [music playback design](../design/music-playback.md).
- **Related:** [ADR 0001](0001-esphome-native-api.md), [ADR 0003](0003-server-side-wake-word-for-mvp.md), [ADR 0007](0007-tv-media-player.md), [requirements](../project/requirements.md) (F8 and the MVP non-goal "writing AirPlay or a music player"), [backlog #5](../project/backlog.md#5-music-via-the-tvs-airplay-2-receiver-and-music-assistant), [EXP-008](../experiments/EXP-008-airplay-media.md), [AEC front-end](../design/aec-frontend.md).

## Context

- The satellite's `media_player` plays only assistant replies, announcements and the timer chime (flags 5124); the "TV" media player is control only (ADR 0007). The owner asked on 2026-11-05 whether music playback could be designed through the HA integration.
- Evidence ([design section 2](../design/music-playback.md#2-evidence-base)): HA's ESPHome integration converts a `play_media` URL to the one format a device advertises, through a one-time ffmpeg proxy URL with no Range support; real ESPHome speakers keep a media and an announcement pipeline and duck on the device; the ESPHome API has no seek, queue or metadata; Music Assistant can drive AirPlay receivers and has a fallback provider for Home Assistant media players; Sendspin is MA's native protocol, still in technical preview. The TV's AirPlay receiver was never exercised (EXP-008 not run).
- The echo and false-wake risk of music near the mic exists with every option, because the mic is near-raw and AEC is parked.

## Proposed decision (not accepted)

1. **Run EXP-008 first** (no code): the TV's AirPlay 2 receiver driven by Music Assistant, to settle requirement F8 for this TV and give a baseline.
2. **Adopt B as the target design** if the owner accepts the scope: native ESPHome `media_player` playback in the satellite (`play_media` with a proxied URL, streaming fetch, `symphonia` FLAC decode, a second `pacat` stream, duck or pause for the wake word, replies, announcements and the alarm), behind `[music] enabled = false` by default, built silently on the PC first.
3. **Do not build Sendspin now**; revisit when it leaves preview. **Leave LG app launching** to HA's webostv integration.
4. The MVP non-goal in requirements.md is **not changed** by this ADR. If the owner accepts B, the non-goal is reworded in a separate edit.

## Consequences (if accepted)

- New config section, a second advertised media format (`DEFAULT`, FLAC 48000 Hz stereo), new entity flags only when enabled, no change to the existing entity key or the announcement format.
- Streaming fetch and decode in `webos-playback`; `symphonia` (MPL-2.0, already linked) stays unmodified, so the licence position in [packaging](../design/packaging.md) holds.
- Music near the mic depends on ducking and, for barge-in, on AEC; the AEC parked status may need to be revisited by the owner.
- One-time proxy URLs mean a dropped stream is not resumed: the track ends and MA or HA decides.
- Sound tests on the TV and running a build on it need the owner's authorization.

## Alternatives considered

- **A only (AirPlay + MA):** no code, but LG decides volume, standby, input and sound policy, and the satellite cannot duck. Kept as step 1 and the fallback.
- **C. Sendspin client:** synchronised multi-room and native metadata, but a second protocol with clock sync over `pacat`, a spec in flux and a Rust crate that needs `libasound`.
- **D. LG apps through webostv:** needs SSAP pairing, fails in standby, not music playback by us.
- **A third "Music" media player entity** instead of extending `media_player`: open question 2 in the design.

## Revisit when

- EXP-008 shows A meets the need (then B may be unnecessary).
- Sendspin reaches a stable spec and the owner runs several Sendspin players.
- M-E4 shows music defeats the wake word without AEC.
- HA's esphome integration changes the proxy or the `supported_formats` handling.
