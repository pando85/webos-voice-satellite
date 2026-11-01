# EXP-001 — Runtime and audio inventory

**Status:** PLANNED · **Risk:** read-only

**Question:** What are the exact kernel/userland and audio/service surfaces on each TV?

Run the repository [probe](../../tools/probe/README.md) once on each TV while fully on. Privately record model and firmware via the UI, without serials or network identities. Inspect ALSA capture/playback endpoints, `/dev/snd` permissions, and available tools. Do not infer physical mic identity from ALSA numbering.

**Acceptance:** a sanitized [device record](../compatibility/DEVICE-TEMPLATE.md) exists for each TV; all untested capabilities remain inconclusive.
