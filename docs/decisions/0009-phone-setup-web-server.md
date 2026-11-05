# ADR 0009: A phone setup web server on the TV, started from the TV app

- **Status:** Accepted (2026-11-05, owner). **Reverses**, for this one feature, the recommendation "Option B is not built" of [install and configuration UX sections 3.2 and 4](../design/install-and-config-ux.md#32-option-b-embedded-web-config-server-plus-a-qr-code-on-the-tv) and reuses its threat model (updated in [phone setup, section 13](../design/phone-setup.md#13-threat-table-and-the-security-review-gate)). Design only: nothing is implemented, nothing was run on a TV.
- **Scope:** a "Set up with your phone" button in the redesigned TV app (version 0.4.0). It shows a QR code with a one-time URL to a small web server on the TV. The phone page offers **Home Assistant setup help and a short list of settings**; every change from the phone needs one OK on the TV remote. Design: [Phone setup](../design/phone-setup.md). Backlog: [#15](../project/backlog.md#15-set-up-with-your-phone-phone-setup-web-server).
- **Related:** [ADR 0001](0001-esphome-native-api.md), [ADR 0008](0008-user-initiated-lg-settings.md), [install and configuration UX](../design/install-and-config-ux.md), [Voice and power screen](../design/voice-and-power-screen.md), [packaging](../design/packaging.md), [AGENTS.md](../../AGENTS.md) (device safety and privacy).

## Context

- Option C (no UI, HA-driven pairing) plus the TV app covers the happy path, but the app is driven by a remote control: it cannot show a comfortable "add this to Home Assistant" page with copy buttons, and a slider or a list of settings is slow to use with arrow keys. The owner wants the phone to do that, and keeps the TV as the place where a change is approved.
- The assessment in install-and-config-ux 3.2 ("no secret left to protect, so a root LAN parser protects almost nothing") still holds as a security fact. What changed is the value side: the owner accepts a small, short-lived, authenticated listener in exchange for a usable setup experience, with the threat model kept and the exposure bounded as below.
- Everything the phone can do already exists as a whitelisted `ctl.sh` command run by the TV app as root through the Homebrew Channel (`set always-ready|hi-lg|remote-off`, `mic on|off`, `pair`), plus one new command (`set reply-volume`). The server adds a network front door to that list, not new powers.

## Decision

1. **A separate, small, std-only Rust binary** (`webos-satellite-setup`, its own crate) serves the phone page. It is **not** part of the daemon: the daemon stays single-threaded with its stall watchdog, its 64 MiB RSS ceiling and the Noise key, and never parses HTTP.
2. **Started only by an explicit app action** (`ctl.sh setup start`, through the Homebrew Channel exec like the app's other actions), **never at boot**, never by the daemon. Lifetime **10 minutes at most**; it ends on Done, on timeout, on a parse error, and when the TV app stops sending heartbeats (the confirmation needs the app open).
3. **Random high port, plain HTTP, no TLS.** Justified only because **no secret passes**: the page never shows or accepts the Noise key, an HA login, an HA token or any HA data. Everything else is protected by: a one-time token of at least 128 bits in the QR URL path (consumed on first request, exchanged for an HttpOnly, SameSite=Strict session cookie), one session at a time, a per-session CSRF token plus a custom header on writes, no CORS, tight request limits, one connection at a time, and a peer-address filter.
4. **Every change needs one OK on the TV remote.** A pending phone request appears as an overlay in the TV app; OK allows it, Back refuses it, a timeout refuses it. Only one request is pending at a time. Reads (the status mirror) need no OK.
5. **Actions map 1:1 onto whitelisted `ctl.sh` commands** run by the server with a fixed argument list (no shell, no free text): Always Ready on/off, Hi LG on/off, keep-listening on/off, microphone on/off, start the pairing window, and the reply volume (a new `ctl.sh set reply-volume`, applied by the running daemon through a reload trigger). **The device name is not changeable from the phone** (see the design, section 9.3); HA already renames devices.
6. **Logging:** the peer address and events only; never the token, the cookie, the CSRF token, a request path or a body.
7. **A security review gate before release** (backlog #3): the 0.4.0 release is not tagged before the checklist in the design, section 13.2, is done and recorded.

## Consequences

- Owners get a phone page for HA setup help and settings without a typed-text path into the TV: nothing typed on the phone reaches a command, only enumerated actions with validated values.
- A root process now listens on the LAN, for up to 10 minutes, after an explicit local action. The exposure is bounded by the token, the TV overlay and the short life; a bug in the HTTP code would be root code execution, so the server is std-only (plus `serde_json`, already in the lock file), `forbid(unsafe_code)`, accepts a tiny HTTP subset, is fuzz-tested, and is reviewed before release.
- One more binary per architecture in the IPK (estimated 0.3 to 0.6 MB, to be measured), one more file set in `install.sh`, `build_ipk.py` and the packaging tests.
- The TV app must keep an app-wide poll (every 2 s, 1 s while a request is pending; interval set by [EXP-012](../experiments/EXP-012-phone-setup-probe.md), which measured 7.7 % of a core at 1 Hz) while a session is active; that poll is also the heartbeat. If the TV app is closed or the TV goes to standby the server ends itself within about 15 s.
- Phone and TV must reach each other (same network, no client isolation). The server cannot detect a blocked path; the TV app shows a hint after 45 s with no scan. Inbound connections on random high ports are `verified-on-target` on c3-a from a same-LAN PC (EXP-012: no firewall); a phone and other networks are not tested.
- The TV app's "no free text reaches `ctl.sh`" rule gets one documented extension: `setup answer <id> allow|refuse`, where `<id>` is eight lowercase hex characters read from the server's own status (checked by a regular expression in the app and again in `ctl.sh`).

## Alternatives considered

- **Keep Option B unbuilt** (the old recommendation): rejected by the owner; the TV-remote-only path is too slow for settings and does not offer copy buttons for HA.
- **The server inside the daemon, or the same binary in a `--setup-server` mode:** rejected, see the design, section 3 (the first puts an HTTP parser next to the microphone and the key, the second links the whole daemon for a feature that needs 1 percent of it and widens the review scope).
- **HTTPS with a self-signed certificate:** rejected; the phone would show a warning that trains users to click through, and the QR code cannot carry a pin that phones honour (`hypothesis`). No secret passes, so TLS would protect little.
- **A typed second-factor code on the phone** (the old 3.2 mitigation): not adopted; the OK on the TV for every write plus the peer address on the overlay cover the same threat with less typing.
- **The phone drives the TV app directly** (the app fetching from the server on loopback): rejected for the first version; polling `ctl.sh setup status` reuses the one trusted channel (exec) and adds no second authenticated interface.
- **Busybox `httpd` with CGI scripts or Python's `http.server`:** rejected; availability on other TVs is unknown and both put untrusted input into an interpreter.

## Revisit when

- The on-target probe shows that inbound connections to a random high port are blocked on the TV (then a fixed, documented port or another route is needed).
- A real user needs a typed setting that cannot be an enumerated action (a device name, a custom config value): that needs a new decision, a longer review, and a typed-input policy.
- A TLS route that phones accept without warnings becomes practical (for example a name under a public domain with a certificate; not assumed).
- The security review finds a defect that std-only parsing cannot fix cheaply.
