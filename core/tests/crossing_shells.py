"""Thin headless shells for the crossing model, shared by both desktop platforms.

Every crossing event calls core.crossing_state and carries out its Outcome. AppKit and Qt
are absent; the only platform difference is the platform argument supplied to the core.
The harness uses real responders, owners and wire messages beneath these shells.
"""

import copy
import threading
import time

from core import crossing_state, protocol, return_edge, settings_sync, ways

SNAP_RADIUS = 12


class Shell:
    platform = None

    def __init__(self, desk):
        self.desk = desk
        self.sent_count = 0
        self.handled_count = 0
        self.lock = threading.RLock()
        self.refused = []
        callbacks = desk.machine.responder._callbacks
        callbacks["arrangement"] = self._arrangement_arrived
        callbacks["settings"] = self._settings_arrived

    def busy(self):
        return False

    def live(self, peer):
        return peer in self.desk.hand.links

    def gone(self, peer):
        return False

    def capable(self):
        return {peer for peer, caps in self.desk.hand.capabilities.items() if "layout" in caps} | {self.desk.id}

    def current(self):
        return copy.deepcopy(self.desk.settings)

    def _args(self):
        return dict(platform=self.platform, live=self.live, gone=self.gone, now=time.time())

    def _send(self, peer, message):
        if self.desk.hand.send(peer, message):
            self.sent_count += 1
            return True
        return False

    def _arrangement_arrived(self, peer, read):
        try:
            with self.lock:
                self.on_arrangement(protocol.id_text(peer), read)
        finally:
            self.handled_count += 1

    def _settings_arrived(self, peer, data):
        try:
            with self.lock:
                self.on_settings(protocol.id_text(peer), data)
        finally:
            self.handled_count += 1

    def carry_out(self, outcome, source=None, arrived=None):
        if outcome["changed"]:
            self.desk.save(outcome["settings"])
        for peer in outcome["tell"]:
            self.tell(peer)
        if outcome["send_picture"]:
            self.send_same()
        if outcome["forward_picture"] and arrived is not None:
            self.send_same(exclude=source, data=arrived)
        if outcome["changed"]:
            self.after_change()
        return outcome

    def paired(self, settings):
        outcome = crossing_state.paired(settings, platform=self.platform,
                                       own_displays=self.desk.screen.display_map(), now=time.time())
        settings.clear()
        settings.update(outcome["settings"])

    def same_state(self):
        settings = self.current()
        tiles = copy.deepcopy(settings.get("layout", {}))
        for entry in settings.get("peers", []):
            tile = tiles.get(entry.get("id"))
            if isinstance(tile, dict) and entry.get("name", "").strip():
                tile["name"] = entry["name"].strip()[:64]
        state = {"set_at": settings.get("layout_set_at", 0),
                 "by": settings.get("layout_by") or self.desk.id, "tiles": tiles}
        return settings_sync.message_data(False, settings.get("same_set_at", 0), {},
                                          by=self.desk.id, layout_state=state)

    def send_same(self, exclude=None, data=None):
        data = self.same_state() if data is None else data
        for peer in list(self.desk.hand.links):
            if peer != exclude and peer in self.capable():
                self._send(peer, protocol.settings_msg(data))

    def link_up(self, other):
        with self.lock:
            outcome = crossing_state.link_up(self.current(), other.id, platform=self.platform,
                         own_displays=self.desk.screen.display_map(), now=time.time(), capable=self.capable())
            self.carry_out(outcome)

    def tell(self, peer):
        settings = self.current()
        entry = ways._entry(settings, peer)
        if entry is None or entry.get("side") not in return_edge.EDGES:
            return
        author = protocol.read_id(entry.get("side_by")) or self.desk.own
        self._send(peer, protocol.arrangement_v6(
            entry["side"], int(entry.get("side_set_at", 0)), author,
            way_back=ways.has_way(settings, peer),
            way_back_by=protocol.read_id(ways.way_back_by(settings, peer))))

    def after_change(self):
        server = self.desk.machine.responder
        server.peers_changed()
        server.rearm()

    def stop(self):
        pass

    def drop(self, moved, x, y, snap=True):
        with self.lock:
            settings = self.current()
            tiles = copy.deepcopy(settings["layout"])
            tile = tiles.get(moved.id, dict(w=moved.tile_size[0], h=moved.tile_size[1]))
            tiles[moved.id] = dict(tile, x=int(x), y=int(y))
            outcome = crossing_state.drop(settings, tiles, moved.id, capable=self.capable(),
                                          snap_radius=SNAP_RADIUS if snap else 0, **self._args())
            if not outcome["accepted"]:
                self.refused.append(moved.id)
                return False
            self.carry_out(outcome)
            return True

    def edit(self, other, methods, parts=None, corner=None):
        with self.lock:
            settings = self.current()
            held = ways.ways(settings, other.id)
            thirds = list(ways.PARTS) if "edge" in methods else (parts if parts is not None else held["parts"])
            outcome = crossing_state.edit_ways(settings, other.id, edge=bool(set(methods) & {"edge", "part"}),
                         thirds=thirds, corner="corner" in methods, corner_at=corner,
                         notch="notch" in methods, capable=self.capable(), **self._args())
            self.carry_out(outcome)
            return True

    def pick_third(self, other, third):
        return self.click_third(other, third)

    def click_third(self, other, third):
        with self.lock:
            settings = self.current()
            held = ways.ways(settings, other.id)
            if ways._entry(settings, other.id).get("left_out") or held["side"] not in ways.EDGES:
                return False
            methods, parts = ways.toggle_third(held["methods"], held["parts"], third)
            outcome = crossing_state.edit_ways(settings, other.id,
                         edge=bool(set(methods) & {"edge", "part"}), thirds=parts,
                         capable=self.capable(), **self._args())
            self.carry_out(outcome)
            return True

    def click_edge_text(self, other):
        with self.lock:
            settings = self.current()
            if ways._entry(settings, other.id).get("left_out"):
                return False
            held = ways.ways(settings, other.id)
            methods, parts = ways.toggle_edge(held["methods"], held["parts"])
            outcome = crossing_state.edit_ways(settings, other.id,
                         edge=bool(set(methods) & {"edge", "part"}), thirds=parts,
                         capable=self.capable(), **self._args())
            self.carry_out(outcome)
            return True

    def click_corner(self, other, corner):
        with self.lock:
            settings = self.current()
            if (ways._entry(settings, other.id).get("left_out")
                    or ways.corner_holder(settings, corner, exclude=other.id) is not None):
                return False
            held = ways.ways(settings, other.id)
            on = not ("corner" in held["methods"] and held["corner"] == corner)
            outcome = crossing_state.edit_ways(settings, other.id, corner=on, corner_at=corner,
                                             capable=self.capable(), **self._args())
            self.carry_out(outcome)
            return True

    def leave_out(self, other):
        with self.lock:
            return self.carry_out(crossing_state.leave_out(self.current(), other.id,
                                                         capable=self.capable(), **self._args()))

    def put_back(self, other):
        with self.lock:
            return self.carry_out(crossing_state.put_back(self.current(), other.id,
                                                        capable=self.capable(), **self._args()))

    def displays_changed(self):
        with self.lock:
            return self.carry_out(crossing_state.displays_changed(self.current(), platform=self.platform,
                                 own_displays=self.desk.screen.display_map(), now=time.time(),
                                 capable=self.capable()))

    def on_arrangement(self, peer, read):
        outcome = crossing_state.receive_arrangement(self.current(), peer, read,
                                                     capable=self.capable(), **self._args())
        return self.carry_out(outcome)["changed"]

    def on_settings(self, peer, data):
        state = settings_sync.read_layout_state(data)
        if state is None:
            return False
        outcome = crossing_state.receive_picture(self.current(), state, source=peer,
                         capable=self.capable(), own_displays=self.desk.screen.display_map(), **self._args())
        return self.carry_out(outcome, source=peer, arrived=data)["changed"]


class MacShell(Shell):
    platform = "mac"


class PcShell(Shell):
    platform = "pc"


SHELLS = {"mac": MacShell, "pc": PcShell}
