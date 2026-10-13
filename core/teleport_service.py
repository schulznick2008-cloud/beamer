"""Teleport-Steuerung in Beamer: Link-tp_*, Helfer-IPC, Schlüssel vom Link-Master."""
from __future__ import annotations

import base64
import binascii
import logging
import os
import secrets
import sys
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Mapping, Optional, Sequence, Set, Tuple

from core import keytable, protocol
from core.teleport_link import LinkRef, link_ref
from core.teleport_input import map_input_for_inject
from core.teleport_ipc import TeleportHelper, TeleportHelperConfig, TeleportIPCError, hello
from core.teleport_keys import TeleportKeySession
from core.teleport_path import PathChooser, PathDecision, PathSettings
from core.teleport_protocol import (
    MSG_ACCEPT,
    MSG_CLOSE,
    MSG_INPUT,
    MSG_OFFER,
    MSG_REJECT,
    MSG_RESIZE,
    MSG_FOCUS,
    MSG_UPDATE,
    accept_msg,
    choose_codec,
    close_msg,
    focus_msg,
    input_msg,
    offer_msg,
    read_tp,
    reject_msg,
)

log = logging.getLogger(__name__)

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MAC_HELPER = ROOT / "teleport" / "mac" / ".build" / "release" / "BeamerTeleport"
DEFAULT_WIN_HELPER = ROOT / "teleport" / "win" / "target" / "release" / "beamer-teleport.exe"
FAKE_HELPER = ROOT / "tools" / "teleport" / "fake_helper.py"


def _next_tid(used: Sequence[int]) -> int:
    for _ in range(32):
        tid = secrets.randbelow(0xFFFFFFFF) + 1
        if tid not in used:
            return tid
    raise TeleportIPCError("keine freie tid")


@dataclass
class PeerTeleportState:
    peer_id: str
    token: str
    peer_platform: str
    teleport_capable: bool = False
    key_session: Optional[TeleportKeySession] = None
    outbound: bool = False  # True: diese Seite hat den Link geöffnet (initiator)


@dataclass
class ActiveTeleport:
    tid: int
    peer_id: str
    link_outbound: bool
    role: str  # "sender" | "receiver"
    salt16: bytes
    codec: str
    local_udp: int = 0
    peer_udp: int = 0
    proxy_focused: bool = False
    stream_keys_sent: bool = False
    wid: Optional[int] = None
    place_x: Optional[float] = None
    place_y: Optional[float] = None
    drag_pending: bool = False


class TeleportService:
    """Eine Instanz pro Beamer-App (Mac oder Windows)."""

    def __init__(
        self,
        platform: str,
        *,
        post_to_peer: Callable[[str, dict, bool], bool],
        helper_config: Optional[TeleportHelperConfig] = None,
        path_settings: Optional[PathSettings] = None,
        path_probe: Optional[Callable[[str, int], tuple[bool, float]]] = None,
        key_style: str = "semantic",
        logger: Optional[logging.Logger] = None,
        pick_front_window: Optional[Callable[[], Optional[Mapping[str, Any]]]] = None,
    ) -> None:
        self.platform = platform
        self._post = post_to_peer
        self._key_style = key_style
        self.logger = logger or log
        self._pick_front_window = pick_front_window
        self._drag_state_handler: Optional[Callable[[Mapping[str, Any]], None]] = None
        self._lock = threading.RLock()
        self._links: Dict[LinkRef, PeerTeleportState] = {}
        self._seen_offers: Dict[LinkRef, Set[Tuple[bytes, int]]] = {}
        self._active: Dict[int, ActiveTeleport] = {}
        self._helper_cfg = helper_config
        self._helper: Optional[TeleportHelper] = None
        self._path = PathChooser(path_settings or PathSettings(), path_probe or (lambda _h, _p: (False, 0.0)))
        self._helper_low_power = False
        self._bitrate_kbps = PathSettings().max_kbps_wlan

    @property
    def proxy_swallow_input(self) -> bool:
        """Windows: KVM-Hook unterdrücken, wenn ein Proxy Fokus hat (bis C05 vollständig native erfasst)."""
        with self._lock:
            return any(t.proxy_focused for t in self._active.values() if t.role == "receiver")

    def set_helper_config(self, cfg: TeleportHelperConfig) -> None:
        self._helper_cfg = cfg

    def set_drag_state_handler(self, handler: Optional[Callable[[Mapping[str, Any]], None]]) -> None:
        self._drag_state_handler = handler

    def _ensure_helper(self) -> TeleportHelper:
        if self._helper is None:
            if self._helper_cfg is None:
                self._helper_cfg = default_helper_config(self.platform)
            self._helper = TeleportHelper(self._helper_cfg, on_event=self._on_helper_event)
            self._helper.start()
            rep = hello(self._helper, "1.5.2")
            self._helper_low_power = bool(rep.get("low_power"))
        return self._helper

    def shutdown(self) -> None:
        with self._lock:
            for tid in list(self._active):
                self._local_close(tid, "error")
            if self._helper is not None:
                self._helper.stop()
                self._helper = None

    def on_link_up(
        self,
        peer_id: str,
        token: str,
        peer_platform: str,
        teleport_capable: bool,
        link_session: protocol.LinkSession,
        outbound: bool,
    ) -> None:
        material = link_session.teleport_handshake_material()
        key_session = None
        if material is not None:
            shared, key_id, pi, pr, si, sr = material
            key_session = TeleportKeySession.from_link(token, shared, key_id, pi, pr, si, sr)
        ref = link_ref(peer_id, outbound)
        with self._lock:
            self._links[ref] = PeerTeleportState(
                peer_id=peer_id,
                token=token,
                peer_platform=peer_platform,
                teleport_capable=teleport_capable and key_session is not None,
                key_session=key_session,
                outbound=outbound,
            )

    def on_link_down(self, peer_id: str, outbound: bool) -> None:
        ref = link_ref(peer_id, outbound)
        with self._lock:
            for tid, tele in list(self._active.items()):
                if tele.peer_id == peer_id and tele.link_outbound == outbound:
                    self._local_close(tid, "error")
            self._links.pop(ref, None)
            self._seen_offers.pop(ref, None)

    def handle_link_message(self, peer_id: str, message: dict, outbound: bool) -> None:
        parsed = read_tp(message)
        if parsed is None:
            return
        kind, data = parsed
        tid = int(data["tid"])
        if kind == MSG_OFFER:
            self._handle_offer(peer_id, data, outbound)
        elif kind == MSG_ACCEPT:
            self._handle_accept(peer_id, data, outbound)
        elif kind == MSG_REJECT:
            self._handle_reject(peer_id, tid, data.get("reason", ""))
        elif kind == MSG_INPUT:
            self._handle_input(peer_id, tid, data["input"], outbound)
        elif kind == MSG_RESIZE:
            self._handle_resize(peer_id, tid, data, outbound)
        elif kind == MSG_FOCUS:
            self._handle_focus(peer_id, tid, outbound)
        elif kind == MSG_UPDATE:
            pass  # UI-Update auf Empfängerseite (später)
        elif kind == MSG_CLOSE:
            self._handle_close(peer_id, tid, data.get("reason", "user"))

    def toggle_front_window_mac(self, peer_id: Optional[str] = None) -> bool:
        """Mac: Vorderfenster zum PC teleportieren oder zurückholen."""
        if self.platform not in keytable.MAC_PLATFORMS:
            return False
        peer = peer_id or self._default_pc_peer()
        if peer is None:
            self.logger.warning("Teleport: kein Windows-Peer mit teleport-Fähigkeit")
            return False
        with self._lock:
            existing = [t for t in self._active.values() if t.peer_id == peer and t.role == "sender"]
            if existing:
                t = existing[0]
                self._request_close(peer, t.tid, "return", t.link_outbound)
                return True
        return self._start_outbound_mac(peer, outbound=True)

    def start_drag_teleport_mac(
        self,
        peer_id: str,
        wid: int,
        place_x: float,
        place_y: float,
    ) -> bool:
        if self.platform not in keytable.MAC_PLATFORMS:
            return False
        helper = self._ensure_helper()
        wl = helper.request("windows.list")
        if not wl.get("ok"):
            return False
        win = next((w for w in wl.get("windows", []) if int(w.get("wid", 0)) == int(wid)), None)
        if win is None:
            return False
        return self._start_outbound_mac(
            peer_id,
            outbound=True,
            win=win,
            place_x=place_x,
            place_y=place_y,
            drag=True,
        )

    def _default_pc_peer(self) -> Optional[str]:
        with self._lock:
            for (pid, outbound), st in self._links.items():
                if outbound and st.teleport_capable and not keytable.is_mac(st.peer_platform):
                    return pid
        return None

    def _peer_state(self, peer_id: str, outbound: bool) -> Optional[PeerTeleportState]:
        with self._lock:
            st = self._links.get(link_ref(peer_id, outbound))
            if st is None or not st.teleport_capable or st.key_session is None:
                return None
            return st

    def _start_outbound_mac(
        self,
        peer_id: str,
        *,
        outbound: bool,
        win: Optional[Mapping[str, Any]] = None,
        place_x: Optional[float] = None,
        place_y: Optional[float] = None,
        drag: bool = False,
    ) -> bool:
        st = self._peer_state(peer_id, outbound)
        if st is None:
            return False
        helper = self._ensure_helper()
        if win is None:
            if self._pick_front_window is not None:
                win = self._pick_front_window()
            else:
                wl = helper.request("windows.list")
                if not wl.get("ok") or not wl.get("windows"):
                    self.logger.warning("Teleport: keine Fenster")
                    return False
                win = max(wl["windows"], key=lambda w: float(w.get("w", 0)) * float(w.get("h", 0)))
        if win is None:
            self.logger.warning("Teleport: kein Vorderfenster")
            return False
        tid = _next_tid([t.tid for t in self._active.values()])
        salt_b64 = base64.b64encode(os.urandom(16)).decode("ascii")
        salt16 = base64.b64decode(salt_b64)
        codec = choose_codec(self._helper_low_power)
        tr = helper.request_retry_busy("transport.open", tries=5, delay=0.3, local_port=0, peer_host="0.0.0.0", peer_port=0)
        local_port = int(tr["local_port"])
        tele = ActiveTeleport(
            tid=tid,
            peer_id=peer_id,
            link_outbound=outbound,
            role="sender",
            salt16=salt16,
            codec=codec,
            local_udp=local_port,
            wid=int(win.get("wid", 0)),
            place_x=place_x,
            place_y=place_y,
            drag_pending=drag,
        )
        with self._lock:
            self._active[tid] = tele
        offer = offer_msg(
            tid,
            app=str(win.get("app", "")),
            title=str(win.get("title", "")),
            icon_png="",
            w=float(win.get("w", 800)),
            h=float(win.get("h", 600)),
            scale=float(win.get("scale", 1.0)),
            codec=codec,
            mode="window",
            udp_port=local_port,
            salt_b64=salt_b64,
            place_x=place_x,
            place_y=place_y,
            drag=drag,
        )
        if not self._post(peer_id, offer, outbound):
            with self._lock:
                self._active.pop(tid, None)
            return False
        return True

    def _handle_offer(self, peer_id: str, data: Mapping[str, Any], outbound: bool) -> None:
        if keytable.is_mac(self.platform):
            return
        st = self._peer_state(peer_id, outbound)
        if st is None:
            return
        tid = int(data["tid"])
        salt_b64 = data.get("salt", "")
        try:
            salt16 = base64.b64decode(salt_b64, validate=True)
        except (binascii.Error, ValueError):
            self._post(peer_id, reject_msg(tid, "error"), outbound)
            return
        if len(salt16) != 16:
            self._post(peer_id, reject_msg(tid, "error"), outbound)
            return
        ref = link_ref(peer_id, outbound)
        with self._lock:
            seen = self._seen_offers.setdefault(ref, set())
            pair = (salt16, tid)
            if pair in seen:
                self._post(peer_id, reject_msg(tid, "error"), outbound)
                return
            seen.add(pair)
        helper = self._ensure_helper()
        codec = str(data.get("codec", "hevc"))
        tr = helper.request_retry_busy(
            "transport.open", tries=5, delay=0.3, local_port=0, peer_host="0.0.0.0", peer_port=0
        )
        local_port = int(tr["local_port"])
        keys = st.key_session.allocate(salt16, tid).as_ipc_dict()
        recv = helper.request(
            "recv.open",
            tid=tid,
            title=str(data.get("title", "")),
            app=str(data.get("app", "")),
            icon_png=str(data.get("icon_png", "")),
            w=float(data.get("w", 800)),
            h=float(data.get("h", 600)),
            scale=float(data.get("scale", 1.0)),
            codec=codec,
            fullscreen=False,
            keys=keys,
        )
        if not recv.get("ok"):
            self._post(peer_id, reject_msg(tid, "error"), outbound)
            return
        tele = ActiveTeleport(
            tid=tid,
            peer_id=peer_id,
            link_outbound=outbound,
            role="receiver",
            salt16=salt16,
            codec=codec,
            local_udp=local_port,
            peer_udp=int(data.get("udp_port", 0)),
            stream_keys_sent=True,
            place_x=data.get("place_x"),
            place_y=data.get("place_y"),
            drag_pending=bool(data.get("drag")),
        )
        with self._lock:
            self._active[tid] = tele
        self._post(peer_id, accept_msg(tid, local_port), outbound)
        peer_host = self._path_host_for_peer(peer_id)
        if peer_host and int(data.get("udp_port", 0)):
            helper.request("transport.peer", peer_host=peer_host, peer_port=int(data["udp_port"]))

    def _handle_accept(self, peer_id: str, data: Mapping[str, Any], outbound: bool) -> None:
        if not keytable.is_mac(self.platform):
            return
        st = self._peer_state(peer_id, outbound)
        if st is None:
            return
        tid = int(data["tid"])
        with self._lock:
            tele = self._active.get(tid)
            if tele is None or tele.role != "sender" or tele.link_outbound != outbound:
                return
            tele.peer_udp = int(data.get("udp_port", 0))
        helper = self._ensure_helper()
        keys = st.key_session.allocate(tele.salt16, tid).as_ipc_dict()
        wid = tele.wid or int(os.environ.get("BEAMER_TELEPORT_WID", "1"))
        start = helper.request_retry_busy(
            "send.start",
            tries=5,
            delay=0.3,
            tid=tid,
            mode="window",
            wid=wid,
            codec=tele.codec,
            max_fps=60,
            bitrate_kbps=self._bitrate_kbps,
            keys=keys,
            virtual_display=True,
        )
        if not start.get("ok"):
            self._request_close(peer_id, tid, "error", outbound)
            return
        peer_host = self._path_host_for_peer(peer_id)
        if peer_host and tele.peer_udp:
            helper.request("transport.peer", peer_host=peer_host, peer_port=tele.peer_udp)

    def _handle_reject(self, peer_id: str, tid: int, reason: str) -> None:
        with self._lock:
            self._active.pop(tid, None)
        self.logger.info("Teleport abgelehnt von %s: %s", peer_id, reason)

    def _handle_input(self, peer_id: str, tid: int, input_obj: Mapping[str, Any], outbound: bool) -> None:
        with self._lock:
            tele = self._active.get(tid)
            st = self._links.get(link_ref(peer_id, outbound))
        if tele is None or tele.role != "sender" or st is None or tele.link_outbound != outbound:
            return
        mapped = map_input_for_inject(
            input_obj,
            st.peer_platform,
            self.platform,
            self._key_style,
        )
        try:
            self._ensure_helper().request("inject", tid=tid, input=mapped)
        except TeleportIPCError as exc:
            self.logger.warning("inject fehlgeschlagen: %s", exc)

    def _handle_resize(self, peer_id: str, tid: int, data: Mapping[str, Any], outbound: bool) -> None:
        if not keytable.is_mac(self.platform):
            return
        with self._lock:
            tele = self._active.get(tid)
        if tele is None or tele.role != "sender" or tele.link_outbound != outbound:
            return
        try:
            self._ensure_helper().request("send.resize", tid=tid, w=float(data["w"]), h=float(data["h"]))
        except TeleportIPCError:
            pass

    def _handle_focus(self, peer_id: str, tid: int, outbound: bool) -> None:
        if not keytable.is_mac(self.platform):
            return
        with self._lock:
            tele = self._active.get(tid)
        if tele is None or tele.link_outbound != outbound:
            return
        try:
            self._ensure_helper().request("send.activate", tid=tid)
        except TeleportIPCError:
            pass

    def _handle_close(self, peer_id: str, tid: int, reason: str) -> None:
        with self._lock:
            tele = self._active.pop(tid, None)
        if tele is None:
            return
        self._stop_streams(tele, restore=reason == "return")

    def _request_close(self, peer_id: str, tid: int, reason: str, outbound: bool) -> None:
        self._post(peer_id, close_msg(tid, reason), outbound)
        self._local_close(tid, reason)

    def _local_close(self, tid: int, reason: str) -> None:
        with self._lock:
            tele = self._active.pop(tid, None)
        if tele is not None:
            self._stop_streams(tele, restore=reason == "return")

    def _stop_streams(self, tele: ActiveTeleport, restore: bool) -> None:
        if self._helper is None:
            return
        try:
            if tele.role == "sender":
                self._helper.request("send.stop", tid=tele.tid, restore=restore)
            else:
                self._helper.request("recv.close", tid=tele.tid)
        except TeleportIPCError:
            pass

    def _on_helper_event(self, ev: Mapping[str, Any]) -> None:
        name = ev.get("ev")
        if name == "windows.drag_state" and self._drag_state_handler is not None:
            try:
                self._drag_state_handler(dict(ev))
            except Exception:
                self.logger.exception("windows.drag_state Handler fehlgeschlagen")
            return
        tid = ev.get("tid")
        if not isinstance(tid, int):
            return
        if name == "proxy.input":
            inp = ev.get("input")
            if isinstance(inp, dict):
                with self._lock:
                    tele = self._active.get(tid)
                if tele is not None and tele.role == "receiver":
                    self._post(tele.peer_id, input_msg(tid, inp), tele.link_outbound)
        elif name == "proxy.focus":
            focused = bool(ev.get("focused"))
            with self._lock:
                tele = self._active.get(tid)
                if tele is not None:
                    tele.proxy_focused = focused
            if focused and tele is not None:
                self._post(tele.peer_id, focus_msg(tid), tele.link_outbound)
        elif name == "proxy.closed":
            with self._lock:
                tele = self._active.get(tid)
            if tele is not None:
                self._request_close(tele.peer_id, tid, "return", tele.link_outbound)
        elif name == "send.gone":
            with self._lock:
                tele = self._active.get(tid)
            if tele is not None:
                self._request_close(tele.peer_id, tid, "gone", tele.link_outbound)
        elif name == "helper_restarted":
            with self._lock:
                for t in list(self._active.values()):
                    self._local_close(t.tid, "error")

    def apply_path_decision(self, decision: PathDecision) -> None:
        self._bitrate_kbps = decision.bitrate_kbps
        if self._helper is None:
            return
        for tele in list(self._active.values()):
            if tele.role == "sender" and decision.peer_host and tele.peer_udp:
                try:
                    self._helper.request(
                        "transport.peer", peer_host=decision.peer_host, peer_port=tele.peer_udp
                    )
                    self._helper.request("send.bitrate", tid=tele.tid, bitrate_kbps=decision.bitrate_kbps)
                except TeleportIPCError:
                    pass

    def _path_host_for_peer(self, peer_id: str) -> str:
        decision = self._path.tick(force=True)
        if decision and decision.peer_host:
            self.apply_path_decision(decision)
            return decision.peer_host
        return ""


def default_helper_config(platform: str, fake: bool = False) -> TeleportHelperConfig:
    if fake or os.environ.get("BEAMER_TELEPORT_FAKE_HELPER") == "1":
        port = int(os.environ.get("BEAMER_FAKE_HELPER_LOCAL_PORT", "47100"))
        env = {
            "PYTHONUNBUFFERED": "1",
            "BEAMER_FAKE_HELPER_PLATFORM": "mac" if keytable.is_mac(platform) else "win",
            "BEAMER_FAKE_HELPER_LOCAL_PORT": str(port),
        }
        return TeleportHelperConfig(
            executable=[sys.executable, str(FAKE_HELPER)],
            extra_args=[],
            env=env,
            restart_on_crash=True,
        )
    if keytable.is_mac(platform):
        path = Path(os.environ.get("BEAMER_TELEPORT_MAC_HELPER", str(DEFAULT_MAC_HELPER)))
    else:
        path = Path(os.environ.get("BEAMER_TELEPORT_WIN_HELPER", str(DEFAULT_WIN_HELPER)))
    return TeleportHelperConfig(executable=[str(path)], extra_args=["--ipc", "stdio"], restart_on_crash=True)
