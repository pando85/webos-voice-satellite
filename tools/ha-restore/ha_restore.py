#!/usr/bin/env python3
"""Snapshot a satellite's Home Assistant customisations, and re-apply them.

Why: a fresh "add to Home Assistant" (EXP-013) deletes the ESPHome entry. Home
Assistant 2026.10.4 keeps removed registry entries for 30 days and restores them
when the same device (same MAC, so same unique ids) is added again, so the
customisations should come back by themselves. This tool is the check and the
fallback: `snapshot` records them before, `restore` compares them with what is
there after and, with --apply, writes only the differences.

    snapshot  read-only. Device (name, area, labels), every entity (registry
              fields, aliases, options, Assist exposure) and what refers to it
              (HA's search/related, dashboards). Writes snapshot.json into
              --out. The file holds the device MAC (it is in the unique ids):
              keep it in the gitignored private folder, never in the repo.
    restore   dry-run by default: prints the plan (what differs). --apply
              sends it. Matching is by unique id, then by entity id. Never
              deletes anything; never creates areas or labels.

Home Assistant calls used (names checked against core 2026.10.4):
    config/device_registry/list, config/device_registry/update,
    config/entity_registry/list, config/entity_registry/get,
    config/entity_registry/update, config/area_registry/list,
    homeassistant/expose_entity/list, homeassistant/expose_entity,
    search/related, lovelace/dashboards/list, lovelace/config.

The access token is read from a file (mode 0600) and never printed. Requires
aiohttp for the network commands only; the planning functions are stdlib.

Usage:
    python3 -I tools/ha-restore/ha_restore.py snapshot --ha-url https://ha.example \\
        --token-file FILE --device-name c3-b --out devices/private/c3-b/EXP-013
    python3 -I tools/ha-restore/ha_restore.py restore --ha-url https://ha.example \\
        --token-file FILE --device-name c3-b --snapshot devices/private/c3-b/EXP-013/snapshot.json [--apply]
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time

SCHEMA = 1
ASSISTANT = "conversation"
# Entity registry fields the restore compares and writes. `aliases` keeps Home
# Assistant's own form (a null item means "also the entity's name").
ENTITY_FIELDS = ("name", "area_id", "aliases", "labels", "icon", "disabled_by", "hidden_by")
DEVICE_FIELDS = ("area_id", "name_by_user", "labels")


# ---- pure logic (tested without a network) ---------------------------------------


def domain_of(entity_id: str) -> str:
    return entity_id.split(".", 1)[0]


def _norm(field: str, value):
    """Comparable form of a registry field."""
    if field == "labels":
        return sorted(value or [])
    if field == "aliases":
        return list(value or [])  # order is kept; null items are kept
    return value


def find_devices(devices: list[dict], needle: str) -> list[dict]:
    """Devices whose name, user name, model or MAC contains `needle` (case-insensitive)."""
    n = needle.lower()
    out = []
    for d in devices:
        hay = [d.get("name") or "", d.get("name_by_user") or ""]
        hay += [str(c[1]) for c in d.get("connections") or []]
        if any(n in h.lower() for h in hay):
            out.append(d)
    return out


def entity_record(reg: dict, exposed: dict, related: dict | None = None) -> dict:
    """One entity as the snapshot stores it. `reg` is a config/entity_registry/get result."""
    eid = reg["entity_id"]
    opts = reg.get("options") or {}
    expose = (exposed.get(eid) or {}).get(ASSISTANT)
    if expose is None:
        expose = (opts.get(ASSISTANT) or {}).get("should_expose")
    rec = {
        "entity_id": eid,
        "unique_id": reg.get("unique_id"),
        "platform": reg.get("platform"),
        "original_name": reg.get("original_name"),
        "entity_category": reg.get("entity_category"),
        "expose_conversation": expose,
        "options": {k: v for k, v in opts.items()},
    }
    for f in ENTITY_FIELDS:
        rec[f] = reg.get(f)
    if related is not None:
        rec["referenced_by"] = related
    return rec


def build_snapshot(device: dict, area_names: dict, entities: list[dict], taken_at: str, ha_version: str = "",
                   dashboards: dict | None = None) -> dict:
    return {
        "schema": SCHEMA,
        "taken_at": taken_at,
        "ha_version": ha_version,
        "device": {
            "id": device.get("id"),
            "name": device.get("name"),
            "name_by_user": device.get("name_by_user"),
            "area_id": device.get("area_id"),
            "area_name": area_names.get(device.get("area_id")),
            "labels": sorted(device.get("labels") or []),
            "disabled_by": device.get("disabled_by"),
            "connections": device.get("connections") or [],
            "identifiers": device.get("identifiers") or [],
            "config_entries": device.get("config_entries") or [],
            "manufacturer": device.get("manufacturer"),
            "model": device.get("model"),
            "sw_version": device.get("sw_version"),
        },
        "entities": sorted(entities, key=lambda e: e["entity_id"]),
        "dashboards": dashboards or {},
    }


def summarize(snap: dict) -> dict:
    """Counts only (safe to print and to put in a report)."""
    ents = snap["entities"]
    refs = [e for e in ents if any((e.get("referenced_by") or {}).values())]
    return {
        "device_area": bool(snap["device"].get("area_id")),
        "entities": len(ents),
        "exposed": sum(1 for e in ents if e.get("expose_conversation") is True),
        "not_exposed": sum(1 for e in ents if e.get("expose_conversation") is False),
        "with_aliases": sum(1 for e in ents if [a for a in e.get("aliases") or [] if a]),
        "with_own_name": sum(1 for e in ents if e.get("name")),
        "with_area": sum(1 for e in ents if e.get("area_id")),
        "with_labels": sum(1 for e in ents if e.get("labels")),
        "disabled": sum(1 for e in ents if e.get("disabled_by")),
        "hidden": sum(1 for e in ents if e.get("hidden_by")),
        "with_options_other_than_exposure": sum(1 for e in ents if any(k != ASSISTANT for k in e.get("options") or {})),
        "referenced_by_other_config": len(refs),
        "dashboards_mentioning": len(snap.get("dashboards") or {}),
    }


def match_entities(snap_entities: list[dict], current: list[dict]) -> tuple[list[tuple[dict, dict]], list[dict]]:
    """Pairs (snapshot entity, current entity) by unique id, then by entity id; and the unmatched snapshot ones."""
    by_uid = {e["unique_id"]: e for e in current if e.get("unique_id")}
    by_eid = {e["entity_id"]: e for e in current}
    used: set[str] = set()
    pairs, missing = [], []
    for s in snap_entities:
        c = by_uid.get(s.get("unique_id")) or by_eid.get(s["entity_id"])
        if c is None or c["entity_id"] in used:
            missing.append(s)
            continue
        used.add(c["entity_id"])
        pairs.append((s, c))
    return pairs, missing


def plan_restore(snap: dict, current_device: dict | None, current: list[dict], known_areas: set | None = None,
                 known_labels: set | None = None) -> list[dict]:
    """The writes that bring the current device and entities back to the snapshot.

    Each op is a dict with `kind` (device_update, entity_rename, entity_update,
    expose, options, or the report-only notes: missing, conflict, skipped) and
    a `why`. Home Assistant calls are made by `op_message`.
    """
    ops: list[dict] = []
    if current_device is None:
        return [{"kind": "missing", "what": "device", "why": "no matching device in Home Assistant"}]
    sd = snap["device"]
    fields = {}
    for f in DEVICE_FIELDS:
        if _norm(f, sd.get(f)) != _norm(f, current_device.get(f)):
            fields[f] = sd.get(f)
    if "area_id" in fields and fields["area_id"] is not None and known_areas is not None and fields["area_id"] not in known_areas:
        ops.append({"kind": "skipped", "what": "device area", "why": "the area %r does not exist now" % fields["area_id"]})
        del fields["area_id"]
    if fields:
        ops.append({"kind": "device_update", "device_id": current_device["id"], "fields": fields,
                    "why": "differs: " + ", ".join(sorted(fields))})
    pairs, missing = match_entities(snap["entities"], current)
    taken = {c["entity_id"] for _, c in pairs}
    all_current = {e["entity_id"] for e in current}
    for s in missing:
        ops.append({"kind": "missing", "what": s["entity_id"], "why": "no entity with this unique id or entity id now"})
    for s, c in pairs:
        eid = c["entity_id"]
        if s["entity_id"] != eid:
            if s["entity_id"] in all_current or s["entity_id"] in taken:
                ops.append({"kind": "conflict", "what": eid, "why": "wanted id %s is used by another entity" % s["entity_id"]})
            else:
                ops.append({"kind": "entity_rename", "entity_id": eid, "new_entity_id": s["entity_id"],
                            "why": "entity id differs"})
                eid = s["entity_id"]
        ef = {}
        for f in ENTITY_FIELDS:
            if _norm(f, s.get(f)) != _norm(f, c.get(f)):
                ef[f] = s.get(f)
        if "area_id" in ef and ef["area_id"] is not None and known_areas is not None and ef["area_id"] not in known_areas:
            ops.append({"kind": "skipped", "what": eid + " area", "why": "the area %r does not exist now" % ef["area_id"]})
            del ef["area_id"]
        if "labels" in ef and known_labels is not None:
            gone = [x for x in ef["labels"] or [] if x not in known_labels]
            if gone:
                ops.append({"kind": "skipped", "what": eid + " labels", "why": "labels missing now: %d" % len(gone)})
                ef["labels"] = [x for x in ef["labels"] if x in known_labels]
        if ef:
            ops.append({"kind": "entity_update", "entity_id": eid, "fields": ef, "why": "differs: " + ", ".join(sorted(ef))})
        # Assist exposure
        if s.get("expose_conversation") is not None and s.get("expose_conversation") != c.get("expose_conversation"):
            ops.append({"kind": "expose", "entity_id": eid, "should_expose": s["expose_conversation"],
                        "why": "exposure %s -> %s" % (c.get("expose_conversation"), s["expose_conversation"])})
        # Options of other domains (display precision and the like)
        so, co = s.get("options") or {}, c.get("options") or {}
        for dom in sorted(set(so) | set(co)):
            if dom == ASSISTANT:
                continue
            if so.get(dom) != co.get(dom) and dom in so:
                ops.append({"kind": "options", "entity_id": eid, "options_domain": dom, "options": so[dom],
                            "why": "options %s differ" % dom})
    return ops


def op_message(op: dict) -> dict | None:
    """The Home Assistant websocket message (without `id`) for a write op; None for report-only ops."""
    k = op["kind"]
    if k == "device_update":
        return {"type": "config/device_registry/update", "device_id": op["device_id"], **op["fields"],
                **({"labels": list(op["fields"]["labels"])} if "labels" in op["fields"] else {})}
    if k == "entity_rename":
        return {"type": "config/entity_registry/update", "entity_id": op["entity_id"], "new_entity_id": op["new_entity_id"]}
    if k == "entity_update":
        return {"type": "config/entity_registry/update", "entity_id": op["entity_id"], **op["fields"]}
    if k == "expose":
        return {"type": "homeassistant/expose_entity", "assistants": [ASSISTANT], "entity_ids": [op["entity_id"]],
                "should_expose": op["should_expose"]}
    if k == "options":
        return {"type": "config/entity_registry/update", "entity_id": op["entity_id"],
                "options_domain": op["options_domain"], "options": op["options"]}
    return None


def render_plan(ops: list[dict]) -> str:
    if not ops:
        return "Nothing to change: the device and its entities match the snapshot."
    lines = []
    for op in ops:
        what = op.get("entity_id") or op.get("what") or op.get("device_id") or ""
        extra = ""
        if op["kind"] in ("entity_update", "device_update"):
            extra = " " + json.dumps(op["fields"], ensure_ascii=False, sort_keys=True)
        elif op["kind"] == "entity_rename":
            extra = " -> " + op["new_entity_id"]
        elif op["kind"] == "expose":
            extra = " should_expose=%s" % op["should_expose"]
        lines.append("%-13s %s%s  (%s)" % (op["kind"], what, extra, op["why"]))
    writes = sum(1 for o in ops if op_message(o))
    lines.append("%d write(s), %d note(s)." % (writes, len(ops) - writes))
    return "\n".join(lines)


# ---- Home Assistant websocket client (network) ------------------------------------


class Ha:
    def __init__(self, url: str, token: str) -> None:
        self.url = url.rstrip("/").replace("https://", "wss://").replace("http://", "ws://") + "/api/websocket"
        self.token = token
        self._n = 0

    async def __aenter__(self):
        import aiohttp  # lazy: the planning code and its tests need no network library

        self._session = aiohttp.ClientSession()
        self._ws = await self._session.ws_connect(self.url, max_msg_size=0)
        await self._ws.receive_json()
        await self._ws.send_json({"type": "auth", "access_token": self.token})
        r = await self._ws.receive_json()
        if r.get("type") != "auth_ok":
            raise SystemExit("Home Assistant refused the token")
        self.version = r.get("ha_version", "")
        return self

    async def __aexit__(self, *a):
        await self._ws.close()
        await self._session.close()

    async def call(self, msg: dict, tolerate: bool = False):
        self._n += 1
        await self._ws.send_json({"id": self._n, **msg})
        while True:
            m = await self._ws.receive_json()
            if m.get("id") == self._n:
                if m.get("success"):
                    return m.get("result")
                if tolerate:
                    return None
                raise RuntimeError("%s failed: %s" % (msg["type"], (m.get("error") or {}).get("code", "error")))


async def read_state(ha: Ha, device: dict | None, with_references: bool):
    """(device, entities) as the snapshot stores them, from the live registries."""
    exposed = (await ha.call({"type": "homeassistant/expose_entity/list"}))["exposed_entities"]
    regs = await ha.call({"type": "config/entity_registry/list"})
    mine = [e for e in regs if device and e.get("device_id") == device["id"]]
    entities = []
    for e in sorted(mine, key=lambda x: x["entity_id"]):
        full = await ha.call({"type": "config/entity_registry/get", "entity_id": e["entity_id"]})
        rel = None
        if with_references:
            r = await ha.call({"type": "search/related", "item_type": "entity", "item_id": e["entity_id"]}, tolerate=True)
            rel = {k: sorted(v) for k, v in (r or {}).items() if k in ("automation", "script", "scene", "group") and v}
        entities.append(entity_record(full, exposed, rel))
    return entities


async def dashboards_mentioning(ha: Ha, entity_ids: list[str]) -> dict:
    """Which storage-mode dashboards mention any of the entity ids: {url_path or '': count}."""
    out = {}
    lst = await ha.call({"type": "lovelace/dashboards/list"}, tolerate=True) or []
    for url_path in [None] + [d.get("url_path") for d in lst]:
        msg = {"type": "lovelace/config"}
        if url_path:
            msg["url_path"] = url_path
        cfg = await ha.call(msg, tolerate=True)
        if cfg is None:
            continue
        text = json.dumps(cfg)
        n = sum(1 for e in entity_ids if e in text)
        if n:
            out[url_path or ""] = n
    return out


def read_token(path: str) -> str:
    st = os.stat(path)
    if st.st_mode & 0o077:
        raise SystemExit("token file %s must not be readable by others (chmod 600)" % path)
    with open(path) as f:
        return f.read().strip()


async def find_device(ha: Ha, args) -> dict | None:
    devs = await ha.call({"type": "config/device_registry/list"})
    if getattr(args, "device_id", None):
        return next((d for d in devs if d["id"] == args.device_id), None)
    hits = find_devices(devs, args.device_name)
    if len(hits) > 1:
        raise SystemExit("%d devices match %r; narrow --device-name or pass --device-id" % (len(hits), args.device_name))
    return hits[0] if hits else None


async def cmd_snapshot(args) -> int:
    async with Ha(args.ha_url, read_token(args.token_file)) as ha:
        dev = await find_device(ha, args)
        if dev is None:
            print("no device matches", file=sys.stderr)
            return 2
        ents = await read_state(ha, dev, with_references=True)
        areas = await ha.call({"type": "config/area_registry/list"})
        names = {a["area_id"]: a["name"] for a in areas}
        dash = await dashboards_mentioning(ha, [e["entity_id"] for e in ents])
        snap = build_snapshot(dev, names, ents, time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), ha.version, dash)
    os.makedirs(args.out, mode=0o700, exist_ok=True)
    path = os.path.join(args.out, "snapshot.json")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(snap, f, indent=2, ensure_ascii=False, sort_keys=True)
    print("wrote %s (mode 0600)" % path)
    print(json.dumps(summarize(snap), sort_keys=True))
    return 0


async def cmd_restore(args) -> int:
    with open(args.snapshot) as f:
        snap = json.load(f)
    if snap.get("schema") != SCHEMA:
        raise SystemExit("unknown snapshot schema")
    async with Ha(args.ha_url, read_token(args.token_file)) as ha:
        dev = await find_device(ha, args)
        ents = await read_state(ha, dev, with_references=False) if dev else []
        areas = {a["area_id"] for a in await ha.call({"type": "config/area_registry/list"})}
        labels = {x["label_id"] for x in (await ha.call({"type": "config/label_registry/list"}, tolerate=True) or [])}
        ops = plan_restore(snap, dev, ents, areas, labels)
        print(render_plan(ops))
        writes = [o for o in ops if op_message(o)]
        if not args.apply:
            print("Dry run: nothing was written. Add --apply to send the %d write(s)." % len(writes))
            return 0
        failed = 0
        for o in writes:
            try:
                await ha.call(op_message(o))
                print("applied   %s" % (o.get("entity_id") or o.get("device_id")))
            except RuntimeError as e:
                failed += 1
                print("FAILED    %s: %s" % (o.get("entity_id") or o.get("device_id"), e))
        return 1 if failed else 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("snapshot", "restore"):
        s = sub.add_parser(name)
        s.add_argument("--ha-url", required=True)
        s.add_argument("--token-file", required=True)
        s.add_argument("--device-name", default="c3-b", help="substring of the device name, user name or MAC")
        s.add_argument("--device-id")
        if name == "snapshot":
            s.add_argument("--out", required=True, help="private folder, for example devices/private/c3-b/EXP-013")
        else:
            s.add_argument("--snapshot", required=True)
            s.add_argument("--apply", action="store_true", help="send the writes (default: dry run)")
    args = p.parse_args(argv)
    return asyncio.run(cmd_snapshot(args) if args.cmd == "snapshot" else cmd_restore(args))


if __name__ == "__main__":
    sys.exit(main())
