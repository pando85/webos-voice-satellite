# Music and existing media integration

## Core questions

Should music run via LG's existing AirPlay 2 receiver, Music Assistant, DLNA, or in-daemon playback? Can playback operate with screen off and remain controlled by HA?

## Evidence currently available (not device proof)

**Status: DOCUMENTED / OBSERVED_OTHER_MODEL**

LG C3 supports AirPlay 2, and Music Assistant supports AirPlay targets. Compatibility of particular firmware/session/power states remains untested.

## Hypotheses and device validation

**Status: HYPOTHESIS — target verification required**

Keep native media receivers separate from voice daemon; test input switching, discovery, resume, volume, network reconnect and simultaneous wake word. Do not use AirPlay as default path for short TTS over Netflix.

## Planned experiments

- [EXP-008](../experiments/README.md)

## Open decision

**Pending.** Record target TV model, firmware, runtime state and actual evidence before accepting an implementation choice. See [sources](sources.md) and [experiment protocol](../experiments/README.md).
