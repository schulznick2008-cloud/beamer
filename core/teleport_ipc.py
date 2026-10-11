"""Python-Seite der Helfer-IPC (SCHNITTSTELLEN.md §1).

Startet den nativen Helfer als Kindprozess, spricht JSON Lines über stdin/stdout,
korreliert Anfragen/Antworten, liefert Ereignisse und startet bei Absturz neu.
Nur Standardbibliothek.
"""
from __future__ import annotations

import json
import logging
import subprocess
import threading
import time
from collections import deque
from dataclasses import dataclass
from typing import Any, Callable, Deque, Dict, List, Mapping, MutableMapping, Optional, Sequence

log = logging.getLogger(__name__)

PROTOCOL_VERSION = 1
DEFAULT_CMD_TIMEOUT = 15.0
DEFAULT_SHUTDOWN_CMD_TIMEOUT = 5.0
DEFAULT_EOF_EXIT_TIMEOUT = 5.0
MAX_LINE_BYTES = 1 << 20
DEFAULT_RESTART_BACKOFF = (0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 30.0)


class TeleportIPCError(Exception):
    """Protokoll- oder Helfer-Zustandsfehler."""


class TeleportIPCTimeout(TeleportIPCError):
    def __init__(self, cmd: str, timeout: float):
        super().__init__(f"keine Antwort auf {cmd!r} innerhalb von {timeout:g} s")
        self.cmd = cmd
        self.timeout = timeout


class TeleportHelperCrashed(TeleportIPCError):
    def __init__(self, returncode: Optional[int]):
        super().__init__(f"Helfer-Prozess beendet (code={returncode})")
        self.returncode = returncode


EventCallback = Callable[[Mapping[str, Any]], None]


def _json_dumps(obj: Mapping[str, Any]) -> str:
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))


@dataclass
class TeleportHelperConfig:
    executable: Sequence[str]
    extra_args: Sequence[str] = ("--ipc", "stdio")
    env: Optional[Mapping[str, str]] = None
    cwd: Optional[str] = None
    restart_on_crash: bool = True
    restart_backoff: Sequence[float] = DEFAULT_RESTART_BACKOFF
    default_timeout: float = DEFAULT_CMD_TIMEOUT


class TeleportHelper:
    """Eine IPC-Session zu einem Helfer-Prozess."""

    def __init__(
        self,
        config: TeleportHelperConfig,
        on_event: Optional[EventCallback] = None,
    ) -> None:
        self._config = config
        self._on_event = on_event
        self._proc: Optional[subprocess.Popen[str]] = None
        self._reader: Optional[threading.Thread] = None
        self._monitor: Optional[threading.Thread] = None
        self._next_id = 1
        self._pending: Dict[int, threading.Event] = {}
        self._replies: Dict[int, Mapping[str, Any]] = {}
        self._events: Deque[Mapping[str, Any]] = deque()
        self._lock = threading.RLock()
        self._io_lock = threading.Lock()
        self._stop_requested = False
        self._intentional_stop = False
        self._crash_generation = 0
        self._restart_index = 0
        self._started = False

    @property
    def running(self) -> bool:
        with self._lock:
            return self._proc is not None and self._proc.poll() is None

    @property
    def returncode(self) -> Optional[int]:
        with self._lock:
            return self._proc.returncode if self._proc else None

    def start(self) -> None:
        with self._lock:
            if self._started and self.running:
                return
            self._stop_requested = False
            self._intentional_stop = False
            self._spawn_process()
            self._started = True

    def _spawn_process(self) -> None:
        cmd = list(self._config.executable) + list(self._config.extra_args)
        env = None
        if self._config.env is not None:
            import os

            env = {**os.environ, **dict(self._config.env)}
        self._proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            cwd=self._config.cwd,
            env=env,
        )
        gen = self._crash_generation
        self._reader = threading.Thread(
            target=self._read_loop,
            name="teleport-ipc-reader",
            args=(gen,),
            daemon=True,
        )
        self._reader.start()
        if self._config.restart_on_crash and self._monitor is None:
            self._monitor = threading.Thread(
                target=self._monitor_loop,
                name="teleport-ipc-monitor",
                daemon=True,
            )
            self._monitor.start()

    def stop(
        self,
        send_shutdown: bool = True,
        eof_timeout: float = DEFAULT_EOF_EXIT_TIMEOUT,
    ) -> int:
        """Beendet den Helfer (optional shutdown, dann EOF auf stdin)."""
        with self._lock:
            self._intentional_stop = True
            self._stop_requested = True
            proc = self._proc
        if proc is None:
            return 0
        if send_shutdown and proc.poll() is None:
            try:
                self.request("shutdown", timeout=DEFAULT_SHUTDOWN_CMD_TIMEOUT)
            except (TeleportIPCError, OSError):
                pass
        self._close_stdin()
        code = self._wait_process(eof_timeout)
        with self._lock:
            self._fail_pending(TeleportHelperCrashed(code))
        return code if code is not None else -1

    def _close_stdin(self) -> None:
        with self._io_lock:
            proc = self._proc
            if proc and proc.stdin:
                try:
                    proc.stdin.close()
                except OSError:
                    pass

    def _wait_process(self, timeout: float) -> Optional[int]:
        proc = self._proc
        if proc is None:
            return None
        try:
            proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=2)
        return proc.returncode

    def request(
        self,
        cmd: str,
        timeout: Optional[float] = None,
        **fields: Any,
    ) -> Mapping[str, Any]:
        if timeout is None:
            timeout = self._config.default_timeout
        req_id = self._allocate_id()
        msg: MutableMapping[str, Any] = {"v": PROTOCOL_VERSION, "id": req_id, "cmd": cmd}
        msg.update(fields)
        ev = threading.Event()
        with self._lock:
            if not self.running:
                raise TeleportIPCError("Helfer läuft nicht")
            self._pending[req_id] = ev
        self._write_line(msg)
        if not ev.wait(timeout):
            with self._lock:
                self._pending.pop(req_id, None)
            raise TeleportIPCTimeout(cmd, timeout)
        with self._lock:
            if req_id not in self._replies:
                raise TeleportHelperCrashed(self.returncode)
            reply = self._replies.pop(req_id)
        return reply

    def pop_events(self) -> List[Mapping[str, Any]]:
        with self._lock:
            out = list(self._events)
            self._events.clear()
        return out

    def drain_events(self) -> List[Mapping[str, Any]]:
        """Alias für ältere Skripte."""
        return self.pop_events()

    def _allocate_id(self) -> int:
        with self._lock:
            i = self._next_id
            self._next_id += 1
            return i

    def _write_line(self, obj: Mapping[str, Any]) -> None:
        line = _json_dumps(obj) + "\n"
        with self._io_lock:
            proc = self._proc
            if proc is None or proc.poll() is not None:
                raise TeleportIPCError("Helfer läuft nicht")
            assert proc.stdin is not None
            proc.stdin.write(line)
            proc.stdin.flush()

    def _deliver(self, msg: Mapping[str, Any]) -> None:
        re = msg.get("re")
        if re is not None and isinstance(re, int):
            with self._lock:
                self._replies[re] = msg
                ev = self._pending.pop(re, None)
            if ev:
                ev.set()
            return
        if msg.get("ev") is not None or msg.get("cmd") is not None:
            with self._lock:
                self._events.append(msg)
            if self._on_event:
                try:
                    self._on_event(msg)
                except Exception:
                    log.exception("on_event handler failed")
            return
        with self._lock:
            self._events.append(msg)

    def _fail_pending(self, exc: BaseException) -> None:
        with self._lock:
            pending = list(self._pending.values())
            self._pending.clear()
        for ev in pending:
            ev.set()

    def _read_loop(self, generation: int) -> None:
        proc = self._proc
        if proc is None or proc.stdout is None:
            return
        try:
            for raw_line in proc.stdout:
                line = raw_line.rstrip("\n")
                if line.endswith("\r"):
                    line = line[:-1]
                if not line.strip():
                    continue
                if len(line.encode("utf-8")) > MAX_LINE_BYTES:
                    self._deliver(
                        {
                            "v": PROTOCOL_VERSION,
                            "ev": "error",
                            "code": "bad_request",
                            "msg": "Zeile länger als 1 MiB",
                        }
                    )
                    continue
                try:
                    parsed = json.loads(line)
                except json.JSONDecodeError:
                    self._deliver(
                        {
                            "v": PROTOCOL_VERSION,
                            "ev": "error",
                            "code": "bad_request",
                            "msg": "ungültige JSON-Zeile vom Helfer",
                        }
                    )
                    continue
                if not isinstance(parsed, dict):
                    self._deliver(
                        {
                            "v": PROTOCOL_VERSION,
                            "ev": "error",
                            "code": "bad_request",
                            "msg": "JSON-Objekt erwartet",
                        }
                    )
                    continue
                self._deliver(parsed)
        finally:
            with self._lock:
                if generation == self._crash_generation:
                    self._fail_pending(TeleportHelperCrashed(proc.poll()))

    def _monitor_loop(self) -> None:
        while True:
            with self._lock:
                if self._stop_requested:
                    return
                proc = self._proc
            if proc is None:
                time.sleep(0.05)
                continue
            code = proc.poll()
            if code is None:
                time.sleep(0.1)
                continue
            if self._intentional_stop:
                return
            if not self._config.restart_on_crash:
                return
            delay = self._config.restart_backoff[
                min(self._restart_index, len(self._config.restart_backoff) - 1)
            ]
            log.warning("Helfer beendet mit Code %s, Neustart in %s s", code, delay)
            time.sleep(delay)
            with self._lock:
                if self._stop_requested or self._intentional_stop:
                    return
                self._restart_index += 1
                self._crash_generation += 1
                self._spawn_process()

    def mark_healthy(self) -> None:
        """Setzt Backoff nach erfolgreichem hello zurück."""
        with self._lock:
            self._restart_index = 0


def default_helper_command(helper_path: str) -> List[str]:
    return [helper_path]


def hello(
    helper: TeleportHelper,
    beamer_version: str,
    timeout: float = DEFAULT_CMD_TIMEOUT,
) -> Mapping[str, Any]:
    reply = helper.request("hello", timeout=timeout, beamer_version=beamer_version)
    if reply.get("ok"):
        helper.mark_healthy()
    return reply
