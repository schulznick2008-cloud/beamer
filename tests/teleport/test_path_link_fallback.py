"""Pfadwahl: Beamer-Link-IP als Fallback für transport.peer (M2 WLAN-first)."""
import json
from pathlib import Path

from core.teleport_path import PathChooser, PathSettings
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


def test_note_link_peer_seeds_wlan_candidate():
    chooser = PathChooser(PathSettings(mode="wlan"), lambda _h, _p: (False, 0.0))
    chooser.note_link_peer("10.20.30.40")
    decision = chooser.tick(force=True)
    assert decision is not None
    assert decision.path == "wlan"
    assert decision.peer_host == "10.20.30.40"


def test_path_host_falls_back_to_peer_link_in_auto_mode():
    with open(VECTORS, encoding="utf-8") as f:
        vectors = json.load(f)
    session = FakeLinkSession(vectors["link"])
    svc = TeleportService(
        "macos",
        post_to_peer=lambda *_a: True,
        path_settings=PathSettings(mode="auto"),
        path_probe=lambda _h, _p: (False, 999.0),
    )
    svc.on_link_up(
        "peer-win",
        vectors["link"]["token"],
        "windows",
        True,
        session,
        outbound=True,
        peer_link_host="192.168.0.44",
    )
    assert svc._path_host_for_peer("peer-win", outbound=True) == "192.168.0.44"


def test_roundtrip_sends_transport_peer_on_both_sides(monkeypatch):
    import os

    from core.teleport_protocol import read_tp

    with open(VECTORS, encoding="utf-8") as f:
        vectors = json.load(f)

    os.environ["BEAMER_TELEPORT_FAKE_HELPER"] = "1"
    inbox = []
    peer_cmds = {"mac": [], "win": []}

    def post_mac(peer, msg, outbound):
        inbox.append(("mac", outbound, msg))
        return True

    def post_win(peer, msg, outbound):
        inbox.append(("win", outbound, msg))
        return True

    mac = TeleportService(
        "macos",
        post_to_peer=post_mac,
        helper_config=default_helper_config("macos", fake=True),
        path_settings=PathSettings(mode="auto"),
        path_probe=lambda _h, _p: (False, 999.0),
    )
    win = TeleportService(
        "windows",
        post_to_peer=post_win,
        helper_config=default_helper_config("windows", fake=True),
        path_settings=PathSettings(mode="auto"),
        path_probe=lambda _h, _p: (False, 999.0),
    )
    session = FakeLinkSession(vectors["link"])
    token = vectors["link"]["token"]
    mac.on_link_up("peer-win", token, "windows", True, session, outbound=True, peer_link_host="10.0.0.1")
    win.on_link_up("peer-mac", token, "macos", True, session, outbound=False, peer_link_host="10.0.0.2")

    def wrap_helper(svc, side):
        helper = svc._ensure_helper()
        orig = helper.request

        def recording(cmd, timeout=None, **fields):
            if cmd == "transport.peer":
                peer_cmds[side].append(dict(fields))
            return orig(cmd, timeout=timeout, **fields)

        helper.request = recording  # type: ignore[method-assign]
        return helper

    wrap_helper(mac, "mac")
    wrap_helper(win, "win")

    assert mac.toggle_front_window_mac("peer-win") is True
    offers = [m for _side, _out, m in inbox if read_tp(m) and read_tp(m)[0] == "tp_offer"]
    win.handle_link_message("peer-mac", offers[0], outbound=False)
    accepts = [m for _side, _out, m in inbox if read_tp(m) and read_tp(m)[0] == "tp_accept"]
    mac.handle_link_message("peer-win", accepts[0], outbound=True)

    offer_udp = read_tp(offers[0])[1]["udp_port"]
    accept_udp = read_tp(accepts[0])[1]["udp_port"]
    assert peer_cmds["win"] == [{"peer_host": "10.0.0.2", "peer_port": offer_udp}]
    assert peer_cmds["mac"] == [{"peer_host": "10.0.0.1", "peer_port": accept_udp}]

    mac.shutdown()
    win.shutdown()
