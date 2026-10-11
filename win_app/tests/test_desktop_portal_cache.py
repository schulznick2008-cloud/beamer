"""Wayland input readers consume Qt geometry captured by the GUI thread."""

import threading
import unittest
from unittest import mock

from PySide6.QtWidgets import QApplication
from core.return_edge import Rect
import desktop_portal


class DesktopPortalCache(unittest.TestCase):
    def test_worker_readers_never_enumerate_qt_screens(self):
        app = QApplication.instance() or QApplication([])
        desktop_portal.set_zones([])
        desktop_portal.set_regions([])
        desktop_portal.refresh_qt_screens()
        expected = desktop_portal.display_map()
        results = []
        with mock.patch("PySide6.QtGui.QGuiApplication.screens", side_effect=AssertionError("worker Qt read")) as screens:
            thread = threading.Thread(target=lambda: results.append((desktop_portal.display_map(), desktop_portal.monitors())))
            thread.start()
            thread.join(2)
        screens.assert_not_called()
        self.assertEqual(results[0][0], expected)

    def test_gui_refresh_updates_geometry_and_keeps_portal_zone_precedence(self):
        app = QApplication.instance() or QApplication([])
        desktop_portal.refresh_qt_screens()
        self.assertTrue(desktop_portal.display_map())
        zone = Rect(10, 20, 300, 200)
        desktop_portal.set_zones([zone])
        self.addCleanup(desktop_portal.set_zones, [])
        self.assertEqual(desktop_portal.monitors(), [zone])


if __name__ == "__main__":
    unittest.main()
