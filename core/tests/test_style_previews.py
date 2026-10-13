"""Whole-crossing specimens retain the journey and freeze at the seam."""

import unittest

from core import effects

STYLES = tuple(dict.fromkeys((*effects.CLASSIC, *effects.EFFECT_IDS)))
PLACES = ("edge", "corner", "notch")
PALETTE = ("#49b59c", "#db8d4b")


def marks(scene):
    return [[op[:2] + ((op[2].kind, op[2].geometry, op[2].stops)
                       if isinstance(op[2], effects.Gradient) else op[2],) + op[3:]
             for op in pen.ops] for pen in scene["pens"]]


class StylePreviewTests(unittest.TestCase):
    def test_the_shipped_still_table_is_what_the_search_finds(self):
        for style in STYLES:
            for place, index in effects._PLACE_INDEX.items():
                with self.subTest(style=style, place=place):
                    effects._search_still.cache_clear()
                    self.assertEqual(effects.STILL_SEARCHED[style][index], effects._search_still(style, place))

    def test_a_length_or_size_step_builds_no_scene(self):
        from unittest import mock
        with mock.patch.object(effects, "preview_still_scene", side_effect=AssertionError("scene built")):
            for style in STYLES:
                for place in (*PLACES, "switch"):
                    for pace, fraction in ((0.37, 0.021), (1.0, None), (2.5, 0.079)):
                        at = effects.still_time(style, place, pace=pace, effect_size_fraction=fraction)
                        start = effects._S_START if place == "switch" else effects._P_CROSS
                        searched = effects.STILL_SEARCHED[style][effects._PLACE_INDEX[place]]
                        self.assertAlmostEqual(at, searched if searched < start else start + (searched - start) * pace)

    def test_a_still_is_the_same_picture_at_every_length(self):
        for style in STYLES:
            for place in (*PLACES, "switch"):
                with self.subTest(style=style, place=place):
                    fx = effects.preview_effect(style)
                    stills = [effects.preview_still_scene(fx, effects.still_time(style, place, pace=pace), place,
                                                          PALETTE, pace=pace) for pace in (0.25, 1.0, 2.5)]
                    def rounded(value):
                        if isinstance(value, float):
                            return round(value, 6)
                        if isinstance(value, (list, tuple)):
                            return type(value)(rounded(item) for item in value)
                        return value
                    for scene in stills[1:]:
                        self.assertEqual(rounded(scene["pointer"]), rounded(stills[0]["pointer"]))
                        self.assertEqual(rounded(marks(scene)), rounded(marks(stills[0])))

    def test_hover_begins_at_approach_after_intent_and_reduced_motion_holds_still(self):
        held = effects.still_time("beam", "edge")
        self.assertEqual(effects.preview_time(held), held)
        self.assertEqual(effects.preview_time(held, -0.05), held)
        self.assertEqual(effects.preview_time(held, 0.0), 0.0)
        self.assertEqual(effects.preview_time(held, 0.2), 0.2)
        self.assertEqual(effects.preview_time(held, 0.2, reduced=True), held)
        self.assertEqual(effects.preview_time(held, 0.2, switch=True), held + 0.2)


    def test_camera_contains_pointer_for_the_entire_crossing(self):
        for style in STYLES:
            for place in PLACES:
                with self.subTest(style=style, place=place):
                    fx = effects.preview_effect(style)
                    view = effects.still_view(fx, place)
                    x, y, width, height = view
                    for step in range(89):
                        scene = effects.preview_still_scene(fx, step * 0.05, place, PALETTE, live=True)
                        px, py, _os = scene["pointer"]
                        self.assertLessEqual(x, px)
                        self.assertLessEqual(px, x + width - 20)
                        self.assertLessEqual(y, py)
                        self.assertLessEqual(py, y + height - 20)
                        self.assertEqual(len(scene["screens"]), 2)


    def test_still_is_a_real_seam_frame_with_pointer(self):
        for style in STYLES:
            for place in PLACES:
                for pace in (0.25, 1.0, 2.0):
                    with self.subTest(style=style, place=place, pace=pace):
                        fx = effects.preview_effect(style)
                        at = effects.still_time(fx, place, pace=pace)
                        self.assertGreaterEqual(at, effects._P_CROSS - 0.3)
                        # The same frame of the effect at every Length: after the crossing it comes later.
                        self.assertLessEqual(at, effects._P_CROSS + 0.3 * max(1.0, pace))
                        scene = effects.preview_still_scene(fx, at, place, PALETTE, pace=pace)
                        self.assertIsNotNone(scene["pointer"])
                        self.assertEqual(len(scene["screens"]), 2)
                        if place != "notch" or style not in ("glow", "beam"):
                            self.assertTrue(scene["pens"])
                        frozen = effects.preview_still_scene(fx, 3.9, place, PALETTE, reduced=True, live=True, pace=pace)
                        self.assertEqual(frozen["pointer"], scene["pointer"])
                        self.assertEqual(marks(frozen), marks(scene))


    def test_whole_journey_and_repeat(self):
        for place in PLACES:
            with self.subTest(place=place):
                fx = effects.preview_effect("aperture")
                scenes = [effects.preview_still_scene(fx, at, place, PALETTE, live=True)
                          for at in (0, 0.4, 2.1, 2.2, 3.4, effects.PREVIEW_LOOP_S)]
                self.assertEqual([scene["pointer"][2] for scene in scenes], ["mac", "mac", "mac", "windows", "windows", "mac"])
                self.assertNotEqual(scenes[0]["pointer"], scenes[1]["pointer"])
                self.assertNotEqual(scenes[3]["pointer"], scenes[4]["pointer"])
                self.assertEqual(scenes[0]["pointer"], scenes[-1]["pointer"])
                self.assertTrue(scenes[2]["pens"] and scenes[3]["pens"])


    def test_notch_contact_and_arrival_pointers_are_clear_of_the_hardware_cutout(self):
        for notch in ((185, 32), (220, 48), (150, 24)):
            with self.subTest(notch=notch):
                fx = effects.preview_effect("aperture")
                contact = effects.preview_still_scene(fx, 1.9, "notch", PALETTE, notch_geometry=notch)
                arrival = effects.preview_still_scene(fx, 2.2, "notch", PALETTE, live=True, notch_geometry=notch)
                _x, top, _w, height, _r = contact["notch"]
                self.assertGreater(contact["pointer"][1], top + height)
                self.assertLess(arrival["pointer"][1] + 20, top)
                self.assertEqual(contact["notch"][2:4], notch)
