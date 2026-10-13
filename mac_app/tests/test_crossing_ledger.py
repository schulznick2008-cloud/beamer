"""Direction A section 9: native ledger controls, round trips and accessibility."""

import copy
import itertools
from unittest import mock

import AppKit

from core import ways
import test_part_edge_thirds as thirds
from test_crossing_page_machines import BEE, SEA, Page, press, token


class LedgerTests(Page):
    layout = thirds.PartEdgeThirds.layout
    row = thirds.PartEdgeThirds.row
    picked = thirds.PartEdgeThirds.picked
    pick_thirds = thirds.PartEdgeThirds.pick_thirds
    whole_edge = thirds.PartEdgeThirds.whole_edge

    def rebuilt(self):
        window = self.build([(peer["id"], peer["token"], peer["name"],
                              {key: value for key, value in peer.items() if key not in ("id", "token", "name")})
                             for peer in self.store.current()["peers"]], self.store.current()["zones"])
        window.atlas_canvas.setFrame_(((0, 0), (480, window.atlas_canvas.HEIGHT)))
        window.atlas_canvas._draw(False)
        return window

    def test_seven_third_combinations_rebuild_to_same_cells_and_map(self):
        self.layout()
        for count in (1, 2, 3):
            for combination in itertools.combinations(ways.PARTS, count):
                with self.subTest(thirds=combination):
                    self.pick_thirds(combination)
                    self.assertEqual(self.store.current()["peers"][0]["ways"]["thirds"], list(combination))
                    self.rebuilt()
                    self.assertEqual(self.picked(), list(combination))
                    self.assertEqual(self.window.atlas_canvas.states[BEE]["picked_thirds"], list(combination))

    def test_last_ledger_segment_off_then_off_click_lights_only_that_segment(self):
        self.layout()
        self.pick_thirds(["start"])
        press(self.row().edge_glyph.hits["start"])
        self.assertEqual(self.row().edge_text, "Off")
        self.assertEqual(self.picked(), [])
        self.assertFalse(self.store.current()["peers"][0]["ways"]["edge"])
        press(self.row().edge_glyph.hits["middle"])
        self.assertEqual(self.picked(), ["middle"])
        press(self.row().edge_detail)
        self.assertEqual(self.picked(), [])
        press(self.row().edge_title)
        self.assertEqual(self.picked(), list(ways.PARTS))

    def test_every_corner_on_and_off_survives_page_rebuild(self):
        self.layout()
        for corner in ways.CORNERS:
            with self.subTest(corner=corner):
                press(self.row().corner_glyph.hits[corner])
                self.assertIn("corner", self.ways_of(BEE)["methods"])
                self.assertEqual(self.ways_of(BEE)["corner"], corner)
                self.rebuilt()
                self.assertEqual(self.row().corner_title.text, corner.replace("_", " ").capitalize())
                self.assertEqual(self.row().corner_glyph.hits[corner].accessibilityValue(), "on")
                self.assertIn((BEE, "corner", corner), self.window.atlas_canvas.mark_layers)
                press(self.row().corner_glyph.hits[corner])
                self.rebuilt()
                self.assertNotIn("corner", self.ways_of(BEE)["methods"])
                self.assertEqual(self.row().corner_title.text, "None")

    def test_second_corner_moves_and_another_machines_corner_refuses_the_click(self):
        self.layout(second=True)
        row = self.row()
        press(row.corner_glyph.hits["top_left"])
        press(row.corner_glyph.hits["bottom_right"])
        self.assertEqual(self.ways_of(BEE)["corner"], "bottom_right")
        sea = self.window.crossing_ledger.rows[SEA]
        self.assertFalse(sea.corner_glyph.hits["bottom_right"].isEnabled())
        self.assertEqual(sea.corner_glyph.hits["bottom_right"].toolTip(), "Bee's corner")
        before = copy.deepcopy(self.store.current())
        press(sea.corner_glyph.hits["bottom_right"])
        self.assertEqual(self.store.current(), before)

    def test_notch_on_and_off_rebuild_and_no_notch_is_disabled(self):
        self.layout()
        self.window.has_notch = True
        for on in (True, False):
            self.row().notch.setState_(int(on))
            press(self.row().notch)
            self.assertEqual("notch" in self.ways_of(BEE)["methods"], on)
            self.rebuilt()
            self.window.has_notch = True
            self.window._show_arrangement()
            self.assertEqual(self.row().notch.state(), int(on))
        self.row().notch.setState_(1)
        press(self.row().notch)
        self.window.has_notch = False
        self.window._show_arrangement()
        self.assertEqual(self.row().no_notch.text, "No notch")
        self.assertFalse(self.row().no_notch.view.isHidden())
        self.assertEqual(self.row().notch.accessibilityLabel(), "Notch, no notch")
        self.assertFalse(self.row().notch.isEnabled())

    def test_cycle_tick_and_jump_chord_round_trip_without_changing_ways(self):
        self.layout()
        before = self.ways_of(BEE)
        for on in (False, True):
            self.row().cycle.setState_(int(on))
            press(self.row().cycle)
            self.rebuilt()
            self.assertEqual(self.row().cycle.state(), int(on))
        import widgets
        recorder = self.row().jump
        event = mock.Mock()
        event.type.return_value = AppKit.NSEventTypeKeyDown
        event.modifierFlags.return_value = AppKit.NSEventModifierFlagCommand
        event.keyCode.return_value = next(code for code, name in widgets.SPECIAL_KEY_NAMES.items() if name == "f18")
        event_api = mock.Mock()
        event_api.addLocalMonitorForEventsMatchingMask_handler_.return_value = object()
        with mock.patch.object(widgets.AppKit, "NSEvent", event_api):
            recorder._clicked()
            self.assertIsNotNone(recorder.monitor)
            self.assertIsNone(recorder._recorded(event))
            self.assertIsNone(recorder.monitor)
        self.rebuilt()
        self.assertEqual(self.row().jump.value, "cmd+f18")
        self.assertIn("F18", self.row().jump.view.toolTip())
        self.assertEqual(self.ways_of(BEE), before)
        press(self.row().clear)
        self.assertEqual(self.row().jump.value, "")

    def test_offline_row_is_dimmed_but_ways_remain_live_and_saved(self):
        self.layout()
        self.window.windows_input.server.links.return_value = []
        self.window._show_arrangement()
        row = self.row()
        self.assertTrue(row.dimmed)
        self.assertEqual(row.status_text, "Windows · Offline, ways kept")
        self.assertEqual(row.cells["edge"].alphaValue(), .5)
        self.assertTrue(row.edge_glyph.hits["start"].isEnabled())
        press(row.edge_glyph.hits["middle"])
        self.assertEqual(self.picked(), ["start", "end"])

    def test_armed_jump_survives_a_cell_refresh_and_escape_cancels(self):
        import widgets
        self.layout()
        recorder = self.row().jump
        event_api = mock.Mock()
        event_api.addLocalMonitorForEventsMatchingMask_handler_.return_value = object()
        with mock.patch.object(widgets.AppKit, "NSEvent", event_api):
            recorder._clicked()
            prompt = recorder.key.text
            press(self.row().edge_glyph.hits["start"])
            self.assertIsNotNone(recorder.monitor)
            self.assertEqual(recorder.key.text, prompt)
            escape = mock.Mock()
            escape.type.return_value = AppKit.NSEventTypeKeyDown
            escape.modifierFlags.return_value = 0
            escape.keyCode.return_value = 53
            self.assertIsNone(recorder._recorded(escape))
            self.assertIsNone(recorder.monitor)
            self.assertEqual(recorder.value, "")

    def test_arming_another_row_cancels_the_first_without_dropping_the_guard(self):
        import widgets
        self.layout(second=True)
        ledger = self.window.crossing_ledger
        first, second = ledger.rows[BEE].jump, ledger.rows[SEA].jump
        event_api = mock.Mock()
        event_api.addLocalMonitorForEventsMatchingMask_handler_.side_effect = [object(), object()]
        with mock.patch.object(widgets.AppKit, "NSEvent", event_api):
            first._clicked()
            with mock.patch.object(self.window, "_jump_arming", wraps=self.window._jump_arming) as guard:
                second._clicked()
                self.assertIsNone(first.monitor)
                self.assertIsNotNone(second.monitor)
                self.assertTrue(self.controller.jump_recording)
                self.assertTrue(all(call.args[0] for call in guard.call_args_list))
                first.cancel()
                self.assertTrue(self.controller.jump_recording)
                second.cancel()
                self.assertFalse(self.controller.jump_recording)

    def test_in_use_false_disables_kept_glyphs_like_left_out(self):
        self.layout()
        settings = self.store.current()
        settings["peers"][0]["in_use"] = False
        self.store.save_settings(settings)
        self.window._show_arrangement()
        row = self.row()
        self.assertIn("Left out", row.status_text)
        for glyph in (row.edge_glyph, row.corner_glyph):
            for button in glyph.hits.values():
                self.assertFalse(button.isEnabled())
                self.assertEqual(button.toolTip(), "Left out: Put back to change its ways")

    def test_glyphs_sit_left_of_top_aligned_text_in_both_layouts(self):
        self.layout()
        for width in (687, 399):
            self.window.crossing_ledger.setFrameSize_((width, 700))
            row = self.row()
            for glyph, title, detail in ((row.edge_glyph, row.edge_title, row.edge_detail),
                                          (row.corner_glyph, row.corner_title.view, row.corner_detail.view)):
                self.assertGreaterEqual(title.frame().origin.x,
                                        glyph.frame().origin.x + glyph.frame().size.width)
                self.assertEqual(title.frame().origin.y, 0)
                self.assertEqual(detail.frame().origin.x, title.frame().origin.x)
                self.assertLessEqual(detail.frame().origin.y + detail.frame().size.height,
                                     row.cells["edge"].frame().size.height)

    def test_standard_width_corner_text_fits_on_one_line_and_elides_when_constrained(self):
        __import__("theme").init_fonts()
        self.layout()
        press(self.row().corner_glyph.hits["bottom_left"])
        ledger = self.window.crossing_ledger
        ledger.setFrameSize_((687, 700))
        for label in (self.row().corner_title, self.row().corner_detail):
            view = label.view
            self.assertGreaterEqual(view.frame().size.width - 4,
                                    view.attributedStringValue().size().width)
            self.assertFalse(view.cell().wraps())
            self.assertEqual(view.cell().lineBreakMode(), AppKit.NSLineBreakByTruncatingTail)
            self.assertEqual(view.toolTip(), label.text)
        ledger.setFrameSize_((620, 700))
        self.assertEqual(self.row().corner_title.text, "Bottom left")
        self.assertEqual(self.row().corner_detail.view.toolTip(), "diagonal push")

    def test_selected_machine_status_dot_is_inset_from_the_rule_at_both_widths(self):
        self.layout()
        for width in (687, 399):
            self.window.crossing_ledger.setFrameSize_((width, 700))
            row = self.row()
            self.assertTrue(row.selected)
            machine = row.cells["machine"].frame()
            self.assertGreaterEqual(machine.origin.x + 1, 12)
            self.assertGreaterEqual(machine.origin.x + row.name.view.frame().origin.x, 12)
            self.assertGreaterEqual(row.status.view.frame().origin.x, 12)

    def test_jump_idle_is_a_text_button_or_fitted_chip_and_only_armed_is_a_field(self):
        import widgets
        self.layout()
        row = self.row()
        self.window.crossing_ledger.setFrameSize_((687, 700))
        self.assertGreaterEqual(row.jump_caption.view.frame().size.width - 4,
                                row.jump_caption.view.attributedStringValue().size().width)
        self.assertEqual(row.jump.key.text, "Set a key")
        self.assertEqual(row.jump.view.layer().borderWidth(), 0)
        row.jump.on_change("cmd+f18")
        chip = row.jump.view
        self.assertEqual(chip.layer().cornerRadius(), 4)
        self.assertEqual(chip.layer().borderWidth(), 1)
        self.assertTrue(row.jump.key.mono)
        self.assertEqual(row.jump.key.size, __import__("theme").TYPE["small"])
        self.assertGreaterEqual(chip.frame().size.width, row.jump.key.view.attributedStringValue().size().width + 12)
        self.assertFalse(row.clear.isHidden())
        self.assertEqual(row.clear.frame().origin.y, chip.frame().origin.y)
        idle_height = chip.frame().size.height
        event_api = mock.Mock()
        event_api.addLocalMonitorForEventsMatchingMask_handler_.return_value = object()
        with mock.patch.object(widgets.AppKit, "NSEvent", event_api):
            row.jump._clicked()
            self.assertGreater(chip.frame().size.height, idle_height)
            self.assertFalse(row.jump.hint.view.isHidden())
            row.jump.cancel()
        self.assertEqual(chip.frame().size.height, idle_height)

    def test_narrow_block_insets_every_cell_from_the_selection_rule(self):
        self.layout(second=True)
        ledger = self.window.crossing_ledger
        ledger.setFrameSize_((399, 700))
        for row in ledger.rows.values():
            for key in ("edge", "corner", "notch", "keys"):
                self.assertGreaterEqual(row.cells[key].frame().origin.x, 10)
            self.assertGreaterEqual(row.cells["machine"].frame().origin.y, 12)
            self.assertGreaterEqual(row.cells["keys"].frame().origin.y,
                                    row.cells["edge"].frame().origin.y + row.cells["edge"].frame().size.height)
            self.assertLess(row.frame().size.height, 250)

    def test_left_out_row_keeps_intent_and_put_back_restores_controls(self):
        self.layout()
        before = self.ways_of(BEE)
        press(self.row().leave)
        row = self.row()
        self.assertTrue(row.dimmed)
        self.assertEqual(row.status_text, "Windows · Left out")
        self.assertIn("Put back", row.leave.accessibilityLabel())
        self.assertFalse(row.edge_glyph.hits["start"].isEnabled())
        self.assertFalse(row.corner_glyph.hits["top_left"].isEnabled())
        held = copy.deepcopy(self.store.current())
        press(row.edge_glyph.hits["start"])
        press(row.corner_glyph.hits["top_left"])
        self.assertEqual(self.store.current(), held)
        self.assertEqual(self.ways_of(BEE), before)
        press(row.leave)
        self.assertFalse(self.row().dimmed)
        self.assertTrue(self.row().edge_glyph.hits["start"].isEnabled())
        self.assertEqual(self.ways_of(BEE), before)

    def test_row_order_follows_map_position_and_left_out_stays_present(self):
        window = self.layout(second=True)
        self.assertEqual(window.crossing_ledger.order, [SEA, BEE])
        press(window.crossing_ledger.rows[SEA].leave)
        self.assertIn(SEA, window.crossing_ledger.order)

    def test_no_contact_disables_edge_while_corner_stays_clickable(self):
        self.layout()
        settings = self.store.current()
        settings["layout"][BEE]["x"] += 100
        self.store.save_settings(settings)
        self.window._show_arrangement()
        self.assertEqual(self.row().edge_text, "Off")
        self.assertEqual(self.row().edge_detail.attributedTitle().string(), "place it on the map")
        self.assertFalse(self.row().edge_title.isEnabled())
        before = copy.deepcopy(self.store.current())
        press(self.row().edge_glyph.hits["start"])
        self.assertEqual(self.store.current(), before)
        press(self.row().corner_glyph.hits["top_left"])
        self.assertIn("corner", self.ways_of(BEE)["methods"])

    def test_narrow_cells_and_visible_controls_fit_inside_the_ledger(self):
        self.layout()
        self.row().jump.on_change("cmd+f18")
        ledger = self.window.crossing_ledger
        ledger.setFrameSize_((440, 700))
        self.assertTrue(all(header.view.isHidden() for header in ledger.headers))
        row = self.row()
        self.assertEqual(row.cells["machine"].frame().origin.x + row.cells["machine"].frame().size.width, 440)
        self.assertEqual(row.cells["edge"].frame().origin.y, row.cells["corner"].frame().origin.y)
        self.assertEqual(row.cells["notch"].frame().origin.y, row.cells["keys"].frame().origin.y)
        for cell in row.cells.values():
            frame = cell.frame()
            self.assertGreaterEqual(frame.origin.x, 0)
            self.assertLessEqual(frame.origin.x + frame.size.width, 440)
            for control in cell.subviews():
                if not control.isHidden():
                    frame = control.frame()
                    self.assertGreaterEqual(cell.frame().origin.x + frame.origin.x, 0)
                    self.assertLessEqual(frame.origin.x + frame.size.width, cell.bounds().size.width)
                    self.assertLessEqual(cell.frame().origin.y + frame.origin.y + frame.size.height,
                                         row.frame().size.height)

    def test_window_width_selects_table_or_stacked_rows_without_sideways_scroll(self):
        from mac_app.tests.design_offscreen import settle
        from mac_app.tests.settings_layout_offscreen import measure
        self.layout(second=True)
        self.window._select_page("crossing")
        for width, narrow in ((960, False), (640, True), (1440, False)):
            with self.subTest(width=width):
                self.window.window.setFrame_display_(((100000, 100000), (width, 900)), False)
                for _ in range(4):
                    settle(self.window.window)
                table = self.window.crossing_ledger
                self.assertEqual(table.bounds().size.width < 620, narrow)
                self.assertEqual(all(header.view.isHidden() for header in table.headers), narrow)
                self.assertFalse(self.window.pages["crossing"].hasHorizontalScroller())
                self.assertEqual(measure(self.window, "crossing")["overflows"], [])
                self.assertFalse(self.window.window.isVisible())

    def test_glyph_hit_bands_reach_the_button_outside_the_screen_outline(self):
        for side in ways.EDGES:
            self.layout(side)
            self.window._select_page("crossing")
            self.window.crossing_ledger.setFrameSize_((700, 700))
            row = self.row()
            cell = row.cells["edge"]
            glyph = row.edge_glyph
            band = glyph.hits["middle"]
            frame = band.frame()
            if side in ("top", "bottom"):
                point = (frame.origin.x + frame.size.width / 2,
                         frame.origin.y + (1 if side == "top" else frame.size.height - 1))
            else:
                point = (frame.origin.x + (1 if side == "left" else frame.size.width - 1),
                         frame.origin.y + frame.size.height / 2)
            point = (glyph.frame().origin.x + point[0], glyph.frame().origin.y + point[1])
            point = (cell.frame().origin.x + point[0], cell.frame().origin.y + point[1])
            with self.subTest(side=side):
                self.assertIs(cell.hitTest_(point), band)

    def test_changes_keep_the_same_row_controls_and_do_not_hide_the_ledger(self):
        self.layout()
        row = self.row()
        controls = list(row.edge_glyph.hits.values()) + list(row.corner_glyph.hits.values())
        with mock.patch("motion.set_hidden", wraps=__import__("motion").set_hidden) as hidden:
            press(row.edge_glyph.hits["middle"])
            press(row.corner_glyph.hits["top_left"])
        self.assertIs(self.row(), row)
        self.assertEqual(controls, list(row.edge_glyph.hits.values()) + list(row.corner_glyph.hits.values()))
        self.assertFalse(self.window.crossing_ledger.isHidden())
        self.assertFalse(any(call.args[0] in (row, self.window.crossing_ledger) for call in hidden.call_args_list))

    def test_accessibility_table_rows_columns_named_segment_states_and_text_button(self):
        self.layout("top")
        ledger, row = self.window.crossing_ledger, self.row()
        self.assertEqual(ledger.accessibilityRole(), AppKit.NSAccessibilityTableRole)
        self.assertEqual(row.accessibilityRole(), AppKit.NSAccessibilityRowRole)
        self.assertEqual(ledger.accessibilityRows(), [row])
        self.assertEqual(len(ledger.accessibilityColumns()), 5)
        self.assertEqual(row.accessibilityLabel(), "Bee")
        button = row.edge_glyph.hits["start"]
        self.assertEqual(button.accessibilityRole(), AppKit.NSAccessibilityButtonRole)
        self.assertEqual(button.accessibilityLabel(), "Top edge, left third, on")
        press(button)
        self.assertEqual(button.accessibilityLabel(), "Top edge, left third, off")
        corner = row.corner_glyph.hits["top_left"]
        self.assertEqual(corner.accessibilityLabel(), "Top left corner, off")
        press(corner)
        self.assertEqual(corner.accessibilityLabel(), "Top left corner, on")
        self.whole_edge()
        self.assertEqual(row.edge_title.accessibilityLabel(), "Edge, whole top edge")
        press(row.edge_title)
        self.assertEqual(row.edge_title.accessibilityLabel(), "Edge, off")

    def test_inline_rename_cancel_and_save_use_the_same_row(self):
        self.layout()
        row = self.row()
        press(row.rename)
        self.assertTrue(row.renaming)
        self.assertEqual(row.rename_hint.text, "Return saves, Escape cancels")
        row.name_field.setStringValue_("Cancelled")
        row.finish_rename(False)
        self.assertEqual(row.name.text, "Bee")
        press(row.rename)
        row.name_field.setStringValue_("Studio PC")
        row.finish_rename(True)
        self.assertIs(self.row(), row)
        self.assertEqual(row.name.text, "Studio PC")

    def test_inline_rename_scope_stays_inside_machine_cell_before_the_next_block(self):
        self.layout()
        self.controller.cfg.same_on_both = False
        row = self.row()
        press(row.rename)
        for width in (399, 687):
            self.window.crossing_ledger.setFrameSize_((width, 700))
            self.assertFalse(row.scope.view.isHidden())
            scope = row.scope.view.frame()
            self.assertLessEqual(scope.origin.y + scope.size.height, row.cells["machine"].frame().size.height)
