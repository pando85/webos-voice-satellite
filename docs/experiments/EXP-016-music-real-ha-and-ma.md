# EXP-016: Real Home Assistant (and Music Assistant) path on c3-a; includes the M2b real-HA check (gate G0b)

**Status:** TARGET_VERIFIED for the direct Home Assistant path; the Music Assistant part is INCONCLUSIVE (Music Assistant is not installed on this Home Assistant)
**Date:** 2026-11-05 (11:02 to 11:44 CET)
**Device alias / model / firmware / webOS / architecture:** c3-a (entity `lg-c3-a`), LG C3 42 inch (OLED42C34LA), firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland, daemon `webos-satellite 0.4.1 (aarch64-unknown-linux-musl)`; Home Assistant **2026.10.4** (Kubernetes pod, ffmpeg 8.1.2, Python 3.14), ESPHome API 1.18 (HA's hello)
**Power and playback state:** `Active`, screen on (the owner's PC display). Silent: satellite volume 0 % for every step in this file (the audible steps are EXP-014).
**Related research / requirements / issue:** [music playback plan](../design/music-playback.md) (sections B and F, milestone M2b and M6, gates G0b and G5, risks R2, R10, R15), [ADR 0010](../decisions/0010-music-playback.md), [EXP-017](EXP-017-music-engine-resources.md), [EXP-014](EXP-014-music-mix-and-ducking.md)
**Risk:** configuration change on the TV (`[music] enabled`, restored); service calls to the satellite's entities only (no other speaker, no HA configuration change)
**Approval needed / obtained:** owner, 2026-11-05 11:00 CET: "you can run tests now" (c3-a only). HA access: a short-lived token from the login flow (the process in the homelab notes), kept in a 0600 file, access tokens last 30 minutes (one refresh was needed); the one refresh token this session created was deleted by its own id over the websocket (the access token returned 401 afterwards); no other token touched.

## Question and falsifiable hypothesis

Does the behaviour of Home Assistant 2026.10.4 and Music Assistant that section B of the plan (the 2026-11-05 review's reading) describes hold on the owner's HA, with the real daemon on the TV as the device: the flags, browsing, the URL sources (HA's ffmpeg proxy, a non-proxy URL), the advertised format, `ENQUEUE`, the proxy's two-conversion limit, pause and stop, the flags pick-up on reconnect, and the Music Assistant flow stream?

## Prerequisites and safe baseline

As in [EXP-017](EXP-017-music-engine-resources.md). Entities before: `media_player.lg_c3_c3_a_voice_satellite` `idle`, volume 0.4, `supported_features` 5124; the "TV" player and all TV entities unchanged throughout. **Music Assistant check:** the HA `/api/config` component list has no `music_assistant` (nor `sendspin`), and no Music Assistant workload runs in the cluster (pods, deployments and stateful sets of all namespaces); the project notes already said "No Music Assistant". Adding it would be an HA configuration change and was not done.

## Commands / steps

Service and websocket calls to the satellite's `media_player` only: `volume_set` 0 first (the stored reply volume followed, 40 to 0 and back at the end), `media_player/browse_media`, `play_media` with media-source ids (`media-source://tts/tts.piper?message=...&language=es_ES`, `media-source://radio_browser/<station>`), `media_pause`, `media_play`, `media_stop`, `play_media` with `enqueue: add`, `play_media` with `announce: true`, `assist_satellite.announce` (with `preannounce` and with `preannounce_media_id`), and `play_media` with a plain URL and `extra: {bypass_proxy: true}` (the path Music Assistant uses, **per the review**) pointed at a stand-in on the PC (`tools/esphome-fake-ha/fake_proxy.py --mode ma-flow`, endless, 44.1 kHz stereo FLAC, chunked, no `Content-Length`). The daemon log and `run/voice.json` were read after each step.

## Expected evidence and failure interpretation

Pass: play, finish, pause, stop and volume work from HA; the proxy behaves as in the fake; the Music Assistant path plays a queue across a track change without the entity going `idle` (or the real behaviour is documented). Inconclusive is allowed for the Music Assistant part. Fail: HA never reaches `play_media` on the entity, or picks a format the satellite cannot decode.

## Observations and redacted evidence links

URLs are never written here: the daemon redacts them to scheme, host and the first path segment (`https://<HA host>/api/...`).

| Check | Result (HA 2026.10.4, c3-a, 0.4.1) |
| --- | --- |
| Flags after enabling and restarting the daemon | `supported_features` **1201669** (= 153093 + `MEDIA_ANNOUNCE`) as predicted; HA's entity went from 5124 to 1201669 on the reconnect 4 s after the restart, and **back to 5124 after the restore restart**: a flag change is picked up by reconnect, no HA reload needed (as the review said) |
| Browsing | `media_player/browse_media` on the satellite entity (no ids) returned HA's media sources (AI generated images, Camera, Image, Image upload, My media, Radio Browser, Text-to-speech) and could be expanded (`media-source://radio_browser/popular` lists stations with `can_play`). The satellite implements nothing for it (the flag makes HA answer). An HA detail: giving an id without a type is rejected (`media_content_type and media_content_id must be provided together`) |
| `play_media` of a TTS media source | `esphome-api: play_media by HA (announce: false)`, `music: play_media https://<HA host>/api/... (source ha_proxy)`, **`track playing flac 48000 Hz 2 ch from ha_proxy`** 297 ms after the command arrived (the proxy used the advertised `DEFAULT` entry: FLAC, 48 kHz, 2 ch), end of the 4 s item: `track ended: ended`, state `playing` then `idle`, `last_end` `ended` |
| Proxy URL shape | `<HA's own base URL>/api/esphome/ffmpeg_proxy/<device id>/<random>.flac` (read in the HA pod: `FFmpegProxyData.async_create_proxy_url`); over **https on the owner's external HA URL, the certificate validated by the daemon** (a LAN address, resolved by the guard without trouble) |
| Radio station (endless) through the proxy | same format, played for 5 minutes (EXP-017); `media_pause` and `media_play` worked (HA state `paused`, `playing` within the same second; `voice.json` `state` followed) and a 60 s pause resumed on the same connection; a **10 minute pause ended at exactly 600 s with `ended: pause_timeout`** and `idle` (D8); `media_stop` gave `idle` at once and no `pacat` remained |
| `ENQUEUE` | **HA does send `ENQUEUE` with a URL for `enqueue: add` although `MEDIA_ENQUEUE` is not offered** (the review's claim holds): `enqueue refused (no queue); the URL is handed up to be released`, then `https://<HA host>/api/... opened and closed without playing (frees HA's proxy slot)` 23 ms later; the music went on and the state stayed `playing` |
| `announce: true` over music | plays on the reply path (`music: announcement ... over the music (play_media, announce)`), the music state stays `playing`; `assist_satellite.announce` with a message plays the same way (`announcement ... start_conversation=false`, `playback done`), music untouched |
| **Proxy limit of 2 conversions per device** | confirmed on the real HA: with the radio conversation running, **two `play_media announce:true` calls 28 ms apart** (three conversions) made HA kill the oldest; the first announcement was replaced by the second (`player: playback 3 stopped`) and the radio stream ended **12.3 s later** (`track 3 ended: ended`, the data already in the satellite's rings and the kernel's TCP buffer was played first), no error, no retry, `idle` when the second announcement finished. HA source (read in the pod): `_MAX_CONVERSIONS_PER_DEVICE = 2`, the oldest unfinished conversion is killed **when the third URL is created** |
| `assist_satellite.announce` with `preannounce` or `preannounce_media_id` while the radio played | **the review's "ends HA-proxy music" was not reproduced** for this call: the radio kept playing, and the daemon logged one announcement URL per call. Why (does HA make no proxy URL for the pre-announcement, or does the satellite not play it) was not investigated |
| `bypass_proxy` | HA 2026.10.4 passed a plain `http://<PC LAN address>:<port>/...` URL through **unchanged** (`source external`); the daemon played it: **`flac 44100 Hz 2 ch from external`**, the entity stayed `playing`, an endless chunked body with no `Content-Length`, pause 60 s and resume worked, only `media_stop` ended it. This is the Music Assistant transport as the review described it, **but the server was a stand-in, not Music Assistant** |
| Other formats from a non-proxy URL | WAV 44.1 kHz and MP3 (a 128 kbit/s file) were decoded (EXP-017). An `mp3` stream from the stand-in produced a 0-byte body: its ffmpeg call adds `-sample_fmt s16`, which **`libmp3lame` rejects (checked on the PC's ffmpeg 9.0.2 and on HA's own ffmpeg 8.1.2: "Specified sample format s16 is not supported by the libmp3lame encoder")**: risk R15 is **confirmed**, HA's proxy cannot make MP3 with width 2 and the plan is right not to advertise it. The daemon logged `ERROR probe reached EOF at 0 bytes` and `track ended: error (decode error ...); not retried` |
| Music Assistant | **INCONCLUSIVE**: not installed. Not tested: the player appearing, the format and rate MA picks, its stream URL host and headers, track changes inside its flow stream, its reaction to the `ENQUEUE` refusal and the missing mute, a format change needing a provider reload |
| Volume sentences while music plays | **NOT TESTED**: the TV's own volume entity is exposed to Assist (owner's earlier setup), so a sentence could change the TV volume, which this run must not do. HA's volume service on the satellite's player worked (`volume_set` 0, 0.25, 0.35, 0.4; the daemon logged each as `reply volume ... by HA`) |
| Safety | the first `play_media` right after enabling used the 0.4.1 guard with the older bind probe for the own-address rule: no false refusal on c3-a (`ip_nonlocal_bind` is 0 there) |

## Conclusion and confidence

**G0b (real-HA smoke): PASS for Home Assistant 2026.10.4, with the Music Assistant items open.** Everything the plan's section B says about HA held on the real HA with the real daemon: the flag sums, browsing, proxy URL source and chosen format, `ENQUEUE` arriving despite the missing flag, the two-conversion limit with kill-the-oldest, a flags change picked up by reconnect, pause and stop, and a non-proxy URL passing through with `bypass_proxy`. One reading was **not** confirmed (the preannounce case). **G5 (real HA on the TV): PASS for the direct HA path; the Music Assistant result is "inconclusive, not installed"**, so nothing is claimed about MA's real stream (headers, track changes, formats). Confidence high for the HA path (every call tried once or twice, all consistent), low for anything MA.

## Rollback and cleanup

`[music] enabled` removed (original `config.toml` restored byte for byte, md5 equal), daemon restarted, HA entity back to 5124, volume 0.4, reply volume 40; `voice.json` `music: {"enabled": false}`; nothing of ours left in the TV's `/tmp`; the stand-in servers on the PC stopped; HA refresh token deleted by id; no HA configuration change.

## Follow-ups / unresolved questions

- Install Music Assistant (owner decision; it is an HA configuration change) or point an existing one at the player, and run the Music Assistant rows above. Until then the MA claims in the plan stay review readings.
- Why `assist_satellite.announce` with a pre-announcement did not use up a third proxy slot.
- The volume-sentence question needs a safe way to check which entity answers (read-only intent matching) without changing the TV volume.
