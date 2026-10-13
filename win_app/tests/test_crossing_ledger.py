"""Section 9: ledger controls round trip through saved intent and the map."""

import copy
import unittest
from itertools import combinations

from PySide6.QtCore import QPoint, Qt
from PySide6.QtGui import QAccessible, QImage
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QApplication, QWidget

import app_config
from core import ways
import test_crossing_page_parity as parity


def picked(window, peer):
    return [third for third, button in window.crossing_ledger.rows[peer].edge_glyph.buttons.items()
            if button.isChecked()]


def band_point(canvas, third, side="right"):
    rect = canvas.tile_rect(canvas._own_id())
    fraction = {"start": 1 / 6, "middle": 1 / 2, "end": 5 / 6}[third]
    if side in ("left", "right"):
        return QPoint(round(rect.left() if side == "left" else rect.right()), round(rect.top() + rect.height() * fraction))
    return QPoint(round(rect.left() + rect.width() * fraction), round(rect.top() if side == "top" else rect.bottom()))


def paint(canvas):
    image = QImage(canvas.size(), QImage.Format.Format_ARGB32)
    image.fill(Qt.GlobalColor.transparent)
    canvas.render(image)
    return image


class CrossingLedgerTests(unittest.TestCase):
    setUp = parity.CrossingPageParityTests.setUp
    tearDown = parity.CrossingPageParityTests.tearDown
    _open_window = parity.CrossingPageParityTests._open_window
    _close_window = parity.CrossingPageParityTests._close_window
    _stop_patches = parity.CrossingPageParityTests._stop_patches

    def test_seven_thirds_combinations_round_trip_and_match_map(self):
        peer = self.peers[0]["id"]
        for size in (1, 2, 3):
            for subset in combinations(ways.PARTS, size):
                with self.subTest(subset=subset):
                    window = self.window
                    if picked(window, peer):
                        window.crossing_ledger.rows[peer].edge_text.click()
                    window.crossing_ledger.rows[peer].edge_text.click()
                    for third in ways.PARTS:
                        if third not in subset:
                            window.crossing_ledger.rows[peer].edge_glyph.buttons[third].click()
                    held = ways.ways(app_config.load_settings(self.path), peer)
                    self.assertEqual(ways.picked_thirds(held["methods"], held["parts"]), list(subset))
                    self.assertEqual(picked(window, peer), list(subset))
                    self.assertEqual(window.atlas_canvas.states[peer]["thirds"], list(subset))
                    self._close_window()
                    self._open_window()
                    self.assertEqual(picked(self.window, peer), list(subset))

    def test_last_third_off_and_click_while_off_on_ledger_and_map(self):
        peer = self.peers[0]["id"]
        for surface in ("ledger", "map"):
            with self.subTest(surface=surface):
                window = self.window
                row = window.crossing_ledger.rows[peer]
                if not picked(window, peer):
                    row.edge_text.click()
                def click(third):
                    if surface == "ledger":
                        row.edge_glyph.buttons[third].click()
                    else:
                        QTest.mouseClick(window.atlas_canvas, Qt.MouseButton.LeftButton, pos=band_point(window.atlas_canvas, third))
                for third in ways.PARTS:
                    click(third)
                self.assertEqual(picked(window, peer), [])
                self.assertEqual(row.edge_value.text(), "Off")
                self.assertFalse(window.atlas_canvas.states[peer]["crosses"])
                click("middle")
                self.assertEqual(picked(window, peer), ["middle"])
                self.assertEqual(row.edge_value.text(), "Middle third")
                row.edge_text.click()
                row.edge_text.click()
                self.assertEqual(picked(window, peer), list(ways.PARTS))

    def test_each_corner_on_and_off_round_trips(self):
        peer = self.peers[0]["id"]
        for corner in ways.CORNERS:
            for on in (True, False):
                with self.subTest(corner=corner, on=on):
                    self.window.crossing_ledger.rows[peer].corner_glyph.buttons[corner].click()
                    held = ways.ways(app_config.load_settings(self.path), peer)
                    self.assertEqual("corner" in held["methods"], on)
                    self.assertEqual(self.window.atlas_canvas.states[peer]["corner_way"], corner if on else "")
                    self._close_window()
                    self._open_window()
                    self.assertEqual(self.window.crossing_ledger.rows[peer].corner_glyph.buttons[corner].isChecked(), on)

    def test_corner_moves_refuses_held_dot_and_pads_have_both_opacities(self):
        first, second = [entry["id"] for entry in self.peers]
        rows = self.window.crossing_ledger.rows
        rows[first].corner_glyph.buttons["top_left"].click()
        rows[first].corner_glyph.buttons["bottom_right"].click()
        self.assertFalse(rows[first].corner_glyph.buttons["top_left"].isChecked())
        before = app_config.load_settings(self.path)
        self.assertFalse(rows[second].corner_glyph.buttons["bottom_right"].isEnabled())
        self.assertEqual(rows[second].corner_glyph.buttons["bottom_right"].toolTip(), "MacBook Pro's corner")
        rows[second].corner_glyph.buttons["bottom_right"].click()
        self.assertEqual(app_config.load_settings(self.path), before)
        rows[second].corner_glyph.buttons["bottom_left"].click()
        self.window._choose_machine(second)
        paint(self.window.atlas_canvas)
        self.assertEqual(set(self.window.atlas_canvas.corner_marks), {(first, "bottom_right", .55), (second, "bottom_left", 1.0)})

    def test_seam_and_track_stop_at_corner_halo(self):
        peer = self.peers[0]["id"]
        self.window._ledger_edge(peer)
        self.window._ledger_corner(peer, "top_right")
        self.window._ledger_third(peer, "start")
        canvas = self.window.atlas_canvas
        paint(canvas)
        rect = canvas.tile_rect(canvas._own_id())
        right_paths = [path for path in canvas.seam_paths if path[0] == "right"]
        self.assertTrue(right_paths)
        self.assertAlmostEqual(min(one.y() for _side, one, _two, _alpha, _width in right_paths), rect.top() + 11)

    def test_shortcut_cycle_and_jump_chord_round_trip(self):
        peer = self.peers[0]["id"]
        row = self.window.crossing_ledger.rows[peer]
        row.cycle.click()
        row.jump.click()
        QTest.keyClick(row.jump, Qt.Key.Key_F18, Qt.KeyboardModifier.AltModifier)
        saved = next(entry for entry in app_config.load_settings(self.path)["peers"] if entry["id"] == peer)
        self.assertFalse(saved["shortcut_cycle"])
        self.assertEqual(saved["jump_key"], "alt+f18")
        self._close_window()
        self._open_window()
        row = self.window.crossing_ledger.rows[peer]
        self.assertFalse(row.cycle.isChecked())
        self.assertEqual(row.jump.key.text(), "Alt F18")
        self.assertTrue(row.clear.isVisible())
        row.clear.click()
        self.assertEqual(row.jump.key.text(), "Set a key")

    def test_offline_and_left_out_rows_keep_intent_and_put_back_controls(self):
        peer = self.peers[0]["id"]
        self.window._ledger_edge(peer)
        row = self.window.crossing_ledger.rows[peer]
        self.window._live = lambda _peer: False
        self.window._reflect_ways()
        self.assertEqual(row.effect.opacity(), .5)
        self.assertEqual(row.status.text(), "macOS · Offline, ways kept")
        self.assertTrue(row.edge_text.isEnabled())
        self.window._canvas_left_out(peer, True)
        self.assertIs(row, self.window.crossing_ledger.rows[peer])
        self.assertEqual(row.status.text(), "macOS · Left out")
        self.assertEqual(row.leave.text(), "Put back")
        self.assertFalse(row.edge_text.isEnabled())
        self.assertEqual(picked(self.window, peer), list(ways.PARTS))
        row.leave.click()
        self.assertTrue(row.edge_text.isEnabled())

    def test_narrow_has_no_horizontal_scroll_and_all_controls_fit(self):
        self.window.resize(640, 700)
        parity.settle(self.window)
        ledger = self.window.crossing_ledger
        self.assertTrue(ledger.narrow)
        self.assertTrue(ledger.header.isHidden())
        scroll = self.window.stack.currentWidget()
        self.assertEqual(scroll.horizontalScrollBar().maximum(), 0)
        for row in ledger.rows.values():
            for control in (*row.cells, row.rename, row.leave, row.cycle, row.jump):
                rect = control.mapTo(ledger, QPoint())
                self.assertGreaterEqual(rect.x(), 0)
                self.assertLessEqual(rect.x() + control.width(), ledger.width())

    def test_accessibility_table_headers_and_named_stateful_controls(self):
        ledger = self.window.crossing_ledger
        self.assertEqual(QAccessible.queryAccessibleInterface(ledger).role(), QAccessible.Role.Table)
        row = ledger.rows[self.peers[0]["id"]]
        self.assertEqual(QAccessible.queryAccessibleInterface(row).role(), QAccessible.Role.Row)
        self.assertEqual(QAccessible.queryAccessibleInterface(row.machine).role(), QAccessible.Role.RowHeader)
        headers = ledger.header.findChildren(QWidget)
        self.assertEqual([w.accessibleName() for w in headers if w.property("ledger_role") == "columnheader"], ["Machine", "Edge", "Corner", "Keys"])
        for button in (*row.edge_glyph.buttons.values(), *row.corner_glyph.buttons.values()):
            self.assertIn(", off", button.accessibleName())
            self.assertEqual(QAccessible.queryAccessibleInterface(button).role(), QAccessible.Role.CheckBox)

    def test_selection_and_cell_updates_keep_all_rows_in_place(self):
        ledger = self.window.crossing_ledger
        rows = dict(ledger.rows)
        before = parity.descendants(ledger)
        for peer in rows:
            QTest.mouseClick(rows[peer].name, Qt.MouseButton.LeftButton)
            self.assertEqual(self.window.atlas_canvas.selected, peer)
            self.assertTrue(rows[peer].selected)
            rows[peer].corner_glyph.buttons["top_left"].click()
            self.assertEqual(ledger.rows, rows)
            self.assertEqual(parity.descendants(ledger).keys(), before.keys())


if __name__ == "__main__":
    unittest.main()
