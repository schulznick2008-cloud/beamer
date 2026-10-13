"""XFixes pointer barriers along the seams between this X11 desktop's monitors.

The small controller is independent of python-xlib so its geometry and resistance behaviour can
be checked without an X server. XFixes 5 creates the barriers; XI 2.3 reports hits and lets us
release a pointer after the configured pressure has been reached.
"""

import struct
from dataclasses import dataclass
from typing import Dict, Iterable, List, Tuple

from core.return_edge import Rect

XI_BARRIER_HIT = 25
XI_BARRIER_LEAVE = 26
XI_ALL_MASTER_DEVICES = 1
XI_BARRIER_EVENTS = (1 << XI_BARRIER_HIT) | (1 << XI_BARRIER_LEAVE)

_CREATE_POINTER_BARRIER = 31
_DESTROY_POINTER_BARRIER = 32
_XI_BARRIER_RELEASE_POINTER = 61


@dataclass(frozen=True)
class Seam:
    """A half-open monitor-to-monitor seam, represented as XFixes endpoints."""

    x1: int
    y1: int
    x2: int
    y2: int
    axis: str

    @property
    def key(self) -> Tuple[int, int, int, int]:
        return self.x1, self.y1, self.x2, self.y2


def seams(monitors: Iterable[Rect]) -> List[Seam]:
    """Return each shared edge segment once; external edges are deliberately omitted."""
    displays = list(monitors)
    found: Dict[Tuple[int, int, int, int], Seam] = {}
    for index, first in enumerate(displays):
        for second in displays[index + 1:]:
            if first.x + first.width == second.x or second.x + second.width == first.x:
                start = max(first.y, second.y)
                end = min(first.y + first.height, second.y + second.height)
                if end - start > 1:
                    x = (first.x + first.width if first.x + first.width == second.x
                         else second.x + second.width)
                    item = Seam(x, start, x, end - 1, "x")
                    found[item.key] = item
            if first.y + first.height == second.y or second.y + second.height == first.y:
                start = max(first.x, second.x)
                end = min(first.x + first.width, second.x + second.width)
                if end - start > 1:
                    y = (first.y + first.height if first.y + first.height == second.y
                         else second.y + second.height)
                    item = Seam(start, y, end - 1, y, "y")
                    found[item.key] = item
    return list(found.values())


def parse_barrier_event(kind: int, data: bytes) -> tuple:
    """Decode the XI 2.3 event body python-xlib leaves opaque on GenericEvent."""
    if kind not in (XI_BARRIER_HIT, XI_BARRIER_LEAVE):
        raise ValueError(f"not an XI barrier event: {kind}")
    # EVENTHEADER: deviceid, time; then eventid, root, event, barrier, full_sequence, dtime,
    # flags, sourceid, padding, root_x, root_y, dx and dy (FP3232 pairs).
    if len(data) < 62:
        raise ValueError("truncated XI barrier event")
    (deviceid, _time, eventid, _root, _event, barrier, _full_sequence, _dtime, _flags,
     _sourceid, root_x, root_y, dx_i, dx_f, dy_i, dy_f) = struct.unpack_from("=HIIIIIIIIH2xii iI iI", data)
    return ("barrier", kind, deviceid, eventid, barrier,
            dx_i + dx_f / 2 ** 32, dy_i + dy_f / 2 ** 32,
            root_x / 65536, root_y / 65536)


class Controller:
    """Reconcile active internal seams and release each barrier after configured outward travel."""

    def __init__(self, backend) -> None:
        self._backend = backend
        self._barriers: Dict[int, Seam] = {}
        self._signature = None
        self._pressure: Dict[Tuple[int, int, int], Tuple[int, float]] = {}
        self._released = set()

    @property
    def active(self) -> bool:
        return bool(self._barriers)

    def configure(self, monitors: Iterable[Rect], resistance_px: int, enabled: bool = True) -> None:
        resistance = max(0.0, float(resistance_px))
        wanted = seams(monitors) if enabled and resistance > 0 else []
        # X protocol coordinates are signed 16-bit screen coordinates. Omit only unrepresentable
        # seams; a bad display arrangement must not prevent ordinary input capture from starting.
        wanted = [item for item in wanted if all(-32768 <= point <= 32767 for point in item.key)]
        signature = (tuple(item.key for item in wanted), resistance, bool(enabled))
        if signature == self._signature:
            return
        self._remove_all()
        self._signature = signature
        if not enabled or resistance <= 0:
            return
        self._resistance = resistance
        try:
            for item in wanted:
                barrier = self._backend.create_barrier(item)
                if barrier is not None:
                    self._barriers[int(barrier)] = item
        except Exception:
            self._remove_all()
            self._signature = None
            raise

    def handle(self, event: tuple) -> None:
        """Consume (kind, device, event id, barrier, dx, dy, ...) from the capture thread."""
        _tag, kind, device, eventid, barrier, dx, dy = event[:7]
        key = (int(device), int(eventid), int(barrier))
        if kind == XI_BARRIER_LEAVE:
            self._pressure.pop(key, None)
            self._released.discard(key)
            return
        seam = self._barriers.get(int(barrier))
        if seam is None or key in self._released:
            return
        outward = float(dx if seam.axis == "x" else dy)
        # A hit event already means the pointer is pushing into the barrier. Direction is kept so
        # reversing against the same event id restarts, rather than completes, the pressure.
        if outward == 0:
            return
        direction, distance = (1 if outward > 0 else -1), abs(outward)
        previous_direction, accumulated = self._pressure.get(key, (direction, 0.0))
        if previous_direction != direction:
            accumulated = 0.0
        accumulated += distance
        self._pressure[key] = (direction, accumulated)
        if accumulated >= self._resistance:
            self._backend.release_pointer(device, barrier, eventid)
            self._released.add(key)

    def close(self) -> None:
        self._remove_all()
        self._signature = None

    def _remove_all(self) -> None:
        for barrier in list(self._barriers):
            try:
                self._backend.destroy_barrier(barrier)
            finally:
                self._barriers.pop(barrier, None)
        self._pressure.clear()
        self._released.clear()


def _request_classes():
    # python-xlib is optional on macOS and Windows; keep its protocol types lazy like capture_x11's
    # connection itself.
    from Xlib.protocol import rq

    class CreatePointerBarrier(rq.Request):
        _request = rq.Struct(rq.Card8("opcode"), rq.Opcode(_CREATE_POINTER_BARRIER), rq.RequestLength(),
                             rq.Card32("barrier"), rq.Card32("window"),
                             rq.Int16("x1"), rq.Int16("y1"), rq.Int16("x2"), rq.Int16("y2"),
                             rq.Card32("directions"), rq.LengthOf("devices", 2),
                             rq.List("devices", rq.Card16))

    class DestroyPointerBarrier(rq.Request):
        _request = rq.Struct(rq.Card8("opcode"), rq.Opcode(_DESTROY_POINTER_BARRIER), rq.RequestLength(),
                             rq.Card32("barrier"))

    class ReleasePointer(rq.Request):
        _request = rq.Struct(
            rq.Card8("opcode"), rq.Opcode(_XI_BARRIER_RELEASE_POINTER), rq.RequestLength(),
            rq.Card32("num_barriers"),
            rq.Card16("deviceid"), rq.Pad(2), rq.Card32("barrier"), rq.Card32("eventid"),
        )

    return CreatePointerBarrier, DestroyPointerBarrier, ReleasePointer


class XlibBackend:
    """The XFixes/XInput protocol requests needed by Controller, on Hooks' X connection."""

    def __init__(self, display, root) -> None:
        self._create, self._destroy, self._release = _request_classes()
        self._display = display
        self._protocol = display.display
        self._root = root
        self._xfixes_opcode = self._protocol.get_extension_major("XFIXES")
        self._xinput_opcode = self._protocol.get_extension_major("XInputExtension")

    def create_barrier(self, seam: Seam) -> int:
        barrier = self._protocol.allocate_resource_id()
        self._create(
            display=self._protocol, opcode=self._xfixes_opcode, barrier=barrier, window=self._root.id,
            x1=seam.x1, y1=seam.y1, x2=seam.x2, y2=seam.y2,
            directions=0, devices=[XI_ALL_MASTER_DEVICES],
        )
        return barrier

    def destroy_barrier(self, barrier: int) -> None:
        self._destroy(display=self._protocol, opcode=self._xfixes_opcode, barrier=barrier)

    def release_pointer(self, device: int, barrier: int, eventid: int) -> None:
        self._release(display=self._protocol, opcode=self._xinput_opcode, num_barriers=1,
                      deviceid=device, barrier=barrier, eventid=eventid)
