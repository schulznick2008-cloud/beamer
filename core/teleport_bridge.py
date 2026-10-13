"""Anbindung von TeleportService an Mac- und Windows-Beamer."""
from __future__ import annotations

import logging
from typing import Callable

from core import protocol
from core.teleport_protocol import read_tp
from core.teleport_service import TeleportService, default_helper_config

log = logging.getLogger(__name__)


def enable_teleport_identity(identity: dict) -> dict:
    out = dict(identity)
    out["teleport"] = True
    return out


def attach_mac_controller(controller) -> TeleportService:
    """Registriert Teleport auf dem Mac-KVMController."""

    def post(peer_id: str, message: dict) -> bool:
        link = controller._peers_up.get(peer_id)
        if link is None or not link.live():
            return False
        return link.post(message)

    service = TeleportService(
        "macos",
        post_to_peer=post,
        helper_config=default_helper_config("macos"),
        key_style=getattr(controller.cfg, "key_map", "semantic"),
        logger=controller.logger,
    )
    controller.teleport = service
    _wrap_mac_link_handlers(controller, service)
    return service


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
            )

    def link_message(link, message):
        if read_tp(message) is not None:
            service.handle_link_message(link.peer_id, message)
            return
        orig_msg(link, message)

    def drop_up(link):
        peer = link.peer_id
        if peer:
            service.on_link_down(peer)
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
            service.on_link_down(peer_id)
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
        )

    responder._callbacks["link"] = link_cb


def attach_win_sender(sender, identity_fn: Callable[[], dict]) -> TeleportService:
    def post(peer_id: str, message: dict) -> bool:
        peer = protocol.read_id(peer_id)
        if peer is None:
            return False
        return sender.send_to(peer, message)

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
            service.handle_link_message(protocol.id_text(peer), message)
            return
        orig_msg(peer, message, began_at)

    sender.on_message = on_message

    orig_link = sender.on_link

    def on_link(peer, up, accepts=False):
        orig_link(peer, up, accepts)
        peer_text = protocol.id_text(peer)
        if not up:
            service.on_link_down(peer_text)
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
            )

    sender.on_link = on_link
    return service
