# Audio capture and LG DSP

## Core questions

Which physical ALSA endpoints correspond to built-in far-field microphones? Channel count, sample format, clocking, gain and whether PCM is raw, beamformed or vendor-processed? Can native LG services share capture?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

own-your-glass documents ALSA capture paths on another rooted LG TV. CleanYourTV captures a DIFFERENT audio path: the Magic Remote through LG voiceinput. Neither proves C3 built-in capture.

The Magic Remote's own microphone (not ALSA on c3-a; LG's `voiceinput` remote stream on webOS 25 TVs) is covered in [remote mic button](remote-mic-button.md#2-the-remotes-own-microphone).

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Never hard-code ALSA card indexes. Map with /proc/asound and per-source consented test recordings. Inspect sample quality, process fd ownership and concurrency; test playback concurrently.

## Planned experiments

- [EXP-001](../experiments/README.md)
- [EXP-002](../experiments/README.md)
- [EXP-003](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
