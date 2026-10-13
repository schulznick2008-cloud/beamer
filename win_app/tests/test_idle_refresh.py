"""Idle refresh must not read unchanged files or replace an unchanged picker."""
import os
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from PySide6.QtWidgets import QApplication
import app_config
import kvm_bridge_win as kvm
import theme
from core import protocol
from core.return_edge import Rect
from core.tests import responder_harness


class IdleRefreshTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QApplication.instance() or QApplication([])
        theme.init_fonts()

    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.path = Path(self.folder.name) / "idle-fixture.json"
        saved = app_config.load_settings(self.path)
        saved["check_updates"] = False
        app_config.write_settings(self.path, saved)
        self.blockers = [
            patch.object(threading.Thread, "start", side_effect=AssertionError("service started")),
            patch.object(kvm.WindowsApplication, "_make_pairing", return_value=None),
            patch.object(kvm.autostart_win, "installed_exe", return_value=None),
        ]
        for blocker in self.blockers:
            blocker.start()
            self.addCleanup(blocker.stop)
        self.window = kvm.WindowsApplication(self.path)
        self.window.refresh_timer.stop()
        self.window.full_screen_timer.stop()
        self.window.show()
        self.app.processEvents()
        for _ in range(8):
            self.window._refresh_window()

    def tearDown(self):
        self.window.hide()
        self.window.deleteLater()
        self.app.processEvents()

    def test_unchanged_ticks_do_not_read_settings_or_replace_model(self):
        model = self.window.design_follow_choice.model()
        with patch.object(app_config, "load_settings", side_effect=AssertionError("idle read")):
            for _ in range(8):
                self.window._refresh_window()
        self.assertIs(self.window.design_follow_choice.model(), model)

    def test_real_file_change_is_visible_on_next_tick(self):
        saved = app_config.load_settings(self.path)
        saved["same_before_sync"] = {"saved_at": int(time.time()), "groups": ["crossing"],
                                     "values": {"resistance_px": 80}}
        app_config.write_settings(self.path, saved)
        self.window._refresh_window()
        self.assertFalse(self.window.same_revert.isHidden())

    def test_hidden_ticks_keep_tray_live_without_refreshing_window(self):
        self.window.hide()
        model = self.window.design_follow_choice.model()
        self.window.sender.crossing_paused = True
        with patch.object(app_config, "load_settings", side_effect=AssertionError("idle read")):
            for _ in range(8):
                self.window.refresh_timer.timeout.emit()
        self.assertEqual(self.window.pause_action.text(), "Resume crossing")
        self.assertIs(self.window.design_follow_choice.model(), model)

    def test_design_model_changes_when_a_link_or_name_changes(self):
        peer = protocol.id_text(responder_harness.B)
        saved = app_config.load_settings(self.path)
        saved["peers"] = [responder_harness.entry(responder_harness.B, "Peer", send=False)]
        app_config.write_settings(self.path, saved)
        self.window._design_peer_caps = lambda: {peer: {"design_sync", "layout"}}
        self.window._refresh_window()
        choice = self.window.design_follow_choice
        self.assertEqual(choice.count(), 2)
        self.assertEqual(choice.itemText(1), "Peer")
        self.assertFalse(choice.model().item(1).isEnabled())
        self.window._design_states[peer] = {}
        self.window._refresh_window()
        self.assertTrue(choice.model().item(1).isEnabled())
        saved["peers"][0]["name"] = "Renamed peer"
        app_config.write_settings(self.path, saved)
        self.window._refresh_window()
        self.assertEqual(choice.itemText(1), "Renamed peer")
        self.window._design_peer_caps = lambda: {}
        self.window._refresh_window()
        self.assertEqual(choice.count(), 1)

    def test_a_temporary_read_failure_retries_and_keeps_last_good_snapshot(self):
        saved = app_config.load_settings(self.path)
        saved["same_before_sync"] = {"saved_at": int(time.time()), "groups": ["crossing"],
                                     "values": {"resistance_px": 80}}
        app_config.write_settings(self.path, saved)
        with patch.object(app_config, "load_settings", side_effect=OSError("temporary failure")):
            self.window._refresh_window()
        self.assertTrue(self.window.same_revert.isHidden())
        self.window._refresh_window()
        self.assertFalse(self.window.same_revert.isHidden())

    def test_column_fit_does_not_set_an_unchanged_width(self):
        fitter = self.window.findChildren(kvm.ColumnFit)[0]
        fitter.fit()
        with patch.object(fitter.column, "setFixedWidth", wraps=fitter.column.setFixedWidth) as setter:
            fitter.fit()
            setter.assert_not_called()
            fitter.scroll.resize(fitter.scroll.width() + 200, fitter.scroll.height())
            fitter.fit()
            setter.assert_called_once()

    def test_pointer_selection_selects_once(self):
        sidebar = self.window.sidebar
        with patch.object(sidebar, "select", wraps=sidebar.select) as select:
            sidebar._pointer_select("design")
            self.assertEqual(select.call_count, 1)
        self.assertEqual(self.window._page, "design")

    def test_unchanged_peer_row_skips_setters_but_changed_row_is_shown(self):
        saved = app_config.load_settings(self.path)
        saved["peers"] = [responder_harness.entry(responder_harness.B, "Peer", send=False)]
        app_config.write_settings(self.path, saved)
        self.window._refresh_window()
        row = next(iter(self.window.machines.rows.values()))
        with patch.object(row.left_out, "setText", wraps=row.left_out.setText) as setter:
            self.window._refresh_window()
            setter.assert_not_called()
            self.window._peer_entries[0]["left_out"] = True
            self.window._refresh_window()
            self.assertEqual(row.left_out.text(), "Put back")
            setter.assert_called_once()

    def test_hidden_file_change_is_reflected_when_shown(self):
        self.window.hide()
        saved = app_config.load_settings(self.path)
        saved["same_before_sync"] = {"saved_at": int(time.time()), "groups": ["crossing"],
                                     "values": {"resistance_px": 80}}
        app_config.write_settings(self.path, saved)
        self.window._refresh_window()
        self.window.show()
        self.assertFalse(self.window.same_revert.isHidden())

    def test_show_refreshes_all_state_before_processing_paint_events(self):
        self.window.hide()
        saved = app_config.load_settings(self.path)
        peer = protocol.id_text(responder_harness.B)
        saved["peers"] = [responder_harness.entry(responder_harness.B, "New peer", send=False)]
        app_config.write_settings(self.path, saved)
        self.window._paired = True
        self.window._config.same_groups = ["design"]
        self.window._config.same_on_both = True
        self.window._status = kvm.ServerState.CONNECTED
        self.window._status_detail = "New connection"
        self.window._sending_detail = "Connected to the new peer"
        self.window.sender.crossing_paused = True
        self.window._design_peer_caps = lambda: {peer: {"design_sync", "layout"}}
        self.window._design_states[peer] = {}
        self.window.show()
        self.assertEqual(self.window.status_heading.text(), kvm.STATUS_TITLES[kvm.ServerState.CONNECTED])
        self.assertEqual(self.window.status_detail.text(), "New connection")
        self.assertEqual(self.window.send_hint.text(), "Connected to the new peer")
        self.assertEqual(self.window.pause_button.text(), "Resume crossing")
        self.assertTrue(self.window.same_checks["design"].isChecked())
        self.assertTrue(self.window.same_checks["everything"].isEnabled())
        self.assertEqual(self.window.design_follow_choice.itemText(1), "New peer")
        self.assertEqual(next(iter(self.window.machines.rows.values())).name.text(), "New peer")

    def test_display_geometry_change_is_reconciled_without_a_file_edit(self):
        self.window._own_display_map = {"probe": Rect(0, 0, 1920, 1080)}
        self.window._refresh_window()
        saved = app_config.load_settings(self.path)
        self.assertEqual(saved["layout"][saved["machine_id"]]["displays"]["probe"],
                         {"x": 0, "y": 0, "w": 1920, "h": 1080})
        self.window._own_display_map = {"probe": Rect(0, 0, 2560, 1440)}
        self.window._refresh_window()
        saved = app_config.load_settings(self.path)
        self.assertEqual(saved["layout"][saved["machine_id"]]["displays"]["probe"]["w"], 2560)

    def test_display_signal_refreshes_hardware_before_reconciling(self):
        def changed_geometry():
            self.window._own_display_map = {"probe": Rect(0, 0, 2560, 1440)}

        with patch.object(self.window, "_refresh_own_displays", side_effect=changed_geometry) as refresh:
            self.app.primaryScreen().geometryChanged.emit(self.app.primaryScreen().geometry())
        refresh.assert_called_once()
        saved = app_config.load_settings(self.path)
        self.assertEqual(saved["layout"][saved["machine_id"]]["displays"]["probe"]["w"], 2560)

    def test_restore_refreshes_state_without_waiting_for_a_tick(self):
        self.window.showMinimized()
        self.window._sending_detail = "Connected after minimising"
        self.window.showNormal()
        self.assertEqual(self.window.send_hint.text(), "Connected after minimising")
