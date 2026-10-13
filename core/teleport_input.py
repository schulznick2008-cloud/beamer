"""Eingabe-Umsetzung für Teleport (§3): Modifier Cmd↔Strg vor inject."""
from __future__ import annotations

from typing import Any, Dict, Mapping, Union

from core import keytable

_MODIFIER_KEYS = frozenset({"ctrl", "ctrl_r", "cmd", "cmd_r", "alt", "alt_r", "shift", "shift_r"})


def map_key_name(
    name: str,
    from_platform: str,
    to_platform: str,
    key_style: Union[str, Mapping[str, str]],
) -> str:
    if name in _MODIFIER_KEYS:
        return keytable.wire_name(name, from_platform, to_platform, key_style)
    return name


def map_input_for_inject(
    input_obj: Mapping[str, Any],
    from_platform: str,
    to_platform: str,
    key_style: Union[str, Mapping[str, str]] = "semantic",
) -> Dict[str, Any]:
    """Kopie von `input` mit gemappten `key`-Namen für den Sender-Helfer."""
    out = dict(input_obj)
    kind = out.get("k")
    if kind == "key" and isinstance(out.get("key"), str):
        out["key"] = map_key_name(out["key"], from_platform, to_platform, key_style)
    return out
