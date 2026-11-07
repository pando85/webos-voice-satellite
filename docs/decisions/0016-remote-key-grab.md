# ADR 0016: Take the Magic Remote's mic key with an evdev grab and re-send the rest into a sibling device

- **Status:** **Proposed (draft, 2026-11-07). NOT accepted.** No owner decision. Nothing here is built; the evidence comes from the test tool `remote-proxy` in [EXP-026](../experiments/EXP-026-mic-key-grab-reinject.md), whose kill test (B3) is still to run. Nothing here may run on a TV on the strength of this draft.
- **Scope:** how the satellite keeps LG's assistant and LG's toast away from the mic button while its mic button feature runs ([ADR 0013](0013-remote-mic-button-coexistence.md)). It adds a mechanism; it does not change what the feature does for the user.
- **Would relax, if accepted:** the AGENTS.md device-safety rules, which today allow only ADR 0013's volatile configd override; this ADR adds an exclusive grab of one LG input device and writes into another (proposed text at the end).
- **Related:** research [mic-button key interception](../research/mic-button-key-interception.md) (candidate C1a, community tools) and [remote mic button](../research/remote-mic-button.md) section 10 (the toast), [EXP-026](../experiments/EXP-026-mic-key-grab-reinject.md), [EXP-023](../experiments/EXP-023-remote-mic-button-handover.md), draft [ADR 0015](0015-silent-mic-key-filter.md), [ADR 0006](0006-soft-firmware-policy-and-compatibility-reports.md) (per-firmware allow list), [design](../design/remote-mic-button.md).

## Context

- **The problem:** with ADR 0013 the satellite sets `com.webos.keyaction.voice` to `ignore` in memory; LG's compositor key filter then shows the toast "Esta función no es compatible." on every mic press (verified-on-target, c3-a; research section 10). No value of that key avoids it.
- **The route (verified-on-target, c3-a `03.30.74`, EXP-026):**
  - `lginput2` writes every Magic Remote event (keys, 428, pointer, wheel, LG codes 1198/1199) to the uinput device `LGE M-RCU - Builtin [0]`; its siblings `[1]` and `[2]` have identical capabilities and stay silent (Part A, B0all).
  - The compositor (LG's `libim` in `surface-manager`) reads `[0]` to `[2]` and **takes keys and the pointer from a sibling** (B1key, B2), but does **not** open a uinput device created later (B4).
  - **B2:** `EVIOCGRAB` of `[0]` and every event except `EV_KEY` 428 re-sent frame by frame into `[2]`, with LG's key left at `launch`: **no toast and no LG assistant** on five mic presses; the pointer, click, wheel, Back and arrows normal (owner); forwarding delay median 0.1 ms, max 3.2 ms over 886 frames; the 10 events of 428 dropped and nothing else (binary log).
  - On webOS 8.3 the compositor's key filter is LG's only trigger of its assistant (research section 9; EXP-022 log), so a key it never sees starts nothing. The remote's audio is independent of the key: `voiceinput` reads it from the HID link, and the satellite's `startStreaming` keeps working (ADR 0013 practice).
- **Kernel facts (documented, Linux `drivers/input/evdev.c`, `input.c`):** while a client holds the grab, the device's events reach only that client; writing into the grabbed device's own node reaches only the grabbing client too (so re-sending into `[0]` is impossible); the grab ends when the fd closes, including on a crash or `SIGKILL`; the input core drops an `ABS` event equal to the current value (harmless for forwarding: we forward the source's own changes).
- **Prior art (observed-on-other-device, project claims):** LG Input Mapper (C5, webOS 10.3) and magic_mapper (C9 webOS 4 to C6H webOS 11.1) use the same grab-and-resend into `[2]` (magic_mapper uses `[1]` from webOS 10, where resending through `[2]` made Back exit apps). A webOS 11.2 report says the AI/mic button no longer reaches evdev there (research section 6).
- **Owner direction:** "I'm open to explore any hacking option that could be extended to any other WebOS version"; the owner ran Part B and reported "the test was perfect".

## Decision (proposed)

1. **Mechanism ("grab"):** while the mic button feature runs, the satellite holds an exclusive grab of the remote's active input device(s) and re-sends every event except `EV_KEY` 428 (press, release, repeat) unchanged, frame by frame, into one idle sibling device. 428 goes to the satellite's push-to-talk machine instead. Nothing else is remapped, dropped, added or delayed.
2. **Choosing devices, never by `eventN`:**
   - Candidates: event nodes named `LGE M-RCU - Builtin [n]`, found in sysfs by name.
   - **Target:** one candidate with capabilities and `ABS` ranges identical to the source's (`EVIOCGBIT`, `EVIOCGABS`), chosen per firmware: `[2]` on webOS 8.3 (EXP-026), `[1]` from webOS 10 (magic_mapper's finding), recorded in the firmware allow list (ADR 0006). If no identical sibling exists, the grab is not used (no uinput clone: B4).
   - **Sources:** every other candidate (on c3-a `[0]` and `[1]`; `lginput2` writes to `[0]`, a second paired remote may get another slot). They are grabbed only when no key is down on them (`EVIOCGKEY`), so the compositor never keeps a key it saw go down.
   - `LGE TONE+ - Builtin [3]`, `LGE RCU` (IR), `LGE Simple Premium` and every other device are never touched.
   - Reconnects: the uinput devices outlive Bluetooth reconnects (EXP-022; `lginput_uinput_destroy` is a stub, static). If `lginput2` restarts, the read fails with `ENODEV`: the helper releases everything, the daemon restarts it with backoff, and it rediscovers by name.
3. **Process model:** a small separate helper process, `remote-proxy serve` (from the EXP-026 tool), started by the daemon, with one job: grab, forward, report. It sends the daemon each 428 press and release with its kernel time, and a heartbeat, over a pipe. The satellite's own passive reader is replaced by this pipe (a passive reader receives nothing from a grabbed device).
4. **Dead-man (all layers always on; the kernel releases the grab whenever the helper's process ends):**
   1. **Loop watchdog in the helper:** a thread that, when the forwarding loop has not turned for 1 s, writes releases for every key held down in the target and `_exit`s.
   2. **Parent link:** the helper exits when its pipe to the daemon closes, and it is started with `PR_SET_PDEATHSIG = SIGKILL`, so it never outlives the daemon.
   3. **Daemon-side heartbeat:** the daemon `SIGKILL`s the helper when its heartbeat is older than 1.5 s, then restarts it with backoff.
   4. **Supervisor and `ctl.sh`:** `supervise.sh`, `ctl.sh stop`, `restore-lg-voice`, `uninstall` and `purge` kill any `remote-proxy` process (by pid file and `comm`) before anything else. A TV restart clears everything; nothing is persistent.
   - Every exit path of the helper releases keys still down in the target before it ends. The EXP-026 test tool's hard cap (60 s) does not apply to the product; layers 1 to 4 replace it.
5. **Relation to ADR 0013: keep its volatile override as the safety net.** While the grab runs, `com.webos.keyaction.voice` stays at `ignore` (ADR 0013 decision 2(a), unchanged, with its restore paths). In normal use the key filter never sees 428, so the override costs nothing; if the grab is lost (helper crash, restart, an unexpected device layout), a press shows LG's toast instead of starting LG's assistant and sending audio to LG's cloud. The toast then doubles as a visible failure signal. On stop the order is: release the grab, then restore the override. Where the grab is not available (firmware not on the allow list, no identical sibling), the feature runs as today with the override alone.
6. **Firmware gate:** the grab is enabled only on firmware where EXP-026 B2 and B3 passed (ADR 0006 allow list); elsewhere ADR 0013 behaviour.
7. **Reporting:** `lg_voice_key` gains `ours (grab + configd)`; status and HA show the helper's state, its restarts, and the forwarding delay (median, max) as diagnostics. Logs carry codes and counts only, never coordinates.
8. **Off is inert:** with the mic button feature off, the helper does not run and nothing is grabbed (as ADR 0013 decision 1).

## Consequences

- **Gain:** no LG toast and no LG assistant on the mic button, with no change to LG's compositor code, LG services or files; the configd override stays only as a safety net.
- **Pointer and latency:** the Magic Remote sends pointer frames at about 100 Hz (EXP-026 trace); one read and one write per frame added a median 0.1 ms and at most 3.2 ms (B2). Budget for acceptance: median below 2 ms, maximum below 10 ms (one frame period). LG's codes 1198 and 1199 (pointer wake and related, trace) pass through unchanged.
- **Risks:**
  - **A dead Magic Remote** if the helper hangs with the grab held: bounded by dead-man layers 1 and 3 (about 1 to 1.5 s); the IR remote and the phone app keep working (separate devices).
  - **Stuck keys:** handled by grabbing only with no key down, releasing on every exit, and a resync after `SYN_DROPPED`.
  - **A second Magic Remote:** if `lginput2` gives it the target slot (`[2]`), its events mix with ours in the target, and its mic key is not dropped (the safety net shows the toast). To verify with two remotes before acceptance on a household that has two.
  - **Firmware updates:** a different slot layout, sibling behaviour (the webOS 10 Back quirk) or a voice key that leaves evdev (the webOS 11.2 report) disables the grab through the firmware gate; the override still works where its own gate passes.
  - **Home and volume during the grab:** absent from B2's grabbed stream although the owner reports them working (EXP-026 residual); to settle before acceptance.
  - **CPU:** one wake-up per remote frame (about 100 per second while the pointer moves, none at rest); to measure on the TV.
- **Rollback:** turn the feature off (the helper stops, the kernel releases the grab, ADR 0013's restore runs), or `ctl.sh restore-lg-voice`, or a TV restart. Nothing on disk changes.
- **Rules:** the exception below is needed. Experiments that exercise the grab still need the owner's authorization per run.

## Draft ADR 0015 (key-filter handler)

**Keep it as a documented fallback, not pursued now.** It is the only known route for firmware where the grab cannot work (no identical sibling, or a voice key that no longer reaches evdev, as reported for webOS 11.2), but it puts our code into LG's compositor and rebuilds the whole key-filter chain on every change. If this ADR is accepted, ADR 0015 stays "proposed (deferred)" and is reconsidered only for such firmware.

## Proposed rule change (proposal only; AGENTS.md is not edited by this ADR)

In [AGENTS.md](../../AGENTS.md), "Device safety and privacy", extend the ADR 0013 exception with:

> In addition (ADR 0016, `docs/decisions/0016-remote-key-grab.md`), while the mic button feature is enabled on firmware where EXP-026 passed, the satellite's helper may hold an exclusive grab (`EVIOCGRAB`) of the `LGE M-RCU - Builtin` input devices that carry the remote's events, and write every event except the mic key (428) unchanged into one idle sibling device of identical capabilities. It must run under the four-layer dead-man of ADR 0016, release the grab and every held key on feature off, stop, crash and uninstall, never touch other input devices, and never persist anything. Experiments that exercise the grab still need the owner's authorization per run.

## Implementation plan (if accepted)

- **Before building:** finish EXP-026 (B3 kill test, the filtered system log of B2's window, cleanup); one trace with Home and volume during a grab; c3-b Part A (read-only).
- **Work (about 7 to 8 days):**
  1. Turn the EXP-026 tool into `remote-proxy serve` (no cap; the pipe protocol: press, release, heartbeat, stats; multiple sources, one target; rediscovery), reusing its tested modules (2 days).
  2. Daemon: start and supervise the helper (PDEATHSIG, heartbeat watchdog, backoff), feed its 428 events into the existing button machine instead of the passive reader, keep ADR 0013's override as the safety net, status and HA fields (2 days).
  3. `supervise.sh`, `ctl.sh` (stop, restore, uninstall, purge), packaging of the second binary for armv7 and aarch64, the firmware gate entry for c3-a `03.30.74` (1 day).
  4. Tests (1.5 days): the existing unit tests; host uinput end-to-end runs (forwarding, 428 dropped, releases on every exit, the helper `SIGSTOP`ped → the daemon kills it within 1.5 s and the grab is gone, daemon killed → the helper exits, `ENODEV` → rediscovery); a 30 min soak on the host.
  5. Docs: design note, AGENTS.md exception, ADR 0013 cross-reference (0.5 day).
- **Release target:** 0.6.0, after 0.5.1 (the install-permission fix). Off by default as today; the grab only where the firmware gate allows it.
- **On-TV acceptance (owner, c3-a, about 20 minutes, install only plus the owner's own use):**
  1. Install 0.6.0, turn the mic button feature on; `ctl.sh status --json` shows `lg_voice_key` `ours (grab + configd)` and the helper running.
  2. Ten mic presses (taps and holds) during normal use: our assistant runs each time; **no toast, no LG assistant**.
  3. Pointer, click, wheel, Back, Home, held volume, arrows, number keys, OK: all normal.
  4. Leave the remote idle 5 minutes, then pick it up and press the mic button at once (wake path).
  5. Kill test: `kill -9` of the helper (or the daemon) from SSH while using the remote: the remote keeps working within about 1.5 s; a mic press then shows the toast (safety net) until the helper is back, then none.
  6. Turn the feature off: the mic button opens LG's assistant again; `lg_voice_key` `lg`.
  7. Status diagnostics: forwarding delay median below 2 ms, max below 10 ms; CPU of the helper with the pointer moving.
