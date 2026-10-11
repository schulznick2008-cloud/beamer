// Pruefling fuer die Eingabe-Abnahme (Paket C04): ein Fenster, das jede Mausbewegung, jeden Klick (mit Klickzahl),
// jedes Scrollen, jede Taste und jeden eingefuegten Text als JSON-Zeile in eine Datei schreibt, dazu ein
// Textfeld, ein Dropdown (NSPopUpButton) und ein Kontextmenue.
//   swiftc -O tools/teleport/input_probe.swift -o /tmp/teleport_probe && /tmp/teleport_probe /tmp/teleport_probe.log &
// Danach: tools/teleport/mac_inject_demo.py
import AppKit

let logPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/teleport_probe.log"
FileManager.default.createFile(atPath: logPath, contents: nil)
let logHandle = FileHandle(forWritingAtPath: logPath)!

func log(_ obj: [String: Any]) {
    var o = obj
    o["t"] = Date().timeIntervalSince1970
    if let w = NSApp.windows.first(where: { $0.title == "TeleportInputProbe" }), let screen = w.screen ?? NSScreen.main {
        // Fensterrahmen in globalen Koordinaten mit Ursprung oben links (wie CGEvent).
        let primaryHeight = NSScreen.screens.first?.frame.height ?? screen.frame.height
        o["win"] = [w.frame.minX, primaryHeight - w.frame.maxY, w.frame.width, w.frame.height]
    }
    if let d = try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]) {
        logHandle.write(d)
        logHandle.write(Data("\n".utf8))
    }
}

func globalTopLeft(_ e: NSEvent) -> [Double] {
    let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
    let p = NSEvent.mouseLocation
    return [Double(p.x), Double(primaryHeight - p.y)]
}

final class ProbeView: NSView {
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .activeAlways, .inVisibleRect], owner: self))
    }
    private func mouse(_ kind: String, _ e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        log(["ev": kind, "view": [Double(p.x), Double(p.y)], "screen": globalTopLeft(e),
             "clicks": e.clickCount, "button": e.buttonNumber])
    }
    override func mouseDown(with e: NSEvent) { mouse("down", e) }
    override func mouseUp(with e: NSEvent) { mouse("up", e) }
    override func rightMouseDown(with e: NSEvent) { mouse("down", e) }
    override func rightMouseUp(with e: NSEvent) { mouse("up", e) }
    override func otherMouseDown(with e: NSEvent) { mouse("down", e) }
    override func otherMouseUp(with e: NSEvent) { mouse("up", e) }
    override func mouseMoved(with e: NSEvent) { mouse("move", e) }
    override func mouseDragged(with e: NSEvent) { mouse("drag", e) }
    override func scrollWheel(with e: NSEvent) {
        log(["ev": "scroll", "dx": Double(e.scrollingDeltaX), "dy": Double(e.scrollingDeltaY),
             "precise": e.hasPreciseScrollingDeltas, "screen": globalTopLeft(e)])
    }
    override func keyDown(with e: NSEvent) {
        log(["ev": "key", "down": true, "code": Int(e.keyCode), "chars": e.charactersIgnoringModifiers ?? "",
             "flags": Int(e.modifierFlags.rawValue & 0xFFFF0000)])
    }
    override func keyUp(with e: NSEvent) { log(["ev": "key", "down": false, "code": Int(e.keyCode)]) }
    override func menu(for event: NSEvent) -> NSMenu? {
        let m = NSMenu()
        for t in ["Kontext A", "Kontext B", "Kontext C"] { m.addItem(NSMenuItem(title: t, action: #selector(menuPicked(_:)), keyEquivalent: "")) }
        return m
    }
    @objc func menuPicked(_ s: NSMenuItem) { log(["ev": "menu", "title": s.title]) }
    override func draw(_ r: NSRect) {
        NSColor(calibratedWhite: 0.93, alpha: 1).setFill(); r.fill()
        for i in stride(from: 0, to: Int(bounds.width), by: 100) {
            NSColor.systemBlue.setFill(); NSRect(x: i, y: 0, width: 1, height: Int(bounds.height)).fill()
        }
    }
}

final class TextDelegate: NSObject, NSTextFieldDelegate {
    func controlTextDidChange(_ n: Notification) {
        if let f = n.object as? NSTextField { log(["ev": "text", "value": f.stringValue]) }
    }
}

final class Popup: NSObject {
    @objc func changed(_ s: NSPopUpButton) { log(["ev": "dropdown", "title": s.titleOfSelectedItem ?? ""]) }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let win = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 1512, height: 950),
                   styleMask: [.titled, .resizable, .closable, .miniaturizable], backing: .buffered, defer: false)
win.title = "TeleportInputProbe"
win.acceptsMouseMovedEvents = true
let view = ProbeView(frame: win.contentView!.bounds)
view.autoresizingMask = [.width, .height]
win.contentView!.addSubview(view)
let field = NSTextField(frame: NSRect(x: 20, y: 20, width: 400, height: 28))
let delegate = TextDelegate()
field.delegate = delegate
field.placeholderString = "Text"
view.addSubview(field)
let popup = NSPopUpButton(frame: NSRect(x: 20, y: 70, width: 240, height: 28), pullsDown: false)
popup.addItems(withTitles: ["Eins", "Zwei", "Drei", "Vier"])
let popupTarget = Popup()
popup.target = popupTarget
popup.action = #selector(Popup.changed(_:))
view.addSubview(popup)
win.makeKeyAndOrderFront(nil)
win.makeFirstResponder(field)
app.activate(ignoringOtherApps: true)
log(["ev": "ready", "field": [20.0, 20.0, 400.0, 28.0], "popup": [20.0, 70.0, 240.0, 28.0]])
app.run()
