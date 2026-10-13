"""The saved picture and local intent, exercised without either desktop toolkit."""

import copy
import unittest
from types import SimpleNamespace

from core import owner, protocol, return_edge, ways
from core.tests.crossing_harness import Hand, Screen
from core.tests.responder_harness import ident


OWN, A, B = [protocol.id_text(ident(seed)) for seed in (90, 60, 120)]
ALL = ["start", "middle", "end"]
DISPLAYS = {"0": return_edge.Rect(0, 0, 1728, 1117)}


def tile(x=0, y=0, w=160, h=100, displays=None):
    result = dict(x=x, y=y, w=w, h=h)
    if displays:
        result["displays"] = displays
    return result


def settings():
    return {"machine_id": OWN, "peers": [{"id": A, "port": 24820, "side": "right"}],
            "zones": [], "layout": {OWN: tile(), A: tile(160)},
            "layout_set_at": 1, "layout_by": OWN}


class CrossingStateTests(unittest.TestCase):
    def state(self):
        from core import crossing_state
        return crossing_state

    def args(self):
        return dict(platform="mac", live=lambda _: True, gone=lambda _: False, now=100)

    def test_paired_defaults_and_own_displays_are_pure(self):
        held = settings()
        held["layout"] = {}
        original = copy.deepcopy(held)
        out = self.state().paired(held, platform="mac", own_displays=DISPLAYS, now=100)
        self.assertEqual(held, original)
        self.assertEqual(out["settings"]["peers"][0]["ways"],
                         dict(edge=True, thirds=ALL, corner=False, corner_at="top_left", notch=False))
        self.assertEqual(out["settings"]["layout"][A]["x"], 160)
        self.assertEqual(out["settings"]["layout"][OWN]["displays"]["0"]["w"], 1728)

    def test_edit_remembers_thirds_off_and_drop_cannot_revive_them(self):
        out = self.state().edit_ways(settings(), A, thirds=["start"], edge=False, **self.args())
        held = out["settings"]
        self.assertEqual(ways.ways(held, A)["parts"], ["start"])
        self.assertEqual(ways.ways(held, A)["methods"], [])
        dropped = self.state().drop(held, held["layout"], A, capable={OWN, A}, snap_radius=12, **self.args())
        self.assertTrue(dropped["accepted"])
        self.assertFalse(ways.has_way(dropped["settings"], A))
        enabled = self.state().edit_ways(dropped["settings"], A, edge=True, thirds=[], **self.args())
        self.assertEqual(ways.ways(enabled["settings"], A)["parts"], ["start"])
        self.assertEqual(enabled["tell"], [A])

    def test_arrangement_from_picture_peer_never_moves_or_echoes(self):
        held = self.state().paired(settings(), platform="mac", own_displays=DISPLAYS, now=100)["settings"]
        picture = copy.deepcopy(held["layout"])
        read = dict(edge="top", set_at=200, by=ident(60), way_back=False, way_back_by=ident(120))
        out = self.state().receive_arrangement(held, A, read, capable={OWN, A}, **self.args())
        self.assertEqual(out["settings"]["layout"], picture)
        self.assertEqual(out["settings"]["peers"][0]["side"], "right")
        self.assertFalse(out["settings"]["peers"][0]["way_back"])
        self.assertEqual(out["settings"]["peers"][0]["way_back_by"], B)
        self.assertEqual(out["tell"], [])
        self.assertFalse(out["send_picture"])
        again = self.state().receive_arrangement(out["settings"], A, read, capable={OWN, A}, **self.args())
        self.assertFalse(again["changed"])

    def test_old_peer_moves_once_and_answers_once(self):
        held = settings()
        read = dict(edge="top", set_at=20, by=ident(60), way_back=True)
        out = self.state().receive_arrangement(held, A, read, capable={OWN}, **self.args())
        self.assertEqual(out["settings"]["peers"][0]["side"], "bottom")
        self.assertEqual(out["settings"]["layout"][A]["y"], 100)
        self.assertIn(A, out["tell"])
        self.assertFalse(self.state().receive_arrangement(out["settings"], A, read,
                                                         capable={OWN}, **self.args())["changed"])

    def test_receive_merges_owner_displays_and_corrects_own_once(self):
        held = self.state().paired(settings(), platform="mac", own_displays=DISPLAYS, now=100)["settings"]
        peer_displays = {"0": dict(x=0, y=0, w=3840, h=2160)}
        incoming = copy.deepcopy(held["layout"])
        incoming[OWN]["displays"] = {"wrong": dict(x=0, y=0, w=10, h=10)}
        incoming[A]["displays"] = peer_displays
        state = dict(set_at=200, by=A, tiles=incoming)
        out = self.state().receive_picture(held, state, source=A, capable={OWN, A},
                                          own_displays=DISPLAYS, **self.args())
        self.assertTrue(out["send_picture"])
        self.assertTrue(out["forward_picture"])
        self.assertEqual(out["settings"]["layout"][A]["displays"], peer_displays)
        self.assertEqual(out["settings"]["layout"][OWN]["displays"]["0"]["w"], 1728)
        self.assertGreater(out["settings"]["layout_set_at"], 200)
        stale = self.state().receive_picture(out["settings"], state, source=A, capable={OWN, A},
                                            own_displays=DISPLAYS, **self.args())
        self.assertFalse(stale["changed"])
        self.assertFalse(stale["forward_picture"])
        state = dict(set_at=300, by=B, tiles={OWN: tile(), A: tile(160)})
        merged = self.state().receive_picture(out["settings"], state, source=B, capable={OWN, A, B},
                                             own_displays=DISPLAYS, **self.args())
        self.assertEqual(merged["settings"]["layout"][A]["displays"], peer_displays)
        self.assertTrue(merged["send_picture"], "a picture missing our geometry needs the owner correction")

    def test_link_and_display_changes_stamp_only_geometry_changes(self):
        out = self.state().link_up(settings(), A, platform="mac", own_displays=DISPLAYS, now=100)
        self.assertEqual(out["tell"], [A])
        self.assertTrue(out["send_picture"])
        held = out["settings"]
        out = self.state().displays_changed(held, platform="mac", own_displays=DISPLAYS, now=200)
        self.assertFalse(out["changed"])
        self.assertFalse(out["send_picture"])
        self.assertEqual(out["settings"]["layout_set_at"], held["layout_set_at"])

    def test_displays_changed_persists_a_missing_zoned_peer_tile_on_its_side(self):
        held = settings()
        held["layout"] = {OWN: tile(displays={"0": {"x": 0, "y": 0, "w": 1728, "h": 1117}})}
        held["layout_set_at"] = 20

        out = self.state().displays_changed(held, platform="mac", own_displays=DISPLAYS, now=100,
                                            capable={OWN, A})

        self.assertEqual(out["settings"]["layout"][A], tile(160))
        self.assertEqual(out["settings"]["peers"][0]["side"], "right")
        self.assertEqual(out["settings"]["layout_set_at"], 20)
        self.assertFalse(out["send_picture"])

    def test_displays_changed_gives_an_old_peer_no_tile_and_keeps_its_side(self):
        # An old peer's tile comes only from an arriving arrangement (crossing model, "Old peers"); a
        # filled one made the Mac count it as a picture machine until its link said otherwise.
        held = settings()
        held["layout"] = {OWN: tile(displays={"0": {"x": 0, "y": 0, "w": 1728, "h": 1117}})}
        held["layout_set_at"] = 20

        out = self.state().displays_changed(held, platform="mac", own_displays=DISPLAYS, now=100,
                                            capable={OWN})

        self.assertNotIn(A, out["settings"]["layout"])
        self.assertEqual(out["settings"]["peers"][0]["side"], "right")
        self.assertEqual(out["settings"]["layout_set_at"], 20)
        self.assertFalse(out["send_picture"])

    def test_a_picture_omitting_two_peers_places_the_tileless_one_clear_of_the_kept_one(self):
        held = settings()
        held["peers"].append({"id": B, "port": 24820, "side": "right"})
        state = dict(set_at=200, by=A, tiles={OWN: tile()})
        held["layout"] = {OWN: tile(), A: tile(160)}
        held["peers"][0]["side"] = "right"
        out = self.state().receive_picture(held, state, source=A, capable={OWN, A, B},
                                          own_displays={}, **self.args())
        layout = out["settings"]["layout"]
        self.assertEqual(layout[A], tile(160))
        self.assertFalse(ways._overlaps(layout[A], layout[B]))

    def test_an_old_peers_arrangement_shares_a_side_another_machine_holds(self):
        held = settings()
        held["peers"][0]["side"] = "left"
        held["peers"].append({"id": B, "port": 24820, "side": "right"})
        held["layout"] = {OWN: tile(), A: tile(-160)}
        out = self.state().receive_arrangement(held, B, dict(edge="right", by=ident(120), set_at=50),
                                              capable={OWN}, **self.args())
        layout, sides = out["settings"]["layout"], {p["id"]: p["side"] for p in out["settings"]["peers"]}
        self.assertEqual((layout[A]["x"], layout[A]["y"], layout[B]["x"], layout[B]["y"]), (-160, -50, -160, 50))
        self.assertEqual((sides[A], sides[B]), ("left", "left"))

    def test_put_back_moves_the_returning_tile_off_an_active_one(self):
        held = settings()
        held["peers"][0]["left_out"] = True
        held["peers"].append({"id": B, "port": 24820, "side": "right"})
        held["layout"] = {OWN: tile(), A: tile(160), B: tile(160)}
        out = self.state().put_back(held, A, platform="mac", live=lambda _: True, gone=lambda _: False, now=100)
        layout, sides = out["settings"]["layout"], {p["id"]: p["side"] for p in out["settings"]["peers"]}
        self.assertEqual((layout[B]["y"], layout[A]["y"], layout[A]["x"]), (-50, 50, 160))
        self.assertEqual((sides[A], sides[B]), ("right", "right"))
        self.assertEqual(out["settings"]["layout_set_at"], 100)
        self.assertTrue(out["send_picture"])

    def test_put_back_of_a_machine_still_not_in_use_moves_nobody(self):
        held = settings()
        held["peers"][0].update(left_out=True, in_use=False)
        held["peers"].append({"id": B, "port": 24820, "side": "right"})
        held["layout"] = {OWN: tile(), A: tile(160), B: tile(160)}
        out = self.state().put_back(held, A, platform="mac", live=lambda _: True, gone=lambda _: False, now=100)
        self.assertEqual(out["settings"]["layout"][B], tile(160))
        self.assertFalse(out["send_picture"])

    def test_share_side_refuses_to_break_a_moved_tiles_other_contact(self):
        layout = {OWN: tile(), A: tile(-160), "C": tile(-320, 60), B: tile(500)}
        self.assertEqual(ways.share_side(layout, OWN, B, "left"), {})

    def test_a_picture_omitting_a_tileless_peer_places_it_against_the_arriving_layout(self):
        held = settings()
        held["layout"] = {OWN: tile()}
        state = dict(set_at=200, by=A, tiles={OWN: tile(160)})
        out = self.state().receive_picture(held, state, source=A, capable={OWN, A},
                                          own_displays={}, **self.args())
        layout = out["settings"]["layout"]
        self.assertEqual((layout[OWN]["x"], layout[A]["x"], layout[A]["y"]), (160, 320, 0))
        self.assertEqual(out["settings"]["peers"][0]["side"], "right")

    def test_relay_applies_owner_displays_in_a_newer_picture(self):
        held = self.state().paired(settings(), platform="mac", own_displays=DISPLAYS, now=100)["settings"]
        held["layout"][A]["displays"] = {"0": dict(x=0, y=0, w=3840, h=2160)}
        incoming = copy.deepcopy(held["layout"])
        incoming[A]["displays"]["0"]["w"] = 100
        state = dict(set_at=200, by=B, tiles=incoming)
        out = self.state().receive_picture(held, state, source=B, capable={OWN, A, B},
                                          own_displays=DISPLAYS, **self.args())
        self.assertEqual(out["settings"]["layout"][A]["displays"]["0"]["w"], 100)
        state["by"] = A
        out = self.state().receive_picture(held, state, source=B, capable={OWN, A, B},
                                          own_displays=DISPLAYS, **self.args())
        self.assertEqual(out["settings"]["layout"][A]["displays"]["0"]["w"], 100)

    def test_old_peer_keeps_arrangement_side_through_page_and_display_events(self):
        held = settings()
        held["peers"][0]["side"] = "left"
        out = self.state().edit_ways(held, A, thirds=["start"], capable={OWN}, **self.args())
        self.assertEqual(out["settings"]["peers"][0]["side"], "left")
        self.assertEqual(out["settings"]["zones"], [dict(peer=A, kind="part", parts=["start"])])
        out = self.state().displays_changed(out["settings"], platform="mac", own_displays=DISPLAYS,
                                            capable={OWN}, now=100)
        self.assertEqual(out["settings"]["peers"][0]["side"], "left")
        self.assertEqual(out["settings"]["zones"], [dict(peer=A, kind="part", parts=["start"])])

    def test_shelf_preserves_intent_and_shelved_drop_is_rejected(self):
        held = self.state().edit_ways(settings(), A, thirds=["end"], **self.args())["settings"]
        out = self.state().leave_out(held, A, **self.args())
        self.assertFalse(ways.has_way(out["settings"], A))
        self.assertEqual(out["settings"]["peers"][0]["ways"], held["peers"][0]["ways"])
        dropped = self.state().drop(out["settings"], held["layout"], A, capable={OWN, A},
                                    snap_radius=12, **self.args())
        self.assertFalse(dropped["accepted"])
        self.assertEqual(dropped["settings"], out["settings"])
        restored = self.state().put_back(out["settings"], A, **self.args())
        self.assertTrue(ways.has_way(restored["settings"], A))
        self.assertEqual(ways.ways(restored["settings"], A)["parts"], ["end"])

    def test_majority_and_tie_have_one_owner_before_intent_narrows(self):
        held = settings()
        held["peers"].append(dict(id=B, port=24820))
        held["layout"] = {OWN: tile(h=120), A: tile(160, -60, h=120), B: tile(160, 60, h=120)}
        out = ways.compile_layout(held, held["layout"], platform="mac", capable={OWN, A, B}, now=100)
        zones = out["settings"]["zones"]
        self.assertEqual([(z["peer"], z.get("parts")) for z in zones], [(A, ["start"]), (B, ["middle", "end"])])
        held["layout"][B]["y"] = 61
        out = ways.compile_layout(held, held["layout"], platform="mac", capable={OWN, A, B}, now=100)
        self.assertEqual(out["settings"]["zones"][0]["parts"], ["start", "middle"])

    def test_a_shifted_contact_only_crosses_its_own_contacting_thirds(self):
        held = settings()
        held["layout"][A]["y"] = -50
        on_upper = copy.deepcopy(held)
        on_upper["machine_id"] = A
        on_upper["peers"] = [dict(id=OWN, port=24820)]
        out = ways.compile_layout(on_upper, on_upper["layout"], platform="pc", now=100)
        self.assertEqual(out["settings"]["zones"], [
            dict(peer=OWN, kind="part", side="left", parts=["middle", "end"])])

    def test_peer_spanning_two_display_tops_compiles_both_contacts(self):
        held = settings()
        held["layout"][OWN]["displays"] = {
            "1": dict(x=0, y=0, w=1728, h=1117),
            "2": dict(x=1728, y=0, w=2560, h=1440)}
        held["layout"][A].update(x=0, y=-100)
        out = ways.compile_layout(held, held["layout"], platform="mac", now=100)
        self.assertEqual([(zone["display"], zone["side"]) for zone in out["settings"]["zones"]],
                         [("1", "top"), ("2", "top")])

    def test_invalid_drop_is_rejected_without_changing_held_settings(self):
        held = settings()
        original = copy.deepcopy(held)
        bad = copy.deepcopy(held["layout"])
        bad[A]["w"] = 0
        out = self.state().drop(held, bad, A, capable={OWN, A}, snap_radius=12, **self.args())
        self.assertFalse(out["accepted"])
        self.assertFalse(out["changed"])
        self.assertEqual(held, original)
        self.assertIsNot(out["settings"], held)

    def test_corners_and_notch_use_first_peer_and_notch_is_mac_only(self):
        held = settings()
        held["peers"].append(dict(id=B, port=24820))
        for peer in held["peers"]:
            peer["ways"] = dict(edge=False, thirds=ALL, corner=True, corner_at="top_left", notch=True)
        held["layout"][B] = tile(-160)
        out = ways.compile_layout(held, held["layout"], platform="mac", now=100)
        self.assertEqual([(z["peer"], z["kind"]) for z in out["settings"]["zones"]],
                         [(A, "corner"), (A, "notch")])
        out = ways.compile_layout(held, held["layout"], platform="pc", now=100)
        self.assertEqual([(z["peer"], z["kind"]) for z in out["settings"]["zones"]], [(A, "corner")])

    def test_old_peer_thirds_give_priority_to_picture_contacts(self):
        held = settings()
        held["peers"].insert(0, dict(id=B, port=24820, side="right", caps=[]))
        held["peers"][1]["caps"] = ["layout"]
        held["layout"][A] = tile(160, 0, h=33)
        held["layout"][B] = tile(400)
        out = ways.compile_layout(held, held["layout"], platform="pc", now=100)
        self.assertEqual(out["settings"]["zones"], [
            dict(peer=B, kind="part", parts=["middle", "end"]),
            dict(peer=A, kind="part", parts=["start"], side="right")])

    def test_target_fields_are_written_together_and_side_is_derived(self):
        held = settings()
        held["layout"][OWN]["displays"] = {"0": dict(x=0, y=0, w=100, h=100)}
        out = ways.compile_layout(held, held["layout"], platform="mac", now=100)
        zone = out["settings"]["zones"][0]
        self.assertNotIn("target_side", zone)
        held["layout"][A]["displays"] = {"other": dict(x=0, y=0, w=100, h=100)}
        held["peers"][0]["side"] = "left"
        out = ways.compile_layout(held, held["layout"], platform="mac", now=100)
        self.assertEqual(out["settings"]["zones"][0]["target_display"], "other")
        self.assertEqual(out["settings"]["zones"][0]["target_side"], "left")
        self.assertEqual(out["settings"]["peers"][0]["side"], "right")

    def test_harness_carries_out_the_owners_home_landing(self):
        screen = Screen({"1": return_edge.Rect(0, 0, 100, 100),
                         "2": return_edge.Rect(100, 0, 200, 100)})
        hand = Hand(SimpleNamespace(own=ident(90), screen=screen))
        hand.carry_out([owner.Moved(None, A, "switch", "bottom", 0.5,
                                   display="2", side="top")])
        self.assertEqual(screen.placed[-1], (200, 0))

    def test_rc12_migration_reads_once_and_removes_old_marks(self):
        own, rig = "TnY4DcwqoBkkPubq9P00QA", "GRMYVCd7dKksFjy5FejD4Q"
        held = {"machine_id": own, "peers": [dict(id=rig, side="left", port=24820)], "zones": [
            dict(peer=rig, kind="edge", contact_parts=ALL),
            dict(peer=rig, kind="part", parts=ALL, off=True, contact_parts=ALL),
            dict(peer=rig, kind="corner", corner="top_left", edge="left", off=True),
            dict(peer=rig, kind="edge", display="0", side="right", target_side="left", off=True),
            dict(peer=rig, kind="notch"),
            dict(peer=rig, kind="edge", display="0", side="bottom", target_side="top", off=True)],
            "layout": {own: tile(-169, 241, displays={"0": dict(x=0, y=0, w=1728, h=1117)}),
                       rig: tile(-329, 241, displays={"0": dict(x=0, y=0, w=3840, h=2160)})}}
        out = self.state().migrate(held, "mac")
        saved = out["settings"]
        self.assertEqual(saved["peers"][0]["ways"], dict(edge=True, thirds=ALL, corner=False,
                                                        corner_at="top_left", notch=True))
        self.assertEqual(saved["zones"], [dict(peer=rig, kind="edge", display="0", side="left",
                                               target_display="0", target_side="right"),
                                          dict(peer=rig, kind="notch")])
        self.assertFalse(self.state().migrate(saved, "mac")["changed"])
        held["zones"][0].update(off=True, page_off=True, aside=True)
        held["zones"][1]["page_parts"] = ["end"]
        saved = self.state().migrate(held, "mac")["settings"]
        self.assertFalse(saved["peers"][0]["ways"]["edge"])
        self.assertEqual(saved["peers"][0]["ways"]["thirds"], ["end"])
        self.assertEqual(saved["zones"], [dict(peer=rig, kind="notch")])

    def test_migration_keeps_a_left_out_machines_ways_for_put_back(self):
        # Codex review, 10-10: rc12 kept the kinds in use in left_out_ways and turned every zone off.
        held = settings()
        held["peers"][0].update(left_out=True, left_out_ways=["edge", "corner", "notch"])
        held["zones"] = [dict(peer=A, kind="edge", off=True), dict(peer=A, kind="corner", corner="top_right",
                                                                   edge="right", off=True),
                         dict(peer=A, kind="notch", off=True)]
        saved = self.state().migrate(held, "mac")["settings"]
        self.assertEqual(saved["peers"][0]["ways"], dict(edge=True, thirds=ALL, corner=True,
                                                        corner_at="top_right", notch=True))
        self.assertEqual(saved["zones"], [])
        back = self.state().put_back(saved, A, platform="mac", now=100)["settings"]
        self.assertIn("edge", [zone["kind"] for zone in back["zones"]])
        self.assertIn("notch", [zone["kind"] for zone in back["zones"]])

    def test_migration_keeps_a_pick_that_touches_no_contact_on(self):
        # Codex review, 10-10: rc12 marked a picked third with no contact `unpicked`, still on.
        held = settings()
        held["zones"] = [dict(peer=A, kind="edge", off=True),
                         dict(peer=A, kind="part", parts=["start"], off=True, unpicked=True, page_parts=["start"])]
        saved = self.state().migrate(held, "mac")["settings"]
        self.assertEqual(saved["peers"][0]["ways"]["edge"], True)
        self.assertEqual(saved["peers"][0]["ways"]["thirds"], ["start"])

    def test_dragging_an_old_peer_sets_its_side_and_tells_it(self):
        # Codex review, 10-10: a 1.5.1 peer crosses by its side; the map is how its side is set.
        held = settings()
        held["peers"][0]["side_set_at"] = 50
        tiles = dict(held["layout"], **{A: tile(-160)})
        out = self.state().drop(held, tiles, A, platform="mac", capable={OWN}, live=lambda _: True,
                                gone=lambda _: False, now=100, snap_radius=12)
        self.assertTrue(out["accepted"])
        self.assertEqual(out["settings"]["peers"][0]["side"], "left")
        self.assertIn(A, out["tell"])
        self.assertTrue(ways.has_way(out["settings"], A))

    def test_migration_keeps_a_whole_edge_in_use_over_a_pick_left_on_an_off_part_zone(self):
        # AGY review, 10-10: the page showed the whole edge, so migration must not shrink it to the pick.
        held = settings()
        held["zones"] = [dict(peer=A, kind="edge"), dict(peer=A, kind="part", parts=["start"], off=True,
                                                         page_parts=["start"])]
        saved = self.state().migrate(held, "mac")["settings"]
        self.assertEqual(saved["peers"][0]["ways"]["thirds"], ALL)
        self.assertTrue(saved["peers"][0]["ways"]["edge"])


if __name__ == "__main__":
    unittest.main()
