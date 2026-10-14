"""Teleport: Schlüssel und tp_* strikt pro Beamer-Link (nicht pro Peer-ID)."""
from __future__ import annotations

from typing import Tuple, Union

from core import protocol

# (peer_id, outbound_from_this_machine)
LinkRef = Tuple[str, bool]

PeerIdLike = Union[str, bytes]


def teleport_peer_id(peer_id: PeerIdLike) -> str:
    """Kanonical wire text (wie Mac-outbound), auch wenn der Responder bytes übergibt."""
    if isinstance(peer_id, bytes):
        return protocol.peer_id_text(peer_id)
    if isinstance(peer_id, str):
        if protocol.read_id(peer_id) is not None:
            return peer_id
        return peer_id
    return protocol.peer_id_text(peer_id)


def link_ref(peer_id: PeerIdLike, outbound: bool) -> LinkRef:
    return (teleport_peer_id(peer_id), outbound)
