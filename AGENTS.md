# Agent instructions

## Mission and current state

Implement an open, local-first Home Assistant voice satellite for **rooted LG webOS TVs**. The repository is currently in **research/feasibility phase**, not product development. Prioritize evidence collection and small, reversible experiments over code volume.

> **Note (2026-11-01):** discovery on c3-a is largely done and the owner has accepted decisions (ESPHome native API, Rust, server-side wake word for the MVP, capture from `hw:LGSIC,0` with LG hands-free off). Work now follows the phases in [the roadmap](docs/project/roadmap.md) (Phase 1, silent build and integration, is current) and the [ADRs](docs/decisions/README.md). The evidence, labelling and device-safety rules below still apply to all phases, including running any binary on a TV (owner authorization required).

## Sources of truth

1. `docs/project/requirements.md` — scope and feasibility gates.
2. `docs/research/` — documented facts, hypotheses, device validation gaps.
3. `docs/experiments/` — reproducible procedures and results.
4. `docs/compatibility/` — findings explicitly scoped to TV model, firmware, and power state.
5. `docs/decisions/` — accepted architecture decisions; unaccepted proposals are not contracts.

Documentation from **webOS OSE** is not proof that the same API is exposed on **commercial LG webOS TV**. A finding on another LG model is not confirmation on this one.

## Research workflow

1. State one bounded question, its importance, and the required evidence.
2. Search official docs, relevant open source implementations, and existing findings **before** reverse engineering binaries.
3. Record each claim as `documented`, `observed-on-other-device`, `hypothesis`, or `verified-on-target` with source, model, firmware, date, and experiment ID where applicable.
4. Write a minimal experiment with prerequisites, commands, expected signals, failure interpretation, and rollback before requesting execution.
5. Make read-only observations first. Compare device and playback/power states where relevant.
6. Record negative or inconclusive results explicitly; never silently turn them into support.
7. Update relevant documents, compatibility records, and the linked issue/PR with evidence and residual unknowns.

## Device safety and privacy

- Assume root privileges can render a TV unusable. No firmware writes, service disabling, mounts, persistence, reboots, or audio recording without explicit authorization.
- Probe output may reveal identifiers or other machine-specific information. **Do not commit raw probe output.**
- Audio recordings are opt-in, short, local, and never committed. Obtain consent from anyone whose voice may be captured.
- Never log ambient speech or credentials by default; restrict server access, validate TLS, and document microphone mute/listening indicators.
- Don't execute remote install pipes or arbitrary binaries on the TVs. Inspect scripts before running them.
- Maintain rollback paths and avoid changing native LG voice services during discovery.

## When to use REA

Use the optional [REA reverse-engineering tool](https://github.com/morluto/rea) **only** where a question depends on proprietary shipped binaries, decompilation, or runtime evidence not established by source. Read [the repository RE skill](.agents/skills/webos-research/SKILL.md) and [REA guide](docs/tools/rea.md). Installing a skill alone does not install the REA MCP service or native analysis engines. Never treat REA findings as device validation without a corresponding experiment.

## Implementation principles

- Native background daemon is a **provisional** design choice, not a proven standby capability.
- Keep capture, preprocessing/AEC, activation, Assist transport, playback, power lifecycle, and media control separable.
- Prefer stable public interfaces. Never hard-code `hw:1,0`, private Luna endpoints, model names, or ALSA indexes as universal truths.
- Home Assistant owns STT/conversation/TTS in MVP. Wake word runs server-side (HA openWakeWord) for the MVP per [ADR 0003](docs/decisions/0003-server-side-wake-word-for-mvp.md); on-TV wake word is a later improvement.
- Music playback is native in the satellite's ESPHome media player (HA and Music Assistant `play_media`), per [ADR 0010](docs/decisions/0010-music-playback.md); the TV's AirPlay receiver is not used.
- Don't invent unverified API calls, provider behavior, latency, audio channels, or wake-word compatibility.

## Deliverables and quality gates

- PRs contain summary, evidence and scope, verification steps, rollback/risk notes, and open questions.
- Include tests for tooling where feasible; `sh -n tools/probe/probe.sh` and `python3 tools/check_docs.py` are initial checks.
- Keep repository content in English. Never commit generated dumps, tokens, recordings, firmware, or third-party binaries.
- If blocked on hardware, provide a precise experiment for the user/agent; do not claim the feature works.
