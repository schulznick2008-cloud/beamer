import os
import sys
import unittest
from pathlib import Path


os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


class AtlasCanvasTests(unittest.TestCase):
    def setUp(self):
        from PySide6.QtWidgets import QApplication

        self.app = QApplication.instance() or QApplication([])

    def test_this_pc_tile_shows_platform_and_linked_status(self):
        import sys

        from diagram import AtlasCanvas

        canvas = AtlasCanvas()
        self.addCleanup(canvas.deleteLater)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC"}, {}, "home", own_id="home")

        state = canvas.states["home"]
        self.assertEqual(state["detail"], "Linux" if sys.platform.startswith("linux") else "Windows")
        self.assertTrue(state["linked"])

    def test_this_pc_tile_keeps_its_machine_name_as_the_own_tile_detail(self):
        from diagram import AtlasCanvas

        canvas = AtlasCanvas()
        self.addCleanup(canvas.deleteLater)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC"}, {"home": {"detail": "Studio PC"}}, "home", own_id="home")

        self.assertEqual(canvas.states["home"]["detail"], "Studio PC")

    def test_this_pc_name_is_painted_over_its_display_boxes(self):
        from PySide6.QtCore import Qt
        from PySide6.QtGui import QColor, QImage

        from diagram import AtlasCanvas
        import theme

        canvas = AtlasCanvas()
        self.addCleanup(canvas.deleteLater)
        canvas.resize(480, 268)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100,
                                    "displays": {"main": {"x": 0, "y": 0, "w": 1728, "h": 1117}}}},
                         {"home": "This PC"}, {"home": {"state": "this"}}, "home", own_id="home")
        image = QImage(canvas.size(), QImage.Format.Format_ARGB32)
        image.fill(Qt.GlobalColor.transparent)
        canvas.render(image)

        rect = canvas.tile_rect("home")
        ink = QColor(theme.colour("ink"))
        visible_ink = any(
            (lambda pixel: abs(pixel.red() - ink.red()) < 24 and abs(pixel.green() - ink.green()) < 24
             and abs(pixel.blue() - ink.blue()) < 24)(image.pixelColor(x, y))
            for y in range(max(0, int(rect.top()) + 6), min(image.height(), int(rect.top()) + 30))
            for x in range(max(0, int(rect.left()) + 20), min(image.width(), int(rect.right()) - 20)))
        self.assertTrue(visible_ink)

    def test_drag_compiles_layout_and_refuses_overlap_at_nearest_free_snap(self):
        from PySide6.QtCore import QPoint, Qt
        from PySide6.QtTest import QTest

        from diagram import AtlasCanvas
        from core import ways

        drops = []
        settings = {"machine_id": "home", "peers": [{"id": "peer", "side": "right"}], "zones": []}

        def compile_drop(tiles, moved):
            result = ways.compile_layout(settings, tiles, moved=moved)
            refused = result
            if not result["accepted"]:
                tiles[moved] = result["snap"][moved]
                result = ways.compile_layout(settings, tiles)
            drops.append((tiles, moved, result, refused))

        canvas = AtlasCanvas(on_drop=compile_drop)
        self.addCleanup(canvas.deleteLater)
        canvas.resize(720, 300)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"}, {"peer": {"state": "edge"}},
                         "home")
        canvas.show()
        self.app.processEvents()
        start = canvas.tile_rect("peer").center().toPoint()
        QTest.mousePress(canvas, Qt.MouseButton.LeftButton, pos=start)
        QTest.mouseMove(canvas, start + QPoint(-120, 0), 10)
        QTest.mouseRelease(canvas, Qt.MouseButton.LeftButton, pos=start + QPoint(-120, 0))
        self.assertEqual(len(drops), 1)
        self.assertEqual(drops[0][1], "peer")
        self.assertTrue(drops[0][2]["accepted"])
        self.assertFalse(drops[0][3]["accepted"])
        self.assertFalse(ways._overlaps(drops[0][0]["peer"], drops[0][0]["home"]))
        self.assertEqual(drops[0][0]["peer"], drops[0][3]["snap"]["peer"])

    def test_alt_click_splits_a_display_and_drag_moves_only_that_display(self):
        from PySide6.QtCore import QPoint, Qt
        from PySide6.QtTest import QTest

        from diagram import AtlasCanvas

        drops = []
        canvas = AtlasCanvas(on_drop=lambda tiles, moved: drops.append((tiles, moved)))
        self.addCleanup(canvas.deleteLater)
        canvas.resize(720, 300)
        canvas.set_tiles({
            "home": {"x": 0, "y": 0, "w": 160, "h": 100},
            "peer": {"x": 172, "y": 0, "w": 160, "h": 100,
                     "displays": {"left": {"x": 0, "y": 0, "w": 100, "h": 100},
                                  "right": {"x": 100, "y": 0, "w": 100, "h": 100}}},
        }, {"home": "This PC", "peer": "Studio"}, {"peer": {"state": "edge"}}, "home")
        canvas.show()
        self.app.processEvents()

        point = canvas.display_rect("peer", "right").center().toPoint()
        QTest.mouseClick(canvas, Qt.MouseButton.LeftButton, Qt.KeyboardModifier.AltModifier, pos=point)
        self.assertIn("right", canvas.tiles["peer"]["display_offsets"])
        self.assertEqual(drops[-1][1], "peer")

        before_tile = dict(canvas.tiles["peer"])
        point = canvas.display_rect("peer", "right").center().toPoint()
        QTest.mousePress(canvas, Qt.MouseButton.LeftButton, pos=point)
        QTest.mouseMove(canvas, point + QPoint(40, 0), 10)
        QTest.mouseRelease(canvas, Qt.MouseButton.LeftButton, pos=point + QPoint(40, 0))
        self.assertEqual(canvas.tiles["peer"]["x"], before_tile["x"])
        self.assertGreater(canvas.tiles["peer"]["display_offsets"]["right"]["x"], 0)

    def test_arrow_moves_focused_tile_a_third_and_announces_sentence(self):
        from PySide6.QtCore import Qt
        from PySide6.QtTest import QTest

        from diagram import AtlasCanvas

        canvas = AtlasCanvas()
        canvas.resize(720, 300)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"},
                         {"peer": {"state": "edge", "announcement": "Crosses on the right."}}, "peer")
        self.addCleanup(canvas.deleteLater)
        canvas.show()
        canvas.setFocus()
        QTest.keyClick(canvas, Qt.Key.Key_Down)
        self.assertEqual(canvas.tiles["peer"]["y"], 33)
        self.assertIn("Studio PC, right of This PC, bottom third.", canvas.accessibleDescription())

    def test_rename_cancel_and_save_actions(self):
        from PySide6.QtCore import Qt
        from PySide6.QtTest import QTest

        from diagram import AtlasCanvas

        renamed = []
        canvas = AtlasCanvas(on_rename=lambda peer, name: renamed.append((peer, name)))
        self.addCleanup(canvas.deleteLater)
        canvas.resize(720, 300)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"}, {"peer": {"state": "edge"}}, "home")
        canvas.begin_rename("peer")
        canvas.rename_edit.setText("Studio")
        QTest.keyClick(canvas.rename_edit, Qt.Key.Key_Escape)
        self.assertEqual(renamed, [])
        canvas.begin_rename("peer")
        canvas.rename_edit.setText("Studio")
        QTest.keyClick(canvas.rename_edit, Qt.Key.Key_Return)
        self.assertEqual(renamed, [("peer", "Studio")])

    def test_clearing_a_rename_restores_the_machine_name(self):
        from PySide6.QtCore import Qt
        from PySide6.QtTest import QTest

        from diagram import AtlasCanvas

        renamed = []
        canvas = AtlasCanvas(on_rename=lambda peer, name: renamed.append((peer, name)))
        self.addCleanup(canvas.deleteLater)
        canvas.resize(720, 300)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"}, {"peer": {"state": "edge"}}, "home")
        canvas.begin_rename("peer")
        canvas.rename_edit.clear()
        QTest.keyClick(canvas.rename_edit, Qt.Key.Key_Return)

        self.assertEqual(renamed, [("peer", "")])

    def test_leave_out_and_put_back_move_the_machine_between_canvas_and_shelf(self):
        from diagram import AtlasCanvas

        calls = []
        canvas = AtlasCanvas(on_leave_out=lambda peer, left: calls.append((peer, left)))
        self.addCleanup(canvas.deleteLater)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"}, {"peer": {"state": "edge"}}, "home")
        canvas.set_left_out({"peer"}, {"peer": True})
        self.assertIn("peer", canvas.shelf_ids)
        canvas.put_back("peer")
        self.assertEqual(calls, [("peer", False)])

    def test_not_in_use_machines_are_shelved_without_becoming_left_out(self):
        from diagram import AtlasCanvas

        canvas = AtlasCanvas()
        self.addCleanup(canvas.deleteLater)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"}, {"peer": {"state": "edge"}}, "peer")
        canvas.set_left_out(set(), {"peer": False})

        self.assertIn("peer", canvas.shelf_ids)
        self.assertNotIn("peer", canvas.left_out_ids)

    def test_shelf_tiles_cannot_be_hit_or_renamed_on_the_canvas(self):
        from PySide6.QtCore import Qt
        from PySide6.QtTest import QTest
        from diagram import AtlasCanvas

        canvas = AtlasCanvas()
        self.addCleanup(canvas.deleteLater)
        canvas.resize(720, 300)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 0, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Shelf"}, {}, "home", own_id="home")
        canvas.set_left_out({"peer"})
        canvas.show()
        point = canvas.tile_rect("peer").center().toPoint()
        QTest.mouseClick(canvas, Qt.MouseButton.LeftButton, pos=point)
        self.assertIsNone(canvas._drag)
        self.assertEqual(canvas.selected, "home")
        QTest.mouseDClick(canvas, Qt.MouseButton.LeftButton, pos=point)
        self.assertIsNone(canvas._rename_peer)

    def test_drag_keeps_the_map_transform_until_release(self):
        from PySide6.QtCore import QPoint, Qt
        from PySide6.QtTest import QTest
        from diagram import AtlasCanvas

        canvas = AtlasCanvas()
        self.addCleanup(canvas.deleteLater)
        canvas.resize(360, 268)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 160, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Peer"}, {}, "peer", own_id="home")
        canvas.show()
        point = canvas.tile_rect("peer").center().toPoint()
        scale, origin = canvas._scale, canvas._origin
        QTest.mousePress(canvas, Qt.MouseButton.LeftButton, pos=point)
        QTest.mouseMove(canvas, point + QPoint(80, 0))
        self.assertEqual(canvas._scale, scale)
        self.assertEqual(canvas._origin, origin)
        self.assertTrue(canvas.hasMouseTracking())

    def test_pressing_this_machine_never_changes_the_selected_peer(self):
        from PySide6.QtCore import Qt
        from PySide6.QtTest import QTest

        from diagram import AtlasCanvas

        selected = []
        canvas = AtlasCanvas(on_select=selected.append)
        self.addCleanup(canvas.deleteLater)
        canvas.resize(720, 300)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"}, {"peer": {"state": "edge"}}, "peer",
                         own_id="home")
        canvas.show()
        self.app.processEvents()
        QTest.mouseClick(canvas, Qt.MouseButton.LeftButton, pos=canvas.tile_rect("home").center().toPoint())

        self.assertEqual(canvas.selected, "peer")
        self.assertEqual(selected, [])

    def test_shortcut_mode_and_tile_cycle_ticks_persist(self):
        from diagram import AtlasCanvas
        from widgets import Choice

        ticks = []
        modes = []
        canvas = AtlasCanvas(on_cycle=lambda peer, enabled: ticks.append((peer, enabled)))
        self.addCleanup(canvas.deleteLater)
        canvas.set_tiles({"home": {"x": 0, "y": 0, "w": 160, "h": 100},
                          "peer": {"x": 172, "y": 0, "w": 160, "h": 100}},
                         {"home": "This PC", "peer": "Studio PC"}, {"peer": {"state": "edge"}}, "home",
                         cycle_mode="ticked")
        canvas.set_cycle("peer", False)
        self.assertEqual(ticks, [("peer", False)])
        choice = Choice((("last", "Last machine"), ("next", "Next in order"),
                         ("ticked", "Next among ticked")), 3, "last", on_change=modes.append)
        choice._buttons["ticked"].click()
        self.assertEqual(modes, ["ticked"])
