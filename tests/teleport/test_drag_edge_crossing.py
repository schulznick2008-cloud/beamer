"""Titelleisten-Drag: Cursor am Beamer-Rand bzw. Fenster fest am Rand."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "mac_app"))

from crossing import DRAG_EDGE_TOLERANCE, CrossingEngine

BOUNDS = (0, 0, 1728, 1117)


def test_block_while_dragging_still_stops_kvm():
    machine = CrossingEngine(methods=("edge",), edge="right", resistance_px=0)
    step = machine.feed(1727.0, 500.0, 40, 0, BOUNDS, 1.0, dragging=True)
    assert step.crossed is False


def test_wide_band_sees_top_under_menu_bar():
    machine = CrossingEngine(methods=("edge",), edge="top", resistance_px=120)
    assert machine.edge_under_pointer(800.0, 20.0, BOUNDS) is None
    hit = machine.edge_under_pointer(800.0, 20.0, BOUNDS, tolerance=DRAG_EDGE_TOLERANCE)
    assert hit is not None and hit[0] == "top"


def test_jammed_side_and_top():
    right = CrossingEngine(methods=("edge",), edge="right")
    assert right.jammed_transition(1200.0, 500.0, 12.0, 0.0, (1200.0, 500.0))[0] == "right"
    assert right.jammed_transition(1210.0, 500.0, 12.0, 0.0, (1200.0, 500.0)) is None
    top = CrossingEngine(methods=("edge",), edge="top")
    assert top.jammed_transition(400.0, 28.0, 0.0, -8.0, (400.0, 28.0))[0] == "top"
    assert right.jammed_transition(400.0, 28.0, 0.0, -8.0, (400.0, 28.0)) is None
