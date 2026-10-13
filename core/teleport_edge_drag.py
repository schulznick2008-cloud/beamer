"""Kanten-Drag-Teleport (Python-Kern): Fenster an Titelleiste über Beamer-Kante ziehen."""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Optional


@dataclass
class DragTeleportIntent:
    peer_id: str
    wid: int
    place_x: float
    place_y: float
    edge: str


class EdgeDragController:
    """Erkennt Ziehen mit gedrückter Taste Richtung Übergangskante; startet tp_offer mit Platzierung."""

    def __init__(
        self,
        start_teleport: Callable[[DragTeleportIntent], bool],
        *,
        is_title_drag: Optional[Callable[[], bool]] = None,
    ) -> None:
        self._start = start_teleport
        self._is_title_drag = is_title_drag or (lambda: False)
        self._drag_wid: Optional[int] = None
        self._button_down = False

    def on_button(self, down: bool, wid: Optional[int] = None) -> None:
        self._button_down = down
        if down and wid is not None:
            self._drag_wid = wid
        if not down:
            self._drag_wid = None

    def on_edge_cross(
        self,
        peer_id: str,
        edge: str,
        cursor_x: float,
        cursor_y: float,
        dragging: bool,
    ) -> bool:
        """True wenn Teleport gestartet wurde (Kante überschritten, Titelleisten-Drag)."""
        if not dragging or not self._button_down:
            return False
        if not self._is_title_drag():
            return False
        wid = self._drag_wid
        if wid is None:
            return False
        intent = DragTeleportIntent(
            peer_id=peer_id,
            wid=int(wid),
            place_x=cursor_x,
            place_y=cursor_y,
            edge=edge,
        )
        return self._start(intent)


def win_title_bar_drag_from_hook(message: int) -> bool:
    """Windows: WM_NCLBUTTONDOWN (0x00A1) o. ä. – Platzhalter bis C05/Win-Helfer meldet."""
    return message == 0x00A1
