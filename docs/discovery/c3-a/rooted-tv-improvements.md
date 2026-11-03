# c3-a (LG C3) rooted TV: improvement backlog

- **Status: documented only, not applied.** Each item needs explicit owner approval and a rollback plan before execution. No command was run on the TV for this document, and nothing in the repository was changed beyond this file.
- Date: 2026-11-01. Device: c3-a, LG C3, webOS TV 8.3.0, firmware 03.30.74, rooted via Homebrew Channel.
- Sources: [D0-D1-runtime.md](D0-D1-runtime.md) and [D2-audio-stack.md](D2-audio-stack.md). Raw probe output is not committed and is not reproduced here.
- Priorities: **high** = security, privacy or stability exposure; **medium** = reduces risk or ambiguity before the next experiment; **low** = hygiene.

## Improvements

### 1. Firmware update blocking gaps (high)

- **Observation:** `/etc/hosts` blocks `snu`, `su`, `su-ssl` and `su-dev` `.lge.com` via a tmpfs bind mount that `startup.sh` rebuilds on each boot. The update binary also references `snu-dev.lge.com`, `nsu.lge.com:5100` and a hard-coded address, none of which is blocked. Usage on this build is unknown. See [D0-D1 > Firmware auto-update status](D0-D1-runtime.md#firmware-auto-update-status). The settings daemon could not be read (`com.webos.service.update` getters gave no reply).
- **Why it matters:** the block is the only current barrier to a firmware change. The firmware has not changed in about 18 months, which is circumstantial evidence and not proof.
- **Proposed change:** (a) add the unblocked hostnames via the existing `startup.sh` hosts logic, or (b) block them at DNS on the home network (router or the user's homelab resolver), which keeps the TV untouched. Option (b) is preferred because it needs no TV write. Confirm first whether the TV resolves or reaches those names at all.
- **Risk:** low for DNS blocking. Hosts edits touch boot-time startup logic, so a mistake could affect the boot path.
- **Rollback:** remove the DNS rule, or remove the added hosts lines and reboot. The hosts file is regenerated each boot, so no on-disk state persists.
- **Priority:** high.

### 2. SSH server hardening (high)

- **Observation:** the SSH daemon is dropbear v2022.83 (from the Homebrew Channel bin dir), started as `dropbear -R`. Key-based root login is configured and telnet is disabled. Password-auth status is **unknown**. Clients warn that this dropbear build has no post-quantum key exchange (from the owner brief, not checked on this device). See [D0-D1 > Root method details](D0-D1-runtime.md#root-method-details).
- **Hazard:** `startup.sh` sets a default root password **only when no `authorized_keys` file exists**. Removing the key would therefore re-enable a password path.
- **Proposed change:** verify that password auth is disabled, using a read-only check of the running config where possible. Keep `authorized_keys` present and record its checksum. Plan a dropbear update through the Homebrew Channel, with an inspected package, and check that the update does not reset flags or keys.
- **Risk:** medium. A lockout is possible if the key or the daemon is broken, and an update could change startup behaviour.
- **Rollback:** keep a second working key and a known-good dropbear binary. Recover from the owner's own access path if SSH breaks. Do not rely on telnet, which is disabled.
- **Priority:** high.

### 3. System-wide PulseAudio with anonymous native socket (high before mic work)

- **Observation:** PulseAudio 9.0 runs system-wide with `--system=1`. The native socket `/var/run/pulse/native` is mode 0777 with `auth-anonymous=1`, so any local process can connect. Every sink and source is currently `SUSPENDED`, and the only clients are audiod and the probe. See [D2 > Sound server architecture](D2-audio-stack.md#sound-server-architecture).
- **Why it matters:** once the built-in mic is opened (D4-a in the D2 plan), any local process could capture or play audio. This matters for the privacy model.
- **Proposed change:** research only at this stage. Options include restricting socket access or the module set. Any change to the LG-owned PulseAudio config is a native-service change, which AGENTS.md forbids without approval.
- **Risk:** medium to high. Changing it can break LG audio routing (UI sounds, TTS, the `ptts` role).
- **Rollback:** keep a copy of the original `system.pa` and socket settings. Restore and restart PulseAudio (or reboot).
- **Priority:** high, before any D4 recording.

### 4. Kernel stability hazard: ASoC DAPM sysfs reads (high, probe guard)

- **Observation:** `cat` on `dapm_widget` under `soc-audio` and `lgsic-audio` caused two kernel oopses (NULL pointer in `mutex_lock`). The kernel taint went from P|O to P|O|D. See [D2 > Incident](D2-audio-stack.md#incident-during-this-step-read-first).
- **Proposed change (repository, not the TV):** add a denylist to `tools/probe/probe.sh`. Block `dapm_widget` paths, `/sys/kernel/debug/asoc/*` and debugfs ASoC files. Wrap every probe command in `timeout`. Add a check to the probe test that a denied path cannot be read. Run `sh -n tools/probe/probe.sh` and `python3 tools/check_docs.py` after the change.
- **Risk:** low. This is a tooling change only.
- **Rollback:** revert the commit that adds the denylist.
- **Priority:** high.

### 5. Kernel is in a post-oops state (high, needs approval)

- **Observation:** the kernel has run with taint D since the oops. The TV stayed up. See [D2 > Incident](D2-audio-stack.md#incident-during-this-step-read-first).
- **Proposed change:** the owner may restart the TV at a convenient time. The agent did not reboot it, and reboots need explicit authorization.
- **Risk:** low to medium. A reboot resets the post-oops state but may change persistent boot behaviour.
- **Rollback:** not applicable, since a reboot is not reversible. Confirm the Homebrew boot state (startup script, `webosbrew_sshd_enabled`) beforehand.
- **Priority:** high.

### 6. Observability gaps in luna-send (medium)

- **Observation:** `luna-send` printed nothing unless stdout was a pipe (`| cat`) and `-w <ms>` was given. The applicationmanager, tvpower and update services returned no reply, even for a nonexistent service, so silence does not prove "unsupported". See [D0-D1 > Baseline](D0-D1-runtime.md#firmware-and-software-version).
- **Working pattern (verified on c3-a):** `luna-send -n 1 -w 5000 <uri> '<json>' | cat`.
- **Open question:** why these services do not reply, and whether a differently identified caller gets an answer (D0-D1 open question 2). The screen and foreground-app state is therefore only partly known.
- **Proposed change:** document the working pattern in the probe tooling. Add a read-only getter check that reports "no reply" separately from "unsupported". Do not use write methods (`checkUpdate`, `startUpdate`) to test.
- **Risk:** low.
- **Rollback:** revert the tooling commit.
- **Priority:** medium.

### 7. `devmode_enabled` directory: preserve, do not break (medium)

- **Observation:** `/var/luna/preferences/devmode_enabled` is an empty **directory**, not a file. LG's power-off script runs `rm -f` on it, which does not remove a directory. This likely keeps developer mode, and therefore `/media/developer`, alive across power cycles. This is **inferred from script text and not tested**. See [D0-D1 > Root method details](D0-D1-runtime.md#root-method-details).
- **Proposed change:** none. Preserve the directory. Add a check to any future probe or change that it still exists as a directory, and do not replace it with a file.
- **Risk:** if the directory is removed or replaced, developer mode and the app storage may be lost after a power cycle.
- **Rollback:** not applicable if nothing is changed. If it is lost, re-enabling developer mode is a vendor step and needs its own plan.
- **Priority:** medium (preserve).

### 8. Boot hook design: keep `init.d` empty (medium)

- **Observation:** `/var/lib/webosbrew/init.d` is empty. `run-parts` runs every hook at early boot with the failsafe flag set. A crash during startup triggers failsafe mode and an on-screen alert on the next boot. See [D0-D1 > Root method details](D0-D1-runtime.md#root-method-details) and [Implications](D0-D1-runtime.md#implications-for-the-toolchain).
- **Proposed change:** no hook until a design is reviewed. Prefer an HBC-style service over `init.d` if autostart is needed. The boot trigger chain is still unknown (D0-D1 open question 8).
- **Risk:** medium to high for a crash-looping hook.
- **Rollback:** remove the hook file and reboot, which is itself a reboot needing approval.
- **Priority:** medium.

### 9. Native LG voice services are running while hands-free is off (medium, do not change)

- **Observation:** `voiceinput`, `voiceconductor`, the ThinQ and Alexa adapters, and the wake-on-voice path are running. Hands-free is off on all providers, the Always Ready display status is `none` (the Always Ready setting itself is on, see [always-on](../../design/always-on.md#2-what-was-observed-on-c3-a-read-only-2026-11-03)), and the built-in mic reports `TV`. Behaviour while a session is active or in standby is **unknown**. See [D2 > TV state](D2-audio-stack.md#tv-state-at-snapshot-time) and [D2 > Who holds which sound device](D2-audio-stack.md#who-holds-which-sound-device-snapshot).
- **Proposed change:** none. AGENTS.md forbids changing native LG voice services during discovery. Observe only (D3-a) and only with user action.
- **Risk:** disabling them could break LG UI features and is not reversible without a vendor reinstall.
- **Rollback:** not applicable.
- **Priority:** medium (note only).

### 10. Capture exclusivity and hands-free conflict (medium)

- **Observation:** each capture PCM has one subdevice and there is no `dsnoop`, so the first opener gets the mic exclusively. voiceinput's `libaec-fe` hard-codes `hw:1,0` to `hw:1,4`. Conflict is **inferred, not tested**. See [D2 > Who holds](D2-audio-stack.md#who-holds-which-sound-device-snapshot).
- **Proposed change:** run D3-d (`arecord --dump-hw-params`) only while hands-free is off and no voice session is active, as the D2 plan already states.
- **Risk:** low to medium (brief contention with voiceinput).
- **Rollback:** stop the probe process. No state is written.
- **Priority:** medium.

## Constraints (not changes)

- **Resources:** 3 of 4 cores online (cpu3 offline, cause unknown). About 750 MB `MemAvailable` out of 2 GB, and CMA nearly exhausted (about 3 MB free of 656 MB). Load average about 12 with about 95% CPU idle, probably uninterruptible sleep (inferred). Keep any native daemon small. See [D0-D1 > Uptime, load and memory](D0-D1-runtime.md#uptime-load-and-memory-headroom).
- **Storage and exec:** `/` is read-only squashfs. `/var`, `/home` and `/media` are rw and nosuid, with no noexec. `/media/developer` (about 4 GB free) allows exec. Writes to `/tmp` are lost on reboot. See [D0-D1 > Mount layout](D0-D1-runtime.md#mount-layout).
- **Toolchain:** 32-bit ARM userland, glibc 2.31, `/lib/ld-linux.so.3`, no hard-float loader. Node 12.22, Python 3.8, Perl 5.30 and BusyBox ash are present. No strace, gdb, tcpdump, gcc or jq. See [D0-D1 > Implications](D0-D1-runtime.md#implications-for-the-toolchain).
- **Telemetry spools:** `startup.sh` bind-mounts tmpfs read-only over the telemetry spool paths. Keep this as is.
- **Kernel config:** `/proc/config.gz` is absent, so the kernel options (including whether seccomp is on) are unknown. See [D0-D1 > CPU and kernel](D0-D1-runtime.md#cpu-and-kernel).

## Not yet known (do not assume)

- Whether `snu-dev`, `nsu:5100` or the hard-coded address are reachable and used (item 1).
- The channel count, format and rate of every LGSIC PCM, and whether opening one disturbs voiceinput or the standby wake-on-voice path. See [D2 > Residual unknowns](D2-audio-stack.md#residual-unknowns).
- Whether the boot trigger for the Homebrew `autostart` method is the one assumed (D0-D1 open question 8).
