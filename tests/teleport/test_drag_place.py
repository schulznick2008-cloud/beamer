"""Kanten-Drag am PC (§2.1, Paket 05b): Offer mit drag -> recv.place, proxy.drag an der Kante -> tp_close return."""
import base64
import json
import os
from pathlib import Path

import pytest

from core import protocol
from core.return_edge import Rect, ReturnEdge
from core.teleport_bridge import win_at_return_edge
from core.teleport_protocol import offer_msg, read_tp
from core.teleport_service import TeleportService

ROOT = Path(__file__).resolve().parents[2]
VECTORS = ROOT / "docs" / "teleport" / "vectors.json"


class FakeLinkSession:
    def __init__(self, link):
        self._link = link

    def teleport_handshake_material(self):
        link = self._link
        return (
            bytes.fromhex(link["shared_hex"]),
            bytes.fromhex(link["key_id_hex"]),
            bytes.fromhex(link["prefix_i_hex"]),
            bytes.fromhex(link["prefix_r_hex"]),
            bytes.fromhex(link["share_i_hex"]),
            bytes.fromhex(link["share_r_hex"]),
        )


class StubHelper:
    """Schreibt Befehle mit und antwortet ok (kein Prozess)."""

    def __init__(self):
        self.calls = []

    def request(self, cmd, **kw):
        self.calls.append((cmd, kw))
        if cmd == "transport.open":
            return {"ok": True, "local_port": 47001}
        return {"ok": True}

    def request_retry_busy(self, cmd, tries=1, delay=0.0, **kw):
        return self.request(cmd, **kw)

    def names(self):
        return [c for c, _ in self.calls]

    def shutdown(self):
        pass


@pytest.fixture()
def win():
    with open(VECTORS, encoding="utf-8") as f:
        link = json.load(f)["link"]
    posted = []

    def post(peer, msg, outbound):
        posted.append((peer, msg, outbound))
        return True

    svc = TeleportService("windows", post_to_peer=post)
    svc.on_link_up("peer-mac", link["token"], "macos", True, FakeLinkSession(link), outbound=False)
    helper = StubHelper()
    svc._helper = helper
    return svc, helper, posted


def _offer(tid, **kw):
    return offer_msg(
        tid,
        app="Finanzen",
        title="Konto",
        icon_png="",
        w=800,
        h=600,
        scale=2.0,
        codec="hevc",
        mode="window",
        udp_port=50000,
        salt_b64=base64.b64encode(os.urandom(16)).decode(),
        **kw,
    )


def _kinds(posted):
    return [read_tp(m)[0] for _p, m, _o in posted if read_tp(m)]


def test_offer_with_drag_places_the_proxy_after_open(win):
    svc, helper, posted = win
    svc.handle_link_message("peer-mac", _offer(7, place_x=120.0, place_y=40.0, drag=True), outbound=False)
    names = helper.names()
    assert "recv.open" in names and "recv.place" in names
    assert names.index("recv.place") > names.index("recv.open")
    place = dict(helper.calls)["recv.place"]
    assert place == {"tid": 7, "x": 120.0, "y": 40.0, "dragging": True}
    assert "tp_accept" in _kinds(posted)


def test_offer_without_drag_stays_as_before(win):
    svc, helper, _posted = win
    svc.handle_link_message("peer-mac", _offer(8), outbound=False)
    svc.handle_link_message("peer-mac", _offer(9, drag=True), outbound=False)  # drag ohne Position
    assert "recv.open" in helper.names()
    assert "recv.place" not in helper.names()


def test_proxy_drag_at_the_mac_edge_returns_the_window(win):
    svc, helper, posted = win
    svc.handle_link_message("peer-mac", _offer(10), outbound=False)
    seen = []

    def probe(peer, x, y):
        seen.append((peer, x, y))
        return x <= 0

    svc.set_return_edge_probe(probe)
    # Mitten auf dem Bildschirm: nichts.
    svc._on_helper_event({"ev": "proxy.drag", "tid": 10, "x": 500, "y": 300, "px_x": 500, "px_y": 300, "dragging": True})
    assert "tp_close" not in _kinds(posted)
    # Loslassen ohne Kante: nichts.
    svc._on_helper_event({"ev": "proxy.drag", "tid": 10, "x": 0, "y": 300, "px_x": 0, "px_y": 300, "dragging": False})
    assert "tp_close" not in _kinds(posted)
    # An der Kante zum Mac (physische Pixel bevorzugt): tp_close return, Proxy zu.
    svc._on_helper_event({"ev": "proxy.drag", "tid": 10, "x": 999, "y": 300, "px_x": 0, "px_y": 300, "dragging": True})
    closes = [read_tp(m)[1] for _p, m, _o in posted if read_tp(m) and read_tp(m)[0] == "tp_close"]
    assert closes == [{"tid": 10, "reason": "return"}] or closes[0]["reason"] == "return"
    assert ("recv.close", {"tid": 10}) in helper.calls
    assert seen[-1] == ("peer-mac", 0.0, 300.0)
    # Weitere Ereignisse zum geschlossenen tid: kein zweites tp_close.
    svc._on_helper_event({"ev": "proxy.drag", "tid": 10, "x": 0, "y": 300, "px_x": 0, "px_y": 300, "dragging": True})
    assert _kinds(posted).count("tp_close") == 1


def test_proxy_drag_without_probe_does_nothing(win):
    svc, _helper, posted = win
    svc.handle_link_message("peer-mac", _offer(11), outbound=False)
    svc._on_helper_event({"ev": "proxy.drag", "tid": 11, "x": 0, "y": 0, "dragging": True})
    assert "tp_close" not in _kinds(posted)


class FakeSender:
    """Zwei Monitore nebeneinander, der Mac links am linken Monitor (Beamer-Kantenmodell)."""

    def __init__(self, peer: bytes):
        self._models = [(peer, ReturnEdge("left"))]

    def _cached_monitors(self):
        return [Rect(-1920, 0, 1920, 1080), Rect(0, 0, 1920, 1080)]

    def _cached_display_map(self):
        return {}


def test_win_at_return_edge_uses_beamers_edge_models():
    peer = bytes(range(16))
    peer_id = protocol.id_text(peer)
    sender = FakeSender(peer)
    assert win_at_return_edge(sender, peer_id, -1920, 500) is True
    # Wie Beamers eigener Wechsel: nur die aeusserste Pixelspalte (dort haelt Windows den Zeiger fest).
    assert win_at_return_edge(sender, peer_id, -1919, 500) is False
    assert win_at_return_edge(sender, peer_id, -1000, 500) is False
    # Innere Naht zwischen den Monitoren ist keine Kante.
    assert win_at_return_edge(sender, peer_id, 0, 500) is False
    # Anderer Peer: nie.
    assert win_at_return_edge(sender, "jemand-anders", -1920, 500) is False
