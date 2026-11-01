# Using REA for webOS reverse engineering

REA ([morluto/rea](https://github.com/morluto/rea)) is a **conditional tool**, not a baseline dependency.

## Appropriate use

Escalate to REA only when official documentation, Linux interfaces, and available open-source implementations cannot answer a **specific** question about an LG-proprietary shipped binary, service, or runtime behavior (for example: whether a voice daemon routes DSP-processed samples through a named socket).

Do not invoke REA for architecture review of this repository, generic Rust code, shell diagnostics or ordinary GitHub searches.

## Agent setup (on the developer workstation)

Read REA's **current official installation guide** before running setup. The upstream agent skill is called `reverse-engineer-anything`; the skill alone does **not** install its analysis engines or register its MCP service in an already-running agent. A possible installation route described by upstream is:

```sh
npx skills add morluto/rea --skill reverse-engineer-anything
# If the REA MCP integration is desired, follow upstream provider setup:
npx rea-agents setup
```

Only run installation after reviewing its dependencies, permissions and environment changes. Ghidra/Hopper/IDA compatibility and requirements vary. No REA dependency is installed by this repository or CI.

## Before opening a binary

1. Write a bounded question and identify TV model, webOS release, firmware, artifact path and SHA256.
2. Choose static read-only examination first; no proprietary binaries committed to Git or publicly uploaded.
3. Collect the smallest evidence supporting the answer: referenced strings, functions, control-flow paths, offsets and relevant log observations.
4. Compare static inference with a **separately approved** on-device experiment.
5. Close analysis sessions and record unresolved alternatives, rather than claiming a version-independent API.

Follow [AGENTS.md](../../AGENTS.md) and [research skill](../../.agents/skills/webos-research/SKILL.md).
