import AppKit
import ApplicationServices
import Foundation

/// Erkennung Titelleisten-Zug fuer ``windows.drag_state`` (§2.1). Laeuft ausserhalb des Beamer-Event-Taps.
public protocol TitleBarHitTesting: AnyObject {
    func hitTitleBar(screenX: CGFloat, screenY: CGFloat) -> (wid: UInt32, titleBar: Bool)?
}

public final class TitleBarDragMonitor {
    private let emit: (JSON) -> Void
    private let probe: TitleBarHitTesting
    private let queue = DispatchQueue(label: "teleport.titlebar.drag", qos: .userInitiated)
    private var monitorDown: Any?
    private var monitorDrag: Any?
    private var monitorUp: Any?
    private var armedWid: UInt32?
    private var lastEmit = DispatchTime.now()
    private let minInterval: Double = 1.0 / 30.0

    public init(emit: @escaping (JSON) -> Void, probe: TitleBarHitTesting) {
        self.emit = emit
        self.probe = probe
    }

    public func start() {
        guard monitorDown == nil else { return }
        monitorDown = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDown) { [weak self] ev in
            self?.onDown(ev)
        }
        monitorDrag = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDragged) { [weak self] ev in
            self?.onDrag(ev)
        }
        monitorUp = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseUp) { [weak self] _ in
            self?.onUp()
        }
    }

    public func stop() {
        if let m = monitorDown { NSEvent.removeMonitor(m) }
        if let m = monitorDrag { NSEvent.removeMonitor(m) }
        if let m = monitorUp { NSEvent.removeMonitor(m) }
        monitorDown = nil
        monitorDrag = nil
        monitorUp = nil
        armedWid = nil
    }

    private func onDown(_ ev: NSEvent) {
        let loc = ev.locationInWindow
        let screen = ev.window?.convertToScreen(NSRect(origin: loc, size: .zero)).origin
            ?? NSEvent.mouseLocation
        queue.async { [weak self] in
            self?.evaluate(screenX: screen.x, screenY: screen.y, dragging: true)
        }
    }

    private func onDrag(_ ev: NSEvent) {
        let screen = NSEvent.mouseLocation
        queue.async { [weak self] in
            self?.evaluate(screenX: screen.x, screenY: screen.y, dragging: true)
        }
    }

    private func onUp() {
        queue.async { [weak self] in
            self?.armedWid = nil
            self?.emitState(dragging: false, titleBar: false, wid: nil, x: NSEvent.mouseLocation.x, y: NSEvent.mouseLocation.y)
        }
    }

    private func evaluate(screenX: CGFloat, screenY: CGFloat, dragging: Bool) {
        guard let hit = probe.hitTitleBar(screenX: screenX, screenY: screenY), hit.titleBar else {
            armedWid = nil
            return
        }
        armedWid = hit.wid
        emitState(dragging: dragging, titleBar: true, wid: hit.wid, x: screenX, y: screenY)
    }

    private func emitState(dragging: Bool, titleBar: Bool, wid: UInt32?, x: CGFloat, y: CGFloat) {
        let now = DispatchTime.now().uptimeNanoseconds
        let last = lastEmit.uptimeNanoseconds
        if dragging && Double(now - last) / 1_000_000_000.0 < minInterval {
            return
        }
        lastEmit = DispatchTime.now()
        var e: JSON = [
            "v": 1,
            "ev": "windows.drag_state",
            "dragging": dragging,
            "title_bar": titleBar,
            "x": Double(x),
            "y": Double(y),
        ]
        if let wid = wid { e["wid"] = Int(wid) }
        emit(e)
    }
}

/// AX/CGWindow-Erkennung (Geraet); in Tests durch Fake ersetzen.
public final class AXTitleBarProbe: TitleBarHitTesting {
    private let timeout: Float = 0.05
    private static let titleBarRole = "AXTitleBar" as CFString

    @_silgen_name("_AXUIElementGetWindow")
    private static func axGetWindow(_ element: AXUIElement, _ out: UnsafeMutablePointer<CGWindowID>) -> AXError

    public init() {}

    public func hitTitleBar(screenX: CGFloat, screenY: CGFloat) -> (wid: UInt32, titleBar: Bool)? {
        let point = CGPoint(x: screenX, y: screenY)
        let systemWide = AXUIElementCreateSystemWide()
        AXUIElementSetMessagingTimeout(systemWide, timeout)
        var element: AXUIElement?
        let err = AXUIElementCopyElementAtPosition(systemWide, Float(point.x), Float(point.y), &element)
        if err != .success || element == nil {
            return nil
        }
        guard let wid = windowID(for: element!) else { return nil }
        let titleBar = isTitleBarRegion(element: element!, at: point)
        return (wid, titleBar)
    }

    private func windowID(for element: AXUIElement) -> UInt32? {
        var wid: CGWindowID = 0
        if Self.axGetWindow(element, &wid) == .success {
            return wid
        }
        return nil
    }

    private func isTitleBarRegion(element: AXUIElement, at point: CGPoint) -> Bool {
        var role: CFTypeRef?
        if AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &role) == .success,
           let r = role as? String, r == Self.titleBarRole as String {
            return true
        }
        var windowRef: CFTypeRef?
        if AXUIElementCopyAttributeValue(element, kAXWindowAttribute as CFString, &windowRef) != .success {
            windowRef = element
        }
        let win: AXUIElement
        if let ref = windowRef {
            win = unsafeBitCast(ref, to: AXUIElement.self)
        } else {
            win = element
        }
        var posRef: CFTypeRef?
        var sizeRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(win, kAXPositionAttribute as CFString, &posRef) == .success,
              AXUIElementCopyAttributeValue(win, kAXSizeAttribute as CFString, &sizeRef) == .success,
              let posVal = posRef, let sizeVal = sizeRef,
              AXValueGetType(posVal as! AXValue) == .cgPoint,
              AXValueGetType(sizeVal as! AXValue) == .cgSize else {
            return false
        }
        var origin = CGPoint.zero
        var size = CGSize.zero
        AXValueGetValue(posVal as! AXValue, .cgPoint, &origin)
        AXValueGetValue(sizeVal as! AXValue, .cgSize, &size)
        let band: CGFloat = 28
        let top = origin.y + size.height
        return point.y >= top - band && point.y <= top
    }
}
