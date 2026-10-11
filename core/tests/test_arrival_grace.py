import collections
import logging
import unittest
from types import SimpleNamespace
from unittest import mock

from core import protocol, receiver, return_edge


MONITORS = [return_edge.Rect(0, 0, 100, 100)]


class Clock:
    def __init__(self):
        self.now = 10.0

    def __call__(self):
        return self.now


class ReturnEdgeArrivalGrace(unittest.TestCase):
    def setUp(self):
        self.clock = Clock()
        self.edge = return_edge.ReturnEdge("right", resistance_px=10, clock=self.clock)

    def test_outward_flick_during_grace_is_held_without_pressure(self):
        self.edge.start_grace()

        outcome = self.edge.feed(MONITORS, (99, 50), 20, 0)

        self.assertEqual(outcome.action, return_edge.HOLD)
        self.assertEqual(outcome.pressure, 0)
        self.assertEqual(outcome.position, (99, 50))

    def test_outward_push_crosses_after_grace(self):
        self.edge.start_grace()
        self.clock.now += 0.3

        outcome = self.edge.feed(MONITORS, (99, 50), 20, 0)

        self.assertEqual(outcome.action, return_edge.CROSS)

    def test_four_pixels_inward_ends_grace(self):
        self.edge.start_grace()
        self.assertEqual(self.edge.feed(MONITORS, (95, 50), -4, 0).action, return_edge.PASS)

        outcome = self.edge.feed(MONITORS, (99, 50), 10, 0)

        self.assertEqual(outcome.action, return_edge.CROSS)

    def test_construction_and_reset_do_not_start_grace(self):
        outcome = self.edge.feed(MONITORS, (99, 50), 20, 0)

        self.assertEqual(outcome.action, return_edge.CROSS)

        edge = return_edge.ReturnEdge("right", resistance_px=10, clock=self.clock)
        edge.start_grace()
        edge.reset()
        self.assertEqual(edge.feed(MONITORS, (99, 50), 20, 0).action, return_edge.CROSS)

    def test_start_grace_begins_a_new_grace_period(self):
        self.clock.now += 0.3
        edge = return_edge.ReturnEdge("right", resistance_px=10, clock=self.clock)
        edge.start_grace()

        outcome = edge.feed(MONITORS, (99, 50), 20, 0)

        self.assertEqual(outcome.action, return_edge.HOLD)

    def test_part_edge_has_the_same_grace(self):
        edge = return_edge.PartEdge("right", ["middle"], 10, self.clock)
        edge.start_grace()

        outcome = edge.feed(MONITORS, (99, 50), 20, 0)

        self.assertEqual(outcome.action, return_edge.HOLD)

    def test_corner_push_has_the_same_grace(self):
        edge = return_edge.CornerPush("top_right", "right", 10, self.clock)
        edge.start_grace()

        outcome = edge.feed(MONITORS, (99, 0), 20, -20)

        self.assertEqual(outcome.action, return_edge.HOLD)

    def test_a_rearm_keeps_the_grace_of_the_zone_it_replaces(self):
        old = return_edge.ReturnEdge("right", resistance_px=10, clock=self.clock)
        old.start_grace()
        new = return_edge.ReturnEdge("right", resistance_px=10, clock=self.clock)
        other = return_edge.ReturnEdge("left", resistance_px=10, clock=self.clock)
        return_edge.keep_grace([(b"a", old)], [(b"a", new), (b"a", other)])
        self.assertEqual(new.feed(MONITORS, (99, 50), 20, 0).action, return_edge.HOLD)
        self.assertEqual(other.feed(MONITORS, (0, 50), -20, 0).action, return_edge.CROSS)

    def test_landing_beside_a_seam_staggered_by_less_than_the_clearance_stays_on_the_wall(self):
        monitors = [return_edge.Rect(0, 0, 2560, 1440), return_edge.Rect(-1920, 5, 1920, 1080)]
        self.assertEqual(return_edge.arrival_position(monitors, "left", 0.0), (0, 0))
        self.assertEqual(return_edge.arrival_position([monitors[0]], "left", 0.0), (0, 10))

    def test_take_starts_grace_only_for_the_zone_back_to_its_owner(self):
        self.assertEqual(self._take({"stay": True}), (return_edge.HOLD, return_edge.CROSS))

    def test_a_take_that_lands_on_an_edge_holds_every_zone(self):
        # rc14 (10-10-2026): the push that brought input here went on through a zone to a third machine.
        self.assertEqual(self._take({"stay": False, "edge": "right", "offset": 0.5}),
                         (return_edge.HOLD, return_edge.HOLD))

    def _take(self, how):
        owner_peer, other_peer = b"owner-peer-id!!!", b"other-peer-id!!!"
        owner_peer_id, other_peer_id = protocol.id_text(owner_peer), protocol.id_text(other_peer)
        peers = [
            {"id": owner_peer_id, "side": "right", "in_use": True, "port": 1},
            {"id": other_peer_id, "side": "right", "in_use": True, "port": 1},
        ]
        zones = [
            {"kind": "edge", "peer": owner_peer_id},
            {"kind": "edge", "peer": other_peer_id},
        ]
        responder_instance = receiver.LinkResponder.__new__(receiver.LinkResponder)
        responder_instance._owner = None
        responder_instance._reach = frozenset((other_peer,))
        responder_instance._resistance = 10
        responder_instance._notch_span = None
        responder_instance._armed = []
        responder_instance._zones_live = False
        responder_instance._visit = 0
        responder_instance._clipboard_mark = None
        responder_instance._sent_home_for_lock = False
        responder_instance._locked_since = None
        responder_instance._owner_events = collections.deque()
        responder_instance._clipboard_stamp = lambda: None
        link = SimpleNamespace(peer_id=owner_peer, own_id=b"this-machine-id!", name="Owner")
        focus = {"reach": (other_peer,), "resistance_px": 10, **how}

        responder_instance._own_locked(link, focus, peers, zones)

        models = dict(responder_instance._armed)
        return (models[owner_peer].feed(MONITORS, (99, 50), 10, 0).action,
                models[other_peer].feed(MONITORS, (99, 50), 10, 0).action)


class IncomingMoveLogging(unittest.TestCase):
    def test_logs_one_second_of_move_counts_and_raw_delta_sums(self):
        link = SimpleNamespace(name="Peer", incoming_moves_at=None, incoming_move_count=0,
                               incoming_move_last_at=None, incoming_dx=0, incoming_dy=0, incoming_largest=0)
        with mock.patch.object(receiver.time, "monotonic", side_effect=(1.0, 1.2, 2.0)):
            with self.assertLogs(receiver.LOGGER, level=logging.DEBUG) as logs:
                receiver.LinkResponder._record_incoming_move(None, link, {"dx": 3, "dy": -4})
                receiver.LinkResponder._record_incoming_move(None, link, {"dx": -5, "dy": 2})
                receiver.LinkResponder._record_incoming_move(None, link, {"dx": 1, "dy": 8})

        self.assertEqual(len(logs.output), 1)
        self.assertIn("incoming moves from Peer: 3 moves, |dx| sum 9, |dy| sum 14, largest 8 in the last second", logs.output[0])

    def test_idle_period_does_not_emit_a_line(self):
        link = SimpleNamespace(name="Peer", incoming_moves_at=None, incoming_move_count=0,
                               incoming_move_last_at=None, incoming_dx=0, incoming_dy=0, incoming_largest=0)
        with mock.patch.object(receiver.time, "monotonic", side_effect=(1.0, 5.0)):
            with self.assertNoLogs(receiver.LOGGER, level=logging.DEBUG):
                receiver.LinkResponder._record_incoming_move(None, link, {"dx": 3, "dy": 4})
                receiver.LinkResponder._record_incoming_move(None, link, {"dx": 5, "dy": 2})


if __name__ == "__main__":
    unittest.main()
