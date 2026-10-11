import json
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import effect_overlay_x11
from PySide6.QtCore import QPointF, QRect


class FakeStdin:
    def __init__(self):
        self.lines = []

    def write(self, line):
        self.lines.append(line)

    def flush(self):
        pass

    def close(self):
        pass


class FakeProcess:
    def __init__(self, returncode=None):
        self.stdin = FakeStdin()
        self.stdout = None
        self.returncode = returncode

    def poll(self):
        return self.returncode

    def wait(self, timeout=None):
        self.returncode = 0
        return self.returncode

    def terminate(self):
        self.returncode = -1


class ProxyTests(unittest.TestCase):
    def setUp(self):
        self.process = FakeProcess()
        self.failed = []
        self.proxy = effect_overlay_x11.EffectOverlayX11Proxy(
            on_failure=lambda: self.failed.append(True), popen=lambda *args, **kwargs: self.process
        )

    def test_calls_write_the_helper_protocol(self):
        self.proxy.configure("flint", "ocean", "long", "large", 1.35, 0.052)
        self.proxy.push("right", 0.7, False, part="end")
        self.proxy.arrive("edge", "left", 768, 432, [0, 0, 1536, 864])
        self.proxy.crossed_in()
        self.proxy.switched(384, 216, [0, 0, 1536, 864], "match")
        self.proxy.configure_glow("beam", "sunset", "short", "small", 0.75, 0.03)
        self.proxy.set_pressure("bottom", 1.0, True, "start")
        self.proxy.stop()

        self.assertEqual([json.loads(line) for line in self.process.stdin.lines], [
            {"op": "effect_configure", "style": "flint", "colour": "ocean", "length": "long", "size": "large",
             "effect_pace": 1.35, "size_fraction": 0.052},
            {"op": "push", "edge": "right", "pressure": 0.7, "crossed": False, "part": "end"},
            {"op": "arrive", "method": "edge", "edge": "left", "x": 768, "y": 432, "desk": [0, 0, 1536, 864]},
            {"op": "crossed_in"},
            {"op": "switched", "x": 384, "y": 216, "desk": [0, 0, 1536, 864], "choice": "match"},
            {"op": "glow_configure", "style": "beam", "colour": "sunset", "length": "short", "size": "small",
             "effect_pace": 0.75, "size_fraction": 0.03},
            {"op": "glow_pressure", "edge": "bottom", "pressure": 1.0, "crossed": True, "part": "start"},
            {"op": "stop"},
        ])

    def test_push_with_cursor_serialises_the_cursor_and_desktop_bounds(self):
        self.proxy.push("right", 0.7, False, cursor=QPointF(1590, 440), desk=QRect(100, -20, 1600, 900))

        self.assertEqual(json.loads(self.process.stdin.lines[0]), {
            "op": "push", "edge": "right", "pressure": 0.7, "crossed": False, "part": None,
            "x": 1590, "y": 440, "desk": [100, -20, 1600, 900],
        })

    def test_push_without_cursor_keeps_the_existing_protocol(self):
        self.proxy.push("right", 0.7, False, part="end")

        self.assertEqual(json.loads(self.process.stdin.lines[0]), {
            "op": "push", "edge": "right", "pressure": 0.7, "crossed": False, "part": "end",
        })

    def test_helper_exit_marks_proxy_failed_once(self):
        self.proxy.configure("flint", "ocean")
        self.process.returncode = 1
        self.proxy.check_process()
        self.proxy.check_process()

        self.assertTrue(self.proxy.failed)
        self.assertEqual(self.failed, [True])

    @unittest.skipIf(sys.platform == "win32", "select() on a pipe is POSIX only; the helper runs only on Linux")
    def test_helper_failure_message_marks_proxy_failed(self):
        read_fd, write_fd = os.pipe()
        self.process.stdout = os.fdopen(read_fd, "r")
        os.write(write_fd, b"failed effect\n")
        os.close(write_fd)

        self.proxy.configure("flint", "ocean")
        self.proxy.check_process()

        self.assertTrue(self.proxy.failed)
        self.assertEqual(self.failed, [True])

    def test_helper_start_failure_marks_proxy_failed(self):
        proxy = effect_overlay_x11.EffectOverlayX11Proxy(
            on_failure=lambda: self.failed.append(True),
            popen=lambda *args, **kwargs: (_ for _ in ()).throw(OSError("unavailable")),
        )
        proxy.configure("flint", "ocean")

        self.assertTrue(proxy.failed)
        self.assertEqual(self.failed, [True])


class DispatchTests(unittest.TestCase):
    def test_dispatch_maps_points_by_fraction_and_calls_the_overlays(self):
        overlay = mock.Mock()
        glow = mock.Mock()
        commands = [
            {"op": "effect_configure", "style": "flint", "colour": "ocean", "length": "normal", "size": "medium",
             "effect_pace": 1.4, "size_fraction": 0.055},
            {"op": "push", "edge": "right", "pressure": 0.5, "crossed": False, "part": None,
             "x": 778, "y": 452, "desk": [10, 20, 1536, 864]},
            {"op": "arrive", "method": "edge", "edge": "left", "x": 768, "y": 432, "desk": [0, 0, 1536, 864]},
            {"op": "crossed_in"},
            {"op": "switched", "x": 384, "y": 216, "desk": [0, 0, 1536, 864], "choice": "match"},
            {"op": "glow_configure", "style": "beam", "colour": "sunset", "length": "short", "size": "small",
             "effect_pace": 0.8, "size_fraction": 0.025},
            {"op": "glow_pressure", "edge": "bottom", "pressure": 1.0, "crossed": True, "part": "end"},
            {"op": "stop"},
        ]

        class Bounds:
            def x(self): return 100
            def y(self): return -50
            def width(self): return 3072
            def height(self): return 1728

        for command in commands[:-1]:
            effect_overlay_x11.dispatch(command, overlay, glow, Bounds(), lambda x, y: (x, y))

        self.assertEqual(overlay.configure.call_args.args,
                         ("flint", "ocean", "normal", "medium", 1.4, 0.055))
        self.assertEqual(overlay.push.call_args, mock.call("right", 0.5, False, cursor=(1636, 814), part=None))
        effect_overlay_x11.dispatch(
            {"op": "push", "edge": "left", "pressure": 0.2, "crossed": False}, overlay, glow, Bounds(),
            lambda x, y: (x, y)
        )
        self.assertEqual(overlay.push.call_args, mock.call("left", 0.2, False, part=None))
        self.assertEqual(overlay.arrive.call_args.args, ("edge", "left", (1636, 814)))
        self.assertEqual(overlay.switched.call_args.args, ((868, 382), "match"))
        self.assertEqual(glow.configure.call_args.args,
                         ("beam", "sunset", "short", "small", 0.8, 0.025))
        self.assertEqual(glow.set_pressure.call_args, mock.call("bottom", 1.0, True, "end"))
        overlay.crossed_in.assert_called_once_with()
        self.assertTrue(effect_overlay_x11.dispatch(commands[-1], overlay, glow, Bounds()))
        overlay.stop.assert_called_once_with()
        glow.hide.assert_called_once_with()


class BridgeSelectionTests(unittest.TestCase):
    def test_proxy_is_selected_only_for_wayland_with_display(self):
        available = effect_overlay_x11.available
        self.assertTrue(available(True, {"DISPLAY": ":0"}))
        self.assertFalse(available(True, {}))
        self.assertFalse(available(False, {"DISPLAY": ":0"}))


if __name__ == "__main__":
    unittest.main()
