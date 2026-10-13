import pytest

from core import ways


def test_teleport_hotkey_reserved_for_jump_keys():
    settings = {"peers": [{"id": "a", "port": 24820}]}
    with pytest.raises(ValueError, match="reserved for Teleport"):
        ways.check_jump_key(settings, "a", "ctrl+alt+cmd+t", "alt_r")
