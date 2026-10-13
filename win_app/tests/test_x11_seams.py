"""Internal X11 seam geometry and pressure without requiring an X server."""

import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
sys.path.append(str(Path(__file__).resolve().parent.parent.parent))

import x11_seams  # noqa: E402
from core.return_edge import Rect  # noqa: E402


class FakeBackend:
    def __init__(self):
        self.created = []
        self.destroyed = []
        self.released = []

    def create_barrier(self, seam):
        self.created.append(seam)
        return len(self.created) + 100

    def destroy_barrier(self, barrier):
        self.destroyed.append(barrier)

    def release_pointer(self, device, barrier, eventid):
        self.released.append((device, barrier, eventid))


class SeamGeometryTest(unittest.TestCase):
    def test_only_shared_monitor_edge_segments_are_returned(self):
        found = x11_seams.seams([
            Rect(-1280, 100, 1280, 1024),
            Rect(0, 0, 1920, 1080),
            Rect(1920, 400, 1280, 800),
        ])
        self.assertEqual([seam.key for seam in found], [
            (0, 100, 0, 1079),
            (1920, 400, 1920, 1079),
        ])

    def test_diagonal_and_gapped_displays_have_no_seam(self):
        self.assertEqual(x11_seams.seams([Rect(0, 0, 100, 100), Rect(100, 101, 100, 100)]), [])
        self.assertEqual(x11_seams.seams([Rect(0, 0, 100, 100), Rect(101, 0, 100, 100)]), [])


class SeamControllerTest(unittest.TestCase):
    def setUp(self):
        self.backend = FakeBackend()
        self.controller = x11_seams.Controller(self.backend)
        self.monitors = [Rect(0, 0, 100, 100), Rect(100, 0, 100, 100)]

    def test_barrier_releases_only_after_configured_outward_pressure(self):
        self.controller.configure(self.monitors, 30)
        barrier = 101
        self.controller.handle(("barrier", x11_seams.XI_BARRIER_HIT, 2, 44, barrier, 12.0, 0.0))
        self.controller.handle(("barrier", x11_seams.XI_BARRIER_HIT, 2, 44, barrier, 15.0, 0.0))
        self.assertEqual(self.backend.released, [])
        self.controller.handle(("barrier", x11_seams.XI_BARRIER_HIT, 2, 44, barrier, 3.0, 0.0))
        self.assertEqual(self.backend.released, [(2, barrier, 44)])
        self.controller.handle(("barrier", x11_seams.XI_BARRIER_HIT, 2, 44, barrier, 20.0, 0.0))
        self.assertEqual(len(self.backend.released), 1)

    def test_reversing_along_or_leaving_does_not_carry_pressure_forward(self):
        self.controller.configure(self.monitors, 30)
        barrier = 101
        key = ("barrier", x11_seams.XI_BARRIER_HIT, 2, 45, barrier)
        self.controller.handle(key + (18.0, 0.0))
        self.controller.handle(key + (-12.0, 0.0))
        self.assertEqual(self.backend.released, [])
        self.controller.handle(("barrier", x11_seams.XI_BARRIER_LEAVE, 2, 45, barrier, 0.0, 0.0))
        self.controller.handle(key + (18.0, 0.0))
        self.assertEqual(self.backend.released, [])

    def test_disabled_state_removes_all_barriers_and_zero_resistance_creates_none(self):
        self.controller.configure(self.monitors, 30)
        self.controller.configure(self.monitors, 30, enabled=False)
        self.assertEqual(self.backend.destroyed, [101])
        self.controller.configure(self.monitors, 0)
        self.assertEqual(len(self.backend.created), 1)

    def test_barrier_event_parser_reads_fixed_point_attempted_motion(self):
        body = struct.pack("=HIIIIIIIIH2xii iI iI", 2, 10, 31, 1, 1, 104, 5, 12, 0,
                           6, 12 * 65536, 24 * 65536, -4, 0x80000000, 2, 0)
        event = x11_seams.parse_barrier_event(x11_seams.XI_BARRIER_HIT, body)
        self.assertEqual(event[:5], ("barrier", x11_seams.XI_BARRIER_HIT, 2, 31, 104))
        self.assertEqual(event[5:7], (-3.5, 2.0))
        self.assertEqual(event[7:9], (12.0, 24.0))

    def test_parser_rejects_truncated_or_unrelated_event(self):
        with self.assertRaises(ValueError):
            x11_seams.parse_barrier_event(x11_seams.XI_BARRIER_HIT, b"short")
        with self.assertRaises(ValueError):
            x11_seams.parse_barrier_event(17, b"" * 62)


if __name__ == "__main__":
    unittest.main()
