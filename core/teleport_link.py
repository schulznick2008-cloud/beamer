"""Teleport: Schlüssel und tp_* strikt pro Beamer-Link (nicht pro Peer-ID)."""
from __future__ import annotations

from typing import Tuple

# (peer_id, outbound_from_this_machine)
LinkRef = Tuple[str, bool]


def link_ref(peer_id: str, outbound: bool) -> LinkRef:
    return (peer_id, outbound)
