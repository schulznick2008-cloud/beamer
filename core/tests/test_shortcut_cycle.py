import unittest

from core import settings_sync, ways


HOME = "home"
A, B, C = "a", "b", "c"


def peer(ident, **values):
    return {"id": ident, "send": True, "in_use": True, "port": 5000, **values}


class ShortcutCycleTests(unittest.TestCase):
    def test_order_sorts_rows_then_columns_and_rotates_after_this_machine(self):
        peers = [peer(A), peer(B), peer(C)]
        positions = {HOME: (100, 100, 100, 100), A: (0, 100, 100, 100),
                     B: (200, 130, 100, 100), C: (200, 220, 100, 100)}
        self.assertEqual(ways.shortcut_order(peers, HOME, positions), [B, C, A, None])

    def test_home_split_uses_the_same_row_tolerance_as_reading_order(self):
        peers = [peer(A), peer(B)]
        positions = {HOME: (100, 100, 100, 100), A: (0, 120, 100, 100),
                     B: (200, 100, 100, 100)}
        self.assertEqual(ways.shortcut_order(peers, HOME, positions), [B, A, None])

    def test_unplaced_peers_follow_in_peer_order_and_left_out_stays_in_cycle(self):
        peers = [peer(A, left_out=True), peer(B), peer(C)]
        positions = {HOME: (0, 0, 100, 100), B: (100, 0, 100, 100)}
        self.assertEqual(ways.shortcut_order(peers, HOME, positions), [B, A, C, None])

    def test_not_sendable_peers_are_excluded(self):
        peers = [peer(A), peer(B, in_use=False), peer(C, send=False)]
        self.assertEqual(ways.shortcut_order(peers, HOME, {HOME: (0, 0, 100, 100)}), [A, None])

    def test_ticked_mode_omits_only_explicitly_unticked_peers(self):
        peers = [peer(A, shortcut_cycle=False), peer(B), peer(C, shortcut_cycle=None)]
        positions = {HOME: (0, 0, 100, 100)}
        self.assertEqual(ways.shortcut_order(peers, HOME, positions, "ticked"), [B, C, None])

    def test_next_target_skips_unready_peers_and_returns_home_after_the_last(self):
        order = [A, B, C, None]
        target, skipped = ways.next_target({"shortcut_goes_to": "next"}, order, None,
                                           lambda ident: ident != A)
        self.assertEqual((target, skipped), (B, [A]))
        self.assertEqual(ways.next_target({}, order, C, lambda ident: True), (None, []))

    def test_when_no_peer_is_ready_first_machine_is_named_for_the_refusal(self):
        self.assertEqual(ways.next_target({}, [A, B, None], None, lambda _ident: False),
                         (A, [A, B]))

    def test_hold_uses_last_machine_or_first_ready_cycle_machine(self):
        order = [A, B, None]
        self.assertEqual(ways.hold_target({"shortcut_goes_to": "last"}, order, A,
                                          lambda _ident: True), (A, []))
        self.assertEqual(ways.hold_target({"shortcut_goes_to": "next"}, order, A,
                                          lambda ident: ident == B), (B, [A]))

    def test_shortcut_mode_is_a_valid_optional_crossing_sync_value(self):
        data = settings_sync.message_data(True, 7, {"shortcut_goes_to": "ticked"})
        self.assertEqual(data["crossing"], {"shortcut_goes_to": "ticked"})
        self.assertEqual(settings_sync.read(data, set())[2], {"shortcut_goes_to": "ticked"})


if __name__ == "__main__":
    unittest.main()
