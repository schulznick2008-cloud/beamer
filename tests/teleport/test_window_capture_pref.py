"""send.start: Fenster-Capture statt virtueller Monitor (Standard)."""
import json
import os
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


def _fake_cfg(monkeypatch, port: int):
    monkeypatch.setenv("BEAMER_TELEPORT_FAKE_HELPER", "1")
    monkeypatch.setenv("BEAMER_FAKE_HELPER_LOCAL_PORT", str(port))
    monkeypatch.delenv("BEAMER_TELEPORT_VIRTUAL_DISPLAY", raising=False)
    return default_helper_config("macos", fake=True)


def test_send_start_uses_window_capture_by_default(monkeypatch):
    vectors = json.loads(VECTORS.read_text())
    calls = []

    mac = TeleportService("macos", post_to_peer=lambda *_: True, helper_config=_fake_cfg(monkeypatch, 47601))
    session = FakeLinkSession(vectors["link"])
    mac.on_link_up("peer-win", vectors["link"]["token"], "windows", True, session, outbound=True)
    helper = mac._ensure_helper()
    orig = helper.request

    def recording(cmd, timeout=None, **fields):
        if cmd == "send.start":
            calls.append(dict(fields))
        return orig(cmd, timeout=timeout, **fields)

    helper.request = recording  # type: ignore[method-assign]

    with mac._lock:
        from core.teleport_service import ActiveTeleport

        mac._active[3] = ActiveTeleport(
            tid=3,
            peer_id="peer-win",
            link_outbound=True,
            role="sender",
            salt16=b"\x00" * 16,
            codec="hevc",
            peer_udp=47100,
        )
    mac._handle_accept("peer-win", {"tid": 3, "udp_port": 47100}, outbound=True)
    assert calls
    assert calls[0].get("virtual_display") is False
    mac.shutdown()


def test_virtual_display_env_enables_virtual_monitor(monkeypatch):
    monkeypatch.setenv("BEAMER_TELEPORT_VIRTUAL_DISPLAY", "1")
    svc = TeleportService("macos", post_to_peer=lambda *_: True)
    assert svc._prefer_virtual_display() is True
    monkeypatch.delenv("BEAMER_TELEPORT_VIRTUAL_DISPLAY", raising=False)
    assert svc._prefer_virtual_display() is False
