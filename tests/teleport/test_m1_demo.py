"""CI-Test für tools/teleport/m1_demo.py (--selftest-fake)."""
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
M1 = ROOT / "tools" / "teleport" / "m1_demo.py"


def test_m1_demo_selftest_fake_json_summary():
    proc = subprocess.run(
        [sys.executable, str(M1), "--selftest-fake", "--seconds", "2"],
        cwd=str(ROOT),
        env={**os.environ, "PYTHONPATH": str(ROOT)},
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert proc.returncode == 0, proc.stderr or proc.stdout
    marker = "---M1_JSON---"
    assert marker in proc.stdout, proc.stdout
    report = json.loads(proc.stdout.split(marker, 1)[1].strip())
    assert report["mode"] == "selftest-fake"
    assert report["pc"]["samples"] >= 1
    assert report["mac"]["samples"] >= 1


def test_pc_run_uses_fresh_salt_each_time(tmp_path):
    """Nie salt16 aus einer alten Session-Datei uebernehmen (Nonce-Wiederverwendung, §4.2)."""
    sys.path.insert(0, str(ROOT))
    from core.teleport_ipc import TeleportHelper
    from core.teleport_keys import TeleportKeySession
    from tools.teleport import m1_demo as m

    session_path = tmp_path / "s.json"
    stale = "00" * 16
    salts = []
    for _ in range(2):
        session = {"salt16_hex": stale}
        h = TeleportHelper(m._helper_config("pc", None, True, 47110))
        h.start()
        try:
            m.run_pc(h, "127.0.0.1", session, session_path, 1, "h264", 0.2,
                     TeleportKeySession(bytes(32)), m.StatsSnapshot(), local_port=47110)
        finally:
            h.stop()
        salts.append(session["salt16_hex"])
    assert stale not in salts
    assert salts[0] != salts[1]
