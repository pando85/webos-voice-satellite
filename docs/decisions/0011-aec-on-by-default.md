# ADR 0011: Echo cancellation on by default (B + NS), as a user setting

- **Status:** Accepted (2026-11-05, owner). Implemented in 0.4.3 (host tests); installed on c3-a, see the [c3-a compatibility record](../compatibility/c3-a.md). Benefit **not significant** in the live A/Bs; this is a product decision.

## Context

- The satellite's echo canceller (WebRTC AEC3 through `sonora`, reference `SoundEngineCapture` on the mic's card) shipped off by default ([AEC front-end](../design/aec-frontend.md)).
- Two live A/Bs on c3-a with TV content and a synthetic talker ([EXP-018](../experiments/EXP-018-aec-live-c3-a.md), [EXP-021](../experiments/EXP-021-aec-live-off-vs-bns.md)) did not show a significant wake word gain for the best configuration, B + NS: 61 % against 59 % at TV volume 10 (EXP-021, paired p 1.0); pooled with EXP-018 66 % against 52 % (p 0.13). The proposed pass criterion (misses halved) was not met. **verified-on-target**, c3-a, firmware 03.30.74, `Active`, TV speaker, one room.
- The same runs showed no harm: 0 wake-word false wakes during content in either arm (a weak bound, about 31 minutes), wake latency unchanged, about 5 points of one core for the aarch64 daemon, 0 reference underruns, a delay lock after every restart.
- The agent's recommendation after EXP-021 was an opt-in setting, default off. The owner decided on 2026-11-05 to make it a setting that is on by default.

## Decision

1. `[aec]` is **on by default** with the B + NS configuration: `enabled = true`, `render_channels = 2`, `ns = true`, `[aec.aec3]` `"linear_mix" = 0.4`, `"ep_strength.default_len" = 0.0`. These are the compiled-in defaults, so configs without an `[aec]` section (the TV app's `config.default.toml` included) run B + NS.
2. **Explicit values override every default**; `[aec.aec3]` is merged key by key over its two defaults.
3. **`[aec] enabled = false` turns it off** (the reference PCM is then never opened). It is the one key a settings toggle maps to; the daemon reads it at start, so a change needs a restart.
   Since 2026-11-06 the toggle is `ctl.sh config aec on|off` (TV app Sound screen) and, per [ADR 0012](0012-config-switches-in-ha.md), the `aec_enabled` switch in Home Assistant.
4. **The reference never costs the microphone:** if the reference cannot be opened or fails, the mic runs alone with a `warn aec: … WITHOUT echo cancellation` line; if the mic does not start together with the reference, or fails twice in a row with it, the plain mic is opened instead and the reference is tried again later.

## Consequences

- Every install gets the reference open and about 5 % of one core more CPU on c3-a (aarch64; the armv7 cost of B + NS is not measured on the TV, the AEC3 default cost 11 % of one core there).
- On other models, firmware or outputs the reference may be missing or behave differently; the fallback keeps the mic running, and the compatibility self-check's C8 then warns ("reference silent").
- The live A/B plans now spell out `enabled = false` for an OFF arm (a missing `[aec]` section means B + NS).
- Owners who see more misses or false wakes with it can turn it off; reports of that are evidence for revisiting this.

## Alternatives considered

- **Opt-in, default off** (the EXP-021 recommendation): rejected by the owner; no harm was seen and the cost is small.
- **Keep it parked:** rejected; it works on the TV as designed.
- **Default on with B (no NS):** not chosen; B + NS was the configuration of the counterbalanced run.

## Revisit when

- A false-wake soak of hours, a live-person test or STT shows harm, or users report it.
- The armv7 CPU of B + NS on a TV turns out too high.
- A TV model or output without a usable reference becomes common.
