"""Teleport-Peer-Erkennung: inbound/outbound, Hotkey, Logging."""
import json
import logging
from pathlib import Path

from core.teleport_service import TeleportService, default_helper_config

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


def _fake_cfg(platform: str, port: int, monkeypatch):
    monkeypatch.setenv("BEAMER_TELEPORT_FAKE_HELPER", "1")
    monkeypatch.setenv("BEAMER_FAKE_HELPER_LOCAL_PORT", str(port))
    monkeypatch.setenv("BEAMER_TELEPORT_PEER_WAIT", "0")
    return default_helper_config(platform, fake=True)


def test_mac_hotkey_finds_inbound_pc_peer(monkeypatch):
    """PC hat den Link geöffnet (Mac: outbound=False) — Hotkey darf den Peer nicht ignorieren."""
    vectors = json.loads(VECTORS.read_text())
    posts = []

    def post(_peer, _msg, outbound):
        posts.append(outbound)
        return True

    mac = TeleportService("macos", post_to_peer=post, helper_config=_fake_cfg("macos", 47401, monkeypatch))
    session = FakeLinkSession(vectors["link"])
    mac.on_link_up(
        "peer-win",
        vectors["link"]["token"],
        "windows",
        True,
        session,
        outbound=False,
    )
    assert mac._default_pc_peer() == "peer-win"
    assert mac.toggle_front_window_mac() is True
    assert posts == [False]
    mac.shutdown()


def test_mac_drag_teleport_uses_inbound_link(monkeypatch):
    vectors = json.loads(VECTORS.read_text())
    posts = []

    def post(_peer, _msg, outbound):
        posts.append(outbound)
        return True

    mac = TeleportService("macos", post_to_peer=post, helper_config=_fake_cfg("macos", 47402, monkeypatch))
    session = FakeLinkSession(vectors["link"])
    mac.on_link_up("peer-win", vectors["link"]["token"], "windows", True, session, outbound=False)
    mac.start_drag_teleport_mac("peer-win", 1, 10.0, 20.0)
    assert posts == [False]
    mac.shutdown()


def test_teleport_peer_ready_lost_logs(caplog, monkeypatch):
    vectors = json.loads(VECTORS.read_text())
    caplog.set_level(logging.INFO, logger="core.teleport_service")
    win = TeleportService("windows", post_to_peer=lambda *_: True, helper_config=_fake_cfg("win", 47403, monkeypatch))
    session = FakeLinkSession(vectors["link"])
    win.on_link_up("peer-mac", vectors["link"]["token"], "macos", True, session, outbound=False)
    assert "teleport peer ready peer-mac inbound" in caplog.text
    win.on_link_down("peer-mac", outbound=False)
    assert "teleport peer lost peer-mac inbound" in caplog.text
    win.shutdown()
