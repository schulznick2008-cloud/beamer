"""Beamer-Link-Nachrichten für Teleport (SCHNITTSTELLEN.md §2)."""
from __future__ import annotations

import base64
import os
from typing import Any, Dict, Mapping, Optional

MSG_OFFER = "tp_offer"
MSG_ACCEPT = "tp_accept"
MSG_REJECT = "tp_reject"
MSG_INPUT = "tp_input"
MSG_RESIZE = "tp_resize"
MSG_FOCUS = "tp_focus"
MSG_UPDATE = "tp_update"
MSG_CLOSE = "tp_close"

CLOSE_REASONS = frozenset({"user", "gone", "error", "return"})
REJECT_REASONS = frozenset({"busy", "unsupported", "denied", "error"})

# Vorschlag: nicht im Vertrag; kollidiert nicht mit Standard-Shortcut (alt_r) noch typischen jump_keys.
DEFAULT_MAC_TELEPORT_HOTKEY = "ctrl+alt+cmd+t"


def _tid_ok(tid: Any) -> bool:
    return isinstance(tid, int) and 0 < tid <= 0xFFFFFFFF


def fresh_salt_b64() -> str:
    return base64.b64encode(os.urandom(16)).decode("ascii")


def offer_msg(
    tid: int,
    *,
    app: str,
    title: str,
    icon_png: str,
    w: float,
    h: float,
    scale: float,
    codec: str,
    mode: str,
    udp_port: int,
    salt_b64: str,
    place_x: Optional[float] = None,
    place_y: Optional[float] = None,
    drag: bool = False,
    title_bar: bool = False,
    dark: bool = True,
    lights: Optional[list] = None,
) -> dict:
    data = {
        "tid": tid,
        "app": app,
        "title": title,
        "icon_png": icon_png,
        "w": w,
        "h": h,
        "scale": scale,
        "codec": codec,
        "mode": mode,
        "udp_port": udp_port,
        "salt": salt_b64,
    }
    if place_x is not None and place_y is not None:
        data["place_x"] = place_x
        data["place_y"] = place_y
    if drag:
        data["drag"] = True
    if title_bar:
        data["title_bar"] = True
    data["dark"] = bool(dark)
    if lights:
        data["lights"] = list(lights)
    return {"type": MSG_OFFER, "data": data}


def accept_msg(tid: int, udp_port: int) -> dict:
    return {"type": MSG_ACCEPT, "data": {"tid": tid, "udp_port": udp_port}}


def reject_msg(tid: int, reason: str) -> dict:
    if reason not in REJECT_REASONS:
        raise ValueError(f"tp_reject reason must be one of {sorted(REJECT_REASONS)}")
    return {"type": MSG_REJECT, "data": {"tid": tid, "reason": reason}}


def input_msg(tid: int, input_obj: Mapping[str, Any]) -> dict:
    return {"type": MSG_INPUT, "data": {"tid": tid, "input": dict(input_obj)}}


def resize_msg(tid: int, w: float, h: float) -> dict:
    return {"type": MSG_RESIZE, "data": {"tid": tid, "w": w, "h": h}}


def focus_msg(tid: int) -> dict:
    return {"type": MSG_FOCUS, "data": {"tid": tid}}


def update_msg(tid: int, **fields: Any) -> dict:
    data = {"tid": tid}
    for key in ("title", "w", "h", "scale"):
        if key in fields:
            data[key] = fields[key]
    return {"type": MSG_UPDATE, "data": data}


def close_msg(tid: int, reason: str) -> dict:
    if reason not in CLOSE_REASONS:
        raise ValueError(f"unknown close reason: {reason!r}")
    return {"type": MSG_CLOSE, "data": {"tid": tid, "reason": reason}}


def read_tp(message: Mapping[str, Any]) -> Optional[tuple[str, dict]]:
    kind = message.get("type")
    data = message.get("data")
    if not isinstance(kind, str) or not isinstance(data, dict):
        return None
    if kind not in (
        MSG_OFFER,
        MSG_ACCEPT,
        MSG_REJECT,
        MSG_INPUT,
        MSG_RESIZE,
        MSG_FOCUS,
        MSG_UPDATE,
        MSG_CLOSE,
    ):
        return None
    tid = data.get("tid")
    if not _tid_ok(tid):
        return None
    if kind == MSG_CLOSE and data.get("reason") not in CLOSE_REASONS:
        return None
    if kind == MSG_INPUT and not isinstance(data.get("input"), dict):
        return None
    return kind, data


def choose_codec(helper_low_power: bool, encoder_slow: bool = False) -> str:
    """Codec-Wahl: H.264 bei low_power (§1.3/§1), sonst HEVC."""
    if helper_low_power or encoder_slow:
        return "h264"
    return "hevc"
