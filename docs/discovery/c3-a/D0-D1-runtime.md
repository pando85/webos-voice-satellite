# c3-a (LG C3): D0 baseline and D1 runtime/ABI discovery

- Date: 2026-11-01
- Device: c3-a (LG C3), commercial webOS TV, rooted (Homebrew Channel).
- Method: read-only shell inspection over SSH (`/proc`, `/sys` listings, `ls`, `cat` of non-credential files, `strings`, `od`, a few getter-style `luna-send` calls). Nothing was written, started, stopped or installed on the TV.
- TV state during the probe: OS fully up (uptime about 16.5 h, 313 processes, systemd services and LG voice daemons running, CPU about 95% idle). Screen state and foreground app could NOT be read: `luna-send` calls to applicationmanager and tvpower returned no reply. The persisted `last_foreground_app_id` (written about one hour earlier) was the HDMI 1 app. Treat the state as "powered on, probably idle, screen state unknown".
- Raw outputs (not committed): `devices/private/c3-a/D0-D1/`.
- Status labels: verified = observed directly on c3-a in this run; inferred = deduced from verified facts; unknown = not established.

## D0 Baseline

### Firmware and software version

**Finding.** Model `OLED42C34LA`, webOS TV 8.3.0 (codename "Rockhopper", release 8.3.0-1508), LG firmware string `03.30.74`, build date 2024-02-26 02:21 UTC. Image name is `lib32-starfish-global-flash`, region GLOBAL, platform tag `webOSTV 23`. SoC tag `O22A3`, board `O22A3_DVB`. Kernel `5.4.96-307` (PREEMPT SMP, built 2024-02-26).

**Evidence.**
```
$ cat /etc/os-release       -> ID=starfish, NAME="webOS TV", VERSION="8.3.0"
$ cat /etc/starfish-release -> Rockhopper release 8.3.0-1508 (number1-nameri)
$ luna-send -n 1 -w 5000 luna://com.webos.service.tv.systemproperty/getSystemInfo '{"keys":["modelName","firmwareVersion","sdkVersion"]}'
  -> {"firmwareVersion":"03.30.74","modelName":"OLED42C34LA","sdkVersion":"8.3.0"}
$ nyx-cmd OSInfo query webos_imagename webos_build_datetime core_os_kernel_version
  -> lib32-starfish-global-flash / 20240226022140 / 5.4.96-307
```

**Confidence.** verified.

**Tooling gotcha.** Over a non-interactive SSH session `luna-send` printed nothing unless its stdout was a pipe (append `| cat`) and `-w <ms>` was given. Several services (applicationmanager, tvpower, `com.webos.service.update`) returned no output at all, even for a non-existent service, so silence is not evidence of "unsupported".

### Uptime, load and memory headroom

**Finding.** Uptime about 16.5 h. Load average about 12.4 while the CPU was about 95% idle and `some` CPU pressure (PSI) averaged about 3%. The high load is most likely tasks in uninterruptible sleep rather than CPU use (inferred, not confirmed). RAM 2.0 GB total, only about 55 MB "free" but about 750 MB `MemAvailable`. Swap is a 600 MB zram device, about 9 MB used. CMA region 656 MB total with about 3 MB free, so contiguous-memory allocations (video/graphics) are tight. Largest resident processes: a preloaded YouTube app (`cobalt`, about 108 MB), `surface-manager` (66 MB), several WebAppMgr/home/inputcommon processes (20-60 MB each).

**Evidence.**
```
MemTotal 2053576 kB / MemAvailable 751060 kB / SwapTotal 614396 kB (zram0) / CmaTotal 671744 kB, CmaFree 3296 kB
top: 2.4% usr 2.4% sys 95.1% idle; load average 12.61 12.44 12.38
/proc/pressure/memory: some avg300=0.00; cpu: some avg300=2.69
```

**Confidence.** verified for the numbers (one sample), inferred for the cause of the load.

### Root method details

**Finding.**
- Homebrew Channel app `org.webosbrew.hbchannel` version **0.6.3**, installed 2023-03 and set up on the TV 2024-05. Companion apps: `org.webosbrew.inputhook` 1.4.0 (LG Input Hook). Other homebrew apps are installed in `/media/developer/apps/usr/palm/applications` (moonlight, retroarch, jellyfin, others).
- The boot hook is the Homebrew Channel service. The app's `previewMetadata.targetEndpoint` is `luna://org.webosbrew.hbchannel.service/autostart`. The service's `autostart` method copies `startup.sh` to `/var/lib/webosbrew/startup.sh` if needed and runs it (inferred from `service.js` string search and `jumpstart.sh`; the actual boot trigger that invokes `autostart` was not traced).
- `/var/lib/webosbrew/startup.sh` (5.4 KB, `#!/bin/bash`, which is BusyBox ash here) ran this boot (`/tmp/webosbrew_startup` exists; no `webosbrew_failsafe` flag present). What it does: block-update hosts entries (see below); optional root telnet (disabled here via `webosbrew_telnet_disabled`); start dropbear if `webosbrew_sshd_enabled` exists (it does); bind-mount tmpfs over telemetry spool paths (`/tmp/rdxd`, `/tmp/uploadd`, `/var/spool/rdxd`, `/var/spool/uploadd/*`) as read-only; set a default root password only if no `authorized_keys` exists (a key is present, so not applied); elevate the HBC service; `run-parts /var/lib/webosbrew/init.d`.
- **`/var/lib/webosbrew/init.d` is empty**: no user startup hooks. Anything we add there would be run at early boot by `run-parts` (all hooks run with the failsafe flag set; a crash during startup triggers failsafe mode and an on-screen alert on the next boot).
- SSH daemon: **dropbear v2022.83**, binary shipped inside the HBC service dir (`.../org.webosbrew.hbchannel.service/bin/dropbear`), started by `startup.sh` as `dropbear -R` (forks per session). Host key stored in `/var/lib/webosbrew/sshd`. Key-based root login is configured. Telnet is disabled. The HBC `bin/` dir also ships `rsync`, `scp`, `sftp-server`, `remount-nosuids.sh`, `sampatcher.py`; **it does not ship busybox**.
- `/var/luna/preferences/devmode_enabled` is a **directory** (empty), not a file. LG's `shutdown.sh` on power-off runs `rm -f .../devmode_enabled` when `start-devmode.sh` is absent from `/media/cryptofs/apps/.../com.palmdts.devmode.service/`, and that file is absent on this TV. `rm -f` does not remove a directory, so this likely keeps developer mode (and thus `/media/developer`, which `devmode.sh` deletes at boot if devmode is disabled) alive across power cycles. This is inferred from script text, not tested.

**Evidence.**
```
$ cat /media/developer/.../org.webosbrew.hbchannel/appinfo.json -> "version": "0.6.3"
$ ls -la /var/lib/webosbrew -> init.d/ (empty), sshd/, startup.sh
$ ls /var/luna/preferences -> webosbrew_block_updates, webosbrew_sshd_enabled, webosbrew_telnet_disabled (1 byte each); no webosbrew_failsafe
$ dropbear -V -> Dropbear v2022.83 ; /proc: dropbear master process runs from the HBC service dir
$ ls -ld /var/luna/preferences/devmode_enabled -> drwxr-xr-x ... (directory)
```

**Confidence.** verified for versions, files and running daemon. inferred for the boot trigger chain and for the devmode directory behaviour.

### Firmware auto-update status

**Finding.** Homebrew-side blocking is active and the firmware has not changed in about 18 months of rooted operation, but the setting is not a complete proof of "no update will ever happen".
- `webosbrew_block_updates` flag exists, so `startup.sh` bind-mounts a tmpfs `/etc/hosts` that maps `snu.lge.com su-dev.lge.com su.lge.com su-ssl.lge.com` to `127.0.0.1` and `::1`. The mount is live (verified in `/proc/mounts` and `/etc/hosts`). It is regenerated each boot, not persistent on disk.
- LG's `/usr/sbin/update` binary also references hostnames that are **not** blocked: `snu-dev.lge.com`, `nsu.lge.com:5100`, and a hard-coded public IPv4 for `CheckSWAutoUpdate.laf`. Whether any of these is used on a production build is unknown.
- `/mnt/lg/cmn_data/update/.disagreeAutoUpdate` exists (recreated at this boot) and `/var/luna/preferences/eula` has every legal-terms entry `"accepted": false`. The update daemon logs "User doesn't agree general terms - do not start NSU auto update". This suggests NSU auto-update is also internally disabled (inferred from binary strings plus these two facts; not a documented guarantee).
- Option `appUpdateMode` is `manual` (that is for apps, not firmware).
- Circumstantial: firmware build date is 2024-02-26 and the TV has been rooted since 2024-05, with no firmware change since.
- No read-only luna getter for the auto-update setting worked: `com.webos.service.update` methods (`getCurrentSWInformation`, `getStatus`, `getVersionChecked`, `getSchedule`) returned no reply. Methods such as `checkUpdate`/`startUpdate` exist and were NOT called.

**Evidence.**
```
$ grep -v '^#' /etc/hosts -> 127.0.0.1 snu.lge.com su-dev.lge.com su.lge.com su-ssl.lge.com (and ::1 equivalent)
$ grep tmpfs /proc/mounts -> tmpfs /etc/hosts tmpfs rw,relatime
$ strings /usr/sbin/update | grep -E 'snu|nsu|su\.lge' -> includes snu-dev.lge.com, nsu.lge.com:5100 (not in hosts)
```

**Confidence.** verified for the hosts block and flags. inferred for "auto-update is also internally off". unknown for whether blocked hostnames are the only ones a production update would use, and for any server-pushed or USB update path.

### Mount layout

**Finding.** Root and system trees are read-only; `/var`, `/home`, `/media` and `/tmp` are writable. No writable mount has `noexec`; `/var`, `/home` and `/media` are `nosuid`.

| Path | Backing | Mode | Size / free |
|---|---|---|---|
| `/` | squashfs (`/dev/root`) | ro | 1.4 GB, 100% used |
| `/etc`, `/lib`, `/usr`, `/mnt` | overlay of squashfs layers | ro | n/a |
| `/var` and `/home` | ext4 on eMMC partition 56 (`/var` was first an ro overlay, then an rw ext4 mount on top) | rw, nosuid, no noexec | 968 MB, 92 MB used, about 810 MB free |
| `/media` (includes `/media/developer`) | ext4 on eMMC partition 61 | rw, nosuid, no noexec | 5.8 GB, 1.7 GB used, about 4.0 GB free |
| `/tmp`, `/run`, `/dev/shm` | tmpfs | rw | about 1 GB each, nearly empty |

`/media/developer` is mode 777 root-owned. Executables run directly from it (dropbear does), which verifies exec is permitted there. About 1,470 mount lines in total (many per-app jail bind mounts).

**Evidence.**
```
$ grep -E ' (/|/var|/media|/tmp) ' /proc/mounts
/dev/root / squashfs ro,relatime
/dev/mmcblk0pNN /var ext4 rw,nosuid,relatime,data=ordered
/dev/mmcblk0pNN /media ext4 rw,nosuid,relatime,data=ordered
tmpfs /tmp tmpfs rw,relatime
```

**Confidence.** verified.

## D1 Runtime and ABI

### CPU and kernel

**Finding.** 4 cores possible, 3 online (cpus 0-2). Implementer 0x41 (Arm), part 0xd0b (Cortex-A76), variant 0x4. aarch64 kernel 5.4.96-307, PREEMPT. `/proc/config.gz` does not exist and no `/boot/config*`, so kernel options could not be grepped. Indirect evidence: the ALSA stack is present (`/proc/asound`, two cards, `/dev/snd` nodes including several capture PCMs), cgroup v1 controllers present (cpuset, cpu, cpuacct, blkio, memory, devices, freezer, perf_event; no `pids`), cgroup2 mounted, FUSE/overlay/ext4/exfat/vfat/ntfs in `/proc/filesystems`, `/lib/modules/5.4.96-307` exists. `/proc/self/status` has no `Seccomp:` line, which suggests CONFIG_SECCOMP is not enabled (inferred; systemd also reports `-SECCOMP`).

**Evidence.**
```
$ grep -E '^(CPU implementer|CPU part)' /proc/cpuinfo | sort | uniq -c -> 3x 0x41, 3x 0xd0b
$ cat /sys/devices/system/cpu/online -> 0-2   (possible 0-3)
$ ls /proc/config.gz -> No such file
$ cat /proc/asound/cards -> two cards present (names omitted here)
```

**Confidence.** verified for CPU and listed facts. inferred for seccomp. unknown for the full kernel config.

### 32-bit vs 64-bit userland

**Finding.** The userland is **32-bit ARM (ELF32, EM_ARM 0x28, EABI5), soft-float calling convention (`e_flags` 0x05000200, i.e. softfp-style, no hard-float flag), dynamic linker `/lib/ld-linux.so.3`** despite the aarch64 kernel. Every sampled binary matched: busybox, luna-send, systemd, update, pulseaudio, node, nyx-cmd, xinetd, ls-hubd, and the homebrew dropbear. Only the 32-bit loader exists: `/lib/ld-linux.so.3 -> ld-2.31.so`; `/lib/ld-linux-armhf.so.3`, `/lib/ld-linux-aarch64.so.1`, `/lib64` and `/usr/lib64` do **not** exist. So only one ABI is present. curl's build triple confirms: `arm-starfishmllib32-linux-gnueabi`. (The 64-bit kernel runs these via compat mode; a native aarch64 binary would need its own loader and libraries, which are not provided.)

**Evidence.**
```
$ od -An -tx1 -N20 /usr/bin/luna-send -> 7f 45 4c 46 01 01 01 00 ... 03 00 28 00   (class 01 = 32-bit, e_machine 0x28)
$ od -An -tx4 -j36 -N4 /usr/bin/luna-send -> 05000200      (EABI5, soft-float flag)
$ strings /usr/bin/luna-send | grep '^/lib/ld' -> /lib/ld-linux.so.3
$ ls /lib/ld-linux-armhf.so.3 /lib/ld-linux-aarch64.so.1 /lib64 -> No such file
$ curl --version -> curl 7.69.1 (arm-starfishmllib32-linux-gnueabi)
```

**Confidence.** verified for ELF class, machine, interpreter and absent loaders. inferred for "softfp" naming (from `e_flags` plus loader name; exact float ABI/FPU attributes were not read with readelf).

### libc and C++ runtime

**Finding.** glibc **2.31** (running `/lib/libc.so.6` prints "GNU C Library stable release version 2.31"), `libstdc++.so.6.0.28` (GLIBCXX up to 3.4.28, which is roughly the GCC 9 series), `libgcc_s.so.1` present in `/lib`, OpenSSL 1.1.1t, `libasound.so.2`, `libpulse.so.0` (PulseAudio 9.0). **No musl.**

**Evidence.**
```
$ /lib/libc.so.6 -> GNU C Library (GNU libc) stable release version 2.31.
$ strings /usr/lib/libstdc++.so.6 | grep GLIBCXX | tail -1 -> GLIBCXX_3.4.28
$ ls /lib/ld-musl* -> No such file
```

**Confidence.** verified.

### Init system

**Finding.** **systemd 244** (PID 1 `/lib/systemd/systemd`, `/sbin/init` links to it). A legacy `initctl` shim and about 62 upstart-style `/etc/init/*.conf` files also exist (LG still ships those; `devmode.conf` is one of them), but `initctl` is not the init. Many service units live under `/etc/systemd/system`.

**Evidence.**
```
$ cat /proc/1/comm -> systemd
$ systemctl --version -> systemd 244 (244.5+) ... -SECCOMP ... default-hierarchy=hybrid
```

**Confidence.** verified.

### Interpreters and tools

**Finding.** Contrary to the premise in the task, the following ARE present and on the SSH session PATH (`/home/root/.local/bin:<HBC bin>:/usr/sbin:/usr/bin:/sbin:/bin`):
- `arecord`/`aplay` 1.2.1 (`arecord` is a symlink to `aplay`), `amixer`, `pactl`, `pacmd`, `pulseaudio` 9.0 (running as a system-wide daemon), `gst-launch-1.0` 1.18.5.
- `node` v12.22.2 (ARM 32-bit, V8 7.8), `python3` 3.8.13, `perl` 5.30.1, `curl` 7.69.1, `wget`, `openssl` 1.1.1t, `opkg` 0.4.2, `flock`, `telnetd`, `nc`.
- **Not present**: `lua`, `ruby`, `php`, `strace`, `gdb`, `gcc`, `tcpdump`, `sox`, `ffmpeg`, `jq`, `git`, `ssh` client, `xz`, `socat`.
- `/bin/sh` and `/bin/bash` are both symlinks to `/bin/busybox.nosuid` (BusyBox v1.31.1, ash; there is no real bash). `/bin/busybox` links to it. The applet list includes `nc`, `telnetd`, `timeout`, `top`, `ps`, `pidof`, `lsof`, `setpriv`, `run-parts`, `devmem`, `i2c*`, `udhcpc` and `xxd`, but not `strace`.
- Python module availability was not checked.

**Confidence.** verified (each version was printed by running it with `--version`; these do not touch audio devices).

### Process count and top consumers

**Finding.** 313 processes. Top RSS: YouTube preload (`cobalt`) about 108 MB, `surface-manager` about 66 MB, `com.webos.app.inputcommon` about 61 MB, `LunaExecutable` (AirPlay app) about 42 MB, `WebAppMgr` about 34 MB, `MaliitServer` about 33 MB, `legacy-broadcast-dvb` about 32 MB (128 threads). No single process dominates CPU (all about 1-2% in one `top` sample). LG voice-related daemons are running: `voiceconductor`, `voiceinput`, `lg.thinqai.adapter`, `amazon-alexa-adapter`, plus a `com.webos.service.voice.performer` jail mount exists. PulseAudio runs in system mode (`--system=1`).

**Confidence.** verified (single sample).

## Open questions

1. What is the TV's real power and screen state during the probe, and which app is in the foreground? (Needs a working luna getter or an observation from the user.)
2. Do `com.webos.service.update` calls work from a differently-identified caller, so the auto-update setting and last check result can be read? Silent no-reply behaviour needs explaining.
3. Are `snu-dev.lge.com`, `nsu.lge.com:5100` or the hard-coded address reachable and used in production? Should they be added to the block list, and does the user want that (a write)?
4. Is the high load average (about 12) caused by uninterruptible tasks, and does it persist when the TV is in standby?
5. What is the exact float ABI/FPU/NEON attribute set of the system libraries (needs `readelf -A` equivalent run off-device on a copied binary, which was not done)?
6. What is the kernel config (no `/proc/config.gz`)? Is `CONFIG_SECCOMP` really off, and is `cpu3` permanently offline?
7. Does boot persistence survive a full power-off and a cold boot from standby? Not tested (reboots and power cycles are out of scope).
8. What boot component calls the HBC `autostart` method?

## Implications for the toolchain

- Build **32-bit ARM, glibc 2.31, softfp** (triple like `arm-linux-gnueabi` / webOS's `arm-webos-linux-gnueabi`, ARMv7-A; NEON/VFP availability to the 32-bit userland was not checked with `readelf -A`; dynamic linker `/lib/ld-linux.so.3`). A plain `arm-linux-gnueabihf` toolchain will not run: the hard-float loader `/lib/ld-linux-armhf.so.3` is missing. An aarch64 build will not run either unless it is fully static; the aarch64 kernel executes only the 32-bit userland that ships. A fully static aarch64 binary might run on the 64-bit kernel but this is untested.
- Alternatively, ship static binaries (musl or glibc static, 32-bit ARM) to avoid dependence on system libs. Keep libstdc++ use within GLIBCXX 3.4.28 if dynamically linking.
- Rust: target `armv7-unknown-linux-gnueabi` (softfp) with glibc 2.31 sysroot, or a static musl target; avoid `gnueabihf`.
- Available on-device runtimes for prototyping: Node 12.22 (old, no recent syntax), Python 3.8, Perl 5.30, BusyBox ash. ALSA tools and `libasound`/`libpulse` exist for capture experiments; PulseAudio 9.0 runs system-wide.
- No `strace`/`gdb`/`tcpdump` on the TV: debugging has to be done with logs or off-device tooling, or with a separately provided static strace (must be copied and run only with explicit authorization).
- Writable and exec-allowed storage for experiments: `/media/developer` (about 4 GB free) and `/tmp` (tmpfs, RAM-backed, cleared on reboot). `/var` has about 810 MB. Memory headroom is limited (about 750 MB available, CMA nearly exhausted): keep a native daemon small.
- Any autostart design must use `/var/lib/webosbrew/init.d` (run-parts, early boot, failsafe risk) or an HBC-style service; there is currently nothing in `init.d`. Anything added must be reversible and authorized.
