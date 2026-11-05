"""Self-tests of tools/ha-restore/ha_restore.py (planning logic only, no network).

Run: python3 -m unittest discover -s tools -p 'test_ha_restore.py'
"""
import copy
import json
import os
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "ha-restore"))
import ha_restore as hr  # noqa: E402


def ent(eid, uid, **over):
    e = {"entity_id": eid, "unique_id": uid, "platform": "esphome", "original_name": "x", "entity_category": None,
         "expose_conversation": False, "options": {"conversation": {"should_expose": False}},
         "name": None, "area_id": None, "aliases": [None], "labels": [], "icon": None, "disabled_by": None, "hidden_by": None}
    e.update(over)
    return e


DEVICE = {"id": "dev1", "name": "LG C3 (c3-x) Voice Satellite", "name_by_user": None, "area_id": "living_room", "labels": [],
          "connections": [["mac", "aa:bb:cc:dd:ee:ff"]], "identifiers": []}
SNAP_ENTS = [
    ent("media_player.living_room_tv", "mac/0/media_player/TV", aliases=["tele", "television"], expose_conversation=True,
        options={"conversation": {"should_expose": True}}),
    ent("button.tv_volume_up", "mac/0/button/Up", aliases=["subir"], expose_conversation=True,
        options={"conversation": {"should_expose": True}}),
    ent("sensor.listening", "mac/0/sensor/Listening"),
]
SNAP = hr.build_snapshot(DEVICE, {"living_room": "Living Room"}, SNAP_ENTS, "2026-11-05T00:00:00Z", "2026.10.4")


class Plan(unittest.TestCase):
    def plan(self, device=DEVICE, current=None, **kw):
        cur = copy.deepcopy(SNAP_ENTS if current is None else current)
        return hr.plan_restore(SNAP, copy.deepcopy(device), cur, **kw)

    def test_identical_has_nothing_to_do(self):
        self.assertEqual(self.plan(known_areas={"living_room"}), [])
        self.assertIn("Nothing to change", hr.render_plan([]))

    def test_everything_lost_after_a_fresh_add(self):
        dev = dict(DEVICE, area_id=None)
        cur = [ent("media_player.lg_c3_tv", "mac/0/media_player/TV", expose_conversation=True,
                   options={"conversation": {"should_expose": True}}),
               ent("button.lg_c3_tv_volume_up", "mac/0/button/Up"), ent("sensor.listening", "mac/0/sensor/Listening")]
        ops = self.plan(dev, cur, known_areas={"living_room"})
        kinds = [(o["kind"], o.get("entity_id")) for o in ops]
        self.assertIn(("device_update", None), [(k, None) for k, _ in kinds])
        self.assertEqual(ops[0]["fields"], {"area_id": "living_room"})
        self.assertIn(("entity_rename", "media_player.lg_c3_tv"), kinds)
        self.assertIn(("entity_rename", "button.lg_c3_tv_volume_up"), kinds)
        # after the rename the later ops use the wanted id
        upd = [o for o in ops if o["kind"] == "entity_update"]
        self.assertEqual({o["entity_id"] for o in upd}, {"media_player.living_room_tv", "button.tv_volume_up"})
        self.assertEqual([o["fields"]["aliases"] for o in upd if o["entity_id"] == "button.tv_volume_up"], [["subir"]])
        exp = [o for o in ops if o["kind"] == "expose"]
        self.assertEqual([(o["entity_id"], o["should_expose"]) for o in exp], [("button.tv_volume_up", True)])

    def test_matching_is_by_unique_id_first(self):
        cur = [ent("sensor.other_name", "mac/0/sensor/Listening")]
        ops = hr.plan_restore(SNAP, copy.deepcopy(DEVICE), cur)
        self.assertEqual([o["new_entity_id"] for o in ops if o["kind"] == "entity_rename"], ["sensor.listening"])
        self.assertEqual(sorted(o["what"] for o in ops if o["kind"] == "missing"),
                         ["button.tv_volume_up", "media_player.living_room_tv"])

    def test_wanted_id_taken_is_a_conflict_not_a_write(self):
        cur = copy.deepcopy(SNAP_ENTS)
        cur[2] = ent("sensor.renamed", "mac/0/sensor/Listening")
        cur.append(ent("sensor.listening", "someone-else"))
        ops = self.plan(current=cur)
        self.assertEqual([o["kind"] for o in ops], ["conflict"])
        self.assertIsNone(hr.op_message(ops[0]))

    def test_missing_area_and_label_are_skipped_not_invented(self):
        dev = dict(DEVICE, area_id=None)
        ops = self.plan(dev, known_areas=set())
        self.assertEqual([o["kind"] for o in ops], ["skipped"])
        snap = copy.deepcopy(SNAP)
        snap["entities"][0]["labels"] = ["a", "b"]
        ops = hr.plan_restore(snap, copy.deepcopy(DEVICE), copy.deepcopy(SNAP_ENTS), {"living_room"}, {"a"})
        self.assertEqual([o["kind"] for o in ops], ["skipped", "entity_update"])
        self.assertEqual(ops[1]["fields"], {"labels": ["a"]})

    def test_alias_order_and_null_item_are_compared_exactly(self):
        cur = copy.deepcopy(SNAP_ENTS)
        cur[0]["aliases"] = ["television", "tele"]
        ops = self.plan(current=cur)
        self.assertEqual([o["kind"] for o in ops], ["entity_update"])
        cur = copy.deepcopy(SNAP_ENTS)
        cur[2]["aliases"] = []
        self.assertEqual([o["kind"] for o in self.plan(current=cur)], ["entity_update"])

    def test_no_device(self):
        ops = hr.plan_restore(SNAP, None, [])
        self.assertEqual(ops[0]["kind"], "missing")

    def test_other_option_domains_are_restored_but_exposure_goes_through_expose(self):
        snap = copy.deepcopy(SNAP)
        snap["entities"][2]["options"]["sensor"] = {"display_precision": 1}
        ops = hr.plan_restore(snap, copy.deepcopy(DEVICE), copy.deepcopy(SNAP_ENTS), {"living_room"})
        self.assertEqual([(o["kind"], o["options_domain"]) for o in ops], [("options", "sensor")])
        msg = hr.op_message(ops[0])
        self.assertEqual(msg["type"], "config/entity_registry/update")
        self.assertEqual(msg["options"], {"display_precision": 1})


class Messages(unittest.TestCase):
    def test_messages(self):
        self.assertEqual(hr.op_message({"kind": "expose", "entity_id": "a.b", "should_expose": True}),
                         {"type": "homeassistant/expose_entity", "assistants": ["conversation"], "entity_ids": ["a.b"], "should_expose": True})
        self.assertEqual(hr.op_message({"kind": "entity_rename", "entity_id": "a.b", "new_entity_id": "a.c"}),
                         {"type": "config/entity_registry/update", "entity_id": "a.b", "new_entity_id": "a.c"})
        m = hr.op_message({"kind": "device_update", "device_id": "d", "fields": {"area_id": "x", "labels": ["l"]}})
        self.assertEqual(m, {"type": "config/device_registry/update", "device_id": "d", "area_id": "x", "labels": ["l"]})
        self.assertIsNone(hr.op_message({"kind": "missing"}))

    def test_render_counts_writes_and_notes(self):
        out = hr.render_plan([{"kind": "expose", "entity_id": "a.b", "should_expose": True, "why": "w"},
                              {"kind": "missing", "what": "c.d", "why": "w"}])
        self.assertIn("1 write(s), 1 note(s).", out)


class Snapshot(unittest.TestCase):
    def test_entity_record_reads_exposure_from_the_exposure_list_then_options(self):
        reg = {"entity_id": "a.b", "unique_id": "u", "platform": "esphome", "options": {"conversation": {"should_expose": False}},
               "aliases": [None], "labels": []}
        self.assertTrue(hr.entity_record(reg, {"a.b": {"conversation": True}})["expose_conversation"])
        self.assertFalse(hr.entity_record(reg, {})["expose_conversation"])

    def test_summary_is_counts_only(self):
        s = hr.summarize(SNAP)
        self.assertTrue(all(isinstance(v, (int, bool)) for v in s.values()))
        self.assertEqual((s["entities"], s["exposed"], s["with_aliases"]), (3, 2, 2))
        self.assertNotIn("mac", json.dumps(s))

    def test_find_devices(self):
        devs = [DEVICE, {"id": "o", "name": "Other", "connections": []}]
        self.assertEqual([d["id"] for d in hr.find_devices(devs, "C3-X")], ["dev1"])
        self.assertEqual([d["id"] for d in hr.find_devices(devs, "AA:BB")], ["dev1"])

    def test_token_file_must_be_private(self):
        with tempfile.NamedTemporaryFile("w", delete=False) as f:
            f.write("t\n")
        try:
            os.chmod(f.name, 0o644)
            with self.assertRaises(SystemExit):
                hr.read_token(f.name)
            os.chmod(f.name, 0o600)
            self.assertEqual(hr.read_token(f.name), "t")
        finally:
            os.unlink(f.name)

    def test_the_tool_holds_no_secrets_or_identifiers(self):
        with open(hr.__file__) as f:
            src = f.read()
        self.assertNotIn("hass.grigri", src)
        self.assertIsNone(__import__("re").search(r"[0-9a-f]{2}(:[0-9a-f]{2}){5}", src, __import__("re").I))


if __name__ == "__main__":
    unittest.main()
