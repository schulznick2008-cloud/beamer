"""Tests für core.teleport_keys gegen docs/teleport/vectors.json."""
import json
import os

import pytest

from core.teleport_keys import (
    TeleportKeySession,
    derive_master,
    derive_stream_keys,
    link_handshake_transcript,
)

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
VECTORS = os.path.join(ROOT, "docs", "teleport", "vectors.json")


@pytest.fixture(scope="module")
def vectors():
    with open(VECTORS, encoding="utf-8") as f:
        return json.load(f)


def test_link_transcript_and_master(vectors):
    link = vectors["link"]
    key_id = bytes.fromhex(link["key_id_hex"])
    parts = [
        bytes.fromhex(link["prefix_i_hex"]),
        bytes.fromhex(link["prefix_r_hex"]),
        bytes.fromhex(link["share_i_hex"]),
        bytes.fromhex(link["share_r_hex"]),
    ]
    shared = bytes.fromhex(link["shared_hex"])
    t = link_handshake_transcript(key_id, *parts)
    assert t.hex() == link["transcript_t_hex"]
    master = derive_master(link["token"], shared, key_id, *parts)
    assert master.hex() == link["master_hex"]


def test_stream_keys_match_vectors(vectors):
    link = vectors["link"]
    master = bytes.fromhex(link["master_hex"])
    for case in vectors["streams"]:
        salt16 = bytes.fromhex(case["salt16_hex"])
        tid = case["tid"]
        keys = derive_stream_keys(master, salt16, tid)
        assert keys.s2r_key.hex() == case["s2r_key_hex"]
        assert keys.s2r_prefix.hex() == case["s2r_prefix_hex"]
        assert keys.r2s_key.hex() == case["r2s_key_hex"]
        assert keys.r2s_prefix.hex() == case["r2s_prefix_hex"]
        assert keys.as_ipc_dict() == case["keys_ipc"]


def test_session_rejects_duplicate_s2r_pair(vectors):
    link = vectors["link"]
    key_id = bytes.fromhex(link["key_id_hex"])
    parts = [
        bytes.fromhex(link["prefix_i_hex"]),
        bytes.fromhex(link["prefix_r_hex"]),
        bytes.fromhex(link["share_i_hex"]),
        bytes.fromhex(link["share_r_hex"]),
    ]
    shared = bytes.fromhex(link["shared_hex"])
    session = TeleportKeySession.from_link(link["token"], shared, key_id, *parts)
    salt16 = bytes.fromhex(vectors["streams"][0]["salt16_hex"])
    session.allocate(salt16, 1)
    with pytest.raises(ValueError, match="already used"):
        session.allocate(salt16, 1)


def test_different_tid_yields_different_keys(vectors):
    master = bytes.fromhex(vectors["link"]["master_hex"])
    salt16 = bytes.fromhex(vectors["streams"][0]["salt16_hex"])
    a = derive_stream_keys(master, salt16, 1)
    b = derive_stream_keys(master, salt16, 2)
    assert a.s2r_key != b.s2r_key
