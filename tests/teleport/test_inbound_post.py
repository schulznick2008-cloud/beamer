"""Inbound-Link: Peer-ID bytes vs. str, tp_offer senden."""
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock

from core import protocol
from core.teleport_bridge import mac_post_to_peer, win_post_to_peer
from core.teleport_protocol import MSG_OFFER, read_tp
from core.teleport_service import TeleportService, default_helper_config

ROOT = Path(__file__).resolve().parents[2]
VECTORS = ROOT / "docs" / "teleport" / "vectors.json"

PC_TEXT = "D2vsgbWmN1KTwWpVZBqxoA"


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


def _fake_cfg(platform: str, port: int, monkeypatch):
    monkeypatch.setenv("BEAMER_TELEPORT_FAKE_HELPER", "1")
    monkeypatch.setenv("BEAMER_FAKE_HELPER_LOCAL_PORT", str(port))
    monkeypatch.setenv("BEAMER_TELEPORT_PEER_WAIT", "0")
    return default_helper_config(platform, fake=True)


def test_mac_post_inbound_with_bytes_peer_id():
    pc_bytes = protocol.read_id(PC_TEXT)
    assert pc_bytes is not None
    responder = MagicMock()
    responder.send.return_value = True
    controller = SimpleNamespace(_peers_up={}, responder=responder)
    offer = {"type": "tp_offer", "data": {"tid": 1}}
    ok, reason = mac_post_to_peer(controller, pc_bytes, offer, outbound=False)
    assert ok, reason
    responder.send.assert_called_once_with(pc_bytes, offer)


def test_mac_hotkey_inbound_bytes_peer_sends_tp_offer(monkeypatch):
    vectors = json.loads(VECTORS.read_text())
    pc_bytes = protocol.read_id(PC_TEXT)
    assert pc_bytes is not None
    posted = []

    def post(peer, msg, outbound):
        posted.append((peer, outbound, read_tp(msg)[0] if read_tp(msg) else None))
        return True

    mac = TeleportService("macos", post_to_peer=post, helper_config=_fake_cfg("macos", 47501, monkeypatch))
    session = FakeLinkSession(vectors["link"])
    mac.on_link_up(pc_bytes, vectors["link"]["token"], "windows", True, session, outbound=False)
    assert mac._default_pc_peer() == PC_TEXT
    assert mac.toggle_front_window_mac() is True
    assert len(posted) == 1
    assert posted[0][0] == PC_TEXT
    assert posted[0][1] is False
    assert posted[0][2] == MSG_OFFER
    mac.shutdown()


def test_win_inbound_accept_posts_outbound_false(monkeypatch):
    vectors = json.loads(VECTORS.read_text())
    mac_bytes = b"\x01" + bytes(15)
    mac_text = protocol.id_text(mac_bytes)
    posted = []

    def post(peer, msg, outbound):
        posted.append((peer, outbound))
        return True

    win = TeleportService("windows", post_to_peer=post, helper_config=_fake_cfg("win", 47502, monkeypatch))
    session = FakeLinkSession(vectors["link"])
    win.on_link_up(mac_text, vectors["link"]["token"], "macos", True, session, outbound=False)
    ident = protocol.read_id(mac_text)
    server = MagicMock()
    server.send.return_value = True
    sender = MagicMock()
    ok, _ = win_post_to_peer(sender, server, mac_text, {"type": "tp_accept", "data": {}}, outbound=False)
    assert ok
    server.send.assert_called_once_with(ident, {"type": "tp_accept", "data": {}})
    win.shutdown()
