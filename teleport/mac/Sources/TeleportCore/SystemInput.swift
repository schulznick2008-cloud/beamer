import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

/// Markierung in `kCGEventSourceUserData` jedes eingespielten Ereignisses (wie Beamers Python-Injektor).
public let injectedMark: Int64 = 0xBEA3

enum AccessibilityPermission {
    /// Ohne Bedienungshilfen-Freigabe ignoriert macOS gepostete Ereignisse stillschweigend und AX-Aufrufe scheitern.
    static func check() throws {
        let fake = ProcessInfo.processInfo.environment["BEAMER_TELEPORT_FAKE_NO_ACCESSIBILITY"] == "1"
        if fake || !AXIsProcessTrusted() {
            throw IPCError(.permission,
                "Bedienungshilfen nicht freigegeben. Systemeinstellungen > Datenschutz & Sicherheit > Bedienungshilfen: dem Programm, das Beamer startet (Terminal/Beamer), die Freigabe geben und neu starten.")
        }
    }

    static var trusted: Bool {
        ProcessInfo.processInfo.environment["BEAMER_TELEPORT_FAKE_NO_ACCESSIBILITY"] != "1" && AXIsProcessTrusted()
    }
}

/// Postet echte CGEvents in den HID-Strom.
public final class CGEventSink: EventSink {
    public init() {}

    private func post(_ e: CGEvent?) {
        guard let e = e else { return }
        e.setIntegerValueField(.eventSourceUserData, value: injectedMark)
        e.post(tap: .cghidEventTap)
    }

    public func mouse(_ type: MouseEventKind, at p: CGPoint, button: MouseButton?, clickState: Int, flags: UInt64) {
        let cgType: CGEventType
        let cgButton: CGMouseButton
        var otherNumber: Int64?
        switch (type, button) {
        case (.moved, _): cgType = .mouseMoved; cgButton = .left
        case (.dragged, .some(.left)): cgType = .leftMouseDragged; cgButton = .left
        case (.dragged, .some(.right)): cgType = .rightMouseDragged; cgButton = .right
        case (.dragged, .some(let b)): cgType = .otherMouseDragged; cgButton = .center; otherNumber = b == .back ? 3 : (b == .forward ? 4 : 2)
        case (.dragged, .none): cgType = .mouseMoved; cgButton = .left
        case (.down, .some(.left)): cgType = .leftMouseDown; cgButton = .left
        case (.up, .some(.left)): cgType = .leftMouseUp; cgButton = .left
        case (.down, .some(.right)): cgType = .rightMouseDown; cgButton = .right
        case (.up, .some(.right)): cgType = .rightMouseUp; cgButton = .right
        case (.down, .some(let b)): cgType = .otherMouseDown; cgButton = .center; otherNumber = b == .back ? 3 : (b == .forward ? 4 : 2)
        case (.up, .some(let b)): cgType = .otherMouseUp; cgButton = .center; otherNumber = b == .back ? 3 : (b == .forward ? 4 : 2)
        default: return
        }
        guard let e = CGEvent(mouseEventSource: nil, mouseType: cgType, mouseCursorPosition: p, mouseButton: cgButton) else { return }
        e.flags = CGEventFlags(rawValue: flags)
        if clickState > 0 { e.setIntegerValueField(.mouseEventClickState, value: Int64(clickState)) }
        if let n = otherNumber { e.setIntegerValueField(.mouseEventButtonNumber, value: n) }
        post(e)
    }

    public func scroll(at p: CGPoint, wheelY: Int, wheelX: Int, unit: ScrollUnit, flags: UInt64) {
        guard let e = CGEvent(scrollWheelEvent2Source: nil, units: unit == .line ? .line : .pixel, wheelCount: 2,
                              wheel1: Int32(wheelY), wheel2: Int32(wheelX), wheel3: 0) else { return }
        e.flags = CGEventFlags(rawValue: flags)
        post(e)
    }

    public func key(code: UInt16, down: Bool, flags: UInt64) {
        guard let e = CGEvent(keyboardEventSource: nil, virtualKey: CGKeyCode(code), keyDown: down) else { return }
        e.flags = CGEventFlags(rawValue: flags)
        post(e)
    }

    public func text(_ chunk: String, down: Bool) {
        guard let e = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down) else { return }
        var units = Array(chunk.utf16)
        e.keyboardSetUnicodeString(stringLength: units.count, unicodeString: &units)
        e.flags = []
        post(e)
    }
}

/// Zeichen je Taste der aktuellen Tastaturbelegung, ohne Text-Input-Sources-Aufrufe (die muessten auf dem
/// Hauptthread laufen): ein Tastenereignis wird gebaut und sein Unicode-Text gelesen.
public final class SystemKeyLayout: KeyLayout {
    private let lock = NSLock()
    private var chars: [UInt16: String] = [:]
    private var codes: [String: UInt16] = [:]
    private var builtAt = Date.distantPast

    public init() {}

    private func refreshIfNeeded() {
        lock.lock(); defer { lock.unlock() }
        guard Date().timeIntervalSince(builtAt) > 10 else { return }
        builtAt = Date()
        var newChars: [UInt16: String] = [:]
        var newCodes: [String: UInt16] = [:]
        let keys = Set(KeyTables.printableFallbacks.keys).union([0x0A])
        for code in keys.sorted() {
            guard let e = CGEvent(keyboardEventSource: nil, virtualKey: CGKeyCode(code), keyDown: true) else { continue }
            e.flags = []
            var buf = [UniChar](repeating: 0, count: 8)
            var len = 0
            e.keyboardGetUnicodeString(maxStringLength: 8, actualStringLength: &len, unicodeString: &buf)
            let s = String(utf16CodeUnits: buf, count: len)
            if len == 1, let sc = s.unicodeScalars.first, !sc.properties.isWhitespace, sc.value >= 0x20 {
                newChars[code] = s
                if newCodes[s] == nil { newCodes[s] = code }
            }
        }
        // Tasten, die die Belegung nicht liest, behalten die US-Antwort (wie keyboard_layout.install).
        for (code, ch) in KeyTables.printableFallbacks where newChars[code] == nil {
            newChars[code] = ch
            if newCodes[ch] == nil { newCodes[ch] = code }
        }
        chars = newChars
        codes = newCodes
    }

    public func code(for character: String) -> UInt16? {
        refreshIfNeeded()
        lock.lock(); defer { lock.unlock() }
        return codes[character]
    }

    public func character(for code: UInt16) -> String? {
        refreshIfNeeded()
        lock.lock(); defer { lock.unlock() }
        return chars[code]
    }
}

/// Ein Fenster per Bedienungshilfen (AX) steuern: nach vorne holen, Groesse und Position lesen/setzen.
public protocol WindowController: AnyObject {
    func activate(windowID: UInt32, pid: pid_t) throws
    func frame(windowID: UInt32, pid: pid_t) throws -> CGRect
    func setFrame(windowID: UInt32, pid: pid_t, origin: CGPoint?, size: CGSize?) throws
    func isFrontmost(windowID: UInt32, pid: pid_t) -> Bool
}

public final class AXWindowController: WindowController {
    public init() {}

    @_silgen_name("_AXUIElementGetWindow")
    private static func _axGetWindow(_ element: AXUIElement, _ out: UnsafeMutablePointer<CGWindowID>) -> AXError

    private func axWindow(windowID: UInt32, pid: pid_t) throws -> AXUIElement {
        try AccessibilityPermission.check()
        let app = AXUIElementCreateApplication(pid)
        var value: CFTypeRef?
        let err = AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value)
        guard err == .success, let windows = value as? [AXUIElement] else {
            throw IPCError(.notFound, "Fenster \(windowID) ist per Bedienungshilfen nicht erreichbar (AX-Fehler \(err.rawValue))")
        }
        for w in windows {
            var id: CGWindowID = 0
            if Self._axGetWindow(w, &id) == .success, id == windowID { return w }
        }
        // Rueckfall: Rahmen vergleichen (_AXUIElementGetWindow ist SPI und kann fehlen).
        if let cg = WindowGeometry.frame(windowID: windowID) {
            for w in windows {
                if let f = try? axFrame(w), abs(f.minX - cg.minX) < 2, abs(f.minY - cg.minY) < 2,
                   abs(f.width - cg.width) < 2, abs(f.height - cg.height) < 2 { return w }
            }
        }
        throw IPCError(.notFound, "Fenster \(windowID) nicht unter den Fenstern der App gefunden")
    }

    private func axFrame(_ w: AXUIElement) throws -> CGRect {
        var pos = CGPoint.zero, size = CGSize.zero
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(w, kAXPositionAttribute as CFString, &v) == .success,
              let pv = v, AXValueGetValue(pv as! AXValue, .cgPoint, &pos) else {
            throw IPCError(.internalError, "AX-Position nicht lesbar")
        }
        guard AXUIElementCopyAttributeValue(w, kAXSizeAttribute as CFString, &v) == .success,
              let sv = v, AXValueGetValue(sv as! AXValue, .cgSize, &size) else {
            throw IPCError(.internalError, "AX-Groesse nicht lesbar")
        }
        return CGRect(origin: pos, size: size)
    }

    public func activate(windowID: UInt32, pid: pid_t) throws {
        let w = try axWindow(windowID: windowID, pid: pid)
        // Ab macOS 14 darf eine Hintergrund-App andere nicht mehr per activate(ignoringOtherApps:) nach vorne holen;
        // die Bedienungshilfen koennen es: AXFrontmost an der App, danach das Fenster anheben.
        AXUIElementSetAttributeValue(AXUIElementCreateApplication(pid), kAXFrontmostAttribute as CFString, kCFBooleanTrue)
        NSRunningApplication(processIdentifier: pid)?.activate()
        AXUIElementPerformAction(w, kAXRaiseAction as CFString)
        AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue)
        AXUIElementSetAttributeValue(w, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }

    public func frame(windowID: UInt32, pid: pid_t) throws -> CGRect {
        try axFrame(try axWindow(windowID: windowID, pid: pid))
    }

    public func setFrame(windowID: UInt32, pid: pid_t, origin: CGPoint?, size: CGSize?) throws {
        let w = try axWindow(windowID: windowID, pid: pid)
        // Erst die Groesse, dann die Position und die Groesse noch einmal: Verschiebt man zuerst, kappt macOS
        // die Groesse am Rand des alten Bildschirms.
        func setSize(_ s: CGSize) throws {
            var s = s
            guard let v = AXValueCreate(.cgSize, &s) else { return }
            let e = AXUIElementSetAttributeValue(w, kAXSizeAttribute as CFString, v)
            if e != .success { throw IPCError(.internalError, "AX-Groesse setzen fehlgeschlagen (\(e.rawValue))") }
        }
        func setPos(_ p: CGPoint) throws {
            var p = p
            guard let v = AXValueCreate(.cgPoint, &p) else { return }
            let e = AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, v)
            if e != .success { throw IPCError(.internalError, "AX-Position setzen fehlgeschlagen (\(e.rawValue))") }
        }
        if let s = size { try setSize(s) }
        if let o = origin {
            try setPos(o)
            // Nur wenn das Verschieben die Groesse veraendert hat (Kappen am Rand des alten Bildschirms), noch einmal setzen.
            if let s = size, let now = try? axFrame(w), abs(now.width - s.width) > 1 || abs(now.height - s.height) > 1 {
                try setSize(s)
            }
        }
    }

    public func isFrontmost(windowID: UInt32, pid: pid_t) -> Bool {
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { return false }
        guard let w = try? axWindow(windowID: windowID, pid: pid) else { return false }
        var v: CFTypeRef?
        let app = AXUIElementCreateApplication(pid)
        guard AXUIElementCopyAttributeValue(app, kAXFocusedWindowAttribute as CFString, &v) == .success,
              let focused = v else { return false }
        return CFEqual(focused, w)
    }
}

/// Rahmen und Zustand eines Fensters aus CoreGraphics (leicht, ohne Freigabe lesbar).
public enum WindowGeometry {
    public static func frame(windowID: UInt32) -> CGRect? {
        guard let arr = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(windowID)) as? [[String: Any]],
              let info = arr.first(where: { ($0[kCGWindowNumber as String] as? UInt32) == windowID }),
              let b = info[kCGWindowBounds as String] else { return nil }
        var rect = CGRect.zero
        guard CGRectMakeWithDictionaryRepresentation(b as! CFDictionary, &rect) else { return nil }
        return rect
    }

    public static func ownerPID(windowID: UInt32) -> pid_t? {
        guard let arr = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(windowID)) as? [[String: Any]],
              let info = arr.first(where: { ($0[kCGWindowNumber as String] as? UInt32) == windowID }),
              let pid = info[kCGWindowOwnerPID as String] as? Int32 else { return nil }
        return pid
    }
}

/// Letzte Rettung bei einem Absturz: Fenster, die auf einen virtuellen Monitor geholt wurden, an ihre alte Stelle
/// zurueckschieben. Best effort (AX-Aufrufe sind in einem Signalhandler nicht erlaubt, aber ohne Handler blieben die
/// Fenster auf dem verschwindenden Monitor; macOS holt sie dann selbst zurueck, nur an eine andere Stelle). Der
/// virtuelle Monitor verschwindet in jedem Fall mit dem Prozess.
public enum CrashRestore {
    nonisolated(unsafe) static var entries: [(wid: UInt32, pid: pid_t, frame: CGRect)] = []
    static let lock = NSLock()

    public static func register(windowID: UInt32, pid: pid_t, frame: CGRect) {
        lock.lock(); entries.append((windowID, pid, frame)); lock.unlock()
    }

    public static func unregister(windowID: UInt32) {
        lock.lock(); entries.removeAll { $0.wid == windowID }; lock.unlock()
    }

    public static func install() {
        for sig in [SIGSEGV, SIGBUS, SIGILL, SIGABRT, SIGFPE] {
            signal(sig) { s in
                let snapshot = CrashRestore.entries
                let ctl = AXWindowController()
                for e in snapshot { try? ctl.setFrame(windowID: e.wid, pid: e.pid, origin: e.frame.origin, size: e.frame.size) }
                signal(s, SIG_DFL)
                raise(s)
            }
        }
    }
}
