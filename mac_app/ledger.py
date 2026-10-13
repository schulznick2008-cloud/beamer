"""The Crossing ledger: saved intent, with stable native controls for each paired desktop."""

import AppKit
import objc

import theme
import widgets
from core import peerlist, ways


class _ActionButton(AppKit.NSButton):
    def acceptsFirstResponder(self):
        return self.isEnabled()

    def resetCursorRects(self):
        self.addCursorRect_cursor_(self.bounds(), AppKit.NSCursor.pointingHandCursor()
                                  if self.isEnabled() else AppKit.NSCursor.arrowCursor())


class _CellView(AppKit.NSView):
    def isFlipped(self):
        return True

    def hitTest_(self, point):
        hit = objc.super(_CellView, self).hitTest_(point)
        if hit is None:
            # Glyph hit bands extend beyond the cell while their visible outline stays inside.
            local = self.convertPoint_fromView_(point, self.superview())
            for child in self.subviews():
                if isinstance(child, Glyph):
                    hit = child.hitTest_(local)
                    if hit is not None:
                        return hit
        return hit


def quiet_button(title, callback):
    target = widgets.callback_target(callback)
    view = _ActionButton.alloc().initWithFrame_(((0, 0), (40, 24)))
    view.setTitle_(title)
    view.setTarget_(target)
    view.setAction_("fire:")
    view.setAccessibilityElement_(True)
    view.setAccessibilityRole_(AppKit.NSAccessibilityButtonRole)
    view.setBordered_(False)
    view.cell().setLineBreakMode_(AppKit.NSLineBreakByTruncatingTail)
    view.cell().setUsesSingleLineMode_(True)
    view.cell().setWraps_(False)
    view.setAttributedTitle_(widgets.attributed(title, 12.5, 500, "ink_2"))
    view.setAccessibilityLabel_(title)
    view._ledger_target = target
    return view


class Glyph(AppKit.NSView):
    def isFlipped(self):
        return True

    @objc.python_method
    def setup(self, row, kind):
        self.row, self.kind = row, kind
        self.hits = {}
        self.setAccessibilityElement_(False)
        keys = ways.PARTS if kind == "edge" else ways.CORNERS
        for key in keys:
            callback = (lambda key=key: row.control._ledger_edge(row.peer, key)) if kind == "edge" else (
                lambda key=key: row.control._ledger_corner(row.peer, key))
            button = quiet_button("", callback)
            button.setTransparent_(True)
            self.addSubview_(button)
            self.hits[key] = button
        return self

    @objc.python_method
    def refresh(self):
        held, entry = self.row.held, self.row.entry
        out = entry.get("left_out") is True or entry.get("in_use", True) is False
        side = self.row.side or held["side"] or "top"
        picked = ways.picked_thirds(held["methods"], held["parts"])
        for index, (key, button) in enumerate(self.hits.items()):
            if self.kind == "edge":
                vertical = side in ("left", "right")
                rect = ((-6 if side == "left" else 32, index * 26 / 3), (12, 26 / 3)) if vertical else (
                    (index * 38 / 3, -6 if side == "top" else 20), (38 / 3, 12))
                name = ways.part_names(side)[key]
                on = key in picked
                label = f"{side.capitalize()} edge, {name.lower()} third, {'on' if on else 'off'}"
                tooltip = f"{name} third: click to {'leave it out' if on else 'turn it on'}"
                enabled = bool(self.row.side) and not out
            else:
                x = 4.5 if key.endswith("left") else 33.5
                y = 4.5 if key.startswith("top") else 21.5
                rect = ((x - 7, y - 7), (14, 14))
                holder = ways.corner_holder(self.row.settings, key, exclude=self.row.peer)
                on = "corner" in held["methods"] and held["corner"] == key
                label = f"{key.replace('_', ' ').capitalize()} corner, {'on' if on else 'off'}"
                tooltip = (peerlist.label_for(self.row.settings["peers"], peer_id=holder) + "'s corner") if holder else (
                    f"{key.replace('_', ' ').capitalize()} corner: click to " +
                    ("turn it off" if on else "move the corner here" if "corner" in held["methods"] else "turn it on"))
                enabled = not holder and not out
            (x, y), size = rect
            button.setFrame_(((x+6, y+6), size))
            button.setEnabled_(enabled)
            button.setAccessibilityLabel_(label)
            button.setAccessibilityValue_("on" if on else "off")
            button.setToolTip_("Left out: Put back to change its ways" if out else tooltip)
        self.setNeedsDisplay_(True)

    def drawRect_(self, rect):
        AppKit.NSGraphicsContext.saveGraphicsState()
        transform = AppKit.NSAffineTransform.transform()
        transform.translateXBy_yBy_(6, 6)
        transform.concat()
        outline = AppKit.NSBezierPath.bezierPathWithRoundedRect_xRadius_yRadius_(((0, 0), (38, 26)), 2.5, 2.5)
        theme.colour("well").setFill()
        outline.fill()
        theme.colour("edge").setStroke()
        outline.setLineWidth_(1)
        outline.stroke()
        held = self.row.held
        if self.kind == "corner":
            for corner in ways.CORNERS:
                on = "corner" in held["methods"] and corner == held["corner"]
                holder = ways.corner_holder(self.row.settings, corner, exclude=self.row.peer)
                radius = 3.2 if on else 2
                x = 4.5 if corner.endswith("left") else 33.5
                y = 4.5 if corner.startswith("top") else 21.5
                dot = AppKit.NSBezierPath.bezierPathWithOvalInRect_(((x-radius, y-radius), (radius*2, radius*2)))
                colour = theme.colour("signal" if on or holder else "edge")
                if holder:
                    colour = colour.colorWithAlphaComponent_(.45)
                colour.setFill()
                colour.setStroke()
                dot.fill() if on else dot.stroke()
        else:
            side = self.row.side or held["side"] or "top"
            picked = ways.picked_thirds(held["methods"], held["parts"]) if self.row.side else []
            vertical = side in ("left", "right")
            length = 26 if vertical else 38
            for index, part in enumerate(ways.PARTS):
                start, end = index*length/3+1, (index+1)*length/3-1
                fixed = 0 if side in ("top", "left") else length
                if vertical:
                    fixed = 0 if side == "left" else 38
                else:
                    fixed = 0 if side == "top" else 26
                path = AppKit.NSBezierPath.bezierPath()
                path.moveToPoint_((fixed, start) if vertical else (start, fixed))
                path.lineToPoint_((fixed, end) if vertical else (end, fixed))
                on = part in picked
                theme.colour("signal" if on else "edge").colorWithAlphaComponent_(1 if on else .7).setStroke()
                path.setLineWidth_(3 if on else 1.5)
                path.setLineCapStyle_(AppKit.NSRoundLineCapStyle)
                path.stroke()
        AppKit.NSGraphicsContext.restoreGraphicsState()


class Row(AppKit.NSView):
    def isFlipped(self):
        return True

    @objc.python_method
    def setup(self, ledger, peer):
        self.ledger, self.control, self.peer = ledger, ledger.control, peer
        self.renaming = False
        self.setAccessibilityElement_(True)
        self.setAccessibilityRole_(AppKit.NSAccessibilityRowRole)
        self.cells = {}
        for key in ("machine", "edge", "corner", "notch", "keys"):
            cell = _CellView.alloc().init()
            self.addSubview_(cell)
            self.cells[key] = cell
        self.name = widgets.Label("", theme.TYPE["body"], 600)
        self.status = widgets.Label("", theme.TYPE["small"], ink="ink_3")
        self.rename = quiet_button("Rename", self.begin_rename)
        self.leave = quiet_button("Leave out", lambda: self.control._set_atlas_left_out(
            self.peer, not (self.entry.get("left_out") is True or self.entry.get("in_use", True) is False)))
        self.name_field = AppKit.NSTextField.alloc().init()
        self.name_field.setDelegate_(self.control)
        self.name_field.setFont_(theme.font(theme.TYPE["body"]))
        self.name_field.setHidden_(True)
        self.rename_hint = widgets.Label("Return saves, Escape cancels", theme.TYPE["small"], ink="ink_3", wrap=True)
        self.scope = widgets.Label("Only on this Mac", theme.TYPE["small"], ink="ink_3")
        for item in (self.name.view, self.status.view, self.rename, self.leave, self.name_field,
                     self.rename_hint.view, self.scope.view):
            self._add("machine", item)
        self.edge_glyph = Glyph.alloc().init().setup(self, "edge")
        self.corner_glyph = Glyph.alloc().init().setup(self, "corner")
        self.edge_title = quiet_button("Off", lambda: self.control._ledger_edge(self.peer))
        self.edge_detail = quiet_button("", lambda: self.control._ledger_edge(self.peer))
        self.corner_title = widgets.Label("", 12.5)
        self.corner_detail = widgets.Label("", 11, ink="ink_3")
        for label in (self.corner_title, self.corner_detail):
            label.view.cell().setLineBreakMode_(AppKit.NSLineBreakByTruncatingTail)
            label.view.cell().setUsesSingleLineMode_(True)
            label.view.cell().setWraps_(False)
        for item in (self.edge_glyph, self.edge_title, self.edge_detail):
            self._add("edge", item)
        for item in (self.corner_glyph, self.corner_title.view, self.corner_detail.view):
            self._add("corner", item)
        self.notch = self._check(lambda on: self.control._ledger_notch(self.peer, on))
        self.no_notch = widgets.Label("No notch", theme.TYPE["body"], ink="ink_3")
        self.cycle = self._check(lambda on: self.control._set_shortcut_cycle(self.peer, on))
        self._add("notch", self.notch)
        self._add("notch", self.no_notch.view)
        self._add("keys", self.cycle)
        self.jump_caption = widgets.Label("Jump", theme.TYPE["small"], ink="ink_3")
        self.jump = widgets.JumpKeyRecorder(on_change=lambda value: self.control._save_jump_key(value, self.peer),
                                          on_arm=self._arm_jump, compact=True)
        self.clear = quiet_button("Clear", lambda: self.control._save_jump_key("", self.peer))
        for item in (self.jump_caption.view, self.jump.view, self.clear):
            self._add("keys", item)
        return self

    @objc.python_method
    def _add(self, cell, view):
        view.setTranslatesAutoresizingMaskIntoConstraints_(True)
        self.cells[cell].addSubview_(view)

    @objc.python_method
    def _check(self, callback):
        target = widgets._Target.alloc().initWithCallback_(lambda sender: callback(sender.state() == AppKit.NSControlStateValueOn))
        view = _ActionButton.alloc().initWithFrame_(((0, 0), (100, 28)))
        view.setButtonType_(AppKit.NSSwitchButton)
        view.setTarget_(target)
        view.setAction_("fire:")
        view.setTitle_("")
        view.setFont_(theme.font(theme.TYPE["body"]))
        view.setAccessibilityElement_(True)
        view.setAccessibilityRole_(AppKit.NSAccessibilityCheckBoxRole)
        view._ledger_target = target
        return view

    @objc.python_method
    def _arm_jump(self, armed):
        if armed:
            for row in self.ledger.rows.values():
                if row is not self:
                    row.jump.cancel()
        self.control._jump_arming(any(row.jump.monitor is not None for row in self.ledger.rows.values()))
        self.jump.hint.view.setHidden_(not armed)
        self.ledger.reflow()

    @objc.python_method
    def begin_rename(self):
        if self.control.page != "crossing":
            self.control._select_page("crossing")
        self.renaming = True
        self.name_field.setStringValue_(self.entry.get("label") or self.entry.get("name") or "")
        self.ledger.reflow()
        self.control.window.makeFirstResponder_(self.name_field)
        self.name_field.selectText_(None)

    @objc.python_method
    def finish_rename(self, save):
        self.renaming = False
        if save:
            self.control._rename_peer(self.entry["token"], self.name_field.stringValue())
        self.ledger.refresh(self.control.settings_store.current(), self.control.chosen_peer)
        self.control.window.makeFirstResponder_(self.control.atlas_canvas)

    @objc.python_method
    def refresh(self, settings, entry, selected):
        self.settings, self.entry, self.selected = settings, entry, selected
        self.held = ways.ways(settings, self.peer)
        tiles, own = ways.derive_layout(settings, gap=0), settings.get("machine_id")
        contact = ways._rect_contact(tiles[own], tiles[self.peer]) if own in tiles and self.peer in tiles else None
        self.side = contact[0] if contact and contact[0] != "corner" else ""
        name = self.control._shown(peerlist.label_for(settings["peers"], peer_id=self.peer) or peerlist.UNNAMED)
        linked = self.control._live(self.peer)
        out = entry.get("left_out") is True or entry.get("in_use", True) is False
        self.dimmed = out or not linked
        platform = {"macos": "macOS", "windows": "Windows", "linux": "Linux"}.get(entry.get("platform"), "Machine")
        caps = self.control._design_peer_caps()
        status = "Left out" if out else "Linked" if linked else "Offline, ways kept"
        self.status_text = f"{platform} · {status}"
        self.old_peer = self.peer in caps and "layout" not in caps[self.peer]
        if self.old_peer:
            self.status_text = f"Beamer {entry.get('app') or '1.5.2'} · only sees which side"
        self.name.set(name, ink="ink_2" if self.dimmed else "ink")
        self.name.view.setToolTip_(name)
        self.name.view.setLineBreakMode_(AppKit.NSLineBreakByTruncatingTail)
        self.status.set(self.status_text)
        self.leave.setAttributedTitle_(widgets.attributed("Put back" if out else "Leave out", 12.5, 500, "ink_2"))
        self.leave.setAccessibilityLabel_("Put back" if out else "Leave out")
        self.leave.setToolTip_("Left out machines stay paired and linked. The shortcut, menu and clipboard still reach them." if out else "Leave this machine out of pointer crossings")
        picked = ways.picked_thirds(self.held["methods"], self.held["parts"]) if self.side else []
        title = ways.edge_form_detail(self.side, self.held["methods"], self.held["parts"]) if picked else "Off"
        detail = f"{self.side} edge" + (" off" if not picked else "") if self.side else "place it on the map"
        self.edge_text = title
        for button, text, size, ink in ((self.edge_title, title, 12.5, "ink" if picked else "ink_3"),
                                       (self.edge_detail, detail, 11, "ink_3")):
            button.setAttributedTitle_(widgets.attributed(text, size, ink=ink))
            button.setAlignment_(AppKit.NSTextAlignmentLeft)
            button.setAccessibilityLabel_("Edge, off" if not picked else
                                          f"Edge, whole {self.side} edge" if len(picked) == 3 else
                                          f"Edge, {title.lower()}, {self.side} edge")
            button.setEnabled_(bool(self.side) and not out)
            button.setToolTip_(text + ": " + ("Left out: Put back to change its ways" if out else
                               "Edge: click to turn it off" if picked else f"Edge: click for the whole {self.side} edge"))
        corner_on = "corner" in self.held["methods"]
        self.corner_title.set(self.held["corner"].replace("_", " ").capitalize() if corner_on else "None",
                              ink="ink" if corner_on else "ink_3")
        self.corner_detail.set("diagonal push" if corner_on else "click a dot")
        self.edge_glyph.refresh()
        self.corner_glyph.refresh()
        self.notch.setTitle_("" if not self.control.has_notch else
                            "On" if "notch" in self.held["methods"] else "Off")
        self.no_notch.view.setHidden_(self.control.has_notch)
        self.notch.setAccessibilityLabel_("Notch, no notch" if not self.control.has_notch else
                                         "Notch, on" if "notch" in self.held["methods"] else "Notch, off")
        self.notch.setState_(int("notch" in self.held["methods"]))
        self.notch.setEnabled_(self.control.has_notch and not out)
        self.notch.setToolTip_(f"Push into the notch to reach {name}")
        self.cycle.setTitle_(widgets.key_title(self.control.key_recorder.value))
        self.cycle.setState_(int(entry.get("shortcut_cycle") is not False))
        self.cycle.setToolTip_("In the shortcut's cycle")
        if self.jump.monitor is None:
            self.jump.set_value(entry.get("jump_key", ""))
        self.jump.hint.view.setHidden_(self.jump.monitor is None)
        self.jump.view.setToolTip_(f"Press {self.jump._title()} to send input to {name}; press again to bring it home.")
        self.clear.setHidden_(not bool(self.jump.value))
        self.rename.setAccessibilityLabel_(f"Rename {name}")
        self.leave.setAccessibilityLabel_(f"{'Put back' if out else 'Leave out'} {name}")
        self.setAccessibilityLabel_(name)
        self.setNeedsDisplay_(True)

    def mouseDown_(self, event):
        self.control._atlas_selected(self.peer)

    def accessibilityRowHeaderUIElements(self):
        return [self.name.view]

    @objc.python_method
    def arrange(self, width, narrow):
        rename_space = 74 if self.renaming else 0
        positions = self.ledger.columns(width)
        if narrow:
            half = (width-40)/2
            positions = [(0, width), (12, half), (half+28, half), (12, half), (half+28, half)]
        positions[0] = (12, positions[0][1] - 12)
        for key, (x, w) in zip(self.cells, positions):
            self.cells[key].setFrame_(((x, 12), (w, 72)))
            self.cells[key].setAlphaValue_(.5 if self.dimmed else 1)
        cell_heights = {"machine": 72 + rename_space, "notch": 28}
        for kind, glyph, title, detail in (("edge", self.edge_glyph, self.edge_title, self.edge_detail),
                                           ("corner", self.corner_glyph, self.corner_title.view, self.corner_detail.view)):
            w = max(1, self.cells[kind].bounds().size.width - 48)
            title_h, detail_h = 22, 20
            # The glyph's six-point hit halo is outside its 38 × 26 visible outline.
            glyph.setFrame_(((-6, -6), (50, 38)))
            title.setFrame_(((48, 0), (w, title_h)))
            detail.setFrame_(((48, title_h), (w, detail_h)))
            cell_heights[kind] = max(38, title_h + detail_h)
        w = self.cells["keys"].bounds().size.width
        self.cycle.setFrame_(((0, 0), (w, 28)))
        self.jump_caption.view.setFrame_(((0, 34), (35, 24)))
        jump_w, jump_h = self.jump.arrange_compact(max(1, w-36))
        self.jump.view.setFrame_(((36, 32), (jump_w, jump_h)))
        clear_x, clear_y = 36 + jump_w + 4, 32
        clear_w = 38
        if clear_x + clear_w > w:
            clear_x, clear_y = 36, 32 + jump_h + 2
        self.clear.setFrame_(((clear_x, clear_y), (clear_w, 24)))
        self.clear.setHidden_(not self.jump.value or self.jump.monitor is not None)
        cell_heights["keys"] = max(60, 32 + jump_h,
                                   clear_y + 24 if not self.clear.isHidden() else 0)
        if narrow:
            edge_y = 12 + cell_heights["machine"] + 12
            edge_h = max(cell_heights["edge"], cell_heights["corner"])
            keys_y = edge_y + edge_h + 12
            bottom_h = max(cell_heights["notch"], cell_heights["keys"])
            places = [(12, cell_heights["machine"]), (edge_y, edge_h), (edge_y, edge_h),
                      (keys_y, bottom_h), (keys_y, bottom_h)]
            height = keys_y + bottom_h + 12
        else:
            height = max(76, max(cell_heights.values()) + 24)
            places = [(12, height-24)] * 5
        for key, (y, h) in zip(self.cells, places):
            cell = self.cells[key]
            cell.setFrame_(((cell.frame().origin.x, y), (cell.bounds().size.width, h)))
        machine = self.cells["machine"].bounds().size.width
        self.name.view.setFrame_(((0, 0), (machine, 22)))
        self.status.view.setFrame_(((12, 25), (machine-12, 19)))
        self.rename.setFrame_(((0, 48), (54, 24)))
        self.leave.setFrame_(((56, 48), (min(74, machine-56), 24)))
        self.name_field.setFrame_(((0, 0), (machine, 24)))
        self.name_field.setHidden_(not self.renaming)
        self.name.view.setHidden_(self.renaming)
        for label, y in ((self.rename_hint, 74), (self.scope, 110)):
            label.view.setFrame_(((0, y), (machine, 32)))
            label.view.setHidden_(not self.renaming or label is self.scope and self.control.controller.cfg.same_on_both)
        w = self.cells["notch"].bounds().size.width
        self.notch.setFrame_(((0, 0), (w if self.control.has_notch else 22, 28)))
        self.no_notch.view.setFrame_(((0, 32), (w, 22)))
        return height

    def drawRect_(self, rect):
        width = self.bounds().size.width
        theme.colour("rule").setFill()
        AppKit.NSRectFill(((0, 0), (width, 1)))
        if self.selected:
            theme.colour("signal").setFill()
            AppKit.NSRectFill(((0, 12), (3, self.bounds().size.height-24)))
        cell = self.cells["machine"].frame()
        theme.colour("amber" if self.old_peer else "signal" if self.control._live(self.peer) else "ink_3").setFill()
        AppKit.NSBezierPath.bezierPathWithOvalInRect_(((cell.origin.x+1, cell.origin.y+32), (6, 6))).fill()


class Ledger(AppKit.NSView):
    def isFlipped(self):
        return True

    @objc.python_method
    def setup(self, control):
        self.control, self.rows = control, {}
        self.order = []
        self.setTranslatesAutoresizingMaskIntoConstraints_(False)
        self.height = self.heightAnchor().constraintEqualToConstant_(0)
        self.height.setActive_(True)
        self.setAccessibilityElement_(True)
        self.setAccessibilityRole_(AppKit.NSAccessibilityTableRole)
        self.setAccessibilityLabel_("Machines and crossing ways")
        self.headers = []
        for title in ("Machine", "Edge", "Corner", "Notch", "Keys"):
            label = widgets.Label(title, theme.TYPE["eyebrow"], 700, "ink_3",
                                  tracking=theme.TRACKING["eyebrow"], upper=True)
            label.view.setTranslatesAutoresizingMaskIntoConstraints_(True)
            label.view.setAccessibilityRole_(AppKit.NSAccessibilityColumnRole)
            self.addSubview_(label.view)
            self.headers.append(label)
        return self

    @objc.python_method
    def columns(self, width):
        weights = (1.3, 1, .9, 0, 1)
        widths = [132, 124, 104, 56, 128]
        remaining = max(488, width - 64 - 56)
        flexible = {0, 1, 2, 4}
        while flexible:
            unit = remaining / sum(weights[index] for index in flexible)
            fixed = {index for index in flexible if widths[index] > unit * weights[index]}
            if not fixed:
                for index in flexible:
                    widths[index] = unit * weights[index]
                break
            remaining -= sum(widths[index] for index in fixed)
            flexible -= fixed
        result, x = [], 0
        for w in widths:
            result.append((x, w))
            x += w+16
        return result

    @objc.python_method
    def refresh(self, settings, selected):
        entries = peerlist.desktops(settings["peers"])
        tiles = ways.derive_layout(settings, gap=0)
        entries.sort(key=lambda entry: (entry["id"] not in tiles,
                     tiles.get(entry["id"], {}).get("x", 0), tiles.get(entry["id"], {}).get("y", 0)))
        ids = [entry["id"] for entry in entries]
        for peer in list(self.rows):
            if peer not in ids:
                self.rows[peer].jump.cancel()
                self.rows.pop(peer).removeFromSuperview()
        for entry in entries:
            peer = entry["id"]
            if peer not in self.rows:
                row = Row.alloc().init().setup(self, peer)
                self.rows[peer] = row
                self.addSubview_(row)
            self.rows[peer].refresh(settings, entry, peer == selected)
        self.order = ids
        self.reflow()

    @objc.python_method
    def reflow(self):
        width = self.bounds().size.width
        narrow = width < 620
        y = 0 if narrow or not self.rows else 34
        for label, (x, w) in zip(self.headers, self.columns(width)):
            label.view.setHidden_(narrow or not self.rows)
            label.view.setFrame_(((x, 0), (w, 24)))
        for peer in self.order:
            row = self.rows[peer]
            h = row.arrange(width, narrow)
            row.setFrame_(((0, y), (width, h)))
            y += h
        self.height.setConstant_(y)
        self.setNeedsDisplay_(True)

    def setFrameSize_(self, size):
        objc.super(Ledger, self).setFrameSize_(size)
        if hasattr(self, "rows"):
            self.reflow()

    def accessibilityRows(self):
        return [self.rows[peer] for peer in self.order]

    def accessibilityColumns(self):
        return [header.view for header in self.headers]

    def accessibilityColumnHeaderUIElements(self):
        return [header.view for header in self.headers]
