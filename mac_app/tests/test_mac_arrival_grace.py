"""rc14, 10-10-2026: the rig's corner sent input home onto the end of the Mac's bottom edge, and the
hand still moving left from that push crossed the Mac's left zone to the loan laptop 14 ms later."""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from core import return_edge
from crossing import CrossingEngine, Ways

BOUNDS = (0, 0, 1728, 1117)


def three_ways():
    """The Mac as the owner's log had it: the rig below, the loan laptop on the left, at 360 px."""
    return CrossingEngine(resistance_px=360, ways=[
        Ways("rig", frozenset({"edge"}), "bottom", frozenset(return_edge.PARTS), "top_right"),
        Ways("acg", frozenset({"edge"}), "left", frozenset(return_edge.PARTS), "top_right"),
    ])


def flick_left(machine, start, events=14, dx=-30.0, every=0.001):
    """A 1 kHz mouse still going left: the OS holds the pointer at the left wall once it gets there."""
    x, y = start
    for index in range(events):
        x = max(0.0, x + dx)
        step = machine.feed(x, y, dx, -2.0, BOUNDS, 10.0 + index * every)
        if step.crossed:
            return step
    return None


class ArrivalGrace(unittest.TestCase):
    def test_landing_at_the_end_of_an_edge_is_clear_of_the_edge_beside_it(self):
        for offset in (0.0, 1.0):
            x, y = CrossingEngine.arrival_point("bottom", offset, BOUNDS)
            self.assertGreaterEqual(x, return_edge.LANDING_CLEAR_PX, offset)
            self.assertLessEqual(x, BOUNDS[2] - 1 - return_edge.LANDING_CLEAR_PX, offset)

    def test_the_push_that_brought_input_home_does_not_go_on_to_another_machine(self):
        machine = three_ways()
        machine.start_grace(10.0)
        self.assertIsNone(flick_left(machine, CrossingEngine.arrival_point("bottom", 0.0, BOUNDS)))

    def test_without_the_grace_the_same_flick_crosses(self):
        self.assertEqual(flick_left(three_ways(), (0.0, 1112.0)).peer, "acg")

    def test_a_push_after_the_grace_crosses(self):
        machine = three_ways()
        machine.start_grace(10.0)
        self.assertIsNone(flick_left(machine, (0.0, 1112.0)))
        step = None
        for index in range(40):
            step = machine.feed(0.0, 600.0, -30.0, 0.0, BOUNDS, 10.0 + return_edge.ARRIVAL_GRACE_SECONDS + index * 0.01)
            if step.crossed:
                break
        self.assertTrue(step.crossed)
        self.assertEqual(step.peer, "acg")

    def test_moving_back_off_an_edge_ends_its_grace_early(self):
        machine = three_ways()
        machine.start_grace(10.0)
        machine.feed(20.0, 600.0, return_edge.ARRIVAL_GRACE_PX + 1, 0.0, BOUNDS, 10.01)
        step = None
        for index in range(40):
            step = machine.feed(0.0, 600.0, -30.0, 0.0, BOUNDS, 10.02 + index * 0.002)
            if step.crossed:
                break
        self.assertTrue(step.crossed, "a deliberate push back out crosses inside the grace")


if __name__ == "__main__":
    unittest.main()
