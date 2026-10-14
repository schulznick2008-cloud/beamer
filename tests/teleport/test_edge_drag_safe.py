"""Kanten-Drag: inaktiv ohne Helfer-drag_state; Event-Tap bleibt KVM-tauglich."""
import queue
import time
from unittest import mock

from core.teleport_bridge import try_queue_edge_drag_teleport
from core.teleport_edge_drag import EdgeDragController


def test_without_drag_state_queue_returns_false_and_no_start():
    started = []

    def start(_intent):
        started.append(1)
        return True

    drag = EdgeDragController(start)
    assert drag.queue_edge_cross("pc", "right", 10.0, 20.0, dragging=True) is False
    time.sleep(0.05)
    assert started == []


def test_drag_state_cursor_used_for_place():
    placed = queue.Queue()

    def start(intent):
        placed.put((intent.place_x, intent.place_y))
        return True

    drag = EdgeDragController(start)
    drag.on_drag_state({"dragging": True, "title_bar": True, "wid": 9, "x": 55.5, "y": 66.6})
    assert drag.queue_edge_cross("pc", "right", 1.0, 2.0, dragging=True) is True
    x, y = placed.get(timeout=2.0)
    assert x == 55.5
    assert y == 66.6


def test_with_drag_state_starts_tp_offer_async():
    started = queue.Queue()

    def start(intent):
        started.put(intent.wid)
        return True

    drag = EdgeDragController(start)
    drag.on_drag_state({"dragging": True, "title_bar": True, "wid": 42})
    assert drag.queue_edge_cross("pc", "right", 100.0, 200.0, dragging=True) is True
    wid = started.get(timeout=2.0)
    assert wid == 42


def test_exception_in_try_queue_still_allows_kvm_path():
    """KVM-Übergang: Handler wirft, Rückgabe False, kein Abbruch des Tap-Pfads."""
    controller = mock.Mock()
    controller.edge_drag = mock.Mock()
    controller.edge_drag.queue_edge_cross.side_effect = RuntimeError("boom")
    controller.logger = mock.Mock()
    assert try_queue_edge_drag_teleport(controller, "pc", "left", 0, 0, True) is False
    controller.logger.exception.assert_called()


def test_handle_local_mouse_cross_does_not_call_pick_front_window():
    """Kantenüberquerung im Tap: nur try_queue_edge_drag_teleport, dann KVM; kein windows.list."""
    from pathlib import Path

    text = Path("mac_app/bridge.py").read_text(encoding="utf-8")
    assert "_edge_drag_title_hint" not in text
    crossed_idx = text.index("if step.crossed:")
    block = text[crossed_idx : crossed_idx + 3500]
    assert "_pick_front_window" not in block
    assert "windows.list" not in block
    tq = block.index("try_queue_edge_drag_teleport")
    sr = block.index("set_redirecting(True", tq)
    assert sr > tq
    assert "if try_queue_edge_drag_teleport" not in block


def test_drag_state_routed_via_service_not_tap():
    from core.teleport_service import TeleportService

    seen = []

    def post(_peer, _msg, _out):
        return True

    svc = TeleportService("macos", post_to_peer=post, helper_config={"cmd": "true"})
    svc.set_drag_state_handler(lambda ev: seen.append(ev))
    svc._on_helper_event({"ev": "windows.drag_state", "dragging": True, "title_bar": True, "wid": 7})
    assert seen == [{"ev": "windows.drag_state", "dragging": True, "title_bar": True, "wid": 7}]


def test_helper_event_format_arms_tracks_cursor_and_disarms():
    """Ereignis wie es der Mac-Helfer (C04d) schickt: dragging/title_bar/wid/x/y, am Ende dragging:false ohne wid."""
    drag = EdgeDragController(lambda _i: True)
    assert drag.is_armed() is False and drag.cursor() is None
    drag.on_drag_state({"v": 1, "ev": "windows.drag_state", "dragging": True, "title_bar": True, "wid": 21745, "x": 600.0, "y": 214.0})
    assert drag.is_armed() is True
    assert drag.cursor() == (600.0, 214.0)
    drag.on_drag_state({"v": 1, "ev": "windows.drag_state", "dragging": True, "title_bar": True, "wid": 21745, "x": 640.5, "y": 220.0})
    assert drag.cursor() == (640.5, 220.0)
    drag.on_drag_state({"v": 1, "ev": "windows.drag_state", "dragging": False, "title_bar": False, "x": 900.0, "y": 300.0})
    assert drag.is_armed() is False
    assert drag.cursor() is None


def test_title_bar_false_never_arms_and_edge_cross_stays_kvm():
    started = []
    drag = EdgeDragController(lambda i: started.append(i) or True)
    drag.on_drag_state({"dragging": True, "title_bar": False, "wid": 5, "x": 1.0, "y": 2.0})
    assert drag.is_armed() is False
    assert drag.queue_edge_cross("pc", "right", 10.0, 20.0, dragging=True) is False
    time.sleep(0.05)
    assert started == []
