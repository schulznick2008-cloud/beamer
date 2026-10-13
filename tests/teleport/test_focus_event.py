"""focus_msg bei proxy.focus (kein NameError)."""
import os

from core.teleport_service import TeleportService, default_helper_config


def test_proxy_focus_sends_focus_msg():
    posts = []

    def post(peer, msg, outbound):
        posts.append((msg.get("type"), outbound))
        return True

    os.environ["BEAMER_TELEPORT_FAKE_HELPER"] = "1"
    os.environ["BEAMER_FAKE_HELPER_LOCAL_PORT"] = "47320"
    svc = TeleportService("windows", post_to_peer=post, helper_config=default_helper_config("win", fake=True))
    svc._active[1] = type(
        "T",
        (),
        {
            "tid": 1,
            "peer_id": "mac",
            "link_outbound": False,
            "role": "receiver",
            "salt16": b"\x00" * 16,
            "codec": "h264",
            "proxy_focused": False,
        },
    )()
    svc._on_helper_event({"ev": "proxy.focus", "tid": 1, "focused": True})
    assert ("tp_focus", False) in posts
    svc.shutdown()
