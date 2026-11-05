# Music playback through the Home Assistant integration

- **Status:** design proposal, **nothing built, nothing run on a TV**. The owner decision is open ([ADR 0010](../decisions/0010-music-playback.md), Proposed). Written 2026-11-05.
- **Owner question (2026-11-05):** "Could we design music playback through the HA integration?"
- **Today:** the satellite's `media_player` (object id `media_player`, feature flags 5124 = `STOP` + `VOLUME_STEP` + `VOLUME_SET`) plays only assistant replies, announcements and the timer chime, through `pacat` to PulseAudio and the TV speaker or ARC. The "TV" media player ([ADR 0007](../decisions/0007-tv-media-player.md), flags 1420) is control only. [requirements.md](../project/requirements.md) lists "writing AirPlay or a music player" as an MVP non-goal; [backlog #5](../project/backlog.md#5-music-via-the-tvs-airplay-2-receiver-and-music-assistant) plans the TV's own AirPlay receiver plus Music Assistant ([EXP-008](../experiments/EXP-008-airplay-media.md), not run; requirement F8 pending).
- **Related:** [AEC front-end](aec-frontend.md), [always-on](always-on.md), [ESPHome API crate](../development/esphome-api.md), [research 06 media](../research/06-media.md), [AGENTS.md](../../AGENTS.md).

Labels follow AGENTS.md: **documented** (vendor docs or source read), **observed-on-other-device**, **hypothesis**, **verified-on-target** (c3-a, LG C3, webOS TV 8.3.0, firmware 03.30.74, or c3-b where named), **verified-on-host** (unit tests or PC runs). "Source reading" means the file was read on 2026-11-05 from the `dev` branch on GitHub (not the exact HA 2026.10.4 tag unless said), so line-level details can drift.

## 1. Short answer

Yes. There are two realistic routes, and they can coexist:

- **A. Use what the TV already has** (AirPlay 2 receiver driven by Music Assistant). No satellite code. Whether it works on this TV is unknown ([EXP-008](../experiments/EXP-008-airplay-media.md)); several of the things we would want (volume, standby, ducking) are decided by LG, not by us.
- **B. Play music inside the satellite** through the same ESPHome `media_player` entity that Home Assistant and Music Assistant already know how to drive (`play_media` with a URL). HA's ESPHome integration converts any source to the one format the device advertises with its ffmpeg proxy, so the satellite needs a **streaming** fetch plus the decoders it already links (FLAC, MP3, WAV through `symphonia`), and a second `pacat` stream. This is the route that lets the satellite **duck or pause music for the wake word, replies, announcements and timers**.

Recommendation ([section 8](#8-recommendation-and-staged-plan)): run the no-code experiment A first (it is cheap and may be enough), and **plan B as the target** for owners who want music that cooperates with voice. Do not start with Sendspin (C) or webostv app launching (D).

## 2. Evidence base

| Fact | Label | Source |
| --- | --- | --- |
| The ESPHome API `MediaPlayerCommandRequest` carries `command`, `volume`, `media_url`, `announcement`. Commands: PLAY, PAUSE, STOP, MUTE, UNMUTE, TOGGLE, VOLUME_UP, VOLUME_DOWN, ENQUEUE, REPEAT_ONE, REPEAT_OFF, CLEAR_PLAYLIST, TURN_ON, TURN_OFF. **No seek, no next or previous.** State is only state, volume, muted: **no position, duration or metadata**. | documented (`crates/esphome-api/proto/api.proto`, ESPHome 2026.10.1) | |
| `ListEntitiesMediaPlayerResponse.supported_formats` is a list of `{format, sample_rate, num_channels, sample_bytes, purpose}` with `purpose` `DEFAULT` or `ANNOUNCEMENT`. | documented (same proto) | |
| HA `esphome/media_player.py` `async_play_media`: resolves `media-source://` ids to URLs; passes `announce` as the `announcement` field; accepts enqueue mode `ADD` only (`NEXT` and `PLAY` raise `enqueue_mode_not_supported`); **when the device lists any `supported_formats` and the id is a URL, the URL is replaced by an HA proxy URL** unless `media_extra.bypass_proxy` is set. The format is the first `DEFAULT`-purpose entry, or the first `ANNOUNCEMENT` entry when `announce` is true. | documented (source reading) | [media_player.py](https://github.com/home-assistant/core/blob/dev/homeassistant/components/esphome/media_player.py) |
| HA `esphome/ffmpeg_proxy.py`: URL `/api/esphome/ffmpeg_proxy/{device_id}/{random}.{format}`; **no auth beyond the random id; one-time URL, no Range, no Content-Length, plain 200 chunked body in 2048-byte chunks; a second request for the same URL kills the running ffmpeg and starts again from the beginning; at most 2 conversions per device; ffmpeg `-i URL -f FORMAT -ar RATE -ac CH -sample_fmt s16 -map_metadata -1 -vn pipe:`**. No volume or loudness filter. | documented (source reading) | [ffmpeg_proxy.py](https://github.com/home-assistant/core/blob/dev/homeassistant/components/esphome/ffmpeg_proxy.py) |
| Real ESPHome speaker media player: two pipelines (`announcement_pipeline`, `media_pipeline`), each to its own speaker, combined by a mixer speaker; **announcements do not pause the media pipeline** ("Announcements do not follow the media player's pause state"); the state `ANNOUNCING` overrides other states while the announcement pipeline runs; ducking is the mixer action `apply_ducking` (`decibel_reduction`, `duration`; the docs' alarm example uses 20 dB, restored over 2 s); native decoding of FLAC, MP3, OPUS and WAV; default transcode format FLAC; per-pipeline buffer default 1,000,000 bytes; OPUS needs S3 plus PSRAM. | documented | [ESPHome docs](https://esphome.io/components/media_player/speaker/), [speaker_media_player.cpp](https://github.com/esphome/esphome/blob/dev/esphome/components/speaker/media_player/speaker_media_player.cpp) |
| HA 2026.10.4 maps only IDLE, PLAYING, PAUSED, OFF, ON from the ESPHome state (`ANNOUNCING` and `NONE` would raise a `KeyError`); a non-finite volume shows as no level; `SELECT_SOURCE` is dropped. The satellite already reports `Announcing` as `playing`. | documented (source reading, [ADR 0007](../decisions/0007-tv-media-player.md); `crates/esphome-api/src/entities.rs`) | |
| HA's ESPHome assist satellite takes the TTS format from the first `ANNOUNCEMENT` format of any media player of the device. A `DEFAULT` format added for music does not change that. | documented (source reading, ADR 0007) | |
| Music Assistant has an "AirPlay" provider: AirPlay 1 (RAOP) and AirPlay 2, chosen per device; AirPlay 2 has no password pairing and no PTP timing yet and "takes longer to connect than RAOP"; per-player volume control can be set to None; a sync delay of up to 500 ms; remote control and wake-before-play are for **Apple TVs** only; "not all AirPlay 2 devices are made equal". | documented | [MA AirPlay](https://www.music-assistant.io/player-support/airplay/), [MA 2.7 blog](https://www.home-assistant.io/blog/2025/12/17/music-assistant-2-7) |
| LG TVs act as AirPlay 2 endpoints (Roon users report some models; one LG OLED reported as not visible in Roon). **No report found of Music Assistant playing to an LG webOS TV.** | observed-on-other-device (search snippets, not read in full) | [Roon thread](https://community.roonlabs.com/t/airplay-2-with-lg-web-os-tv/276470) |
| The TV runs an AirPlay app (`LunaExecutable`, about 42 MB RSS) and an `airplay` service; an LG responder owns mDNS 5353 on c3-a. Nothing else is known about the receiver's behaviour. | verified-on-target (process list only) | [D0-D1](../discovery/c3-a/D0-D1-runtime.md), [D2](../discovery/c3-a/D2-audio-stack.md) |
| Music Assistant lists "Home Assistant Media Players" as a provider (a fallback for devices without a native provider; "if there is a MA player available and a HA integration then you should always choose the MA player"). The page says nothing on URL streaming, formats or ESPHome. How MA streams to such a player (an MA stream URL passed to `media_player.play_media`) is **widely reported for Voice PE-class devices but was not checked here.** | documented (provider list), hypothesis (mechanism) | [MA players](https://www.music-assistant.io/player-support/) |
| Sendspin: open protocol of the Open Home Foundation, built into Music Assistant, "technical preview" in MA and "Release Candidate 1" on its site; WebSocket transport (clients advertise `_sendspin._tcp` on port 8928 or connect to `ws://<MA>:8927/sendspin`), JSON control plus binary audio chunks with play-at timestamps, a Kalman clock-sync filter, Noise `KKpsk2` encryption (unencrypted "legacy clients" still allowed by default, temporary), codecs FLAC and Opus in the web player. Reference client and server Apache-2.0; the spec is under the Community Specification License 1.0. Ships in MA, ESPHome, LedFx and HA Voice PE. Official SDKs: C++, JavaScript, Python. A community Rust crate `sendspin` 0.3.4 (MIT OR Apache-2.0, depends on `tokio-tungstenite` and `cpal`) exists. | documented (web pages, partly read) | [MA Sendspin](https://beta.music-assistant.io/player-support/sendspin), [spec](https://github.com/Sendspin/spec), [sendspin-audio.com](https://www.sendspin-audio.com), [crate](https://docs.rs/crate/sendspin/0.3.4/source/src/protocol/client_builder.rs) |
| What exists in the repository: `webos-playback` has `fetch` (whole body into memory, **10 MiB cap, 20 s overall deadline**, HTTP and HTTPS, bundled roots or a `ca_file`, no validation switch), `decode` (**whole file in memory**, `symphonia` 0.6 with mp3, flac, wav, pcm, ogg, vorbis; capped at about 5 minutes of 48 kHz stereo), `resample` (whole-buffer windowed sinc), sinks (`PacatSink`, WAV file, null). The satellite's `Player` does **one playback at a time**: streamed TTS written to `pacat` at its native rate and mono, and URL media (TTS URL mode, announcements, with an optional preannounce) through `play_url` (fetch whole, decode, resample, `PacatSink`). The announcement path therefore **does fetch and play URLs**, but only short, whole-file ones. | verified-on-host (code read, tests exist) | `crates/playback/src/*.rs`, `crates/satellite/src/player.rs` |
| The satellite's media player advertises one `ANNOUNCEMENT` format (`flac`, 22050 Hz, mono, 2 bytes), has no `PLAY_MEDIA`, `PAUSE`, `PLAY` or `MEDIA_ANNOUNCE`; its `STOP` stops a ringing timer alarm. A media URL sent to an application-owned player is ignored. | verified-on-host (code read) | `crates/esphome-api/src/entities.rs`, `app.rs` |
| A `pacat` tone and the HDMI program audio are both present in the TV's output taps (so they mix in the TV's audio path); SoundEngineCapture (`hw:LGSIC,3`) is the candidate AEC reference and carried both. | verified-on-target (c3-a) | [P2 session 1](../discovery/c3-a/P2-session1-audible-and-echo.md) |
| `Screen Off`: a `pacat` tone is heard (c3-b). `Active Standby` (the remote's power key with Always Ready on): `pacat` runs but the tone is silent (c3-b). `Suspend`: nothing runs. | verified-on-target (c3-b) | [always-on](always-on.md) |
| `sink.gain = 0` makes every byte written to `pacat` zero (the satellite's silent test method; `pacat --volume` is ignored on c3-a). | verified-on-target (c3-a) | `crates/satellite/src/player.rs` header, [P1 playback](../discovery/c3-a/P1-playback-silent-validation.md) |
| Daemon RSS about 1.7 MB (0.2.3, c3-a, idle in the wake chain); the watchdog ceiling is `limits.max_rss_mb = 64`. The AEC stage costs about 1.0 ms per 10 ms frame (armv7 build, 11 % of one core) or 0.21 ms (aarch64 build). | verified-on-target | [ADR 0007](../decisions/0007-tv-media-player.md), [AEC section 3.10](aec-frontend.md) |

## 3. Options

### 3.A The TV's AirPlay 2 receiver, driven by Music Assistant

```text
 Music Assistant server (HA add-on or container)
   |  streams PCM/ALAC over AirPlay 2 (RAOP or AirPlay 2, per device)
   v
 LG webOS TV: built-in AirPlay app/service (LunaExecutable, "airplay")
   |  LG's own audio path (TV master volume, input and policy decided by LG)
   v
 TV speaker / ARC

 HA  <--ESPHome API-->  satellite (unchanged)
   media_player "TV" (ADR 0007): volume, mute, turn on/off of the TV
   media_player "Speaker": replies only
```

- **Code:** none in the satellite. Configuration in Music Assistant only. (hypothesis: MA discovers the TV through its `_airplay._tcp` mDNS advertisement; not seen on c3-a, since only the process list is known.)
- **How the TV media player still helps:** HA's built-in volume and mute sentences reach the TV by its alias ([ADR 0007](../decisions/0007-tv-media-player.md), verified on the owner's HA). If MA's AirPlay player has its volume control set to **None** (documented MA option), the TV's master volume stays with the TV media player and the remote, and "turn up the TV" works as today. If MA sends AirPlay volume, the TV may map it to its master volume (hypothesis; EXP-008 must check, because it would fight the `volume_limit` of the TV entities).
- **Standby and wake:** MA's wake-before-play is documented for Apple TVs only. Whether an AirPlay request wakes an LG TV from `Active Standby` or `Suspend`, and whether the receiver answers there, is unknown. HA's webostv "turn on" does not work in standby either ([always-on](always-on.md)); the satellite's `tv_turn_on` button exists but is a separate step.
- **Voice interplay:** none. The satellite does not know that AirPlay audio is playing (it could infer it from the foreground app, hypothesis), cannot duck it, and the wake word runs on a mic that hears the music (section 6). The AirPlay audio goes through the TV's sound engine, so SoundEngineCapture would carry it as a reference for AEC (hypothesis; it carried HDMI program audio and a `pacat` tone).
- **What EXP-008 must check** (extend the current text, all on one TV at a time, results scoped to model, firmware and power state):
  1. Read-only first: does the TV advertise `_airplay._tcp` or `_raop._tcp` (mDNS browse from the PC), with which `features` and `model` TXT keys; does MA list it as a player (no sound needed).
  2. Pairing or approval prompt on the TV (screen content, remote needed or not).
  3. Playback of a short, quiet, owner-chosen track: start latency, glitches, TV input switching or an on-screen AirPlay overlay, effect on a running HDMI or app source (does the music replace or mix with it).
  4. Volume: MA volume None versus sending volume; effect on the TV master volume; mute.
  5. Pause, resume, stop, track change (gapless or a gap), seek.
  6. Power states: `Active`, `Screen Saver`, `Screen Off`, `Active Standby` (Always Ready on), `Suspend`: is it reachable, does it wake the TV, is it audible (compare the `pacat` result in [always-on](always-on.md): silent in `Active Standby`).
  7. Coexistence with the satellite: wake word during music, a spoken reply and the timer chime during music (does the reply sound over the music, does AirPlay audio pause), mDNS port 5353 interplay.
  8. CPU and memory of the AirPlay process while playing (`ps`, read-only).
- **Pros:** zero code and zero binary growth; works with whatever the TV can decode; MA owns queue, metadata, gapless and sync. **Cons:** all control of the TV's behaviour is LG's; likely no standby support; no ducking; unknown MA compatibility with LG's receiver; MA's AirPlay 2 support is young.

### 3.B Native ESPHome media player playback in the satellite

```text
 Music Assistant / HA UI / HA scripts
   |  media_player.play_media(entity = satellite "Speaker", media_id = URL or media-source)
   v
 Home Assistant, esphome integration (async_play_media)
   |  resolves media-source, then (formats advertised) creates a one-time ffmpeg proxy URL
   |  http(s)://HA/api/esphome/ffmpeg_proxy/<device>/<random>.flac   (FLAC 48000 Hz 2 ch s16)
   |  sends MediaPlayerCommandRequest{command, media_url, announcement, volume}
   v
 ESPHome API (existing connection, encrypted) ----> satellite daemon
   |   music slot:  streaming fetch -> bounded buffer -> symphonia (FLAC) -> gain/duck ramps -> pacat #2
   |   reply slot:  TTS stream / URL / announcement / alarm chime (as today)          -> pacat #1
   |   voice events (WakeWordEnd, RunEnd, TtsEnd, announce, alarm) drive ducking and resume
   v
 PulseAudio mixes the streams -> TV speaker / ARC

 State back to HA: MediaPlayerStateResponse {idle | playing | paused, volume, muted}
 Music Assistant (if it wraps this player) keeps queue, metadata, next-track decisions
```

**How real ESPHome devices work, and what we copy** (documented, section 2): one entity with two pipelines (`media`, `announcement`), HA sends `play_media` with a proxied URL in the one advertised format, `announcement=true` plays on the announcement pipeline over the media, ducking is done on the device, and the entity reports a single state.

**What exists in the repository.** Fetch, decode, resample and the `pacat` sink exist and are tested, but for **short whole files** (10 MiB, 20 s, decode into memory, 5-minute cap), one playback at a time. The ESPHome crate parses `MediaPlayerCommandRequest` and already carries `media_url` and `announcement` fields in the proto; the application player ignores them by design. **Missing for music:**

- a **streaming** fetch (the proxy stream has no length and lasts as long as the track) and a **streaming** decode with a bounded buffer and an idle timeout rather than a total deadline;
- a **second playback slot** that can run while a reply, announcement or the alarm plays (the `Player` has one `current` playback);
- the entity: `PLAY_MEDIA`, `PAUSE`, `PLAY`, optionally `MEDIA_ANNOUNCE` flags; a `DEFAULT` supported format; an `AppEvent` for play media; `PlayerCommand` mapping for pause, play, stop, and the `STOP` that today stops only the alarm;
- state reporting that distinguishes music from replies (`playing`, `paused`, `idle`);
- ducking and the wake word interplay ([section 5](#5-b-detail-interaction-with-voice)), config, limits, logs, tests, docs.

Details in [section 4](#4-b-detail-playback-engine) and [5](#5-b-detail-interaction-with-voice).

### 3.C Music Assistant's own streaming protocols

```text
 Music Assistant server  <--- Sendspin (WebSocket ws://MA:8927/sendspin or client advert on 8928) --->  satellite
   JSON control + binary audio chunks with play-at timestamps, Kalman clock sync, Noise encryption
 satellite: decode (FLAC/Opus) -> schedule by timestamp -> pacat
 HA sees the player only through the Music Assistant integration (MA creates the HA entity)
```

- **Documented** (section 2): Sendspin is MA's native player protocol, technical preview in MA, RC1 on its site, with synchronised multi-room playback, native metadata, artwork and a static delay setting. Voice PE and ESPHome ship clients. MA also lists Snapcast, Squeezelite, DLNA, Cast and others as providers; none of those is a fit (Snapcast and Squeezelite need their own client software; DLNA is a different TV-side receiver, see 3.E).
- **Fit for the satellite:** the official SDKs are C++, JS and Python; a Rust crate exists but depends on `cpal` (needs `libasound`, not a static musl build with no system libraries) and would need a `pacat` sink. We would add a second network protocol with a clock-sync filter and timestamped scheduling on top of `pacat`, whose output latency we cannot read precisely (only the `--latency-msec` setting), so sync would rely on MA's manual delay. The protocol is still changing.
- **Value over B:** synchronised multi-room (this TV plus other speakers), MA-native metadata and seek, no ffmpeg proxy hop. **Value is small for one TV**, so this is a later option, not a first step. Revisit when the spec leaves preview (see [ADR 0010](../decisions/0010-music-playback.md) "Revisit when").

### 3.D LG native apps through HA's webostv integration

```text
 HA webostv integration --SSAP (needs pairing key; does not work in standby)--> TV
   media_player.select_source("Spotify") or play_media(media_content_type = app)
 TV runs the LG app; the app's own audio path plays; music control = the app's own (e.g. Spotify Connect)
```

- Completeness only. This launches an app; it is not music playback by the satellite or by MA. The TV screen turns on and the app owns the audio. Spotify's app is a Spotify Connect target on other platforms (observed-on-other-device, not checked on an LG). It does not work in standby and needs SSAP pairing, which is the very thing ADR 0005 and ADR 0007 avoid ([always-on](always-on.md)). **Out of scope** for the satellite. HA owners who want it can use the webostv integration without us.

### 3.E Also visible in MA's list: DLNA (hypothesis only)

MA's DLNA provider needs a UPnP renderer on the TV (LG TVs expose one on other firmwares, hypothesis; `umediaserver` runs on c3-a). Nothing is known on c3-a or c3-b. EXP-008 step 1 can include an SSDP browse from the PC at no cost; treat as a spare option, same limits as A (LG controls the behaviour).

## 4. B in detail: playback engine

### 4.1 Format negotiation: advertise one music format

HA converts everything to the first `DEFAULT` format with ffmpeg, so **the device decodes exactly one format**, not "whatever MA serves". Proposal: add a second `supported_format` with `purpose = DEFAULT`: **FLAC, 48000 Hz, 2 channels, 2 bytes** (config `[music] format`, rate, channels). Reasons: (1) the TV's output runs at 48 kHz (the AEC reference is captured at 48 kHz, [AEC](aec-frontend.md)), so no sample rate conversion is needed on our side and PulseAudio has nothing to resample (hypothesis: PA resamples anyway if it runs at 44.1 kHz; the TTS stream at 16 kHz mono already works this way, verified-on-target); (2) FLAC decoding is integer-only and the cheapest of the codecs in `symphonia` (ESPHome's own docs say FLAC has the lowest CPU use and needs a good network; MP3 less data, more CPU; documented); (3) the satellite is on a LAN to HA. The TV's userland is 32-bit ARM soft-float (D0-D1) and the build is opt-level `z`, so MP3's float math is the part to measure. WAV is the fallback for a debug or a very weak link; MP3 stays decodable for `bypass_proxy` and for direct URLs a user sends. Opus is not offered (needs a codec `symphonia` may not provide; ESPHome says it is the most CPU and memory hungry; hypothesis, recheck at build time).
 
The existing `ANNOUNCEMENT` format (flac 22050 mono) stays as is (HA's TTS format selection must not change, [ADR 0007](../decisions/0007-tv-media-player.md)).

If the owner's HA has **no ffmpeg** reachable by the esphome integration, the proxy fails; the satellite cannot fix that (document it).

### 4.2 Fetch: from whole file to a stream

- New `fetch_stream(url, cfg)` in `webos-playback`: `ureq` blocking reader on a dedicated thread (no tokio blocking-pool starvation for an hours-long download), `Content-Type` hint, **no total deadline and no body cap**, a **read-idle timeout** (for example 10 s; a stalled proxy ends the track), connect timeout as today, same TLS rules (HTTPS validated with bundled roots or the configured `ca_file`, **no switch to disable validation**), the existing URL redaction in logs (the proxy URL is a bearer secret: anyone with the random id can pull the stream once).
- The URL is **one-time and has no Range** (documented): a dropped connection cannot be resumed; the satellite reports the track as finished with an error and HA or MA decides (MA may re-issue). Do not retry the same URL: it would kill the running ffmpeg and restart the track from its beginning (documented). A reconnect is therefore not a feature.
- Only `http` and `https`; other schemes rejected (as today). Redirects follow `ureq` defaults, to be checked for downgrades from `https` to `http` in the test.
- Backpressure: the HA side writes ffmpeg's stdout as fast as the client reads, so a slow consumer slows ffmpeg (documented behaviour of a pipe, hypothesis for the proxy's flow control). The satellite reads only when its buffer has room.

### 4.3 Decode and buffer

- `symphonia` is already in the tree (**MPL-2.0**, [packaging section 8.1](packaging.md): section 3.3 of the MPL allows combining with GPL-3.0-or-later code; file-level copyleft applies to the unmodified crates; keep them unmodified and named in the notices). Formats already enabled: mp3, flac, wav, pcm, ogg, vorbis; **no AAC or ALAC feature is on** (adding `aac` or `alac` is a feature flag in `symphonia` 0.6, hypothesis, and not needed behind the proxy). Binary size: the decoders are already linked for TTS; streaming adds code, not codecs (expected tens of KiB, hypothesis, measure at M2).
- Streaming decode: a `MediaSource` over the fetch reader (`Read` only, `is_seekable() = false`), packets decoded on a dedicated thread into **interleaved s16 chunks** (for example 20 ms) sent to the async side through a bounded channel. Format changes mid-stream (rate or channels) end the track with an error, as `decode` does today.
- **Buffering.** Two stages with fixed caps: compressed read-ahead about 512 KiB (FLAC at 48 kHz stereo is roughly 0.6 to 1.0 Mbit/s, so 4 to 7 s; hypothesis, level and ratio vary), and decoded PCM about 1 s (192 KB at 48 kHz stereo s16). Start playing after about 1 s of PCM is ready. ESPHome's default is 1,000,000 bytes per pipeline (documented); we target **at most 1 MiB of music buffers in total**, far below the 64 MiB ceiling. Underrun: write silence to keep `pacat` fed, count it, and log once.
- **CPU.** Unknown on the TV. The AEC numbers show the armv7 build costs 23 times the host on float DSP; FLAC is integer and should be far cheaper, but this is a **hypothesis**. Build `symphonia` with `opt-level = 3` through `[profile.release.package.*]` as done for `webos-dsp` (a size cost of tens of KiB). M-E1 measures it with `sink.gain = 0` (silent).

### 4.4 Output

- **No resampling in the satellite for the proxied path**: the stream is already at the advertised rate; `pacat --rate=48000 --channels=2 --format=s16le --raw --stream-name=music` is started once per track (or once per session) with `--latency-msec` as for replies ([config](../development/esphome-api.md)). A direct URL with another rate or channel count is passed to `pacat` as its own native format (as the TTS stream does) and PulseAudio converts; no change to `resample.rs` is needed (it stays whole-buffer for announcements).
- A **second `pacat` process** for music next to the reply `pacat` (the existing code starts one process per playback, `kill_on_drop`). PulseAudio mixes them (standard behaviour, **hypothesis on this TV**; M-E2 checks it with a quiet tone and a reply). Extra memory is the child's own RSS (unmeasured; it does not count toward the daemon's RSS ceiling, which reads the daemon process only).
- Gain: the same software gain path as replies, `sink.gain` times a volume, plus a **duck factor** ramped per chunk. `sink.gain = 0` writes only zeros, so every TV test of the engine can run silent.

### 4.5 Semantics of the commands HA can send

| HA command or action | Satellite behaviour |
| --- | --- |
| `play_media` (URL, `announcement=false`, command none) | Replace: stop the current music, start the new stream. State `playing` once the first PCM chunk is queued, `idle` on end. |
| `play_media` with `ENQUEUE` (HA allows `ADD` only) | **Not offered at first** (no `MEDIA_ENQUEUE` flag). The queue lives in MA or HA, and a gap between tracks is accepted at first (hypothesis: MA issues the next `play_media` near the track end). A one-item next-track prefetch could be a later improvement. |
| `play_media` with `announcement=true` | Only if `MEDIA_ANNOUNCE` is offered: play over the music through the reply slot with the music ducked (same path as `Announce`), without stopping the music. Without the flag HA never sends it. See [open question 3](#9-open-questions-for-the-owner). |
| `PAUSE` | Stop reading and writing; keep the connection and decoder state. Audible stop within the `pacat` latency (about 0.2 s); a long pause may let the proxy's ffmpeg time out or fill its pipe (hypothesis), in which case the track is lost and `play` restarts nothing; report `idle` with a logged reason. Cap pause at a configurable time (default 10 minutes, then drop the stream and report `idle`). |
| `PLAY` | Resume from the buffer if paused; otherwise nothing (HA's `PLAY` on `idle` has no URL to start). |
| `STOP` | Stop music; if music is not playing, stop the alarm as today. `idle`. |
| `SET_VOLUME`, `VOLUME_UP`, `VOLUME_DOWN` | One entity volume, applied to music and replies as in ESPHome (see [open question 2](#9-open-questions-for-the-owner)); music gain is `volume` times a configurable `[music] gain` trim and capped by `[music] max_volume_percent`. |
| `MUTE`, `UNMUTE` | Not advertised on this entity today (the reply volume is its meaning). If advertised, mute sets the music gain to 0 and keeps the stream. |
| Seek, next, previous, shuffle, repeat | **Not in the ESPHome API** (documented): not offered. MA and HA handle next, previous and queue on their side. |
| `TURN_ON`, `TURN_OFF` | Not advertised (stays with the "TV" player). |

### 4.6 State reporting

The ESPHome state response carries only state, volume and muted (documented). HA shows no title, artist or position for this entity. With MA in front, **MA's own entity** carries metadata and queue, so the raw satellite entity is the transport. States: `playing` while music is producing audio, `paused`, `idle`. The reply and announcement path keeps reporting `Announcing` as `playing` ([entities.rs](../../crates/esphome-api/src/entities.rs)): a design question is whether that should be `playing` while music is only ducked (yes: the entity plays something).

### 4.7 HA side: one entity or a third

- **Recommended: one `media_player` ("Speaker") with media and announce** like ESPHome's speaker media player (single volume and state, HA's native `play_media`, HA's built-in media intents such as "pause the music" would be able to target it once it offers `PAUSE`; hypothesis, the volume intents are the ones read in ADR 0007, which keep only `playing` players when several match without a name). It keeps the existing object id `media_player` and entity id untouched.
- **A third entity** ("Music", object id `music_player`) would isolate music volume and state from the reply volume and avoid changing the meaning of the existing entity, at the cost of three media players on one device (the TV, the speaker replies, the music) and of the format-selection question: HA takes the TTS format from the first `ANNOUNCEMENT` format of any player, which a music-only entity would not have. A new entity key does not touch the existing one. It is the safer migration path and the heavier UX; listed as [open question 2](#9-open-questions-for-the-owner).
- Duplicated entities: if MA wraps the satellite through its Home Assistant provider, HA shows the raw ESPHome player **and** MA's wrapper. Name the raw one "Speaker" and say in the docs to use MA's entity for music.

## 5. B in detail: interaction with voice

### 5.1 Why this is the hard part

The satellite streams the mic **continuously** to HA, where `alexa` is detected ([ADR 0003](../decisions/0003-server-side-wake-word-for-mvp.md)). The mic is near-raw: only a high-pass filter and +24 dB gain; **AEC is off by default and parked** (`[aec] enabled = false`, [backlog #1](../project/backlog.md#1-professional-audio-front-end)). So music from the same speaker is heard by the mic at full level and (a) can trigger false wakes ("alexa"-like words in lyrics; false accepts are not measured with music, no corpus yet), (b) lowers the chance of hearing the real wake word, and (c) pollutes STT after a wake. **This risk is independent of the option**: AirPlay audio on the TV speaker hits the same mic. Only B can do something about (c), and AEC is the only fix for (a) and (b).

### 5.2 What B can do, in order of value

1. **Duck or pause on the wake word.** The satellite learns of the wake from HA's `WakeWordEnd` event (the daemon already turns it into `AppEvent::WakeWordDetected`, see `voice.rs`). Duck the music (ESPHome's own example: 20 dB, ramped; documented) at that event, hold the duck during STT, intent and the spoken reply, restore about 1 s after `RunEnd` (and after a continued conversation ends). Latency: the event arrives after HA has heard the wake word, so the first words of the command still carry music; **pausing** is cleaner for STT than ducking and costs a gap. Config `[music] on_wake = "duck" | "pause" | "none"`, default `duck` with `duck_db = -20` (values to be tuned by M-E4, not measured).
2. **Duck for replies, announcements and the timer chime** (the reply slot is active): same duck, restore when the reply ends. If the chime rings, keep ducked until it stops.
3. **Barge-in "stop"**: "alexa, stop" while music plays at normal volume needs the wake word to survive the music. Without AEC it will often fail at higher volumes; with the reference tap it should work (AEC measured 15 to 22 dB ERLE on host with TV and `pacat` content, [AEC](aec-frontend.md); **not measured with music on the TV**). Also add the button entity and the media player `STOP`/`PAUSE` as non-voice stops.
4. **AEC dependency.** The reference tap (SoundEngineCapture) sees both the music and the replies, since both go through PulseAudio into the TV's sound engine (verified-on-target for a `pacat` tone). So B and A both benefit from AEC; B additionally knows the exact stream and could later feed the reference directly. **Recommendation:** ship music with AEC off but document the limit, run M-E4 to measure false wakes and barge-in with and without AEC, and **ask the owner to un-park AEC (backlog #1) if M-E4 is bad**. Do not change the parked status here.
5. **Microphone mute and music.** The "Mute microphone" switch mutes capture only; music keeps playing. A "Listening" state with music playing should still be truthful (the mic is on).
6. **Safety and privacy.** Music playback adds no capture and no new data to HA beyond the state. The URLs are bearer tokens: redact them in logs and in HA-visible text (existing `redact_url`); never log titles from ambient audio (none are read).

### 5.3 Reply path while music plays

A reply is a separate `pacat` stream, so music and reply play together unless ducked. Order of events for "alexa, what time is it" during music: `WakeWordEnd` (duck to -20 dB), STT, intent, TTS stream or URL (reply at its volume), `RunEnd` (restore). Alarm chime: same duck. Announcement from HA (`Announce` event): same duck, then `AnnounceFinished`. HA's own `start_conversation` flow after an announcement: duck stays until the run ends.

### 5.4 Power states

| State | Music (hypothesis, to verify) | Basis |
| --- | --- | --- |
| `Active`, `Screen Saver` | plays | replies verified in both ([P1](../discovery/c3-a/P1-playback-silent-validation.md), [P2](../discovery/c3-a/P2-session1-audible-and-echo.md)) |
| `Screen Off` | plays (the recommended mode, [always-on](always-on.md) mode A) | verified for a `pacat` tone on c3-b |
| `Active Standby` (Always Ready, TV "off" by remote) | **silent**: LG cuts the DSP input gain (verified for a tone on c3-b); the daemon already wakes the TV and darkens it on the remote's power key when "Keep listening" is on | [ADR 0008](../decisions/0008-user-initiated-lg-settings.md) context, [always-on](always-on.md) |
| `Suspend` / `Power Off` | not running | verified-on-target |

On `play_media` while `Active Standby` or the screen dark, the first version only **plays what the TV lets through** and reports truthfully; it does not wake the TV. An opt-in "wake the TV for music" (the existing power-on path) is a follow-up and an owner decision (it lights the room). The satellite's `tv_power_state` sensor is the signal HA scripts can use.

### 5.5 Resource budget

| Item | Budget | Note |
| --- | --- | --- |
| Music buffers | at most 1 MiB resident (section 4.3) | 64 MiB RSS ceiling, current RSS about 1.7 MB (verified-on-target) |
| Decoder state | under 1 MiB (FLAC frames, hypothesis) | measure at M2 |
| Music thread CPU | target under 5 % of one core at 48 kHz stereo FLAC | **hypothesis**; M-E1 measures with `sink.gain = 0` |
| Second `pacat` | its own RSS, outside the daemon's ceiling | measure |
| Network | one TCP stream, FLAC about 0.6 to 1.0 Mbit/s plus the API connection | hypothesis |
| Watchdog | the loop-stall and RSS watchdogs apply unchanged; the music thread must never block the main loop (bounded channels, no unbounded buffering) | design rule |
| Binary size | unknown; expect under 100 KiB (hypothesis; AEC cost 400 KiB) | measure |

## 6. Comparison

| | A. AirPlay 2 + MA | B. Native `media_player` | C. Sendspin | D. webostv app launch |
| --- | --- | --- | --- | --- |
| Works today | **Unknown** (EXP-008 not run) | No (design only) | No | Yes in HA's webostv, not through us; needs pairing |
| Satellite code | none | about 1.5 to 2.5 kLOC Rust plus tests (estimate) | about 2 to 4 kLOC plus a crate to adapt (estimate) | none |
| Binary growth | none | small (hypothesis) | medium (WebSocket, Noise, clock sync) | none |
| Effort (agent steps) | 1 experiment (owner present for sound) | M1 to M6 below, about 6 | 8 or more, after the spec settles | none |
| Duck / pause for voice | No (LG decides) | **Yes** | possible later (we own the pipeline) | No |
| Standby behaviour | unknown, likely silent or unreachable | silent in `Active Standby` (LG), plays in `Screen Off` | same as B | webostv fails in standby |
| UX in HA | MA player for the TV, TV player for volume | MA player wrapping the satellite, or direct `play_media`; metadata only from MA | MA player, synced groups | app on the TV screen |
| Metadata, queue, gapless | MA | MA (queue), gaps possible | MA, best | the app |
| Needs AEC for barge-in | yes | yes | yes | yes |
| Main risks | LG receiver compatibility, volume fight, no ducking | one-time proxy URL (no reconnect), CPU on the TV, mix with the TV's own program, scope change (a "music player") | spec in flux, `cpal`/`libasound` mismatch, sync accuracy over `pacat` | out of scope |
| Reversibility | full | full (`[music] enabled = false`, default) | full | n/a |

## 7. Non-goal and scope

The MVP non-goal "writing AirPlay or a music player" in [requirements.md](../project/requirements.md) stays as written. B is **not** a music player in the usual sense (no library, queue or UI); it is the transport half of ESPHome's `media_player` that HA and MA drive. Whether that is still a non-goal is the owner's decision ([ADR 0010](../decisions/0010-music-playback.md)).

## 8. Recommendation and staged plan

**Recommendation.** (1) Run EXP-008 (A) first: it costs no code, it answers requirement F8 for this TV and it sets the baseline B has to beat (ducking, standby, one entity). (2) **Adopt B as the target design**, behind `[music] enabled = false` by default, if the owner accepts the scope. Build it silently on the PC first (M1 to M4); nothing needs the TV until M5. (3) Do not build C now; revisit it when Sendspin leaves preview and the owner has several Sendspin players. (4) Leave D to HA's webostv integration.

### Staged plan

| Step | What | Where it runs | Sound / TV |
| --- | --- | --- | --- |
| **0** EXP-008 | AirPlay with MA (section 3.A list). Part 1 (mDNS browse, MA player list) is read-only and silent; the playback parts need the owner present and a sound authorization. Scope the record to model, firmware and power state in [compatibility](../compatibility/README.md). | owner's MA, TV | sound authorization for 0.3 to 0.8 |
| **M1** | ESPHome crate: optional music features. With `[music] enabled`, advertise `PLAY_MEDIA`, `PAUSE`, `PLAY` (and `MEDIA_ANNOUNCE` if chosen) and a second `DEFAULT` supported format (FLAC 48000 2 ch 2 bytes); new `AppEvent::PlayMedia { url, announcement }` and pause/play commands; flags stay 5124 when disabled (non-breaking); key and entity id unchanged; fake-HA (aioesphomeapi) tests that check the entity info, the commands and that a media URL still reaches nothing when disabled. | PC | none |
| **M2** | `webos-playback`: `fetch_stream` (idle timeout, no body cap, TLS as now, redaction) and a streaming symphonia pipeline with bounded stages and a PCM chunk channel; tests with a local HTTP server (FLAC, MP3, WAV, truncated, slow, stalled, redirect to http, huge, format change), plus an armv7 qemu run. | PC | none |
| **M3** | Satellite `MusicPlayer` slot beside `Player`: second `pacat` (named args), gain and duck ramps, pause, resume, stop, end of stream, error, volume; state to HA; unit tests with a fake `pacat` as for replies; `sink.gain = 0` writes zeros. | PC | none |
| **M4** | Interplay: duck or pause on `WakeWordDetected`, reply, announcement, alarm; restore after `RunEnd` (continue-conversation aware); `STOP` rules (music first, then alarm); config validation (`[music]` section, `max_volume_percent`, `on_wake`, `duck_db`, `pause_timeout_s`); watchdog and log redaction checks; fake-HA end-to-end with a synthetic FLAC server. | PC | none |
| **M5** | On-TV, silent: install the build, play a 5 to 10 minute FLAC with `sink.gain = 0`; record CPU, RSS (daemon and `pacat`), network, buffer underruns, behaviour across the TV's power states and an HA disconnect. Needs **owner authorization to run a binary on a TV** (AGENTS.md). | c3-a, then c3-b | none (zero bytes) |
| **M6** | On-TV, audible: M-E2 and M-E3 below, then the docs, HA UX text and the compatibility record. | c3-a | owner authorization for sound |

### Experiments needed on the target (all need owner authorization; IDs to be assigned when written)

- **M-E1 resource, silent:** streaming decode and buffers at `sink.gain = 0` (CPU of a 1.4 GHz A76 core in the armv7 build and the aarch64 build, RSS, bytes on the wire, underruns, behaviour when HA closes the stream). Expected signal: under 5 % of one core and under 2 MiB of music RSS; failure means opt-level or format change, not abandonment.
- **M-E2 audible mix:** a quiet owner-chosen track at a low TV volume, a reply and the timer chime during it, ducking depth and ramp, pause and resume latency, one run per power state (`Active`, `Screen Saver`, `Screen Off`, `Active Standby`). Compare with EXP-008 results for A.
- **M-E3 mix with TV content:** music while an HDMI source or app plays (it mixes, because `pacat` and the program both reach the sound engine, verified for a tone). Check that the TV's own volume, mute and sound mode stay consistent.
- **M-E4 wake and barge-in with music (needs a consented corpus, [P2 corpus](../experiments/P2-corpus.md) style):** false accepts per hour of `alexa` with music at three volumes, "alexa, stop" success with and without AEC (`[aec] enabled`), STT word error after a wake with `duck` versus `pause`. This also feeds the open decision on un-parking AEC.
- **M-E5 HA and MA path on the owner's HA 2026.10.4:** `media_player.play_media` from the HA UI, a script, MA's Home Assistant provider and (if used) `music_assistant.play_media`, confirming the proxy URL shape, that MA lists the entity, how MA sequences tracks and whether a gap appears. This is the check for the **hypothesis that MA drives ESPHome media players through the HA provider**.

## 9. Open questions for the owner

1. **Scope:** is B (the satellite as an ESPHome music transport, no library or UI) acceptable under "do not write a music player", or does the non-goal stand and A remain the only path? ([ADR 0010](../decisions/0010-music-playback.md).)
2. **One entity or a third:** extend `media_player` (one volume for replies and music, like ESPHome) or add a separate "Music" player with its own volume (safer for the existing reply volume, one more entity)?
3. **Announcements over music:** offer `MEDIA_ANNOUNCE` (HA and MA could then send announcements through the media player) or keep announcements on the existing `Announce` path only?
4. **Wake behaviour:** default `duck` (-20 dB) or `pause` when the wake word is heard? Is a short pause acceptable?
5. **Standby:** play only what the TV lets through, or also an opt-in "wake the TV to play music"?
6. **AEC:** if M-E4 shows false wakes or failed "stop" with music, un-park AEC (backlog #1) for music?
7. **Sound authorization** for EXP-008 and M-E2 to M-E4 (volume limits, times, who is present), and **authorization to run a built binary on c3-a** for M5.
8. **Does the owner already use Music Assistant**, and which provider would the satellite appear under (Home Assistant Media Players)? This decides whether B has any consumer on day one.
