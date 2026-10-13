"""The Overview's list of paired machines and the pairing sheet (WIRE.md section 6).

The list shows every machine this Mac has paired, each with its state, the two directions as that
machine's own switches, and a way to remove it. The sheet is the other half of pairing: this Mac
shows a code (and the same code as a QR) for another machine to enter, or enters one shown by a
machine it hears on the network, or at an address typed in. The sentences are core/peerlist.py's,
the same on Windows.

All of it runs on the main thread. The pairing itself blocks, so it runs on a thread of its own
and comes back through AppHelper; `PairingService` calls `store` and `peers` from its socket
threads, so nothing here hops to the main thread from those.
"""

import re
import socket
import threading

import AppKit
from PyObjCTools import AppHelper

import machines
import motion
import qr
import theme
import widgets
from core import feature_flags, pairing, peerlist, ways as core_ways

QR_SIDE = 176
CODE_GROUP = 3


def grouped(code):
    """The six digits as two groups of three, which is how a person reads them across a room."""
    return " ".join(code[start:start + CODE_GROUP] for start in range(0, len(code), CODE_GROUP))


def seconds(count):
    return f"Expires in {count} s"


class _RenameDelegate(AppKit.NSObject):
    def control_textView_doCommandBySelector_(self, control, _text_view, selector):
        if selector == "insertNewline:":
            self.panel._save_rename(self.token, control.stringValue())
            return True
        if selector == "cancelOperation:":
            self.panel._cancel_rename(self.token)
            return True
        return False


class MachinesPanel:
    def __init__(self, controller, settings_store, logger, owner):
        self.controller = controller
        self.settings_store = settings_store
        self.logger = logger
        self.owner = owner
        # Set by the app once the beacon listener exists.
        self.service = None
        self.open = False
        self.removing = None
        self.chosen = None
        self.pairing = False
        self._focus_after = None
        self.wide = True
        self._targets = []
        self._rows_key = None
        self._rename_views = {}
        self._cycle_controls = {}
        self.directions_open = set()
        self._heard_key = None
        self._heard = []
        self._showing = False
        self._shown_outcome = None
        self._hosted = None
        self._pair_ink = "ink_2"
        self._qr_for = None
        self.machines_module = self._machines_module()
        self.sheet = self._sheet()

    # Building

    def _machines_module(self):
        module = widgets.Module()
        header = widgets.stack(spacing=12)
        widgets.add(header, widgets.eyebrow("Machines"))
        self.pair_button = widgets.action_button("Pair a machine", self._toggle_sheet, style="primary")
        widgets.add(header, self.pair_button.view, full_width=False)
        module.add(header)
        self.list = widgets.stack(spacing=8)
        module.add(self.list)
        self.empty = widgets.note("No machines paired yet.")
        module.add(self.empty.view)
        line = widgets.stack(vertical=False, spacing=10)
        self.list_note = widgets.note("Beamer connects to each machine on its own whenever both are running.", reading=False)
        line.addArrangedSubview_(self.list_note.view)
        module.add(line)
        return module

    def own_name_module(self):
        module = widgets.Module()
        module.add(widgets.eyebrow("This Mac"))
        line = widgets.stack(vertical=False, spacing=9)
        own_name = self.settings_store.current().get("name", "")
        name = widgets.Label(own_name or socket.gethostname().split(".")[0] or "Mac",
                             theme.TYPE["body"], 600)
        widgets.squeeze(name.view)
        line.addArrangedSubview_(name.view)
        rename = self._chip("Rename", lambda: self._begin_rename(None), "Rename this Mac")
        line.addArrangedSubview_(rename)
        module.add(line)
        box, edit = widgets.field(mono=False)
        edit.setAccessibilityLabel_("Name for this Mac")
        edit.setStringValue_(own_name)
        box.setHidden_(True)
        note = widgets.note("Only on this Mac", reading=False)
        note.view.setHidden_(True)
        delegate = _RenameDelegate.alloc().init()
        delegate.panel, delegate.token = self, None
        edit.setDelegate_(delegate)
        module.add(box)
        module.add(note.view)
        self._own_rename_views = (name.view, rename, box, edit, note, delegate)
        return module

    def _sheet(self):
        module = widgets.Module()
        module.add(widgets.eyebrow("Pair a machine"))
        self.pair_grid = widgets.stack(spacing=20)
        self.pair_grid.setAlignment_(AppKit.NSLayoutAttributeTop)
        show = self._show_half()
        enter = self._enter_half()
        self.pair_grid.addArrangedSubview_(show)
        self.pair_grid.addArrangedSubview_(enter)
        module.add(self.pair_grid)
        self.pair_stacked = [
            view.widthAnchor().constraintEqualToAnchor_(self.pair_grid.widthAnchor()) for view in (show, enter)
        ]
        return module

    def _show_half(self):
        half = widgets.stack(spacing=8)
        widgets.add(half, widgets.label("Show a code on this Mac", theme.TYPE["body"], 600))
        widgets.add(half, widgets.note(
            "On the other machine, open Beamer, choose this Mac in its list and type the code."
        ).view)
        self.show_button = widgets.action_button("Show a code", self._show_code, style="primary")
        half.addArrangedSubview_(self.show_button.view)
        self.shown_code_boxes = widgets.CodeBoxes(pairing.CODE_DIGITS, None, editable=False)
        for control in self.shown_code_boxes.fields:
            control.setFont_(theme.mono_font(theme.TYPE["readout"], 700))
        widgets.add(half, self.shown_code_boxes.view)
        self.code_expiry = widgets.Label("", theme.TYPE["small"], mono=True, ink="ink_3")
        widgets.add(half, self.code_expiry.view)
        self.code_address = widgets.Label("", theme.TYPE["small"], ink="ink_3")
        widgets.add(half, self.code_address.view)
        self.qr_view = widgets._autolayout(AppKit.NSImageView.alloc().init())
        # Whole points to a module already: scaling would soften the edges.
        self.qr_view.setImageScaling_(AppKit.NSImageScaleNone)
        self.qr_view.setImageAlignment_(AppKit.NSImageAlignTopLeft)
        self.qr_view.setAccessibilityLabel_("The pairing code as a QR code")
        widgets.size(self.qr_view, QR_SIDE, QR_SIDE)
        half.addArrangedSubview_(self.qr_view)
        self.qr_caption = widgets.note("")
        widgets.add(half, self.qr_caption.view)
        self.hosted_status = widgets.note()
        widgets.add(half, self.hosted_status.view)
        self.cancel_button = widgets.action_button("Cancel code", self._cancel_code, scale="small")
        half.addArrangedSubview_(self.cancel_button.view)
        self._show_code_widgets(False)
        return half

    def _enter_half(self):
        half = widgets.stack(spacing=8)
        widgets.add(half, widgets.label("Enter a code from another machine", theme.TYPE["body"], 600))
        heard = widgets.stack(spacing=8)
        widgets.add(heard, widgets.label("On this network", theme.TYPE["note"], 600, ink="ink_2"))
        self.heard_list = widgets.stack(spacing=1)
        self.heard_frame = widgets.box("rule")
        self.heard_frame.addSubview_(self.heard_list)
        widgets.pin(self.heard_list, self.heard_frame, (1, 1, 1, 1))
        widgets.add(heard, self.heard_frame)
        self.heard_empty = widgets.note()
        widgets.add(heard, self.heard_empty.view)
        find_box, self.find_field = widgets.field()
        find_secret_box, self.find_secret = widgets.field(secure=True)
        find_secret_box.setHidden_(True)
        self.find_boxes = (find_box, find_secret_box)
        for control in (self.find_field, self.find_secret):
            control.setPlaceholderAttributedString_(widgets.attributed("Address", theme.TYPE["small"], ink="ink_3", mono=True))
            control.setAccessibilityLabel_("Address of a machine that is not listed")
            control.setTarget_(self._target(self._address_typed))
            control.setAction_("fire:")
        heard.setCustomSpacing_afterView_(14, self.heard_empty.view)
        heard.setCustomSpacing_afterView_(14, self.heard_frame)
        widgets.add(heard, widgets.label("Not listed? Type its address.", theme.TYPE["note"], ink="ink_2"))
        find_line = widgets.stack(vertical=False, spacing=8)
        for box in self.find_boxes:
            find_line.addArrangedSubview_(widgets.hug(box, AppKit.NSLayoutPriorityDefaultLow))
        widgets.add(heard, find_line)
        widgets.add(half, heard)
        widgets.add(half, widgets.label("Code shown on the other machine", theme.TYPE["note"], 600, ink="ink_2"))
        self.code_boxes = widgets.CodeBoxes(pairing.CODE_DIGITS, lambda _control: self._pair())
        widgets.add(half, self.code_boxes.view)
        line = widgets.stack(vertical=False, spacing=10)
        line.setAlignment_(AppKit.NSLayoutAttributeTop)
        self.pair_status = widgets.note()
        line.addArrangedSubview_(self.pair_status.view)
        self.confirm_button = widgets.action_button("Pair", self._pair, style="primary")
        line.addArrangedSubview_(self.confirm_button.view)
        widgets.add(half, line)
        self._say("Choose a machine, or type its address, then type the code it shows.")
        return half

    def _target(self, callback):
        target = widgets.callback_target(callback)
        self._targets.append(target)
        return target

    # Reading the window's state

    def _shown(self, text):
        return self.owner._shown(text)

    def rows(self):
        """The list's rows, from the controller and the settings as they stand now."""
        controller = self.controller
        peers = controller.book.peers()
        names = peerlist.labels(peers)
        inbound = self.owner.inbound_ids()
        rows = []
        for entry in peers:
            link = controller.links.get(entry["token"])
            ident = entry.get("id") or None
            rows.append(machines.row(
                entry, names[entry["token"]],
                live=bool(link is not None and link.live()),
                kind=link.kind if link is not None else "none",
                status=link.status if link is not None else "",
                here=ident is not None and controller.owner.on == ident,
                driving=ident is not None and controller.driver == ident,
                inbound=ident is not None and ident in inbound,
                has_link=link is not None,
            ))
        return rows

    # The list

    def refresh(self):
        rows = self.rows()
        hide = self.controller.cfg.hide_addresses
        # Not the sentence under each state: a link that is still trying changes it every attempt,
        # and a rebuild takes keyboard and VoiceOver focus off whatever it was on.
        key = (hide, self.removing, self.wide, tuple(
            (r.token, r.label, r.platform, r.address, r.state.key, r.state.word, r.state.tone, r.state.led,
             r.state.detail if r.state.key == "dormant" else "",
             r.in_use, r.left_out, r.drives, r.driven)
            for r in rows))
        key += (tuple(sorted(self.directions_open)),)
        mode = getattr(getattr(self.owner, "shortcut_goes_to", None), "value", "last")
        key += (mode, tuple((entry.get("token"), entry.get("shortcut_cycle") is not False)
                            for entry in self.controller.book.peers()))
        if key != self._rows_key:
            self._rows_key = key
            self._build_rows(rows)
        self.empty.view.setHidden_(bool(rows))
        self.list_note.view.setHidden_(not rows)
        wanted = self.open or not rows
        motion.set_hidden(self.sheet.view, not wanted)
        self.pair_button.set_title("Cancel" if self.open and rows else "Pair a machine")
        self.pair_button.view.setHidden_(False)
        if wanted:
            self._refresh_sheet(hide)
        elif self._showing:
            self._cancel_code()

    def _build_rows(self, rows):
        self._cycle_controls = {}
        focus = self._focus_after or self._focused_control()
        self._focus_after = None
        for view in list(self.list.arrangedSubviews()):
            self.list.removeArrangedSubview_(view)
            view.removeFromSuperview()
        for row in rows:
            widgets.add(self.list, self._row_view(row))
        if focus is not None:
            self._focus(focus)

    def _focused_control(self):
        """The id of the list control that has keyboard focus, or None."""
        view = self.owner.window.firstResponder()
        if view is None or not hasattr(view, "isDescendantOf_") or not view.isDescendantOf_(self.list):
            return None
        while view is not None and getattr(view, "focus_id", None) is None:
            view = view.superview()
        return getattr(view, "focus_id", None)

    def _focus(self, ident):
        """Gives focus to the control rebuilt under `ident`, as a rebuild takes it off the old one."""
        stack = [self.list]
        while stack:
            view = stack.pop()
            if getattr(view, "focus_id", None) == ident:
                self.owner.window.makeFirstResponder_(view)
                return
            stack.extend(view.subviews())

    def _row_view(self, row):
        state = row.state
        frame = widgets.box("ground", "rule", theme.RADIUS["field"])
        label = self._shown(row.label)
        frame.setAccessibilityLabel_(f"{label}, {self._shown(state.word)}")
        body = widgets.stack(spacing=6)
        head = widgets.stack(vertical=False, spacing=9)
        led = widgets.LED()
        led.set(state.led, state.blink)
        head.addArrangedSubview_(led.view)
        name = widgets.Label(label, theme.TYPE["body"], 600, wrap=True)
        head.addArrangedSubview_(widgets.hug(widgets.squeeze(name.view), AppKit.NSLayoutPriorityDefaultLow))
        rename = self._chip("Rename", lambda t=row.token: self._begin_rename(t), f"Rename {label}")
        head.addArrangedSubview_(rename)
        tag = widgets.Label(self._shown(state.word), theme.TYPE["eyebrow"], 700, state.tone,
                            tracking=theme.TRACKING["eyebrow"], upper=state.key != "dormant")
        if self.wide:
            head.addArrangedSubview_(tag.view)
        widgets.add(body, head)
        if not self.wide:
            widgets.add(body, tag.view)
        remove = self._removal(row)
        repair = (self._chip("Re-pair", lambda t=row.token: self.re_pair(t), f"Re-pair {label}")
                  if state.key == "dormant" else None)
        where = row.platform if row.phone else "  ".join((row.platform, self._shown(row.address)))
        widgets.add(body, widgets.Label(where, theme.TYPE["small"], ink="ink_3", wrap=True).view)
        if row.left_out:
            widgets.add(body, widgets.note("Left out of the layout. The shortcut and the menu still reach it.").view)
        widgets.add(body, widgets.note(self._shown(state.detail)).view)
        entry = next((item for item in self.controller.book.peers() if item.get("token") == row.token), {})
        if not row.phone and getattr(getattr(self.owner, "shortcut_goes_to", None), "value", "last") == "ticked":
            cycle = widgets.Switch("In the shortcut's cycle",
                                   on_change=lambda on, peer=entry.get("id"): self.owner._set_shortcut_cycle(peer, on))
            cycle.value = entry.get("shortcut_cycle") is not False
            cycle.set_enabled(row.in_use)
            cycle.view.focus_id = (row.token, "shortcut_cycle")
            self._cycle_controls[row.token] = cycle
            widgets.add(body, cycle.view)
        edit_box, edit = widgets.field(mono=False)
        edit.setAccessibilityLabel_(f"Name for {label}")
        edit.setStringValue_(entry.get("label", "") or entry.get("name", ""))
        edit_box.setHidden_(True)
        delegate = _RenameDelegate.alloc().init()
        delegate.panel, delegate.token = self, row.token
        edit.setDelegate_(delegate)
        note = widgets.note("Only on this Mac", reading=False)
        note.view.setHidden_(True)
        self._rename_views[row.token] = (name.view, rename, edit_box, edit, note, delegate)
        widgets.add(body, edit_box)
        widgets.add(body, note.view)
        in_use = widgets.Switch("In use", on_change=lambda on, t=row.token: self._direction(t, in_use=on))
        in_use.value = row.in_use
        in_use.view.setToolTip_(f"Off: no input or settings link to {label}. Pairing, directions, ways and jump key are kept. Turn it back on here to reconnect.")
        if row.phone:
            in_use.view.setToolTip_(f"Off: {label} cannot connect or drive this Mac. Turn it back on here to reconnect.")
        in_use.view.focus_id = (row.token, "in_use")
        left_out = self._chip("Put back" if row.left_out else "Leave out",
                              lambda out=not row.left_out, t=row.token: self._left_out(t, out),
                              f"{'Put back' if row.left_out else 'Leave out'} {label}", (row.token, "left_out"))
        drives = widgets.Switch("This Mac drives it", on_change=lambda on, t=row.token: self._direction(t, send=on))
        drives.value = row.drives
        drives.view.setToolTip_(f"Input moves only when this Mac enables this switch and {label} enables It drives this Mac.")
        drives.view.focus_id = (row.token, "drives")
        driven = widgets.Switch("It drives this Mac", on_change=lambda on, t=row.token: self._direction(t, allow_drive=on))
        driven.value = row.driven
        driven.view.setToolTip_(f"Input moves only when {label} enables This machine drives it and this Mac enables this switch.")
        driven.view.focus_id = (row.token, "driven")
        opened = row.token in self.directions_open
        disclosure = self._chip("Hide directions" if opened else "Directions", lambda t=row.token: self._toggle_directions(t),
                                f"{'Hide' if opened else 'Show'} directions for {label}",
                                (row.token, "directions"))
        widgets.add(body, self._controls(row, in_use, left_out, disclosure, drives, driven, remove, repair))
        frame.addSubview_(body)
        widgets.pin(body, frame, (10, 12, 10, 12))
        return frame

    def _begin_rename(self, token):
        views = self._own_rename_views if token is None else self._rename_views.get(token)
        if views is None:
            return
        name, button, box, edit, note, _delegate = views
        if token is None:
            edit.setStringValue_(self.settings_store.current().get("name", ""))
        else:
            entry = next((peer for peer in self.controller.book.peers() if peer.get("token") == token), {})
            edit.setStringValue_(entry.get("label", "") or entry.get("name", ""))
        name.setHidden_(True)
        button.setHidden_(True)
        box.setHidden_(False)
        note.view.setHidden_(self.controller.cfg.same_on_both and "names" in self.controller.cfg.same_groups)
        self.owner.window.makeFirstResponder_(edit)
        edit.selectText_(None)

    def _save_rename(self, token, text):
        self.owner._rename_own(text) if token is None else self.owner._rename_peer(token, text)
        self._cancel_rename(token)

    def _cancel_rename(self, token):
        views = self._own_rename_views if token is None else self._rename_views.get(token)
        if views is None:
            return
        name, button, box, _edit, note, _delegate = views
        name.setHidden_(False)
        button.setHidden_(False)
        box.setHidden_(True)
        note.view.setHidden_(True)

    def _chip(self, text, callback, label, focus_id=None):
        chip = widgets.pressable(callback, "well", "edge", theme.RADIUS["field"])
        chip.focus_id = focus_id
        chip.setAccessibilityLabel_(label)
        word = widgets.Label(text, theme.TYPE["small"], 600, ink="ink")
        chip.addSubview_(word.view)
        widgets.pin(word.view, chip, (6, 12, 6, 12))
        return chip

    def _controls(self, row, in_use, left_out, disclosure, drives, driven, remove, repair=None):
        """The machine switch leads; its two direction switches stay under their disclosure."""
        if self.removing == row.token:
            column = widgets.stack(spacing=6)
            for view in (remove,):
                widgets.add(column, view)
            return column
        line = widgets.stack(vertical=not self.wide, spacing=12)
        if not self.wide:
            line.setAlignment_(AppKit.NSLayoutAttributeTrailing)
        if self.wide:
            line.addArrangedSubview_(widgets.hug(widgets.box(), AppKit.NSLayoutPriorityDefaultLow))
        if repair is not None:
            line.addArrangedSubview_(repair)
        widgets.add(line, in_use.view, full_width=not self.wide)
        if not row.phone:
            line.addArrangedSubview_(left_out)
        if not row.phone:
            line.addArrangedSubview_(disclosure)
        line.addArrangedSubview_(remove)
        column = widgets.stack(spacing=4)
        widgets.add(column, line)
        if not row.phone and row.token in self.directions_open:
            directions = widgets.stack(vertical=not self.wide, spacing=12)
            for view in (drives.view, driven.view):
                widgets.add(directions, view, full_width=not self.wide)
            widgets.add(column, directions)
        return column

    def _toggle_directions(self, token):
        if token in self.directions_open:
            self.directions_open.remove(token)
        else:
            self.directions_open.add(token)
        self.refresh()

    def _removal(self, row):
        label = self._shown(row.label)
        if self.removing != row.token:
            return self._chip("Remove", lambda t=row.token: self._ask_to_remove(t), f"Remove {label}", (row.token, "remove"))
        line = widgets.stack(vertical=False, spacing=8)
        words = widgets.note(f"Remove {label}? It stays paired on that machine until you remove this one there too.", ink="ink", reading=False)
        line.addArrangedSubview_(words.view)
        line.addArrangedSubview_(self._chip("Remove", lambda t=row.token: self._remove(t), f"Confirm removing {label}", (row.token, "confirm")))
        line.addArrangedSubview_(self._chip("Keep", self._keep, f"Keep {label}", (row.token, "keep")))
        return line

    def _ask_to_remove(self, token):
        # Focus goes to Keep: pressing Space twice must not remove a machine.
        self.removing = token
        self._focus_after = (token, "keep")
        self.refresh()

    def _keep(self):
        token, self.removing = self.removing, None
        self._focus_after = (token, "remove")
        self.refresh()

    def _remove(self, token):
        self.removing = None
        before = core_ways.way_back_state(self.settings_store.current())
        try:
            removed = self.settings_store.remove_peer(token)
        except Exception as exc:
            self.logger.warning("could not remove a machine: %s", exc)
            self.owner._say(f"Could not remove it: {exc}", "fault")
            self.refresh()
            return
        if removed is not None:
            self.logger.info("removed %s", peerlist.label_for([removed], token=token) or "a machine")
        self.owner.peers_changed()
        after = core_ways.way_back_state(self.settings_store.current())
        changed_peers = [peer for peer, state in after.items() if before.get(peer) != state]
        for peer in changed_peers:
            self.owner._tell(peer)

    def _direction(self, token, **change):
        handler = self.owner.direction_handler
        if handler is not None:
            handler(token, **change)
        self.refresh()

    def _left_out(self, token, enabled):
        handler = self.owner.left_out_handler
        if handler is not None:
            handler(token, enabled)
        self.refresh()

    # The sheet

    def _toggle_sheet(self):
        self.open = not self.open
        if not self.open:
            self._leave_sheet()
        self.refresh()

    def re_pair(self, token):
        entry = next((item for item in self.controller.book.peers() if item.get("token") == token), None)
        if entry is None:
            return
        self.open = True
        self.chosen = None
        for field in (self.find_field, self.find_secret):
            field.setStringValue_("")
        address = entry.get("host") or ""
        field = self.find_secret if self.controller.cfg.hide_addresses else self.find_field
        field.setStringValue_(address)
        self._heard_key = None
        self.refresh()
        self.code_boxes.focus(self.owner.window)

    def _leave_sheet(self):
        self.code_boxes.clear()
        self.chosen = None
        self._heard_key = None
        self._say("Choose a machine, or type its address, then type the code it shows.")

    def _refresh_sheet(self, hide):
        self._refresh_code(hide)
        self._refresh_heard()
        self.confirm_button.set_enabled(not self.pairing and self._wanted() is not None)

    def invalidate(self):
        """Draw the rows again from the settings on the next refresh."""
        self._rows_key = None

    def stop_showing(self):
        """The window closed: a code nobody can see does not stay up."""
        if self._showing or (self.service is not None and self.service.code is not None):
            self._cancel_code()
        self.open = False

    def _say(self, message, ink="ink_2"):
        self._pair_ink = ink
        self.pair_status.set(self._shown(message), ink=ink)

    # Showing a code

    def _show_code_widgets(self, showing):
        self.show_button.view.setHidden_(showing)
        for view in (self.shown_code_boxes.view, self.code_expiry.view, self.code_address.view, self.qr_view,
                     self.qr_caption.view, self.cancel_button.view):
            view.setHidden_(not showing)

    def _show_code(self):
        service = self.service
        if service is None:
            return
        try:
            service.begin_pairing()
        except pairing.PairingError as exc:
            self.hosted_status.set(peerlist.pairing_error_text(exc, "this machine"), ink="fault")
            self.hosted_status.view.setHidden_(False)
            return
        self._hosted = None
        self._shown_outcome = None
        self._qr_for = None
        self.hosted_status.set("")
        self.refresh()

    def _cancel_code(self):
        if self.service is not None:
            self.service.cancel_pairing()
        self._showing = False
        self._shown_outcome = None
        self.hosted_status.set("")
        self._show_code_widgets(False)

    def _refresh_code(self, hide):
        service = self.service
        code = service.code if service is not None else None
        if code is not None:
            self._showing = True
            self._show_code_widgets(True)
            for control, digit in zip(self.shown_code_boxes.fields, code):
                control.setStringValue_(digit)
            self.code_expiry.set(seconds(service.seconds_left))
            address = pairing.pairing_address()
            qr_text = service.qr_text(address)
            if hide:
                self.code_address.set(self._shown(f"{address} · port {service.tcp_port}") if address else "")
                self._set_qr(None, "The QR is hidden while addresses are hidden.")
            elif qr_text is None:
                self.code_address.set(f"{address} · port {service.tcp_port}" if address else "No network address found")
                self._set_qr(None, f"No QR: {service.tcp_error}" if service.tcp_error else "No QR while this Mac has no address.")
            else:
                self.code_address.set(f"{address} · port {service.tcp_port}")
                self._set_qr(qr_text, "The same code as a QR, for a phone to scan.")
            self.hosted_status.view.setHidden_(not self.hosted_status.text)
            return
        if self._showing:
            self._showing = False
            self._show_code_widgets(False)
            self._shown_outcome = service.outcome if service is not None else None
            known = service.known if service is not None else None
            text = peerlist.host_outcome_text(self._shown_outcome, self._hosted if self._shown_outcome == "paired" else known)
            ink = "signal" if self._shown_outcome == "paired" else "fault"
            self.hosted_status.set(text, ink=ink)
        self.hosted_status.view.setHidden_(not self.hosted_status.text)

    def _set_qr(self, text, caption):
        if not feature_flags.PAIRING_QR_VISIBLE:
            text, caption = None, ""
        self.qr_caption.set(caption)
        self.qr_caption.view.setHidden_(not caption)
        self.qr_view.setHidden_(text is None)
        if text == self._qr_for:
            return
        self._qr_for = text
        self.qr_view.setImage_(qr.image(text, QR_SIDE) if text is not None else None)

    def hosted_pairing(self, entry):
        """A machine paired with the code this Mac showed (on the main thread, after the entry was stored)."""
        self._hosted = entry
        self.open = False
        self.owner.peers_changed()
        self.owner._say(peerlist.host_outcome_text("paired", entry), "signal")

    # Entering a code

    def _address_text(self):
        field = self.find_secret if self.controller.cfg.hide_addresses else self.find_field
        return field.stringValue().strip()

    def _address_typed(self):
        if self._address_text():
            self.chosen = None
            self._heard_key = None
        self._refresh_heard()
        self.code_boxes.focus(self.owner.window)

    def _wanted(self):
        """What a code would be entered for: the address typed, else the machine chosen, else None."""
        address = self._address_text()
        if address:
            return ("address", address)
        if self.chosen is not None:
            return ("machine", self.chosen)
        return None

    def _refresh_heard(self):
        service = self.service
        heard = service.machines() if service is not None else []
        chosen = self.chosen["address"] if self.chosen is not None else None
        hide = self.controller.cfg.hide_addresses
        key = (chosen, hide, tuple(tuple(sorted((k, str(v)) for k, v in item.items())) for item in heard))
        if key == self._heard_key:
            return
        self._heard_key = key
        self._heard = heard
        current = next((item for item in heard if item["address"] == chosen), None)
        if current is not None and current != self.chosen:
            showed = self.chosen["pair_id"]
            self.chosen = current
            fault = self._pair_ink == "fault"
            if current["pair_id"] != showed and not self.pairing and (current["pair_id"] or not fault):
                self._say_choice()
        for view in list(self.heard_list.arrangedSubviews()):
            self.heard_list.removeArrangedSubview_(view)
            view.removeFromSuperview()
        for index, (item, shared) in enumerate(zip(heard, peerlist.sharing_a_name(heard))):
            self._heard_row(index, item, item["address"] == chosen, shared)
        if service is not None and service.error:
            self.heard_empty.set(f"Beamer cannot look for machines: {service.error}", ink="fault")
        else:
            self.heard_empty.set(
                "Looking for machines running Beamer. Open it on the other machine and it appears here; "
                "across a VPN or on guest Wi-Fi it may not, so type its address below.", ink="ink_2")
        self.heard_frame.setHidden_(not heard)
        self.heard_empty.view.setHidden_(bool(heard))
        if chosen is not None and chosen not in {item["address"] for item in heard}:
            self.chosen = None
            self._heard_key = None

    def _heard_row(self, index, item, picked, shared_name):
        older = item.get("pairing") != pairing.PAIRING_V3
        showing = item["pair_id"] is not None and not older
        row = widgets.pressable(lambda index=index: self._choose(index), "well" if picked else "ground",
                                role=AppKit.NSAccessibilityRadioButtonRole)
        row.ring_inset = 1.0
        platform = machines.platform_name(item.get("platform") or "")
        row.setAccessibilityLabel_(self._shown(f"{item['name']}, {item['address']}")
                                   + (", older Beamer" if older else ", showing a code" if showing else ""))
        line = widgets.stack(vertical=False, spacing=9)
        led = widgets.LED()
        led.set("amber" if older else "signal" if showing else "off")
        line.addArrangedSubview_(led.view)
        name_text = self._shown(item["name"])
        name = widgets.Label(name_text, theme.TYPE["note"], 600 if picked else 400)
        name.view.setToolTip_(name_text)
        name.view.setAccessibilityLabel_(name_text)
        line.addArrangedSubview_(widgets.hug(widgets.squeeze(name.view), AppKit.NSLayoutPriorityDefaultLow))
        name.view.widthAnchor().constraintGreaterThanOrEqualToConstant_(100).setActive_(True)
        # A current machine's row always says where it is; an older one's only where its name is shared.
        where = self._shown(item["address"]) if shared_name or not older else ""
        detail = " · ".join(part for part in ("Older Beamer" if older else platform, where) if part)
        detail_label = widgets.Label(detail, theme.TYPE["small"], ink="amber" if older else "ink_3")
        detail_label.view.setToolTip_(detail)
        detail_label.view.setAccessibilityLabel_(detail)
        line.addArrangedSubview_(widgets.squeeze(detail_label.view, AppKit.NSLayoutPriorityDefaultLow))
        row.addSubview_(line)
        widgets.pin(line, row, (7, 10, 7, 10))
        widgets.add(self.heard_list, row)

    def _choose(self, index):
        if not 0 <= index < len(self._heard):
            return
        self.chosen = self._heard[index]
        for box_field in (self.find_field, self.find_secret):
            box_field.setStringValue_("")
        self.code_boxes.clear()
        self.code_boxes.view.setAccessibilityLabel_(self._shown(f"Code shown on {self.chosen['name']}"))
        self._say_choice()
        self._heard_key = None
        self._refresh_heard()
        self.code_boxes.focus(self.owner.window)

    def _say_choice(self):
        chosen = self.chosen
        if chosen.get("pairing") != pairing.PAIRING_V3:
            self._say(peerlist.pairing_error_text(pairing.PairingError(pairing.ERROR_VERSION), chosen["name"]), "fault")
        elif chosen["pair_id"] is None:
            self._say(f"{chosen['name']} is not showing a code yet. Show one in Beamer on it first.")
        else:
            self._say(f"Six digits, as shown on {chosen['name']}.")

    def _pair(self):
        wanted = self._wanted()
        service = self.service
        if wanted is None or self.pairing or service is None:
            return
        code = re.sub(r"\D", "", self.code_boxes.value)
        if len(code) != pairing.CODE_DIGITS:
            self._say(f"The code is {pairing.CODE_DIGITS} digits.", "fault")
            return
        kind, target = wanted
        if kind == "machine":
            # The list may have refreshed since the row was chosen; pair with the latest beacon.
            target = next((item for item in self._heard if item["address"] == target["address"]), target)
            name = target["name"]
            if target.get("pairing") != pairing.PAIRING_V3:
                self.chosen = target
                self._say_choice()
                return
            if target["pair_id"] is None:
                self._say(peerlist.pairing_error_text(pairing.PairingError(pairing.ERROR_NOT_PAIRING), name), "fault")
                return
            run = lambda: service.pair(target, code)
        else:
            name = "That machine"
            run = lambda: service.pair_by_address(target, code)
        self.pairing = True
        self.confirm_button.set_enabled(False)
        self._say(self._shown(f"Pairing with {name}…" if kind == "machine" else f"Pairing with {target}…"))

        def work():
            try:
                result = run()
            except pairing.PairingError as exc:
                result = exc
            except Exception as exc:
                self.logger.exception("pairing failed")
                result = pairing.PairingError(str(exc))
            AppHelper.callAfter(self._finished, name, result)

        threading.Thread(target=work, name="pair", daemon=True).start()

    def _finished(self, name, result):
        self.pairing = False
        if isinstance(result, pairing.PairingError):
            self._say(self._shown(peerlist.pairing_error_text(result, name)), "fault")
            return
        # The name the exchange proved, not the one a beacon claimed.
        proved = peerlist.label_for(self.controller.book.peers(), token=result.get("token")) or name
        self.code_boxes.clear()
        for box_field in (self.find_field, self.find_secret):
            box_field.setStringValue_("")
        self.chosen = None
        self._heard_key = None
        self.open = False
        self._say("Choose a machine, or type its address, then type the code it shows.")
        self.owner.peers_changed()
        self.owner._say(f"Paired with {proved}. Connecting…", "signal")
        self.logger.info("paired with %s", proved)
