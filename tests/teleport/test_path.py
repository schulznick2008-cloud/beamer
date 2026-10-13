"""Tests für core.teleport_path (§7)."""
import ipaddress

from core.teleport_path import (
    PathChooser,
    PathSettings,
    filter_interfaces,
    is_cable_interface,
    parse_cable_subnet,
)


def test_cable_interface_detection():
    subnet = parse_cable_subnet("192.168.77.0/24")
    assert is_cable_interface("192.168.77.1", "255.255.255.0", subnet)
    assert is_cable_interface("169.254.12.34", "255.255.0.0", subnet)
    assert not is_cable_interface("10.0.0.5", "255.255.255.0", subnet)


def test_path_chooser_switches_to_cable_after_two_probes():
    probes = {"192.168.77.2": [False, True, True]}

    def probe(host, port):
        seq = probes[host]
        ok = seq.pop(0) if seq else False
        return ok, 1.5 if ok else 999.0

    chooser = PathChooser(PathSettings(mode="auto"), probe)
    chooser.note_beacon("192.168.77.2", "cable")
    assert chooser.tick(force=True) is None  # erste Probe fail
    assert chooser.tick(force=True) is None  # zweite Probe ok, Streak 1
    decision = chooser.tick(force=True)  # dritte Probe ok, Streak 2
    assert decision is not None
    assert decision.path == "cable"
    assert decision.switched is True
    assert decision.bitrate_kbps == 150_000


def test_filter_interfaces_labels_wlan_and_cable():
    subnet = ipaddress.IPv4Network("192.168.77.0/24")
    out = filter_interfaces([("192.168.77.1", "255.255.255.0"), ("10.0.0.2", "255.255.255.0")], subnet)
    assert ("192.168.77.1", "255.255.255.0", "cable") in out
    assert ("10.0.0.2", "255.255.255.0", "wlan") in out
