# Music playback through the satellite's Home Assistant media player

- **Status:** **plan accepted in principle, nothing built, nothing run on a TV.** The owner decided on 2026-11-05: "We don't want to use AirPlay. Go with plan B." Option B (native ESPHome `media_player` playback in the satellite) is accepted in [ADR 0010](../decisions/0010-music-playback.md). The defaults in [section C](#c-proposed-defaults-owner-to-confirm) are **proposals the owner has not yet confirmed**. Written 2026-11-05.
- **Owner question (2026-11-05):** "Could we design music playback through the HA integration?"
- **Today:** the satellite's `media_player` (object id `media_player`, feature flags 5124 = `STOP` + `VOLUME_STEP` + `VOLUME_SET`) plays only assistant replies, announcements and the timer chime, through `pacat` to PulseAudio and the TV speaker or ARC. The "TV" media player ([ADR 0007](../decisions/0007-tv-media-player.md), flags 1420) is control only.
- **Related:** [requirements F8](../project/requirements.md), [backlog #5](../project/backlog.md#5-music-playback-through-the-satellites-esphome-media_player), [AEC front-end](aec-frontend.md), [always-on](always-on.md), [ESPHome API crate](../development/esphome-api.md), [research 06 media](../research/06-media.md), [AGENTS.md](../../AGENTS.md).

The document has two parts. **Part I (sections A to K) is the plan** to review and execute. **Part II (sections 1 to 9) is the analysis** the plan rests on (evidence, options, details), kept as background; where Part I and Part II differ, Part I wins. Part II still names the option letters A to D: A = the TV's AirPlay 2 receiver with Music Assistant, **B = native playback in the satellite (chosen)**, C = Sendspin, D = LG apps through webostv.

Labels follow AGENTS.md: **documented** (vendor docs or source read), **observed-on-other-device**, **hypothesis**, **verified-on-target** (c3-a, LG C3, webOS TV 8.3.0, firmware 03.30.74, or c3-b where named), **verified-on-host** (unit tests or PC runs). "Source reading" means the file was read on 2026-11-05 from the `dev` branch on GitHub (not the exact HA 2026.10.4 tag unless said), so line-level details can drift. Code facts below were read from this repository on 2026-11-05 (the `crates/` paths are given so a reviewer can check them).

# Part I. Plan

## A. Goal, non-goals, decision

**Goal.** Home Assistant (UI, scripts, automations, and Music Assistant through its "Home Assistant Media Players" provider) can play a music URL on the TV speaker through the **satellite's existing `media_player` entity**, using the same protocol ESPHome speaker devices use: HA sends `play_media` with a URL, HA's ffmpeg proxy converts it to the one format the device advertises, and the satellite fetches, decodes and plays it. While music plays, the satellite **ducks it for the wake word, replies, announcements and the timer chime**, so the voice assistant stays usable.

**Decision (owner, 2026-11-05).** B is accepted. AirPlay (A, including [EXP-008](../experiments/EXP-008-airplay-media.md)) is **rejected as a path for this feature, reason: owner decision**; C (Sendspin) stays deferred, D (LG apps through webostv) stays out of scope. The MVP non-goal "writing AirPlay or a music player" is lifted for satellite music playback by ADR 0010 (see [requirements](../project/requirements.md); the original wording stays visible there).

**Non-goals (stay non-goals).**

- No music library, queue, playlist, search, metadata, artwork, seek, next/previous, shuffle, repeat or UI in the satellite. The ESPHome API has none of them (documented, Part II section 2); Music Assistant or HA own them.
- No gapless playback in the first version (a gap between tracks is accepted), no multi-room sync (that is Sendspin, C), no crossfade.
- No decoding beyond what `symphonia` already links (FLAC, MP3, WAV, Ogg Vorbis). The format HA's proxy converts to is fixed by what the satellite advertises (one format).
- No change to the TV's own volume, mute, input or sound mode, and no LG private interface. The satellite does not wake the TV for music (default D6).
- No change to the reply, announcement and timer behaviour when `[music] enabled = false` (the default).

## B. What HA and Music Assistant see

**Entity.** The existing satellite `media_player` (object id `media_player`, key and entity id unchanged). No third media player is added (default D1). The "TV" media player of ADR 0007 is untouched.

**Feature flags** (`MediaPlayerEntityFeature` bits, `crates/esphome-api/src/entities.rs`):

| Setting | Flags | Value |
| --- | --- | --- |
| `[music] enabled = false` (default, today) | `STOP` 4096, `VOLUME_SET` 4, `VOLUME_STEP` 1024 | **5124**, unchanged |
| enabled | adds `PLAY_MEDIA` 512, `PAUSE` 1, `PLAY` 16384 | **22021** |
| enabled and `announce = true` (default) | adds `MEDIA_ANNOUNCE` 1048576 | **1070597** |

Not offered: `VOLUME_MUTE` (it would sit next to the "Mute microphone" switch and confuse; volume 0 is silence), `MEDIA_ENQUEUE`, `BROWSE_MEDIA`, `TURN_ON`/`TURN_OFF` (the "TV" player owns them), seek, next, previous, shuffle, repeat (not in the API). Whether HA adds its own browse support for a `PLAY_MEDIA` entity is a hypothesis to read in the HA source at M2.

**Supported formats** (`supported_formats`, in this order): the existing `ANNOUNCEMENT` entry (`flac`, 22050 Hz, 1 channel, 2 bytes; unchanged, because HA takes the TTS format from the first `ANNOUNCEMENT` entry, ADR 0007) and a new `DEFAULT` entry (`flac`, 48000 Hz, 2 channels, 2 bytes; configurable). HA's proxy converts every `play_media` URL to the first `DEFAULT` entry, or to the first `ANNOUNCEMENT` entry when `announce` is true (documented, Part II section 2).

**States.** `idle` (nothing), `playing` (music producing audio, **also while ducked**, and while a reply or announcement plays, as today: the existing `Announcing` state is already sent as `playing`), `paused`. The API has no error state: a failed or ended stream returns to `idle` and the reason is in the satellite log. No title, artist, position or duration is reported (the API has none); Music Assistant's own entity carries them.

**Volume.** One volume, 0 to 100 %, shared by replies and music (default D2). It uses the existing decibel taper (100 % is 0 dB, each percent below is 0.5 dB, `crates/satellite/src/volume.rs`) and the existing stored value. The TV's own volume is not touched. HA's volume buttons and sentences move this one slider.

**Commands HA can send and what happens:**

| HA service | Result |
| --- | --- |
| `media_player.play_media` (URL or `media-source://`) | replaces the current music; `playing` when the first audio is queued; `idle` when it ends |
| `play_media` with `announce: true` (needs `MEDIA_ANNOUNCE`) | plays over the music on the reply path with the music ducked; music not interrupted; the entity stays `playing` |
| `media_pause` / `media_play` | pause and resume the music (resume only while the stream is still held; see D8) |
| `media_stop` | stops a ringing timer alarm first, otherwise stops the music (default D5) |
| `volume_set`, `volume_up`, `volume_down` | the shared volume |
| enqueue, seek, next, previous, mute | not offered; HA does not send them |

**What Music Assistant sees (hypothesis, EXP-016 checks it).** With its Home Assistant provider, MA lists the satellite's HA entity as a player and calls `media_player.play_media` with an MA stream URL; HA proxies it like any URL. MA gets only state and volume back, no position, so it tracks progress itself and moves on when the state returns to `idle`. Because there is no seek and no enqueue, a gap between tracks is expected. HA shows two players for the device if MA wraps it (the raw one and MA's); the docs tell users to use MA's for music.

## C. Proposed defaults (owner to confirm)

Every row is a **proposed default, owner to confirm**. Each is a config value or a small code switch, so a different answer changes little. The first three affect M2 and must be confirmed before it starts.

| # | Question | Proposed default (owner to confirm) | Reason |
| --- | --- | --- | --- |
| D1 | One entity or a separate "Music" player | **Extend the existing `media_player`** with a media pipeline and an announcement path | It is how ESPHome speaker players are modelled and what HA and Music Assistant expect for ESPHome satellites; HA picks the TTS format from the same entity; no third media player, no new entity key. Fallback if rejected: a separate `music_player` entity (a new key, its own volume), about one extra day in M2 |
| D2 | Volume model | **One shared volume** for replies and music, same taper, plus `[music] gain_db = 0` trim | Matches the ESPHome model and HA's volume sentences; the TV volume sets the room level. A separate music volume is the fallback |
| D3 | `MEDIA_ANNOUNCE` | **Offer it** (`[music] announce = true`) | ESPHome speaker players take TTS and announcements this way (`tts.speak`, `announce: true`); costs one event type and a `Kind` that sends no `AnnounceFinished` |
| D4 | Wake word while music plays | **Duck** by `-20 dB`, attack 200 ms, hold until the run ends, release over 1000 ms after a further 800 ms hold; `on_wake = "pause"` and `"none"` available | Duck keeps the one-time proxy stream alive (pause can lose it, D8) and avoids a hard gap; -20 dB is ESPHome's documented example; values are untested here and EXP-014 and EXP-015 set the final numbers |
| D5 | Replies, announcements, alarm | **Duck** the same way (never pause); the alarm chime also ducks. `STOP` stops a ringing alarm first, then the music | A reply or chime must be heard over music; a stop meant for the alarm must not kill the user's playlist |
| D6 | Power states | **Play what the TV lets through, never wake the TV.** In `Active Standby` (LG silences the output, verified for a tone on c3-b) the satellite still plays and logs one warning; in `Suspend` nothing runs | Truthful and simple; an opt-in "wake the TV for music" is a follow-up owner decision (it lights the room) |
| D7 | `[music] enabled` | **false** until gates G2 to G6 pass and the owner signs off | Reversible, and the entity flags (5124) stay exactly as today when off |
| D8 | Pause | Pause stops reading; the connection is held up to `pause_timeout_s = 600`, then the stream is dropped and the entity goes `idle`. No retry of the proxy URL, ever | The URL is one-time and has no Range; a second request restarts the track (documented). Whether HA's proxy survives a pause is measured in EXP-016 |
| D9 | HA disconnect during music | **Keep playing up to `disconnect_stop_s = 60`**, then stop | A stream nobody can stop from HA should not run forever; short Wi-Fi drops must not cut the song |
| D10 | Format | **FLAC, 48000 Hz, 2 channels, 16 bit**; `wav` and `mp3` selectable in `[music] format` | The TV's output side runs at 48 kHz, FLAC is integer-only and cheapest of the compressed formats; EXP-013 decides FLAC against WAV (no decode, 1.5 Mbit/s) on measured CPU |
| D11 | Caps | `max_volume_percent = 100` (no cap), `idle_timeout_s = 10` (a stalled proxy ends the track), buffers at most 1 MiB total | No invented cap before measuring; the stall and memory bounds are safety rules |
| D12 | Enqueue and gapless | **Not offered / accepted gap** in this version | The API has no queue; revisit after EXP-016 shows what MA does at track ends |

Sketch of the config section (names are proposals; the daemon rejects unknown keys like the other sections):

```toml
[music]
enabled = false          # D7
format = "flac"          # D10: flac | wav | mp3
rate = 48000
channels = 2
announce = true          # D3
on_wake = "duck"         # D4: duck | pause | none
duck_db = -20
duck_attack_ms = 200
duck_hold_ms = 800
duck_release_ms = 1000
gain_db = 0              # D2
max_volume_percent = 100 # D11
idle_timeout_s = 10      # D11
pause_timeout_s = 600    # D8
disconnect_stop_s = 60   # D9
```

## D. Architecture

Final data and control flow (new parts marked `*`):

```text
 Home Assistant UI / scripts / Music Assistant (HA Media Players provider)
   |  media_player.play_media(satellite entity, URL or media-source, announce?)
   v
 HA esphome integration: resolves media-source; if the device lists supported_formats,
   replaces the URL by a one-time proxy URL (ffmpeg -> first DEFAULT format, or first
   ANNOUNCEMENT format when announce is true):
   http(s)://HA/api/esphome/ffmpeg_proxy/<device>/<random>.flac   (no Range, chunked)
   |  MediaPlayerCommandRequest{command | volume | media_url | announcement}
   |  (the existing encrypted API connection)
   v
 crates/esphome-api   server.rs  satellite_media_player_command   [changed]
   |     AppEvent::PlayMedia{url, announcement}*  MediaPause*  MediaPlay*  MediaStop  VolumeChanged
   |     AppCommand::SetMusicState*  ---> merged with the voice state into MediaPlayerStateResponse
   v
 crates/satellite   daemon.rs  (one event loop; owns everything below)         [changed]
   |
   |-- Player (existing)  replies, announcements, alarm        ----------------------> pacat #1 "tts-reply"
   |      Kind::MediaAnnounce*  (announce:true; plays like Announce, sends no AnnounceFinished)
   |
   |-- MusicPlayer*  music.rs
   |     fetch thread (ureq, read-idle timeout, no body cap)
   |        -> compressed ring, about 512 KiB
   |        -> decode thread (symphonia, non-seekable) -> 20 ms s16 chunks, bounded (about 1 s)
   |        -> async music task: gain = sink.gain * volume * duck, written at most 150 ms ahead of
   |           the clock (like the alarm chime), underrun -> silence + counter  ----> pacat #2 "music"
   |
   |-- DuckController*  duck.rs   inputs: wake/run active, reply or announce playing, alarm ringing,
   |                              pause command; output: target dB with attack, hold, release
   v
 PulseAudio mixes #1 + #2 (+ the TV program) -> TV sound engine -> TV speaker / ARC

 Mic path (unchanged): capture -> HPF and gain (AEC parked) -> ESPHome API -> HA wake word.
 The mic hears the music; that is risk R1 and gate G4.
```

**Event order for "alexa, what time is it" during music:** HA sends `VoiceAssistantWakeWordEnd` (existing, becomes `AppEvent::WakeWordDetected`) -> duck to -20 dB; STT; intent; TTS reply on pacat #1 at the reply volume; `RunEnd` (and, for `continue_conversation`, the run that follows) -> hold 800 ms -> release over 1000 ms. An announcement or the alarm chime takes the same duck and releases after the last of them ends. The duck is applied in the music task at write time and the writes stay about 150 ms ahead of the clock, so the audible lag is that plus `sink.latency_msec` (hypothesis; measured in EXP-014). Without the 150 ms pacing the Linux pipe buffer (64 KiB, about 340 ms of 48 kHz stereo) would add its delay.

**Component list** (paths read on 2026-11-05):

| Component | New or changed | Role |
| --- | --- | --- |
| `crates/esphome-api/src/config.rs` | changed | `Config::music` (format, announce flag), validation (format flac/wav/mp3, rate 8000 to 96000, 1 or 2 channels) |
| `crates/esphome-api/src/entities.rs` | changed | flags 22021 / 1070597 when enabled, second `supported_formats` entry, state merge (voice state and music state) |
| `crates/esphome-api/src/server.rs` | changed | `satellite_media_player_command`: `media_url`, `announcement`, `PAUSE`, `PLAY`; today only `STOP` and volume are handled |
| `crates/esphome-api/src/app.rs` | changed | new `AppEvent::PlayMedia`, `MediaPause`, `MediaPlay`; new `AppCommand::SetMusicState` (the enums are `#[non_exhaustive]`) |
| `crates/esphome-api/examples/fake_device.rs` | changed | a `--music` switch so the Python fake HA can test the entity |
| `crates/playback/src/stream.rs` | **new** | `fetch_stream` (blocking reader on its own thread, idle timeout, no body cap, same TLS rules and redaction as `fetch.rs`) and `StreamDecoder` (non-seekable symphonia, bounded stages, chunk channel, cancel) |
| `crates/playback/src/{lib,error}.rs` | changed | exports, new error cases (idle timeout, stream ended early) |
| `crates/playback/tests/` | changed | streaming tests with a local HTTP server and synthetic fixtures |
| `crates/satellite/src/music.rs` | **new** | `MusicPlayer`: one music slot, second `pacat` (reusing `pacat_named_args`), pacing, pause, resume, stop, state, stats, wav and null sinks for tests |
| `crates/satellite/src/duck.rs` | **new** | pure state machine from sources to a target gain with ramps (unit-testable without audio) |
| `crates/satellite/src/player.rs` | changed | `Kind::MediaAnnounce`; export `gain_s16le`, `pacat_named_args` to `music.rs`; the alarm still owns the single reply slot |
| `crates/satellite/src/daemon.rs` | changed | route the new events, feed the duck sources (`WakeWordDetected`, `RunEnd`, `Announce`, TTS stream start, alarm start and stop), `STOP` order, HA-disconnect and `Resumed` rules, state publication, shutdown |
| `crates/satellite/src/config.rs` | changed | `[music]` section, defaults, validation |
| `Cargo.toml` (workspace) | changed | `[profile.release.package.symphonia*] opt-level = 3` if EXP-013 needs it (as for `webos-dsp`) |
| `tools/esphome-fake-ha/` | changed, **new `fake_proxy.py`** | HA's ffmpeg proxy and `play_media` behaviour, scenarios (section F) |
| `packaging/webos/` | changed | config template with `[music]` (commented, disabled) |
| docs: `docs/development/esphome-api.md`, `docs/compatibility/c3-a.md`, user docs | changed | entity flags, config, limits, results scoped to model, firmware and power state |

No new third-party crate is planned (std threads, `ureq`, `symphonia` already in `crates/playback`). `symphonia` is MPL-2.0, already linked and unmodified, which the project's GPL-3.0-or-later position already covers ([packaging section 8.1](packaging.md)). If M3 finds that `symphonia` cannot probe a non-seekable FLAC or an unknown-length WAV, the fallback is a small pure-Rust FLAC decoder crate (`claxon`, Apache-2.0, hypothesis, check the licence then); that is a decision for M3, not now.

## E. Milestones

Sizes are rough estimates of production code plus tests, not measurements. "PC" means no TV, no sound. Every milestone ends green on `cargo fmt --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`, `sh -n tools/probe/probe.sh` and `python3 tools/check_docs.py`, and gets a `code-review` pass before it is merged.

| Milestone | Scope and files | Acceptance (what proves it on the PC) | Depends on / parallel | Size | Risks |
| --- | --- | --- | --- | --- | --- |
| **M1 Fake-HA music harness** | `tools/esphome-fake-ha/fake_proxy.py` (new), helpers in `fake_ha.py`: emulate HA's `play_media` and ffmpeg proxy (section F); Python unit tests for the proxy | proxy test: one-time URL, second GET kills the first conversion and restarts from the start, chunks of 2048 bytes, no `Content-Length`, no Range (a `Range` header is ignored, full 200), the format and rate in the URL are what ffmpeg produced (checked with `ffprobe`); `tools/esphome-fake-ha/run.sh --device-only` unchanged and green | none; **parallel with M2, M3** | S (about 400 lines Python) | the emulator drifts from HA's real proxy: re-read `ffmpeg_proxy.py` at the start and note the HA version; EXP-016 is the real check |
| **M2 Entity and commands** | `esphome-api` `config.rs`, `entities.rs`, `server.rs`, `app.rs`, `fake_device` | unit tests: flags 5124 off, 22021 and 1070597 on; `supported_formats` list; every command (`play_media` URL, announce, `PAUSE`, `PLAY`, `STOP`, volume) becomes the right event; unadvertised commands (enqueue, mute) are ignored and the state is echoed; media URL still ignored when music is off (existing test kept); state merge matrix (announcing, music playing, paused, idle); config validation. Fake-HA scenario with aioesphomeapi 46.6.0: the entity shows the flags and formats, commands reach `fake_device`'s event log | needs D1, D3 confirmed; **parallel with M1, M3** | M (about 500 lines) | HA's reaction to changed flags (an existing device) is only checkable on a real HA, EXP-016 |
| **M3 Streaming playback** | `crates/playback/src/stream.rs` and tests | with a local HTTP server and synthetic fixtures (tone and noise as FLAC, MP3, WAV, generated by a documented script, ours): (1) decode equals the reference PCM (RMS error under a set bound; bit-exact for FLAC and WAV); (2) chunked body without length plays; (3) truncated stream ends with an error, not a hang; (4) a stalled stream ends within `idle_timeout + 1 s`; (5) a 1 GiB generated WAV streams with resident memory growth under 3 MiB; (6) backpressure: a slow consumer bounds what the server could send; (7) cancel joins both threads within 1 s; (8) mid-stream format change ends with an error; (9) redirect from `https` to `http` is refused; TLS validation and `ca_file` reuse the existing tests; (10) no URL in any error text. Run the test binary for armv7 under qemu as the playback tests do now | none; **parallel with M1, M2**; freeze the `MusicStream` interface (chunk type, cancel, stats) first so M4 can start | M (about 700 lines plus fixtures) | `symphonia` and non-seekable FLAC or unknown-length WAV (fallback in section D); decode thread starving the main loop (an option: lower its priority) |
| **M4 MusicPlayer and duck engine** | `music.rs`, `duck.rs`, `player.rs` exports | unit tests with a fake `pacat` (the pattern in `player.rs` tests) and `tokio::time::pause`: written bytes equal input times `sink.gain * volume * duck` (DC test signal); ramp reaches `duck_db` within `attack` plus one chunk and returns within `release`; several duck sources hold until the last one ends; pause writes nothing and resume continues with no lost or repeated sample; stop leaves no `pacat`; writes never run more than 150 ms ahead; underrun writes silence and counts it; `sink.gain = 0` writes only zeros; a **streaming** wav file sink for music (the reply path's `stream_wav` buffers the whole playback in memory and writes the file at the end, which is wrong for a long track and for a stop in the middle), paced in real time so envelopes are testable; pause timeout; shutdown | the `MusicStream` interface from M3 (can use a fake stream); **parallel with M3 after the interface freeze** | M (about 600 lines) | PulseAudio buffering (hypothesis) shows only on the TV (EXP-014); duck arithmetic in `f32` on soft-float armv7 (cost measured in EXP-013) |
| **M5 Daemon integration, config, PC end to end** | `daemon.rs`, `satellite/config.rs`, `player.rs` (`Kind::MediaAnnounce`), packaging template | daemon unit tests for routing, `STOP` order, HA-disconnect and `Resumed` rules, mic mute not affecting music, volume sharing, log redaction (no URL in any line); config parse, defaults and validation. **End to end with the real daemon**, a file microphone, the wav sink, the Python fake HA and `fake_proxy.py`: S1 play, finish, `idle`; S2 pause, play, stop; S3 volume change while playing; S4 `announce` over music (music ducked and restored, entity stays `playing`, no stray announce-finished); S5 voice run during music (wake word, STT, TTS, run end: duck then release, envelope check on the wav); S6 continue-conversation keeps the duck; S7 timer alarm over music, `STOP` stops the alarm first; S8 proxy stall and early close end the track and report `idle`, with no retry of the URL; S9 HA disconnect: plays on, stops after `disconnect_stop_s`; S10 `[music] enabled = false`: flags 5124 and a media URL ignored | M2, M3, M4 (M1 for the e2e) | M to L (about 600 lines plus Python scenarios) | the existing wav sink neither paces nor streams (`stream_wav` collects all chunks, then writes); M4 adds both for music; the e2e then runs in real time (a few minutes) |
| **M6 On the TV, silent** | build for armv7, install on c3-a (silent), run EXP-013 and the silent part of EXP-016 | thresholds in section G; results in `docs/compatibility/c3-a.md` scoped to model, firmware, power state, with redacted evidence; rollback documented | M5, **owner authorization to run a binary on a TV** | S for the code, a session for the runs | CPU or memory over the limit leads to a format change (WAV) or an opt-level change, not abandonment |
| **M7 On the TV, audible, and docs** | EXP-014, EXP-015, remaining EXP-016, user docs, HA text, compatibility record, backlog and requirements F8 update; the default decision | gates G3 to G6 decided and recorded; docs let another owner enable `[music]` | M6, **sound authorization**, a consented session for EXP-015, Music Assistant if the owner has it | S for code (tuning), several sessions for the runs | the false-wake result may require the parked AEC (gate G4) |
| **M8 Later (backlog, not planned)** | next-track prefetch or a session-long `pacat` for a smaller gap, `on_wake = "pause"` refinement, wake-the-TV option, Sendspin revisit | each needs its own note | after G6 | n/a | n/a |

Rough total: about 1.8 to 2.5 kLOC of production Rust, about the same again in Rust tests, and about 500 lines of Python. M1, M2, M3 run in parallel; M4 joins after the M3 interface freeze; M5 is the join point.

## F. What the fake HA must emulate

Behaviours of HA's ESPHome integration (documented, source reading, Part II section 2) that `tools/esphome-fake-ha/` must reproduce; re-read `media_player.py` and `ffmpeg_proxy.py` when M1 starts and record the commit read.

1. **Format negotiation.** HA reads `supported_formats`, takes the first `DEFAULT` entry for `play_media` or the first `ANNOUNCEMENT` entry when `announce` is true, and builds the proxy URL with that format, rate and channels. The fake asserts the satellite lists exactly one entry per purpose it intends and picks accordingly.
2. **The proxy.** URL `/api/esphome/ffmpeg_proxy/{device_id}/{random}.{format}`; a real `ffmpeg -i SRC -f FORMAT -ar RATE -ac CH -sample_fmt s16 -map_metadata -1 -vn pipe:` (ffmpeg is installed on the PC); plain 200, chunked body in 2048-byte chunks, no `Content-Length`, no Range; **one-time**: a second GET of the same URL kills the running conversion and starts over from the beginning; at most 2 conversions per device (behaviour on the third to be read from the source); ffmpeg blocks when the client is slow (pipe backpressure); no auth beyond the random id. HTTPS variant with a private CA for the `ca_file` path.
3. **Commands.** `media_player_command` with `media_url`, `announcement`, `PAUSE`, `PLAY`, `STOP`, volume set, volume up and down; commands HA would not send for unadvertised features are also sent once, to prove the satellite ignores them.
4. **Announce while music plays.** `announce: true` with an `ANNOUNCEMENT`-format proxy URL while a `DEFAULT` stream is running.
5. **Voice run while music plays.** `VoiceAssistantRequest`, wake-word end, STT, TTS (stream and URL mode), run end, with and without `continue_conversation`.
6. **Failures.** A proxy that stalls, closes early, returns 404 or 500, serves an unexpected format, or serves a very long stream; HA disconnect and reconnect during playback.
7. **State view.** What HA would show from `MediaPlayerStateResponse`: the five states HA 2026.10.4 maps (idle, playing, paused, on, off); `announcing` is never sent.

## G. On-target experiments

IDs follow the next free numbers in [the experiments index](../experiments/README.md): **EXP-012 is reserved by [ADR 0009](../decisions/0009-phone-setup-web-server.md) (another session)**, so this plan claims **EXP-013 to EXP-016**. Each gets a file from [TEMPLATE.md](../experiments/TEMPLATE.md) (prerequisites, steps, expected signals, failure interpretation, rollback) **when its milestone starts**; if the numbers are taken by then, renumber and fix the links here. The old placeholders M-E1 to M-E5 in Part II map to EXP-013, 014, 014, 015, 016. EXP-008 (AirPlay) is **not planned** (owner decision).

Common rules: c3-a first, c3-b only after c3-a and with the owner's agreement; one device, firmware and power state per result; negative and inconclusive results are recorded as such; no raw dumps, audio or identifiers committed; the build is installed with the normal procedure and rolled back to the previous version afterwards; music playback is never left running unattended.

| ID | Question and measures | Authorization | Pass / fail (proposed thresholds, owner to confirm) | Rollback |
| --- | --- | --- | --- | --- |
| **EXP-013** Music engine resources, silent | With `sink.gain = 0` and a 10 minute FLAC (and WAV, MP3) from the PC through the proxy emulator: CPU of the daemon and decode thread (`/proc/<pid>/stat` deltas) for the armv7 build, RSS of the daemon and of `pacat` #2 (`ps`), underruns per 10 min, network rate, mic-stream dropouts compared with a no-music baseline, behaviour when the proxy closes mid-stream, a 10 minute pause, HA disconnect, and the remote's power key (`Screen Off`, `Active Standby`) and a suspend and resume (stream dropped cleanly, no orphan `pacat`) | run a build on c3-a (AGENTS.md); **no sound** (zero bytes only) | **Pass:** decode CPU at most 5 % of one core; daemon RSS at most 8 MiB (limit 64); `pacat` #2 RSS recorded; 0 underruns in 10 min; no mic dropouts beyond baseline; clean end in every failure case. **Amber:** 5 to 15 % CPU: try `opt-level = 3` for the decoder crates, then WAV. **Fail:** over 15 % CPU after both, or any mic dropout: stop and reopen ADR 0010 | stop playback, set `[music] enabled = false`, reinstall the previous version; confirm RSS and capture back to baseline |
| **EXP-014** Audible mix and ducking | Quiet owner-chosen track at a low TV volume: do pacat #1 and #2 mix (reply and chime over music); duck depth, attack and release as measured at the SoundEngineCapture tap (local recording, deleted afterwards); pause and resume latency; behaviour with HDMI or app content playing; TV volume, mute and sound mode unchanged; `Active`, `Screen Saver`, `Screen Off`, `Active Standby` (compare with [always-on](always-on.md): silent expected) | **sound authorization**: volume ceiling, times, who is present, tracks | **Pass:** the two streams mix; duck at least 15 dB reached within 300 ms of the wake event and restored within 1.5 s after the run; reply intelligible over the duck (owner listening, yes or no); pause audible within 500 ms; TV settings unchanged; no glitches in the TV program. **Fail:** PulseAudio does not mix two clients on this TV: redesign as an in-process mix into one `pacat` (gate G3) | stop, disable `[music]`, restore TV volume if touched |
| **EXP-015** Wake and barge-in with music | Per [P2 corpus](../experiments/P2-corpus.md) statistics (counts with exact Poisson 95 % intervals, rule of three for zero, never "0 per hour"): false wakes of `alexa` per hour with music at the owner's typical TV volume and at one louder volume, a 1 hour screen each, then a 3 hour soak at the typical volume if the screen shows at most 1; "alexa, stop" and "alexa, what time is it" success over music (at least 10 trials per volume, Wilson interval); STT word error after a wake with `duck` against `pause`; each with `[aec] enabled` false and, if the owner un-parks it, true. Detection counts come from HA's pipeline logs (the rig's `ha_assist_watch.py`), not from recording the room | **sound authorization** and a **consented** session (voices in the room; stimulus playback of the existing corpus phrases, never ambient recording); long runs are unattended-listening, so the owner chooses time and place | **Pass (G4):** 0 false wakes in at least 3 h at the typical volume (bound at most 1 per hour) and "stop" success at least 80 % at the typical volume. **Amber:** 1 to 3 per hour or 50 to 80 %: enable only with documented limits and ask the owner to un-park AEC. **Fail:** over 3 per hour or under 50 %: AEC is a prerequisite for enabling music by default | stop playback; no recordings are kept |
| **EXP-016** Real HA and Music Assistant path | On the owner's HA 2026.10.4, silent (`sink.gain = 0`) on c3-a: `media_player.play_media` from the UI and a script with a URL and a `media-source://`; the proxy URL shape; end state `idle`; `announce: true`; pause then resume after 1 and 10 minutes (does the proxy keep the stream); stop; volume sentences ("turn up the volume") when music plays (which entity answers; ADR 0007's hypothesis that `playing` players win); whether HA picks up a changed `[music] enabled` flag set without removing the device; if Music Assistant is available: does its Home Assistant provider list the entity, how it starts and sequences tracks, the gap at a track end, what it does without seek or enqueue | run a build on c3-a, HA and (optional) Music Assistant access; **no sound** | **Pass:** play, finish, pause, stop and volume work from HA; flags change is picked up (or the needed step is documented); the proxy behaves as documented. **Inconclusive** is allowed for the MA part if MA is unavailable or does not list the entity (record it; the direct HA path still stands). **Fail:** HA never sends `play_media` or never picks the `DEFAULT` format: re-read the HA source and the fake, fix M2 | disable `[music]`, restart so HA sees flags 5124 again |

## H. Risks and mitigations

| # | Risk | Effect | Mitigation | Check |
| --- | --- | --- | --- | --- |
| R1 | **Music near the mic: false wakes, missed wake word, polluted STT.** The mic is near-raw and AEC is parked (`[aec] enabled = false`) | the assistant triggers on lyrics, "stop" fails at volume, bad STT after a wake | duck on wake; document the limit; `max_volume_percent` available; **gate G4**: measure, and if bad ask the owner to un-park AEC (backlog #1; the reference tap sees the music because both streams go through PulseAudio into the TV sound engine, verified for a `pacat` tone) | EXP-015 |
| R2 | **One-time proxy URL, no Range, no resume** (documented) | a dropped connection or a long pause loses the track | never retry the URL (it would restart the track); report `idle` and let HA or MA decide; `pause_timeout_s`; prefer duck over pause | M3 tests, EXP-016 |
| R3 | **CPU on the TV** (armv7 soft-float, opt-level `z`; the AEC stage cost 23 times the host on float DSP) | mic or API starvation, watchdog stalls | FLAC integer decode, `opt-level = 3` for the decoder crates, WAV as a no-decode format, decode on its own thread, optional lower priority | EXP-013 thresholds |
| R4 | **Memory under the 64 MiB RSS limit** (`limits.max_rss_mb`, daemon about 1.7 MB idle, verified on c3-a) and the watchdog | the watchdog exits with code 72 | fixed caps, 1 MiB of music buffers, bounded channels, no unbounded growth; the second `pacat` is a child process outside the daemon's count (record it) | M3 memory test, EXP-013 |
| R5 | **PulseAudio mixing with the TV program and with reply `pacat`** | music replaces or distorts instead of mixing | a tone and the HDMI program already mix in the TV's taps (verified); EXP-014 checks two `pacat` clients; fallback: in-process mix into one `pacat` | EXP-014, gate G3 |
| R6 | **Standby silence** (`Active Standby` silences the output, `Suspend` stops everything; verified on c3-b) | music "plays" with no sound | play what the TV lets through and log it, never wake the TV (D6); the `tv_power_state` sensor lets HA scripts react | EXP-013, EXP-014 |
| R7 | **Duck lag from buffering** (pipe 64 KiB, `pacat` latency) | the first words of a command still hear music | writes at most 150 ms ahead of the clock; `sink.latency_msec` as for replies | EXP-014 |
| R8 | **Licensing** of the decoder against GPL-3.0-or-later | a compliance problem | `symphonia` is MPL-2.0, already linked, kept unmodified and named in the notices ([packaging section 8.1](packaging.md)); no new crate planned; any new decoder crate gets a licence check in its milestone | M3 review |
| R9 | **Binary size** (AEC cost about 400 KiB) | the package grows | expected under 100 KiB (hypothesis); measure the stripped armv7 binary before and after M3 and M5; investigate above 150 KiB | M3, M5 |
| R10 | **HA behaviour not in the sources read** (flags pickup, proxy limits, volume intents with several players, `announce`) | surprises on the real HA | M1 re-reads the source; EXP-016 on the owner's HA before the default flips | EXP-016 |
| R11 | **Privacy and secrets.** The proxy URL is a bearer token; ambient audio must not be logged | a leak in logs or reports | `redact_url` on every log line, no titles or URLs in HA-visible text, no new data sent to HA beyond the state, mic mute does not touch music but the "Listening" state stays truthful | M3 and M5 tests |
| R12 | **Suspend or Wi-Fi drop leaves a dead stream** | a hung music task and a `pacat` child | `AppCommand::Resumed` and the idle timeout end the stream; shutdown kills `pacat` | M5, EXP-013 |

## I. Decision gates

| Gate | When | Rule | If it fails |
| --- | --- | --- | --- |
| **G0** Defaults | before M2 (D1, D3), before M4 (D4 to D9) | the owner confirms or changes section C | change the config values; D1 fallback adds a new entity |
| **G1** PC complete | end of M5 | all M1 to M5 acceptance green, review done | fix before any TV run |
| **G2** Resources | after EXP-013 | CPU, RSS, underruns, mic dropouts within thresholds | `opt-level = 3`, then WAV, then stop and reopen the ADR |
| **G3** Mixing | after EXP-014 | PulseAudio mixes two clients; duck and pause latencies within thresholds | in-process mixer into one `pacat` (extra milestone) before anything else |
| **G4** False wakes | after EXP-015 | **if false wakes exceed 3 per hour at the owner's typical volume, or "stop" success is under 50 %, un-park AEC (backlog #1) before enabling music by default**; 1 to 3 per hour or 50 to 80 %: allowed with documented limits and an owner request to un-park AEC | keep `enabled = false` by default and ship as experimental |
| **G5** Real HA | after EXP-016 | the direct HA path works on 2026.10.4; MA result recorded | fix M2 or the fake; document what MA can and cannot do |
| **G6** Default | after G2 to G5 | the owner decides whether `[music] enabled` becomes true by default and whether the feature leaves "experimental" | stays false |

The thresholds in G2, G4 and the experiments are proposals set before any measurement; the owner confirms or changes them at G0.

## J. Owner actions and when

| When | Action |
| --- | --- |
| Now (G0) | Confirm or change D1 to D12; confirm the proposed thresholds in section G; note that AGENTS.md still says "Existing TV AirPlay receiver / Music Assistant is a candidate for music" (not edited here, it is a project instruction file) |
| Before M6 | **Authorize running a built binary on c3-a** for a silent music run (`[music] enabled = true`, `sink.gain = 0`); say whether the routine version deploy authorization already covers it, and whether c3-b joins later |
| Before M6 | Say whether **Music Assistant** is available on the HA instance (and which provider the satellite should appear under); without it EXP-016 covers only the direct HA path |
| Before M6 | A media source HA can serve to the satellite for the silent run (a local file or a radio stream); HA needs ffmpeg for the proxy (HA OS has it) |
| Before M7 | **Sound authorization** for EXP-014 and EXP-015: TV volume ceiling, dates, who is present, the tracks to use; **consent** of anyone whose voice is in the room for EXP-015; the owner's "typical volume" |
| At G4 | Decide whether to un-park the AEC front-end for music |
| At G6 | Decide the default of `[music] enabled` and the "experimental" label |

## K. UI/UX integration (later)

**Handoff for the TV app and phone setup redesign (another session, [voice-and-power screen](voice-and-power-screen.md), [ADR 0009](../decisions/0009-phone-setup-web-server.md)). This plan designs no screens, menus or layouts.** It only lists what the backend will offer so the redesign can use it. Everything here is a **proposal**, nothing is implemented, and the names are not final until the redesign agrees them. The channels already exist: `ctl.sh status --json`, the daemon's `run/voice.json` status file (schema 1, written atomically, rewritten on change plus a 30 s keep-alive) and `ctl.sh set ...` for the satellite's own settings (the `reply-volume` pattern: write a file under `etc/`, the daemon picks it up).

**Settings the UI may expose** (config keys from section C; none is needed to use music from HA):

| Setting | Config key | How a change would be applied | Via |
| --- | --- | --- | --- |
| Music on or off | `[music] enabled` | needs a daemon restart or reconnect, because HA reads the entity flags (5124 or 22021) when it connects; same class as other settings that change the entity list | `ctl.sh set music on\|off` (proposed) |
| Duck mode on wake | `[music] on_wake` (`duck`, `pause`, `none`) | picked up by the running daemon within a second | `ctl.sh set music-on-wake MODE` (proposed) |
| Duck depth | `[music] duck_db` (default -20) | same | `ctl.sh set music-duck-db N` (proposed) |
| Music level trim | `[music] gain_db` | same | `ctl.sh set music-gain-db N` (proposed) |

The shared volume is already the existing `reply-volume` setting (`ctl.sh set reply-volume N`, `reply_volume` in `status --json`); there is no separate music volume (default D2). Attack, hold and release times, format, rate, timeouts and `max_volume_percent` stay config-file only.

**Status fields the UI may show** (all read-only; **no URL, title, artist or any content name is ever carried**, see R11):

| Field | Meaning | Carried in |
| --- | --- | --- |
| `music.enabled` | `[music] enabled` of the running daemon | `status --json` (`music_enabled`) and `run/voice.json` |
| `music.state` | `idle`, `playing` or `paused` (the state HA also sees) | `run/voice.json`, `status --json` under `music` |
| `music.ducked` | true while a duck is held, with `music.duck_db` the current depth | `run/voice.json` |
| `music.format` | the format in use, for example `flac 48000 Hz 2 ch` | `run/voice.json` |
| `music.source` | where the play command came from, only as a class (`ha`), never an address or name | `run/voice.json` |
| `music.last_end` | how the last track ended: `ended`, `stopped`, `error`, `idle_timeout`, `ha_gone` | `run/voice.json` |
| `music.underruns` | counter since the daemon started | `run/voice.json`, also in the compatibility report counters |

The `run/voice.json` rules from the voice screen design apply unchanged (written only on change, keep-alive, deleted on a clean shutdown, stale after 60 s means unknown). The music block is optional; an older daemon omits it and the UI must treat it as "music not available". `ctl.sh report` gets the music counters and the `[music] enabled` value only, with the same redaction as today.

---

# Part II. Background analysis

The material below is the analysis written before the owner decision. It is kept as evidence and detail. **Option A (AirPlay) was rejected by the owner on 2026-11-05**, so its text is history, not a plan. The placeholder experiment names M-E1 to M-E5 are now EXP-013 to EXP-016 (section G), the old milestones M1 to M6 are replaced by section E, and sections 7 to 9 below are superseded.

## 1. Short answer (historical)

When the question was asked there were two realistic routes:

- **A. Use what the TV already has** (AirPlay 2 receiver driven by Music Assistant). No satellite code; LG decides volume, standby and ducking. **Rejected by the owner (2026-11-05).**
- **B. Play music inside the satellite** through the same ESPHome `media_player` entity that Home Assistant and Music Assistant already know how to drive (`play_media` with a URL). HA's ESPHome integration converts any source to the one format the device advertises with its ffmpeg proxy, so the satellite needs a **streaming** fetch plus the decoders it already links (FLAC, MP3, WAV through `symphonia`), and a second `pacat` stream. This is the route that lets the satellite **duck or pause music for the wake word, replies, announcements and timers**. **Chosen.**

Sendspin (C) and webostv app launching (D) were never recommended and stay out.


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

> **Rejected by the owner on 2026-11-05 ("We don't want to use AirPlay").** Kept as the record of what was considered and why it was a candidate. [EXP-008](../experiments/EXP-008-airplay-media.md) is not planned. Text below is history.

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

- `symphonia` is already in the tree (**MPL-2.0**, [packaging section 8.1](packaging.md): section 3.3 of the MPL allows combining with GPL-3.0-or-later code; file-level copyleft applies to the unmodified crates; keep them unmodified and named in the notices). Formats already enabled: mp3, flac, wav, pcm, ogg, vorbis; **no AAC or ALAC feature is on** (adding `aac` or `alac` is a feature flag in `symphonia` 0.6, hypothesis, and not needed behind the proxy). Binary size: the decoders are already linked for TTS; streaming adds code, not codecs (expected tens of KiB, hypothesis, measure at M3 and M5).
- Streaming decode: a `MediaSource` over the fetch reader (`Read` only, `is_seekable() = false`), packets decoded on a dedicated thread into **interleaved s16 chunks** (for example 20 ms) sent to the async side through a bounded channel. Format changes mid-stream (rate or channels) end the track with an error, as `decode` does today.
- **Buffering.** Two stages with fixed caps: compressed read-ahead about 512 KiB (FLAC at 48 kHz stereo is roughly 0.6 to 1.0 Mbit/s, so 4 to 7 s; hypothesis, level and ratio vary), and decoded PCM about 1 s (192 KB at 48 kHz stereo s16). Start playing after about 1 s of PCM is ready. ESPHome's default is 1,000,000 bytes per pipeline (documented); we target **at most 1 MiB of music buffers in total**, far below the 64 MiB ceiling. Underrun: write silence to keep `pacat` fed, count it, and log once.
- **CPU.** Unknown on the TV. The AEC numbers show the armv7 build costs 23 times the host on float DSP; FLAC is integer and should be far cheaper, but this is a **hypothesis**. Build `symphonia` with `opt-level = 3` through `[profile.release.package.*]` as done for `webos-dsp` (a size cost of tens of KiB). EXP-013 measures it with `sink.gain = 0` (silent).

### 4.4 Output

- **No resampling in the satellite for the proxied path**: the stream is already at the advertised rate; `pacat --rate=48000 --channels=2 --format=s16le --raw --stream-name=music` is started once per track (or once per session) with `--latency-msec` as for replies ([config](../development/esphome-api.md)). A direct URL with another rate or channel count is passed to `pacat` as its own native format (as the TTS stream does) and PulseAudio converts; no change to `resample.rs` is needed (it stays whole-buffer for announcements).
- A **second `pacat` process** for music next to the reply `pacat` (the existing code starts one process per playback, `kill_on_drop`). PulseAudio mixes them (standard behaviour, **hypothesis on this TV**; EXP-014 checks it with a quiet tone and a reply). Extra memory is the child's own RSS (unmeasured; it does not count toward the daemon's RSS ceiling, which reads the daemon process only).
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

1. **Duck or pause on the wake word.** The satellite learns of the wake from HA's `WakeWordEnd` event (the daemon already turns it into `AppEvent::WakeWordDetected`, see `voice.rs`). Duck the music (ESPHome's own example: 20 dB, ramped; documented) at that event, hold the duck during STT, intent and the spoken reply, restore about 1 s after `RunEnd` (and after a continued conversation ends). Latency: the event arrives after HA has heard the wake word, so the first words of the command still carry music; **pausing** is cleaner for STT than ducking and costs a gap. Config `[music] on_wake = "duck" | "pause" | "none"`, default `duck` with `duck_db = -20` (values to be tuned by EXP-015, not measured).
2. **Duck for replies, announcements and the timer chime** (the reply slot is active): same duck, restore when the reply ends. If the chime rings, keep ducked until it stops.
3. **Barge-in "stop"**: "alexa, stop" while music plays at normal volume needs the wake word to survive the music. Without AEC it will often fail at higher volumes; with the reference tap it should work (AEC measured 15 to 22 dB ERLE on host with TV and `pacat` content, [AEC](aec-frontend.md); **not measured with music on the TV**). Also add the button entity and the media player `STOP`/`PAUSE` as non-voice stops.
4. **AEC dependency.** The reference tap (SoundEngineCapture) sees both the music and the replies, since both go through PulseAudio into the TV's sound engine (verified-on-target for a `pacat` tone). So B and A both benefit from AEC; B additionally knows the exact stream and could later feed the reference directly. **Recommendation:** ship music with AEC off but document the limit, run EXP-015 to measure false wakes and barge-in with and without AEC, and **ask the owner to un-park AEC (backlog #1) if EXP-015 is bad**. Do not change the parked status here.
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
| Decoder state | under 1 MiB (FLAC frames, hypothesis) | measure at M3 and M5 |
| Music thread CPU | target under 5 % of one core at 48 kHz stereo FLAC | **hypothesis**; EXP-013 measures with `sink.gain = 0` |
| Second `pacat` | its own RSS, outside the daemon's ceiling | measure |
| Network | one TCP stream, FLAC about 0.6 to 1.0 Mbit/s plus the API connection | hypothesis |
| Watchdog | the loop-stall and RSS watchdogs apply unchanged; the music thread must never block the main loop (bounded channels, no unbounded buffering) | design rule |
| Binary size | unknown; expect under 100 KiB (hypothesis; AEC cost 400 KiB) | measure |

## 6. Comparison

Column A is kept for history (rejected by the owner); B was chosen.

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

The MVP non-goal "writing AirPlay or a music player" in [requirements.md](../project/requirements.md) was **lifted for satellite music playback by [ADR 0010](../decisions/0010-music-playback.md) on 2026-11-05** (owner decision). B is still **not** a music player in the usual sense (no library, queue or UI); it is the transport half of ESPHome's `media_player` that HA and MA drive. The non-goal stays for AirPlay, a library, a queue and a UI. See section A.

## 8. Recommendation and staged plan

**Superseded** by the owner decision and by sections A to K above. The earlier recommendation (run EXP-008 first, then B) is void: EXP-008 is not planned. The earlier staged plan M1 to M6 is replaced by the milestones in section E, and the earlier experiment placeholders by EXP-013 to EXP-016 in section G.

## 9. Open questions for the owner

Answered by the owner on 2026-11-05 (question 1) or given **proposed defaults, owner to confirm** in section C (the rest):

1. Scope: B accepted; AirPlay rejected (owner).
2. One entity or a third: D1, one entity.
3. Announcements over music: D3, offer `MEDIA_ANNOUNCE`.
4. Wake behaviour: D4, duck at -20 dB.
5. Standby: D6, play what the TV lets through, never wake it.
6. AEC: gate G4 after EXP-015.
7. Sound and binary authorizations: section J.
8. Music Assistant availability: section J.
