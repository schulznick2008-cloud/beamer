"""Teleport-Hotkey: kein blockierender Helfer-Aufruf im Event-Tap."""
from pathlib import Path


def test_handle_teleport_hotkey_enqueues_only():
    text = Path("mac_app/bridge.py").read_text(encoding="utf-8")
    idx = text.index("def _handle_teleport_hotkey")
    block = text[idx : idx + 2500]
    assert "toggle_front_window_mac" not in block
    assert "enqueue_teleport_hotkey" in block
