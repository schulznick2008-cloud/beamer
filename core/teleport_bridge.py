"""Anbindung von TeleportService an Mac- und Windows-Beamer."""
from __future__ import annotations

import logging
import queue
import threading
from typing import Callable

from core import protocol
from core.teleport_mac_pick import pick_frontmost_window
from core.teleport_protocol import read_tp
from core.teleport_service import TeleportService, default_helper_config

log = logging.getLogger(__name__)


def enable_teleport_identity(identity: dict) -> dict:
    out = dict(identity)
    out["teleport"] = True
    return out


def attach_mac_controller(controller) -> TeleportService:
    """Registriert Teleport auf dem Mac-KVMController."""

    def post(peer_id: str, message: dict, outbound: bool) -> bool:
        if outbound:
            link = controller._peers_up.get(peer_id)
            if link is None or not link.live():
                return False
            return link.post(message)
        responder = controller.responder
        ident = protocol.read_id(peer_id)
        if responder is None or ident is None:
            return False
        return responder.send(ident, message)

    def pick_front():
        svc = getattr(controller, "teleport", None)
        if svc is None:
            return None
        helper = svc._ensure_helper()
        return pick_frontmost_window(lambda: helper.request("windows.list"))

    service = TeleportService(
        "macos",
        post_to_peer=post,
        helper_config=default_helper_config("macos"),
        key_style=getattr(controller.cfg, "key_map", "semantic"),
        logger=controller.logger,
        pick_front_window=pick_front,
    )
    controller.teleport = service
    _wrap_mac_link_handlers(controller, service)
    _attach_mac_edge_drag(controller, service)
    _attach_teleport_hotkey_worker(controller, service)
    return service


def _peer_link_host(link) -> str:
    dialled = getattr(link, "dialled", None)
    if dialled:
        return str(dialled[0])
    host = getattr(link, "host", None)
    if host:
        return str(host)
    entry_fn = getattr(link, "entry", None)
    if entry_fn is not None:
        ent = entry_fn() or {}
        h = ent.get("host")
        if h:
            return str(h)
    return ""


def _attach_teleport_hotkey_worker(controller, service: TeleportService) -> None:
    pending: "queue.SimpleQueue[object]" = queue.SimpleQueue()
    controller._teleport_hotkey_queue = pending

    def run() -> None:
        while True:
            item = pending.get()
            if item is None:
                break
            try:
                service.toggle_front_window_mac()
            except Exception:
                controller.logger.exception("Teleport-Hotkey fehlgeschlagen")

    threading.Thread(target=run, name="teleport-hotkey", daemon=True).start()


def enqueue_teleport_hotkey(controller) -> None:
    """Event-Tap: nur in die Queue legen, kein Helfer-IPC."""
    q = getattr(controller, "_teleport_hotkey_queue", None)
    if q is not None:
        q.put(True)


def _attach_mac_edge_drag(controller, service: TeleportService) -> None:
    from core.teleport_edge_drag import EdgeDragController

    def start(intent):
        return service.start_drag_teleport_mac(
            intent.peer_id, intent.wid, intent.place_x, intent.place_y
        )

    drag = EdgeDragController(start, logger=controller.logger)
    controller.edge_drag = drag
    service.set_drag_state_handler(drag.on_drag_state)


def try_queue_edge_drag_teleport(controller, peer_id, edge, x, y, dragging) -> bool:
    """Aus dem Event-Tap: nur nicht-blockierende Queue; Ausnahmen werden geloggt."""
    edge_drag = getattr(controller, "edge_drag", None)
    if edge_drag is None or not peer_id:
        return False
    try:
        return edge_drag.queue_edge_cross(peer_id, edge, x, y, dragging)
    except Exception:
        controller.logger.exception("Kanten-Drag-Anfrage im Event-Tap fehlgeschlagen")
        return False


def _wrap_mac_link_handlers(controller, service: TeleportService) -> None:
    orig_up = controller._link_up
    orig_msg = controller._link_message
    orig_drop = controller._drop_up

    def link_up(link, fields):
        orig_up(link, fields)
        peer = protocol.id_text(fields["id"])
        session = link.session
        if session is not None:
            service.on_link_up(
                peer,
                link.token,
                fields.get("platform", ""),
                bool(fields.get("teleport")) and bool(controller.identity().get("teleport")),
                session,
                outbound=True,
                peer_link_host=_peer_link_host(link),
            )

    def link_message(link, message):
        if read_tp(message) is not None:
            service.handle_link_message(link.peer_id, message, outbound=True)
            return
        orig_msg(link, message)

    def drop_up(link):
        peer = link.peer_id
        if peer:
            service.on_link_down(peer, outbound=True)
        orig_drop(link)

    controller._link_up = link_up
    controller._link_message = link_message
    controller._drop_up = drop_up


def wire_responder_teleport(responder, service: TeleportService, token_for_peer: Callable[[str], str]) -> None:
    """Eingehende Links auf dem Responder (Gegenstelle hat den Link geöffnet)."""
    orig_cb = responder._callbacks.get("link")

    def link_cb(peer_id, up):
        if orig_cb is not None:
            orig_cb(peer_id, up)
        if not up:
            service.on_link_down(peer_id, outbound=False)
            return
        link = responder._links.get(peer_id)
        if link is None:
            return
        service.on_link_up(
            peer_id,
            token_for_peer(peer_id),
            link.peer_platform,
            link.peer_teleport and bool(responder._identity().get("teleport")),
            link.session,
            outbound=False,
            peer_link_host=_peer_link_host(link),
        )

    responder._callbacks["link"] = link_cb


def attach_win_sender(sender, identity_fn: Callable[[], dict], server=None) -> TeleportService:
    def post(peer_id: str, message: dict, outbound: bool) -> bool:
        peer = protocol.read_id(peer_id)
        if peer is None:
            return False
        if outbound:
            return sender.send_to(peer, message)
        if server is None:
            return False
        return server.send(peer, message)

    service = TeleportService(
        "windows",
        post_to_peer=post,
        helper_config=default_helper_config("windows"),
        logger=logging.getLogger("Beamer"),
    )
    sender.teleport = service

    orig_msg = sender.on_message

    def on_message(peer, message, began_at=None):
        if read_tp(message) is not None:
            service.handle_link_message(protocol.id_text(peer), message, outbound=True)
            return
        orig_msg(peer, message, began_at)

    sender.on_message = on_message

    orig_link = sender.on_link

    def on_link(peer, up, accepts=False):
        orig_link(peer, up, accepts)
        peer_text = protocol.id_text(peer)
        if not up:
            service.on_link_down(peer_text, outbound=True)
            return
        link = sender.links._by_peer.get(peer)
        session = link.session if link is not None else None
        if link is not None and session is not None:
            service.on_link_up(
                peer_text,
                link.token,
                link.peer_platform,
                bool(getattr(link, "_peer_teleport", False)) and bool(identity_fn().get("teleport")),
                session,
                outbound=True,
                peer_link_host=_peer_link_host(link),
            )

    sender.on_link = on_link
    service.set_return_edge_probe(lambda peer_id, x, y: win_at_return_edge(sender, peer_id, x, y))
    return service


def win_at_return_edge(sender, peer_id: str, x: float, y: float) -> bool:
    """Liegt der Punkt (physische Pixel) an der Kante, über die dieser PC zu ``peer_id`` wechselt?
    Nutzt dieselben Kantenmodelle wie Beamers eigener Wechsel (``sender._models``), nur ohne Druck: der
    Proxy ist an die Wand gezogen, das genügt."""
    try:
        monitors = sender._cached_monitors()
        display_map = sender._cached_display_map()
        point = (int(round(x)), int(round(y)))
        for peer, model in list(getattr(sender, "_models", [])):
            if protocol.id_text(peer) != peer_id:
                continue
            if model._at_edge(monitors, point, display_map):
                return True
    except Exception:
        log.exception("Kantenprüfung fehlgeschlagen")
    return False
