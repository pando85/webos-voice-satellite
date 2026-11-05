# ADR 0007: The TV as a control-only media player, next to the separate TV entities

- **Status:** Accepted (2026-11-04, owner). **Supersedes decision 3 of [ADR 0005](0005-tv-control-entities.md)** ("separate entities, not a TV `media_player`") in part: the separate entities stay, and a TV `media_player` is added. The rest of ADR 0005 stands.
- **Scope:** how Home Assistant's own voice intents (volume, mute, turn on and off) reach the TV. Design: [TV control section 3.5](../design/tv-control.md#35-the-tv-media-player-adr-0007). Implemented on the PC (fake-HA and fake `luna-send` tests); **run on both TVs and against the owner's HA 2026.10.4 as 0.2.3 on 2026-11-04**: the volume, mute and turn on/off sentences reach the TV by its alias ([results table](../design/tv-control.md#35-the-tv-media-player-adr-0007)).
- **Related:** [ADR 0001](0001-esphome-native-api.md), [ADR 0005](0005-tv-control-entities.md), [P4 sound session on c3-b](../discovery/c3-b/P4-sound-session.md), [ESPHome API crate](../development/esphome-api.md)

## Context

- On c3-b (2026-11-04) HA's Spanish sentences "baja el volumen de la tele del hall" and "silencia la tele del hall" were recognised but HA's built-in volume and mute intents (`HassSetVolume`, `HassSetVolumeRelative`, `HassMediaPlayerMute`) only target `media_player` entities; the `tv_volume` number and the `tv_mute` switch of ADR 0005 cannot be reached by them ([P4](../discovery/c3-b/P4-sound-session.md)). Label: observed on the owner's HA 2026.10.4 (recognition only).
- HA 2026.10.4 `components/media_player/intent.py` (source reading): `HassSetVolume` requires `VOLUME_SET`; `HassSetVolumeRelative` matches `VOLUME_SET` entities and calls `volume_up`/`volume_down` (or a set for an amount), and when several players match without a name it keeps only those in state `playing`; mute and unmute require `VOLUME_MUTE`. Label: documented (source).
- HA 2026.10.4 `components/esphome/media_player.py` (source reading): features come from `feature_flags` (API 1.14 via `feature_flags_compat`); `SELECT_SOURCE` is in `_UNSUPPORTED_FEATURES` (dropped), and the ESPHome API (`api.proto` 2026.10.1 vendored here) has no source list or source command; only `IDLE`, `PLAYING`, `PAUSED`, `OFF` and `ON` states are mapped (`NONE` or `ANNOUNCING` would raise a `KeyError`); a non-finite volume is shown as no volume level; the device class is always `speaker`. Label: documented (source).
- HA's ESPHome assist satellite takes the TTS and announcement format from the first `ANNOUNCEMENT` format of any media player of the device, and announcements go through `VoiceAssistantAnnounceRequest`, not through a media player entity (`components/esphome/assist_satellite.py`, source reading). Label: documented (source).
- The satellite's existing media player (object id `media_player`) is the reply volume and the announcement format carrier; its key and HA entity id must not change.

## Decision

1. **Add a second ESPHome `media_player`, "TV"** (object id `tv_media_player`), on by default (`[tv_control] media_player = true`). The owner adds the Assist aliases in HA ("tele del despacho" on c3-a, "tele del hall" on c3-b).
2. **Control only.** Features: `VOLUME_SET` and `VOLUME_STEP` (with `volume`), `VOLUME_MUTE` (with `mute`), `TURN_ON` (with `power_on`), `TURN_OFF` (with `screen` or `power_off`). Never `PLAY_MEDIA`, `MEDIA_ANNOUNCE`, `BROWSE_MEDIA`, `MEDIA_ENQUEUE`, `PLAY`, `PAUSE` or `STOP`, and no audio format, so HA never sends it TTS, announcements or media (the API crate refuses those flags for an application player and ignores a media URL sent to it anyway).
3. **Same paths as the separate entities:** the same setters, rate limits, capability checks, read-backs and firmware policy. Volume is the share of the TV's `maxVolume` (set: rounded to the nearest TV step; `volume_limit` clamps as for the number); volume up and down are one TV step each (`volumeUp`/`volumeDown`, as the remote and the step buttons); turn on is the `tv_turn_on` button; turn off is the opt-in power-off of ADR 0005 decision 4 when `power_off` is enabled and the always-on select is on "Keep listening" (with all its preconditions; a refusal stays a refusal), otherwise the screen off.
4. **State:** `playing` when the TV is lit and the media service reports `playState` `playing`, `on` when lit otherwise, `off` in `Screen Off` and `Active Standby` (the TV looks off). Volume and mute where they are live (`Active`, `Screen Saver`, `Screen Off`); an unknown volume is sent as NaN (no level). A transition or an unknown power state is held: the media player has no "unknown" once a state was sent. The play state is polled every 10 s while lit (also when the `tv_play_state` sensor is off).
5. **Input selection stays the `tv_input` select.** The ESPHome API cannot carry a source list and HA's ESPHome platform drops `SELECT_SOURCE`.
6. **The separate TV entities stay** (switches, number, selects, buttons, sensors), and **the satellite's media player is unchanged** (object id `media_player`, its key, features and announcement format).

## Consequences

- HA's built-in volume, mute and turn on/off sentences can target the TV by its alias with no custom sentences or scripts. Verified on the owner's HA 2026.10.4 with the Spanish phrasings of P4 (2026-11-04, text).
- Two media players on one device: an unnamed "baja el volumen" in the satellite's area matches both; HA then keeps only the `playing` ones, so it reaches the TV while something plays on it, and fails ("nothing playing") otherwise. Naming the TV ("de la tele") avoids this. Verified on HA 2026.10.4 (2026-11-04): on c3-b the unnamed command reached the TV while it was `playing` (screen saver); on c3-a (`on`, not `playing`) it failed with `no_valid_targets`, nothing changed.
- "Volume down" by voice is one TV step (1 of 60 on c3-b), as with the remote and HA's `webostv` integration; a relative amount or an absolute value uses the set.
- `off` covers `Screen Off`, where an app's sound may still play (LG documents "Screen is turned off and only sound is played"); `playing` is only shown while lit.
- One extra read-only poll (`com.webos.media/getForegroundAppInfo`, 10 s, lit only) by default; getter answered on c3-b, and on c3-a (HDMI 1, an empty `foregroundAppInfo` list, so `on`); in 0.2.3 the cost is inside the run-to-run noise of the daemon's CPU (1.0 to 1.5 % of one core, RSS about 1.7 MB, [c3-a P4](../discovery/c3-a/P4-permanent-install.md#update-to-023-2026-11-04-tv-media-player-wake-session-fix)). The screen saver reports `playing`, so the player shows `playing` there (c3-b).
- With the default `power_off = false`, an HA "turn off the TV" is a screen off: the running app keeps playing sound. The owner's rule from ADR 0005 (no real power-off from HA) is kept.

## Alternatives considered

- **Keep separate entities only and add custom sentences or scripts:** works, but every phrasing and language would need its own sentence, and HA's built-in intents still could not act.
- **Repurpose the satellite's media player as the TV:** would change the meaning of an existing entity (the reply volume) and mix the announcement format carrier with TV state; rejected by the owner.
- **Source selection on the media player:** not possible over the ESPHome API (no source list), and HA's ESPHome platform drops the flag.

## Revisit when

- HA's ESPHome platform or the ESPHome API gains a source list, or a TV device class.
- The verification on the owner's HA shows the intents do not reach the TV, or the two players confuse unnamed commands.
- A TV run shows the play state is not reliable enough for `playing`.
