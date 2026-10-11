"""The Crossing ledger: kept intent, edited directly in each machine's row."""

from PySide6.QtCore import QEvent, QPointF, QRectF, QSize, Qt
from PySide6.QtGui import QAccessible, QColor, QFontMetrics, QPainter, QPen
from PySide6.QtWidgets import (QAccessibleWidget, QCheckBox, QFrame, QGraphicsOpacityEffect,
                               QGridLayout, QHBoxLayout, QLabel, QLineEdit, QPushButton,
                               QSizePolicy, QStyle, QStyleOptionButton, QVBoxLayout, QWidget)

from core import ways
import theme
import widgets


def _accessible_factory(_name, obj):
    role = obj.property("ledger_role") if isinstance(obj, QWidget) else None
    roles = {"table": QAccessible.Role.Table, "row": QAccessible.Role.Row,
             "columnheader": QAccessible.Role.ColumnHeader, "rowheader": QAccessible.Role.RowHeader,
             "cell": QAccessible.Role.Cell}
    if role in roles:
        return QAccessibleWidget(obj, roles[role])


QAccessible.installFactory(_accessible_factory)


def _quiet(text, callback=None):
    button = QPushButton(text)
    button.setProperty("vernier", "plain")
    button.setProperty("ledger_quiet", True)
    button.setFont(theme.font(theme.TYPE["note"]))
    button.setCursor(Qt.CursorShape.PointingHandCursor)
    button.setMinimumWidth(0)
    button.setFixedHeight(24)
    _quiet_style(button)
    if callback:
        button.clicked.connect(callback)
    return button


def _quiet_style(button):
    button.setStyleSheet(f"QPushButton {{ color: {theme.colour('ink_2')}; background: transparent; border: 0; padding: 0; font-size: {round(theme.TYPE['note'])}px; font-weight: 400; text-align: left; }} QPushButton:focus {{ border: 1px solid {theme.colour('signal')}; }}")


class ScreenGlyph(QWidget):
    def __init__(self, kind, callback, parent=None):
        super().__init__(parent)
        self.kind, self.callback = kind, callback
        self.side, self.picked, self.corner, self.holders = "", [], "", {}
        self.setFixedSize(50, 38)
        self.buttons = {}
        details = ways.PARTS if kind == "edge" else ways.CORNERS
        for detail in details:
            button = QPushButton(self)
            button.setCheckable(True)
            button.setCursor(Qt.CursorShape.PointingHandCursor)
            button.setStyleSheet("QPushButton { background: transparent; border: 0; padding: 0; } QPushButton:focus { border: 1px solid " + theme.colour("signal") + "; }")
            button.clicked.connect(lambda _checked=False, value=detail: self.callback(value))
            self.buttons[detail] = button

    def reflect(self, side="", picked=(), corner="", holders=None, left_out=False):
        self.side, self.picked, self.corner, self.holders = side or "", list(picked), corner, holders or {}
        rect = QRectF(7.5, 7.5, 35, 23)
        for index, (detail, button) in enumerate(self.buttons.items()):
            button.setStyleSheet("QPushButton { background: transparent; border: 0; padding: 0; } QPushButton:focus { border: 1px solid " + theme.colour("signal") + "; }")
            if self.kind == "edge":
                horizontal = side in ("top", "bottom")
                span = rect.width() if horizontal else rect.height()
                fixed = rect.top() if side == "top" else rect.bottom() if side == "bottom" else rect.left() if side == "left" else rect.right()
                start = (rect.left() if horizontal else rect.top()) + index * span / 3
                hit = QRectF(start, fixed - 6, span / 3, 12) if horizontal else QRectF(fixed - 6, start, 12, span / 3)
                title = ways.part_names(side or "top")[detail]
                on = detail in picked
                name = f"{(side or 'Unplaced').capitalize()} edge, {title.lower()} third, {'on' if on else 'off'}"
                tooltip = f"{title} third: click to {'leave it out' if on else 'turn it on'}"
                allowed = bool(side)
            else:
                x = 10.5 if detail.endswith("left") else 39.5
                y = 10.5 if detail.startswith("top") else 27.5
                hit = QRectF(x - 7, y - 7, 14, 14)
                title = detail.replace("_", " ").capitalize()
                on = corner == detail
                name = f"{title} corner, {'on' if on else 'off'}"
                tooltip = (f"{self.holders[detail]}'s corner" if detail in self.holders else
                           f"{title} corner: click to {'turn it off' if on else 'move the corner here' if corner else 'turn it on'}")
                allowed = detail not in self.holders
            button.setGeometry(hit.toAlignedRect())
            button.setAccessibleName(name)
            button.setToolTip("Left out: Put back to change its ways" if left_out else tooltip)
            button.setEnabled(allowed and not left_out)
            button.blockSignals(True)
            button.setChecked(on)
            button.blockSignals(False)
        self.update()

    def paintEvent(self, _event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setPen(QPen(QColor(theme.colour("edge")), 1))
        painter.setBrush(QColor(theme.colour("well")))
        rect = QRectF(7.5, 7.5, 35, 23)
        painter.drawRoundedRect(rect, 2.5, 2.5)
        if self.kind == "edge" and self.side:
            horizontal = self.side in ("top", "bottom")
            fixed = rect.top() if self.side == "top" else rect.bottom() if self.side == "bottom" else rect.left() if self.side == "left" else rect.right()
            span = rect.width() if horizontal else rect.height()
            origin = rect.left() if horizontal else rect.top()
            for index, third in enumerate(ways.PARTS):
                on = third in self.picked
                colour = QColor(theme.colour("signal" if on else "edge"))
                colour.setAlphaF(1 if on else .7)
                pen = QPen(colour, 3 if on else 1.5)
                pen.setCapStyle(Qt.PenCapStyle.RoundCap)
                painter.setPen(pen)
                start, end = origin + index * span / 3 + 1, origin + (index + 1) * span / 3 - 1
                painter.drawLine(QPointF(start, fixed), QPointF(end, fixed)) if horizontal else painter.drawLine(QPointF(fixed, start), QPointF(fixed, end))
        elif self.kind == "corner":
            for corner in ways.CORNERS:
                x = 10.5 if corner.endswith("left") else 39.5
                y = 10.5 if corner.startswith("top") else 27.5
                on, held = corner == self.corner, corner in self.holders
                colour = QColor(theme.colour("signal" if on or held else "edge"))
                colour.setAlphaF(.45 if held else 1)
                painter.setPen(QPen(colour, 1))
                painter.setBrush(colour if on else Qt.BrushStyle.NoBrush)
                radius = 3.2 if on else 2
                painter.drawEllipse(QPointF(x, y), radius, radius)
        painter.end()


class _Name(QLabel):
    def __init__(self):
        super().__init__()
        self.full_text = ""
        self.setMinimumWidth(0)
        self.setFont(theme.font(theme.TYPE["body"], 600))

    def set_name(self, text):
        self.full_text = text
        self.setAccessibleName(text)
        self.setToolTip(text)
        self._fit()

    def sizeHint(self):
        return QSize(132, self.fontMetrics().height())

    def minimumSizeHint(self):
        return QSize(0, self.fontMetrics().height())

    def _fit(self):
        self.setText(QFontMetrics(self.font()).elidedText(self.full_text, Qt.TextElideMode.ElideRight, max(1, self.width())))

    def resizeEvent(self, event):
        self._fit()
        super().resizeEvent(event)


class _Jump(widgets.JumpKeyRecorder):
    def __init__(self, callback, on_arm):
        super().__init__(callback, on_arm)
        self.setMinimumHeight(24)
        self.layout().setContentsMargins(5, 2, 5, 2)
        self.key.setFont(theme.mono_font(theme.TYPE["small"]))
        self.hint.hide()

    def sizeHint(self):
        return QSize(self.key.fontMetrics().horizontalAdvance(self.key.text()) + 10,
                     24 if not self.armed else 56)

    def heightForWidth(self, width):
        return 24 if not self.armed else 56

    def set_value(self, value):
        super().set_value(value)
        if not value and not self.armed:
            self.key.setText("Set a key")
        self._reflect_style()

    def _reflect_style(self):
        chip = bool(self.value) or self.armed
        widgets.set_role(self, "keycap-live" if self.armed else "keycap" if chip else "plain")
        self.key.setFont(theme.mono_font(theme.TYPE["small"]) if chip else theme.font(theme.TYPE["note"]))
        if chip:
            self.setStyleSheet("")
        else:
            _quiet_style(self)

    def _toggle(self):
        super()._toggle()
        self._reflect_style()
        self.hint.setVisible(self.armed)
        self.updateGeometry()

    def cancel(self):
        super().cancel()
        self._reflect_style()
        self.hint.hide()
        self.updateGeometry()

    def eventFilter(self, watched, event):
        if self.armed and event.type() == QEvent.Type.KeyPress and event.key() == Qt.Key.Key_Escape:
            self.cancel()
            return True
        return super().eventFilter(watched, event)


class _Cycle(QCheckBox):
    def paintEvent(self, event):
        super().paintEvent(event)
        if not self.isChecked():
            return
        option = QStyleOptionButton()
        self.initStyleOption(option)
        rect = self.style().subElementRect(QStyle.SubElement.SE_CheckBoxIndicator, option, self)
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        pen = QPen(QColor(theme.colour("ground")), 1.8)
        pen.setCapStyle(Qt.PenCapStyle.RoundCap)
        painter.setPen(pen)
        points = [QPointF(rect.x() + rect.width() * x, rect.y() + rect.height() * y)
                  for x, y in ((.25, .52), (.43, .7), (.78, .28))]
        painter.drawLine(points[0], points[1])
        painter.drawLine(points[1], points[2])
        painter.end()


class LedgerRow(QFrame):
    def minimumSizeHint(self):
        return QSize(0, self.grid.minimumSize().height())

    def __init__(self, peer, owner):
        super().__init__()
        self.peer, self.owner, self.selected, self.narrow = peer, owner, False, None
        self.setProperty("ledger_role", "row")
        self.setProperty("vernier", "plain")
        self.setMinimumWidth(0)
        self.grid = QGridLayout(self)
        self.grid.setContentsMargins(12, 12, 0, 12)
        self.grid.setHorizontalSpacing(16)
        self.grid.setVerticalSpacing(8)
        self.cells = []
        for _ in range(4):
            cell = QWidget(self)
            cell.setProperty("vernier", "plain")
            cell.setProperty("ledger_role", "cell")
            cell.setMinimumWidth(0)
            self.cells.append(cell)
        self.machine, self.edge, self.corner_cell, self.keys = self.cells
        self.machine.setProperty("ledger_role", "rowheader")
        machine = QVBoxLayout(self.machine)
        machine.setContentsMargins(0, 0, 0, 0)
        machine.setSpacing(2)
        machine.setSizeConstraint(machine.SizeConstraint.SetMinimumSize)
        self.name = _Name()
        self.name.installEventFilter(self)
        self.machine.installEventFilter(self)
        self.status = widgets.label("", "small", wrap=True)
        self.status.installEventFilter(self)
        status_line = QHBoxLayout()
        status_line.setSpacing(6)
        self.dot = QLabel()
        self.dot.setFixedSize(6, 6)
        status_line.addWidget(self.dot, 0, Qt.AlignmentFlag.AlignVCenter)
        status_line.addWidget(self.status, 1)
        self.edit = QLineEdit()
        self.edit.setMinimumWidth(0)
        self.edit.hide()
        self.edit.returnPressed.connect(self.save_name)
        self.edit.installEventFilter(self)
        self.rename_hint = widgets.label("Return saves, Escape cancels · Only on this PC", "small", wrap=True)
        self.rename_hint.hide()
        machine.addWidget(self.name)
        machine.addWidget(self.edit)
        machine.addWidget(self.rename_hint)
        machine.addLayout(status_line)
        actions = QHBoxLayout()
        actions.setSpacing(10)
        self.rename = _quiet("Rename", self.begin_rename)
        self.leave = _quiet("Leave out", lambda: owner._canvas_left_out(peer, not self.left_out))
        actions.addWidget(self.rename)
        actions.addWidget(self.leave)
        actions.addStretch()
        machine.addLayout(actions)
        self.edge_glyph = ScreenGlyph("edge", lambda third: owner._ledger_third(peer, third))
        self.corner_glyph = ScreenGlyph("corner", lambda corner: owner._ledger_corner(peer, corner))
        self.edge_text = _quiet("", lambda: owner._ledger_edge(peer))
        self.edge_text.setMaximumHeight(16777215)
        self.edge_text.setMinimumHeight(40)
        text = QVBoxLayout(self.edge_text)
        text.setContentsMargins(0, 0, 0, 0)
        text.setSpacing(1)
        text.setSizeConstraint(text.SizeConstraint.SetMinimumSize)
        self.edge_value = widgets.label("", "body", wrap=True)
        self.edge_detail = widgets.label("", "small", wrap=True)
        for label in (self.edge_value, self.edge_detail):
            label.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents)
            text.addWidget(label)
        for cell, glyph, content in ((self.edge, self.edge_glyph, self.edge_text), (self.corner_cell, self.corner_glyph, None)):
            box = QHBoxLayout(cell)
            box.setContentsMargins(0, 0, 0, 0)
            box.setSpacing(4)
            box.addWidget(glyph, 0, Qt.AlignmentFlag.AlignTop)
            if content is None:
                content = QWidget()
                content.setProperty("vernier", "plain")
                texts = QVBoxLayout(content)
                texts.setContentsMargins(0, 0, 0, 0)
                texts.setSpacing(1)
                self.corner_value = widgets.label("", "body", wrap=True)
                self.corner_detail = widgets.label("", "small", wrap=True)
                texts.addWidget(self.corner_value)
                texts.addWidget(self.corner_detail)
            box.addWidget(content, 1, Qt.AlignmentFlag.AlignTop)
        keys = QVBoxLayout(self.keys)
        keys.setContentsMargins(0, 0, 0, 0)
        keys.setSpacing(4)
        self.cycle = _Cycle()
        self.cycle.setFont(theme.font(theme.TYPE["body"]))
        self.cycle.setToolTip("In the shortcut's cycle")
        self.cycle.toggled.connect(lambda on: owner._set_shortcut_cycle(peer, on))
        keys.addWidget(self.cycle)
        jump = QHBoxLayout()
        jump.setSpacing(5)
        jump.addWidget(widgets.label("Jump", "small"))
        self.jump = _Jump(lambda value: owner._ledger_jump(peer, value), owner._jump_arming)
        jump.addWidget(self.jump, 0, Qt.AlignmentFlag.AlignLeft)
        self.clear = _quiet("Clear", lambda: owner._ledger_jump(peer, ""))
        self.clear.setAccessibleName("Clear jump key")
        jump.addWidget(self.clear)
        jump.addStretch()
        keys.addLayout(jump)
        keys.addStretch()
        self.effect = QGraphicsOpacityEffect(self)
        self.setGraphicsEffect(self.effect)
        self.reflow(False)

    def reflow(self, narrow):
        if self.narrow == narrow:
            return
        self.narrow = narrow
        for cell in self.cells:
            self.grid.removeWidget(cell)
        for col in range(4):
            self.grid.setColumnMinimumWidth(col, 0)
            self.grid.setColumnStretch(col, 0)
        if narrow:
            self.grid.addWidget(self.machine, 0, 0, 1, 2)
            self.grid.addWidget(self.edge, 1, 0)
            self.grid.addWidget(self.corner_cell, 1, 1)
            self.grid.addWidget(self.keys, 2, 0, 1, 2)
            self.grid.setColumnStretch(0, 1)
            self.grid.setColumnStretch(1, 1)
        else:
            for index, (cell, minimum, stretch) in enumerate(zip(self.cells, (132, 124, 104, 128), (13, 10, 9, 10))):
                self.grid.addWidget(cell, 0, index)
                self.grid.setColumnMinimumWidth(index, minimum)
                self.grid.setColumnStretch(index, stretch)

    def reflect(self, settings, entry, name, side, linked, selected, trigger, labels, legacy=""):
        self._reflection = (settings, entry, name, side, linked, selected, trigger, labels, legacy)
        for button in (self.rename, self.leave, self.edge_text, self.clear):
            _quiet_style(button)
        self.entry, self.left_out, self.selected = entry, bool(entry.get("left_out")), selected
        self.setAccessibleName(name)
        self.machine.setAccessibleName(name)
        self.name.set_name(name)
        self.name.setStyleSheet(f"color: {theme.colour('ink' if linked and not self.left_out else 'ink_2')}")
        platform = {"windows": "Windows", "macos": "macOS", "linux": "Linux"}.get(entry.get("platform"), "Computer")
        status = "Left out" if self.left_out else "Linked" if linked else "Offline, ways kept"
        self.status.setText(legacy or f"{platform} · {status}")
        self.status.setStyleSheet(f"color: {theme.colour('amber' if legacy else 'ink_3')}")
        self.dot.setStyleSheet(f"background: {theme.colour('amber' if legacy else 'signal' if linked and not self.left_out else 'ink_3')}; border-radius: 3px;")
        self.status.setAccessibleDescription("Linked" if linked else status)
        self.leave.setText("Put back" if self.left_out else "Leave out")
        self.leave.setToolTip("Left out machines stay paired and linked. The shortcut, menu and clipboard still reach them." if self.left_out else "Leave this machine out of crossings")
        held = ways.ways(settings, self.peer)
        picked = ways.picked_thirds(held["methods"], held["parts"])
        on = bool(picked) and bool(side)
        self.edge_glyph.reflect(side, picked if on else (), left_out=self.left_out)
        self.edge_value.setText(ways.edge_form_detail(side, held["methods"], held["parts"], here="This PC") if on else "Off")
        self.edge_value.setStyleSheet(f"color: {theme.colour('ink' if on else 'ink_3')}")
        self.edge_detail.setText(f"{side} edge{' off' if not on else ''}" if side else "place it on the map")
        self.edge_text.setEnabled(bool(side) and not self.left_out)
        self.edge_text.setToolTip("Left out: Put back to change its ways" if self.left_out else "Edge: click to turn it off" if on else f"Edge: click for the whole {side} edge")
        self.edge_text.setAccessibleName(f"Edge, {'whole ' + side + ' edge' if len(picked) == 3 and side else self.edge_value.text().lower()}")
        corner = held["corner"] if "corner" in held["methods"] else ""
        holders = {c: labels.get(holder, "Another machine") for c in ways.CORNERS
                   if (holder := ways.corner_holder(settings, c, exclude=self.peer))}
        self.corner_glyph.reflect(corner=corner, holders=holders, left_out=self.left_out)
        self.corner_value.setText(corner.replace("_", " ").capitalize() if corner else "None")
        self.corner_value.setStyleSheet(f"color: {theme.colour('ink' if corner else 'ink_3')}")
        self.corner_detail.setText("diagonal push" if corner else "click a dot")
        self.cycle.blockSignals(True)
        self.cycle.setText(trigger)
        self.cycle.setChecked(entry.get("shortcut_cycle", True))
        self.cycle.blockSignals(False)
        self.jump.set_value(entry.get("jump_key", ""))
        self.jump.setToolTip(f"Press {self.jump.key.text()} to send input to {name}; press again to bring it home." if self.jump.value else f"Choose a key combination to jump to {name} and back.")
        self.clear.setVisible(bool(self.jump.value))
        self.effect.setOpacity(.5 if self.left_out or not linked else 1)
        self.update()

    def refresh_colours(self):
        if hasattr(self, "_reflection"):
            self.reflect(*self._reflection)

    def begin_rename(self):
        self.edit.setText(self.entry.get("label") or self.entry.get("name") or "")
        self.name.hide()
        self.edit.show()
        self.rename_hint.show()
        self.edit.setFocus()
        self.edit.selectAll()

    def save_name(self):
        self.owner._rename_canvas_peer(self.peer, self.edit.text().strip()[:64])
        self.cancel_name()

    def cancel_name(self):
        self.edit.hide()
        self.rename_hint.hide()
        self.name.show()

    def eventFilter(self, watched, event):
        if watched is self.edit and event.type() == QEvent.Type.KeyPress and event.key() == Qt.Key.Key_Escape:
            self.cancel_name()
            return True
        if watched in (self.machine, self.name, self.status) and event.type() == QEvent.Type.MouseButtonPress:
            self.owner._choose_machine(self.peer)
            return True
        return super().eventFilter(watched, event)

    def paintEvent(self, event):
        super().paintEvent(event)
        painter = QPainter(self)
        painter.setPen(QPen(QColor(theme.colour("rule")), 1))
        painter.drawLine(0, self.height() - 1, self.width(), self.height() - 1)
        if self.selected:
            painter.fillRect(0, 12, 3, max(1, self.height() - 24), QColor(theme.colour("signal")))
        painter.end()


class CrossingLedger(QWidget):
    def minimumSizeHint(self):
        return QSize(0, self.column.minimumSize().height())

    def __init__(self, owner):
        super().__init__()
        self.owner, self.rows, self.narrow = owner, {}, False
        self.setProperty("ledger_role", "table")
        self.setProperty("vernier", "plain")
        self.setAccessibleName("How each machine is reached")
        self.setMinimumWidth(0)
        self.column = QVBoxLayout(self)
        self.column.setContentsMargins(0, 0, 0, 0)
        self.column.setSpacing(0)
        self.header = QWidget()
        self.header.setProperty("vernier", "plain")
        head = QGridLayout(self.header)
        head.setContentsMargins(12, 0, 0, 8)
        head.setHorizontalSpacing(16)
        for index, (title, minimum, stretch) in enumerate(zip(("Machine", "Edge", "Corner", "Keys"), (132, 124, 104, 128), (13, 10, 9, 10))):
            label = widgets.Eyebrow(title)
            label.setProperty("ledger_role", "columnheader")
            label.setAccessibleName(title)
            head.addWidget(label, 0, index)
            head.setColumnMinimumWidth(index, minimum)
            head.setColumnStretch(index, stretch)
        self.header.setStyleSheet(f"border-bottom: 1px solid {theme.colour('rule')};")
        self.column.addWidget(self.header)

    def cancel_recording(self):
        for row in self.rows.values():
            row.jump.cancel()

    def refresh_colours(self):
        self.header.setStyleSheet(f"border-bottom: 1px solid {theme.colour('rule')};")

    def reflect(self, settings, layout, labels, chosen, live, trigger, legacy=None):
        self.header.setStyleSheet(f"border-bottom: 1px solid {theme.colour('rule')};")
        entries = {p["id"]: p for p in settings.get("peers", [])}
        order = sorted(entries, key=lambda p: (0, layout[p]["x"], layout[p]["y"]) if p in layout else (1, 0, 0))
        for peer in self.rows.keys() - entries.keys():
            row = self.rows.pop(peer)
            row.jump.cancel()
            self.column.removeWidget(row)
            row.deleteLater()
        for index, peer in enumerate(order):
            row = self.rows.get(peer)
            if row is None:
                row = LedgerRow(peer, self.owner)
                self.rows[peer] = row
                row.reflow(self.narrow)
            if self.column.indexOf(row) != index + 1:
                self.column.insertWidget(index + 1, row)
            own = layout.get(settings.get("machine_id"))
            tile = layout.get(peer)
            contact = ways._rect_contact(own, tile) if own and tile else None
            side = contact[0] if contact and contact[0] != "corner" else ""
            row.reflect(settings, entries[peer], labels.get(peer, "Other machine"), side, live(peer), chosen == peer, trigger, labels, (legacy or {}).get(peer, ""))
        self.setVisible(bool(entries))

    def resizeEvent(self, event):
        super().resizeEvent(event)
        narrow = self.width() < 620
        if narrow != self.narrow:
            self.narrow = narrow
            self.header.setVisible(not narrow)
            for row in self.rows.values():
                row.reflow(narrow)
