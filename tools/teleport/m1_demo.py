#!/usr/bin/env python3
"""M1-Demo: Mac-Sender und PC-Empfänger über Teleport-Helfer (SCHNITTSTELLEN.md §1/§4).

Jede Rolle startet den lokalen Helfer per ``core.teleport_ipc``. Schlüssel kommen aus
``core.teleport_keys`` (frisches ``salt16`` je Lauf bzw. nach ``helper_restarted``).

Siehe ``docs/teleport/M1-DEMO.md``.
"""
from __future__ import annotations

import argparse
import json
import logging
import os
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Mapping, Optional

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from core.teleport_ipc import TeleportHelper, TeleportHelperConfig, TeleportIPCError, hello
from core.teleport_keys import TeleportKeySession

log = logging.getLogger(__name__)

DEFAULT_MAC_HELPER = ROOT / "teleport" / "mac" / ".build" / "release" / "BeamerTeleport"
DEFAULT_WIN_HELPER = ROOT / "teleport" / "win" / "target" / "release" / "beamer-teleport.exe"
FAKE_HELPER = ROOT / "tools" / "teleport" / "fake_helper.py"
VECTORS = ROOT / "docs" / "teleport" / "vectors.json"


@dataclass
class StatsSnapshot:
    samples: int = 0
    fps_sum: float = 0.0
    loss_sum: float = 0.0
    rtt_sum: float = 0.0
    image_age_sum: float = 0.0
    last: Dict[str, Any] = field(default_factory=dict)

    def ingest(self, ev: Mapping[str, Any]) -> None:
        if ev.get("ev") != "stats":
            return
        self.samples += 1
        self.fps_sum += float(ev.get("fps") or 0)
        self.loss_sum += float(ev.get("loss_pct") or 0)
        self.rtt_sum += float(ev.get("rtt_ms") or 0)
        age = ev.get("recv_to_present_ms")
        if age is None:
            age = ev.get("latency_ms")
        self.image_age_sum += float(age or 0)
        self.last = dict(ev)

    def summary(self, role: str) -> Dict[str, Any]:
        n = max(self.samples, 1)
        return {
            "role": role,
            "samples": self.samples,
            "fps_avg": self.fps_sum / n,
            "loss_pct_avg": self.loss_sum / n,
            "rtt_ms_avg": self.rtt_sum / n,
            "image_age_ms_avg": self.image_age_sum / n,
            "last": self.last,
        }


def _load_vectors_master() -> bytes:
    with open(VECTORS, encoding="utf-8") as f:
        data = json.load(f)
    return bytes.fromhex(data["link"]["master_hex"])


def _fresh_salt16() -> bytes:
    return os.urandom(16)


def _load_session(path: Path) -> Dict[str, Any]:
    if not path.exists():
        return {}
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def _save_session(path: Path, data: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    tmp.replace(path)


def _peer_host_port(peer: str) -> tuple[str, int]:
    if ":" not in peer:
        raise SystemExit(f"--peer braucht Host:Port, bekam: {peer!r}")
    host, port_s = peer.rsplit(":", 1)
    return host, int(port_s)


def _helper_config(role: str, helper: Optional[str], fake: bool, fake_port: int) -> TeleportHelperConfig:
    if fake:
        env = {
            "PYTHONUNBUFFERED": "1",
            "BEAMER_FAKE_HELPER_PLATFORM": "mac" if role == "mac" else "win",
            "BEAMER_FAKE_HELPER_LOCAL_PORT": str(fake_port),
        }
        return TeleportHelperConfig(
            executable=[sys.executable, str(FAKE_HELPER)],
            extra_args=[],
            env=env,
            restart_on_crash=True,
        )
    if role == "mac":
        path = Path(helper) if helper else DEFAULT_MAC_HELPER
    else:
        path = Path(helper) if helper else DEFAULT_WIN_HELPER
    if not path.is_file():
        raise SystemExit(f"Helfer nicht gefunden: {path}")
    return TeleportHelperConfig(executable=[str(path)], extra_args=["--ipc", "stdio"], restart_on_crash=True)


def _keys_for_run(key_session: TeleportKeySession, salt16: bytes, tid: int) -> Dict[str, str]:
    return key_session.allocate(salt16, tid).as_ipc_dict()


def _area(w: Mapping[str, Any]) -> float:
    return float(w.get("w", 0)) * float(w.get("h", 0))


def _pick_window(windows: List[Mapping[str, Any]], app_hint: Optional[str]) -> Mapping[str, Any]:
    """Fenster der App `app_hint` (genauer App-Name vor Teilstring, dann das groesste). Ohne passendes
    Fenster wird abgebrochen statt auf ein fremdes Fenster auszuweichen. Nur ohne `app_hint` gilt das
    groesste Fenster."""
    if not windows:
        raise TeleportIPCError("keine Fenster vom Helfer")
    if app_hint:
        hint = app_hint.lower()
        exact = [w for w in windows if str(w.get("app", "")).lower() == hint]
        cands = exact or [w for w in windows if hint in str(w.get("app", "")).lower()]
        if not cands:
            apps = sorted({str(w.get("app", "")) for w in windows})
            raise TeleportIPCError(
                f"kein sichtbares Fenster der App {app_hint!r} (Fenster offen und nicht minimiert?); "
                f"vorhanden: {', '.join(apps)}"
            )
        return max(cands, key=_area)
    return max(windows, key=_area)


def run_pc(
    helper: TeleportHelper,
    peer_host: str,
    session: Dict[str, Any],
    session_path: Path,
    tid: int,
    codec: str,
    seconds: float,
    key_session: TeleportKeySession,
    stats: StatsSnapshot,
    on_restart: Optional[Any] = None,
    local_port: int = 0,
    salt_override: Optional[str] = None,
) -> None:
    # Pro Lauf immer ein frisches salt16 (nie aus einer alten Session-Datei uebernehmen: gleiche
    # Schluessel mit neu beginnendem seq waeren Nonce-Wiederverwendung, SCHNITTSTELLEN.md 4.2).
    salt_hex = salt_override or _fresh_salt16().hex()
    session["salt16_hex"] = salt_hex
    salt16 = bytes.fromhex(salt_hex)
    keys = _keys_for_run(key_session, salt16, tid)

    hello(helper, "m1-demo")
    # Peer-Port 0: der Empfaenger lernt die Gegenstelle aus dem ersten authentifizierten Paket
    # (Roaming, 4.1/7); NACK/Feedback (r2s) gehen danach an diese Adresse.
    peer_port = int(session.get("peer_udp_port") or 0)
    tr = helper.request("transport.open", local_port=local_port, peer_host=peer_host, peer_port=peer_port)
    if not tr.get("ok"):
        raise TeleportIPCError(f"transport.open: {tr}")
    local_port = int(tr["local_port"])
    session["pc_udp_port"] = local_port
    session["tid"] = tid
    _save_session(session_path, session)
    print(
        "PC bereit. Auf dem Mac jetzt starten:\n"
        f"  python3 tools/teleport/m1_demo.py --role mac --peer <PC-IP>:{local_port} "
        f"--salt-hex {salt_hex} --tid {tid} --codec {codec} --seconds <s> --app <App>",
        flush=True,
    )

    recv = helper.request(
        "recv.open",
        tid=tid,
        title=session.get("title", "Teleport M1"),
        app=session.get("app", "M1-Demo"),
        w=float(session.get("w", 800)),
        h=float(session.get("h", 600)),
        scale=float(session.get("scale", 1.0)),
        codec=codec,
        fullscreen=False,
        keys=keys,
    )
    if not recv.get("ok"):
        raise TeleportIPCError(f"recv.open: {recv}")

    end = time.time() + seconds
    while time.time() < end:
        for ev in helper.pop_events():
            stats.ingest(ev)
            if ev.get("ev") == "helper_restarted" and on_restart:
                on_restart(ev)
        time.sleep(0.2)

    helper.request("recv.close", tid=tid)
    helper.request("transport.close")


def reopen_pc_after_restart(
    helper: TeleportHelper,
    peer_host: str,
    session: Dict[str, Any],
    tid: int,
    codec: str,
    key_session: TeleportKeySession,
    stats: StatsSnapshot,
) -> None:
    hello(helper, "m1-demo-restart")
    salt16 = bytes.fromhex(session["salt16_hex"])
    keys = _keys_for_run(key_session, salt16, tid)
    peer_port = int(session.get("mac_udp_port") or session.get("peer_udp_port") or 0)
    tr = helper.request("transport.open", local_port=0, peer_host=peer_host, peer_port=peer_port)
    local_port = int(tr["local_port"])
    session["pc_udp_port"] = local_port
    helper.request(
        "recv.open",
        tid=tid,
        title=session.get("title", "Teleport M1"),
        app=session.get("app", "M1-Demo"),
        w=float(session.get("w", 800)),
        h=float(session.get("h", 600)),
        scale=float(session.get("scale", 1.0)),
        codec=codec,
        fullscreen=False,
        keys=keys,
    )


def run_mac(
    helper: TeleportHelper,
    peer_host: str,
    peer_port: int,
    session: Dict[str, Any],
    session_path: Path,
    tid: int,
    codec: str,
    seconds: float,
    key_session: TeleportKeySession,
    stats: StatsSnapshot,
    app_hint: Optional[str],
    on_restart: Optional[Any] = None,
    salt_override: Optional[str] = None,
) -> None:
    if salt_override:
        session["salt16_hex"] = salt_override
    hello(helper, "m1-demo")
    tr = helper.request_retry_busy(
        "transport.open", tries=5, delay=0.4, local_port=0, peer_host=peer_host, peer_port=peer_port
    )
    if not tr.get("ok"):
        raise TeleportIPCError(f"transport.open: {tr}")
    session["mac_udp_port"] = int(tr["local_port"])
    _save_session(session_path, session)

    wl = helper.request("windows.list")
    if not wl.get("ok"):
        raise TeleportIPCError(f"windows.list: {wl}")
    win = _pick_window(wl["windows"], app_hint)
    session.update(
        title=win.get("title"),
        app=win.get("app"),
        w=win.get("w"),
        h=win.get("h"),
        scale=win.get("scale"),
    )
    _save_session(session_path, session)

    salt_hex = session.get("salt16_hex")
    if not salt_hex:
        salt_hex = _fresh_salt16().hex()
        session["salt16_hex"] = salt_hex
        _save_session(session_path, session)
    salt16 = bytes.fromhex(salt_hex)
    keys = _keys_for_run(key_session, salt16, tid)

    start = helper.request_retry_busy(
        "send.start",
        tries=5,
        delay=0.4,
        tid=tid,
        mode="window",
        wid=int(win["wid"]),
        codec=codec,
        max_fps=60,
        bitrate_kbps=20_000,
        keys=keys,
        virtual_display=False,
    )
    if not start.get("ok"):
        raise TeleportIPCError(f"send.start: {start}")

    end = time.time() + seconds
    while time.time() < end:
        for ev in helper.pop_events():
            stats.ingest(ev)
            if ev.get("ev") == "helper_restarted" and on_restart:
                on_restart(ev)
            if ev.get("ev") == "send.gone":
                log.error("send.gone: %s", ev.get("reason"))
                break
        time.sleep(0.2)

    helper.request("send.stop", tid=tid, restore=True)
    helper.request("transport.close")


def reopen_mac_after_restart(
    helper: TeleportHelper,
    peer_host: str,
    peer_port: int,
    session: Dict[str, Any],
    tid: int,
    codec: str,
    key_session: TeleportKeySession,
    stats: StatsSnapshot,
    app_hint: Optional[str],
) -> None:
    hello(helper, "m1-demo-restart")
    tr = helper.request_retry_busy(
        "transport.open", tries=5, delay=0.4, local_port=0, peer_host=peer_host, peer_port=peer_port
    )
    session["mac_udp_port"] = int(tr["local_port"])
    wl = helper.request("windows.list")
    win = _pick_window(wl["windows"], app_hint)
    salt16 = bytes.fromhex(session["salt16_hex"])
    keys = _keys_for_run(key_session, salt16, tid)
    helper.request_retry_busy(
        "send.start",
        tries=5,
        delay=0.4,
        tid=tid,
        mode="window",
        wid=int(win["wid"]),
        codec=codec,
        max_fps=60,
        bitrate_kbps=20_000,
        keys=keys,
        virtual_display=False,
    )


def _wait_session_key(session_path: Path, key: str, timeout: float = 120.0) -> Any:
    deadline = time.time() + timeout
    while time.time() < deadline:
        data = _load_session(session_path)
        if data.get(key) is not None:
            return data[key]
        time.sleep(0.25)
    raise SystemExit(f"Timeout: Session-Feld {key!r} in {session_path}")


def run_selftest_fake(seconds: float) -> Dict[str, Any]:
    """Zwei Fake-Helfer auf localhost (CI)."""
    import threading

    session_path = Path(os.environ.get("TMPDIR", "/tmp")) / "beamer_m1_fake_session.json"
    session: Dict[str, Any] = {"tid": 1, "master_hex": _load_vectors_master().hex()}
    _save_session(session_path, session)
    master = bytes.fromhex(session["master_hex"])
    key_pc = TeleportKeySession(master)
    key_mac = TeleportKeySession(master)
    pc_stats = StatsSnapshot()
    mac_stats = StatsSnapshot()
    err: List[BaseException] = []

    def pc_thread() -> None:
        try:
            h = TeleportHelper(_helper_config("pc", None, True, 47100))
            h.start()

            def pc_restart(_ev: Mapping[str, Any]) -> None:
                session["salt16_hex"] = _fresh_salt16().hex()
                _save_session(session_path, session)
                reopen_pc_after_restart(h, "127.0.0.1", session, 1, "h264", key_pc, pc_stats)

            run_pc(
                h,
                "127.0.0.1",
                session,
                session_path,
                1,
                "h264",
                seconds,
                key_pc,
                pc_stats,
                on_restart=pc_restart,
            )
            h.stop()
        except BaseException as e:
            err.append(e)

    t = threading.Thread(target=pc_thread, daemon=True)
    t.start()
    time.sleep(0.5)
    _wait_session_key(session_path, "pc_udp_port", timeout=5.0)
    data = _load_session(session_path)
    pc_port = int(data["pc_udp_port"])
    h_mac = TeleportHelper(_helper_config("mac", None, True, 47101))
    h_mac.start()
    try:

        def mac_restart(_ev: Mapping[str, Any]) -> None:
            data["salt16_hex"] = _fresh_salt16().hex()
            _save_session(session_path, data)
            reopen_mac_after_restart(
                h_mac, "127.0.0.1", pc_port, data, 1, "h264", key_mac, mac_stats, None
            )

        run_mac(
            h_mac,
            "127.0.0.1",
            pc_port,
            data,
            session_path,
            1,
            "h264",
            seconds,
            key_mac,
            mac_stats,
            None,
            on_restart=mac_restart,
        )
    finally:
        h_mac.stop()
    t.join(timeout=seconds + 10)
    if err:
        raise err[0]
    out = {
        "mode": "selftest-fake",
        "pc": pc_stats.summary("pc"),
        "mac": mac_stats.summary("mac"),
    }
    return out


def print_summary(pc: Optional[StatsSnapshot], mac: Optional[StatsSnapshot], extra: Dict[str, Any]) -> None:
    report: Dict[str, Any] = dict(extra)
    if pc:
        report["pc"] = pc.summary("pc")
    if mac:
        report["mac"] = mac.summary("mac")
    lines = ["=== M1-Demo Zusammenfassung ==="]
    for side in ("pc", "mac"):
        if side not in report:
            continue
        s = report[side]
        lines.append(
            f"{side}: fps≈{s['fps_avg']:.1f}  Verlust≈{s['loss_pct_avg']:.2f}%  "
            f"RTT≈{s['rtt_ms_avg']:.1f} ms  Bildalter≈{s['image_age_ms_avg']:.1f} ms  ({s['samples']} stats)"
        )
    text = "\n".join(lines)
    print(text)
    print("---M1_JSON---")
    print(json.dumps(report, indent=2, ensure_ascii=False))


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Beamer Teleport M1-Demo")
    ap.add_argument("--role", choices=("mac", "pc"), help="Diese Maschine: Sender (mac) oder Empfänger (pc)")
    ap.add_argument("--peer", help="Gegenstelle als IP:Port (UDP des anderen Helfers, nach Session)")
    ap.add_argument("--codec", choices=("h264", "hevc"), default="h264")
    ap.add_argument("--seconds", type=float, default=20.0)
    ap.add_argument("--tid", type=int, default=1)
    ap.add_argument("--session", type=Path, default=Path("m1_demo_session.json"))
    ap.add_argument("--helper", help="Pfad zum lokalen Helfer-Binary")
    ap.add_argument("--app", help="Teilstring App-Name (nur mac)")
    ap.add_argument("--master-hex", help="32-Byte-Master hex (Default: vectors.json, NUR LABOR)")
    ap.add_argument("--salt-hex", help="salt16 (32 Hex-Zeichen) vom PC-Lauf; der PC gibt die fertige Mac-Zeile aus")
    ap.add_argument("--local-port", type=int, default=None, help="UDP-Port des lokalen Helfers (PC-Default 47001, Mac 0)")
    ap.add_argument("--selftest-fake", action="store_true", help="Zwei Fake-Helfer lokal (CI)")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args(argv)

    logging.basicConfig(level=logging.DEBUG if args.verbose else logging.INFO)

    if args.selftest_fake:
        report = run_selftest_fake(args.seconds)
        print_summary(None, None, report)
        return 0

    if not args.role:
        ap.error("--role mac|pc oder --selftest-fake")
    if not args.peer:
        ap.error("--peer erforderlich")

    peer_host, peer_port = _peer_host_port(args.peer)
    session = _load_session(args.session)
    master_hex = args.master_hex or session.get("master_hex")
    if not master_hex:
        master_hex = _load_vectors_master().hex()
        session["master_hex"] = master_hex
        _save_session(args.session, session)
    if master_hex == _load_vectors_master().hex():
        print(
            "WARNUNG: Master aus docs/teleport/vectors.json (oeffentliche Testvektoren). "
            "Nur fuer M1 im Labor, NIE in Produktion; ab E5 kommt der Master aus dem Beamer-Link.",
            file=sys.stderr,
            flush=True,
        )
    if args.salt_hex and len(bytes.fromhex(args.salt_hex)) != 16:
        ap.error("--salt-hex braucht 32 Hex-Zeichen (16 Byte)")
    key_session = TeleportKeySession(bytes.fromhex(master_hex))
    session["tid"] = args.tid

    stats = StatsSnapshot()
    helper = TeleportHelper(_helper_config(args.role, args.helper, False, 0))
    helper.start()
    try:
        if args.role == "pc":
            session["peer_udp_port"] = peer_port

            def pc_restart(_ev: Mapping[str, Any]) -> None:
                session["salt16_hex"] = _fresh_salt16().hex()
                _save_session(args.session, session)
                reopen_pc_after_restart(
                    helper, peer_host, session, args.tid, args.codec, key_session, stats
                )

            run_pc(
                helper,
                peer_host,
                session,
                args.session,
                args.tid,
                args.codec,
                args.seconds,
                key_session,
                stats,
                on_restart=pc_restart,
                local_port=47001 if args.local_port is None else args.local_port,
                salt_override=args.salt_hex,
            )
            print_summary(stats, None, {"role": "pc", "session": str(args.session)})
        else:
            if not args.salt_hex and not session.get("salt16_hex"):
                ap.error("--salt-hex fehlt (steht in der Ausgabe des PC-Laufs)")
            if peer_port == 0:
                peer_port = int(_wait_session_key(args.session, "pc_udp_port"))

            def mac_restart(_ev: Mapping[str, Any]) -> None:
                session["salt16_hex"] = _fresh_salt16().hex()
                _save_session(args.session, session)
                reopen_mac_after_restart(
                    helper,
                    peer_host,
                    peer_port,
                    session,
                    args.tid,
                    args.codec,
                    key_session,
                    stats,
                    args.app,
                )

            run_mac(
                helper,
                peer_host,
                peer_port,
                session,
                args.session,
                args.tid,
                args.codec,
                args.seconds,
                key_session,
                stats,
                args.app,
                on_restart=mac_restart,
                salt_override=args.salt_hex,
            )
            print_summary(None, stats, {"role": "mac", "session": str(args.session)})
    finally:
        helper.stop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
