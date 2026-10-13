"""Zwei TeleportService-Instanzen mit Fake-Helfern (CI)."""
import json
import os
from pathlib import Path

import pytest

from core.teleport_protocol import read_tp
from core.teleport_service import TeleportService, default_helper_config

ROOT = Path(__file__).resolve().parents[2]
VECTORS = ROOT / "docs" / "teleport" / "vectors.json"


@pytest.fixture(scope="module")
def vectors():
    with open(VECTORS, encoding="utf-8") as f:
        return json.load(f)


def _fake_cfg(platform: str, port: int) -> dict:
    os.environ["BEAMER_TELEPORT_FAKE_HELPER"] = "1"
    os.environ["BEAMER_FAKE_HELPER_LOCAL_PORT"] = str(port)
    return default_helper_config(platform, fake=True)


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


def test_mac_offer_win_accept_roundtrip(vectors):
    inbox = []

    def post_mac(peer, msg, outbound):
        inbox.append(("mac", outbound, msg))
        return True

    def post_win(peer, msg, outbound):
        inbox.append(("win", outbound, msg))
        return True

    mac = TeleportService("macos", post_to_peer=post_mac, helper_config=_fake_cfg("macos", 47201))
    win = TeleportService("windows", post_to_peer=post_win, helper_config=_fake_cfg("win", 47202))
    session = FakeLinkSession(vectors["link"])
    token = vectors["link"]["token"]
    mac.on_link_up("peer-win", token, "windows", True, session, outbound=True)
    win.on_link_up("peer-mac", token, "macos", True, session, outbound=False)

    assert mac.toggle_front_window_mac("peer-win") is True

    offers = [m for side, _out, m in inbox if read_tp(m) and read_tp(m)[0] == "tp_offer"]
    assert offers
    win.handle_link_message("peer-mac", offers[0], outbound=False)

    accepts = [m for side, _out, m in inbox if read_tp(m) and read_tp(m)[0] == "tp_accept"]
    assert accepts
    mac.handle_link_message("peer-win", accepts[0], outbound=True)

    mac.shutdown()
    win.shutdown()
