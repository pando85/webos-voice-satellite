# EXP-012 — Phone setup web server: on-target probe (Part A, read-only plus short temporary listeners)

**Status:** Part A TARGET_VERIFIED on c3-a (items P1 to P4, P6, P7, P9; P5 partly, by static evidence only); P8, P10 and Part B NOT RUN
**Date:** 2026-11-05 (about 07:45 to 08:10 CET)
**Device alias / model / firmware / webOS / architecture:** c3-a, LG C3 42 inch, firmware `03.30.74`, webOS TV 8.3.0 (Rockhopper release 8.3.0-1508), Homebrew Channel 0.6.3, kernel 5.4.96 aarch64, 32-bit userland, satellite 0.3.0 running
**Power and playback state:** `Active`, the owner's PC display, no playback, no app opened or closed by the experiment; the TV was on Wi-Fi (Ethernet port without carrier)
**Related research / requirements / issue:** [Phone setup design](../design/phone-setup.md) (section 15 is the checklist this file implements), [ADR 0009](../decisions/0009-phone-setup-web-server.md), [backlog #15](../project/backlog.md#15-set-up-with-your-phone-phone-setup-web-server)
**Risk:** read-only, plus listeners on the TV that lived for under 1 minute (started by hand, killed by hand, with `timeout` as a backstop) and a few `exec` calls to the Homebrew Channel service
**Approval needed / obtained:** owner authorization for Part A on c3-a in the coordinator's brief (2026-11-05). Part B (the owner's phone, the real remote) needs the owner present and is not covered.

Raw output is in the gitignored `devices/private/c3-a/EXP-012/` and is not committed. Addresses below are masked (`192.0.2.x`, "LAN /24", `2001:db8::/64`); MAC addresses were never recorded.

## Question and falsifiable hypothesis

Can the phone setup server of ADR 0009 work on this TV, as designed? Seven bounded questions, each with the hypothesis that is being tested:

1. **Tools (P1):** which of `iptables`, `ip6tables`, `nft`, `ip`, `ss`, `netstat`, `nc`, `httpd`, `python3` exist (decides probe method and fallbacks).
2. **Firewall (P2):** the TV filters inbound TCP on random high ports. Falsified if the INPUT policy is ACCEPT with no rules.
3. **Reachability (P3, P4):** a listener on `0.0.0.0` on a random port in 49152-65535 (and in 32768-49151, and one low port) is reachable from a host on the same LAN; also over IPv6. Hypothesis: yes for IPv4 (HA reaches 6053), no knowledge for the others.
4. **Web engine (P5):** the TV app runs on Chromium 94 (LG's documented figure for webOS TV 8, recalled), so ES2015 and later are available.
5. **Exec survival (P6):** a detached `setsid` process started by the Homebrew Channel `exec` survives the call's return and the service's idle exit (the daemon already does).
6. **Exec polling cost (P7):** polling `ctl.sh ... --json` through `exec` once a second is cheap enough to be the session heartbeat.
7. **Log leakage (P9):** the exec `command` string or stdout is not retained in readable logs.

## Prerequisites and safe baseline

- SSH as root to the TV (address in the gitignored local config). `luna-send` to the HBC `exec` method needs a pty on this firmware: copy the script to `/tmp/exp012/` and run it with `ssh -tt` (a script piped into `sh -s` over a pty never gets EOF).
- The PC and the TV on the same LAN, to test the listener. Satellite 0.3.0 running, HA connected (the satellite is the owner's permanent entry and must stay up).
- Hard rules: no sound, no reboot, no power, screen, input, app or volume change, **no `iptables` change**, no `apiadapter/system/getSystemInfo`, no read of ASoC `dapm_widget` sysfs or `/sys/kernel/debug`, do not open the app on screen.
- Baseline recorded before: satellite pid, RSS and CPU ticks, `iptables -S`, listening sockets.
- Everything the experiment puts on the TV is under `/tmp/exp012/` (and one `/tmp/exp012-listen.py`), removed at the end.

## Commands / steps (copyable and bounded)

All commands run on the TV through `ssh root@<tv>`; scripts that call `luna-send` go through `ssh -tt`.

**P1 tools:** `command -v iptables ip6tables nft ip ifconfig ss netstat nc ncat httpd python3 perl curl wget timeout setsid tcpdump luna-send`; `busybox` applet list; `python3 -V` and an import loop over `socket http.server socketserver select selectors json ssl pty os threading asyncio subprocess resource`; `ls -l /proc/net/route /proc/net/fib_trie`.

**P2 firewall (read only):** `iptables -S`, `iptables -t nat -S`, `iptables -t mangle -S`, `ip6tables -S`, `ip6tables -t nat -S`, `lsmod | grep -E 'ip_tables|nf_|xt_'`. Never `-A`, `-I`, `-D`, `-F`, `-P`.

**P4 addresses:** `ip -o addr`, `ip -o link` (MACs masked before recording), `ip route`, `ip -6 route`, `/proc/net/route`, `netstat -tln`, `netstat -uln`, `sysctl net.ipv4.ip_local_port_range`.

**P3 reachability:** a throw-away Python 3 listener (`socket` and `select` only, no `http.server`) that binds the given IPv4 ports on `0.0.0.0` and the given IPv6 ports on `::` (V6ONLY), answers `EXP012 ok` and logs the peer address, with a 90 s time limit and `timeout 100` around it:

```sh
ssh root@<tv> 'timeout 100 python3 /tmp/exp012-listen.py 51734,58219,62907,38211,8765 51734,58219 90'
```

Then from the PC, in a second shell: connect to each port over IPv4 (`socket.create_connection`, 4 s timeout), to two of them over the TV's global IPv6 address, to 6053 (control, known good) and to a closed port (control, expect `Connection refused`). Afterwards: kill the listener, confirm no socket on those ports with `netstat -tln`, `rm` the script. (Pitfall met in the run: `pkill -f <name>` run through `ssh` also matches the remote shell whose command line holds the name and kills it; look the pid up with `pgrep -x python3` and the full command line, or rely on the `timeout`.)

**P5 web engine (no app opened):** `cat /etc/starfish-release /etc/os-release`; `ls /usr/lib/webappmanager /usr/lib/cbe`; `strings -n 6 /usr/lib/libcbe.so | grep -E 'Chrome/[0-9]+|[0-9]+\.0\.[0-9]{4}\.[0-9]+'`, then the lines around each hit in the full `strings` output; the `WebAppMgr` command line flags (`user-agent-suffix`, `webos-js-flags`). `navigator.userAgent` and feature checks need the screen and are **not** part of this run.

**P6 survival:** `t6.sh` (copied to `/tmp/exp012/`, run with `ssh -tt`): list the HBC service processes, then

```sh
luna-send -n 1 -f luna://org.webosbrew.hbchannel.service/exec '{"command":"setsid sleep 299 </dev/null >/dev/null 2>&1 & echo started"}'
```

then every 10 s for 120 s count `sleep 299` processes and HBC service processes (cmdline containing `hbchannel.service`). Kill the `sleep` by pid afterwards (check its `/proc/<pid>/cmdline` first).

**P7 polling cost:** `t7.sh LABEL INTERVAL_S DURATION_S KIND` (copied to `/tmp/exp012/`, run with `ssh -tt`). For KIND `true`, `help` (`sh ctl.sh --help >/dev/null`, a proxy for a light `setup status`: the script start with `lib.sh` and no luna call) or `status` (`sh ctl.sh status --json 2>&1`, the app's own command line shape), it calls `luna-send -n 1 -f luna://org.webosbrew.hbchannel.service/exec '{"command":"..."}'` on a fixed schedule, timing each call from `/proc/uptime` (10 ms resolution), checking `"returnValue": true`, and reads CPU ticks (`/proc/<pid>/stat` fields utime+stime and cutime+cstime, 10 ms) before and after for: the satellite, `ls-hubd`, the HBC service node process (self and children), the loop's own shell (children: `timeout`, `luna-send`, `sleep`), and the system-wide busy ticks of `/proc/stat`. Runs: baseline without calls (45 s, before and after), `true` and `help` and `status` at 1 s and at 2 s.

**P9 leakage:** a random marker `EXP012LEAK<n>Z` is built at run time (so that it never appears in a file written for the experiment), sent as `{"command":"echo <marker>-OUT; true"}` through `exec` (the marker is in the command string and in stdout; a second call used it as a non-existent command name), 3 s later searched for in: every file under `/tmp/var/log` (which is `/var/log`), `/run/log`, `/tmp/pmlogdaemon`, `/var/luna`, `/media/developer/log`, with `grep -a` and `zcat` for the `.gz` and `.tgz` files, plus `journalctl -a`, `logread` and `dmesg`.

**P8 (placeholder listener resources), P10 (listener bound to the Wi-Fi address only, access from outside the private ranges):** not run (the server was not built when this was written; P4's build has since landed, so they belong to Part B).

## Expected evidence and failure interpretation

| Item | Expected if the design holds | If not |
| --- | --- | --- |
| P2 | INPUT policy ACCEPT, or an explicit allow for the high range | A DROP policy or per-port rules: a fixed documented port, a user-run firewall step, or unsupported on that TV (design 5.2) |
| P3 | every IPv4 port connects, the closed control is refused | A timeout on the high ports with 6053 working: a firewall or a Wi-Fi isolation rule; then the fallbacks above |
| P5 | Chromium 87 or later; ES2015 syntax and `fetch` available | An older engine: keep the app ES5 and drop QR drawing in the app (draw it server-side or as text) |
| P6 | the `sleep` is alive after the HBC service exited, with `ppid` 1 | The process dies with the call or the service: the server would need another start path |
| P7 | one call well under 1 s and a small CPU cost | A call near or above the interval: the poll cannot be 1 s and the command must be lighter |
| P9 | the marker is found nowhere | A hit: the token must not pass through `exec` stdout or arguments, or must be consumed before it is logged |

A result that is not conclusive is recorded as such.

## Observations and redacted evidence links

### P1 Tools (c3-a, 2026-11-05, `verified-on-target`)

- Present: `iptables` and `ip6tables` (v1.8.4 legacy), `ip` (busybox applet), `ifconfig`, `netstat` (busybox), `nc` (busybox, `/usr/bin/nc`), `python3` 3.8.13, `perl`, `curl`, `wget`, `timeout`, `setsid`, `nohup`, `top`, `ps`, `luna-send`; busybox 1.31.1 has `tcpsvd`, `udpsvd`, `nc`, `telnetd`, `ftpd`, `ipneigh`, `lsof`, `nmeter`.
- **Absent:** `ss`, `nft`, `ncat`, `tcpdump`, and **`httpd`** (no busybox `httpd` applet).
- **python3 modules:** `socket`, `select`, `selectors`, `json`, `ssl`, `os`, `threading`, `asyncio`, `subprocess` import; **`http.server` fails (no module `html`), `socketserver`, `pty` and `resource` are missing.** So neither Python's `http.server` nor `socketserver` can be a fallback web server on this TV (this matches the ADR's rejection of both, now for an additional reason), and the earlier note that the `pty` module is absent is confirmed.
- `/proc/net/route` and `/proc/net/fib_trie` are readable (the files report size 0 as procfs files do); `fib_trie` content was not read.

### P2 Firewall (`verified-on-target`, read only)

- `iptables -S`, `-t nat -S` and `-t mangle -S`: **all chains policy ACCEPT, no rules at all** (INPUT, FORWARD, OUTPUT; PREROUTING, POSTROUTING). The `ip_tables` family works (the tables exist), there is just nothing in them.
- `ip6tables`: **the kernel has no ip6 tables** (`Module ip6_tables not found`, `Table does not exist`). IPv6 is not filtered by `ip6tables` either: there is nothing to filter with (no `nft` on the TV). Nothing was changed.
- `lsmod` printed nothing for netfilter modules (the interface is built in or not listed); not interpreted.
- Result for the design: **no host firewall on c3-a**. A firewall in another router or access point (client isolation, guest VLAN) is outside what this item can see.

### P4 Interfaces, addresses, route (`verified-on-target`, masked)

- `wlan0`: UP, one IPv4 address `192.0.2.x/24` (the LAN /24), plus a global IPv6 `/64` with a stable address and a privacy-extension address, and a link-local address. **This is the only interface with an IPv4 address and the default route** (`default via <LAN gateway> dev wlan0`; the routing table also has one host route to a public address through the gateway, left unexplained).
- `eth0`: present, `NO-CARRIER`, no address. `p2p0` (Wi-Fi Direct): link-local IPv6 only, no IPv4, `NO-CARRIER`. `sit0`: down. `lo`.
- `net.ipv4.ip_local_port_range = 32768 60999`; `net.ipv6.conf.all.disable_ipv6 = 0`.
- Sockets already listening on `0.0.0.0` (TCP): 22, 1316, 1411, 1532, 1542, 1582, 2002, 3000, 3001, 6053 (the satellite), 9998, 18181, 36866; on the Wi-Fi address only: 7250; on loopback: several. So **LG services already listen on `0.0.0.0`** on this TV, and nothing else is unusual about one more listener on a high port.
- `p2p0` and a second address were not present: no multi-homing to resolve on this unit in this state. c3-b and a TV on Ethernet were not probed.

### P3 Inbound reachability (`verified-on-target` for the PC on the same /24; phone and other VLANs NOT TESTED)

| From | To | Result |
| --- | --- | --- |
| the PC (wired Ethernet, the same LAN /24, the TV on Wi-Fi) | TCP 51734, 58219, 62907 (random, 49152-65535), IPv4 `0.0.0.0` listener | connected, banner received, peer address logged correctly |
| same | TCP 38211 (32768-49151) and 8765 (low range) | connected |
| same | TCP 51734 and 58219 over the TV's **global IPv6** address (a `::` V6ONLY listener) | connected |
| same | 6053 (the satellite, control) | connected |
| same | 40001 (nothing listening, control) | `Connection refused` (so the path is not silently dropping) |

- The PC and the TV are on the **same /24 subnet**; the PC is wired and the TV is on Wi-Fi, so this crosses the access point's bridge and shows that Wi-Fi client isolation is **off for the PC-to-TV path** on this network.
- Listeners: five IPv4 sockets and two IPv6 sockets, alive about 40 s in total, then stopped; afterwards no socket on those ports, the script removed. The IPv4 listener bound `0.0.0.0` (not an interface address); a listener bound to the Wi-Fi address only (P10) was not run.
- **NOT TESTED:** a phone on the same Wi-Fi (Part B), a host on another VLAN or the guest network, a connection from outside the private ranges, power states other than `Active`. The design's hint text for those cases stays a design claim.

### P5 Web engine (`hypothesis`, from static evidence; nothing was run in the web view)

- `/etc/os-release`: `webOS TV 8.3.0`; `/etc/starfish-release`: `Rockhopper release 8.3.0-1508`. The web runtime is `libcbe.so` (118 MB) launched by the small `WebAppMgr` loader with `--user-agent-suffix=SmartTV`.
- `strings` on `libcbe.so` shows **two Chromium version strings:** `94.0.4606.128` next to the app-shell code (`extensions/shell/neva/platform_register_app.cc`, `AppShell_Linux`) and `Chrome/87.0.3945.79` next to `crow/content/crow_content_main_delegate.cc` (LG's `crow` browser content client; the same file also holds user agent strings that say `Chrome/68.0.3440.106` and `SmartTV/9.0`, a spoof table for web sites). My reading: **the app runtime is Chromium 94** (matches LG's documented figure for webOS TV 8), and 87 belongs to the Crow browser build in the same library. This is an inference from where the strings sit, not a read of `navigator.userAgent`.
- ES level: every reading (87, 94) is far above what ES2015 (`const`, `let`, arrow functions, `Promise`, classes) and `fetch`, `async` need, so **ES2015+ is expected (`hypothesis`)**. Not verified: nothing was executed in the web view; `navigator.userAgent`, `KeyboardEvent.keyCode` for OK (13) and Back (461), `document.visibilityState` and timers while hidden, and CSS flex gap are **not tested** (they need the screen: Part B or a scratch diagnostic).

### P6 Exec: a second long-lived process (`verified-on-target`)

- `exec` with `setsid sleep 299 </dev/null >/dev/null 2>&1 & echo started` returned `returnValue true`, `stdoutString "started\n"` at once.
- The `sleep` had `ppid 1` and lived in the cgroup `system.slice/ls-hubd.service`.
- **The HBC service (node process `org.webosbrew.hbchannel.service`) exited by itself between 20 s and 30 s after the call (idle exit).** The `sleep` was still alive at every 10 s check up to 120 s (12 of 12). Killed by pid afterwards; none left.
- Not tested: the app being closed (not opened), an HBC service restart, `Screen Off` and `Active Standby`. The cgroup says the process belongs to `ls-hubd.service`, so a restart of that unit is a plausible way to lose it (`hypothesis`; the server must end itself anyway when the app stops polling).

### P7 Exec polling cost (`verified-on-target`, `Active`, 2026-11-05)

Per-call round trip, measured on the TV from the `luna-send` start to its return (one call per interval, no failures, `returnValue true` every time):

| Command through `exec` | Interval | Calls | min / median / p90 / max (ms) |
| --- | --- | --- | --- |
| `true` (floor of the channel) | 1 s | 45 | 30 / 40 / 40 / 60 |
| `sh ctl.sh --help` (ctl.sh start only, proxy for a light `setup status`) | 1 s | 45 | 70 / 80 / 90 / 90 |
| `sh ctl.sh --help` | 2 s | 30 | 70 / 80 / 90 / 90 |
| `sh ctl.sh status --json` (today's command) | 1 s | 58 | 860 / 950 / 990 / 1040 |
| `sh ctl.sh status --json` | 2 s | 30 | 860 / 940 / 980 / 990 |

The first measured call after the service had been idle is not in these numbers (the HBC service was warmed first); a cold service took about 3.5 s earlier (P4 0.3.0 record). The service stayed alive (same pid) throughout every 1 s and 2 s run.

CPU of the whole chain, 10 ms ticks over a 45 s run, one core = 100 percent (the TV's own load average is about 12 and the system-wide busy ticks are noisy; the per-process figures are the reliable ones):

| Run | HBC node self + children | `ls-hubd` | loop shell children (`luna-send`, `timeout`, `sleep`) | Sum | Per call |
| --- | --- | --- | --- | --- | --- |
| `help`, 1 s (45 calls) | 57 + 131 | 73 | 85 | 346 ticks = **7.7 % of one core** | about 77 ms |
| `help`, 2 s (23 calls) | 26 + 67 | 41 | 43 | 177 ticks in 46 s = **3.8 % of one core** | about 77 ms |
| `true`, 1 s (45 calls) | 47 + 33 | 72 | 83 | 235 ticks = 5.2 % | about 52 ms |
| `status --json`, 2 s (23 calls) | 72 + 1152 | 38 | 43 | 1305 ticks in 46 s = **28 % of one core** | about 570 ms |

- The satellite's own CPU was 1.6 to 2.0 percent of a core in every run, baseline included (73 and 77 ticks in 45 s without any call; 76 to 89 with calls): not distinguishable from noise. System-wide busy ticks: baseline about 21 per second (before and after, 956 and 969 per 45 s), `true`/`help` at 1 s about 27 per second, `status --json` at 1 s about 90 per second (every call lasts nearly the whole second, so it ran back to back).
- **`ctl.sh status --json` is far too heavy for a heartbeat:** about 0.95 s per call and about half a core-second per call, because it asks the daemon binary for its version, reads the firmware file and more. At a 1 s interval it never idles. A light command costs about 80 ms per call.
- The HBC service is a node process with 9 threads and about 31 MB RSS that stays resident for the whole session when polled.

### P9 Log leakage (`verified-on-target` for the places searched)

- A marker present in the exec **command string** and in its **stdout** was **not found anywhere readable**: 77 files under `/tmp/var/log` (`/var/log` is a symlink to it), `/var/luna`, `/tmp/pmlogdaemon` (text and compressed), nothing in `/run/log`, `/media/developer/log` (both empty), `journalctl -a` (there are no journal files), `logread`, `dmesg`. A first search with a 90 s limit over a wider tree timed out and was redone over the named directories.
- `/var/log/legacy-log` (about 2 MB, rotated with `.gz` copies) records only the start of the HBC service (`run-js-service FULL_SERVICE_NAME org.webosbrew.hbchannel.service`, 11 lines), never an `exec` call or its text.
- Limits: the search could not cover memory-only buffers (`ls-hubd`'s own log level can be raised by LG tools), and it used `luna-send` as the caller, not the app's `PalmServiceBridge`. A command line is of course visible in `/proc/<pid>/cmdline` of `luna-send`, `sh` and the command while it runs.

### Satellite resources (item 9, `verified-on-target`)

- Same pid 8898 before and after (no restart); `webos-satellite` RSS 1.7 MB (the documented figure for 0.3.0 is 1.7 MB), 6 threads; 1.6 to 2.0 percent of one core in 45 s windows with and without the exec calls; `ctl.sh status --json` after: running, no crash loop, microphone not muted; kernel `tainted` unchanged at 4097; the only kernel messages in the window are the Wi-Fi driver's usual `WARN` lines. HA's side was not read (no HA access in this run).

## Conclusion and confidence

- **c3-a accepts inbound connections on random high ports, in both IPv4 and IPv6, from the same /24 (verified for one wired PC).** No host firewall exists (`iptables` empty with ACCEPT policy, no `ip6tables` support). The design's fallbacks for a blocked port (5.2) stay as design text for other TVs and networks; they are not needed here. Phone and other-VLAN reachability remain open (Part B).
- **Bind choice:** the TV has one IPv4 address (Wi-Fi). `0.0.0.0` with the peer filter and the `Host` check is fine; deriving the interface address is not needed on this unit. Binding the Wi-Fi address only (P10) would add nothing here and was not tested.
- **The TV has no usable fallback web server:** no `httpd` applet, no `http.server`, no `socketserver`.
- **Web engine:** Chromium 94 by inference (`hypothesis`), ES2015+ expected and not verified.
- **Exec:** a detached process survives the exec return and the HBC service's idle exit; polling through `exec` works at 1 s and 2 s with no failures. **A light command costs about 80 ms and about 7.7 percent of a core at 1 Hz (3.8 percent at 0.5 Hz); `ctl.sh status --json` costs 0.95 s and half a core-second and cannot be the poll.** The polled command (`setup status --json`) must be as light as `ctl.sh --help`: read `state.json` only, no luna call, no daemon binary.
- **Recommended poll interval:** 2 s normally, 1 s only while a request is pending or its answer has not yet been seen; a 15 s heartbeat grace is then 7 missed beats. Confidence: medium (one TV, one state, one run of 45 to 60 s each).
- No leakage of the exec command or its output into any readable log (high confidence for the places searched, none for in-memory buffers).

## Rollback and cleanup

- Nothing on the TV was changed: no rule, no service, no setting, no package. The listeners were stopped (no socket on the test ports afterwards), the `sleep 299` was killed (none left), `/tmp/exp012/` and `/tmp/exp012-listen.py` removed, the satellite stayed up with the same pid.
- On the PC: raw output stays in `devices/private/c3-a/EXP-012/` (gitignored).

## Follow-ups / unresolved questions

1. **Part B** with the owner and a phone: the QR scan, the page, cookies on plain HTTP, the real remote's key codes 13 and 461, the `Host` check, a phone on another VLAN or the guest network, `navigator.userAgent` in the app, timers while the app is hidden. Needs the built server and the redesigned app.
2. P8 and P10 with the built `webos-satellite-setup` (RSS, CPU, descriptor limits, `0.0.0.0` against the Wi-Fi address).
3. Survival of a detached process when the app is closed, the HBC service restarts, and the TV goes to `Screen Off` or `Active Standby` (not tested: the TV is the owner's active display).
4. Repeat P1 to P4 on c3-b (read only) if the owner agrees; c3-b may be on Ethernet and have a different default route.
5. Cold start cost of the HBC service after an idle exit (about 3.5 s in the earlier record, not remeasured here).
6. Whether `ls-hubd` logs `exec` commands when its log level is raised, and whether the app's `PalmServiceBridge` path logs differently from `luna-send`.
