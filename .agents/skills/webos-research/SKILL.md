---
name: webos-research
description: Evidence-driven investigation of rooted LG webOS TV audio, lifecycle and Home Assistant integration. Use for research questions, hardware probes, compatibility findings and experiment design.
---

# webOS voice research

Read `AGENTS.md` and `docs/research/README.md` first. Work on one bounded technical question. Search existing findings and source ledger before proposing commands.

## Method

1. Record a hypothesis and its evidence class: DOCUMENTED, OBSERVED_OTHER_MODEL, HYPOTHESIS, TARGET_VERIFIED, REJECTED, INCONCLUSIVE.
2. Propose the minimal read-only experiment; identify exact TV model, firmware, power state and audio input.
3. Require explicit approval before recordings, strace/ptrace, system modifications or persistent hooks; do not run an experiment just because SSH access exists.
4. Redact identifiers and private speech. Keep raw reports outside Git.
5. Update research, experiment and compatibility documents as necessary. Leave precise remaining unknowns.

## REA escalation

Only use [REA](https://github.com/morluto/rea) when a bounded question requires analysis of **shipped proprietary binary/runtime** behavior, not for ordinary source code review. Agent-local REA skill installation is optional; the skill alone does not configure its MCP or decompiler. Read its native-artifact and runtime-observation guides when applicable. Keep file hashes, firmware identifiers, symbol offsets, evidence and unsupported inferences separate. Do not upload or commit proprietary LG files.

## Finished means

A question is answered with specific verifiable evidence, or marked PARTIAL/INCONCLUSIVE with a concrete next experiment. Never treat a result from a different TV as verified on our targets.
