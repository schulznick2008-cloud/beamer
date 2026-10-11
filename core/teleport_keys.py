"""Teleport-Schlüsselableitung (SCHNITTSTELLEN.md §4.2). Nur Stdlib + core.protocol.hkdf."""
from __future__ import annotations

import base64
import struct
from dataclasses import dataclass
from typing import Dict, Set, Tuple

from core import protocol

TELEPORT_MASTER_SALT = b"beamer-teleport-v1"


def link_handshake_transcript(
    key_id: bytes,
    prefix_i: bytes,
    prefix_r: bytes,
    share_i: bytes,
    share_r: bytes,
) -> bytes:
    """Handshake-Transkript T ohne ``beamer-frames`` und ohne Richtungs-Byte."""
    full = protocol.link_info(key_id, prefix_i, prefix_r, share_i, share_r, 1)
    if not full.startswith(protocol.KEY_INFO):
        raise ValueError("unexpected link_info layout")
    body = full[len(protocol.KEY_INFO) :]
    if not body or body[-1:] not in (b"\x01", b"\x02"):
        raise ValueError("missing direction byte in link_info")
    return body[:-1]


def derive_master(
    token: str,
    shared: bytes,
    key_id: bytes,
    prefix_i: bytes,
    prefix_r: bytes,
    share_i: bytes,
    share_r: bytes,
) -> bytes:
    t = link_handshake_transcript(key_id, prefix_i, prefix_r, share_i, share_r)
    return protocol.hkdf_sha256(protocol.link_ikm(token, shared), TELEPORT_MASTER_SALT, b"tp-master" + t)


@dataclass(frozen=True)
class StreamKeys:
    s2r_key: bytes
    s2r_prefix: bytes
    r2s_key: bytes
    r2s_prefix: bytes

    def as_ipc_dict(self) -> Dict[str, str]:
        return {
            "s2r_key": base64.b64encode(self.s2r_key).decode("ascii"),
            "s2r_prefix": base64.b64encode(self.s2r_prefix).decode("ascii"),
            "r2s_key": base64.b64encode(self.r2s_key).decode("ascii"),
            "r2s_prefix": base64.b64encode(self.r2s_prefix).decode("ascii"),
        }


def derive_stream_keys(master: bytes, salt16: bytes, tid: int) -> StreamKeys:
    if len(master) != 32:
        raise ValueError("master must be 32 bytes")
    if len(salt16) != 16:
        raise ValueError("salt16 must be 16 bytes")
    if tid == 0 or tid > 0xFFFFFFFF:
        raise ValueError("tid must be a non-zero u32")
    sid = struct.pack(">I", tid)
    hkdf_salt = salt16 + sid
    return StreamKeys(
        s2r_key=protocol.hkdf_sha256(master, hkdf_salt, b"tp-key-s2r"),
        r2s_key=protocol.hkdf_sha256(master, hkdf_salt, b"tp-key-r2s"),
        s2r_prefix=protocol.hkdf_sha256(master, hkdf_salt, b"tp-pfx-s2r")[:4],
        r2s_prefix=protocol.hkdf_sha256(master, hkdf_salt, b"tp-pfx-r2s")[:4],
    )


class TeleportKeySession:
    """Hält den Link-Master und verhindert Wiederverwendung desselben s2r-Schlüsselpaars."""

    def __init__(self, master: bytes) -> None:
        if len(master) != 32:
            raise ValueError("master must be 32 bytes")
        self._master = master
        self._used_s2r: Set[Tuple[bytes, bytes]] = set()

    @classmethod
    def from_link(
        cls,
        token: str,
        shared: bytes,
        key_id: bytes,
        prefix_i: bytes,
        prefix_r: bytes,
        share_i: bytes,
        share_r: bytes,
    ) -> TeleportKeySession:
        return cls(derive_master(token, shared, key_id, prefix_i, prefix_r, share_i, share_r))

    def allocate(self, salt16: bytes, tid: int) -> StreamKeys:
        keys = derive_stream_keys(self._master, salt16, tid)
        pair = (keys.s2r_key, keys.s2r_prefix)
        if pair in self._used_s2r:
            raise ValueError("s2r key/prefix pair already used in this session")
        self._used_s2r.add(pair)
        return keys
