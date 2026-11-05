# Phone setup: a QR code on the TV, a small web server, every change approved on the TV

- **Status:** design, accepted by the owner on 2026-11-05 ([ADR 0009](../decisions/0009-phone-setup-web-server.md)). **EXP-012 Part A was run on c3-a on 2026-11-05** ([results](../experiments/EXP-012-phone-setup-probe.md): no firewall, inbound high ports reachable from the LAN, exec survival and polling cost measured, no log leakage, web engine by static evidence); **Part B, TV side, was run with package 0.4.0 on 2026-11-05** ([EXP-012 Part B](../experiments/EXP-012-phone-setup-probe.md#part-b-the-tv-side-with-package-040-2026-11-05-verified-on-target): the server, the heartbeat through `exec`, the listener, the files, no token leak, SR-03 and the HTTP checks from a PC are verified on c3-a); the owner's phone test is **pending**: **his first attempt (2026-11-05) failed** because the phone did not send the session cookie after the QR redirect; the server was fixed ([section 6.7](#67-fix-after-the-owners-first-phone-test-2026-11-05)) and the retest is pending. The rest of the TV statements (a phone, other VLANs, the app's web view, the overlay with the real remote) stay `hypothesis` until the owner's part of Part B of the probe in [section 15](#15-on-target-probe-checklist-exp-012-part-a-read-only-first) is run with the owner's authorization.
- **Date:** 2026-11-05. TV facts are for c3-a (LG C3, webOS TV 8.3.0, firmware `03.30.74`, Homebrew Channel 0.6.3) and c3-b (firmware `03.30.73`) where the repository already has them; other models are not covered. External research date: 2026-11-05 (HA core `dev` branch files read on that date, see [Sources](#sources)).
- **Version:** planned for 0.4.0, together with the redesigned TV app (four screens: Home, Voice and power, Home Assistant, Diagnostics; its mockup is made separately).
- **Related:** [install and configuration UX](install-and-config-ux.md) (section 3.2 is the threat model reused here, section 5 the pairing window), [Voice and power screen](voice-and-power-screen.md) (the string tables and the outcome matrix, `window.voicePower`), [packaging](packaging.md), [ADR 0008](../decisions/0008-user-initiated-lg-settings.md), backlog [#15](../project/backlog.md#15-set-up-with-your-phone-phone-setup-web-server) and [#3](../project/backlog.md#3-install-and-configuration-ux).

## 1. Scope

**In scope (owner, 2026-11-05):** Home Assistant setup help and settings, from a phone, with every change approved on the TV.

| Area | What the phone page does |
| --- | --- |
| Home Assistant setup help | Shows the TV's address and port with copy buttons, offers the pairing window (an action, approved on the TV), explains the steps in HA, links to HA's ESPHome add flow, and shows "paired" when HA has connected ([section 10](#10-add-to-home-assistant)) |
| Settings (changes) | Always Ready on/off, LG hands-free "Hi LG" on/off, "keep listening after the power button" on/off, microphone on/off, reply volume, start the pairing window ([section 9](#9-actions-and-what-each-one-runs)) |
| Read-only status | The same facts as `ctl.sh status --json` and `ctl.sh tv-settings --json`, through an allow-list, with the same power-button sentence the TV app shows ([section 8.4](#84-get-apistate)) |

**Not in scope:** typing any text into a command (no device name, no HA URL, no key); showing or accepting the Noise key, an HA login or token, or any HA data; start, stop, update, uninstall, autostart, reset pairing, report, firmware acknowledgement; playing a sound (no "test sound" button: no sound is played without the owner's authorization); remote access from outside the LAN; more than one phone at once.

**What the TV app gets** (built in the redesign, [P7](#16-implementation-steps)): a "Set up with your phone" button, a screen with the QR code, the state line and a Done button, and a confirmation overlay.

## 2. Evidence and labels

Labels as in [AGENTS.md](../../AGENTS.md). "Repo code" means read in this repository at 0.3.0 (not a device claim).

| Claim | Label | Source |
| --- | --- | --- |
| The daemon reads `etc/reply_volume` only at startup; Home Assistant's media-player volume is the only runtime writer (`on_volume`, which also stores the file); `AppCommand::SetVolume` at runtime updates HA's media-player state without being echoed back | repo code | `crates/satellite/src/volume.rs`, `daemon.rs` (`on_volume`, startup `volume::load`), `crates/esphome-api/src/app.rs`, `server.rs` |
| The `remote_off` reload (`run/remote-off-reload`, polled once a second with `consume_trigger`) and the `lg-settings-refresh` trigger exist and are the pattern for a reply-volume reload | repo code | commit `f018c91`, `daemon.rs` (`poll_compat_triggers`), [Voice and power section 6.5](voice-and-power-screen.md#65-how-the-running-daemon-picks-up-a-remote_off-change-without-a-restart) |
| The MAC that HA uses as the device identity is derived from `mac_seed` (or from `name` when no seed or MAC is set); the packaged config always sets a random `mac_seed` | repo code | `crates/esphome-api/src/config.rs` (`Mac::derive`), `packaging/webos/config.default.toml` |
| HA's ESPHome integration, on a connect where the MAC matches but the device name differs, overwrites the stored device name silently and continues; it does **not** change the entry title or entity ids; HA does not pass an expected name to the connection. A different MAC is an "Unexpected device" error | documented (HA core `dev`, `homeassistant/components/esphome/manager.py`, read 2026-11-05); **not run** on the owner's HA 2026.10.x | [Sources](#sources) |
| HA's ESPHome config flow `async_step_user` takes `host` and `port` only from the submitted form; nothing reads a URL parameter or a flow context to prefill them (reconfigure and discovery set them internally) | documented (HA core `dev`, `config_flow.py`, read 2026-11-05) | Sources |
| `https://my.home-assistant.io/redirect/config_flow_start/?domain=esphome` starts the add-integration flow for ESPHome | documented (an older ESPHome getting-started guide uses it as its badge link); the current My Home Assistant page for `config_flow_start` was fetched but lists no parameters, so the `domain` parameter is **not confirmed on the current page**; not run | Sources |
| `navigator.clipboard` is available only in secure contexts (HTTPS or localhost), so a plain-HTTP phone page needs the `document.execCommand("copy")` fallback | documented (MDN Clipboard API; recalled, not re-fetched on 2026-11-05) | |
| The daemon survives the Homebrew Channel `exec` that starts it (`setsid nohup`, no inherited descriptors) | observed with the daemon on c3-a and c3-b (`start.sh`, autostart); **not observed for a second process** | [packaging](packaging.md), [P4 c3-a](../discovery/c3-a/P4-permanent-install.md) |
| The kill switch lives in `/tmp` and is lost at reboot, so `/tmp` is volatile on the TV | repo docs (verified with the kill switch) | [packaging](packaging.md), `lib.sh` |
| Whether the TV's firewall accepts inbound TCP on a random high port | **verified-on-target** (EXP-012, c3-a, firmware `03.30.74`, 2026-11-05, `Active`, TV on Wi-Fi): `iptables` has no rules and policy ACCEPT, no ip6 tables; a PC on the same /24 connected to random ports in 49152-65535 and 32768-49151, a low port, and (IPv6) a global address. **Not tested:** a phone, another VLAN or guest network, other power states | [section 15](#15-on-target-probe-checklist-exp-012-part-a-read-only-first) |
| The web engine of the TV app on webOS 8 (the app is ES5, with `qrcodegen.js` needing ES2015) | **hypothesis**, with static evidence (EXP-012, c3-a): `libcbe.so` holds `94.0.4606.128` beside the app-shell code and `Chrome/87.0.3945.79` in LG's Crow browser part, so the app runtime is read as Chromium 94 and ES2015+ and `fetch` are expected; `navigator.userAgent` and key codes are read in Part B | section 15 |
| Tools on c3-a: `python3` 3.8.13, `nc`, `netstat`, `ip`, `iptables`, `ip6tables` (no tables), `timeout`, `setsid` exist; **`ss`, `nft`, `tcpdump` and busybox `httpd` do not; python3 has no `http.server`, `socketserver`, `pty` or `resource`** | verified-on-target (EXP-012) | [EXP-012](../experiments/EXP-012-phone-setup-probe.md) |
| Network layout of c3-a: one IPv4 address (Wi-Fi `wlan0`, the default route), `eth0` without carrier, `p2p0` without IPv4; LG services already listen on `0.0.0.0`; ephemeral range 32768-60999 | verified-on-target (EXP-012, 2026-11-05) | EXP-012 |
| A detached `setsid` process started by HBC `exec` survives the call's return and the HBC service's idle exit (the service exits 20 to 30 s after the last call; the process had `ppid` 1, cgroup `ls-hubd.service`); the app closing, an HBC restart and standby were not tested | verified-on-target (EXP-012) | EXP-012 |
| Exec round trip on c3-a: `true` 40 ms, `ctl.sh --help` 80 ms, `ctl.sh status --json` 950 ms (about 0.57 core-seconds per call); a light call costs about 77 ms CPU in total (7.7 % of one core at 1 Hz, 3.8 % at 0.5 Hz); no failures at 1 s or 2 s | verified-on-target (EXP-012) | EXP-012 |
| The exec `command` string and stdout are not retained in `/var/log`, `/var/luna`, `/tmp/pmlogdaemon`, the journal, `logread` or `dmesg` | verified-on-target for those places (EXP-012) | EXP-012 |

## 3. Process model

### 3.1 Decision: a separate, small, std-only binary `webos-satellite-setup`

A new workspace crate `crates/setup-server` builds the binary `webos-satellite-setup` for armv7 and aarch64 musl (static, like the daemon). It is installed beside the daemon in `/var/lib/webos-satellite/bin/`, verified by `install.sh` the same way (`--version` must print its own target triple), and started only by `ctl.sh setup start`.

| Option | For | Against | Verdict |
| --- | --- | --- | --- |
| **A. A task or thread in the daemon** | no extra process, direct access to daemon state | the daemon is a single-threaded loop with a stall watchdog (`limits.loop_stall_s`) and a 64 MiB RSS ceiling; an HTTP parser would share an address space with the Noise key, the microphone and the HA session, and a bug or a slow client could trip the watchdog and restart the voice path; the listener would need a runtime switch inside the voice process | rejected |
| **B. The same binary in a `--setup-server` mode** | one artifact per architecture, no packaging change | the process links tokio, the audio, DSP, AEC and TV-control crates for a feature that needs a small fraction of them; the security review scope and dependency audit would cover the whole daemon; one binary to patch for both concerns; RSS is by touched pages, so the saving in memory is small, but the attack surface and the review scope are not | rejected |
| **C. A separate std-only binary (chosen)** | process isolation (a crash or a hang ends only the setup session); the review and audit scope is one small crate; no tokio, no audio, no key access (the process never opens `etc/noise_psk` or the config); can use its own resource limits (`nice`, descriptor limit) and its own log; the daemon is untouched | one more artifact per architecture in the IPK (estimated 0.3 to 0.6 MB, `hypothesis`, measured in [P8](#16-implementation-steps)), a second `--version` check in `install.sh` | **chosen** |
| D. `busybox httpd` with shell CGI, or Python `http.server` | no new binary | availability on other TVs unknown; untrusted input would go through an interpreter or `sh`; hard to bound memory and lifetime | rejected |

**RSS budget (target, not measured):** under 4 MiB resident at idle and under 8 MiB during a session, one thread for the HTTP loop and one worker thread for commands. The daemon's own ceiling stays untouched. **Crash isolation:** the daemon never depends on the server; if the server dies, `ctl.sh setup status` reports `ended` with reason `crashed` and the TV app says so.

### 3.2 Why a second thread

The HTTP loop must never wait for a command: `tv-settings --json` can take 12 s in the worst case (two reads over a pseudo-terminal, [Voice and power section 6](voice-and-power-screen.md#6-ctlsh-interface)), and an LG write plus read-back about 20 s. A **worker thread** runs the commands from a queue of one (state refresh or an approved action); the HTTP loop only reads a cached state. The loop ticks every 100 ms (non-blocking accept, one connection at a time) to check the lifetime, the heartbeat, the answer file and the pending timeout.

### 3.3 Not at boot, only on an explicit action

- `ctl.sh setup start` is the only way to start it. The boot hook and `ctl.sh start` never start it (a packaging test greps for it, [section 14](#14-tests)).
- The TV app calls it from the "Set up with your phone" button (a fixed whitelisted string). SSH can call it too.
- The server refuses to start without the install marker, and it exits by itself in every end condition of [section 4.2](#42-states-and-end-conditions).

### 3.4 Privileges

The server runs as root, because it is started by `ctl.sh` through the Homebrew Channel exec and because the commands it runs need root (`luna-send`, files in `etc/`). It does not drop privileges (a user to drop to, and who owns `etc/`, is not known; not assumed). Hardening therefore relies on what it does not do: no shell, no file serving, no config or key access, a fixed argument list per action, a cleared environment for children, and a short life. A seccomp filter or a chroot is a later option, not a design assumption.

## 4. Lifecycle, files and `ctl.sh setup`

### 4.1 Directory and files

The session state lives in a **volatile** directory so that the token never reaches the flash: `/tmp/webos-satellite-setup/` (mode 0700, root; `WEBOS_SAT_SETUP_DIR` overrides it in tests). This is a deliberate difference from `run/`, which is under the persistent prefix. A reboot removes it.

| File | Written by | Content |
| --- | --- | --- |
| `setup.pid` | `ctl.sh setup start` | the server's pid; liveness is checked by `/proc/<pid>/cmdline` naming `webos-satellite-setup` |
| `state.json` | the server (atomic: temporary file and `rename`, mode 0600), on every change and every 2 s | the session state, the one the TV app reads ([section 4.3](#43-ctlsh-setup-interface)) |
| `heartbeat` | `ctl.sh setup status` (touch) | the server compares the mtime with the one it saw last; **a change counts as a heartbeat on the server's own monotonic clock**, so a clock step on the TV (NTP) cannot end or extend a session |
| `answer.json` | `ctl.sh setup answer` (atomic) | `{"id":"a1b2c3d4","verdict":"allow"}`; the server consumes and deletes it |

The server's log goes to `/var/lib/webos-satellite/log/setup.log` (mode 0600; `ctl.sh setup start` truncates it above 64 KiB). Log lines, in English, carry only events and the peer address ([section 6.6](#66-logging)).

### 4.2 States and end conditions

```
start -> waiting (listening, QR shown, token unused)
waiting -> active (the token was consumed: a phone has a session)
waiting|active -> ended (reason)
```

| End reason (`reason` in `state.json`) | Trigger |
| --- | --- |
| `done` | the phone's Done button (`POST /api/session/end`) or the TV app's Done (`ctl.sh setup stop`) |
| `timeout` | 10 minutes after the start (hard limit, not extended by activity) |
| `idle` | after the token was consumed, no request from the session for 180 s |
| `app-gone` | no heartbeat for **15 s** (the TV app is closed, in the background, or the TV left `Active`); a pending request is refused |
| `parse-error` | a request that is not valid HTTP within the accepted subset, oversize, or with an invalid encoding (owner's rule: any parse error ends the session; see [open question 2](#17-open-questions)). A TLS handshake on the port is not a parse error ([6.4](#64-limits-all-enforced-before-any-command-runs)) |
| `too-many-refusals` | three requests refused or expired on the TV in one session |
| `too-many-bad-requests` | 20 unauthenticated requests (wrong token, no or wrong cookie) |
| `crashed` | the server process disappeared without writing `ended` (found by `ctl.sh`) |

On any end the server closes the listener first, deletes `state.json`'s token path, writes `ended`, removes `answer.json`, and exits. The TV app keeps showing the last `ended` state until the user leaves the screen.

### 4.3 `ctl.sh setup` interface

New commands (root, install lock only for `start`), all with fixed arguments. The app's whitelist gets exactly these strings and the one pattern of `setup answer`.

```
setup start            start the server; prints the state JSON (below). Refuses when not installed,
                       not root, or when a session is already active (state "active");
                       a session still "waiting" is returned as it is (the app was reopened)
setup status --json    print state.json (+ liveness) and touch the heartbeat; "none" when no server
setup answer ID allow|refuse
                       answer the pending request ID (8 lowercase hex characters, validated here)
setup stop             end the session (reason "done"): touch <dir>/stop, wait up to 2 s for the server
                       to exit (it polls the file every 100 ms), TERM it if it is still there, then remove
                       the directory; prints the "none" state JSON, exit 0 also when nothing runs
```

`setup status --json` output (schema 1). `path` exists only while `state` is `waiting`; the app builds the URL from its own address knowledge (`http://<tvAddress>:<port><path>`, [section 5.4](#54-the-address-in-the-qr-code)):

```json
{
  "schema": 1,
  "state": "waiting",
  "reason": "",
  "port": 49731,
  "path": "/s/q3V2...43-characters...",
  "expires_in_s": 587,
  "peer": null,
  "peer_other_subnet": null,
  "pending": null,
  "last_result": null
}
```

After a phone connects: `state` `active`, `path` removed, `peer` `"192.168.1.23"` (an IP literal only). While a request awaits the OK:

```json
"pending": {"id": "a1b2c3d4", "action": "hi-lg", "value": "off", "peer": "192.168.1.23", "timeout_in_s": 47}
```

and after it is answered: `"last_result": {"id": "a1b2c3d4", "action": "hi-lg", "value": "off", "verdict": "allowed", "result": "applied", "error": ""}`. `verdict`: `allowed`, `refused`, `expired`, `aborted` (session ended first). `result` is the `result` field of the `ctl.sh set` JSON (`applied`, `already`, `mismatch`, `failed`, `refused`), or `ok`/`failed` for commands that print no JSON, or `""` when not run. `error` is the one-line error already allowed by the Voice and power design, never a reply body.

Exit status as the other ctl.sh commands: 0 printed, 1 failed, 2 usage. `setup status` exits 0 whenever it printed JSON.

**As built in P3 (`ctl.sh`, tests `packaging/webos/tests/setup.sh`):** the token is made by the server alone and written only to `state.json` (field `path`, mode 0600, tmpfs); `ctl.sh` creates and passes none (the server gets `--ctl <prefix>/bin/ctl.sh` as its only argument and `WEBOS_SAT_SETUP_DIR` and `WEBOS_SAT_PREFIX` in its environment). `setup status --json` is the app's once-a-second poll and stays light (EXP-012: `ctl.sh status --json` costs about 950 ms on c3-a): no root check, no luna, no daemon, no `ctl.sh status`, no `cat`, `touch`, `tr` or `grep` processes (shell built-ins; the heartbeat is a truncating write of the empty `heartbeat` file, which sets its mtime; the pid is checked through `/proc/<pid>/cmdline` read with `read`; a host test wraps those commands and `luna-send` and asserts that none runs). It prints the server's `state.json` unchanged (the server does the allow-listing); without a session it prints the same keys with `"state":"none"`, and `"state":"starting"` if the process runs but has not written the file yet; a process that vanished without writing `ended` is reported once as `"state":"ended","reason":"crashed"` and `state.json` is rewritten without the path. `setup start` stays `waiting`-idempotent, refuses while `active` and replaces a leftover directory. `stop_daemon` in `lib.sh` also ends a session, so `ctl.sh stop`, `restart`, `install` (update), `uninstall`, `purge` and `stop.sh` all do. Notes of `setup stop` go to stderr so that stdout is the JSON alone. `pending` carries `confirm` and `state` (`pending` or `running`), `last_result.verdict` can be `auto` (microphone off, applied without a TV confirmation), as the server documents; `ctl.sh` passes them through.

### 4.4 Heartbeat

The TV app polls `setup status --json` **every 2 s while a session is `waiting` or `active`, and every 1 s while a request is `pending` or its result has not been shown yet, from any screen** (a pending request must surface wherever the user is), and stops when the state is `ended`. The poll is the heartbeat. Nothing else keeps a session alive, so a closed app, a suspended web view or a TV that left `Active` ends it in at most 15 s. **Measured on c3-a** (EXP-012 P7, 2026-11-05): an exec call of a light command takes about 80 ms and about 77 ms of CPU in total (HBC node, `ls-hubd`, `luna-send`), so 1 Hz costs about 7.7 % and 0.5 Hz about 3.8 % of one core, with no failed call; the HBC service stays resident (about 31 MB) while polled and exits 20 to 30 s after the last call. `ctl.sh status --json` takes 950 ms and about 0.57 core-seconds per call, **so `setup status` must not be built like it**: it reads `state.json` (and the heartbeat file) only, with no luna call and no run of the daemon binary, and must stay near the 80 ms of `ctl.sh --help` (to be re-measured when P3 lands). The 2 s default cuts the cost in half; 1 s is kept for the short moments where the user waits for the TV (a pending request) and the 15 s grace is still seven missed beats.

## 5. Network

### 5.1 Listener

- **IPv4 only**, `0.0.0.0` on a port the kernel picks (`bind` to port 0, then read it back: a random high port from the ephemeral range, never fixed, never 6053). Reasons for IPv4 only: the QR URL carries an IPv4 literal (the app already shows the TV's address), IPv6 would add privacy addresses and scope ids for no benefit, and the ESPHome listener's default bind is `0.0.0.0` as well.
- **Peer filter:** the server accepts a connection only from private, link-local or (in tests) loopback addresses: `10/8`, `172.16/12`, `192.168/16`, `169.254/16`. Anything else is closed at once without a response and counts as a bad request. This keeps a port forward or a public address of the TV from reaching it. Carrier-grade NAT or tailnet addresses (`100.64/10`) are not accepted in 0.4.0.
- **`Host` check (DNS rebinding):** the `Host` header must equal `<local address of the accepted socket>:<port>` exactly (an IP literal). A hostname is refused. The phone opened an IP-literal URL, so this always matches for a real use.
- One listener, `listen` backlog 4, closed the instant the session ends.
- The server does **not** try to choose the interface (`std` has no interface list). **Decided after EXP-012 (c3-a): bind `0.0.0.0`.** The TV has one IPv4 address (Wi-Fi), the listener on `0.0.0.0` was reachable from the LAN, and LG services already listen on `0.0.0.0` on this TV; the peer filter and the `Host` check do the narrowing. IPv6 stays unbound: the TV's global IPv6 address answered a test listener on `::`, so the IPv4-only listener is what keeps the setup port off IPv6. A TV with Ethernet and Wi-Fi at once (c3-b?) has not been probed.

### 5.2 If the TV's firewall blocks inbound connections

**c3-a: no host firewall** (EXP-012: `iptables` empty, policy ACCEPT, no ip6 tables; random high ports reachable from a PC on the same /24, `verified-on-target`); c3-b and other firmware are unknown. If a filter blocks random high ports on another TV, the options (decided after the probe, not before) are: a documented fixed port added to the allow list by the user, the daemon's own port 6053 reused with a different route (rejected: it would put HTTP into the daemon), or declaring the feature unsupported on that TV with a clear message. The probe in section 15 lists exactly what to measure.

### 5.3 Phone and TV on different networks (clear failure hint)

The server cannot see a blocked path: a phone that cannot reach the TV never connects. So the hint is on the TV app's phone screen, shown **45 s after the start while the state is still `waiting`**, in English:

> Nothing has opened the link yet. Your phone must be on the same network as this TV. A guest network, a different VLAN, a Wi-Fi "client isolation" setting, or a VPN on the phone will block it. This TV: 192.168.1.50 (port 49731).

If a phone does connect from an address outside the TV's own `/24` (a heuristic: the netmask is not known to the server), the state carries `"peer_other_subnet": true` and the TV screen adds "The phone connected from a different network (10.0.2.5). That is unusual: check who it is." It is informational; the TV overlay still shows the peer for every request. The static docs for this feature repeat the hint.

### 5.4 The address in the QR code

The QR URL is `http://<TV IPv4>:<port>/s/<token>`. The TV app already finds the TV's address through `connectionmanager/getStatus` (`tvAddress` in `app.js`); the server only knows its port and path. If the app cannot find an address, it shows the path and port and says so (and no QR). The URL is under 80 characters, so the existing `qrcodegen.js` draws it at low error correction (the compatibility-report QR is longer).

## 6. Authentication, session and request hygiene

### 6.1 Token

- 32 random bytes from `/dev/urandom` (256 bits; the requirement is at least 128), base64url without padding (43 characters), generated by the server at start. If `/dev/urandom` cannot be read the server exits (fail closed).
- Lives in server memory, in `state.json` while `waiting` (tmpfs, 0600) and in the app's memory. It is never on the persistent flash, never in a log, an argument or the environment of any process.
- **Single use:** `GET /s/<token>` is the only request that can consume it (compared in constant time). `HEAD` and other methods get 405 and do not consume it. **A speculative load does not consume it either** (2026-11-05, [section 6.7](#67-fix-after-the-owners-first-phone-test-2026-11-05)): a `GET` with `Sec-Purpose: prefetch…`, `Purpose: prefetch`, `X-Purpose: preview` or `X-Moz: prefetch` gets `503 prefetch`, is logged as `rejected prefetch token` and does not count as a bad request. A second `GET` of the same URL gets `410 Gone` ("this link was already used; press Start again on the TV"). A wrong token gets the same `404` as every other unknown path.
- Consuming it creates the session and removes `path` from `state.json`, so the QR code disappears from the TV (the app shows "Phone connected from <peer>" with a Disconnect button). **If a link preview, a scanner app or a second person consumed the token first, the owner sees a different peer on the TV and can disconnect; the cost of a consumed token is a restart.**

### 6.2 Session cookie

- The response to the token request sets `sat_session=<32 random bytes, base64url>; Path=/; HttpOnly; SameSite=Strict; Max-Age=600` and answers with a small **200 page that moves on to `/` itself** (`<meta http-equiv="refresh" content="0; url=/">` and a Continue link), so the token does not stay in the address bar. **Not a `302`** (changed 2026-11-05, [section 6.7](#67-fix-after-the-owners-first-phone-test-2026-11-05)): a link opened from another app is a cross-site navigation for the cookie rules, and the `Strict` cookie set on a redirect is then not sent to `/`. `Secure` cannot be set on plain HTTP (a `__Host-` prefix is therefore not usable).
- **One session at a time.** After the token is consumed, a second token request is `410`; there is no way to start a second session without a restart on the TV.
- The session is **bound to the peer IP** of the token request. A request with the right cookie from another IP is refused (`403`) and logged. `hypothesis` (probe): an IPv4 phone does not change its source address within a Wi-Fi session; a phone switching between Wi-Fi and mobile data would lose the session, which is acceptable and gives a restart hint.
- Cookie value comparison is constant-time. The server stores the value only in memory.

### 6.3 CSRF, content type and cross-origin

- `GET /api/session` (cookie required) returns the per-session CSRF token (16 random bytes, hex). Every `POST` must carry `X-Setup-CSRF: <token>` and `Content-Type: application/json`. Without them: `403` (CSRF) or `415`. Because the header is custom and the content type is JSON, a cross-origin page cannot send it without a CORS preflight, and **the server never answers a preflight or sends any `Access-Control-*` header** (`OPTIONS` gets `405`).
- If an `Origin` header is present it must equal `http://<Host>`; if `Sec-Fetch-Site` is present it must be `same-origin` or `none`. Otherwise `403`.
- The cookie is `SameSite=Strict`, so a cross-site navigation does not carry it.

### 6.4 Limits (all enforced before any command runs)

| Limit | Value |
| --- | --- |
| Request line and headers | 4 KiB in total, at most 24 headers; no folded headers, no `Transfer-Encoding`, no `Expect`, no `Upgrade`, no pipelining, `HTTP/1.1` only |
| Body | `Content-Length` required on `POST`, at most 1 KiB, exactly one such header, no body on `GET` |
| Time | 3 s to read a whole request (headers and body), 3 s to write the response; slower is `408` and the connection is closed |
| Connections | one at a time; the next is accepted after the current one closes; every response carries `Connection: close` |
| Requests | at most 20 per second and 2000 per session |
| Response size | at most 128 KiB (embedded assets are far smaller) |
| Unauthenticated requests | 20 per session, then the session ends (`too-many-bad-requests`) |

Parse errors end the session ([section 4.2](#42-states-and-end-conditions)). **Exception (2026-11-05):** a connection whose first bytes are a TLS handshake record (`0x16 0x03`, a browser trying `https://` on this port, for example an HTTPS upgrade or "always use secure connections") is closed without a response, logged as `rejected tls`, and is neither a parse error nor a bad request, so it never ends a session ([section 6.7](#67-fix-after-the-owners-first-phone-test-2026-11-05)). Every other byte outside the subset stays a parse error.

### 6.5 Why plain HTTP is acceptable

No secret passes. The page never contains and the API never accepts the Noise key, an HA URL, login, token, or any HA data; the status mirror is allow-listed and carries no address, serial, name, key, log line or path ([section 8.4](#84-get-apistate)). What a passive sniffer on the LAN or the same Wi-Fi **can** see: the token request (single use, already consumed when the sniffer could replay it), the session cookie and CSRF token of every later request, and the page data. A sniffer that copies the cookie is rejected by the peer binding unless it also spoofs the phone's IP (possible only for an active attacker on the same segment), and even then **every write still needs the owner's OK on the TV, which shows the peer and the action**. The remaining risk is stated in the threat table. TLS was rejected because the only available certificate would be self-signed (warnings that train users to click through; no pinning route for ordinary phones, `hypothesis`) and it would protect nothing that matters here.

### 6.6 Logging

One line per event in `setup.log`, with the peer address: `session start port=49731`, `token consumed peer=…`, `request GET api-state peer=…` (a route name, **never the raw path**, which contains the token on the first request), `rejected csrf peer=…`, `action requested id=… action=hi-lg value=off peer=…`, `answer allow|refuse`, `action result id=… result=applied`, `session end reason=…`. Never logged: the token, the cookie, the CSRF token, request paths, headers, bodies, response bodies. Command output is reduced to the allowed `result` and `error` fields.

**Added 2026-11-05 (diagnosis of the first phone test):** `token consumed` and every refused request without a valid session also carry `site=` and `browser=`, two fixed words derived from headers, never the header text:

- `site=` is `Sec-Fetch-Site` reduced to `none`, `same-origin`, `same-site`, `cross-site`, `other` or `absent`;
- `browser=` is the `User-Agent` family, with no version and no device: `chrome`, `safari`, `firefox`, `samsung`, `edge`, `opera`, `google-app`, `android-webview`, `ios-webview`, `curl`, `other` or `absent`.

The refusal word now says why there is no session: `no-session` (no phone has used the token yet), `no-cookie` (the request had no `Cookie` header), `no-session-cookie` (a `Cookie` header without `sat_session`), `wrong-session` or `wrong-peer`. Also logged: `rejected tls` and `rejected prefetch token`.

### 6.7 Fix after the owner's first phone test (2026-11-05)

**What happened** (c3-a, 0.4.0, the owner's phone on Wi-Fi, phone model and browser unknown; evidence from `log/setup.log`, which holds no token, `verified-on-target`): two attempts, each with the same sequence. The QR code was scanned and the token was used from the phone's address (`token consumed`). Within a second the phone followed the redirect, and its `GET /` was refused with `rejected no-session page`. Two later reloads were refused the same way. The QR code disappeared on the TV (the session was `active`), and the phone showed the "Session ended" page. The TV app's `Done` then ended the first attempt (`done`), and leaving the app ended the second (`app-gone`).

Not seen in the log: no parse error, no TLS, no `HEAD`, no `410` (`token-used`), no second address and no refused origin. A prefetch, a scanner's preview from another client, an HTTPS upgrade, peer binding and the budgets are therefore ruled out for this phone.

**Cause** (`hypothesis`, strongly supported):

- The phone sent no `sat_session` cookie at all after the token, although it had just received one.
- A link opened from another app (the camera or a QR scanner) is treated as a cross-site navigation by Chrome and Firefox on Android. Such browsers store a `SameSite=Strict` cookie set during that navigation, but do not send it on the redirect to `/`, nor on a reload of that page. They still send `Sec-Fetch-Site: none`, which is why the server's origin check passed.
- A host emulation with Chromium reproduces the c3-a log exactly with the 0.4.0 server: a link from another site, with `Sec-Fetch-Site` rewritten to `none` by a proxy, gives `token consumed`, then `no-session page` on the redirect and on the reload.
- The fixed server, in the same emulation, gets the cookie on the page's own navigation, `Sec-Fetch-Site: same-origin`, and on reloads.
- Not confirmed on the owner's phone: its browser is unknown.

**Fix (server 0.4.x):**

- The token's answer is a 200 page that sets the cookie and navigates to `/` itself, by a meta refresh and a link. That navigation is started by the TV's own origin, so the `Strict` cookie goes with it. The cookie stays `SameSite=Strict`: no rule was relaxed.
- The "no session" and "link already used" pages get a same-origin link to `/` ("try again", "continue"). A phone that holds the cookie, but reached a page in a cross-site context, can go on with one tap. Anyone else still gets the 403 page.
- Hardening for other phones: the TLS exception in [6.4](#64-limits-all-enforced-before-any-command-runs), the prefetch rule in [6.1](#61-token), and the log words above.

Security properties are unchanged:

- the token is single use and `GET` only;
- the cookie is `HttpOnly`, `SameSite=Strict` and bound to the peer;
- CSRF, `Origin`, `Sec-Fetch-Site` and `Host` are checked as before;
- the continue page carries the same CSP (a meta refresh is not script);
- a TLS record or a prefetch can neither use the token nor reach a route.

Tests:

- `session::tests::token_is_single_use_and_sets_a_strict_cookie` (the 200 page, its refresh and link, the log words);
- `a_prefetch_never_uses_the_token_and_is_not_a_bad_request`;
- `a_tls_hello_ends_nothing_and_counts_nothing`;
- `a_page_without_the_cookie_logs_why`;
- `http::tests::a_tls_client_hello_is_not_a_parse_error`;
- `prefetch_headers_user_agent_and_fetch_site_words`;
- the integration test `tls_hellos_and_prefetches_end_nothing` (a real socket: 30 ClientHellos before and after the token, two prefetches, then the page).

## 7. TV confirmation

1. The phone posts an action ([section 8.5](#85-post-apiactions)). The server validates it against the action table ([section 9](#9-actions-and-what-each-one-runs)), allows **one** pending request at a time (`409 busy` otherwise), makes up an id (4 random bytes as 8 hex characters) and writes `pending` into `state.json`.
2. The TV app's one-second poll sees `pending`, and shows an **overlay** in its own words: the action text comes from the **app's own string table keyed by `action` and `value`** (built from the confirm texts of the Voice and power design, plus new ones for the microphone, the reply volume and the pairing window), plus the peer address (an IP literal, validated by the app). An unknown action or value, or a `peer` that is not an IP literal, is refused automatically (`ctl.sh setup answer <id> refuse`). The overlay says: "OK to allow, Back to refuse", and counts down from `timeout_in_s`.
3. **OK** (Enter) calls `ctl.sh setup answer <id> allow`; **Back** (the Back key, 461, and Esc or Backspace as in the app's other screens) calls `... refuse`. Nothing else answers: not the phone, not a timeout (a timeout refuses).
4. The server reads `answer.json` (every 200 ms while a request is pending), checks the id matches the pending request, and either hands the action to the worker (allow) or finishes with `refused`.
5. **Request timeout: 60 s** from the post. A request not answered in that time is `expired` (the phone shows "the TV did not answer"). After a refusal or an expiry there is a 3 s cooldown before the next request; three refusals or expiries end the session.
6. While an allowed action runs (up to 25 s), the pending slot stays taken (`state` `running` on the phone, the overlay shows "Applying…"). The result goes into `last_result`; the TV app shows it as the existing result line and re-reads its own screens.
7. If the heartbeat is lost, the session ends, the pending request becomes `aborted`, and nothing runs.

The overlay needs the app open, which is why the heartbeat is part of the design. The answer path is the same root exec primitive as every other app action: another app on the TV that can run `ctl.sh` as root could answer too, which is the existing trust boundary (install-and-config-ux 3.1), not a new one.

## 8. HTTP API

HTTP/1.1, `Connection: close`, UTF-8 JSON bodies (`Content-Type: application/json; charset=utf-8`). Every response carries `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`; HTML responses add `Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. No CORS headers are ever sent. Errors are `{"error":"<code>","message":"<English sentence>"}`.

### 8.1 Routes

| Method and path | Needs | Purpose | Success | Errors |
| --- | --- | --- | --- | --- |
| `GET /s/<token>` | token | consume the token, set the cookie | `200`, a page that moves on to `/` ([6.7](#67-fix-after-the-owners-first-phone-test-2026-11-05)); `302` to `/` for the same phone with its cookie | `404` (unknown or wrong), `410` (already used), `503` (a prefetch, not consumed) |
| `GET /` | cookie | the phone page (HTML) | `200` | `403` page "No session in this browser", with a same-origin "try again" link to `/` and the hint to scan again and to use the browser, not a scanner app (no cookie or wrong; the title was "Session ended" until 2026-11-05), `404` otherwise |
| `GET /assets/<name>` | cookie | embedded JS and CSS (`phone.js`, `voicepower.js`, `phone.css`) | `200` | `404` |
| `GET /api/session` | cookie | the CSRF token and the remaining time | `200` | `403` |
| `GET /api/state` | cookie | the status mirror | `200` | `403` |
| `POST /api/actions` | cookie, CSRF | request one action | `202` | `400`, `403`, `409`, `415`, `422`, `429`, `503` |
| `GET /api/actions/<id>` | cookie | state of a request | `200` | `403`, `404` |
| `POST /api/session/end` | cookie, CSRF | the Done button | `200` | `403` |

Any other path: `404`. A known path with a wrong method: `405` with `Allow`. Oversize body `413`, oversize headers `431`, slow request `408`, bad `Host` or `Origin` `400` or `403`. (`429` is the 3 s cooldown and the 20 per second limit; `503` is "the TV app is not answering", i.e. the heartbeat is late by more than 8 s.)

### 8.2 `GET /api/session`

```json
{"schema": 1, "csrf": "9f3c...32 hex characters", "expires_in_s": 540, "idle_timeout_s": 180}
```

### 8.3 Cookie and token exchange

`GET /s/<token>` response: `200`, `Set-Cookie: sat_session=…; Path=/; HttpOnly; SameSite=Strict; Max-Age=600`, and a small HTML page (`assets/continue.html`, the usual CSP and headers) with `<meta http-equiv="refresh" content="0; url=/">` and a Continue link to `/`. Until 2026-11-05 this was a `302` to `/`, which phones that opened the link from another app followed without the cookie ([6.7](#67-fix-after-the-owners-first-phone-test-2026-11-05)).

### 8.4 `GET /api/state`

A cached merge of two commands the worker refreshes **only while the phone page is polling** (SR-03, changed from "every 3 s for the whole session"), plus a refresh after each action: `ctl.sh status --json` (package state) and `ctl.sh tv-settings --json` (LG settings and microphone). **The server copies an allow-list, never the whole output** (no `prefix`, `log`, `pid`, `firmware` string, `name`, `uid`, HA peer, key detail):

```json
{
  "schema": 1,
  "age_s": 1.2,
  "device": {
    "installed": true, "installed_version": "0.4.0", "running": true,
    "key": "paired", "port": 6053,
    "reply_volume": 40, "reply_volume_default": 40,
    "pairing_window_s": 600,
    "pairing": {"open": false, "ends_in_s": null},
    "ha": {"state": "connected", "since_s": 42}
  },
  "tv": {
    "luna": true,
    "always_ready": {"state": "off", "raw": "off", "error": ""},
    "hi_lg": {"state": "off", "thinq": false, "amazon": false, "dangbei": false, "error": ""},
    "remote_off": {"state": "keep_listening", "active": true},
    "mic": {"state": "listening", "kill_switch": false},
    "power_state": "Active",
    "last_change": null
  },
  "pending": null
}
```

- `key` is the existing `ctl.sh status` word (`fixed`, `paired`, `unpaired`, `cleared`, `none`); the page uses it to decide whether to offer the pairing window.
- `reply_volume` is the stored value, or `reply_volume_default` when none is stored; `pairing_window_s` comes from the config's `window_s` (the remaining time is `pairing.ends_in_s` of `ctl.sh status --json`, from `run/voice.json`, [open question 7](#17-open-questions)).
- `ha` (P3, `run/voice.json` through `ctl.sh status --json`) is the Home Assistant connection: `state` is one of `connected`, `disconnected`, `unpaired` or `unknown` (else `null`) and `since_s` a whole number of seconds (at most ten years, else `null`). **Nothing else is copied**: no address, name, version or peer of Home Assistant (test `mirror::tests::ha_connection_is_allow_listed`). `pairing` is `{"open", "ends_in_s"}` as in `status --json`.
- **When the refresh runs (SR-03, as built):** only while a phone has a session **and** `/api/state` was asked for within the last **10 s** (the use of the token counts as a poll, so the first refresh runs at once). The pause between two refreshes is **5 s** from the end of the previous one (the page polls every 3 s and may see the same cache twice); the refresh after an action waits at least 2 s after the previous one. A hidden or closed page therefore costs the TV nothing (the page stops polling when hidden; the session then ends at the 180 s idle limit). Test switches `--refresh-s` (at most 5) and `--poll-window-s` (at most 10). Consequence: an action's precondition (section 9.1) is checked against the cache, which may be older than 5 s only when the page has not polled for more than 10 s; the page polls again as soon as it is visible.
- A command that fails or times out leaves its part `null` with `"error"`, never a guess. `age_s` tells the page how old the cache is.
- The page computes the power-button sentence with the **shared** `window.voicePower.outcome(...)` ([section 11](#11-static-assets-and-the-shared-string-tables)) from `tv` and `device`, the same function the TV app runs.

### 8.5 `POST /api/actions`

Request: `{"action": "<id>", "value": "<value>"}`; the id and value come from the table in section 9 and nothing else is accepted (unknown keys, other types, duplicate keys: `400`).

Response `202`: `{"id": "a1b2c3d4", "state": "pending", "timeout_s": 60}`.

Errors: `400 bad_request` (shape or unknown action or value), `403 csrf` or `no_session`, `409 busy` (a request is pending or running), `415 unsupported` (content type), `422 not_applicable` (a precondition of the table failed, with a `message` such as "The pairing window is only for an unpaired TV"), `429 rate_limited` (3 s cooldown), `503 app_gone` (no heartbeat for 8 s: the TV app cannot show the overlay, so nothing is queued).

### 8.6 `GET /api/actions/<id>`

```json
{"id": "a1b2c3d4", "state": "pending", "timeout_in_s": 47, "result": null}
```

`state`: `pending` (waiting for the OK), `running`, `done`, `refused`, `expired`, `aborted`. With `done`: `"result": {"result": "applied", "error": ""}` using the `result` vocabulary of [section 4.3](#43-ctlsh-setup-interface). Only the latest request is kept, for 60 s after it finished; another id is `404`. The page polls this every second while `pending` or `running`.

### 8.7 `POST /api/session/end`

Body `{}`; response `200 {"state":"ended","reason":"done"}`; the server closes after sending it.

## 9. Actions and what each one runs

### 9.1 The table

The server holds a fixed table. For each row it builds the argument list itself; the value is checked against the listed set before it is placed in the list, so no request byte reaches a command except a validated integer.

| Action id | Values | Runs (argv, no shell) | Precondition (server, from the cached state) | TV overlay says (app string table) |
| --- | --- | --- | --- | --- |
| `always-ready` | `on`, `off` | `ctl.sh set always-ready on|off` | `luna` true and `always_ready.raw` is `off` or `alwaysReady` (`ctl.sh` also refuses otherwise) | the Always Ready confirm text of the Voice and power design |
| `hi-lg` | `on`, `off` | `ctl.sh set hi-lg on|off` | `luna` true; thinQ is the only provider the toggle changes | the Hi LG confirm text |
| `remote-off` | `normal`, `keep` | `ctl.sh set remote-off normal|keep` | none | the Keep listening confirm text |
| `mic` | `off`, `on` | `ctl.sh mic off|on` (the kill switch) | none | "Turn the microphone off?" / "Turn the microphone on? The satellite will listen again." |
| `pair` | none | `ctl.sh pair` | daemon running and `key` is `unpaired` or `cleared` (a paired device is not offered a window) | "Open the pairing window for 10 minutes? Anyone on your network who connects first can claim this TV while it is open." |
| `reply-volume` | integer 0 to 100 as a decimal string, at most 3 digits | `ctl.sh set reply-volume N` (new, [9.2](#92-reply-volume)) | none | "Set the reply volume to N %?" (0 adds "Replies and timer alarms will be silent.") |

Excluded on purpose: start, stop, restart, install, update, uninstall, purge, autostart, reset pairing, report, self-check, firmware acknowledgement, and any device name or config value. These stay on the TV (or SSH). Commands run with `env_clear` plus a fixed `PATH`, stdin from `/dev/null`, stdout limited to 64 KiB, and a 25 s limit (the worker kills the child; the result is `failed` `timeout`). The commands' own history log (`etc/tv-settings.log`, ADR 0008) still records `source=ctl`; the setup log adds the peer.

### 9.2 Reply volume

**Findings (repo code, 0.3.0):** the reply volume is stored as a whole percent (0 to 100) in `sink.volume_file`, which resolves to `etc/reply_volume` in the package config. The daemon reads it **only at startup** (`volume::load`); at runtime the file is written only by the daemon when **Home Assistant** sets the volume (`on_volume`: gain `0.5 dB per percent`, 100 % = 0 dB, 0 % silent; then `volume::store`). A file written by something else is **ignored until a restart**, and HA's media-player slider would not follow. The `remote_off` change (commit `f018c91`) shows the pattern; `AppCommand::SetVolume(percent)`, used at startup to publish the stored value, also works at runtime (it refreshes HA's media-player state and is not echoed back as a change).

**Design (one new ctl.sh command and one daemon trigger, step [P2](#16-implementation-steps); built as written, `Daemon::reload_reply_volume`, trigger `run/reply-volume-reload`):**

1. `ctl.sh set reply-volume N` (digits only, 0 to 100, else exit 2): read the current stored value (`etc/reply_volume`, or the config default); equal means `already`, no write. Otherwise write the percent with a temporary file and `mv` (the daemon's own atomic way, mode 0600), create the trigger `run/reply-volume-reload`, wait up to 3 s for the daemon to consume it, and print one JSON line like `set remote-off`: `{"schema":1,"setting":"reply-volume","requested":"30","before":"40","after":"30","result":"applied","daemon":"applied","error":""}` (`already` has `"daemon":"none"`; a write failure is `failed` with `error` `cannot write the file`, exit 1; the value must match `0|[1-9][0-9]?|100`, else exit 2; the file is `sink.volume_file` of the config, default `etc/reply_volume`) (`daemon` is `pending` when the trigger was not consumed in 3 s and `not running` when stopped; the next start reads the file). One history line is appended to `etc/tv-settings.log` as for `remote-off`.
2. The daemon's once-a-second trigger poll also consumes `reply-volume-reload`, calls `volume::load` and, for a valid value that differs from the current one, sets the shared gain (`volume_gain.set(gain_for(p))`, so a reply already playing changes too), sends `AppCommand::SetVolume(p)` so HA's media player follows, and logs `reply volume P % (…) set by ctl.sh`. An unusable file keeps the current value with one `warn` (as `reload_remote_off`). It does **not** write the file back (already there). A consumed trigger with an unchanged value is a `debug` line.
3. The phone page offers a slider in steps of 5 and the text of the dB mapping from `volume.rs` ("40 % is -30 dB"); 0 shows "silent: replies and timer alarms make no sound" (the daemon already warns about a 0 % alarm).

No sound is played by any of this. A first real run on a TV would not play a test reply either.

### 9.3 Device name

**What changing the ESPHome device name does to the existing HA entry** (documented from HA core source, **not tested** on the owner's HA 2026.10.x; [Sources](#sources)):

- The HA entry is keyed by the **MAC** (unique id). The satellite derives the MAC from `mac_seed`, which the packaged config always sets, so a name change keeps the identity. If a user's config has neither `mac` nor `mac_seed`, the MAC is derived **from the name**: a rename would then create a new device in HA. The setup server cannot assume the seed exists.
- On the next connect with the same MAC and a new name, HA silently overwrites the stored device name in the entry data and continues; the entry title stays, the entity ids stay (they live in HA's entity registry); the device registry name follows `friendly_name`. The mDNS instance name changes too (mDNS is off by default). HA's zeroconf step with the same MAC only updates host and port.
- The name is read from `config.toml` at startup, so a change needs a **config write and a daemon restart** (a few seconds without HA connection and without the microphone). A shell edit of TOML from a web request is the riskiest piece of text handling in the whole feature, for a setting that HA already lets the user rename in its own UI (device page, "rename").

**Decision: the device name (`device.name`) is not changeable from the phone in 0.4.0, and neither is `friendly_name`.** The name is the satellite's identity string (also the base of HA's default entity names, a unique id for mDNS and the key-exchange logs), and renaming is done in HA. `friendly_name` is a possible later action `friendly-name` (a restricted character set, at most 32 characters, one config key rewrite by `ctl.sh set friendly-name`, a restart, the TV overlay showing the new text) but only with an owner request and a TOML-writing test; it is [open question 4](#17-open-questions). The name is not in the status allow-list (section 8.4), so the phone page does not show it either.

## 10. "Add to Home Assistant"

What the phone page shows, in this order (all text in the page, English):

1. **State line** from `device.key`: not paired, paired, or unpaired by Home Assistant. When `paired` the page says so and offers nothing more here.
2. **"Open the pairing window"** (an action; the TV asks for OK; only when `key` is `unpaired` or `cleared` and the daemon is running). The page then says it is open for up to `pairing_window_s` seconds from the OK (the daemon's own window; the exact remaining time is not known to `ctl.sh` today).
3. **Address and port with copy buttons.** The address is `location.hostname` of the page (the IP the phone used to reach the TV, which is by construction an address the phone can reach; HA must reach it too, which is the same-network assumption) and the port is `device.port` (6053 by default; the server does not know the HA-facing port otherwise). `navigator.clipboard.writeText` needs a secure context and **plain HTTP is not one**; the page uses `document.execCommand("copy")` on a selected, read-only text input as the primary route here, falls back to "touch and hold to copy" with the text pre-selected, and uses the Clipboard API only if `isSecureContext` is true (documented, section 2).
4. **A link that opens HA's add flow:** `https://my.home-assistant.io/redirect/config_flow_start/?domain=esphome` (opens in a new tab with `rel="noopener noreferrer"`). **Finding:** this link goes to HA's "add integration" flow for ESPHome (the `domain` parameter is documented through ESPHome's older getting-started guide and **not confirmed on the current My Home Assistant page**; labelled `documented, unconfirmed on the current page`, to be tested once on the owner's phone, probe item P11). **Host and port cannot be prefilled:** the ESPHome config flow's user step takes them only from the submitted form, nothing reads a URL parameter or flow context (HA core `dev` source, 2026-11-05; `documented`). So the page says: "In Home Assistant: Settings, Devices & services, Add integration, ESPHome. Paste the host and the port." The page does not ask for or store the user's HA address: the My redirect uses the instance URL saved in the phone's own browser (documented by My Home Assistant).
5. **Step text for the flow after the link:** "Host: [copy] 192.168.1.50 Port: [copy] 6053. With Home Assistant 2026.9 or newer no key is needed while the window is open; Home Assistant stores the key itself." (the existing app text, [packaging](packaging.md)).
6. **Done detection:** the page polls `/api/state`; when `key` becomes `paired` it shows "Paired with Home Assistant". It cannot know that the HA entry is configured otherwise (no HA data is read).

Not offered: a second route that types the key, an HA URL field, an "open my HA" link built from a typed URL, and mDNS (`mdns = false` by default).

## 11. Static assets and the shared string tables

- **Served from the binary, not from files.** The server has a fixed route table and `include_bytes!`ed assets from `packaging/webos/app/phone/` (`phone.html`, `phone.js`, `phone.css`) and the shared model file. There is **no filesystem serving**, so there is no path traversal surface, no dependency on the app package's location (which changes with updates and is found through `app-dir`), and the page always matches the binary's API. The cost (a rebuild for a text change) is accepted; the IPK build already rebuilds both for a release. Size budget: under 60 KB of assets uncompressed.
- **One model file for both screens.** The strings and the outcome matrix move out of `app.js` into **`packaging/webos/app/voicepower.js`**: ES5, no DOM, a UMD wrapper that sets `window.voicePower` in a browser and `module.exports` in Node (the existing `tests/app_matrix.js` then requires it directly instead of reading `app.js`). It holds exactly what `window.voicePower` exports today (`outcome`, `rows`, `resultMessage` and the `POWER`, `LISTEN`, `NOTE`, `POWER_EXTRA` tables) plus the confirm texts and the new action texts of section 9. The TV app loads it with a `<script>` tag from its package directory; the server embeds the **same file** with `include_str!("../../../packaging/webos/app/voicepower.js")`, so there is one copy in the repository. A test fails if the server crate's embedded file differs from the app's (it is the same path, so the test checks that the include path exists and the build copies nothing).
- **Phone page code is separate.** `phone.js` is modern JavaScript for a current mobile browser (fetch, `async`, CSS grid); it only reads `window.voicePower`. The TV app stays ES5. Rule for `voicepower.js`: **ES5 only** (the webOS 8 web engine is read as Chromium 94 from static evidence on c3-a, EXP-012, so ES2015 would work, but it is not run-tested; the rule stays ES5 as a margin for other firmware, `qrcodegen.js` needs ES2015 and is already used).
- **No inline script or style, no remote assets, no fonts, no analytics.** The CSP of section 8 enforces that. The page is usable at 360 px width and in a dark theme (`prefers-color-scheme`), large touch targets, no hover-only controls; it works without cookies being readable by script (HttpOnly) and says what to do when cookies are blocked (some QR apps open links in an embedded view that blocks them).
- **Coordination with the redesign.** `app.js` is being restructured; the extraction ([P5](#16-implementation-steps)) must be done inside the redesign's Voice and power screen work or right after it, not in parallel on the old `app.js`. The contract the redesign must keep: **the strings and the matrix live in `voicepower.js`**, pure and ES5.

### 11.1 As built in P5 and P7 (TV app, 2026-11-05)

Host-tested only (`tests/app_smoke.js`, `tests/app_matrix.js`, shared fake DOM `tests/app_fake.js`); nothing was run on a TV.

- **`voicepower.js`** (ES5, UMD, no DOM, checked with `acorn --ecma5`): the matrix (`outcome`, `rows`, `resultMessage`, `confirmText`, `wordOf`, `rowKind`, `arInfo`, `hlInfo`, `roInfo`); tables with the **short and the full wording in one entry**: `POWER[cell]` = `{headline, detail, short, tone}`, `LISTEN[cell]` = `{full, short, detail, tone}` (the 0.3.0 tables were plain strings), `KEY[word]` (Home Assistant card) and `COMPAT[name]` (firmware notice and tag), `ROWS` (labels, LG/Satellite tag, sub-lines, action ids), `NOTE`, `POWER_EXTRA`; card helpers `haSummary(status)` (uses `pairing.ends_in_s`; the key word, for the Home Assistant screen), `haCard(status)` (the Home card: `ha.state` connected, disconnected, unpaired or unknown from `HA_CONN`, with an open pairing window and "not set up" winning) and `SETUP_UNAVAILABLE` (the app shows it when `ctl.sh setup start` finds no server binary, an ipk built without it) and `compat(status)`; phone requests: `REQUEST[action][value]` = `{ask, detail, done}` and `requestText(action, value)` (null for anything not in section 9, including `reply-volume` outside `0|[1-9][0-9]?|100`; `value` `null` is `pair`), `phoneResult(last_result)` (the result line, from `verdict` and `result`; only `error` is taken from the server), `endedText(reason)`, `isIPv4(s)`. The tones are `ok`, `warn`, `bad`, `idle`, `accent`. The CSS custom properties `--t-*` in `style.css` are the tokens the phone page reuses.
- **Overlay:** shown only for `pending.confirm` true and `pending.state` `pending`, with a valid 8-hex id, a known action and value and an IPv4 `peer`; otherwise a `pending` request is refused with `setup answer <id> refuse` without asking (once per id), and `confirm` false or `state` `running` shows no overlay (`running` shows "From the phone: ... Changing..."). The overlay wins over everything on the screen below: it disarms any armed row or two-press button, the arrows move only between Allow and Refuse, OK on Allow allows, Back (461, 27, 8) refuses. **OK is ignored for 700 ms after the overlay opens** (`ALLOW_GUARD_MS`; found in the 0.4.0 overlay review: a press or a held key meant for the screen below, or a double press, could otherwise allow at once); refusing is always possible. The only code that answers `allow` is the click of the Allow button (a test reads the source for it). **The app refuses by itself when its own 60 s countdown ends** (in addition to the server's expiry; an answer for a request the server already expired is ignored there). Microphone off from the phone (`verdict` `auto`) gives the line "Microphone turned off from the phone." and no overlay, per the owner decision in [12.1](#121-as-built-in-p4-cratessetup-server-2026-11-05).
- **Poll:** `SETUP_POLL_MS` 2000 and `SETUP_POLL_FAST_MS` 1000 (section 4.4, EXP-012): fast from the moment a `pending` is seen (or the app answered) until a new `last_result.id` is shown. One poll at app start finds a session that is still running; no poll otherwise. A new result is detected by its `id`. After a result the app reads `tv-settings --json` again for the setting actions and updates the microphone from the result itself; it **does not** call `status --json` beyond the existing 10 s refresh (about 950 ms per call, EXP-012).
- **QR screen** (Home Assistant, "Set up with your phone"): the URL is built only when `state` is `waiting`, `port` is an integer, `path` matches `^/s/[A-Za-z0-9_-]{16,128}$` and the app's address is an IPv4 literal; QR at medium error correction. `active` greys the code ("Code used") and names the phone, with the different-network warning when `peer_other_subnet` is true; the 45 s hint as in 5.3. **Done** runs `setup stop` (ends the session) and returns to the Home Assistant screen; the remote's Back returns to Home Assistant (its parent) and leaves the session running (the poll and the overlay go on from every screen; the top bar shows "Phone code open" or "Phone connected" and the Home Assistant screen offers "End phone session"). An `ended` state shows `endedText(reason)` and "Start again".
- **Re-check when P3 lands on main:** the field names of `setup status --json` (coded against this document and the P4 contract: `state`, `reason`, `port`, `path`, `expires_in_s`, `peer`, `peer_other_subnet`, `pending.{id,action,value,peer,state,confirm,timeout_in_s}`, `last_result.{id,action,value,verdict,result,error}`), that `setup stop` prints the JSON last on stdout, and the `starting` state.
- **As built in P6 (phone page, 2026-11-05):** the phone page lives in `crates/setup-server/assets/` (`phone.html`, `phone.js`, `phone.css`, `no-session.html`, `link-used.html`) and is served from the binary through a fixed table (`assets.rs`), with a compile-time budget of 80 KiB in total. **`voicepower.js` is not copied**: `assets.rs` includes `packaging/webos/app/voicepower.js` itself (`include_bytes!`), so the TV app and the phone page run the same file and the same tables and matrix; the page loads every asset as `/assets/<name>`. The page uses `textContent` only, no inline script or style (CSP `default-src 'none'`). Host tests: `crates/setup-server/tests/phone_logic.js` (node, fake DOM) and the asset tests in `tests/server.rs`. Not run on a phone yet.
- **Overlay review (0.4.0 candidate, 2026-11-05):** the overlay never allows by itself (no timer or error path answers `allow`; the 60 s end, Back and every unknown, malformed or unsafe request answer `refuse`); the text comes only from `requestText(action, value)` (own-property lookup in app tables, `reply-volume` checked by regex) plus the peer, shown only when it is an IPv4 literal; the id is `^[0-9a-f]{8}$` and `ctl` accepts only the exact `setup answer <8 hex> allow|refuse`. The one change was the 700 ms guard above. The result line shows the server's one-line `error` text (printable ASCII, at most 160 characters, set with `textContent`); that is the only server text on the TV.
- **Back key (fix after 0.4.0 on c3-a, 2026-11-05):** on the TV the default runtime handling of Back closed the app from any screen. Per the [webOS TV "Back button" guide](https://webostv.developer.lge.com/develop/guides/back-button) the runtime handles Back through the DOM history unless `appinfo.json` sets `"disableBackHistoryAPI": true`; the app then gets a `keydown` with `keyCode` 461 and decides. `build_ipk.py` now writes that flag (and `check` asserts it). Routing: overlay open -> refuse; sub-screen (phone code, report, more actions) -> its parent (Home Assistant, Diagnostics, Diagnostics); any other screen -> Home; on Home -> leave the app with `webOS.platformBack()` when webOSTV.js is loaded (this app does not ship it; webOS TV 6.0 and later then ask whether to exit, 5.0 and lower open the launcher), otherwise `window.close()`, which the same guide documents for an app's own exit. Only 461 leaves the app; Escape and Backspace (desktop testing) do nothing on Home. Host-tested (`app_smoke.js`, `ipk.sh`); **not yet verified on a TV** (the exit behaviour of `window.close()` on webOS 8 is from the documentation only).
- **Fonts:** no font file is vendored (a font is a third-party binary under the repository rules, and the TV has no network); the stack starts with LG's system font (`LG Smart UI`) and falls back to common sans fonts. The mockup's Atkinson Hyperlegible Next and IBM Plex Mono are only named as later fallbacks.

## 12. Implementation language and dependencies

- **Rust, std-only HTTP/1.1** for the server: a hand-written parser for the subset of section 6.4 (one request per connection, no chunked bodies, no keep-alive, no upgrade). It is smaller and easier to audit than a general parser because it rejects everything outside the subset; a dependency such as `httparse` (MIT or Apache-2.0, no dependencies) is the fallback if the hand parser grows past roughly 300 lines or the review prefers it.
- **Dependencies:** `serde_json` and `serde` (already in the lock file through `webos-tv-control` and `webos-satellite`; `MIT OR Apache-2.0`, pure Rust, audited in [packaging section 8.1](packaging.md#8-license-decision-and-dependency-audit)), and nothing else. No `tokio`, no `hyper`, `axum`, `tiny_http`, `rustls`, `ring` (those add roughly a megabyte, many packages and a larger review for one connection at a time). Randomness from `/dev/urandom` through `std::fs` (fail closed); constant-time comparison is a ten-line fold; child processes through `std::process::Command`; descriptor and priority limits through `rustix` (already a workspace dependency; its `process` features are already enabled). `#![forbid(unsafe_code)]` through the workspace lint.
- **Build:** `cargo zigbuild --release -p webos-satellite-setup --target armv7-unknown-linux-musleabi` and `aarch64-unknown-linux-musl` with the existing flags, `opt-level = "z"`, `lto`, `strip`; the dlmalloc allocator is not needed (a session is short). **Size:** estimated 0.3 to 0.6 MB per architecture (`hypothesis`; measured in P8; budget 0.7 MB). Licence of the crate: GPL-3.0-or-later like the workspace; no new third-party license enters the notice file (the same external packages as the daemon's subset).
- The dependency audit of packaging section 8.1 is repeated for the new crate's tree (`cargo metadata` for the package, `cargo deny check licenses` and `cargo audit`, both installed locally) and recorded in P8.

### 12.1 As built in P4 (`crates/setup-server`, 2026-11-05)

Host tests only; nothing was run on a TV. Where the build differs from or adds to the sections above:

- **Owner decisions (2026-11-05):** (1) **microphone off from the phone runs without the OK on the TV** (the safe direction): `POST /api/actions` answers `202 {"state":"running"}`; while it runs `pending` has `"confirm":false,"state":"running","timeout_in_s":null` (the TV app shows no overlay); the result is `last_result` with `"verdict":"auto"` (the TV app shows a short notice). It is exempt from the 3 s cooldown and the `503 app_gone` check and does not count as a refusal. Microphone on and every other action still need the OK. (2) **Done on the TV ends the whole session** (`ctl.sh setup stop`): `ctl.sh` creates `<dir>/stop`; the server polls it every 100 ms, also inside a slow client's read, then closes the listener, writes `ended`/`done`, deletes `answer.json` and exits (well under a second in the host test). A removed directory ends the session the same way. The server installs no signal handler (no `unsafe`), so `SIGTERM` is an immediate exit without the final state write; `ctl.sh` then reports the end itself.
- **Command line** (no config file; the server never reads the satellite's config or key): `--prefix DIR` (default `$WEBOS_SAT_PREFIX`, else `/var/lib/webos-satellite`; must hold `.webos-satellite-install`, else exit 1), `--ctl FILE` (default `PREFIX/bin/ctl.sh`), `--dir DIR` (default `$WEBOS_SAT_SETUP_DIR`, else `/tmp/webos-satellite-setup`; created 0700, or accepted only as a real directory owned by the same user and forced to 0700), `--log FILE` (default `PREFIX/log/setup.log`, appended, 0600, at most 256 KiB per run), `--bind IPV4` (default `0.0.0.0`, decided by EXP-012, open question 5), `--version` (`webos-satellite-setup <version> (<target triple>)`). Test switches can only shorten the design values: `--lifetime-s`, `--idle-s`, `--heartbeat-grace-s`, `--request-timeout-s`, `--command-timeout-s`, `--cooldown-s`, `--refresh-s`, `--poll-window-s`, and `--allow-loopback` (peers in `127/8`). All paths must be absolute. Exit status: 0 after any end condition, 1 when it cannot start, 2 usage. Nothing secret is in the argv or the environment: the token, cookie and CSRF token are drawn from `/dev/urandom` at start (failing closed) and the token is written only to `state.json` `path`.
- **Parse errors** (open question 2, owner default): before the token is used, the third one ends the session; after it is used, the first one does. A connection that sends no byte at all (closed, or silent for 3 s: a browser's speculative connection) gets no response and is neither a parse error nor a bad request; a request cut off before its end is a parse error (`truncated`). The request target is restricted to `[A-Za-z0-9-._~/]`, at most 128 characters (no query, no percent-encoding): anything else is a parse error. Also refused: `HTTP2-Settings`, a duplicate `Host`, `Content-Length`, `Content-Type`, `Cookie`, `Origin`, `Sec-Fetch-Site` or `X-Setup-CSRF`, a body on any method but `POST`, a `Content-Length` with a sign or leading zero, bare CR or LF, and any byte outside visible ASCII, space and tab in the head.
- **Bad requests** (the budget of 20): wrong token, a second use of the link, no or wrong cookie, the right cookie from another address, a wrong `Host`, a cross-origin `Origin` or `Sec-Fetch-Site`, a CSRF failure, an unknown path or wrong method without a session, a filtered peer and every parse error. Rate-limit answers (`429`) do not count.
- **Additions:** the same phone opening the used link again **with its valid cookie** is sent to `/` (`302`, no new cookie) instead of `410`; the `Origin` and `Sec-Fetch-Site` checks apply to every request, not only writes; HTML responses also carry `X-Frame-Options: DENY`; the cookie's `Max-Age` is the remaining lifetime (at most 600). `state.json` keys come out sorted (serde_json), compact on one line; `pending` has `id`, `action`, `value` (`null` for `pair`), `peer`, `state` (`pending` or `running`), `confirm` and `timeout_in_s`; `last_result` also has `peer`. An action still running when the session ends is reported with its verdict, `result` `""` and `error` `the session ended before the result was known` (the child is not killed). `GET /api/state` puts the error strings of a failed part in `"errors": {"device": "...", "tv": "..."}` next to the `null` part, carries `pending` without `peer`, and copies `device.pairing` (`{"open", "ends_in_s"}`, from P2's `status --json`).
- **Children:** `ctl.sh setup start` starts the server itself with `env -i`, so that its environment is only `PATH` (`/usr/sbin:/usr/bin:/sbin:/bin`), `WEBOS_SAT_PREFIX` and `WEBOS_SAT_SETUP_DIR` (review C2, tested with hostile variables in the caller's environment); the commands it runs get the allow-list below, and on a TV none of those five variables is set. `ctl.sh` runs with `env_clear`, `PATH=/usr/sbin:/usr/bin:/sbin:/bin`, `WEBOS_SAT_PREFIX` from `--prefix` and only `WEBOS_SAT_INITD`, `WEBOS_SAT_LOCK`, `WEBOS_SAT_ALLOW_NONROOT`, `WEBOS_SAT_LUNA_SEND` and `WEBOS_SAT_SETTLE` passed through (tests and the PC end-to-end run of P9 need them; none is secret), working directory `/`, stdin `/dev/null`, stderr discarded, in its own process group; at the time limit the whole group is killed. The listener and accepted sockets are close-on-exec (a host test checks that `ctl.sh` holds no socket). `mic` and `pair` report `ok`, or `failed` with `exit status N`; `set` reports the `result` of its JSON line.
- **Assets (P6, as built):** the real phone page in `crates/setup-server/assets/` (`phone.html`, `phone.js`, `phone.css`, `no-session.html`, `link-used.html`); the shared `voicepower.js` is included from `packaging/webos/app/voicepower.js` by `assets.rs` (no copy); the compile-time check allows 80 KiB in total (the design said 60 KB before `voicepower.js` grew to about 26 KB). See [11.1](#111-as-built-in-p5-and-p7-tv-app-2026-11-05).
- **Size** (release, `opt-level = "z"`, LTO, stripped, static musl, version 0.3.0): armv7 551,716 bytes, aarch64 530,480 bytes, within the 0.7 MB budget. RSS and CPU on a TV are not measured (EXP-012 P8 and Part B).

## 13. Threat table and the security review gate

### 13.1 Threat table (updated from [install-and-config-ux 3.2](install-and-config-ux.md#32-option-b-embedded-web-config-server-plus-a-qr-code-on-the-tv))

Assets: the TV's root control plane (the server runs as root; a defect is root on the TV), the microphone state and the pairing window (who can receive audio), the LG settings, and the owner's attention at the TV. **Not assets here:** the Noise key (never shown, accepted or readable by this process), HA credentials and HA data (none touched).

| Threat | Notes | Mitigation in this design |
| --- | --- | --- |
| LAN attacker finds the open port | a random port on `0.0.0.0` is scannable for the session's life | explicit start only, never at boot; random port; closed on every end condition; 10 minutes at most; peer filter to private ranges; no listener the rest of the time (checked by an `nmap` run in the review) |
| Guessing the token | online brute force | 256-bit token, single use, constant-time compare; 20 unauthenticated requests end the session; one connection at a time and 20 requests per second make 2^256 unreachable |
| Passive sniffer on Wi-Fi or LAN sees the token, cookie or page | plain HTTP | no secret is on the page or in the API; the token is already consumed when seen; the cookie is bound to the peer IP; every write needs the OK on the TV with the peer address shown; the session lasts at most 10 minutes. The cookie and the status mirror are visible to a sniffer (accepted: non-secret, no address or key in them) |
| Active attacker (ARP spoofing, rogue AP) | can relay, replace the page, or send writes with a stolen cookie from a spoofed address | the TV overlay is out of band and shows the peer and the action text from the app's own table; the owner approves only what they asked for; the phone page cannot approve; at most whitelisted actions; the blast radius is the six actions (the sharpest are "microphone on" and "pairing window", both only on an explicit OK and the window only for an unpaired TV) |
| Someone else scans the QR first (a visitor, a photo of the screen) | they get the session | the QR disappears when the token is consumed; the TV shows "Phone connected from <peer>" with a Disconnect button; every write still needs the OK; the QR is on screen only while waiting; the first scanner sees only non-secret status |
| Link previews, scanner apps or prefetchers consume the token | availability only | `HEAD` and non-GET do not consume; a `GET` marked as a prefetch or preview (`Sec-Purpose`, `Purpose`, `X-Purpose`, `X-Moz`) gets `503` and does not consume; on a consumed link the phone gets `410` with instructions; restart creates a new token. A preview fetcher that sends none of those headers still consumes it (no way to tell it from the phone) |
| CSRF or a malicious page opened on the phone | a browser can post to a LAN address | `SameSite=Strict` cookie, custom header plus per-session token, JSON content type, `Origin` and `Sec-Fetch-Site` checks, no CORS, `OPTIONS` refused |
| DNS rebinding | a hostile page re-resolves its name to the TV | the `Host` header must equal the socket's local IP and port; a name is refused; the cookie is host-bound |
| Shoulder surfing or a camera on the screen | QR on screen for minutes | see "someone else scans first"; hidden after the first use; 10 minutes |
| Secrets the page may **show** | | none: no key, no HA data, no names, no addresses beyond the one the phone used, no log lines |
| Secrets the page may **accept** | | none: no text is accepted; enumerated actions and one validated integer only |
| Parser or logic bug becomes root code execution | root process with a LAN-facing parser | std-only subset parser (rejects everything else), no `unsafe`, no shell, fixed argv, cleared environment, no file serving, hand-fuzzed malformed corpus, size and time limits, a separate process with no key access; the independent review below; short life |
| A phone request makes the TV do something unwanted | a toggle flipped | the action list is the app's own (section 9); every change needs OK; ADR 0008 rules (read-back, history log) still hold; the daemon never changes an LG setting |
| Overlay spam | repeated requests pester the owner | one pending request, 3 s cooldown, three refusals end the session |
| Pairing window abused | a LAN peer claims the satellite | offered only when unpaired; needs OK; the window is the daemon's existing 10-minute window ([install-and-config-ux 5.3](install-and-config-ux.md#53-threat-model-for-the-pairing-window)) |
| Logging | secrets in logs | events and peer only; never the token, cookie, CSRF, path, body (a test greps the log) |
| The token reaches a persistent or shared place | `state.json` and the exec reply | `state.json` is on tmpfs, 0600; the exec reply goes only to the app; the app does not log it; EXP-012 (c3-a) searched `/var/log`, `/var/luna`, `/tmp/pmlogdaemon`, the journal, `logread` and `dmesg` for a marker sent in an exec command and its stdout and found **nothing** (`verified-on-target` for those places; memory-only buffers and `ls-hubd` at a raised log level not covered); the command line is visible in `/proc` while the call runs |
| Resource exhaustion | one single-threaded server | tiny limits, one connection, a 10-minute life, worker queue of one, children capped at 25 s and 64 KiB |
| A local app on the TV answers the overlay itself | any HBC app can exec `ctl.sh` as root | unchanged trust boundary (an app with root exec can already do every action directly); not a new exposure |

### 13.2 Security review gate (backlog [#3](../project/backlog.md#3-install-and-configuration-ux)) before the 0.4.0 release

The release is not tagged and the package is not published until all of this is done and recorded in a new document under `docs/design/` or the discovery records:

1. An **independent review** of `crates/setup-server`, `ctl.sh setup`, the app's overlay and the phone page by someone other than the author (a fresh agent running the repository's security-review process, then the owner): parser, state machine, token and cookie handling, CSRF, limits, command construction, file permissions, log content.
2. A **malformed-input corpus** and a short fuzz run (host): bad request lines, oversized and duplicated headers, `Content-Length` conflicts, `Transfer-Encoding`, non-UTF-8, NUL bytes, slowloris, request smuggling shapes, huge cookie, 10 000 connections in a loop; the process must end the session cleanly or refuse, never panic and never hang.
3. **Dependency and license audit** of the new crate (`cargo metadata`, `cargo deny check licenses`, `cargo audit`), size recorded.
4. **On the TV, with the owner:** an `nmap` (or equivalent) scan from another LAN host shows **no** extra listener while idle and while waiting before start; the port is closed after each end condition (done, timeout, app-gone, parse error); `ps` and `/proc/*/cmdline` show no token; the files in `/tmp/webos-satellite-setup/` have mode 0600/0700; the log contains no token, cookie or path ([section 15](#15-on-target-probe-checklist-exp-012-part-a-read-only-first), Part B).
5. The **threat table** above is walked row by row and each row has a test or a recorded reason.
6. The owner reads the result and signs off. A failed item blocks the release.

## 14. Tests

### 14.1 Host tests (no TV)

- **Rust, `crates/setup-server` (integration tests with a fake `ctl.sh`, a temporary setup directory, 127.0.0.1 and an ephemeral port, a std `TcpStream` client; a second loopback address such as 127.0.0.2 plays "another phone"):**
  - token: unknown token 404; first use 200 (the continue page, since 2026-11-05) with the cookie flags (`HttpOnly`, `SameSite=Strict`, `Path=/`, `Max-Age`), second use 410, `HEAD` 405 and not consuming; the token never appears in the log or in `state.json` after use;
  - session: no cookie 403 page, wrong cookie 403, right cookie from another address 403, one session only, idle end, hard timeout end (`--lifetime-s 2` test switch), `Host` and `Origin` mismatches, `Sec-Fetch-Site: cross-site`;
  - CSRF and types: missing and wrong header 403, wrong content type 415, `OPTIONS` 405 with no CORS headers;
  - limits: oversize headers 431, oversize body 413, missing or duplicate `Content-Length`, `Transfer-Encoding`, slow headers and slow body 408, a parse error ends the session, 20 unauthenticated requests end it, one connection at a time (a second connection waits, it is not served in parallel), rate limit 429;
  - actions: every row of the table builds exactly the expected argv (checked by the fake `ctl.sh` recording its arguments), invalid values and unknown keys 400, the precondition 422, one pending at a time 409, the 3 s cooldown, three refusals end the session;
  - confirmation: allowed (fake `ctl.sh setup answer` writes `answer.json`) runs the action and returns its `result`; refused and expired (`--request-timeout-s 1`) run nothing; a wrong id is ignored; heartbeat loss (`--heartbeat-grace-s 2`) ends the session and aborts a pending request; a worker command that hangs is killed at its limit;
  - state mirror: the allow-list (a canary field in the fake `status --json`, `name` and `prefix`, must never appear), a failing or slow fake command shows `null` and `error` and does not block the HTTP loop;
  - cleanup: after every end reason the listener is closed, `state.json` says `ended`, no token is left.
- **Shell, `packaging/webos/tests/setup.sh` (new, added to `make test-packaging` and CI):** `ctl.sh setup start|status|answer|stop` with a **fake server** (a shell script that writes `state.json` and honours the heartbeat); start refused when not installed, not root or already active; `status` touches the heartbeat and prints `none` without a server; `answer` validates the id (8 lowercase hex) and the verdict; a stale pid is `crashed`; `stop` removes the directory; `uninstall`, `purge`, `install` (update) and `stop` all stop the server; **the boot hook template and `ctl.sh start` never reference `setup`**; the directory mode is 0700 and nothing is written outside it and `log/`; `set reply-volume` (write, `already`, trigger, bad value, history line); the reload with the fake daemon not running.
- **Rust, daemon:** the reply-volume reload (file written, trigger consumed, gain changed, `SetVolume` sent, invalid file keeps the value), beside the existing `reload_*` tests in `daemon.rs`.
- **Packaging (`ipk.sh`, `build_ipk.py check`):** both architectures of the new binary are in the payload, in `SHA256SUMS`, and `install.sh` installs it and checks `--version`; the app lists `voicepower.js`; no extra files.
- **App (node, standard library only, `app_smoke.js` and `app_matrix.js`):** the whitelist contains exactly the new strings and the one `setup answer` pattern; the poll runs every 2 s (every 1 s while a request is pending or unshown) only while `waiting` or `active`, from any screen, and stops on `ended`; the overlay maps OK to `allow` and Back to `refuse`; an unknown action, an unknown value or a non-IP `peer` is auto-refused; the 45 s hint; the QR is built from the app's address and the state's path; `app_matrix.js` still passes with the strings coming from `voicepower.js`.
- **Phone page (node with a fake DOM and a fake fetch):** copy fallbacks (`execCommand` path, `isSecureContext` false), the sentence computed with `voicePower`, polling and the cooldown, error and expired texts, `key` states, the pairing step visibility.
- **PC end to end (P9):** the real server and `ctl.sh` with fake `luna-send` and a fake app loop that polls `setup status` and answers; a scripted HTTP client does the whole flow for each action; optionally a headless browser run if one is installed (not required).

### 14.2 On the TV (c3-a; the owner does the phone scan once)

Part A (probe, read-only first, before the server is built) and Part B (the real build) in [section 15](#15-on-target-probe-checklist-exp-012-part-a-read-only-first). Every run needs the owner's authorization; no sound is played; each LG setting written from the phone is restored and read back; the pairing window is **not** opened on c3-a's permanent entry (the page must not offer it while paired; the pairing action is tested on a PC fake HA).

## 15. On-target probe checklist (EXP-012, Part A read-only first)

Experiment ID **[EXP-012](../experiments/EXP-012-phone-setup-probe.md)** (written and Part A run on c3-a on 2026-11-05: P1, P2, P3 for a PC on the same LAN, P4, P6, P7 and P9 done, P5 by static evidence only; P8, P10 and everything in Part B not run). Run on c3-a, then on c3-b read-only if the owner agrees. Record the model, firmware, HBC version, power state and date with each item; negative and inconclusive results are recorded as such; no raw output is committed.

**Part A: before anything is built (read-only, no listener, except P3 and P7 which need an explicit OK)**

| # | Measure | Why |
| --- | --- | --- |
| P1 | Which tools exist: `iptables`, `ip6tables`, `nft`, `ip`, `ifconfig`, `ss`, `netstat`, `nc`/`busybox nc`, `busybox httpd`, `python3`; `/proc/net/fib_trie`, `/proc/net/route` readable | decides the interface choice (5.1), the probe method, and the fallback options |
| P2 | The firewall state, read-only: `iptables -S` and `ip6tables -S` (or `nft list ruleset`): INPUT policy, any rule on TCP destination ports, any rule per interface or source range; kernel modules for netfilter present or not | whether inbound high ports could be blocked at all |
| P3 | **Inbound reachability** (needs the owner's OK: a short-lived listener, 30 s, started by hand, killed after): a listener on a random port in 49152-65535, another in 32768-49151, and 6053 as the known-good control, from (a) a phone on the same Wi-Fi, (b) a PC on the same Wi-Fi, (c) a wired host if the TV has both interfaces, (d) a host on another VLAN or the guest network if one exists; record which succeed per interface, whether the TV was on Wi-Fi or Ethernet, and the power state (`Active` only matters: the app needs a lit TV) | the central hypothesis of 5.2 and the cross-VLAN failure hint of 5.3 |
| P4 | The TV's own addresses: every interface and IPv4 address (what `connectionmanager/getStatus` reports for the app versus what the kernel has), netmask, default route interface; whether more than one address is up (Wi-Fi Direct, Ethernet and Wi-Fi at once) | the QR address (5.4), the `/24` heuristic, the bind choice |
| P5 | The TV app's web engine: `navigator.userAgent` and feature checks (`fetch`, `Promise`, `Array.prototype.includes`, CSS flex gap, `position: fixed` overlay stacking, `KeyboardEvent.keyCode` for OK (13) and Back (461), `document.visibilityState` and timers while the app is hidden, `performance.now`), read through a one-line diagnostic added to the existing app on a scratch copy, or the Diagnostics screen once redesigned | the ES5 rule and the overlay and heartbeat design |
| P6 | Exec channel: that a **second** process started through `ctl.sh`-style `setsid nohup` from an HBC `exec` survives the exec's return and the app being closed; HBC service restart; how the TV treats the process when it goes to `Screen Off` and `Active Standby` (the server should die with the app anyway) | section 3.3 |
| P7 | Exec polling cost: `ctl.sh status`-like call once a second for 10 minutes through the app's `exec`: round-trip time, CPU of the fork, any LG or HBC log growth, missed beats | the 1 s heartbeat and the 15 s grace |
| P8 | Resource baseline for the server: RSS, CPU and startup time of a placeholder std-only listener built for the TV (measured in P4's first build) idle and under 10 requests per second; descriptor limits | the RSS budget (3.1) |
| P9 | Does HBC or the TV's log (`journald`, `pmlogd`, `/var/log`, HBC's own log) retain an exec call's stdout, so a token printed by `setup status` could persist? Look with a harmless marker string in an exec reply, then search the likely logs | the token-in-logs row of the threat table |
| P10 | A **test listener on the TV bound to `0.0.0.0` and a second to the Wi-Fi address only**: whether both are reachable; whether a connection from outside the private ranges is possible (if the TV has a public path, unlikely) | the peer filter and bind choice |

**Part B: after P1 to P9 and the server is built (deploy to c3-a, owner authorization):**

| # | Check |
| --- | --- |
| P11 | **The owner's phone scan, once:** from the TV app's button, scan the QR with the phone's camera app (note the phone model, OS, browser and version); does the page load, is there a "not secure" or HTTPS-first interstitial, does the cookie stick (an embedded QR-reader view may block it), does the copy button work on plain HTTP, does the My Home Assistant link open the add flow for ESPHome with the `domain` parameter (documented, unconfirmed) and what it shows |
| P12 | The overlay on the TV for each action, OK and Back with the **real remote** (key codes 13 and 461), the 60 s timeout, the cooldown, and that Back refuses; the app polling while on another screen |
| P13 | Each action end to end on c3-a: Always Ready and Hi LG (authorized writes, restored and read back), keep listening on/off, microphone on/off (the kill switch restored), reply volume changed and restored (HA's media-player slider follows, **no reply is played**), the pairing action only checked to be hidden while paired |
| P14 | End conditions on the TV: Done on the phone and on the TV, 10-minute timeout, closing the app (15 s), TV to standby, a malformed request from a LAN host ending the session; after each, the port is closed (`nmap` or a connect test from another host) and `/tmp/webos-satellite-setup/` is gone |
| P15 | No extra listener while idle (before start and after end), no token in `ps`, `/proc/*/cmdline`, logs or the persistent prefix; RSS and CPU of the server during the session; daemon RSS, loop-stall and HA connection unaffected during the session (the daemon's own log and HA's Listening state) |
| P16 | The network hint: from a host on another VLAN or the guest network (if available) the page fails to load and the TV shows the 45 s hint |

## 16. Implementation steps

Each step is sized for one agent and says what it can run in parallel with. **The TV app redesign** (four screens, being mocked up separately) owns `packaging/webos/app/`; steps that edit that directory are serial with it.

| Step | Work | Needs | Parallel with the redesign? |
| --- | --- | --- | --- |
| **P0** | This design, ADR 0009, backlog #15. **Done 2026-11-05.** Docs only | | n/a |
| **P1** | **Done 2026-11-05 (Part A on c3-a):** [EXP-012](../experiments/EXP-012-phone-setup-probe.md). Write EXP-012 (Part A and B of section 15 as a template-conforming plan); with owner authorization run Part A on c3-a and record the results (compatibility rows scoped to model, firmware, power state); decides the bind choice, the firewall fallback and the engine facts | owner authorization for P3 and P6 to P10 | **yes** (also answers open questions the redesign needs: the web engine) |
| **P2** | Reply volume: `ctl.sh set reply-volume` (+ usage text, whitelist notes, the history line), the daemon's `reply-volume-reload` trigger and reload (beside `reload_remote_off`), host tests in shell and Rust, design notes in [Voice and power section 6](voice-and-power-screen.md#6-ctlsh-interface) | none | **yes** |
| **P3** | `ctl.sh setup start|status|answer|stop`, the file protocol of section 4 (state file, heartbeat, answer, volatile directory, log truncation), integration in `stop`, `uninstall`, `purge` and `install`; `tests/setup.sh` with a fake server; usage text | the contract of this document | **yes** (shell only) |
| **P4** | `crates/setup-server`: the HTTP subset parser, limits, token, cookie, CSRF, peer and `Host` checks, the session and end conditions, the worker thread and action table, the state mirror allow-list, pending and answer files, the log; embedded placeholder assets; Rust integration tests (section 14.1); size and RSS note. May be split: P4a (HTTP core, auth, limits, end conditions) and P4b (actions, confirmation, state mirror) | P3 file contract (this document); P1 results can refine the bind choice | **yes** |
| **P5** | Extract the strings and the outcome matrix from `app.js` into `voicepower.js` (ES5, UMD), load it from `index.html`, point `app_matrix.js` and `app_smoke.js` at it, add the action texts of section 9 | the redesign's Voice and power screen | **no** (serial: do it in or right after the redesign's app work) |
| **P6** | The phone page (`phone.html`, `phone.js`, `phone.css`): layout, status mirror, sentence through `voicePower`, the actions with the pending and result states, "Add to Home Assistant" (copy fallbacks, My link, steps, done detection), failure and expiry texts, dark theme, 360 px; page tests with a fake DOM and fetch | the API of section 8; `voicepower.js` for the sentence | **partly**: layout and API client can start against a mock server before P5; the sentence needs P5 |
| **P7** | TV app: the "Set up with your phone" button and screen (QR from the app's address and the state path, state lines, 45 s hint, Done and Disconnect), the app-wide one-second poll, the confirmation overlay with OK and Back, the whitelist additions (exact strings and the `setup answer` pattern), app tests | P3 interface; the redesign's structure and navigation | **no** (serial with the redesign) |
| **P8** | Packaging: `Makefile` targets for both architectures, `build_ipk.py` and `install.sh` (install, `--version` triple check, `SHA256SUMS`), `ipk.sh`/`run.sh` checks, size and license-audit record for the new crate, `config.default.toml` unchanged | P4 compiles | **yes** (does not touch the app directory except the IPK file list) |
| **P9** | PC end-to-end test (real server and `ctl.sh`, fake `luna-send`, a scripted app loop) for every action and every end condition; docs updates: [development](../development.md), [packaging](packaging.md), [install-and-config-ux](install-and-config-ux.md) status | P3, P4, P6, P7, P8 | after the others |
| **P10** | **Security review gate** (section 13.2, items 1 to 3, 5, 6): independent review, malformed corpus and fuzz, threat table walk; findings fixed or recorded | P9 | after P9 |
| **P11** | Deploy to c3-a (silent), run EXP-012 Part B with the owner doing the phone scan once; record the experiment, the compatibility rows and, after the TV-side review items (13.2 item 4), the sign-off | P10, owner present with a phone | after P10 |
| **P12** | Version 0.4.0 release (bump, packaging checks, docs, release notes) | P11 and the owner's sign-off | last |

## 17. Open questions

1. **Inbound reachability** of a random high port on the TV: **answered for c3-a and a same-LAN PC** (EXP-012: reachable, no firewall). Still open: a phone (Part B), another VLAN, other TVs. If blocked elsewhere, which fallback (a documented fixed port, a user-run command, or unsupported on that TV)?
2. **A parse error ends the session** (owner's rule). A port scanner or a stray connection sending garbage would end a waiting session before the owner scans. Recommendation (not applied): count parse errors **before the token is consumed** and end only after 3, while a parse error from the session's own peer still ends it at once.
3. **IPv4 only.** Is that acceptable for IPv6-only home networks (rare)? The TV app's address lookup would also need to return an IPv6 literal in brackets.
4. **`friendly_name` from the phone** (section 9.3): wanted at all, given HA's own rename? If yes, the config-write test and the restart cost need an owner decision.
5. **Bind to the default-route interface** instead of `0.0.0.0`: **decided, no** (EXP-012 on c3-a: one IPv4 address, `0.0.0.0` reachable, the peer filter narrows). Revisit if a TV with two IPv4 interfaces shows a problem.
6. **Peer binding** may annoy a phone that roams between Wi-Fi bands or switches to mobile data; the alternative (cookie only) is weaker. Keep, and watch the first real use.
7. **Pairing window countdown:** **done in P2**: the daemon writes `pairing_until` (epoch seconds, or `null`) to `run/voice.json` on open and close only, and `ctl.sh status --json` has `"pairing":{"open":bool,"ends_in_s":N|null}` and `"pairing_window_s"` (the config's `window_s`). The phone page and the TV app can show the countdown; the server's allow-list should copy `pairing` (not copied until P4b adds it).
8. **`100.64/10` peers** (a phone on a tailnet while at home): refused in 0.4.0; revisit if users ask.
9. **The My Home Assistant link:** the `domain` parameter is unconfirmed on the current page; if P11 shows it does not preselect ESPHome, the page should link to the generic add-integration redirect and name ESPHome in the text.
10. **Where the redesign shows the pending-request overlay** when the user is inside another full-screen flow (the compatibility-report QR, an arm-and-confirm prompt): which has priority? **Decided (owner, 2026-11-05; built in P7, [11.1](#111-as-built-in-p5-and-p7-tv-app-2026-11-05)):** the phone overlay always wins and disarms any other armed button.

## Sources

Research date 2026-11-05.

- Home Assistant core `dev`: `homeassistant/components/esphome/manager.py` (connect: MAC check, name overwrite, device registry) and `config_flow.py` (user step, zeroconf, reauth), raw files read on 2026-11-05: <https://raw.githubusercontent.com/home-assistant/core/dev/homeassistant/components/esphome/manager.py>, <https://raw.githubusercontent.com/home-assistant/core/dev/homeassistant/components/esphome/config_flow.py>. A local copy was grepped for `expected_name` (not present).
- My Home Assistant: <https://www.home-assistant.io/integrations/my/> (no parameter list for `config_flow_start`) and <https://my.home-assistant.io/redirect/config_flow_start/> (title "Link to Integrations: add integration"; no parameters shown). The older ESPHome getting-started guide badge `my.home-assistant.io/redirect/config_flow_start/?domain=esphome` was found through a web search (copies of the esphome-docs file), not on the current docs.
- MDN, Clipboard API requires a secure context (recalled, not re-fetched).
- In this repository: [install and configuration UX](install-and-config-ux.md), [Voice and power screen](voice-and-power-screen.md), [packaging](packaging.md), `crates/satellite/src/{volume,daemon}.rs`, `crates/esphome-api/src/{app,server,config}.rs`, `packaging/webos/{ctl.sh,config.default.toml}`, `packaging/webos/app/app.js`.
