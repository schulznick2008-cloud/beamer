"""send.size vom Mac-Helfer wird als tp_update an den Proxy geschickt."""
import os

from core.teleport_protocol import MSG_UPDATE
from core.teleport_service import ActiveTeleport, TeleportService, default_helper_config


def test_send_size_posts_tp_update_in_points():
    posts = []

    def post(_peer, msg, outbound):
        posts.append((msg, outbound))
        return True

    os.environ["BEAMER_TELEPORT_FAKE_HELPER"] = "1"
    os.environ["BEAMER_FAKE_HELPER_LOCAL_PORT"] = "47321"
    svc = TeleportService("macos", post_to_peer=post, helper_config=default_helper_config("macos", fake=True))
    svc._active[9] = ActiveTeleport(
        tid=9,
        peer_id="pc",
        link_outbound=True,
        role="sender",
        salt16=b"\x00" * 16,
        codec="hevc",
    )
    svc._on_helper_event({"ev": "send.size", "tid": 9, "width": 1600, "height": 1200, "scale": 2.0})
    assert posts
    msg, outbound = posts[0]
    assert msg["type"] == MSG_UPDATE
    assert msg["data"]["tid"] == 9
    assert msg["data"]["w"] == 800.0
    assert msg["data"]["h"] == 600.0
    assert outbound is True
    svc.shutdown()
