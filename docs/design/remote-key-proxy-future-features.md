# Remote key proxy: possible future features

- **Status:** ideas only (owner request 2026-11-07: "document all of those ideas as possible future features"). **Nothing here is designed in detail, decided or built.** Each feature would need its own owner decision, and most need an ADR, because they extend what the grab of [ADR 0016](../decisions/0016-remote-key-grab.md) (itself proposed, not accepted) does.
- **Builds on:** the grab-and-resend proxy of ADR 0016 (`remote-proxy serve`), proven for the mic key on c3-a in [EXP-026](../experiments/EXP-026-mic-key-grab-reinject.md); key codes and device layout from EXP-026 and the [interception research](../research/mic-button-key-interception.md).
- **Extension point (proposed in ADR 0016):** `remote-proxy serve` reads a small configuration list of keys to catch, each with an action (`report`, `drop`, or `report and drop`). The mic key is the first entry; the features below are new entries plus logic in the daemon, not a rewrite of the proxy.

## Cross-cutting points

- **Always-on grab:** the mic button feature grabs the remote only while it is on. Remapping (1, 2), assistant controls (3), the activity signal (4) and blocking (5) need the grab **whenever the feature runs**, which can be all the time. That raises the stakes on the dead-man of ADR 0016 (helper loop watchdog, parent link, daemon heartbeat, supervisor kill): a hang would cost the household the Magic Remote until it fires. Before any of these, the dead-man needs a long soak on the TV (days, not minutes) and a recorded count of restarts.
- **Privacy:** only keys on the configured list ever leave the TV, and only as the configured event (for example "red button pressed"), never as a key log. No keystroke logging, no counts per key in HA, no timing patterns beyond what a feature needs. The pointer is never reported.
- **Safety:** Power, Back, Home, volume and the navigation keys are never dropped, remapped or blocked (LG Input Mapper keeps the same list unremappable). The IR remote (`LGE RCU`) and the phone app stay untouched by every feature, as the user's way out.
- **Portability:** as ADR 0016: the grab and the sibling resend are reported by LG Input Mapper and magic_mapper from webOS 4 to webOS 11; key codes vary by remote generation and firmware (on c3-a: Home 773, Back 412, OK 28 or `BTN_LEFT` 272, LG codes 1194 to 1199), so every key list is per firmware, read from a trace (`remote-proxy trace`), never assumed.
- **Entities:** report actions go to Home Assistant as ESPHome events on the satellite's existing API connection (an event entity with a fixed list of event types), so automations use them like any button.

## 1. Remap unused buttons to Home Assistant events

- **What:** buttons the household does not use (streaming-app shortcuts such as Netflix or Prime Video, the colour buttons, LG's own app keys) become Home Assistant events instead of their LG action.
- **User value:** physical buttons for scenes, lights or a "goodnight" routine, on a remote already in hand. This is the main use of LG Input Mapper and magic_mapper.
- **Mechanism:** the configured keys get `report and drop`: the proxy drops them from the resend (LG never sees them) and sends the daemon the press and release; the daemon fires an ESPHome event (for example `remote_button` with type `red`).
- **Privacy:** only the configured keys are reported, as named buttons; nothing else.
- **Safety:** a list that can never include Power, Back, Home, volume, navigation or OK; a dropped key that HA does not handle does nothing (no fallback to LG while dropped).
- **Dependencies:** ADR 0016 accepted and shipped; the config list in the proxy; ESPHome event entities in the satellite's API crate (new); per-firmware key codes (trace); a new ADR for remapping (it changes what LG's keys do beyond the mic key).

## 2. Long press and double press

- **What:** on top of 1, a short press, a long press (for example 500 ms) and a double press of one button can be three different events.
- **User value:** more actions from the few free buttons.
- **Mechanism:** the daemon times presses and releases (the remote sends no repeats for most keys, EXP-022; LG Input Mapper and magic_mapper time long presses themselves). A key used this way must be `report and drop` (LG would otherwise act on the first press).
- **Privacy and safety:** as 1.
- **Dependencies:** 1.

## 3. Assistant controls

- **What:** while our assistant is busy, Back cancels a running Assist run or stops the spoken answer; a chosen button stops Music Assistant or other media the satellite plays.
- **User value:** a natural way to interrupt the assistant without a voice command.
- **Mechanism:** Back must keep working for LG, so it is `report` only (not dropped): the proxy still resends it, and the daemon additionally aborts its run or TTS when one is active (as the existing barge-in and stop paths do). A dedicated stop button would be `report and drop` (as 1).
- **Privacy:** only the event "Back pressed while a run was active" is used locally; nothing is sent to HA except the run's own abort.
- **Safety:** Back is never dropped; the double effect (LG closes a menu and our run stops) must be acceptable to the owner, or the action is limited to the moment our overlay is visible.
- **Dependencies:** ADR 0016; the daemon's run abort and media stop (exist); the overlay state (ADR 0014) if limited to its visibility.

## 4. "Remote in use" activity signal

- **What:** an active/idle boolean for the remote (any event in the last N minutes), as an occupancy hint for HA.
- **User value:** presence detection for the living room, for lights or heating, with no extra sensor.
- **Mechanism:** the proxy already sees every event; the daemon keeps one "last activity" time and reports only `active` / `idle` as a binary sensor, switched with hysteresis.
- **Privacy:** a boolean only, never which keys, how many, or pointer data; off by default, because it still reveals when someone is in the room.
- **Safety:** no keys are dropped; read only on top of the grab.
- **Dependencies:** ADR 0016; a binary sensor entity (exists in the API crate's entity set); owner consent for the household signal.

## 5. Schedule-based key blocking

- **What:** from HA, block chosen keys for a time, for example input switching or app keys during children's bedtime.
- **User value:** simple parental limits without LG's own lock menus.
- **Mechanism:** HA switches select a block list; the proxy drops those keys while the list is active.
- **Privacy:** HA knows only the block state, not presses (blocked presses may be counted, not reported, if at all).
- **Safety:** Power, Back, Home and volume are never blockable; the IR remote and the phone app are not affected and remain a way out; a failsafe ends any block when HA is unreachable for a set time and at every daemon restart, so a lost HA never leaves keys blocked.
- **Dependencies:** ADR 0016; an HA config switch per list (as ADR 0012's pattern); a new ADR (it restricts the user's own TV).

## 6. Injecting keys from HA

- **What:** HA sends a key (for example an arrow or OK) that the satellite writes into the free sibling.
- **User value:** small local automations without the network remote API.
- **Mechanism:** the proxy writes the key press and release into the sibling (EXP-026 B1key showed that the compositor takes keys from a sibling on c3-a).
- **Assessment:** **mostly redundant:** HA's LG webOS integration already sends buttons over LG's SSAP network API, without root and without our proxy. Worth doing only for a key SSAP cannot send, or offline from LG's network API.
- **Privacy:** none beyond the command itself.
- **Safety:** an allow list of harmless keys (arrows, OK, Back); never Power or input keys from an automation without the owner's choice.
- **Dependencies:** ADR 0016; an HA service or button entity; a decision that SSAP is not enough.

## Order, if pursued

1 and 4 give the most value for the least risk once ADR 0016 has a long soak behind it; 2 follows 1; 3 needs only small daemon changes; 5 needs its own ADR and the failsafe; 6 is the lowest priority.
