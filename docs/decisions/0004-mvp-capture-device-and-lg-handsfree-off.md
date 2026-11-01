# ADR 0004: MVP capture from hw:LGSIC,0 with LG hands-free off

- **Status:** Accepted (2026-11-01, owner)
- **Scope:** which audio source the MVP reads, and its relationship to LG's own voice features.
- **Related:** [ADR 0003](0003-server-side-wake-word-for-mvp.md), [backlog](../project/backlog.md), [requirements](../project/requirements.md)

## Context

- The built-in mic is ALSA `hw:LGSIC,0` ("WoV PDM Mic"): 2 channels, S24_LE (LSB-justified, sign-extended), 16 kHz, period 256 frames and buffer 8192 frames required. The signal is near-raw: DC removal plus a fixed +16 dB gain, no detected AGC or noise suppression, noise floor -68 dBFS. There is a startup burst of about 5 ms, so the first 250 ms are discarded. The two channels are two physically spaced microphones (about 6 to 8 cm, inferred) ([D3](../discovery/c3-a/D3-mic-identification.md)).
- `hw:LGSIC,1` ("SpeakerFeedback", 8 ch, S32_LE, 48 kHz) is opened by LG `voiceinput` together with the mic when hands-free is on. It is a candidate AEC reference, **untested**.
- **Exclusivity:** with LG hands-free on, `voiceinput` holds both PCMs continuously. With hands-free off they are released. There is no `dsnoop`, so the first opener gets the mic ([D2](../discovery/c3-a/D2-audio-stack.md#who-holds-which-sound-device-snapshot), [D3](../discovery/c3-a/D3-mic-identification.md#setup-and-method)).
- The Magic Remote mic does not use ALSA capture (Bluetooth path) and is not a candidate ([D3 step 5a](../discovery/c3-a/D3-mic-identification.md#2-step-5a-magic-remote-mic-button-2026-11-01)).
- Disabling or altering LG voice services is out of scope ([AGENTS.md](../../AGENTS.md)).

## Decision

The MVP captures from **`hw:LGSIC,0`**, channel 0, converted to 16 kHz mono S16 with a fixed gain and a high-pass filter, and requires **LG hands-free to be OFF**. The satellite detects that the mic is busy and reports it clearly rather than competing with `voiceinput`. The device name and parameters are configuration with runtime verification, not universal constants: capability discovery arrives in Phase 5.

## Consequences

- LG's own hands-free ("Hi LG") feature cannot be used on the TV while the satellite runs. The user must keep it off, and the satellite cannot enforce that.
- The Magic Remote voice feature still works, since it does not use the ALSA mic.
- Behaviour while LG's feature is switched on later, in standby, and with the TV playing sound is unknown (D4 and D5 pending).
- Channel 0 only: a naive channel average reduced SNR in D3; two-microphone processing is a backlog item.
- Echo from TV playback is not cancelled in the MVP; Phase 2 evaluates `hw:LGSIC,1` as a reference.
- The hardware names (`LGSIC`, card order) are c3-a facts and must not be assumed on other models or firmware.

## Alternatives considered

- **Keep LG hands-free on and tap its audio:** no shared-access tap was found in discovery and the PCMs are exclusive (no `dsnoop`). Not feasible without modifying LG services, which is not allowed.
- **Replace the LG voice services:** forbidden by project safety rules and not reversible without a vendor reinstall.
- **Magic Remote mic:** not available as ALSA capture and not hands-free.
- **External USB or network microphone:** a valid fallback, outside the project's goal of using built-in hardware.
- **Use both channels from the start (mean or beamformer):** deferred; see the [backlog](../project/backlog.md).

## Revisit when

- Phase 2 or Phase 3 shows that capture is unreliable, or the owner wants LG hands-free kept on.
- A shared-access method (an official or discovered mix or tap) becomes known.
- A firmware update changes the PCM names, parameters or exclusivity.
- A second TV model has a different capture topology.
