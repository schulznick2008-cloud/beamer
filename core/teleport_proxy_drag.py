"""Proxy-Kanten-Drag (PC → Mac): ``proxy.drag``-Events, Rückgabe an der Übergangskante."""
from __future__ import annotations

import logging
from typing import Callable, Optional

ReturnEdgeCheck = Callable[[float, float], bool]


class ProxyDragController:
    """Wertet ``proxy.drag`` aus; bei Kante ``tp_close`` mit ``return``."""

    def __init__(
        self,
        on_return: Callable[[str, int], None],
        edge_at: Optional[ReturnEdgeCheck] = None,
        logger: Optional[logging.Logger] = None,
    ) -> None:
        self._on_return = on_return
        self._edge_at = edge_at
        self._log = logger or logging.getLogger(__name__)

    def set_edge_at(self, edge_at: Optional[ReturnEdgeCheck]) -> None:
        self._edge_at = edge_at

    def on_proxy_drag(self, peer_id: str, tid: int, ev: dict) -> None:
        if not ev.get("dragging"):
            return
        if self._edge_at is None:
            return
        try:
            x = float(ev.get("x", 0))
            y = float(ev.get("y", 0))
        except (TypeError, ValueError):
            return
        if not self._edge_at(x, y):
            return
        self._log.info("Proxy-Drag erreichte Rückgabekante (tid=%s)", tid)
        self._on_return(peer_id, tid)
