# ADR 0010: Music playback through the Home Assistant integration

- **Status:** **Accepted for option B (owner, 2026-11-05).** AirPlay (option A) **rejected by the owner**. The owner accepted the plan's defaults D1 to D14 on 2026-11-05. Built and tested on the PC up to the daemon integration (milestones M1 to M5, behind `[music] enabled = false`); nothing run on a TV yet.
- **Scope:** how music reaches the TV speaker under Home Assistant control. Plan, evidence and milestones: [music playback design](../design/music-playback.md).
- **Related:** [ADR 0001](0001-esphome-native-api.md), [ADR 0003](0003-server-side-wake-word-for-mvp.md), [ADR 0007](0007-tv-media-player.md), [requirements](../project/requirements.md) (F8 and the MVP non-goal "writing AirPlay or a music player", lifted for this feature below), [backlog #5](../project/backlog.md#5-music-playback-through-the-satellites-esphome-media_player), [AEC front-end](../design/aec-frontend.md).

## Context

- The satellite's `media_player` plays only assistant replies, announcements and the timer chime (flags 5124); the "TV" media player is control only (ADR 0007). The owner asked on 2026-11-05 whether music playback could be designed through the HA integration.
- Evidence ([design section 2](../design/music-playback.md#2-evidence-base)): HA's ESPHome integration converts a `play_media` URL to the one format a device advertises, through a one-time ffmpeg proxy URL with no Range support; real ESPHome speakers keep a media and an announcement pipeline and duck on the device; the ESPHome API has no seek, queue or metadata; Music Assistant has a fallback provider for Home Assistant media players; Sendspin is MA's native protocol, still in technical preview.
- The echo and false-wake risk of music near the mic exists with every option, because the mic is near-raw and AEC is parked.

## Decision

1. **Option B is accepted (owner, 2026-11-05: "We don't want to use AirPlay. Go with plan B.").** Native ESPHome `media_player` playback in the satellite: HA sends `play_media` with a URL (HA's proxied URL, or, for Music Assistant, MA's own continuous stream, which bypasses the proxy), the satellite does a streaming fetch, `symphonia` decode and a second `pacat` stream, and ducks the music for the wake word, replies, announcements and the timer chime. It is built silently on the PC first, behind `[music] enabled = false` by default.
2. **Option A (the TV's AirPlay receiver with Music Assistant) is rejected as a path for this feature. Reason: owner decision.** [EXP-008](../experiments/EXP-008-airplay-media.md) is not planned. A is kept in the design only as a rejected alternative.
3. **Scope change.** The MVP non-goal "writing AirPlay or a music player" in requirements.md is **lifted for satellite music playback** by this ADR. It stays a non-goal to write AirPlay, a music library, a queue, a playlist, metadata or a UI, and to do gapless or multi-room playback. [requirements.md](../project/requirements.md) keeps the original wording visible and notes the change; F8 now targets B.
4. **Sendspin (C) stays deferred** until it leaves preview; **LG app launching (D) stays with HA's webostv integration**, out of scope.
5. The concrete defaults (one entity, ducking levels, `MEDIA_ANNOUNCE`, behaviour per power state, format, timeouts) are **proposals** in section C of the design, **owner to confirm** (gate G0). Enabling music by default needs the gates in section I of the design (resources, mixing, false wakes, real HA), and the owner's sign-off (G6).

## Consequences

- New `[music]` config section, a second advertised media format (`DEFAULT`, FLAC 48000 Hz stereo), new entity flags only when enabled, no change to the existing entity key or the announcement format.
- Streaming fetch and decode in `webos-playback`; `symphonia` (MPL-2.0, already linked) stays unmodified, so the licence position in [packaging](../design/packaging.md) holds; no new crate is planned.
- The entity offers `BROWSE_MEDIA` and handles `ENQUEUE` explicitly (refused); it does not offer mute.
- Music near the mic depends on ducking and, for barge-in, on AEC; the owner may be asked to un-park AEC if the false-wake gate (G4) fails.
- HA's proxy URLs have no Range and a new request restarts the conversion, so a dropped stream is not resumed: the track ends and MA or HA decides. Music Assistant sends one endless stream and bypasses the proxy; the satellite must handle both.
- The daemon fetches URLs as root from any host, so the plan makes fetch security (redirect limit, no downgrade, loopback, link-local and TV-local targets refused, plaintext sessions refused) and decoder robustness (`panic = "abort"`, fuzzing) preconditions of the daemon integration milestone.
- Running a build on a TV and any sound test need the owner's authorization (AGENTS.md); the plan lists them in section J.
- `AGENTS.md` still says "Existing TV AirPlay receiver / Music Assistant is a candidate for music"; that line is older than this decision, is an owner edit and was not changed here. README and `docs/project/vision.md` were updated. This ADR takes precedence for music.

## Review 2026-11-05

An independent review found the first plan not ready; its findings (Music Assistant bypasses the proxy, proxy slot eviction, `ENQUEUE`, idle and cancel at the socket, decoder panics, fetch security, `BROWSE_MEDIA`, latency budget, EXP-015 privacy, an earlier real-HA smoke) were applied. See [design section L](../design/music-playback.md#l-review-2026-11-05). The decision itself (B accepted, A rejected) is unchanged.

## Alternatives considered

- **A. AirPlay + Music Assistant** (the TV's receiver): no code, but LG decides volume, standby, input and sound policy, and the satellite cannot duck. **Rejected by the owner (2026-11-05).**
- **C. Sendspin client:** synchronised multi-room and native metadata, but a second protocol with clock sync over `pacat`, a spec in flux and a Rust crate that needs `libasound`. Deferred.
- **D. LG apps through webostv:** needs SSAP pairing, fails in standby, not music playback by us. Out of scope.
- **A third "Music" media player entity** instead of extending `media_player`: the fallback if the owner rejects default D1 in the plan.

## Revisit when

- Sendspin reaches a stable spec and the owner runs several Sendspin players.
- A gate in the plan fails: EXP-013 (CPU or memory) makes decoding on the TV impractical, or EXP-015 shows music defeats the wake word without AEC.
- HA's esphome integration changes the proxy or the `supported_formats` handling.
