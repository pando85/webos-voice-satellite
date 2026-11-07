# Security and privacy

Root-level access to a TV is sensitive. Until a production security design is accepted, the project only ships read-only diagnostics and research documentation.

## Requirements

- No unsolicited ambient recording or audio transmission.
- Explicit user opt-in for microphones; clear local mute and visible/inspectable listening state are future acceptance criteria.
- No capture logs or PCM dumps in Git. Keep device data private and redact identifiers before publishing.
- Never place Home Assistant long-lived tokens in scripts, examples, process arguments, public logs, or PRs. Explore least-privilege credentials and secure provisioning.
- No unprotected listening sockets or unauthenticated control endpoints; network transports must use appropriate authentication.
- Boot hooks, service overlays and firmware modifications need a rollback plan and explicit approval.
- Treat downloaded software, binary dumps and reverse-engineering outputs as untrusted.

## Pairing

The optional `[pairing]` mode lets Home Assistant (2026.9 or newer) set the Noise key itself, so nobody types or copies it. Know its limits before you enable it ([details](docs/development.md#pairing-without-typing-a-key), [threat model](docs/design/install-and-config-ux.md#53-threat-model-for-the-pairing-window)):

- **The key exchange is not authenticated.** HA sends the key inside a Noise session that uses the publicly known all-zero key. A fresh key exchange inside it stops passive sniffing, but in HA's own words it "does not authenticate either side against an active man in the middle". Whoever on the LAN connects first while the pairing window is open can set the key, and from then on receive microphone audio.
- **The window is the only protection.** It is open only while the device has no key, for `pairing.window_s` (default 10 minutes), and closes as soon as a key is set. Open it (start, `--pair`, the `pair` file) only when you are about to add the device in HA, and check the `PAIRED by "<client>" from <address>` log line afterwards. If it is not your HA, reset the pairing.
- **Before a key exists** a connection gets the device identity, an empty entity list and the key exchange only: no audio, no voice pipeline, no controls. The microphone stays closed.
- **After pairing** only the key holder can connect. The key is stored with mode 0600 in `pairing.state_dir` and is never logged. Anyone with root on the TV can read it, as any key file there.
- **Reset is local; only the key holder can clear remotely.** `--reset-pairing` or the `reset-pairing` file deletes the key and reopens the window. Remotely, only a connection that proved the stored key can clear it (Home Assistant does this when you delete the device); the satellite then stays unpaired with the pairing window **closed**, also after a restart, so nobody else can pair it until you open the window locally (`--pair` or the `pair` file). A clear from a connection without the key is refused. A key in the configuration (`device.noise_psk_file`) is never changed or cleared and disables pairing entirely; prefer it on networks you do not trust.

## App package

The webOS app package ([packaging design](docs/design/packaging.md)) adds no listening socket beyond the satellite's API port. Its status app runs the package's own `ctl.sh` as root through the Homebrew Channel's `exec`, with a fixed list of arguments and never with typed or received text. The daemon runs as root from `/var/lib/webos-satellite`, where the config, the key (mode 0600) and the logs stay until "Remove service and all data" (`ctl.sh purge`); removing only the app keeps them. Start at boot is off until the user turns it on.

The app's own directory is laid out by the TV's installer and can be writable by other users (mode 0777 on one tested TV), so root never runs a file from it that is not owned by root, and once the satellite is installed the app runs the installed `ctl.sh` instead of its own. Everything root runs from the install directory is checked first (owned by root, not writable by group or others; `run/` private); see [file permissions of what runs as root](docs/security/install-permissions.md).

For security issues, contact maintainers privately instead of disclosing exploitable device details in a public issue. A dedicated reporting channel will be specified before production releases.
