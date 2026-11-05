# Install and configuration UX: Homebrew Channel package, first run and pairing

- **Status:** **proposal, not accepted as a whole.** Milestone 3b, the daemon side of HA-driven pairing (section 5), was implemented on 2026-11-02 at the owner's request and tested on a PC, including one run against a real HA 2026.10.4 in a local venv; see [section 8](#8-implementation-status-milestone-3b). Nothing was run on a TV or against the owner's HA. **Milestone 3a, the HBC package with the TV app and the autostart wiring, was built on 2026-11-03 at the owner's request and tested on a PC only** ([packaging design](packaging.md); TV verification planned in [EXP-009](../experiments/EXP-009-ipk-install-update.md)). Config entities are still a proposal. Accepting it needs an owner decision and, for the TV-side parts, the verification list in section 7.
- **Date:** 2026-11-02. Research date for every external source below: 2026-11-02 (GitHub trees and files read at the tags or heads named in the [Sources](#sources) list).
- **Scope:** backlog [#3 Install and configuration UX](../project/backlog.md) (owner priority 2: public project, interleaved into [roadmap](../project/roadmap.md) Phases 3 and 5). Device in scope for TV facts: c3-a (LG C3, webOS TV 8.3.0, firmware 03.30.74, Homebrew Channel 0.6.3). Other models and firmware are **not** covered.
- **Related:** [autostart design](autostart.md), [ADR 0001](../decisions/0001-esphome-native-api.md), [D0-D1 root method](../discovery/c3-a/D0-D1-runtime.md#root-method-details), [rooted-TV improvements](../discovery/c3-a/rooted-tv-improvements.md), [ESPHome research](../research/esphome-rust-satellite.md), [ESPHome API crate](../development/esphome-api.md), [P1 runtime smoke (UDP 5353)](../discovery/c3-a/P1-runtime-smoke.md#udp-5353-mdns).

Labels follow [AGENTS.md](../../AGENTS.md): `documented` (official or upstream source text), `observed-on-other-device` (upstream code or data for other TVs), `verified-on-target` (seen on c3-a), `hypothesis`. "Code" means this repository at the commit this document was written against.

## 1. Goals and constraints

**Goals**

1. A rooted-TV owner who is not a developer installs the satellite from the Homebrew Channel (HBC) and pairs it with HA without SSH, without a text editor and without copying a secret by hand.
2. Pairing does not need the Noise encryption key typed on a TV remote, a phone or in HA.
3. The result survives reboots (via the [autostart design](autostart.md)) and fails safe: wrong state means the microphone stays closed.
4. Everything stays local. No cloud service, no account, no telemetry.
5. The owner's own two TVs and other users' TVs use the same flow (public project, [owner priorities](../project/backlog.md)).

**Non-goals**

- Rooting a TV. The rooting guide is [webosbrew.org/rooting](https://www.webosbrew.org/rooting/); the project documents it as a prerequisite and links to it.
- A configuration UI for things HA already owns (pipelines, wake word choice, STT/TTS).
- Any change to LG native services, the firmware update block or the LG hosts file.

**Constraints**

- **Rooted TVs only.** The package has `rootRequired: true` semantics (see 2.1). On a non-rooted TV it must say so and do nothing.
- **Firmware and model variety.** Only the LG OLED C3 series (c3-a firmware 03.30.74, c3-b 03.30.73) is verified ([verified TVs](../compatibility/verified-models.md)). Since 2026-11-04 (owner decision, [firmware policy](firmware-policy-and-reports.md), implemented in the repository, not yet run on a TV) the daemon does **not** keep the microphone off on another firmware: it starts, warns, self-checks the capture stream and shows a status (`compat_status`, the app banner); `safety.firmware_policy = "strict"` keeps the old block as an opt-in. The installer must not hide that status. HBC itself varies: c3-a has 0.6.3, the public repo lists 0.7.3 (2026-11-02), and 0.7.0 changed install locations and autostart (2.1).
- **Userland.** c3-a is a 32-bit ARM soft-float userland on a 64-bit kernel, with glibc 2.31 ([D0-D1](../discovery/c3-a/D0-D1-runtime.md#32-bit-vs-64-bit-userland)). The daemon is a static musl binary ([development](../development.md#target-decision)). Other webOS versions may differ; nothing here assumes the c3-a userland holds elsewhere.
- **Safety and privacy rules from AGENTS.md apply unchanged.** No raw probe output committed, no secret in logs, no remote install pipes (`curl | sh`) in the public instructions, TLS validated, any persistent change on a TV needs the owner's authorization, native LG voice services untouched.
- **No secrets in logs or on screen by default.** The Noise key is never printed, never put in mDNS, never in `--check-config` output and never shown in a UI.
- **Persistence is a TV change.** Writing the package, the boot hook or a key file is a persistent write on the TV. The first one of those on c3-a still needs the owner's approval, as in the [autostart design](autostart.md).

## 2. Research findings

### 2.1 Homebrew Channel packages, repository, elevation and autostart

| # | Claim | Label | Source |
| --- | --- | --- | --- |
| H1 | HBC is an unofficial store and root-tooling app. It exposes a Luna service: `install` (args `ipkUrl`, `ipkHash` SHA-256, `subscribe`), `exec` (root command, "may not execute as root if a device is not rooted"), `spawn` (long-running). Additional repositories are added by launching HBC with `launchMode: addRepository` and a `url`. | documented | [HBC README](https://github.com/webosbrew/webos-homebrew-channel) |
| H2 | At boot `/var/lib/webosbrew/startup.sh` sets a failsafe flag, elevates the HBC service, runs `run-parts /var/lib/webosbrew/init.d`, sleeps 10 s, then clears the flag. A crash inside that window opens a telnet emergency shell on the next boot and skips the hooks. | documented (HBC `services/startup.sh`) and verified-on-target for c3-a (the same file was read there) | [HBC startup.sh](https://github.com/webosbrew/webos-homebrew-channel/blob/main/services/startup.sh), [autostart design](autostart.md#how-the-boot-hook-runs-verified-on-target-read-only) |
| H3 | Package manifest fields: required `id`, `version`, `type`, `title`, `iconUri`, `ipkUrl`, `ipkHash.sha256`; optional `appDescription`, `sourceUrl`, `rootRequired` (`true`, `false` or `"optional"`). A repository file is `{"packages": [...]}` with `id`, `title`, `iconUri`, and either `manifestUrl` or an inline `manifest`. | documented | [webosbrew/docs schemas](https://github.com/webosbrew/docs/tree/main/schemas) |
| H4 | The public repo's JSON (`https://repo.webosbrew.org/api/apps.json`) also carries `requirements.webosRelease` (for example `">=4.0"` for PicCap), `ipkSize`, `pool`, `screenshots`. On 2026-11-02 it listed 56 packages, 20 with `rootRequired: true`. HBC's own manifest is generated at release time by `tools/gen-manifest.js` (id, version, hash of the `<id>_<version>_all.ipk`). | observed-on-other-device | [apps.json](https://repo.webosbrew.org/api/apps.json), [gen-manifest.js](https://github.com/webosbrew/webos-homebrew-channel/blob/main/tools/gen-manifest.js) |
| H5 | To list in the central repo, open a PR to `webosbrew/apps-repo` adding `packages/<name>.yml` with `title`, `iconUri`, `manifestUrl` (a manifest hosted by the author, for example a GitHub release asset), `category`, `pool` (`main` for open source, `non-free` otherwise), `description` (Markdown), optional `screenshots` and `funding`. Rules: no piracy; "be considerate to users' TV"; ports must follow the original license; and **"NO AI SLOP"**: applications made primarily by AI and submitted without meaningful human development, testing and review are rejected, AI use must be disclosed in the PR and the submitter is responsible for the code. "Self-host a repository" is listed as a TODO in that README. | documented | [apps-repo README](https://github.com/webosbrew/apps-repo) |
| H6 | An app and its service ship in one IPK with `ares-package <appdir> <servicedir>`, provided the service id starts with the app id. Sample IPK names end in `_all.ipk`. The LG TV docs describe JS services; they do not describe native services (webOS OSE docs do, which is not proof for the commercial TV). | documented | [LG CLI guide](https://webostv.developer.lge.com/develop/tools/cli-dev-guide), [OSE native services](https://webosose.org/docs/tutorials/native-services) |
| H7 | **Elevation.** Rooted-TV community apps run their service as root by asking HBC to run `elevate-service <id>` through `exec`. Reading `services/elevate-service.ts`: for a native service it rewrites the `Exec=` line of the Luna service definition to drop `/usr/bin/jailer`, for a JS service it points `Exec=` at HBC's `run-js-service`, and it patches the role file so the service may use any bus name. HBC's `startup.sh` only auto-elevates HBC's own service. | observed-on-other-device (source) | [elevate-service.ts](https://github.com/webosbrew/webos-homebrew-channel/blob/main/services/elevate-service.ts) |
| H8 | **PicCap / hyperion-webos** ships a web UI app plus a native daemon in one IPK (`services.json` with `"engine":"native"` and an `executable`). The daemon registers on the Luna bus (`LSRegister`), reports `elevated: getuid() == 0`, and the UI polls that, calls `exec` with `elevate-service org.webosbrew.piccap; elevate-service org.webosbrew.piccap.service` and restarts it. The build uses the NDK toolchain ([buildroot-nc4](https://github.com/openlgtv/buildroot-nc4)), so the binary is dynamic and links the webOS libraries. | observed-on-other-device (source) | [piccap](https://github.com/TBSniller/piccap), [hyperion-webos `src/service.c`](https://github.com/webosbrew/hyperion-webos) |
| H9 | **PicCap autostart.** When the user ticks "autostart", the service creates the symlink `/var/lib/webosbrew/init.d/piccapautostart` pointing at a script in its own service directory, and removes it when unticked. The script is `luna-send -n 1 -f luna://org.webosbrew.piccap.service/isRunning '{}' & exit 0`, so the hook returns at once and the Luna hub starts the service on first call. The Hyperion.NG loader does the same with a script that runs `elevate-service` again "for good measure" and then calls `/start`. | observed-on-other-device (source) | [PicCap `servicefiles/`](https://github.com/TBSniller/piccap), [hyperion.ng-webos-loader `service/scripts/autostart.sh`](https://github.com/webosbrew/hyperion.ng-webos-loader) |
| H10 | The Hyperion.NG loader carries a 27 MB payload (the daemon and libraries) inside the IPK's service directory and starts it with `posix_spawn` from a small native Luna service. So large payloads in an IPK are normal for HBC apps. | observed-on-other-device | [apps.json](https://repo.webosbrew.org/api/apps.json), [loader `daemon.c`](https://github.com/webosbrew/hyperion.ng-webos-loader) |
| H11 | HBC 0.7.0 (2024-09) added autostart through activities on webOS 3.5+, install locations other than `/media/developer`, and `homebrewBaseDir` in `/getConfiguration`. 0.7.1 and 0.7.2 fixed elevation on webOS 3 and Luna calls when not elevated. | documented | [HBC CHANGELOG](https://github.com/webosbrew/webos-homebrew-channel/blob/main/CHANGELOG.md) |
| H12 | `webosbrew-autostart` is a tiny app that registers as an "input app" so webOS launches it at boot, bind-mounts over `/var/lib/eim` so the registration persists, and calls `luna://org.webosbrew.hbchannel.service/autostart`, which runs the startup hooks. | documented | [webosbrew-autostart](https://github.com/webosbrew/webosbrew-autostart) |
| H13 | Rooted TVs do not need the 1000-hour Dev Mode renewal. Root persistence across firmware updates is "not guaranteed". | documented | [rooting guide](https://www.webosbrew.org/rooting/) |

Consequences for this project:

- **Our daemon is not a Luna service.** It is a static musl binary and does not link `libLS2` ([development](../development.md#target-decision)). So the PicCap path (a Luna-registered native service elevated through `elevate-service`) does not apply as is. What does apply is H9: **an `init.d` entry that starts the payload at boot** (`hypothesis`: a hook started by `run-parts` runs as root, since `startup.sh` itself does; the existing [draft hook](../../packaging/webos/init.d/webos-satellite) relies on this and it is still not installed on c3-a). A Luna wrapper service becomes necessary only if the UI needs a Luna method to talk to the daemon; section 4 avoids that.
- **The boot trigger that runs `startup.sh` on c3-a is still untraced** ([D0-D1 open question 8](../discovery/c3-a/D0-D1-runtime.md)). H11 and H12 show two mechanisms used on other TVs (activities, input-app trick). They are `observed-on-other-device`; neither is confirmed on c3-a.
- **The install location is not always `/media/developer`.** The package must resolve it at install time (HBC `getConfiguration` `homebrewBaseDir` on 0.7.x; the app's own directory on every version), not hard-code it. c3-a runs 0.6.3, which predates that field.
- **ipk contents.** Whether the installer keeps the exec bit and accepts an ELF in an app or service directory is `observed-on-other-device` (PicCap `chmod +x` before `ares-package`; the Hyperion loader ships a daemon in its service dir) and `hypothesis` for our package. A minimal test IPK on a TV settles it.
- **Trust.** HBC checks only the SHA-256 in the manifest (H1). The trust anchor is therefore the manifest URL and the repository review, so the public instructions must give the manifest URL over HTTPS, publish the hash and keep builds reproducible.
- **Central repo policy (H5).** This project is developed with AI assistance. Listing in `apps-repo` requires disclosure in the PR and meaningful human testing and review. Decided by the owner on 2026-11-03 (open question 7): the target is the official `apps-repo`, with the disclosure in the PR, and there is no self-hosted repository. Follow-up: [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing).

### 2.2 How HA provisions ESPHome devices

Sources for this section: HA 2026.10.4 `esphome/config_flow.py` and `manager.py`, ESPHome 2026.10.1 `api_connection.cpp`, `api_server.cpp` and the `provisioning` component, vendored `api.proto` ([Sources](#sources)). All `documented`.

1. **Discovery.** HA's zeroconf step needs the mDNS TXT key `mac` (otherwise the flow aborts with `mdns_missing_mac`). The device name is the hostname minus `.local.`, `friendly_name` is optional, and a truthy TXT `api_encryption` marks a Noise device.
2. **Plaintext first.** If TXT has no `api_encryption`, the flow opens a plaintext connection and reads `DeviceInfo`. If the device requires Noise, the flow probes with a deliberately wrong key to learn the name, tries every key HA already knows for that MAC (its own key storage, the ESPHome dashboard), and only then asks the user for the key ("encryption key" step).
3. **A device can start with no key and get one later.** After connecting without a key, HA's manager checks `DeviceInfoResponse.api_encryption_supported` (field 19). If true and HA has no key, it generates 32 random bytes (or reuses the key it stored earlier for the same MAC), sends them with `NoiseEncryptionSetKeyRequest` (id 124, `bytes key = 1`), stores the key in the config entry and in its key storage, and syncs it to the ESPHome dashboard. The user never sees or types the key.
4. **Zero-PSK transport.** If `api_encryption_provisionable` (field 26) is true, HA opens a short Noise session using the well-known all-zero PSK (`aioesphomeapi.ZERO_NOISE_PSK`) and sends the key there. HA's own comment: this stops passive sniffing but "does not authenticate either side against an active man in the middle". Otherwise HA pushes the key over the plaintext connection, which ESPHome marks deprecated and says it will remove on keyless devices in 2027.2.0 (comment in `api_connection.cpp`). A failure is retried on the next connect cycle.
5. **Version floor.** The provisionable field and the zero-PSK path are in HA 2026.9.0 and absent in 2026.8.0 (checked at those tags). Older HA only has the plaintext push.
6. **Device behaviour (ESPHome reference).** With no key the device accepts plaintext and upgrades the connection to Noise when the client opens with a Noise hello, using the all-zero PSK. `api_encryption_supported` is true, `api_encryption_provisionable` is "no key set". `SetKey` fails on bad base64 or length and **rejects the all-zero key**; an empty key clears the key. Once a key is active, plaintext and zero-PSK sessions are rejected and sessions opened before the key are closed (code comments).
7. **Provisioning window (ESPHome `provisioning:`).** ESPHome 2026.10 models "ships unprovisioned" devices with a window: opened at boot, with a required timeout; if it expires while unprovisioned the window closes, the closed state is RAM-only (a power cycle reopens it), new connections are refused with `DisconnectReason.PROVISIONING_CLOSED` and `SetKey` is refused. HA shows "The ESPHome device closed its setup window ... Power the device off and on again, then retry." This is the reference design for a timed pairing window.
8. **Outgoing connection mode** exists (device dials HA, `api_outgoing_connection_supported`), not researched further here; see the [ESPHome research](../research/esphome-rust-satellite.md).

### 2.3 What our daemon does today (code)

Written against commit 89a658d, before milestone 3b. Rows marked **3b** changed with the pairing work (section 8).

| Topic | Status | Where |
| --- | --- | --- |
| Noise with a pre-shared key | implemented; verified-on-target against HA 2026.10.4 with a key typed in the config flow | [`server.rs`](../../crates/esphome-api/src/server.rs), [P1 HA integration](../discovery/c3-a/P1-ha-integration.md) |
| No key configured | serves **plaintext only** and logs a development warning; a Noise client gets `Bad indicator byte`. The daemon refuses to start without a key unless `allow_plaintext = true`. **3b:** or `[pairing] enabled = true`, which starts unpaired instead | `server.rs` (`Server::bind`, `handshake_inner`), [config](../../config/example.toml) |
| `DeviceInfoResponse.api_encryption_supported` | `true` only when a key is configured, so HA never tries to provision. **3b:** also `true` on keyless pairing sessions | `server.rs` (`device_info`, `keyless_device_info`) |
| `api_encryption_provisionable` | never set. **3b:** `true` on keyless pairing sessions (only possible while the window is open) | `server.rs` |
| `NoiseEncryptionSetKeyRequest` | answered with `success = false` and a warning. **3b:** accepted over zero-PSK Noise while unpaired and the window is open; rotation by the key holder | `server.rs`, [`pairing.rs`](../../crates/esphome-api/src/pairing.rs) |
| Key storage | read once from `device.noise_psk_file`; held in an immutable `Arc<Resolved>`; nothing writes it. **3b:** a provisioned key lives in `<pairing.state_dir>/noise_psk`, written atomically; the runtime key state is owned by the server loop | [`config.rs`](../../crates/esphome-api/src/config.rs), `pairing.rs` |
| mDNS `_esphomelib._tcp` | implemented with `mdns-sd` 0.21 (cargo feature `mdns`, on by default); **off by default in the config** (`device.mdns = false`). TXT: `mac`, `friendly_name`, `version`, `platform`, `board`, `network`, and `api_encryption` only when a key is set. Host name `<name>.local.`. Fails soft | [`mdns.rs`](../../crates/esphome-api/src/mdns.rs) |
| mDNS evidence | the TXT builder has a unit test and `fake_device --mdns` exists; **no run on the TV, no recorded discovery by a real HA**. On c3-a UDP 5353 is already bound by `mdnsd`, `avahi-daemon` and `iconnectivity` (v4) and `mdnsd` and `chip-service` (v6); the reuse flags were not read and whether LG's `mdnsd` coexists or answers for foreign services is unknown ([P1 runtime smoke](../discovery/c3-a/P1-runtime-smoke.md#udp-5353-mdns), verified-on-target for the binders). `mdns-sd` sets `SO_REUSEADDR` and `SO_REUSEPORT` (documented, [research](../research/esphome-rust-satellite.md)) | |
| HA discovery in the owner's homelab | not used: HA runs in a pod without multicast reach, the entry was added by host and port | [P1 HA integration](../discovery/c3-a/P1-ha-integration.md) |
| Config entities in HA | a media player, a "Mute microphone" switch, the `select` and the two listening-indicator binary sensors; the crate also supports generic `number`, `button` and `binary_sensor` entities with `entity_category` ([esphome-api.md](../development/esphome-api.md#decisions-taken-in-this-crate)), none used for configuration yet | [`entities.rs`](../../crates/esphome-api/src/entities.rs), `api.proto` |

**Can we support HA-driven key provisioning?** Yes, in principle; the pieces exist or are small, but it is a security-relevant change. Needed (all PC-testable with `aioesphomeapi`, which implements the HA side, and the existing fake-HA tooling):

1. A **keyless "unprovisioned" mode** that accepts plaintext (for HA's first `DeviceInfo` read) and Noise with the all-zero PSK (for `SetKey`) on the same port, selected by the first byte as ESPHome does.
2. `DeviceInfo`: `api_encryption_supported = true`, `api_encryption_provisionable = true` while unprovisioned.
3. A real `SetKey` handler: decode base64, require 32 bytes, reject all-zero, write the key atomically, then activate it, drop every session opened before activation and require the key from then on.
4. **Mutable key state.** `Resolved` is immutable today; the key needs shared interior state (for example a `watch` or `RwLock`) that the accept loop reads per connection.
5. mDNS TXT without `api_encryption` while unprovisioned, with it afterwards (already how `txt_records` behaves).
6. A **pairing window** (section 5.3) so a keyless device is not claimable by anyone on the LAN for an unbounded time.

HA and `aioesphomeapi` are the only clients we have to satisfy; HA 2026.9.0 or newer is the floor for the no-typing flow. For older HA the key must be entered once, as today.

## 3. Options

### 3.1 Option A: a webOS app UI on the TV (remote control)

- **What:** a web app in the IPK (type `web`) with a few screens: status (running, paired, firmware validated, HA peer, mute), start/stop, autostart on/off, mic kill switch, "start pairing", "reset pairing", and the line "In HA add ESPHome with host `<TV IP>` port 6053". It controls the daemon through HBC `exec` calling a fixed set of subcommands (for example `webos-satellite ctl status --json`), not through a new network port.
- **Effort:** medium. A small static page plus `ctl` subcommands in the daemon or scripts; no Luna service (section 2.1). The UI shell is also a prerequisite for B (a daemon cannot draw a QR code on the TV; a toast carries text only, and I found no documented image toast, `hypothesis`).
- **Security:**
  - No listener beyond the existing 6053. `exec` is a root primitive that any app on the TV can already call; our app adds no exposure but must **never interpolate free text into the command** (fixed subcommands, arguments restricted to a whitelist or base64).
  - HBC `exec` runs as root only if HBC is elevated; the app must check (`checkRoot` exists, HBC 0.6.1 changelog) and refuse otherwise.
- **UX:** remote-control text entry is slow, so the design avoids it entirely. Anything that needs typing (a custom device name) is a non-goal for the first slice. Works without a phone and without a network path to the TV from a browser.
- **Limits:** only one user at a time, only while the app is in the foreground; no remote administration.

### 3.2 Option B: embedded web config server plus a QR code on the TV

- **What:** the daemon (or a helper) serves a config page on the LAN. The TV app shows a QR code with the URL, the phone opens it, the user enters the HA connection details and the key.
- **Effort:** high. HTTP server in a 64 MiB-RSS-limited static daemon (or a second process), QR rendering in the TV app, session handling, TLS decision, a rate limiter, tests, a security review ([backlog #3](../project/backlog.md) already requires one). Plus Option A's UI shell.
- **UX:** good for typing; poor for trust (browser warnings if TLS is self-signed; phone and TV must be on the same VLAN, and guest or IoT VLANs often block phone-to-TV traffic).

**Threat model for B**

Assets: the Noise key (it decides who can stream microphone audio to a controller), the device identity and settings, and the TV's root-level control plane (the daemon runs as root, so any injection bug is root on the TV).

| Threat | Notes | Mitigation if B is built |
| --- | --- | --- |
| LAN attacker finds the open port | a listener on 0.0.0.0 is scannable; a root daemon with a web parser is high value | listen **only in an explicit setup mode** (started from the TV app), on a random high port, closed after the first successful submit or 10 minutes, never at boot; separate from the 6053 listener; refuse when the app is not in setup mode |
| Guessing the access secret | online brute force | one-time token of at least 128 bits in the QR URL path (single use, consumed on first request, exchanged for a session cookie), plus a code shown on the screen and typed on the phone for the second factor (8 characters, 5 attempts, then the window closes) |
| Passive sniffer on the LAN or Wi-Fi sees the token or the key | plain HTTP exposes both | HTTP is **not acceptable for anything secret**. TLS needs a certificate the phone trusts; a self-signed one trains users to click through, and the QR could carry the certificate fingerprint but ordinary phone cameras and browsers will not pin it (`hypothesis`). So: do not send the key at all (below) |
| Active attacker (ARP, rogue AP) | can relay or replace the page | an on-screen "device connected: `<phone address>`, press OK to allow" confirmation on the TV for every write; never accept a write without it |
| CSRF or a malicious web page on the phone | browser can post to a LAN address | require a custom header plus a per-session token; same-origin checks; no CORS |
| Shoulder surfing the screen | QR and code are on the TV for minutes | short window; code hidden after first use |
| Secrets it may **show** | | none: never show the existing key, never show HA data. A status line may show "paired: yes/no" |
| Secrets it may **accept** | | none that matter if pairing is HA-driven (2.2): the satellite needs no HA URL, no HA token and no key typed, because HA connects to it and sets the key itself. Accept only non-secret settings (name, mic gain, sink) |
| Logging | | log events and the peer address, never the token, code or body |
| Memory and DoS | single-threaded daemon with a 64 MiB RSS ceiling and a stall watchdog | tiny request size limits, one connection at a time, exit setup mode on any parse error |

**Assessment:** with HA-driven pairing and with settings exposed as HA entities (below), B has **no secret left to protect and no setting that HA cannot already host**. It would add a root-level LAN parser to protect almost nothing. The remaining value is typing a device name, which can default to a unique generated name.

### 3.3 Option C: no UI; zeroconf plus HA-driven key provisioning

- **What:** the daemon advertises `_esphomelib._tcp`, starts unprovisioned with a pairing window, and HA discovers it, adds it with one click and provisions the key (2.2). Settings that remain (mic gain, output gain, kill switch) are exposed as ESPHome config entities in HA (`number`, `switch`, `entity_category: config`), so the HA UI is the configuration UI and HA's auth protects it. Where HA cannot see multicast (containers, VLANs, the owner's pod), the user adds the device in HA by host and port (the TV address must still be learned somewhere, see below).
- **Effort:** medium for the daemon (section 2.3 list) plus small for entities. No UI on the TV for the happy path. mDNS on the TV is **unverified** and is the main risk (2.3).
- **Security:** no extra listener. The exposure is the pairing window: during it, a LAN peer that connects first can set the key and then receive microphone audio. Mitigations in 5.3. After pairing: Noise with a 256-bit key, plaintext and zero-PSK refused.
- **UX:** the best case is zero typing: install from HBC, tap "start pairing" (or auto-open on first run), click "Add" on the discovered device in HA. The weak case (no mDNS) needs the TV address, which Option A's status screen displays.
- **Limits:** needs HA 2026.9.0 or newer for the no-typing flow; discovery only on the same multicast domain; without any TV UI the user cannot see state, which is why C needs A's small status screen.

### 3.4 Comparison

| | A: TV app | B: web server + QR | C: zeroconf + HA provisioning |
| --- | --- | --- | --- |
| Effort | medium | high (needs A too) | medium (daemon) + small (entities) |
| New listening ports | none | yes, root LAN parser | none (6053 only) |
| Secrets typed by the user | none if paired by HA | key and HA details, or none if paired by HA | none |
| Needs a phone | no | yes | no |
| Works without mDNS | yes | yes | needs host and port in HA |
| Verified on a TV | no | no | no (mDNS, HBC package, init.d hook all unverified) |
| Who it suits | everyone for status and start/stop | people who want to type names | everyone with HA 2026.9.0 or newer |

## 4. Recommendation

**C as the pairing and configuration path, with a minimal A as the on-TV face; B is not built unless a concrete need appears after C.**

1. Package the daemon as an HBC IPK with a small web app (A) that shows state and offers start, stop, autostart, kill switch, start pairing and reset pairing, using only fixed `exec` subcommands.
2. Pair through HA (C): the daemon starts unprovisioned inside a timed pairing window, HA generates and sets the key; the key never appears on screen, in the phone or in logs.
3. Expose tunable settings as HA config entities instead of building a settings UI. HA authenticates the user.
4. Keep B on the backlog with the threat model above. Reopen it only if a setting that needs typing and cannot be an entity emerges.

### 4.1 Phase 3 slice (with autostart; owner-approval gated on the TV)

Milestone 3a, package and autostart (no pairing change). **Built 2026-11-03, PC only:** the [packaging design](packaging.md) records what was implemented and where it differs from the bullets below (state under `/var/lib/webos-satellite` instead of the app's directory, the daemon run from a copy there, the hook written as a file rather than a symlink, pairing instead of a generated key, no repository listing yet):

- **IPK** `<app id>_<version>_all.ipk`: web app plus a payload directory with the static binary and the existing [`packaging/webos/`](../../packaging/webos/) scripts. No Luna service. The binary architecture is chosen by the installer script. **Implemented (2026-11-02, PC only) in `install.sh`:** the package carries the armv7 and aarch64 builds; `--arch auto` (default) tries aarch64 when `uname -m` is `aarch64` and armv7 otherwise, `--arch` overrides it, and the chosen binary must answer `--version` with its own target triple before it replaces the installed one, else aarch64 falls back to armv7 ([development](../development.md#choosing-the-build-armv7-or-aarch64)). The aarch64 daemon has not run on a TV yet (pending); `--arch armv7` pins the build verified on c3-a.
- **First-run script** (run by the app through `exec`, or by hand): creates the prefix with the existing `install.sh` layout, generates a stable unique name (`webos-satellite-<6 hex>` from `/dev/urandom`) and a `mac_seed`, writes `config.toml` with safe defaults (`sink.type = "pacat"` at the approved gain, `mdns = true` once verified, validated firmware list from the package), and **generates the key file on the TV** (`head -c 32 /dev/urandom | base64`, mode 0600). It prints nothing secret.
- **Autostart** exactly as in the [autostart design](autostart.md), but the hook is created by the app on explicit user confirmation, as PicCap does (H9): a symlink in `/var/lib/webosbrew/init.d/` to the script in the package directory (removed by "autostart off" and by uninstall), so an IPK update replaces the target and the hook returns at once. The design's rules stay: 60 s delay, `disable`, `crashloop`, kill switch and firmware guard.
- **Uninstall** path: app button and `uninstall.sh` remove the symlink first, stop the daemon, then remove the prefix, then (optionally) the key. HBC's own uninstall of the app must not leave a dangling hook; the hook checks that its target exists (as the draft hook does).
- **Release assets** (manifest with `rootRequired: true`, `ipkHash.sha256`, `requirements.webosRelease` if known) hosted by the project, as the `apps-repo` entry needs. No self-hosted repository file (decided 2026-11-03, open question 7); until the listing exists testers install by hand.
- For 3a the key still has to reach HA once. For the owner's own TVs that is the existing flow; for others it is a stopgap, which is why 3b follows immediately.

Milestone 3b, HA-driven pairing (PC work first):

- **Done (2026-11-02):** daemon changes from 2.3 (keyless unprovisioned mode, SetKey, mutable key state, window). See section 8.
- **Done:** tests with `aioesphomeapi` and the fake-HA harness on a PC: provisionable flag, zero-PSK `SetKey` (accepted, all-zero rejected, bad length rejected, closed window refused), activation drops older sessions, plaintext and zero-PSK refused afterwards, no microphone data on an unprovisioned session, key file mode 0600, nothing secret in logs. Also one run against a real HA 2026.10.4 on the PC (not the owner's HA).
- **Pending:** one run against the owner's HA, on a TV, with a **new** test device name (never the permanent `lg-c3-a` entry); owner decision and authorization.
- **Partly done:** the app's `ctl pair` and `ctl reset-pairing` map onto the trigger files `<state_dir>/pair` and `<state_dir>/reset-pairing` (and the `--pair`, `--reset-pairing` flags); a `ctl` wrapper and a `--status --json` output with no secrets are not written.

### 4.2 Phase 5 slice

- Config entities in HA (mic gain, output gain, mic kill switch as the existing mute, optionally pairing window state), `entity_category: config`.
- **Power options (owner, 2026-11-03; design only, depends on D5):** if D5 shows that a power-related TV setting is critical for the satellite to work (screen off instead of standby, Always Ready, Quick Start+), expose it both as an HA config entity on the device page and in the TV app, with the risks next to the option (power draw, panel/OLED considerations, what stops working, how to revert), a stored original value and a restore action. The daemon never writes a TV setting without an explicit user action. *(Answered 2026-11-05, [ADR 0008](../decisions/0008-user-initiated-lg-settings.md): the TV app writes Always Ready and LG hands-free on a press; see [Voice and power screen](voice-and-power-screen.md).)* Full design: [always-on section 7](always-on.md#7-exposing-power-options-in-the-ui-design).
- Capability discovery (already a Phase 5 deliverable) feeds the first-run script so no `LGSIC` name or ALSA index is baked in.
- Publication: central `apps-repo` PR with the AI-use disclosure (decided 2026-11-03; wording in [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing)); release checklist with package inspection and published SHA-256.
- Second-TV validation of the whole flow; compatibility record per model, firmware and HBC version.
- Polish of the TV app (translations, state wording, recovery screens).
- Option B stays unbuilt unless the open questions show a need.

## 5. Pairing mode design (Option C detail)

### 5.1 States

| State | Mic | 6053 accepts | mDNS | Entered by |
| --- | --- | --- | --- | --- |
| **Provisioned** | per mute and firmware guard | Noise with the key only | TXT `api_encryption` set | key file present |
| **Unprovisioned, window open** | **closed** (kill-switch semantics) | plaintext (Hello, DeviceInfo only) and zero-PSK Noise (`SetKey` only) | TXT without `api_encryption` | no key and the window is open |
| **Unprovisioned, window closed** | closed | nothing; connections refused as ESPHome does with `PROVISIONING_CLOSED` | advertisement stopped | window timeout (RAM-only), or a key clear by the key holder (kept across restarts by a marker file until a local open or reset) |

### 5.2 Rules

1. Microphone data never flows on a keyless session; `SubscribeVoiceAssistant` is ignored until a real key is active. HA's manager runs the key step right after `DeviceInfo`, before entity setup, so no audio is needed to pair. **Implemented:** keyless sessions never become the main session; the daemon also keeps the PCM closed ("not paired") until the key is active. One addition was needed: HA lists entities before the key step, so keyless sessions answer `ListEntitiesRequest` with an empty list.
2. The window opens only on an explicit local action (TV app button, `ctl pair`, or the first launch of the app after install), **not** on every boot. Default length 10 minutes, visible countdown in the app. After the first successful `SetKey` the window closes. **Implemented differently:** with `pairing.open_on_start = true` (default) the window opens at every start **while no key is stored** (as ESPHome's `provisioning:`); once paired a restart does not open it. `open_on_start = false` gives this rule's behaviour (explicit action only). No countdown UI exists yet.
3. `SetKey`: base64, exactly 32 bytes, not all-zero; write to a temporary file in the same directory, `fsync`, `rename` to the key file (mode 0600), then activate. Never log the key or its length beyond "key set". One `SetKey` attempt per second.
4. After activation: close all sessions, refuse plaintext and zero-PSK, set TXT `api_encryption`.
5. The pairing result is shown on the TV: peer address and the HA `client_info` string the daemon already logs ("Home Assistant 2026.10.4"), so the owner can see that it is their HA and press "reset pairing" otherwise.
6. `reset-pairing` is local only (TV app, `ctl`, or deleting the key file with the daemon stopped). **Changed (2026-11-02, question 11):** a remote empty-key `SetKey` is accepted from the key holder only (a session keyed with the stored key, as HA sends it when its entry is deleted); it unpairs the device with the window **closed**, also across restarts, until a local open or reset. It is refused from keyless sessions and when the key is fixed in the configuration. HA keeps its stored key per MAC, so re-pairing a TV whose key was reset needs the device removed in HA first; the stable `mac_seed` must survive an IPK update and a reinstall (stored in the prefix, not regenerated). **Correction (PC run with HA 2026.10.4, section 8):** removal is not needed. After a reset HA's old key fails, HA opens a re-authentication flow (`reauth_encryption_removed_confirm`); once the user confirms it, HA sets the key from its own storage (the same key as before) and reconnects. A reset therefore does not change the key when the same HA re-pairs.
7. Optional hardening (open question 3): hold the `SetKey` reply until the owner presses OK on the TV. Not assumed possible; HA's reply timeout is unknown.

### 5.3 Threat model for the pairing window

| Threat | Result | Mitigation |
| --- | --- | --- |
| Passive sniffer | cannot read the key (ephemeral X25519 inside the zero-PSK session, documented) | none needed |
| LAN peer connects before HA | **can claim the device and later receive microphone audio** | explicit, short window; the owner opens it when HA is ready; peer and client string shown; reset pairing |
| Active MITM during the window | not prevented by the protocol (HA states this) | same as above; the window is the only exposure and the attacker needs to be on the LAN at that time |
| Plaintext session used to read data | only Hello and DeviceInfo, no state or audio | rule 1 |
| Key at rest | root on the TV can read it (same as the key file today) | mode 0600, in the prefix, never in logs or mDNS |
| Window left open forever | | timeout and RAM-only closed state |
| Device removed in HA (HA clears the key) | the device would be keyless; in ESPHome it accepts keyless sessions until a reboot | the clear needs the stored key; afterwards the window stays closed, persisted, until a local open or reset |
| Guest or IoT VLAN | HA cannot reach the TV or cannot see mDNS | document; fall back to host and port in HA |

## 6. Open questions

1. **mDNS on the TV.** Does `mdns-sd` coexist with `mdnsd`, `avahi-daemon` and `iconnectivity` on UDP 5353 on c3-a, does a real HA on the same L2 see the card, and does it survive interface changes and standby? If not, is an avahi service file or a call to LG's `mdnsd` a better route (a write outside `/tmp`, needs approval)?
2. **HBC package mechanics.** Does the installer keep the exec bit and an ELF payload in an app or service directory on 0.6.3 and 0.7.x? Which directory is writable and persistent for `etc/` and the key (is the app directory replaced on update)? Is `/media/developer` guaranteed? **Design answer (2026-11-03, [packaging](packaging.md#1-decisions)):** the package no longer depends on the exec bit (the daemon is copied and `chmod`ed into `/var/lib/webos-satellite`, scripts run with `sh`), state lives outside the app directory, and the app directory is taken from the app's own location. Still to verify on a TV: that appinstalld accepts the package ([EXP-009](../experiments/EXP-009-ipk-install-update.md)).
3. **Confirm-on-TV.** Can the `SetKey` reply be held until the owner confirms, and what is HA's timeout for `noise_encryption_set_key`?
4. **HA version support.** Accept only HA 2026.9.0 or newer for the no-typing flow, or also offer the old plaintext push (deprecated, to be removed in 2027.2.0 on the ESPHome side)? Recommendation: support only the zero-PSK path and document the manual key for older HA. **Implemented as recommended** (plaintext `SetKey` refused); still open for the owner to confirm.
5. **Other firmware.** What does the installer do on an unvalidated firmware? Recommendation: install, keep the mic closed, show why, and require an explicit "I understand" override before capture, because the capture ioctls and the DAPM oops hazard are validated only on c3-a ([improvements 4](../discovery/c3-a/rooted-tv-improvements.md)). **Superseded for the public release (owner, 2026-11-03):** no hard firmware whitelist; a published verified-models list, a sanitized compatibility report, and a "verified / not verified" status plus a firmware-changed warning instead of a block. Design: [packaging section 9](packaging.md#9-verified-models-list-instead-of-a-firmware-whitelist). **Further superseded (owner, 2026-11-04):** the mic starts on any firmware, with self-checks and a one-click report; see [firmware policy and reports](firmware-policy-and-reports.md). The current gate stays until the Homebrew Channel release.
6. **Architecture selection.** Which binaries to ship (armv7 musl only, or also aarch64) and how the installer picks them on a TV whose userland differs from c3-a. **Partly answered (2026-11-02):** both are shipped and `install.sh` picks by `uname -m` with a `--version` check and an armv7 fallback (section 4.1). Open: a run of the aarch64 daemon on c3-a, and a TV with a 32-bit kernel to confirm the fallback on hardware.
7. **Central repo and AI policy.** **Decided (2026-11-03):** submit to `webosbrew/apps-repo` with the AI-use disclosure required by its rule (H5); no self-hosted repository. Follow-up in [backlog #10](../project/backlog.md#10-official-homebrew-channel-listing). The repository license is decided (2026-11-03): GPL-3.0-or-later ([packaging P15](packaging.md#8-license-decision-and-dependency-audit)).
8. **Where does the first-run script run?** Through HBC `exec` from the app (needs root HBC) or from the init.d hook. Failure modes with a non-elevated HBC (0.7.1 and 0.7.2 fixes) are unknown.
9. **Entities.** Are `number` and `select` config entities worth the extra code in the daemon's ESPHome layer versus a TV app setting for mic gain? HA handles authentication, so the recommendation is entities.
10. **Window trigger on first run.** Auto-open on the first app launch only, or always require the button? Needs UX testing with non-technical users.
11. **Key clear on entry removal.** Accept HA's empty-key `SetKey` on an authenticated session (back to unpaired, as ESPHome) or keep reset local only and leave HA's stored key behind (measured 2026-11-02, section 8). **Decided (2026-11-02):** accept it from the key holder only, then unpaired with the window closed until a local action (rule 6). Not yet re-verified with a real HA.
12. **Power options UI.** Which TV power settings the app and HA expose (screen policy, Always Ready, Quick Start+), whether the daemon may write them through `settingsservice` (private luna API, root caller allowed is a hypothesis), and how the risk text is shown where an HA entity cannot hold a paragraph: [always-on section 7](always-on.md#7-exposing-power-options-in-the-ui-design), decided after the D5 results. **Answered (owner 2026-11-05, [ADR 0008](../decisions/0008-user-initiated-lg-settings.md)):** the app exposes Always Ready, LG hands-free and the keep-listening select on a "Voice and power" screen ([design](voice-and-power-screen.md)); the app, not the daemon, writes the two LG settings through `ctl.sh` (private luna methods, the root caller works on c3-b, payloads pending EXP-011); Quick Start+ and the screen policy are not exposed; HA gets two read-only sensors and the risk text lives in the app.

## 7. What must be verified on a TV before acceptance

Each item is a bounded experiment with its own plan, run only with the owner's authorization, read-only first, and recorded as `verified-on-target`, negative or inconclusive with firmware, HBC version and power state. Items 1 to 3 and 7 to 10 have not been run (items 1 to 3, 7 and 10 are planned for c3-b in [EXP-009](../experiments/EXP-009-ipk-install-update.md), with the real package instead of a test IPK); 4 to 6 were partly run on c3-a on 2026-11-02 (section 8).

1. **Test IPK** (an empty app plus a payload with a `chmod +x` script): installs through `luna-send` or HBC from a hashed manifest, exec bit and directory layout survive, uninstall leaves nothing behind.
2. **`exec` as root:** the app's `exec` call runs as root with HBC elevated on c3-a (0.6.3); a clear failure when it is not.
3. **init.d symlink hook** from the package directory: returns at once, no failsafe, survives an IPK update, disappears on uninstall; test reboot approved by the owner ([autostart design](autostart.md#proposed-install-needs-the-owners-approval-not-done)).
4. **mDNS advertisement** from the daemon: `_esphomelib._tcp` visible from another host on the LAN, HA discovery card appears, no disturbance of LG's responders (check that `mdnsd` and `avahi-daemon` still answer), behaviour across screen-off and standby (D5). **2026-11-02:** seen from a PC on the TV's L2 with LG's responders unaffected; HA discovery and standby still open ([P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md)).
5. **Unprovisioned mode, PC first:** the daemon passes the section 4.1 test list against `aioesphomeapi`; then against the owner's HA with a throwaway entry: discovery or host and port, no key typed, the key stored in HA, reconnect after an HA restart and after a daemon restart. **PC part done** (section 8), including a real HA 2026.10.4 on the PC; **TV with the owner's HA done (2026-11-02, section 8)** except an HA restart (an entry reload was used).
6. **Pairing window:** closes on timeout; refuses connections afterwards; reopens only on the local action; no audio in any keyless session; no key in logs or `ps` output; the key file mode is 0600 and survives a reboot. **PC part done**; on the TV (2026-11-02): no audio before the key, no key in logs or `ps`, file mode 0600; the timeout refuses connections but HA shows `connection_error` instead of the setup-window message (section 8, item 4; the race is fixed since, re-test with the owner's HA pending). Reboot not done.
7. **Reset and reinstall:** `reset-pairing`, IPK update and reinstall keep the stable MAC and either re-pair cleanly or tell the user what to do in HA.
8. **Firmware status through the installer (updated 2026-11-04):** an unverified firmware does not keep the mic closed; the app shows the status and a "Report compatibility" button (QR code), and a failed self-check closes the mic with the reason shown. The old block exists only with `firmware_policy = "strict"`.
9. **No secret leakage:** the app screens, `ctl status`, `--check-config`, logs and mDNS TXT contain no key, token or other secret (grep test on a PC run, then on the TV).
10. **HBC variance:** repeat 1 to 3 on HBC 0.7.x (second C3 or another rooted TV) before the compatibility record claims anything beyond c3-a.

## 8. Implementation status (milestone 3b)

Implemented on 2026-11-02 in commits 84de706 (API crate) and 849a1dd (daemon). The implementation notes and test results below are from the PC; the TV verification against the owner's HA (2026-11-02) is at the end of this section.

**What exists**

- `webos-esphome-api`: `Config::provisioning` ([`pairing.rs`](../../crates/esphome-api/src/pairing.rs), [`server.rs`](../../crates/esphome-api/src/server.rs)). Behaviour, divergences from ESPHome and test lists: [ESPHome API crate, Pairing](../development/esphome-api.md#pairing-ha-driven-key-provisioning).
- Daemon: `[pairing]` (`enabled`, default false; `state_dir`, `window_s`, `open_on_start`), `--pair`, `--reset-pairing`, trigger files `pair` and `reset-pairing`, the "not paired" microphone gate. User guide: [development, Pairing](../development.md#pairing-without-typing-a-key). Security notes: [SECURITY.md](../../SECURITY.md#pairing).
- `[pairing] enabled` defaults to false: a config without a key still fails to start as before, so nobody gets an open pairing window by accident. The first-run script of milestone 3a would set it.
- mDNS follows the key state (section 5.1) but stays off by default (`device.mdns = false`) because the advertiser is unverified on c3-a, where UDP 5353 is shared with `mdnsd`, `avahi-daemon` and others (2.3). Discovery would let HA offer the device with one click during the window; without it the user adds host and port, which the PC run showed works without typing a key.

**Decisions taken while implementing**

| Question | Decision | Basis |
| --- | --- | --- |
| Rotation by a session that proved the key | allowed (new valid key; same key is a no-op) | ESPHome 2026.10.1 `api_connection.cpp` accepts `SetKey` on any authenticated session; HA 2026.10.4 never rotates |
| Empty key (clear) | **changed 2026-11-02:** accepted from a session keyed with the stored key only; then unpaired with the window closed, persisted (`<key_file>.cleared`), until a local open or reset; refused from keyless sessions and for a key from the configuration | question 11; ESPHome 2026.10.1 accepts it from any session and then stays keyless and open until a reboot (`api_server.cpp` `clear_noise_psk`, `provisioning:`) |
| `DisconnectRequest(PROVISIONING_CLOSED)` on a closed window | **changed 2026-11-02:** after the client's next message (or 2 s), not right behind the `HelloResponse` as ESPHome 2026.10.1 does | section 8 item 4: aioesphomeapi 46.6.0 reports the reason only after `connect()` finished |
| `SetKey` over plaintext | refused | open question 4; ESPHome deprecates it |
| Key fixed in the config | never changed (`success = false`) | ESPHome with a YAML key |
| Corrupt stored key at start | the daemon refuses to start | fail safe: no silent reopening of pairing |
| Rate limit | one `SetKey` attempt per second (any session) | rule 3 |

**Test results (2026-11-02, host x86_64):** API crate 54 unit tests, 4 loopback tests and 7 pairing loopback tests; satellite 51 unit tests; `fake_ha.py` 58 checks (16 new: scenarios D and E); `satellite_e2e.py` 29 checks (12 new: scenarios P and W); `cargo fmt`, `clippy -D warnings`, the armv7 and aarch64 musl release builds and `tools/check_docs.py` pass. Real HA 2026.10.4 on the PC: the flow worked end to end without typing a key ([development](../development.md#testing-without-ha-reaching-the-device) has the observations and how to repeat it).

**Verification on a TV with the owner's HA (2026-11-02, [P3 TV verification](../discovery/c3-a/P3-tv-verify-aec-pairing.md#3-pairing-on-the-tv-with-the-owners-ha)).** A new test device (`webos-sat-pairtest`, new `mac_seed`) on c3-a, HA 2026.10.4 in its pod, removed afterwards:

1. **verified-on-target:** HA added the TV by host and port with `[pairing] enabled = true`, no key typed. The key in the new entry, in HA's key storage and in `<state_dir>/noise_psk` on the TV are the same (hashes compared); the file is mode 0600 in a 0700 directory.
2. **verified-on-target:** HA reconnected with Noise 5 s after pairing, after a daemon restart (4 s) and after a reload of the entry (25 ms). An HA restart was **not** done (the reload stands in for it).
3. **verified-on-target:** the `reset-pairing` trigger file deleted the key, closed the session and the mic and reopened the window; HA opened `reauth_encryption_removed_confirm` on the test entry only; confirming it made HA set its stored key again and reconnect.
4. **negative:** after the window timed out, HA's config flow showed `connection_error` (4 of 4), not "closed its setup window", although the daemon sent `DisconnectRequest(PROVISIONING_CLOSED)` after the hello. From the PC on the TV's L2 the same client code got `provisioning_closed` 5 of 5. Hypothesis: aioesphomeapi 46.6.0 reports the reason only when the disconnect arrives after `connect()` finished, so the two back-to-back frames race. Proposed fix (not implemented): send the `DisconnectRequest` only after the client's next message or about 250 ms. **Fixed afterwards (2026-11-02, PC only):** confirmed from aioesphomeapi 46.6.0 (`connection.py` `_cleanup` fires the closed event only when `was_connected`) and HA 2026.10.4 (`config_flow.py` `_fetch_device_info` maps the error to `provisioning_closed` only after that event); the server now sends the request after the client's next message (HA's `DeviceInfoRequest`) or 2 s. A fake-HA proxy that batches the device's frames reproduces the TV result with the old build (6 of 6 `connection_error`) and gives `provisioning_closed` 6 of 6 with the new one. **Not yet re-verified with the owner's HA.**
5. **verified-on-target:** the mic PCM stayed `closed` until the key was set and opened 33 ms after it; the key appears in no log file under the prefix and not in `ps`.
6. **partly verified:** with `device.mdns = true` the PC on the TV's L2 saw `_esphomelib._tcp` (no `api_encryption` TXT while unpaired) and LG's `mdnsd`, `avahi-daemon` and `iconnectivity` kept running and answering; the record disappeared on stop. The owner's HA (pod network) saw no discovery, as expected; discovery by an HA on the same L2 is still open.

Findings for the design:

- **Entry removal:** HA 2026.10.4 tries to clear a dynamically provisioned key on the device (empty `SetKey`) when the entry is deleted. The daemon refused it then (old rule 6), so HA kept the generated key in its `esphome.encryption_keys` storage and the device stayed paired. Re-adding the same device works because HA reuses its stored key. **Changed afterwards (question 11, PC only):** the clear is accepted from the key holder; HA then removes its stored key (it does so only on `success = true`, `__init__.py` `_async_clear_dynamic_encryption_key`), and the device stays unpaired with the window closed until a local action. Not yet re-verified with the owner's HA.
- **Same port, other identity:** an existing entry that reaches a device with another MAC logs `Unexpected device found` and **stops reconnecting** until the entry is reloaded or HA restarts (no re-authentication, no change to the entry). Testing a second identity on a TV's only reachable port therefore needs a reload of the permanent entry afterwards.

## Sources

HBC and community packages (read 2026-11-02):

- [webos-homebrew-channel](https://github.com/webosbrew/webos-homebrew-channel): README (Luna `install`, `exec`, `spawn`, `/var/lib/webosbrew/init.d`, failsafe), `services/startup.sh`, `services/elevate-service.ts`, `services/jumpstart.sh`, `tools/gen-manifest.js`, `CHANGELOG.md`.
- [webosbrew/docs](https://github.com/webosbrew/docs) schemas `HomebrewPackageManifest.schema.json` and `HomebrewRepository.schema.json`.
- [webosbrew/apps-repo](https://github.com/webosbrew/apps-repo) README (submission format and rules); [repo.webosbrew.org apps.json](https://repo.webosbrew.org/api/apps.json) (observed contents).
- [TBSniller/piccap](https://github.com/TBSniller/piccap) (`frontend/js/servicecalls.js`, `servicefiles/`, `package.json`), [webosbrew/hyperion-webos](https://github.com/webosbrew/hyperion-webos) (`src/service.c`), [webosbrew/hyperion.ng-webos-loader](https://github.com/webosbrew/hyperion.ng-webos-loader), [webosbrew/webosbrew-autostart](https://github.com/webosbrew/webosbrew-autostart).
- [webosbrew.org rooting guide](https://www.webosbrew.org/rooting/); [LG webOS TV CLI guide](https://webostv.developer.lge.com/develop/tools/cli-dev-guide); [webOS OSE native services tutorial](https://webosose.org/docs/tutorials/native-services) (OSE, not proof for commercial TVs).

ESPHome and Home Assistant:

- HA [2026.10.4 `esphome/config_flow.py`](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/config_flow.py), [`manager.py`](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/manager.py) (`_handle_dynamic_encryption_key`, `_async_provision_key_over_noise`), [`strings.json`](https://github.com/home-assistant/core/blob/2026.10.4/homeassistant/components/esphome/strings.json); tags 2026.8.0 and 2026.9.0 compared for `api_encryption_provisionable`.
- ESPHome [2026.10.1 `api_connection.cpp`, `api_server.cpp`, `api_frame_helper_noise.cpp`](https://github.com/esphome/esphome/tree/2026.10.1/esphome/components/api) and [`provisioning` component](https://github.com/esphome/esphome/tree/2026.10.1/esphome/components/provisioning); the vendored copy of `api.proto` is in [`crates/esphome-api/proto/`](../../crates/esphome-api/proto/api.proto).

Repository evidence: [`server.rs`](../../crates/esphome-api/src/server.rs), [`mdns.rs`](../../crates/esphome-api/src/mdns.rs), [`entities.rs`](../../crates/esphome-api/src/entities.rs), [`config/example.toml`](../../config/example.toml), [`packaging/webos/`](../../packaging/webos/).
