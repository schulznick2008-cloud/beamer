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
