# P3: AEC on the TV (CPU, daemon smoke) and pairing with the owner's HA (c3-a)

- **Device:** c3-a (LG C3), webOS TV 8.3.0, firmware 03.30.74, 4 Cortex-A76 cores at 1.4 GHz (`performance` governor), 32-bit userland. Power state `Screen Saver` at the start and `Active` at the end (nothing in this session changed it), foreground HDMI 1 (the PC's idle desktop, no audio playing), volume 6, not muted, `tv_speaker`, LG hands-free off. Load average about 13 from LG processes all session; `MemAvailable` about 480 MB.
- **Date:** 2026-11-02, unattended (nobody in the room, no audible sound).
- **Binaries:** built from commit 696393f in a clean worktree: `webos-satellite` and the `aec_offline` example, static `armv7-unknown-linux-musleabi` (VFPv3-D16 hardware float, the repository's `.cargo/config.toml`), 3,061,796 and 751,812 bytes; for comparison `aec_offline` for `aarch64-unknown-linux-musl` (707,224 bytes). Checksums compared on the TV. Installed with `packaging/webos/install.sh` under `/tmp/webos-satellite`, removed with `uninstall.sh`.
- **HA:** the owner's HA 2026.10.4 (aioesphomeapi 46.6.0) in its Kubernetes pod, reaching the TV on TCP 6053 directly.
- **Related:** [AEC front-end, section 9](../../design/aec-frontend.md#9-on-tv-verification-plan-pending), [install and configuration UX, section 8](../../design/install-and-config-ux.md#8-implementation-status-milestone-3b), [P3 robustness](P3-robustness.md), [P1 HA integration](P1-ha-integration.md).
- **Privacy and safety:** no sound was played (`pacat` sink at in-process gain 0; nothing was ever sent to it). The live mic streamed to HA's wake word stage only, through a temporary pipeline whose conversation agent could not match a command (built-in agent, language `ru`), selected on each satellite and deleted afterwards. Nothing was recorded on the TV. No ASoC/DAPM sysfs or debugfs node was read. Only `/tmp` was written and it was cleaned. Raw outputs are in the gitignored `devices/private/c3-a/P3-tv-verify/`.

Labels: **verified-on-target**, **verified-on-host**, **observed-on-other-device** (the dev PC), **hypothesis**, **not verified**.

## Summary

- **AEC CPU on the TV (armv7):** the canceller costs about **1.0 ms per 10 ms frame**, 23 times the host figure. The whole stage runs at a real-time factor of **0.11** (11 % of one core); with NS 0.16. The aarch64 build of the same code runs at **0.028** (0.21 ms per frame), four times faster. Peak RSS of the offline tool is 6.9 MB with the canceller and 2.1 MB without. **verified-on-target**
- **Same numbers as the emulator:** the TV's ERLE and delay match the armv7 binary under `qemu-arm` exactly; the small differences from x86_64 (up to 0.9 dB) are codegen, not the TV.
- **Daemon with `[aec] enabled = true`, 10 minutes, wake mode:**
  - `SoundEngineCapture` opened with the satellite's own ioctls and **linked** to the mic: both trigger timestamps within 2 µs.
  - CPU **13.0 % of one core** (P3 without AEC: 1.4 %), RSS 1.86 MB and flat.
  - 0 xruns, 0 reference underruns, 0 dropped chunks.
  - Delay lock and ERLE were **not verified**: the TV played nothing, so the reference had 0 active frames.
  - With `bypass_when_silent = true` the daemon used **2.8 %**.
- **PCM gate with AEC:** HA's mute, the kill-switch file and `stop.sh` closed **both** the mic and the reference PCM. Unmute and kill-switch removal reopened both, linked again. **verified-on-target**
- **Pairing with the owner's HA** (a new test device on the TV, no key typed):
  - Pairing, the 0600/0700 key file, the Noise reconnect, the entities, the mic gate, no key in the logs, the restart and reload reconnects, and reset with HA's re-authentication all passed. **verified-on-target**
  - **Window timeout: negative.** HA's flow showed `connection_error`, not "closed its setup window". The cause is a timing race (below); a fix is proposed.
  - **Entry removal:** HA tries to clear the key on the device. The satellite refuses by design, so HA keeps the generated key in its key storage.
- **Identity isolation:** the permanent `lg-c3-a` entry did connect to the test daemon (same host and port). HA rejected it as a different device by MAC (`Unexpected device found`), with no re-authentication, repair or entry change. That check also **stops** the entry's reconnect loop, so HA needed one reload of `lg-c3-a` to reconnect afterwards.
- **mDNS:** the advertisement was visible from the PC on the same L2, and LG's responders (`mdnsd`, `avahi-daemon`, `iconnectivity`) kept running and answering. HA in its pod saw nothing, which was expected.

## 1. AEC CPU benchmark (section 9 step 1)

`aec_offline` (the daemon's front-end on a recorded pair; no audio device opened) ran 3 times per variant under BusyBox `time -v`, writing to `/dev/null`. Inputs:

- `drift`: the P2 session 1 tap3-drift-v10 pair (chirps, 183.75 s of audio).
- `c10`: the first 180 s of the session 2 `content-v10` pair (Spanish and English speech, then music with vocals at TV volume 10; 179.75 s after the 250 ms skip). It was trimmed because the full 10-minute pair (307 MB) would not fit comfortably in RAM-backed `/tmp`.

"µs/frame" is the canceller's own time per 10 ms frame (`cpu_us_mean`); RTF is wall time over audio time for the whole stage.

| Build, input, variant | Wall (s) | RTF | µs/frame mean | µs/frame max | Peak RSS | ERLE / delay (tool) |
| --- | --- | --- | --- | --- | --- | --- |
| armv7, drift, canceller (default) | 20.40 to 21.19 | 0.111 to 0.115 | 1008 to 1046 | 5986 to 7040 | 6.8 to 6.9 MB | 22.53 dB / 16.52 ms |
| armv7, drift, `--ns` | 27.92 to 29.17 | 0.152 to 0.159 | 1419 to 1477 | 3140 to 5047 | 7.1 MB | 32.63 dB / 16.52 ms |
| armv7, drift, `--bypass` (14,034 of 18,375 frames bypassed) | 5.84 to 6.11 | 0.032 to 0.033 | 999 to 1047 (processed frames) | 1818 to 4218 | 6.9 MB | 22.28 dB / 16.52 ms |
| armv7, drift, `--no-aec` | 0.53 to 0.58 | 0.003 | | | 2.1 MB | |
| armv7, c10, canceller (default) | 19.82 to 20.12 | 0.110 to 0.112 | 1001 to 1013 | 2616 to 5028 | 6.9 MB | 16.61 dB / 18.28 ms |
| armv7, c10, `--ns` | 28.07 to 28.22 | 0.156 to 0.157 | 1452 to 1457 | 5821 to 7961 | 7.1 MB | 24.50 dB / 18.28 ms |
| armv7, c10, `--bypass` (54 frames bypassed: content never silent) | 19.52 to 20.36 | 0.109 to 0.113 | 989 to 1024 | 2132 to 3524 | 6.9 MB | 16.83 dB / 18.28 ms |
| armv7, c10, `--no-aec` | 0.50 to 0.53 | 0.003 | | | 2.1 MB | |
| armv7, c10, canceller, a second core busy (`yes`), 1 run | 19.46 | 0.108 | 983 | 5821 | 6.8 MB | 16.61 dB |
| aarch64, c10, canceller | 4.96 to 5.27 | 0.028 to 0.029 | 207 to 220 | 1539 to 4034 | 6.9 to 7.2 MB | 16.72 dB / 18.28 ms |
| aarch64, c10, `--ns` | 5.66 to 6.22 | 0.032 to 0.035 | 245 to 268 | 2637 to 4394 | 7.3 MB | 23.72 dB |
| aarch64, c10, `--no-aec` | 0.33 to 0.36 | 0.002 | | | 2.1 MB | |
| host (Ryzen 9 5950X), c10, canceller | 1.19 to 1.19 | 0.0066 | 43 | 87 to 1075 | 4.2 to 4.6 MB | 16.80 dB / 18.28 ms |
| host, drift, canceller | 1.24 to 1.26 | 0.0068 | 44 | 95 to 116 | 4.3 to 4.4 MB | 22.49 dB / 16.52 ms |
| host, c10, `--ns` | 1.45 to 1.46 | 0.0081 | 57 to 58 | 137 to 155 | 4.3 MB | 23.60 dB |

- **verified-on-target** for the TV rows; **verified-on-host** for the host rows. The tool's ERLE is a broadband figure over active-reference frames, not the in-band 200 to 4000 Hz figure in the design document, so compare it only within this table.
- **Host versus TV:** the canceller is 23 times slower per frame on the armv7 build (1008 against 44 µs) and the whole stage 16 to 17 times. On the TV the stage is 11 % of one core, against 0.6 % on the host.
  - That is in the design's "10 to 30 %" band: keep the canceller, use `bypass_when_silent`, and watch thermal and watchdog behaviour.
  - Single frames took up to 8 ms (offline) and up to 12.6 ms (daemon, below) on a loaded TV, against a 10 ms frame. The 512 ms ALSA buffer absorbed this: no overrun in the daemon run.
- **aarch64:** the same code at 0.21 ms per frame. sonora has SIMD for aarch64 but not for armv7 (**documented**, research), which fits the four-fold gap. The aarch64 build ran on c3-a, so a satellite built for aarch64 would cost about 3 % of a core with the canceller (**hypothesis** until the daemon runs as aarch64 on the TV; the other daemon threads were not measured in that build).
- **Determinism:** the armv7 binary under `qemu-arm-static` on the PC gave exactly the TV's ERLE (16.61 and 24.50 dB on c10) and delay. The host-to-armv7 differences are therefore codegen (float operation order), not a TV-specific fault. The design document's earlier note "same ERLE as the host build" held for other recordings only.
- A second busy core did not change the result.

## 2. Daemon smoke with AEC (section 9 step 2)

Config: wake mode, `lg-c3-a` identity and key (the permanent entry connected), `[aec] enabled = true` with the defaults, `[sink] pacat` gain 0.0 and `latency_msec` 200, log level debug. HA reconnected by itself 25 s after the start; its pipeline select was then switched to the temporary pipeline. Sampling of `/proc/<pid>/stat`, `/proc/<pid>/status` and `/proc/asound/card1/pcm{0,3}c/sub0/status` every 13 s from the PC; a 10 ms polling loop on the TV logged PCM state changes during the gate tests.

| Check | Result | Label |
| --- | --- | --- |
| Reference opens and is linked | `aec: echo cancellation on: reference PCM "SoundEngineCapture" (2 ch, 48000 Hz, s32_le), linked start`. `hw_params` as requested on both (mic S24_LE 2 ch 16 kHz, reference S32_LE 2 ch 48 kHz, period 256, buffer 8192); both `trigger_time` values equal within 2 µs; `hw_ptr` ratio 3.0008 after 75 s | verified-on-target |
| Delay lock | none: `delay n/a` in every 60 s line, because the reference had **0 active frames** (the TV played nothing) | not verified |
| ERLE estimate | `ERLE n/a dB (sonora 0.2 dB) over 0 active-reference frames`, for the same reason | not verified |
| CPU | 12.97 % of one core over 539 s (`utime` 63.4 s, `stime` 6.6 s); the daemon's own line: sonora 1280 to 1314 µs per frame mean, max 7.8 to 12.6 ms, 12.8 to 13.1 % of a core | verified-on-target |
| CPU with `bypass_when_silent = true` (2-minute run) | 2.8 % of one core; after the first 5 s every frame was bypassed (6000 of 6000 in the second minute) | verified-on-target |
| Memory | VmRSS and VmHWM 1856 kB, flat for 10 minutes (2120 kB in the bypass run) | verified-on-target |
| Xruns, underruns, drops | `xruns 0`, `reference underruns 0`, `dropped 0` in every 60 s line | verified-on-target |
| Mic levels | in -67 to -69 dBFS RMS, out (+24 dB) -44 to -45 dBFS: a quiet room | verified-on-target |
| HA mute switch on | `microphone MUTED by Home Assistant`, then `microphone closed, PCM released` 5 ms later; **both** `pcm0c` and `pcm3c` `closed` in the same 10 ms poll | verified-on-target |
| HA mute switch off | both reopened (the mic in `SETUP` for about 30 ms, then both `RUNNING`), linked again (trigger times equal) | verified-on-target |
| Kill-switch file created | both `closed` within 110 ms of the `touch` (TV clock, including the shell); HA showed the mute switch on; HA's unmute was refused (`unmute refused: kill switch is active`) | verified-on-target |
| Kill-switch file removed | both `RUNNING` 140 ms later, linked | verified-on-target |
| `stop.sh` | the reference closed first (it is unlinked before close), the mic 40 ms later; all LGSIC capture PCMs `closed` | verified-on-target |

Nothing misbehaved with the reference open, so AEC was not disabled. The [design's](../../design/aec-frontend.md#5-failure-modes) row "Mute, kill switch, firmware guard, unpaired: both PCMs closed" is now **verified-on-target** for mute, kill switch and stop.

## 3. Pairing on the TV with the owner's HA

### Identity, and why the permanent entry was affected

HA's ESPHome entries are keyed by MAC (`unique_id`).

- **New device:** the config flow aborts only on the same MAC. A **name** match against another entry opens a name-conflict step.
- **Existing entry:** while it reconnects, a device with another MAC is rejected:
  - after a good handshake: `Unexpected device found`, plus a repair issue if the names match;
  - after a failed Noise handshake: through the name and MAC in the server hello.

  Either way the entry's reconnect loop **stops** until the entry is reloaded or HA restarts. **documented** (HA 2026.10.4 `esphome/manager.py`, `config_flow.py`)

The test daemon therefore used a new name (`webos-sat-pairtest`), a new friendly name and a new `mac_seed`. The `lg-c3-a` entry still dialled 6053 and reached it (the only port HA can reach on the TV):

- Its Noise handshake failed with the test daemon's name and MAC. HA logged `Unexpected device found ... expected lg-c3-a ... found webos-sat-pairtest` and stopped that entry's reconnect loop.
- There was no re-authentication flow, no repair issue and no change to the entry. **verified-on-target**
- Consequence: after the test, HA did not reconnect to the restarted `lg-c3-a` satellite by itself (0 connection attempts in 100 s). One reload of the entry (no data or key change) restored it; HA connected 0.4 s later.

### Results

The daemon config: `[pairing] enabled = true`, `state_dir = "pairing"` (under the `/tmp` prefix), no key, wake mode, gain 0.

| Install-UX item | Check | Result | Label |
| --- | --- | --- | --- |
| 1 | Config flow by host and port 6053, no key typed | `create_entry` "Pairing test (c3-a)" 0.13 s after the submit. The daemon logged a keyless plaintext session (HA's device-info probe), then a zero-PSK Noise session: `key set ... (mode 0600)`, `PAIRED by "Home Assistant 2026.10.4"` | verified-on-target |
| 1 | Key at rest | `etc/pairing` mode 700, `noise_psk` mode 600. SHA-256 of the TV key = HA's config-entry key = HA's `esphome.encryption_keys` entry (hashes compared; no key printed) | verified-on-target |
| 2 | Noise reconnect | HA came back with Noise 5.1 s after the key was set (two plaintext probes rejected first, as on the PC) | verified-on-target |
| 2 | Entities | assist satellite `idle`, media player `idle`, mute switch `off`, two pipeline selects, the finished-speaking select; two wake word selects `unavailable` (expected) | verified-on-target |
| 5 | Mic closed until the key was set | `capture: microphone off (not paired)` at start; the PCM stayed `closed` from the start until the key (7.8 s), then `RUNNING` 33 ms after `PAIRED` | verified-on-target |
| 5 | No key in logs | the base64 key compared in memory on the TV: 0 matches in the daemon log and every file under the prefix; not in `ps` | verified-on-target |
| 2 | Daemon restart | `paired (stored key)`, no window opened; HA reconnected 4 s after the start | verified-on-target |
| 2 | HA-side reload of the entry (stand-in for an HA restart; HA was not restarted) | HA reconnected 25 ms after the disconnect. HA logged one warning `Ignored unload request ... esphome.assist_satellite; no platform is loaded` (HA-side, no effect seen) | verified-on-target (reload only) |
| 3 | Reset via `<state_dir>/reset-pairing` | within 1 s: key deleted, trigger file removed, HA session closed, mic `closed`, window reopened. HA's next connect failed (`decrypt error`) and HA opened `reauth_encryption_removed_confirm` **on the test entry only**. Confirming it returned `reauth_successful`; HA set its stored key over zero-PSK 0.4 s later, the mic reopened and HA reconnected | verified-on-target |
| 3 | Key after reset | equals HA's stored key (entry and storage hashes). That it equals the first key is inferred from HA's code path (stored key reused) and the PC run; the first hash was not kept | verified-on-target / inferred |
| 4 | Window timeout | started with `window_s = 5`. After the window closed, HA's flow returned **`connection_error`** in 4 of 4 tries, not `provisioning_closed`. The daemon did send `HelloResponse` then `DisconnectRequest(PROVISIONING_CLOSED)` | **negative** |
| 4 | Same check from the PC (aioesphomeapi 46.6.0, HA's flow code path, same L2 as the TV) | `provisioning_closed` in 5 of 5 | observed-on-other-device |
| | Entry removal | HA connected and sent an empty `SetKey` (its "clear the dynamic key on removal" step); the daemon refused (`an empty key (clear) is refused; reset pairing locally`). HA then **keeps** the generated key in `esphome.encryption_keys` (one entry for the test MAC remains). Entry, device and entities are gone | verified-on-target |
| 6 | mDNS (separate run, unpaired, `device.mdns = true`) | `_esphomelib._tcp` `webos-sat-pairtest` seen from the PC with TXT friendly name, version, platform, board and network, and no `api_encryption` (unpaired); the TV's own `_display._tcp` still answered; `mdnsd`, `avahi-daemon`, `iconnectivity` still running; UDP 5353 sockets 8 → 10 → 8; the record was gone after the stop. HA showed **no** zeroconf discovery flow in 48 s: its pod is not on the TV's L2 (no host network) | verified-on-target (PC on the same L2); HA discovery not possible here |

### Why the window timeout shows `connection_error` (hypothesis)

From aioesphomeapi 46.6.0's source (in HA's pod):

- The connection-closed event that carries the reason fires only if the connection had already reached `CONNECTED` (`was_connected` in `_cleanup`).
- If `HelloResponse` and `DisconnectRequest` are handled in the same read, the disconnect cleans up while `connect()` is still finishing. `connect()` then fails without the event, and HA's flow maps that to `connection_error`.
- From the PC, one hop away, the two frames were handled apart every time. From HA's pod, behind a router, they never were.

ESPHome 2026.10.1 also sends the two messages back to back (`api_connection.cpp`), so a real ESPHome device may show the same race (**hypothesis**, not tested).

**Proposed fix (not implemented):** after the `HelloResponse` on a closed window, wait for the client's next message (HA's flow sends `DeviceInfoRequest`), or about 250 ms, before sending `DisconnectRequest`. The client is then `CONNECTED` and receives the reason. Then re-test against the owner's HA.

## Negative and inconclusive results

- The delay lock and the in-daemon ERLE were not verified: there was no reference signal, because the TV played nothing. They need content or a test signal over HDMI (section 9 steps 2 and 3, owner's sound window).
- The pairing window timeout does not reach HA's "closed its setup window" message from the owner's HA (above).
- HA's discovery of the mDNS record could not be tested from the owner's HA (pod network).
- An HA restart was not done (the reload stands in for it).

## Residue and rollback (verified at the end)

- **TV:**
  - `/tmp/webos-satellite`, the benchmark directory, the polling script and the trigger and marker files were removed. `/tmp` lists the same names as at the start.
  - No `webos-satellite`, `pacat`, polling or `yes` process; 0 sink-inputs.
  - All capture PCMs of both cards `closed`; kernel taint 4097 (unchanged).
  - Volume 6, not muted, HDMI 1, LG hands-free off. The power state was `Active` (it was `Screen Saver` at the start; not changed by this session).
- **HA:**
  - The test entry, device and entities are deleted, and no ESPHome flow is open.
  - The temporary pipeline is deleted (Kiwi is preferred, Kiwi Test unchanged), and `lg-c3-a`'s pipeline select is back to `preferred`.
  - `lg-c3-a` (same entry id, title and source) still has the same key (hash compared with the private key file). It was reloaded once, and no ESPHome repair issue exists.
  - Residue: one generated key for the deleted test MAC in HA's `esphome.encryption_keys` storage (see entry removal above); it cannot be removed through HA's API and is harmless.
- **Tokens:** this session's token was revoked (API answers 401 with it). Eight earlier agent refresh tokens from the same day (minted through the login flow, never used again) were revoked too.

## Open items

1. Implement and re-test the deferred `DisconnectRequest` for the closed window.
2. Decide whether the satellite should accept HA's empty-key clear on entry removal (as ESPHome does) or keep "reset is local" and document HA's leftover key. Re-adding works either way, because HA reuses its stored key.
3. Measure the delay lock and ERLE in the daemon with content on the TV (owner's sound window), then section 9 step 3.
4. Decide the TV build: aarch64 makes the canceller four times cheaper. Run the daemon as aarch64 on c3-a (capture, `pacat`, the watchdog) before switching.
5. HA restart (not reload) and mDNS discovery from an HA on the TV's L2.
