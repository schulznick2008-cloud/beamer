"""Mac: Vorderfenster per NSWorkspace + Helfer windows.list (Stdlib + optional AppKit)."""
from __future__ import annotations

from typing import Any, Callable, Dict, Mapping, Optional


def pick_window_for_frontmost(
    windows: list,
    *,
    app_name: str,
    pid: Optional[int] = None,
) -> Optional[Dict[str, Any]]:
    """Größtes Fenster der vordersten App (pid bevorzugt)."""
    if not windows:
        return None
    cands = []
    for w in windows:
        if pid is not None and int(w.get("pid") or 0) == int(pid):
            cands.append(w)
        elif app_name and str(w.get("app", "")) == app_name:
            cands.append(w)
    if not cands:
        return None
    return max(cands, key=lambda w: float(w.get("w", 0)) * float(w.get("h", 0)))


def frontmost_via_appkit() -> tuple[Optional[str], Optional[int]]:
    try:
        import AppKit  # type: ignore
    except ImportError:
        return None, None
    app = AppKit.NSWorkspace.sharedWorkspace().frontmostApplication()
    if app is None:
        return None, None
    name = app.localizedName() or ""
    pid = int(app.processIdentifier())
    return name, pid


def pick_frontmost_window(list_windows: Callable[[], Mapping[str, Any]]) -> Optional[Dict[str, Any]]:
    app_name, pid = frontmost_via_appkit()
    wl = list_windows()
    if not wl.get("ok"):
        return None
    windows = wl.get("windows") or []
    return pick_window_for_frontmost(windows, app_name=app_name or "", pid=pid)
