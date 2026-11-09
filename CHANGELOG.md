# Changelog

Releases of the webOS voice satellite package (`io.github.pando85.voicesatellite`). Earlier versions (0.1.0 to 0.5.1) are described in the git history and in the install records ([c3-a](docs/discovery/c3-a/P4-permanent-install.md), [c3-b](docs/discovery/c3-b/P4-permanent-install.md)).

## Unreleased

- **Home Assistant's "turn off the TV" is a real power-off** ([ADR 0005 decision 4, amended 2026-11-09](docs/decisions/0005-tv-control-entities.md#amendment-2026-11-09-decision-4-a-real-power-off-from-ha), owner). The TV media player's turn off now sends the remote's power key without `[tv_control] power_off` (Always Ready is read again right before the key; a second power command is held until the TV reacts, at most 10 s): with "When the TV is turned off with the remote" on "Turn off normally" and Always Ready on, the TV goes to `Active Standby` (red standby LED, app sound stops) and the satellite keeps listening; replies, announcements and music are silent there, as after the remote's own key. With Always Ready off or unknown (the `lg_always_ready` sensor off or unreadable) it stays a screen off, logged at `info` with the reason, because the key would suspend the TV and freeze the satellite. "Keep listening" is unchanged in effect (key, then wake and darken to `Screen Off`) but no longer needs `power_off`. Other refusals (kill switch, firmware guard, transition, darkener budget) stay refusals.
- The opt-in `tv_power` switch (same object id) uses the same key and checks, without the screen-off fallback; its on is now a plain turn on (no longer refused by the kill switch or Always Ready); its name is now "TV power".
- Not run on a TV: [EXP-010 Part C](docs/experiments/EXP-010-tv-control-setters.md#part-c-ha-turn-off-as-a-real-power-off-adr-0005-decision-4-amended-2026-11-09) is the check on c3-b (owner authorization needed). c3-a's TV power paths are untested.

## 0.6.1 (2026-11-08)

- Remote key grab verified on firmware `03.30.73` (c3-b), by owner decision (2026-11-08; commit `c5dc2a7`, [ADR 0016](docs/decisions/0016-remote-key-grab.md)), next to `03.30.74` (c3-a). Other firmware is still refused: the grab stays off there and the configd override alone is used.
- Still off by default: nothing of the grab runs unless `[remote] mic_button = "ptt"`.
- Package sha256 `a7b367e9211fb8bcda2e0f07d9f04b3a89c2a7b083ddc0c412ef2194b30f43cf`. Installed on c3-b on 2026-11-08 with the owner's mic button on, so the grab started there (`key_grab` `on`, `lg_voice_key` `ours (grab + configd)`, no WARN or ERROR; [c3-b P4](docs/discovery/c3-b/P4-permanent-install.md#update-to-061-2026-11-08-key-grab-started)). **Not installed on c3-a:** unreachable (no ping, no SSH) at 00:25Z and 06:39Z; c3-a stays on 0.6.0.

## 0.6.0 (2026-11-07)

- **Remote key grab ([ADR 0016](docs/decisions/0016-remote-key-grab.md)).** With the mic button feature on (`[remote] mic_button = "ptt"`), a helper (`bin/remote-proxy`, new in the package) takes an exclusive grab of the Magic Remote's input device and re-sends every event except the mic key into an idle sibling device, so LG's toast and LG's assistant do not react to the mic key. It runs under ADR 0016's four-layer dead-man and releases the grab and every held key on feature off, stop, crash and uninstall. ADR 0013's in-memory configd override stays as the safety net.
- **Off unless the mic button is on.** With `mic_button = "off"` (the default) nothing of it runs: no helper process, no grab. `key_grab = "auto" | "off"` in `[remote]` only applies with the mic button on.
- **Firmware:** the grab is used only on firmware verified by EXP-026: `03.30.74` (c3-a). On any other firmware (c3-b's `03.30.73` included) the grab stays off and the configd override alone is used; `ctl.sh status` reports `key_grab` with the reason.
- Not yet run with the mic button on, on a TV: the owner's acceptance test is in [next steps](docs/project/next-steps.md).
- Package sha256 `1cf220c39d9b8d609a0586001d9372b258c6285085f87cd3a72f58ee16425805`; installed on c3-a and c3-b on 2026-11-07 with the mic button off (no helper running, no WARN or ERROR; [c3-a P4](docs/discovery/c3-a/P4-permanent-install.md#update-to-060-2026-11-07-remote-key-grab-off), [c3-b P4](docs/discovery/c3-b/P4-permanent-install.md#update-to-060-2026-11-07)).
