"""Zwei Links (out/in): tp_* und Schlüssel nur über denselben Link."""
import json
import os
from pathlib import Path

from core.teleport_protocol import MSG_ACCEPT, MSG_OFFER, read_tp
from core.teleport_service import TeleportService, default_helper_config

ROOT = Path(__file__).resolve().parents[2]
VECTORS = ROOT / "docs" / "teleport" / "vectors.json"


class SessionStub:
    def teleport_handshake_material(self):
        link = json.loads(VECTORS.read_text())["link"]
        return (
            bytes.fromhex(link["shared_hex"]),
            bytes.fromhex(link["key_id_hex"]),
            bytes.fromhex(link["prefix_i_hex"]),
            bytes.fromhex(link["prefix_r_hex"]),
            bytes.fromhex(link["share_i_hex"]),
            bytes.fromhex(link["share_r_hex"]),
        )


def _fake_cfg(platform: str, port: int):
    os.environ["BEAMER_TELEPORT_FAKE_HELPER"] = "1"
    os.environ["BEAMER_FAKE_HELPER_LOCAL_PORT"] = str(port)
    return default_helper_config(platform, fake=True)


def _offer(tid=7, salt_b64="QUFBQUFBQUFBQUFBQUFBQUE="):
    return {
        "type": "tp_offer",
        "data": {
            "tid": tid,
            "app": "A",
            "title": "T",
            "icon_png": "",
            "w": 10,
            "h": 10,
            "scale": 1,
            "codec": "h264",
            "mode": "window",
            "udp_port": 47100,
            "salt": salt_b64,
        },
    }


def test_mac_posts_offer_on_outbound_link():
    posts = []

    def post(peer, msg, outbound):
        posts.append(outbound)
        return True

    mac = TeleportService("macos", post_to_peer=post, helper_config=_fake_cfg("macos", 47311))
    mac.on_link_up("pc", "tok", "windows", True, SessionStub(), outbound=True)
    mac.on_link_up("pc", "tok", "windows", True, SessionStub(), outbound=False)
    mac._start_outbound_mac(
        "pc",
        outbound=True,
        win={"wid": 1, "app": "A", "title": "T", "w": 100, "h": 100, "scale": 1.0},
    )
    assert posts == [True]
    mac.shutdown()


def test_win_offer_on_wrong_link_ignored():
    posts = []

    def post(peer, msg, outbound):
        posts.append((outbound, msg.get("type")))
        return True

    win = TeleportService("windows", post_to_peer=post, helper_config=_fake_cfg("win", 47312))
    win.on_link_up("mac", "tok", "macos", True, SessionStub(), outbound=False)
    win.handle_link_message("mac", _offer(tid=8), outbound=True)
    assert posts == []
    win.handle_link_message("mac", _offer(tid=9, salt_b64="QkJCQkJCQkJCQkJCQkJCQg=="), outbound=False)
    assert any(t == MSG_ACCEPT for _, t in posts)
    win.shutdown()


def test_duplicate_offer_rejected_with_error():
    posts = []

    def post(peer, msg, outbound):
        posts.append(msg)
        return True

    win = TeleportService("windows", post_to_peer=post, helper_config=_fake_cfg("win", 47313))
    win.on_link_up("mac", "tok", "macos", True, SessionStub(), outbound=False)
    offer = _offer()
    win.handle_link_message("mac", offer, outbound=False)
    win.handle_link_message("mac", offer, outbound=False)
    rejects = [m for m in posts if m.get("type") == "tp_reject"]
    assert rejects
    assert rejects[-1]["data"]["reason"] == "error"
    win.shutdown()
