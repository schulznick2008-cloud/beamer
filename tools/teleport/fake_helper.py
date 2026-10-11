#!/usr/bin/env python3
"""Fake-Helfer für Teleport-IPC-Tests (SCHNITTSTELLEN.md §1).

Simuliert JSON-Lines-Protokoll auf stdin/stdout. Steuerung über Umgebungsvariablen:

  BEAMER_FAKE_HELPER_MODE   normal | hang | crash | bad_line | busy (Standard: normal)
  BEAMER_FAKE_HELPER_HANG_CMD   Befehl, auf den nicht geantwortet wird (hang)
  BEAMER_FAKE_HELPER_CRASH_CMD  Befehl, nach dem der Prozess mit Code 1 endet
  BEAMER_FAKE_HELPER_BUSY_CMD   Befehl mit ok:false busy
"""
from __future__ import annotations

import json
import os
import sys
from typing import Any, Dict, Optional

V = 1


def _write(obj: Dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(obj, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def _error_event(code: str, msg: str, req_id: Optional[int] = None) -> None:
    if req_id is not None:
        _write({"v": V, "re": req_id, "ok": False, "error": {"code": code, "msg": msg}})
    else:
        _write({"v": V, "ev": "error", "code": code, "msg": msg})


def _handle(msg: Dict[str, Any], mode: str, hang_cmd: str, crash_cmd: str, busy_cmd: str) -> bool:
    """Returns False wenn der Prozess beendet werden soll."""
    if "cmd" not in msg:
        _error_event("bad_request", "cmd fehlt")
        return True
    req_id = msg.get("id")
    if not isinstance(req_id, int):
        _error_event("bad_request", "id fehlt oder ungültig")
        return True
    cmd = msg["cmd"]
    if mode == "bad_line" and cmd == "hello":
        sys.stdout.write("das ist keine json-zeile\n")
        sys.stdout.flush()
    if mode == "hang" and cmd == hang_cmd:
        return True
    if mode == "crash" and cmd == crash_cmd:
        os._exit(1)
    if mode == "busy" and cmd == busy_cmd:
        _write({"v": V, "re": req_id, "ok": False, "error": {"code": "busy", "msg": "simuliert busy"}})
        return True

    if cmd == "hello":
        _write(
            {
                "v": V,
                "re": req_id,
                "ok": True,
                "helper_version": "fake-0.1",
                "platform": "mac",
                "caps": ["send_window", "send_display", "recv", "inject", "audio"],
                "low_power": False,
            }
        )
    elif cmd == "transport.open":
        _write({"v": V, "re": req_id, "ok": True, "local_port": 47001})
    elif cmd == "transport.peer":
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "transport.close":
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "windows.list":
        _write(
            {
                "v": V,
                "re": req_id,
                "ok": True,
                "windows": [
                    {
                        "wid": 1,
                        "pid": 100,
                        "app": "FakeApp",
                        "title": "Test",
                        "x": 0,
                        "y": 0,
                        "w": 800,
                        "h": 600,
                        "display": 0,
                        "scale": 2.0,
                    }
                ],
            }
        )
    elif cmd == "send.start":
        _write({"v": V, "re": req_id, "ok": True, "width": 1600, "height": 1200, "scale": 2.0})
        _write({"v": V, "ev": "stats", "tid": msg.get("tid", 1), "fps": 30, "kbps": 5000,
                "rtt_ms": 2, "loss_pct": 0.0, "latency_ms": 0, "encoder_slow": False, "low_power": False})
    elif cmd == "send.stop":
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "shutdown":
        _write({"v": V, "re": req_id, "ok": True})
        return False
    elif cmd == "emit.gone":
        tid = msg.get("tid", 1)
        _write({"v": V, "ev": "send.gone", "tid": tid, "reason": "window_closed"})
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "emit.slow":
        tid = msg.get("tid", 1)
        _write(
            {
                "v": V,
                "ev": "stats",
                "tid": tid,
                "fps": 10,
                "kbps": 1000,
                "rtt_ms": 5,
                "loss_pct": 0.5,
                "latency_ms": 0,
                "encoder_slow": True,
                "low_power": True,
            }
        )
        _write({"v": V, "re": req_id, "ok": True})
    else:
        _write({"v": V, "re": req_id, "ok": False, "error": {"code": "unsupported", "msg": f"unbekannt: {cmd}"}})
    return True


def main() -> int:
    mode = os.environ.get("BEAMER_FAKE_HELPER_MODE", "normal")
    hang_cmd = os.environ.get("BEAMER_FAKE_HELPER_HANG_CMD", "hang.test")
    crash_cmd = os.environ.get("BEAMER_FAKE_HELPER_CRASH_CMD", "crash.test")
    busy_cmd = os.environ.get("BEAMER_FAKE_HELPER_BUSY_CMD", "busy.test")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            _error_event("bad_request", "kein gültiges JSON")
            continue
        if not isinstance(msg, dict):
            _error_event("bad_request", "JSON-Objekt erwartet")
            continue
        if msg.get("cmd") == "hang.test":
            continue
        if not _handle(msg, mode, hang_cmd, crash_cmd, busy_cmd):
            break
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
