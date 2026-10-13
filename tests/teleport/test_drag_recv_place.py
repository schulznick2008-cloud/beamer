"""Kanten-Drag: recv.place nach recv.open; proxy.drag → tp_close return."""
import json
from pathlib import Path

from core.teleport_protocol import read_tp
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
    monkeypatch.setenv("BEAMER_FAKE_HELPER_PLATFORM", "win" if platform == "windows" else "mac")
    return default_helper_config(platform, fake=True)


def test_drag_offer_calls_recv_place(monkeypatch):
    with open(VECTORS, encoding="utf-8") as f:
        vectors = json.load(f)
    inbox = []
    places = []

    def post_win(_peer, msg, _out):
        inbox.append(msg)
        return True

    win = TeleportService("windows", post_to_peer=post_win, helper_config=_fake_cfg("windows", 47310, monkeypatch))
    session = FakeLinkSession(vectors["link"])
    win.on_link_up("peer-mac", vectors["link"]["token"], "macos", True, session, outbound=False)

    helper = win._ensure_helper()
    orig = helper.request

    def recording(cmd, timeout=None, **fields):
        if cmd == "recv.place":
            places.append(dict(fields))
        return orig(cmd, timeout=timeout, **fields)

    helper.request = recording  # type: ignore[method-assign]

    offer = {
        "type": "tp_offer",
        "data": {
            "tid": 99,
            "app": "App",
            "title": "T",
            "icon_png": "",
            "w": 400.0,
            "h": 300.0,
            "scale": 1.0,
            "codec": "hevc",
            "mode": "window",
            "udp_port": 47123,
            "salt": "AAAAAAAAAAAAAAAAAAAAAA==",
            "place_x": 120.0,
            "place_y": 80.0,
            "drag": True,
        },
    }
    win.handle_link_message("peer-mac", offer, outbound=False)
    assert places == [{"tid": 99, "x": 120.0, "y": 80.0, "dragging": True}]
    win.shutdown()


def test_proxy_drag_at_edge_sends_tp_close_return(monkeypatch):
    monkeypatch.setenv("BEAMER_TELEPORT_RETURN_MARGIN", "50")
    with open(VECTORS, encoding="utf-8") as f:
        vectors = json.load(f)
    posted = []

    def post_win(_peer, msg, _out):
        posted.append(msg)
        return True

    from core.teleport_bridge import _attach_win_proxy_drag

    win = TeleportService("windows", post_to_peer=post_win, helper_config=_fake_cfg("windows", 47311, monkeypatch))
    _attach_win_proxy_drag(win)
    session = FakeLinkSession(vectors["link"])
    win.on_link_up("peer-mac", vectors["link"]["token"], "macos", True, session, outbound=False)
    tid = 7
    with win._lock:
        from core.teleport_service import ActiveTeleport

        win._active[tid] = ActiveTeleport(
            tid=tid,
            peer_id="peer-mac",
            link_outbound=False,
            role="receiver",
            salt16=b"\x00" * 16,
            codec="hevc",
        )

    win._on_helper_event({"ev": "proxy.drag", "tid": tid, "x": 10, "y": 200, "dragging": True})
    closes = [m for m in posted if read_tp(m) and read_tp(m)[0] == "tp_close"]
    assert closes
    assert read_tp(closes[0])[1].get("reason") == "return"
    win.shutdown()
