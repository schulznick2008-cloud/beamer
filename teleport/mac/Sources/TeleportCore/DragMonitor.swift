import ApplicationServices
import CoreGraphics
import Foundation

// Titelleisten-Zug melden (SCHNITTSTELLEN.md §2.1, `windows.drag_state`): Python teleportiert ein Fenster nur dann ueber
// die Uebergangskante, wenn der Nutzer es an der Titelleiste haelt. Der Helfer beobachtet dafuer die Maus passiv
// (eigener listenOnly-Tap auf eigenem Thread, nie im Beamer-Event-Tap) und prueft bei jedem Druck per
// Bedienungshilfen im Hintergrund, ob der Cursor auf einer Titelleiste liegt.

/// Ergebnis der AX-Pruefung an einem Punkt.
public struct TitleBarHit: Equatable {
    public var wid: UInt32
    public var titleBar: Bool
    public init(wid: UInt32, titleBar: Bool) { self.wid = wid; self.titleBar = titleBar }
}

/// Wie der Helfer herausfindet, was unter dem Cursor liegt. Echt: AX; in Tests eine Attrappe.
public protocol TitleBarProbe: AnyObject {
    func hit(at point: CGPoint) -> TitleBarHit?
}

/// Reine Entscheidung "Titelleiste oder nicht" aus der AX-Elternkette (leicht testbar).
public enum TitleBarRules {
    /// Hoehe des oberen Fensterrahmens, wenn AX keine Rolle `AXTitleBar` liefert (Punkte).
    public static let fallbackHeight: CGFloat = 28
    static let interactive: Set<String> = ["AXButton", "AXMenuButton", "AXPopUpButton", "AXCheckBox", "AXRadioButton",
                                           "AXSlider", "AXTextField", "AXSearchField", "AXComboBox", "AXTextArea",
                                           "AXIncrementor", "AXDisclosureTriangle", "AXSegmentedControl"]
    static let content: Set<String> = ["AXScrollArea", "AXWebArea", "AXTable", "AXOutline", "AXList", "AXBrowser",
                                       "AXTabGroup", "AXSplitGroup", "AXSheet", "AXDrawer"]

    /// `roles`: Rollen vom Element unter dem Cursor bis hinauf zum `AXWindow` (erstes = Blatt).
    public static func isTitleBar(roles: [String], point: CGPoint, windowFrame: CGRect) -> Bool {
        if roles.contains("AXTitleBar") { return true }
        if roles.contains(where: interactive.contains) || roles.contains(where: content.contains) { return false }
        guard windowFrame.contains(point) else { return false }
        return point.y - windowFrame.minY < fallbackHeight
    }

    /// Echte `AXTitleBar` im Baum: ScreenCaptureKit nimmt die Leiste mit. Ohne sie (SwiftUI,
    /// full-size content view) zeichnet der Windows-Proxy die Leiste selbst.
    public static func hasVisibleTitleBar(childRoles: [String]) -> Bool {
        childRoles.contains("AXTitleBar")
    }

    /// Ampel-Mittelpunkte in Fensterecken, Punkte. Leer, wenn eine Ampel fehlt.
    public static func lightCenters(close: CGRect?, minimize: CGRect?, zoom: CGRect?, window: CGRect) -> [(Double, Double)] {
        guard let c = close, let m = minimize, let z = zoom, window.width > 0, window.height > 0 else { return [] }
        func p(_ r: CGRect) -> (Double, Double) {
            (Double(r.midX - window.minX), Double(r.midY - window.minY))
        }
        return [p(c), p(m), p(z)]
    }
}

/// Kern: Mausereignisse rein, `windows.drag_state`-Ereignisse raus. Kein Systemzugriff ausser ueber `probe`.
public final class DragMonitorCore {
    private let probe: TitleBarProbe
    private let emit: (JSON) -> Void
    private let async: (@escaping () -> Void) -> Void
    private let now: () -> TimeInterval
    private let minInterval: TimeInterval
    private let lock = NSLock()
    private var generation = 0
    private var isDown = false
    private var lastPoint = CGPoint.zero
    private var activeWid: UInt32?
    private var lastEmit: TimeInterval = -1

    /// `async` fuehrt die AX-Pruefung aus (Standard: Hintergrund-Queue), `maxPerSecond` drosselt die Updates im Zug.
    public init(probe: TitleBarProbe, maxPerSecond: Double = 30,
                async: @escaping (@escaping () -> Void) -> Void = { DispatchQueue.global(qos: .userInteractive).async(execute: $0) },
                now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime },
                emit: @escaping (JSON) -> Void) {
        self.probe = probe
        self.emit = emit
        self.async = async
        self.now = now
        self.minInterval = 1.0 / maxPerSecond
    }

    private func event(dragging: Bool, wid: UInt32?, at p: CGPoint) -> JSON {
        var j: JSON = ["v": 1, "ev": "windows.drag_state", "dragging": dragging, "title_bar": dragging,
                       "x": Double(p.x), "y": Double(p.y)]
        if let w = wid { j["wid"] = Int(w) }
        return j
    }

    public func mouseDown(at p: CGPoint) {
        lock.lock()
        generation += 1
        let gen = generation
        isDown = true
        lastPoint = p
        activeWid = nil
        lock.unlock()
        async { [self] in
            let hit = probe.hit(at: p)
            lock.lock()
            // Nur wenn der Knopf noch dieselbe Taste ist und unten: ein spaetes Ergebnis nach dem Loslassen verwerfen.
            guard gen == generation, isDown, let h = hit, h.titleBar else { lock.unlock(); return }
            activeWid = h.wid
            lastEmit = now()
            let ev = event(dragging: true, wid: h.wid, at: lastPoint)
            lock.unlock()
            emit(ev)
        }
    }

    public func mouseDragged(at p: CGPoint) {
        lock.lock()
        lastPoint = p
        guard let wid = activeWid, now() - lastEmit >= minInterval else { lock.unlock(); return }
        lastEmit = now()
        let ev = event(dragging: true, wid: wid, at: p)
        lock.unlock()
        emit(ev)
    }

    public func mouseUp(at p: CGPoint) {
        lock.lock()
        isDown = false
        generation += 1
        let was = activeWid
        activeWid = nil
        lock.unlock()
        if was != nil { emit(event(dragging: false, wid: nil, at: p)) }
    }
}

/// Echte AX-Pruefung: Element unter dem Cursor, Elternkette bis zum Fenster, Fenster-ID per `_AXUIElementGetWindow`.
public final class AXTitleBarProbe: TitleBarProbe {
    private let system = AXUIElementCreateSystemWide()
    public init(timeout: Float = 0.05) { AXUIElementSetMessagingTimeout(system, timeout) }

    private func role(_ e: AXUIElement) -> String {
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(e, kAXRoleAttribute as CFString, &v) == .success else { return "" }
        return (v as? String) ?? ""
    }

    private func parent(_ e: AXUIElement) -> AXUIElement? {
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(e, kAXParentAttribute as CFString, &v) == .success, let p = v else { return nil }
        return (p as! AXUIElement)
    }

    private func frame(_ e: AXUIElement) -> CGRect? {
        var pos = CGPoint.zero, size = CGSize.zero
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(e, kAXPositionAttribute as CFString, &v) == .success, let pv = v,
              AXValueGetValue(pv as! AXValue, .cgPoint, &pos),
              AXUIElementCopyAttributeValue(e, kAXSizeAttribute as CFString, &v) == .success, let sv = v,
              AXValueGetValue(sv as! AXValue, .cgSize, &size) else { return nil }
        return CGRect(origin: pos, size: size)
    }

    public func hit(at point: CGPoint) -> TitleBarHit? {
        guard AXIsProcessTrusted() else { return nil }
        var found: AXUIElement?
        guard AXUIElementCopyElementAtPosition(system, Float(point.x), Float(point.y), &found) == .success,
              var el = found else { return nil }
        AXUIElementSetMessagingTimeout(el, 0.05)
        var roles: [String] = []
        var windowEl: AXUIElement?
        for _ in 0..<12 {
            let r = role(el)
            roles.append(r)
            if r == "AXWindow" { windowEl = el; break }
            guard let p = parent(el) else { break }
            el = p
        }
        guard let w = windowEl, let f = frame(w) else { return nil }
        var id: CGWindowID = 0
        guard AXWindowController._axGetWindow(w, &id) == .success, id != 0 else { return nil }
        return TitleBarHit(wid: id, titleBar: TitleBarRules.isTitleBar(roles: roles, point: point, windowFrame: f))
    }
}

/// Passiver Maus-Beobachter auf eigenem Thread (listenOnly-Tap). Ohne Freigabe (Bedienungshilfen/Eingabeüberwachung)
/// gibt es einmal ein Log auf stderr und keine Ereignisse.
public final class DragMonitor {
    private let core: DragMonitorCore
    private var thread: Thread?

    public init(emit: @escaping (JSON) -> Void, probe: TitleBarProbe = AXTitleBarProbe()) {
        core = DragMonitorCore(probe: probe, emit: emit)
    }

    public func start() {
        guard AccessibilityPermission.trusted else {
            logErr("teleport: windows.drag_state aus: Bedienungshilfen-Freigabe fehlt")
            return
        }
        let t = Thread { [core] in
            let mask = (1 << CGEventType.leftMouseDown.rawValue) | (1 << CGEventType.leftMouseDragged.rawValue)
                | (1 << CGEventType.leftMouseUp.rawValue)
            let box = Unmanaged.passRetained(core)
            var tapRef: CFMachPort?
            let cb: CGEventTapCallBack = { _, type, event, info in
                let core = Unmanaged<DragMonitorCore>.fromOpaque(info!).takeUnretainedValue()
                if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput { return Unmanaged.passUnretained(event) }
                // Eigene, eingespielte Ereignisse (Eingabe vom Proxy) sind kein Zug des Nutzers.
                if event.getIntegerValueField(.eventSourceUserData) == injectedMark { return Unmanaged.passUnretained(event) }
                switch type {
                case .leftMouseDown: core.mouseDown(at: event.location)
                case .leftMouseDragged: core.mouseDragged(at: event.location)
                case .leftMouseUp: core.mouseUp(at: event.location)
                default: break
                }
                return Unmanaged.passUnretained(event)
            }
            guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .listenOnly,
                                              eventsOfInterest: CGEventMask(mask), callback: cb, userInfo: box.toOpaque()) else {
                logErr("teleport: windows.drag_state aus: Maus-Beobachter liess sich nicht anlegen (Eingabeüberwachung freigeben?)")
                box.release()
                return
            }
            tapRef = tap
            let src = CFMachPortCreateRunLoopSource(nil, tap, 0)
            CFRunLoopAddSource(CFRunLoopGetCurrent(), src, .commonModes)
            CGEvent.tapEnable(tap: tap, enable: true)
            _ = tapRef
            CFRunLoopRun()
        }
        t.name = "beamer.teleport.dragmonitor"
        t.qualityOfService = .userInteractive
        t.start()
        thread = t
    }
}
