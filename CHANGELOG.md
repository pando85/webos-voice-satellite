# Changelog

Releases of the webOS voice satellite package (`io.github.pando85.voicesatellite`). Earlier versions (0.1.0 to 0.5.1) are described in the git history and in the install records ([c3-a](docs/discovery/c3-a/P4-permanent-install.md), [c3-b](docs/discovery/c3-b/P4-permanent-install.md)).

## 0.6.0 (2026-11-07)

- **Remote key grab ([ADR 0016](docs/decisions/0016-remote-key-grab.md)).** With the mic button feature on (`[remote] mic_button = "ptt"`), a helper (`bin/remote-proxy`, new in the package) takes an exclusive grab of the Magic Remote's input device and re-sends every event except the mic key into an idle sibling device, so LG's toast and LG's assistant do not react to the mic key. It runs under ADR 0016's four-layer dead-man and releases the grab and every held key on feature off, stop, crash and uninstall. ADR 0013's in-memory configd override stays as the safety net.
- **Off unless the mic button is on.** With `mic_button = "off"` (the default) nothing of it runs: no helper process, no grab. `key_grab = "auto" | "off"` in `[remote]` only applies with the mic button on.
- **Firmware:** the grab is used only on firmware verified by EXP-026: `03.30.74` (c3-a). On any other firmware (c3-b's `03.30.73` included) the grab stays off and the configd override alone is used; `ctl.sh status` reports `key_grab` with the reason.
- Not yet run with the mic button on, on a TV: the owner's acceptance test is in [next steps](docs/project/next-steps.md).
