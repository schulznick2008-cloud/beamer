"""proxy.drag → tp_close return (Python-Seite)."""
from core.teleport_proxy_drag import ProxyDragController


def test_proxy_drag_ignored_when_not_at_edge():
    closed = []

    ctrl = ProxyDragController(lambda _p, tid: closed.append(tid), edge_at=lambda x, _y: x <= 8)
    ctrl.on_proxy_drag("peer", 3, {"dragging": True, "x": 50, "y": 10})
    assert closed == []


def test_proxy_drag_triggers_return_at_edge():
    closed = []

    ctrl = ProxyDragController(lambda _p, tid: closed.append(tid), edge_at=lambda x, _y: x <= 8)
    ctrl.on_proxy_drag("peer", 3, {"dragging": True, "x": 4, "y": 10})
    assert closed == [3]


def test_proxy_drag_ignored_on_mouse_up_event():
    closed = []

    ctrl = ProxyDragController(lambda _p, tid: closed.append(tid), edge_at=lambda x, _y: x <= 8)
    ctrl.on_proxy_drag("peer", 3, {"dragging": False, "x": 1, "y": 1})
    assert closed == []
