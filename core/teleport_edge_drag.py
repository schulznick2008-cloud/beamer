"""Kanten-Drag-Teleport: nur mit Helfer-``windows.drag_state`` (asynchron), nie im Event-Tap."""
from __future__ import annotations

import logging
import queue
import threading
from dataclasses import dataclass
from typing import Callable, Optional


@dataclass
class DragTeleportIntent:
    peer_id: str
    wid: int
    place_x: float
    place_y: float
    edge: str


class EdgeDragController:
    """Aktiv nur wenn der Helfer ``ev: windows.drag_state`` mit ``title_bar`` und ``wid`` meldet."""

    def __init__(
        self,
        start_teleport: Callable[[DragTeleportIntent], bool],
        logger: Optional[logging.Logger] = None,
    ) -> None:
        self._start = start_teleport
        self._log = logger or logging.getLogger(__name__)
        self._lock = threading.Lock()
        self._dragging = False
        self._title_bar = False
        self._wid: Optional[int] = None
        self._cursor: Optional[tuple] = None
        self._pending: "queue.SimpleQueue[Optional[tuple]]" = queue.SimpleQueue()
        self._worker = threading.Thread(target=self._run, name="teleport-edge-drag", daemon=True)
        self._worker.start()

    def on_drag_state(self, ev: dict) -> None:
        """Vom Helfer-IPC (Hintergrundthread), nicht aus dem Event-Tap."""
        with self._lock:
            self._dragging = bool(ev.get("dragging"))
            self._title_bar = bool(ev.get("title_bar"))
            wid = ev.get("wid")
            self._wid = int(wid) if wid is not None else None
            if not (self._dragging and self._title_bar and self._wid):
                self._wid = None
            # Cursor des Helfers (globale Punkte) zuletzt gemeldet; ohne x/y bleibt der alte Wert stehen.
            x, y = ev.get("x"), ev.get("y")
            if isinstance(x, (int, float)) and isinstance(y, (int, float)):
                self._cursor = (float(x), float(y))
            if not self._dragging:
                self._cursor = None

    def cursor(self) -> Optional[tuple]:
        """Letzte vom Helfer gemeldete Cursorposition (x, y) waehrend eines Titelleisten-Zugs, sonst ``None``."""
        with self._lock:
            return self._cursor

    def is_armed(self) -> bool:
        with self._lock:
            return bool(self._dragging and self._title_bar and self._wid)

    def queue_edge_cross(
        self,
        peer_id: str,
        edge: str,
        cursor_x: float,
        cursor_y: float,
        dragging: bool,
    ) -> bool:
        """Nur im Event-Tap: speichert Intent oder ignoriert. Kein IPC, kein Blockieren."""
        if not dragging or not peer_id:
            return False
        if not self.is_armed():
            return False
        with self._lock:
            wid = self._wid
        if wid is None:
            return False
        self._pending.put((peer_id, edge, cursor_x, cursor_y, wid))
        return True

    def _run(self) -> None:
        while True:
            item = self._pending.get()
            if item is None:
                break
            peer_id, edge, x, y, wid = item
            try:
                self._start(
                    DragTeleportIntent(peer_id=peer_id, wid=wid, place_x=x, place_y=y, edge=edge)
                )
            except Exception:
                self._log.exception("Kanten-Drag-Teleport fehlgeschlagen")

    def shutdown(self) -> None:
        self._pending.put(None)
