"""Die Tastentabelle des Windows-Helfers (teleport/win/src/keytable_gen.rs) muss aus Beamers Tabellen erzeugt
und aktuell sein (tools/teleport/gen_rust_keytable.py, Muster wie gen_swift_keycodes.py fuer den Mac)."""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GEN = os.path.join(ROOT, "tools", "teleport", "gen_rust_keytable.py")
OUT = os.path.join(ROOT, "teleport", "win", "src", "keytable_gen.rs")


def test_generated_table_is_current():
    r = subprocess.run([sys.executable, GEN, "--check"], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


def test_names_are_physical_and_valid():
    from core.keytable import SPEC_NAMES

    rows = dict(
        (int(vk, 16), name)
        for vk, name in re.findall(r'\(0x([0-9A-F]+), "([a-z0-9_]+)"\)', open(OUT, encoding="utf-8").read())
    )
    assert len(rows) > 50
    assert set(rows.values()) <= SPEC_NAMES
    # Roh/physisch (SCHNITTSTELLEN.md §3): Strg-Taste = "ctrl", Windows-Taste = "cmd".
    assert rows[0xA2] == "ctrl" and rows[0xA3] == "ctrl_r"
    assert rows[0x5B] == "cmd" and rows[0x5C] == "cmd_r"
    assert rows[0xA4] == "alt" and rows[0x0D] == "enter" and rows[0x70] == "f1"
