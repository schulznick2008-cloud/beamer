#!/usr/bin/env python3
"""Fake-Helfer für Teleport-IPC-Tests (SCHNITTSTELLEN.md §1).

Simuliert JSON-Lines-Protokoll auf stdin/stdout. Steuerung über Umgebungsvariablen:

  BEAMER_FAKE_HELPER_MODE   normal | hang | crash | bad_line | busy (Standard: normal)
  BEAMER_FAKE_HELPER_HANG_CMD   Befehl, auf den nicht geantwortet wird (hang)
  BEAMER_FAKE_HELPER_CRASH_CMD  Befehl, nach dem der Prozess mit Code 1 endet
  BEAMER_FAKE_HELPER_BUSY_CMD   Befehl mit ok:false busy
  BEAMER_FAKE_HELPER_PLATFORM   mac | win (hello/stats)
  BEAMER_FAKE_HELPER_LOCAL_PORT UDP-Port für transport.open
  BEAMER_FAKE_HELPER_RACE_CMD   Antwort verzögert (Race mit Timeout)
"""
from __future__ import annotations

import json
import os
import sys
import threading
import time
from typing import Any, Dict, Optional

V = 1
_BUSY_REMAINING: Optional[int] = None


def _write(obj: Dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(obj, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def _error_event(code: str, msg: str, req_id: Optional[int] = None) -> None:
    if req_id is not None:
        _write({"v": V, "re": req_id, "ok": False, "error": {"code": code, "msg": msg}})
    else:
        _write({"v": V, "ev": "error", "code": code, "msg": msg})


def _flood_stderr() -> None:
    line = "E" * 512 + "\n"
    total = 0
    target = int(os.environ.get("BEAMER_FAKE_HELPER_STDERR_MB", "1")) * 1024 * 1024
    while total < target:
        sys.stderr.write(line)
        sys.stderr.flush()
        total += len(line.encode("utf-8"))


def _handle(
    msg: Dict[str, Any],
    mode: str,
    hang_cmd: str,
    crash_cmd: str,
    busy_cmd: str,
    late_cmd: str,
    late_delay: float,
    race_cmd: str,
    race_delay: float,
    platform: str,
    local_port: int,
) -> bool:
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
        count_env = os.environ.get("BEAMER_FAKE_HELPER_BUSY_COUNT")
        if count_env not in (None, ""):
            global _BUSY_REMAINING
            if _BUSY_REMAINING is None:
                _BUSY_REMAINING = int(count_env)
            if _BUSY_REMAINING > 0:
                _BUSY_REMAINING -= 1
                _write({"v": V, "re": req_id, "ok": False, "error": {"code": "busy", "msg": "simuliert busy"}})
                return True
            _write({"v": V, "re": req_id, "ok": True, "local_port": 47001})
            return True
        _write({"v": V, "re": req_id, "ok": False, "error": {"code": "busy", "msg": "simuliert busy"}})
        return True
    if mode == "race" and cmd == race_cmd:
        rid = req_id

        def race_reply() -> None:
            time.sleep(race_delay)
            _write({"v": V, "re": rid, "ok": True, "local_port": local_port})

        threading.Thread(target=race_reply, daemon=True).start()
        return True
    if mode == "late" and cmd == late_cmd:
        rid = req_id

        def delayed() -> None:
            time.sleep(late_delay)
            _write({"v": V, "re": rid, "ok": True, "late": True})

        threading.Thread(target=delayed, daemon=True).start()
        return True

    if cmd == "hello":
        _write(
            {
                "v": V,
                "re": req_id,
                "ok": True,
                "helper_version": "fake-0.1",
                "platform": platform,
                "caps": ["send_window", "send_display", "recv", "inject", "audio"],
                "low_power": False,
            }
        )
    elif cmd == "transport.open":
        _write({"v": V, "re": req_id, "ok": True, "local_port": local_port})
    elif cmd == "transport.peer":
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "transport.close":
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "windows.chrome":
        _write({"v": V, "re": req_id, "ok": True, "title_bar": False, "dark": True})
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
                        "title": os.environ.get(
                            "BEAMER_FAKE_HELPER_TITLE", "Fenster – Übersicht"
                        ),
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
        tid = msg.get("tid", 1)
        _write({"v": V, "re": req_id, "ok": True, "width": 1600, "height": 1200, "scale": 2.0})
        _write(
            {
                "v": V,
                "ev": "stats",
                "tid": tid,
                "fps": 30,
                "kbps": 5000,
                "rtt_ms": 2,
                "loss_pct": 0.0,
                "latency_ms": 0,
                "encoder_slow": False,
                "low_power": False,
            }
        )
        _emit_stats(tid, platform, role="send")
    elif cmd == "recv.open":
        tid = msg.get("tid", 1)
        _write({"v": V, "re": req_id, "ok": True})
        _emit_stats(tid, platform, role="recv")
    elif cmd == "recv.place":
        _write({"v": V, "re": req_id, "ok": True, "placed": True})
    elif cmd == "emit.proxy_drag":
        tid = msg.get("tid", 1)
        _write(
            {
                "v": V,
                "ev": "proxy.drag",
                "tid": tid,
                "x": msg.get("x", 0),
                "y": msg.get("y", 0),
                "dragging": bool(msg.get("dragging", True)),
            }
        )
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "emit.drag_state":
        _write(
            {
                "v": V,
                "ev": "windows.drag_state",
                "dragging": bool(msg.get("dragging", True)),
                "title_bar": bool(msg.get("title_bar", True)),
                "wid": msg.get("wid", 1),
                "x": msg.get("x", 0),
                "y": msg.get("y", 0),
            }
        )
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "recv.close":
        _write({"v": V, "re": req_id, "ok": True})
    elif cmd == "recv.place":
        _write({"v": V, "re": req_id, "ok": True})
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


_STATS_STARTED: set = set()


def _emit_stats(tid: int, platform: str, role: str) -> None:
    key = (tid, role)
    if key in _STATS_STARTED:
        return
    _STATS_STARTED.add(key)

    def loop() -> None:
        n = 0
        while True:
            time.sleep(1.0)
            n += 1
            base = {
                "v": V,
                "ev": "stats",
                "tid": tid,
                "fps": 29.5 + (n % 3) * 0.1,
                "kbps": 4000 + n,
                "rtt_ms": 3.2,
                "loss_pct": 0.1 * (n % 2),
            }
            if role == "recv" and platform == "win":
                base["latency_ms"] = 18.0 + n
                base["recv_to_present_ms"] = 16.5 + n * 0.2
            else:
                base["latency_ms"] = 0.0
            _write(base)

    threading.Thread(target=loop, daemon=True).start()


def main() -> int:
    mode = os.environ.get("BEAMER_FAKE_HELPER_MODE", "normal")
    hang_cmd = os.environ.get("BEAMER_FAKE_HELPER_HANG_CMD", "hang.test")
    crash_cmd = os.environ.get("BEAMER_FAKE_HELPER_CRASH_CMD", "crash.test")
    busy_cmd = os.environ.get("BEAMER_FAKE_HELPER_BUSY_CMD", "busy.test")
    late_cmd = os.environ.get("BEAMER_FAKE_HELPER_LATE_CMD", "transport.open")
    late_delay = float(os.environ.get("BEAMER_FAKE_HELPER_LATE_DELAY", "0.4"))
    race_cmd = os.environ.get("BEAMER_FAKE_HELPER_RACE_CMD", "transport.open")
    race_delay = float(os.environ.get("BEAMER_FAKE_HELPER_RACE_DELAY", "0.25"))
    platform = os.environ.get("BEAMER_FAKE_HELPER_PLATFORM", "mac")
    local_port = int(os.environ.get("BEAMER_FAKE_HELPER_LOCAL_PORT", "47001"))

    if os.environ.get("BEAMER_FAKE_HELPER_STDERR_FLOOD", "").strip() in ("1", "true", "yes"):
        threading.Thread(target=_flood_stderr, daemon=True).start()

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
        if not _handle(
            msg,
            mode,
            hang_cmd,
            crash_cmd,
            busy_cmd,
            late_cmd,
            late_delay,
            race_cmd,
            race_delay,
            platform,
            local_port,
        ):
            break
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
