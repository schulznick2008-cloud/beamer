"""Pfadwahl WLAN/Kabel (SCHNITTSTELLEN.md §7). Nur Stdlib."""
from __future__ import annotations

import ipaddress
import time
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Sequence, Tuple

DEFAULT_CABLE_SUBNET = "192.168.77.0/24"
DEFAULT_MAC_HOST = "192.168.77.1"
DEFAULT_PC_HOST = "192.168.77.2"
PROBE_PORT = 24820
PROBE_INTERVAL_S = 2.0
CABLE_SUCCESS_NEEDED = 2
CABLE_FAIL_DROP = 2
MAX_KBPS_WLAN = 40_000
MAX_KBPS_CABLE = 150_000

PathName = str  # "wlan" | "cable"


def parse_cable_subnet(text: str) -> ipaddress.IPv4Network:
    return ipaddress.IPv4Network(text, strict=False)


def is_link_local_or_cable(address: str, cable_subnet: ipaddress.IPv4Network) -> bool:
    try:
        ip = ipaddress.IPv4Address(address)
    except ValueError:
        return False
    if ip in ipaddress.IPv4Network("169.254.0.0/16"):
        return True
    return ip in cable_subnet


def is_cable_interface(address: str, netmask: str, cable_subnet: ipaddress.IPv4Network) -> bool:
    """Interface-Kandidat für Kabel (up, Broadcast, Adresse in 169.254/16 oder cable_subnet)."""
    if not address or not netmask:
        return False
    try:
        network = ipaddress.IPv4Network(f"{address}/{netmask}", strict=False)
    except ValueError:
        return False
    if network.prefixlen >= 31:
        return False
    return is_link_local_or_cable(address, cable_subnet)


@dataclass
class PathCandidate:
    addr: str
    path: PathName
    last_seen: float = 0.0
    rtt_ms: Optional[float] = None
    probe_ok_streak: int = 0
    probe_fail_streak: int = 0


@dataclass
class PathSettings:
    mode: str = "auto"  # auto | cable | wlan
    cable_subnet: str = DEFAULT_CABLE_SUBNET
    max_kbps_wlan: int = MAX_KBPS_WLAN
    max_kbps_cable: int = MAX_KBPS_CABLE


@dataclass
class PathDecision:
    path: PathName
    peer_host: str
    bitrate_kbps: int
    switched: bool = False


class PathChooser:
    """Wählt wlan vs. cable aus Kandidaten und TCP-Proben."""

    def __init__(
        self,
        settings: PathSettings,
        probe: Callable[[str, int], Tuple[bool, float]],
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._settings = settings
        self._probe = probe
        self._clock = clock
        self._subnet = parse_cable_subnet(settings.cable_subnet)
        self._candidates: Dict[str, PathCandidate] = {}
        self._active: PathName = "wlan"
        self._last_probe_at = 0.0

    @property
    def active_path(self) -> PathName:
        return self._active

    def note_beacon(self, addr: str, path: PathName, rtt_ms: Optional[float] = None) -> None:
        now = self._clock()
        cand = self._candidates.get(addr)
        if cand is None:
            cand = PathCandidate(addr=addr, path=path)
            self._candidates[addr] = cand
        cand.last_seen = now
        cand.path = path
        if rtt_ms is not None:
            cand.rtt_ms = rtt_ms

    def fixed_cable_peers(self, peer_is_mac: bool) -> List[str]:
        """Gegenstelle .1/.2 im cable_subnet ohne Beacon."""
        if peer_is_mac:
            return [DEFAULT_MAC_HOST]
        return [DEFAULT_PC_HOST]

    def tick(self, force: bool = False) -> Optional[PathDecision]:
        now = self._clock()
        if not force and now - self._last_probe_at < PROBE_INTERVAL_S:
            return None
        self._last_probe_at = now
        for cand in list(self._candidates.values()):
            ok, rtt = self._probe(cand.addr, PROBE_PORT)
            cand.rtt_ms = rtt
            if ok:
                cand.probe_ok_streak += 1
                cand.probe_fail_streak = 0
            else:
                cand.probe_fail_streak += 1
                cand.probe_ok_streak = 0

        mode = self._settings.mode
        if mode == "wlan":
            return self._decision("wlan", switched=False)

        cable_ready = any(
            c.path == "cable" and c.probe_ok_streak >= CABLE_SUCCESS_NEEDED for c in self._candidates.values()
        )
        if mode == "cable":
            if not cable_ready:
                return None
            host = self._best_host("cable")
            return self._decision("cable", switched=host is not None, host=host)

        # auto
        if cable_ready and self._active != "cable":
            host = self._best_host("cable")
            if host:
                return self._decision("cable", switched=True, host=host)
        if self._active == "cable":
            bad = not any(c.path == "cable" and c.probe_ok_streak >= CABLE_SUCCESS_NEEDED for c in self._candidates.values())
            fails = any(c.path == "cable" and c.probe_fail_streak >= CABLE_FAIL_DROP for c in self._candidates.values())
            if bad or fails:
                return self._decision("wlan", switched=True)
        return None

    def _best_host(self, path: PathName) -> Optional[str]:
        cands = [c for c in self._candidates.values() if c.path == path and c.probe_ok_streak > 0]
        if not cands:
            return None
        cands.sort(key=lambda c: (c.rtt_ms if c.rtt_ms is not None else 1e9, c.addr))
        return cands[0].addr

    def _decision(self, path: PathName, switched: bool, host: Optional[str] = None) -> PathDecision:
        if path == "cable":
            bitrate = self._settings.max_kbps_cable
        else:
            bitrate = self._settings.max_kbps_wlan
        if host is None:
            host = self._best_host(path) or ""
        self._active = path
        return PathDecision(path=path, peer_host=host, bitrate_kbps=bitrate, switched=switched)


def filter_interfaces(
    interfaces: Sequence[Tuple[str, str]],
    cable_subnet: ipaddress.IPv4Network,
) -> List[Tuple[str, str, PathName]]:
    """(address, netmask, path) für Kabel-Kandidaten."""
    out: List[Tuple[str, str, PathName]] = []
    for address, netmask in interfaces:
        if is_cable_interface(address, netmask, cable_subnet):
            out.append((address, netmask, "cable"))
        else:
            out.append((address, netmask, "wlan"))
    return out
