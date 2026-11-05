# EXP-014: Audible music mix and ducking on c3-a

**Status:** TARGET_VERIFIED for the mix, duck depth and timing, restore, pause, resume, volume and STOP order with announcements and the timer alarm; the wake-word duck source, an owner's listening judgement and other power states NOT TESTED
**Date:** 2026-11-05 (11:30 to 11:35 CET)
**Device alias / model / firmware / webOS / architecture:** c3-a, LG C3 42 inch (OLED42C34LA), firmware `03.30.74`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland, daemon `webos-satellite 0.4.1 (aarch64-unknown-linux-musl)`, Home Assistant 2026.10.4
**Power and playback state:** `Active`, screen on, HDMI 1 (the owner's PC display; the TV's own program audio was whatever the PC played, not controlled). `[music] enabled = true`; satellite volume 25 % (-37.5 dB) for all audible steps; **TV volume 89, mute off and everything else untouched**.
**Related research / requirements / issue:** [music playback plan](../design/music-playback.md) (section G, section D duck budget, milestone M7, gate G3), [EXP-017](EXP-017-music-engine-resources.md) (resources), [EXP-016](EXP-016-music-real-ha-and-ma.md)
**Risk:** short audible output at a low level (about 113 s of music in three segments, the longest 50 s; one alarm of 6 s; three announcements), no recording
**Approval needed / obtained:** owner, 2026-11-05 11:00 CET: "you can run tests now": sound allowed on c3-a at a low level (satellite volume at most 40 %, TV volume unchanged), each audible segment at most 60 s, at most 10 minutes in total, between 09:00 and 21:30 CET (`date` checked before each run), no audio recording (level numbers only), no 3 hour soak

## Question and falsifiable hypothesis

Do the reply or announcement stream (`pacat` #1) and the music stream (`pacat` #2) mix in PulseAudio on c3-a; is the music ducked by 15 dB or more within 400 ms of the daemon's trigger line and restored within 1.5 s after the source ends; do pause, resume and the shared volume work; does `media_stop` stop a ringing timer alarm first and the music second?

## Prerequisites and safe baseline

As in EXP-017 (same daemon, same config change, restored afterwards). Ducking sources that need no person: `assist_satellite.announce` with a short Spanish message, and a 10 s timer started through the conversation API (`conversation/process`, text "pon un temporizador de 10 segundos", agent `conversation.home_assistant`, `satellite_id` of the satellite). The wake-word duck source needs a spoken "alexa"; it was **not** triggered (nobody asked to speak, and the TV must not play the wake word).

**How the level was measured.** The daemon owns the capture PCM, so the satellite's microphone cannot be read in parallel and no microphone level is reported. Instead a small meter on the TV (python3 `audioop`, copied to `/tmp` and deleted afterwards) read **our own stream only**: `parec -d pcm_output.monitor --monitor-stream=<sink-input index of the music stream>`, one RMS and one peak value per 20 ms, nothing stored but those numbers. This is the stream as PulseAudio mixes it (after the daemon's gain and ramps and after `pacat`'s buffering; PulseAudio reported a sink latency of 17.9 ms), not the speaker output and not the microphone. The clocks are the TV's own (the daemon's log lines and the meter use the same clock).

## Commands / steps

HA set the satellite volume to 25 %, `play_media` of the Radio Browser station "Classic Vinyl HD" through HA's proxy (FLAC 48 kHz stereo), then scripted (all times relative to the start):

- Run 1: at +12 s `assist_satellite.announce` ("Esta es una prueba de anuncio sobre la musica."); at +27 s the timer request (10 s); the alarm rang at +37 s; at +43 s `media_stop` (expected: the alarm); at +50 s `media_stop` (expected: the music).
- Run 2: at +10 s `media_pause`, at +16 s `media_play`, at +24 s volume 35 %, at +30 s volume 25 %, at +37 s `media_stop`.
- Run 3: at +10 s a longer announcement (101 characters, 5.6 s of audio); `pactl list short sink-inputs` and a second meter on the reply stream (`--stream-name`/`media.name` `tts-reply`) while it played.

## Expected evidence and failure interpretation

Pass (plan G): the two streams mix; duck of at least 15 dB within 400 ms of the daemon's trigger log time and restored within 1.5 s after the source ends; pause audible within 500 ms; TV settings unchanged. Fail: PulseAudio does not mix two clients on this TV, which would mean an in-process mixer into one `pacat` (gate G3).

## Observations and redacted evidence links

Level numbers are dBFS of our own stream at the PulseAudio tap, 100 ms or 20 ms windows; the music was at 25 % (-37.5 dB), about -55 to -63 dBFS RMS (a quiet radio track).

| Step | Trigger (daemon log, TV clock) | Result |
| --- | --- | --- |
| Announcement over music | `announcement ... (46 chars)` at 10:30:42.658Z | music reaches -6 dB at **124 ms**, **-15 dB at 145 ms**, -19 dB at 166 ms (target -20 dB, ramp 50 ms; the deepest 100 ms window was 27 dB down because the track also got quieter); `voice.json` `ducked` true while it played |
| Release after the announcement | `playback 5 done` at 10:30:47.781Z | back within 10 dB after **921 ms**, within 3 dB after **1081 ms** (hold 800 ms plus a 1 s release ramp) |
| Timer alarm over music | `timer alarm: ringing` at 10:31:07.864Z (the timer: started 10:30:57.863, finished 10:31:07.863 for 10 s) | music -6 dB at 118 ms, **-15 dB at 139 ms**, -19 dB at 161 ms |
| Release after the alarm stop | `timer alarm stopped (media player stop; was Ringing)` at 10:31:13.657Z | back within 10 dB after 1206 ms, within 3 dB after **1366 ms** |
| STOP order | `media_stop` 1 at 10:31:13.6Z, `media_stop` 2 at 10:31:20.7Z | the first stopped **only the alarm** (music kept playing, restored as above); the second logged `music: stopping (stopped)`, state `idle`; the alarm binary sensor `off`, the timers sensor `none` |
| Pause | `music: paused by Home Assistant` at 10:33:32.620Z | the stream is at -99 dBFS (silence) in the 100 ms window starting 160 ms after the log line, **within 500 ms** (the output stays open, a 10 ms fade) |
| Resume | `music: resumed by Home Assistant` at 10:33:38.703Z | the level is back at the next window, **within 100 ms** |
| Volume 25 % to 35 % and back | `reply volume 25 -> 35 % by HA` | mean level +6.5 dB (nominal +5 dB; the programme varies), back to 25 %: within 1.1 dB of the first level |
| Mixing | `pactl list short sink-inputs` during a 5.6 s announcement over the music | **two sink inputs on the same sink at once**: the music (s16le 2 ch 48000 Hz) and the reply (s16le 2 ch 44100 Hz); PulseAudio mixes them |
| Reply level over the ducked music | meter on the reply stream | the first 1.8 s (the meter ended with `Stream error: Entity killed` when the reply stream closed) peaked at **-53.8 dBFS**; the ducked music in the same window was at -79.4 dBFS (it was at -58.4 before): the reply peak was about 25 dB above the ducked music at 25 % volume |
| Underruns | `voice.json` `music.underruns` | 4 before and **4 after all three runs** (no underruns during ducks, the alarm, pause or volume changes) |
| TV settings | HA entities at the end | TV volume 89, mute off, input `HDMI 1`, app `Other`, screen on, sound output `tv_speaker`, power state `Active`: unchanged; no kernel warning |

Audible time: music 50 s (run 1), 38 s (run 2) and 25 s (run 3) = **113 s**, the alarm about 6 s, announcements 3, 3 and 6 s: well inside the 10 minute budget, no segment over 60 s.

## Conclusion and confidence

**G3 (mixing and ducking): PASS** for what was measured, with a **partial** scope: two clients mix in PulseAudio on c3-a; the duck reaches 15 dB 139 to 145 ms after the daemon's announcement or alarm log line (threshold 400 ms; the budget in design section D was 210 ms for a wake trigger) and is restored 1.1 to 1.4 s after the source ends (threshold 1.5 s); pause, resume and the shared volume work; STOP order matches D5. Confidence medium: one repetition per case; the measuring point is PulseAudio's stream, so any delay after PulseAudio (the TV sound engine) is not included, and the clock reference is the announcement or alarm event, **not the wake-word event**.

**NOT TESTED:** the wake-word duck source (needs a spoken "alexa", `ducked` timing from `wake word detected by HA`); whether the reply is intelligible over the duck, **by ear** (nobody was asked to listen; only numbers); behaviour with HDMI or app content of the TV's own (the TV was on its HDMI 1 input with whatever the PC played, not controlled and not measured); `Screen Saver`, `Screen Off` and `Active Standby` (music stops per D6); `max_volume_percent`; the satellite microphone's own view of the music (risk R1 and gate G4 belong to EXP-015, not authorized in this run).

## Rollback and cleanup

Music stopped; volume set to 40 % (the original); the meter scripts deleted from the TV's `/tmp`; the config restored and the daemon restarted as in EXP-017.

## Follow-ups / unresolved questions

- Repeat with the wake word (EXP-015 Part 2, owner present): the duck from `wake word detected by HA` is the budget this experiment was meant to check; the announcement and alarm sources use the same engine path, so the expectation is a similar time, a **hypothesis** until measured.
- The daemon logs no duck begin or end line (only `voice.json` `ducked`, written on change); a log line per duck would make this measurable without a meter.
- An owner listening session at a normal TV volume: is the reply intelligible over the duck, is -20 dB the right depth.
