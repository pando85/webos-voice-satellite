# EXP-013 — Fresh "add to Home Assistant" on c3-b, and what comes back

**Status:** RUN 2026-11-05 (agent steps A1 to A6 and A8 done, A7 not needed; owner steps O1 to O6 done by the owner, who reported only "it works like a charm", so his texts and phone-link remarks are INCONCLUSIVE)
**Date:** prepared 2026-11-05; run when c3-b is on and the owner is present
**Device alias / model / firmware / webOS / architecture:** c3-b, LG `OLED55C36LC`, firmware `03.30.73`, webOS TV 8.3.0, aarch64 kernel with 32-bit userland, Homebrew Channel 0.7.2, Ethernet, ARC sound output (all from [P4 on c3-b](../discovery/c3-b/P4-permanent-install.md)). Satellite package on it now: **0.2.3**. Target package: **0.4.1**.
**Power and playback state:** must be on and idle (`Active` or `Screen Saver`), nothing playing, nobody watching. The TV is the household's hall TV. LG hands-free ("Hi LG") is on as the household left it and is **not touched** (the microphone is busy, so no audio path is exercised; the test needs none).
**Related research / requirements / issue:** [phone setup, section 10](../design/phone-setup.md#10-add-to-home-assistant), [pairing window, install UX section 5](../design/install-and-config-ux.md#5-pairing-mode-design-option-c-detail), [c3-b P4](../discovery/c3-b/P4-permanent-install.md), [c3-a P4 (0.4.1 update)](../discovery/c3-a/P4-permanent-install.md), [phone-page probe EXP-012](EXP-012-phone-setup-probe.md) (items P11 to P13 are the same phone path)
**Risk:** configuration change on a household TV (an in-place update, a pairing reset) and **writes in Home Assistant** (delete the ESPHome entry; re-apply customisations). No sound, no reboot, no LG setting change.
**Approval needed / obtained:** owner approved the test (2026-11-05, through the coordinator): install 0.4.1, reset pairing, delete the HA ESPHome entry of c3-b, the owner adds it again from his phone, then the customisations are restored. **The standing "install only" rule on TVs must be lifted by the owner for this run** (record the date and time of that message in Observations). The owner must be present with the phone.

Raw output stays in the gitignored `devices/private/c3-b/EXP-013/` and is never committed. This file has no address, hostname, MAC, key or HA id.

## Question and falsifiable hypothesis

A person who already has the TV in Home Assistant sets it up again following only what the product tells them (TV app, QR code, phone page, Home Assistant). Three questions:

1. **Flow.** Does the path work end to end with no key typed: delete the old entry, TV app "Set up with your phone", scan the QR code, "Start pairing" (OK on the TV), "Open Home Assistant", ESPHome add with the address and port, Home Assistant sets the key?
2. **Guidance.** Are the new texts right and enough: the confirm texts of Reset pairing and Remove service on the TV, and the phone page's "Open ESPHome in Home Assistant" link with the delete-the-old-entry sentence?
3. **What comes back.** After the new entry exists, are the entity ids, the device area (Living Room), the Assist exposure and the aliases the same as before?

Hypotheses, each falsifiable. Labels follow [AGENTS.md](../../AGENTS.md); sources are Home Assistant core `2026.10.4` files read on 2026-11-05 unless said otherwise.

| # | Hypothesis | Label |
| --- | --- | --- |
| H1 | When the entry is deleted and Home Assistant can still connect with its key, it sends an empty key to the device and removes its stored key (`_async_clear_dynamic_encryption_key`, `esphome/__init__.py`). The TV then shows "Unpaired by Home Assistant" (`key = cleared`) with the window closed ([install UX 5.2 rule 6](../design/install-and-config-ux.md#5-pairing-mode-design-option-c-detail), observed with a PC fake in its section 8) | documented (source); observed-on-other-device (PC run); **not yet verified on c3-b** |
| H2 | If Home Assistant cannot connect at deletion (TV off, key already reset), the TV stays `paired` or `unpaired` and Home Assistant **keeps its stored key for this MAC** in `.storage/esphome.encryption_keys` (the store holds a key for each of c3-a and c3-b today, counts only). The next add then **reuses that stored key** instead of generating one (`_handle_dynamic_encryption_key`, "Use the stored key") | documented (source); storage seen read-only; hypothesis for the outcome |
| H3 | No key is typed: the add flow connects to the keyless window, the manager sends the key over the zero-key Noise session (`_async_provision_key_over_noise`), the TV logs `key set` and `PAIRED by` | documented; verified-on-target on c3-b on 2026-11-04 (first install, [P4](../discovery/c3-b/P4-permanent-install.md)), with 0.2.0 and no phone |
| H4 | Entity ids come back the same, including the 11 that carry the area name (`living_room_...`). Removing a config entry moves its registry entries to `deleted_entities`; creating an entity with the same domain, platform and unique id within 30 days (`ORPHANED_ENTITY_KEEP_SECONDS`) restores its **entity id (if free), aliases, area, labels, name, icon, disabled and hidden state and options**. The unique ids hold the MAC (kept by `mac_seed`, which survives updates) | documented (`entity_registry.py`: `async_remove`, `async_get_or_create`); hypothesis for this entry |
| H5 | The device comes back with its area (Living Room), user name and labels: the device registry restores a deleted (orphaned) device with the same domain and MAC connection within 30 days (`device_registry.py`, "re-adding an integration restores the device id, area, labels and name") | documented (source); hypothesis for this device |
| H6 | The Assist exposure comes back with the entity: it is stored in the entity's registry options (`options.conversation.should_expose`, seen read-only on all 36 entities) and options are restored (H4) | documented (source); observed in the live registry |
| H7 | Entities that did not exist in the snapshot (new in 0.4.1) are created fresh, with new ids that carry the area name if the restored device already has the area, and exposure by Home Assistant's own defaults | hypothesis |
| H8 | The My Home Assistant link `https://my.home-assistant.io/redirect/integration/?domain=esphome` opens the ESPHome integration page of the owner's instance | documented in the redirect list (`redirect.json`: `integration` takes `domain`, introduced 2023.6); the human pages list no parameters; **not tested on a phone** |

If H4 to H6 hold, restoring is not needed and the restore tool only proves it. If one fails, the tool re-applies the difference.

## Prerequisites and safe baseline

- c3-b on and idle (agent: `ping`, then `ctl.sh status --json` over SSH, **read-only**), owner present with the phone on the same network as the TV, owner has lifted the install-only rule for this run, no sound anywhere.
- Home Assistant 2026.10.4 at the owner's address; the owner's phone has the Home Assistant instance URL saved for My Home Assistant (the first use shows a confirm page).
- `make test-packaging` green and the 0.4.1 ipk built with the setup server (`make build-tv-all`, `make ipk`, the same as for [c3-a](../discovery/c3-a/P4-permanent-install.md)); its sha256 recorded.
- A short-lived HA token in a `0600` scratchpad file, from the login flow in `~/homelab/docs/troubleshooting/home-assistant-api-hacs-deploy.md`; the refresh token this session creates is the **only** one deleted at the end (its id is the `iss` claim of the access token).
- **The "before" snapshot** (this run's own, taken at the start, plus the one from 2026-11-05 already in the private folder): `python3 -I tools/ha-restore/ha_restore.py snapshot --ha-url <HA> --token-file <FILE> --device-name "(c3-b)" --out devices/private/c3-b/EXP-013`. A dry-run `restore` against it must say "Nothing to change".
- Nothing in Home Assistant refers to these entities, so no automation breaks while they are gone: all files under `/config` of the HA pod (except the database and logs), the AppDaemon `/conf`, and the GitOps repository were searched read-only for the entity id stem, the device id, the host name and the MAC. Only the registries, the config entry and ESPHome storage, and (in GitOps) a network diagram matched; HA's `search/related` returned no automation, script, scene or group for any of the 36 entities and no storage dashboard mentions them.

### Snapshot of 2026-11-05 (counts only; the file is private)

One device, area Living Room, no user name, no labels. **36 entities**, all ESPHome: 10 exposed to Assist (`conversation`), 26 not; 9 with aliases (Spanish, "del hall"; the media player "TV", the screen and mute switches, the volume number, the volume up and down buttons, the turn-on button, the TV input and TV app selects); 0 with an own name, 0 with an entity-level area (they inherit the device's), 0 labels, 0 disabled, 0 hidden, 0 options other than exposure. **11 entities have the area name in their entity id** (they were created after the device got its area); the other 25 do not. Aliases are stored the Home Assistant way, a list in which a null item means "also the entity's name"; the tool keeps them exactly.

## Commands / steps (copyable and bounded)

Order matters, see H1 and H2: deleting **first**, with the TV on and paired, lets Home Assistant clear the key on the device and in its store, so the new add generates a **fresh** key. Resetting the pairing on the TV first makes Home Assistant's deletion unable to clear anything, and the add then reuses the stored key (H2).

**Agent steps** (A), with the owner's lifting recorded; TV commands under `timeout`; every command read-only unless marked:

- **A1.** Read-only baseline: SSH to c3-b, `ctl.sh status --json` (key, port, version, ha), power state, foreground app; HA: the device is `unavailable` or connected, entity count. Take the "before" snapshot (above).
- **A2.** *Install 0.4.1 over the running 0.2.3* (as for c3-a): `scp` of the ipk, `tv/appinstall.sh install`, then `ctl.sh install` (config and key kept, the daemon restarts, HA reconnects within about 10 s with the stored key). Record the installer output, `ctl.sh status`, the HA reconnect, **the entity count and which entity ids are new** (H7), and that the key is still `paired`.
- **A3.** *Delete the entry* (**HA write**): the agent calls `DELETE /api/config/config_entries/entry/<entry_id>` (the id is in the device's `config_entries` of the live registry), **or** the owner does it in the HA app by hand (Settings, Devices & services, ESPHome, the TV, Delete), which also tests the wording of the guidance. Record the time.
- **A4.** Read-only after the delete: HA has no device and no entities for the MAC; on the TV `ctl.sh status --json` says `key: cleared` (H1) and `pairing.open: false`; the daemon log records the clear by the key holder (read the line; it never contains the key). **If the key is still `paired`** (Home Assistant could not connect, H2): ask the owner to do step O0.
- **A5.** During the owner's steps, watch read-only: the HA config flow in progress (`config_entries/flow/progress`), the TV log tail (the `key set`, `PAIRED by` lines only, no audio), `ctl.sh status --json` (`pairing`, `key`, `ha.state`), and the new device and entities in HA.
- **A6.** After the add: take the "after" snapshot into `devices/private/c3-b/EXP-013/after/` and run `restore` as a **dry run** against the "before" file. Record the plan's counts per kind (rename, update, expose, missing, conflict, skipped).
- **A7.** Only if the plan is not empty (an H4 to H6 failure): `python3 -I tools/ha-restore/ha_restore.py restore ... --apply` (**HA write**, approved for this test), then a dry run again, which must say "Nothing to change". Record what had to be re-applied; that is the finding.
- **A8.** Read-only conversation check (no sound, no action): the sentence "¿está la tele del hall encendida?" through the conversation agent's API answers from the alias. Then delete the agent's own refresh token and shred the token file.

**Owner steps** (O), on the TV remote and the phone:

- **O0.** *Only if A4 found the key still paired:* on the TV open the app, Home Assistant, **Reset pairing**; the first press shows the confirm text: **it must say to delete the device in Home Assistant too** (Settings, Devices & services, ESPHome, this TV, Delete) and that Home Assistant shows an error until it is paired again. Press OK again. (This also tests the new text. If A4 showed `cleared`, O0 is skipped; the owner can still press Reset pairing once, read the text, and let the 5 s pass without the second press.)
- **O1.** TV app: Home, Home Assistant, **Set up with your phone**. A QR code appears.
- **O2.** Phone: scan the code with the camera and open the link **in the browser** (not an in-app scanner, which loses the cookie). The page shows the status cards, then "Add to Home Assistant" with the address, the port, the copy buttons, **Start pairing**, **Open Home Assistant**, the new link **Open ESPHome in Home Assistant** and, under it, the sentence about deleting the old entry.
- **O3.** Phone: tap **Start pairing**. The TV shows the question to open the pairing window for 10 minutes: press **OK**. Both screens then show a countdown.
- **O4.** Phone: tap **Open Home Assistant** (the first time Home Assistant's own confirm page appears: "Open link"). Home Assistant shows its add dialog. If it does not open the ESPHome form, follow the steps on the page: Settings, Devices & services, Add integration, ESPHome. Enter the address and the port (copy buttons), submit. **No key is asked.** If Home Assistant shows a discovered device instead, accept it.
- **O5.** If Home Assistant offers to choose an area at the end, choose **Skip** (the area must come back by itself; write down what Home Assistant proposed).
- **O6.** Phone: tap **Open ESPHome in Home Assistant** (H8) and see whether the ESPHome integration page opens. Then **End session** on the page (or Done on the TV).

## Expected evidence and failure interpretation

| Signal | Expected | If not |
| --- | --- | --- |
| After A3 (A4) | `key: cleared`, window closed, device and entities gone in HA | `paired` still: H2 (HA could not connect); do O0. A failed delete: HA error text; stop. |
| TV pairing countdown (O3) | Home Assistant screen shows "left to pair" counting down from about 10:00 | No countdown: pairing not opened (the OK was not pressed or timed out after 60 s); repeat. |
| Phone countdown (O3) | "Pairing is open, m:ss left" | Page stuck on "Waiting for OK on the TV": the OK was not pressed; or the session ended (10 min). |
| Add flow (O4) | Host and port accepted, no key prompt, entry created within seconds; TV log: keyless probe, zero-key session, `key set`, `PAIRED by "Home Assistant ..."` | A key prompt: the window is closed (expired, or never opened), or an old keyed satellite answered: repeat O3. "Cannot connect": HA cannot reach the TV address and port (a network rule, as in the c3-a notes; not a satellite bug). "Already configured": the old entry is not gone. |
| New key (H2) | If A4 showed `cleared`: the key on the TV changes (compare `sha256sum` of `etc/noise_psk` before and after, **never print it**) | Equal: HA reused a stored key, so H1 did not clear the store; record it, it is still a working path. |
| Entity ids, area, exposure, aliases (A6) | The dry-run plan says "Nothing to change" (H4 to H6) | A rename plan: H4 failed, the ids came back different (record the pattern, for example the area prefix missing). Updates or exposes: H5 or H6 failed. `missing`: an entity of the snapshot was not created by 0.4.1 (record its name; it may be a removed feature). |
| Extra entities (H7) | A few entities not in the snapshot (new in 0.4.1), listed by name with their ids | None: nothing new in this version. |
| Phone link (H8) | The ESPHome integration page opens | A "not found" or a generic page: the link is wrong; fix `MY_HA_ESPHOME` and the doc. |
| Guidance texts | The owner finds the delete path in the TV text and the phone page and can follow it | Too long or unclear on the TV: shorten in `voicepower.js` `HA_CLEANUP`. |

Anything not run is `INCONCLUSIVE`, not `verified-on-target`.

## Observations and redacted evidence links

**Part 1 (2026-11-05, agent steps A1 to A4).** The owner lifted the install-only rule for c3-b on 2026-11-05 (relayed by the coordinator in the task message; the time of day was not given). Target package is now **0.4.2** (not 0.4.1: it adds the clean-up guidance texts of `5d7320e`); sha256 `9db7038069790571d748276e8d7b37c27f5ebb9640a33d9f89dfafc28dd508d1`. Times are UTC.

- **A1.** Baseline before the update (c3-b on, `Screen Saver`): 0.2.3 running, supervised, key `paired`, HA connected. "Before" snapshot: 36 entities, 10 exposed, 9 with aliases, device in the area Living Room, 0 labels; a dry-run restore against it said "Nothing to change".
- **A2.** 0.4.2 installed over 0.2.3 at about 11:01 (details in [c3-b P4](../discovery/c3-b/P4-permanent-install.md)); config and key kept, HA reconnected in 4 s. **38 entities** now: the two new diagnostic text sensors "Always Ready" and "LG hands-free" (H7: created with the area name in the entity id, as the device already has the area; not exposed). A second snapshot (38 entities) was taken as the baseline for part 2; the dry run against the 36-entity one still said "Nothing to change". c3-a: 0.4.2 installed too, install only (see its P4).
- **A3.** At 11:02:06 the agent deleted the c3-b entry with `DELETE /api/config/config_entries/entry/<id>` (answer `require_restart: false`) while HA was still connected.
- **A4 (H1 verified-on-target).** Within 1 s the daemon logged `PAIRING CLEARED by the key holder ... stored key deleted ... unpaired, and the pairing window stays closed until it is opened locally`, then `HA disconnected`, `microphone closed, PCM released (not paired)`. `ctl.sh status --json` 6 s after the delete: `key` `cleared`, `pairing.open` false, `ha.state` `unpaired`, the daemon still running; `etc/pairing/` holds only `noise_psk.cleared`. In HA: the c3-b entry is gone from the ESPHome entries (the c3-a entry untouched) and the device is no longer found. H2 (HA could not connect) did **not** occur, so O0 is not needed. The key's `sha256` before the delete was not captured (wrong path in the command), so the "key changes" check of the table compares only against `cleared`.
**Part 2 (2026-11-05, owner steps and agent verification).** The owner did O1 to O6 on his own and reported "it works like a charm" (no details: whether Home Assistant asked for a key, what it proposed for the area, what the TV and phone texts looked like, and whether the ESPHome link opened are **not reported**). The agent read the evidence below afterwards; all times UTC, from the daemon log, the setup log and Home Assistant.

- **Timeline.** 11:02:06 entry deleted, key `cleared`. 11:09:10 to 11:09:13 the kill switch file was present (the mic reported muted, then off "not paired"; not by the agent, cause not recorded). 11:09:57 the pairing window opened (`pair` file found, 600 s). 11:10:10 setup session start, 11:10:17 phone page opened (one Firefox, other subnet, state polled for about 10 s). **11:12:03.10 the new config entry created** (HA `created_at`), 11:12:03.25 `key set` and `PAIRED by "Home Assistant 2026.10.4"`, 11:12:03.29 microphone open, **11:12:08.28 HA connected with the new key** (5 s later), streaming at 11:12:13. 11:13:13 `setup session end reason=done`. Delete to new entry: 10 min; pairing window to entry: 2 min 6 s.
- **A5/O3/O4 (H3 verified-on-target again, with 0.4.2).** The daemon log at 11:12:03 shows three keyless sessions in the same second: a plaintext probe by `aioesphomeapi`, a plaintext probe by Home Assistant, and a keyless zero-PSK Noise session; the last one set the key (`key set by ... (mode 0600)`, the other two closed: "pairing state changed"). A key typed by the owner would not produce that sequence, so **no key was typed (inferred from the TV log; the owner did not say)**. HA's stored entry: `source` `user` (not `zeroconf`), `host` and `port` 6053 set (the TV's address and port, as shown by the TV), `noise_psk` present (set by HA), `options` `allow_service_calls` false, no discovery key. So the flow was user-initiated with host and port. Two plaintext handshakes failed with `unexpected frame preamble 0x00` and HA logged `Connection requires encryption` once, all within 5 s after the key was set (HA's first connect after the key change); no effect.
- **TV after the add (read-only).** `ctl.sh status --json`: `key` `paired`, `pairing.open` false, `ha.state` `connected`, `mic_off` false, daemon pid unchanged since the update. `etc/pairing/` holds `noise_psk` (mode 0600, written at 11:12) and no `.cleared` file. The setup server is gone: the setup log ends with the session `done`, no setup process, no listener on the session port (the only daemon listener is the API port). TV `/tmp` holds nothing of ours. **New key, not the old one:** the TV's key file was deleted at 11:02 and rewritten at 11:12 by HA's provisioning; the old key's hash was not captured (see A4), so "differs from the old one" is **not hash-verified**; HA's key store holds the same number of keys as before (2, counts only), consistent with H1 clearing the c3-b key there and the add storing a new one, but that is not proof.
- **A6 (H4, H5, H6 verified-on-target).** After snapshot: **38 entities, the same 38 entity ids and unique ids as the baseline** (36 + the two sensors new since 0.2.3). The device id is the same as before the delete, only the config entry id is new; area Living Room, name, no user name, no labels. Exposure (10 of 38 exposed), the 9 alias lists, options, own names, icons and entity areas compare **identical** to the baseline; the 13 entities with the area name in their id (11 of the 36 plus the two sensors) kept their ids. The **dry-run restore against the baseline said "Nothing to change" (0 writes)**, so A7 was **not run**: nothing was re-applied, Home Assistant restored everything itself.
- **H7 not exercised.** The two entities that did not exist in the 0.2.3 snapshot had already been created by the 0.4.2 update *before* the delete, so they were restored with the rest. "New entities after a re-add get fresh ids" was not observed; keep it `hypothesis`.
- **H8 INCONCLUSIVE.** Neither the TV nor Home Assistant sees a click on a My Home Assistant link, and the owner did not report it.
- **Unplanned, not by the agent.** (1) At 11:12:08, 5 s after the new entry connected, the satellite fetched and played an announcement URL (0 chars of text, 4,353 ms of audio, its media player `playing` 11:12:08 to 11:12:13). It is audible output on the household TV; the agent made no service call. Hypothesis (HA code not read): the connection test of Home Assistant's own voice-satellite setup dialog, which the frontend opens after adding an ESPHome voice satellite and which plays a test sound. (2) At 11:13:26 Home Assistant set the satellite's **reply volume** from 40 to 79 % (the satellite's own setting, not the TV volume; it stays at 79 % until changed). (3) The "Always Ready" sensor read `Off` at 11:01 and `On` at 11:12 (an LG setting; changed by the owner or the app's hint, not by the agent; not read from the TV). (4) 11:15:42 the TV was powered off with the remote (`Request Power Off`, reason `remoteKey`) and is in `Active Standby`; the satellite changed nothing ("turn off normally").
- **Cleanup.** The agent's own refresh token was deleted over the websocket by its own id (46 tokens listed, one with the current flag; the token was rejected with 401 afterwards); no other token touched. Token file shredded. The two snapshots and the part 1 snapshot stay in the gitignored `devices/private/c3-b/EXP-013/`.

## Conclusion and confidence

Verdicts (c3-b, `OLED55C36LC`, firmware `03.30.73`, satellite 0.4.2, Home Assistant 2026.10.4, 2026-11-05):

| # | Verdict | Label |
| --- | --- | --- |
| H1 | Holds: deleting the entry while HA is connected cleared the key on the TV within 1 s (`key` `cleared`, window closed, mic closed, daemon idle) | verified-on-target |
| H2 | Not exercised (HA could connect at the delete) | not tested |
| H3 | Holds: no key typed (inferred from the TV log), key set over the zero-key Noise session, HA connected with the new key 5 s later | verified-on-target (key typing inferred, not reported by the owner) |
| H4 | Holds: the same 38 entity ids, including the 13 with the area name | verified-on-target |
| H5 | Holds: same device id, area Living Room, name | verified-on-target |
| H6 | Holds: Assist exposure (10) and aliases (9) came back unchanged | verified-on-target |
| H7 | Not exercised (the new entities existed before the delete) | hypothesis |
| H8 | Not tested | INCONCLUSIVE |

Restoring was not needed: the dry run was empty and `restore --apply` was not run. Not answered: the owner's view of the TV confirm texts and the phone page (only "works like a charm"), what Home Assistant proposed for the area, whether the key is new by hash. Confidence: high for H1, H3 to H6 on this unit and this Home Assistant version; the 30-day limit of the restore is documentation, not tested.

## Rollback and cleanup

- **The new entry fails** (cannot connect, wrong key, no entities): on the TV, Home Assistant, **Reset pairing** (or `ctl.sh reset-pairing`, which opens the window), delete any half-made entry in Home Assistant, and repeat O1 to O4 or add by host and port directly. The daemon, the config and the boot hook are not touched by any of this.
- **Home Assistant side is wrong after the add:** `restore --apply` from the snapshot (A7); it never deletes anything and never creates areas or labels. Entities that exist in Home Assistant and not in the snapshot are left alone.
- **TV software:** 0.4.1 stays (the owner approved the same updates as c3-a). To go back: install the 0.2.3 ipk and `ctl.sh install`; `etc/` is kept.
- **Cleanup:** end the phone session; delete the agent's own refresh token over the websocket and shred the token file; leave `devices/private/c3-b/EXP-013/` (it holds the MAC, keep it private). No recordings exist. No LG setting was changed, the TV power state is left as found.

## The restore tool

[`tools/ha-restore/ha_restore.py`](../../tools/ha-restore/ha_restore.py), with `python3 -m unittest discover -s tools -p 'test_ha_restore.py'` (planning logic, matching, the Home Assistant messages, the token-file mode, and that the tool holds no address or MAC). It needs `aiohttp` (already a requirement of the test tools) for the network commands only.

- `snapshot --ha-url URL --token-file FILE --device-name SUBSTR --out DIR`: read-only. Writes `DIR/snapshot.json` (mode 0600): the device (name, user name, area, labels, config entry ids, connections) and every entity of it (entity id, unique id, name, aliases, area, labels, icon, disabled and hidden state, options, Assist exposure, what refers to it by `search/related`, and which storage dashboards mention it). Prints counts only.
- `restore --ha-url URL --token-file FILE --device-name SUBSTR --snapshot FILE [--apply]`: **dry run by default.** Finds the device now, matches entities by unique id then entity id, and prints the plan: `device_update`, `entity_rename`, `entity_update`, `expose`, `options`, and the notes `missing` (no such entity now), `conflict` (the wanted entity id belongs to another entity: not written) and `skipped` (the area or label does not exist now: not invented). With `--apply` it sends only the writes, one call each, and reports failures; it never deletes and never creates anything.
- The token file must not be readable by others (the tool refuses otherwise) and its content is never printed.

## Follow-ups / unresolved questions

- H4 to H6 held and are now noted in [phone setup section 10](../design/phone-setup.md#10-add-to-home-assistant); the TV text still does not say that a re-add keeps the customisations for 30 days after the delete (`ORPHANED_ENTITY_KEEP_SECONDS`, documented, not tested beyond minutes). Decide whether to add it to `HA_CLEANUP`.
- Home Assistant played a test sound on the TV right after the add (see Observations, unplanned item 1). Confirm with the owner and, if it is HA's setup dialog, mention it in the phone page and the install guide (it is audible on a household TV; the reply volume was also raised to 79 %).
- The same test on c3-a is not planned (it is the owner's PC display).
- A test of the zero-delay case where the TV is **off** at deletion (H2) is covered here only if A4 shows `paired`.
