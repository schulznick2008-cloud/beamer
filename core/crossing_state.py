"""Pure crossing events shared by both desktop apps and their headless shells.

The picture owns positions, each peer owns local intent, and zones are compiled output.
Callers save the Outcome and carry out its messages before re-arming their responder.
"""

import copy
import math
import time
from typing import TypedDict

from core import protocol, return_edge, settings_sync, ways


class Outcome(TypedDict):
    settings: dict
    changed: bool
    accepted: bool
    snap: dict
    tiles: dict
    tell: list
    send_picture: bool
    forward_picture: bool


def _outcome(before, saved=None, result=None, *, accepted=True, tell=(),
             send_picture=False, forward_picture=False):
    saved = copy.deepcopy(before if saved is None else saved)
    return Outcome(settings=saved, changed=saved != before, accepted=accepted,
                   snap=copy.deepcopy((result or {}).get("snap", {})),
                   tiles=copy.deepcopy((result or {}).get("tiles", {})),
                   tell=list(dict.fromkeys(tell)), send_picture=send_picture,
                   forward_picture=forward_picture)


def _initial(settings, capable=(), fill=True):
    saved = copy.deepcopy(settings)
    for entry in saved.get("peers", []):
        if ways._zoned(entry):
            entry["ways"] = ways._read_intent(settings, entry["id"])
            entry.pop("left_out_ways", None)
    if not saved.get("layout"):
        saved["layout"] = ways.derive_layout(saved, gap=0)
    elif fill:
        # A peer paired after the layout existed gets its tile here, or compile clears its side for
        # want of a contact and every render invents a new place for it (rc13, the AC laptop). Not
        # stamped: a machine back from offline must not outrank the picture the others moved since.
        # Only a machine whose side is known: an unplaced one is never given a side it did not choose.
        # Only a picture machine: an old peer keeps its side without a tile, and a tile would make the
        # Mac count it as one until its link says otherwise, so every link-up re-armed the zones.
        picture = ways.picture_capable(saved, capable)
        derived = ways.derive_layout(saved, gap=0)
        for entry in saved.get("peers", []):
            if (ways._zoned(entry) and entry["id"] in picture and entry.get("side") in ways.EDGES
                    and entry["id"] not in saved["layout"] and entry["id"] in derived):
                saved["layout"][entry["id"]] = derived[entry["id"]]
    return saved


def _compile(saved, platform, *, capable=(), live=None, gone=None, now=None):
    return ways.compile_layout(saved, saved["layout"], platform=platform, capable=capable,
                               live=live, gone=gone, now=now)


def _tells(before, saved, *, sides=False):
    old = ways.way_back_state(before)
    told = [peer for peer, value in ways.way_back_state(saved).items() if old.get(peer) != value]
    if sides:
        held = {p.get("id"): p.get("side", "") for p in before.get("peers", [])}
        told += [p["id"] for p in saved.get("peers", [])
                 if ways._zoned(p) and held.get(p["id"], "") != p.get("side", "")]
    return list(dict.fromkeys(told))


def _stamp(saved, now):
    saved["layout_set_at"] = settings_sync.next_stamp(saved.get("layout_set_at"), now)
    saved["layout_by"] = saved.get("machine_id", "")


def _shelf(settings, gone=None):
    return {p.get("id") for p in settings.get("peers", [])
            if p.get("left_out") is True or p.get("in_use", True) is False
            or ways.peerlist.dormant_reason(p) is not None or gone is not None and gone(p.get("id"))}


def drop(settings, tiles, moved, *, platform, capable, live, gone, now, snap_radius):
    """Validate and snap a map drop, keeping shelved tiles aside."""
    saved = _initial(settings, capable)
    shelf = _shelf(saved, gone)
    if moved in shelf or not isinstance(tiles, dict):
        return _outcome(settings, accepted=False)
    held = saved["layout"]
    active = copy.deepcopy({peer: tile for peer, tile in tiles.items() if peer not in shelf})
    result = ways.compile_layout(saved, active, moved=moved, platform=platform,
                                capable=capable, live=live, gone=gone, now=now)
    if not result["accepted"]:
        snap = result.get("snap", {}).get(moved)
        if snap is None or snap == active.get(moved):
            return _outcome(settings, result=result, accepted=False)
        active[moved] = {**active[moved], **snap}
        result = ways.compile_layout(saved, active, moved=moved, platform=platform,
                                    capable=capable, live=live, gone=gone, now=now)
        if not result["accepted"]:
            return _outcome(settings, result=result, accepted=False)
    if moved in active and moved != saved.get("machine_id"):
        original = active[moved]
        others = {peer: tile for peer, tile in active.items() if peer != moved}
        snap = ways.snap_position(original, saved["machine_id"], others, radius=snap_radius)
        if ways.within_snap_radius(original, snap, saved["machine_id"], others, snap_radius):
            candidate = {**active, moved: {**original, **snap}}
            retry = ways.compile_layout(saved, candidate, moved=moved, platform=platform,
                                        capable=capable, live=live, gone=gone, now=now)
            if retry["accepted"]:
                active, result = candidate, retry
    saved["layout"] = {**{peer: copy.deepcopy(tile) for peer, tile in held.items() if peer in shelf},
                       **active}
    _stamp(saved, now)
    entry = ways._entry(saved, moved)
    own = saved["layout"].get(saved.get("machine_id"))
    if entry is not None and moved not in capable and own is not None and moved in saved["layout"]:
        # An old peer (no `layout`) crosses by its side, not the picture: a drop is how the map sets it,
        # and the side check below sends it the arrangement.
        contact = ways._rect_contact(own, saved["layout"][moved])
        if contact and contact[0] in ways.EDGES and contact[0] != entry.get("side"):
            entry.update(side=contact[0], side_set_at=settings_sync.next_stamp(entry.get("side_set_at"), now),
                         side_by=saved.get("machine_id", ""))
    result = _compile(saved, platform, capable=capable, live=live, gone=gone, now=now)
    saved = result["settings"]
    return _outcome(settings, saved, result, tell=_tells(settings, saved, sides=True), send_picture=True)


def edit_ways(settings, peer, *, platform, edge=None, thirds=None, corner=None,
              corner_at=None, notch=None, live, gone, now, capable=()):
    """Update only specified choices; an empty third selection keeps the last choice."""
    saved = _initial(settings, capable)
    entry = ways._entry(saved, peer)
    if entry is None:
        raise KeyError(peer)
    if not ways._zoned(entry):
        raise ValueError("a phone cannot have ways")
    intent = entry["ways"]
    for key, value in (("edge", edge), ("corner", corner), ("notch", notch)):
        if value is not None and (key != "notch" or platform == "mac"):
            intent[key] = bool(value)
    if thirds is not None:
        picked = [part for part in ways.PARTS if part in thirds]
        if picked:
            intent["thirds"] = picked
    if corner_at in ways.CORNERS:
        intent["corner_at"] = corner_at
    result = _compile(saved, platform, capable=capable, live=live, gone=gone, now=now)
    saved = result["settings"]
    return _outcome(settings, saved, result, tell=_tells(settings, saved))


def receive_picture(settings, state, *, source, platform, capable, live, gone, own_displays, now):
    """Take newer positions, retaining omitted displays and correcting only this owner's geometry."""
    if not settings_sync.layout_arrived(state, settings.get("layout_set_at"), settings.get("layout_by", "")):
        return _outcome(settings)
    # A missing tile is placed against the arriving picture (retain_held_peer_tiles), not this one.
    saved = _initial(settings, fill=False)
    layout = ways.retain_held_peer_tiles(saved, state["tiles"], saved["layout"])
    for peer, tile in layout.items():
        held = saved["layout"].get(peer, {})
        if "displays" in held and "displays" not in tile:
            tile["displays"] = copy.deepcopy(held["displays"])
            if "display_offsets" in tile:
                tile["display_offsets"] = {key: offset for key, offset in tile["display_offsets"].items()
                                           if key in tile["displays"]}
    own = saved.get("machine_id")
    saved.update(layout=layout, layout_set_at=state["set_at"], layout_by=state["by"])
    corrected = ways.refresh_displays(saved, own_displays, now=now)
    if (own_displays and not corrected
            and state["tiles"].get(own, {}).get("displays") != saved["layout"][own].get("displays")):
        _stamp(saved, now)
        corrected = True
    result = _compile(saved, platform, capable=capable, live=live, gone=gone, now=now)
    if not result["accepted"]:
        return _outcome(settings)
    saved = result["settings"]
    return _outcome(settings, saved, result, tell=_tells(settings, saved),
                    send_picture=corrected, forward_picture=True)


def receive_arrangement(settings, peer, read, *, platform, capable, live, gone, now):
    """Picture peers supply return-way status only; old peers settle a side by its stamp."""
    saved = copy.deepcopy(settings)
    entry = ways._entry(saved, peer)
    author = read.get("by")
    edge = read.get("edge")
    stamp = read.get("set_at")
    if (entry is None or not ways._zoned(entry) or edge not in ways.EDGES
            or not isinstance(author, bytes) or len(author) != 16 or not any(author)
            or type(stamp) is not int or not 0 <= stamp <= settings_sync.MAX_STAMP
            or stamp > now + settings_sync.DAY):
        return _outcome(settings)
    back = read.get("way_back")
    if isinstance(back, bool):
        entry["way_back"] = back
        holder = read.get("way_back_by")
        if (back is False and isinstance(holder, bytes) and len(holder) == 16 and any(holder)
                and holder != protocol.read_id(settings.get("machine_id"))):
            entry["way_back_by"] = protocol.id_text(holder)
        else:
            entry.pop("way_back_by", None)
    if peer in capable:
        return _outcome(settings, saved)
    held = (int(entry.get("side_set_at") or 0), protocol.read_id(entry.get("side_by")) or b"")
    newer = (stamp, author) > held
    if newer:
        entry.update(side=return_edge.OPPOSITE[edge], side_set_at=stamp, side_by=protocol.id_text(author))
        saved = _initial(saved, capable)
        saved["layout"].setdefault(peer, dict(x=0, y=0, w=160, h=100))
        own_id = saved["machine_id"]
        placed = ways.place_beside(saved["layout"], own_id, peer, entry["side"])
        contact = ways._rect_contact(saved["layout"][own_id], placed)
        # A side another machine already holds is shared, as derive_layout shares it, not cornered.
        shared = {} if contact and contact[0] == entry["side"] else ways.share_side(
            saved["layout"], own_id, peer, entry["side"], _shelf(saved, gone))
        saved["layout"].update(shared or {peer: placed})
        if saved["layout"] != settings.get("layout"):
            _stamp(saved, now)
        result = _compile(saved, platform, capable=capable, live=live, gone=gone, now=now)
        saved = result["settings"]
    else:
        result = None
    changed = saved != settings
    return _outcome(settings, saved, result, tell=[*_tells(settings, saved), peer] if changed else [],
                    send_picture=newer and saved.get("layout") != settings.get("layout"))


def link_up(settings, peer, *, platform, own_displays, now, capable=()):
    saved = _initial(settings, capable)
    ways.refresh_displays(saved, own_displays, now=now)
    result = _compile(saved, platform, capable=capable, now=now)
    saved = result["settings"]
    entry = ways._entry(saved, peer) or {}
    return _outcome(settings, saved, result, tell=[peer] if entry.get("side") in ways.EDGES else [],
                    send_picture=True)


def leave_out(settings, peer, *, platform, live=None, gone=None, now=None, capable=()):
    saved = _initial(settings, capable)
    entry = ways._entry(saved, peer)
    if entry is None:
        raise KeyError(peer)
    entry["left_out"] = True
    result = _compile(saved, platform, capable=capable, live=live, gone=gone, now=now)
    saved = result["settings"]
    return _outcome(settings, saved, result, tell=_tells(settings, saved))


def put_back(settings, peer, *, platform, live=None, gone=None, now=None, capable=()):
    saved = _initial(settings, capable)
    entry = ways._entry(saved, peer)
    if entry is None:
        raise KeyError(peer)
    entry.pop("left_out", None)
    # Its kept tile may sit where an active machine was placed while it was shelved: it moves, not them.
    tile, own = saved["layout"].get(peer), saved.get("machine_id")
    shelf = _shelf(saved, gone)
    others = {key: value for key, value in saved["layout"].items() if key != peer and key not in shelf}
    moved = False
    if (isinstance(tile, dict) and peer not in shelf and own in others
            and any(ways._overlaps(tile, other) for other in others.values())):
        side = entry.get("side") if entry.get("side") in ways.EDGES else ""
        shared = ways.share_side(saved["layout"], own, peer, side, shelf) if side else {}
        saved["layout"].update(shared or {peer: {**tile, **ways.snap_position(tile, own, others, radius=math.inf)}})
        moved = saved["layout"] != settings.get("layout")
        if moved:
            # Sent, or the next picture from another machine puts it back on top of the active one.
            _stamp(saved, time.time() if now is None else now)
    result = _compile(saved, platform, capable=capable, live=live, gone=gone, now=now)
    saved = result["settings"]
    return _outcome(settings, saved, result, tell=_tells(settings, saved), send_picture=moved)


def displays_changed(settings, *, platform, own_displays, now, capable=()):
    saved = _initial(settings, capable)
    changed = ways.refresh_displays(saved, own_displays, now=now)
    result = _compile(saved, platform, capable=capable, now=now)
    saved = result["settings"]
    return _outcome(settings, saved, result, tell=_tells(settings, saved), send_picture=changed)


def paired(settings, *, platform, own_displays, now, capable=()):
    saved = _initial(settings, capable)
    desktops = [p for p in saved.get("peers", []) if ways._zoned(p)]
    if platform == "mac" and len(desktops) == 1 and not settings.get("layout"):
        peer = desktops[0]["id"]
        saved["layout"][peer] = ways.place_beside(saved["layout"], saved["machine_id"], peer, "right")
    ways.refresh_displays(saved, own_displays, now=now)
    result = _compile(saved, platform, capable=capable, now=now)
    saved = result["settings"]
    return _outcome(settings, saved, result, tell=_tells(settings, saved, sides=True), send_picture=True)


def migrate(settings, platform, *, capable=()):
    """Read legacy zone choices once, then discard their historical marks by compiling."""
    saved = _initial(settings, capable)
    result = _compile(saved, platform, capable=capable)
    return _outcome(settings, result["settings"], result)
