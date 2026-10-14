import AppKit
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

/// Zeitlimits fuer ScreenCaptureKit-Aufrufe (Sekunden). Unter Last kehren sie manchmal nie zurueck.
enum SCKTimeouts {
    static let query = 2.5     // SCShareableContent (je Versuch, zwei Versuche)
    static let start = 3.5     // startCapture (je Versuch, zwei Versuche)
    static let stop = 1.0      // stopCapture
    static let update = 1.0    // updateConfiguration
}

/// `SCShareableContent` mit einem zweiten Versuch: Die Abfrage dauert normal ~60 ms; kehrt sie nicht zurueck (unter
/// Last, mit mehreren Helfern), hilft meist ein neuer Aufruf.
func queryContent() throws -> SCShareableContent {
    var last: Error?
    for attempt in 1...2 {
        do {
            return try runBlockingLocked(timeout: SCKTimeouts.query, "SCShareableContent") {
                try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: false)
            }
        } catch let t as BlockingTimeout {
            last = t
            logErr("teleport: \(t) (Versuch \(attempt) von 2)")
        }
    }
    throw last!
}

/// Quelle der Pixelbilder (ScreenCaptureKit oder, fuer Selbsttests, ein Muster-Erzeuger).
public protocol FrameSource: AnyObject {
    func start(onFrame: @escaping (CVPixelBuffer, CMTime) -> Void, onStop: @escaping (String) -> Void) throws
    func stop()
    /// Laufender Zustand fuer `stats`: "ok", "minimized", "offscreen".
    var state: String { get }
    var pixelSize: (width: Int, height: Int) { get }
    var scale: Double { get }
    /// Einmal pro Sekunde: Zustand neu lesen ("ok", "minimized_or_other_space", "gone") und die Capture-Groesse
    /// an eine geaenderte Fenstergroesse anpassen. `sizeChanged`, wenn sich die Pixelgroesse geaendert hat.
    func refresh() -> (state: String, sizeChanged: Bool)
    /// Effektive Bildrate senken oder wieder anheben (langsamer Encoder, Energiesparmodus).
    func setMaxFps(_ fps: Int)
    /// Aktuelle Groesse der Quelle in Punkten, billig (ohne ScreenCaptureKit), um Groessenaenderungen schnell zu
    /// bemerken. `nil`, wenn unbekannt.
    func currentPointSize() -> CGSize?
    /// Capture auf eine bekannte neue Groesse in Punkten umstellen, ohne ScreenCaptureKit erst abzufragen (schnell:
    /// ein Aufruf). `true`, wenn sich die Pixelgroesse geaendert hat. Standard: `refresh()`.
    func applySize(points: CGSize) -> Bool
}

extension FrameSource {
    public func currentPointSize() -> CGSize? { nil }
    public func applySize(points: CGSize) -> Bool { refresh().sizeChanged }
    public func refresh() -> (state: String, sizeChanged: Bool) { (state, false) }
    public func setMaxFps(_ fps: Int) {}
}

enum CapturePermission {
    static func check() throws {
        // Testhaken: BEAMER_TELEPORT_FAKE_NO_PERMISSION=1 tut so, als fehle die Freigabe (Abnahme: kein Absturz).
        let fake = ProcessInfo.processInfo.environment["BEAMER_TELEPORT_FAKE_NO_PERMISSION"] == "1"
        if fake || !CGPreflightScreenCaptureAccess() {
            throw IPCError(.permission,
                "Bildschirmaufnahme nicht freigegeben. Systemeinstellungen > Datenschutz & Sicherheit > Bildschirm- und Systemaudioaufnahme: dem Programm, das Beamer startet (Terminal/Beamer), die Freigabe geben und neu starten.")
        }
    }

    /// Ein Fehler aus ScreenCaptureKit, der "nicht freigegeben" bedeutet?
    static func mapped(_ error: Error) -> IPCError {
        if let t = error as? BlockingTimeout {
            return IPCError(.busy, "ScreenCaptureKit antwortet nicht (\(t.label), \(Int(t.seconds)) s). Der Mac ist wohl ueberlastet; spaeter erneut versuchen.")
        }
        let ns = error as NSError
        if ns.domain == SCStreamErrorDomain, ns.code == SCStreamError.userDeclined.rawValue,
           CGPreflightScreenCaptureAccess() {
            // Die Freigabe ist da, ScreenCaptureKit meldet sie unter Last trotzdem manchmal als verweigert.
            return IPCError(.busy, "ScreenCaptureKit meldet kurzzeitig \"nicht freigegeben\", obwohl die Freigabe vorhanden ist (Mac ueberlastet?). Spaeter erneut versuchen.")
        }
        if ns.domain == SCStreamErrorDomain, ns.code == SCStreamError.userDeclined.rawValue {
            return IPCError(.permission, "Bildschirmaufnahme nicht freigegeben (SCStreamError.userDeclined). In den Systemeinstellungen unter Datenschutz & Sicherheit freigeben.")
        }
        return IPCError(.internalError, "ScreenCaptureKit: \(error.localizedDescription)")
    }
}

/// Welche Fenster Teleport-Kandidaten sind. Unsichtbare System-Hilfsfenster (Service-Prozesse, leere
/// Fenster ohne Titel, die nicht auf dem Bildschirm liegen, Fenster von Hilfsprozessen) fallen heraus.
public enum WindowFilter {
    public static func keep(layer: Int, width: Double, height: Double, isOnScreen: Bool, title: String,
                            regularApp: Bool) -> Bool {
        // Menueleisten-/Overlay-Fenster und winzige Hilfsfenster sind keine Kandidaten.
        guard layer == 0, width >= 100, height >= 50 else { return false }
        let hasTitle = !title.trimmingCharacters(in: .whitespaces).isEmpty
        // Sichtbar oder (mit Titel) minimiert: unsichtbar und ohne Titel ist ein Hilfsfenster.
        guard isOnScreen || hasTitle else { return false }
        // Nur Fenster normaler Apps (Hilfsprozesse wie WindowManager oder "Open and Save Panel Service" nicht).
        return regularApp
    }
}

public enum WindowCatalog {
    public static func list() throws -> [WindowInfo] {
        try CapturePermission.check()
        let content: SCShareableContent
        do {
            content = try queryContent()
        } catch {
            throw CapturePermission.mapped(error)
        }
        let displays = content.displays
        return content.windows.compactMap { w -> WindowInfo? in
            let policyRegular = w.owningApplication.flatMap {
                NSRunningApplication(processIdentifier: $0.processID)?.activationPolicy == .regular
            } ?? false
            guard WindowFilter.keep(layer: Int(w.windowLayer), width: Double(w.frame.width),
                                    height: Double(w.frame.height), isOnScreen: w.isOnScreen,
                                    title: w.title ?? "", regularApp: policyRegular) else { return nil }
            let disp = displays.max(by: { a, b in
                a.frame.intersection(w.frame).area < b.frame.intersection(w.frame).area
            })
            return WindowInfo(
                wid: w.windowID, pid: w.owningApplication?.processID ?? 0,
                app: w.owningApplication?.applicationName ?? "", title: w.title ?? "",
                x: Double(w.frame.minX), y: Double(w.frame.minY),
                w: Double(w.frame.width), h: Double(w.frame.height),
                display: disp?.displayID ?? 0, scale: backingScale(of: disp))
        }
    }

    static func backingScale(of display: SCDisplay?) -> Double {
        guard let d = display, let mode = CGDisplayCopyDisplayMode(d.displayID), mode.width > 0 else { return 1 }
        return Double(mode.pixelWidth) / Double(mode.width)
    }

    /// Zustand eines Fensters ueber CoreGraphics (leichtgewichtig, jede Sekunde abfragbar).
    public static func windowState(_ id: UInt32) -> (exists: Bool, onScreen: Bool, bounds: CGRect) {
        guard let arr = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(id)) as? [[String: Any]],
              let info = arr.first(where: { ($0[kCGWindowNumber as String] as? UInt32) == id }) else {
            return (false, false, .zero)
        }
        let onScreen = (info[kCGWindowIsOnscreen as String] as? Bool) ?? false
        var rect = CGRect.zero
        if let b = info[kCGWindowBounds as String] {
            CGRectMakeWithDictionaryRepresentation(b as! CFDictionary, &rect)
        }
        return (true, onScreen, rect)
    }
}

private extension CGRect {
    var area: CGFloat { isNull ? 0 : width * height }
}

/// ScreenCaptureKit-Strom fuer ein Fenster oder einen Bildschirm.
public final class ScreenCaptureSource: NSObject, FrameSource, SCStreamOutput, SCStreamDelegate {
    private static let hintLock = NSLock()
    nonisolated(unsafe) private static var scaleHints: [UInt32: Double] = [:]

    /// Fuer einen frisch angelegten virtuellen HiDPI-Monitor meldet ScreenCaptureKit/CoreGraphics zunaechst Skalierung 1.
    public static func setScaleHint(_ s: Double, forDisplay id: UInt32) {
        hintLock.lock(); scaleHints[id] = s; hintLock.unlock()
    }

    public static func clearScaleHint(forDisplay id: UInt32) {
        hintLock.lock(); scaleHints.removeValue(forKey: id); hintLock.unlock()
    }

    static func scaleHint(forDisplay id: UInt32) -> Double? {
        hintLock.lock(); defer { hintLock.unlock() }
        return scaleHints[id]
    }

    private let mode: CaptureMode
    private let maxFps: Int
    private var stream: SCStream?
    private var filter: SCContentFilter?
    private var resolvedDisplay: CGDirectDisplayID?
    private var config: SCStreamConfiguration?
    private var onFrame: ((CVPixelBuffer, CMTime) -> Void)?
    private var onStop: ((String) -> Void)?
    private let queue = DispatchQueue(label: "teleport.capture", qos: .userInteractive)
    private(set) var width = 0
    private(set) var height = 0
    public private(set) var scale = 1.0
    private let stateLock = NSLock()
    private var lastStatus: SCFrameStatus = .complete
    private var stopped = false

    public init(mode: CaptureMode, maxFps: Int) {
        self.mode = mode
        self.maxFps = maxFps
    }

    public var pixelSize: (width: Int, height: Int) { (width, height) }

    public private(set) var state = "ok"

    /// Richtet den Filter ein und liefert Pixelgroesse und Skalierung. Wirft `permission`/`not_found`.
    public func prepare() throws {
        try CapturePermission.check()
        let content: SCShareableContent
        do {
            content = try queryContent()
        } catch {
            throw CapturePermission.mapped(error)
        }
        let filter: SCContentFilter
        var windowFramePoints: CGRect?
        switch mode {
        case .window(let id):
            guard let w = content.windows.first(where: { $0.windowID == id }) else {
                throw IPCError(.notFound, "Fenster \(id) nicht gefunden")
            }
            filter = SCContentFilter(desktopIndependentWindow: w)
            // Voller Fensterrahmen inkl. Titelleiste und Ampeln (nicht nur contentRect).
            windowFramePoints = w.frame
        case .display(let id):
            if content.displays.isEmpty {
                // Unter Last liefert ScreenCaptureKit manchmal eine leere Liste: kein Fehler des Aufrufers.
                throw IPCError(.busy, "ScreenCaptureKit liefert keine Bildschirme (Mac ueberlastet?). Spaeter erneut versuchen.")
            }
            guard let d = content.displays.first(where: { $0.displayID == id }) ?? (id == 0 ? content.displays.first : nil) else {
                throw IPCError(.notFound, "Bildschirm \(id) nicht gefunden")
            }
            filter = SCContentFilter(display: d, excludingWindows: [])
            resolvedDisplay = d.displayID
        }
        let info = SCShareableContent.info(for: filter)
        var pxScale = Double(info.pointPixelScale)
        // Ein virtueller HiDPI-Monitor meldet hier manchmal 1; die echte Pixelzahl steht im Anzeigemodus.
        if let did = resolvedDisplay, let dm = CGDisplayCopyDisplayMode(did), dm.width > 0 {
            pxScale = max(pxScale, Double(dm.pixelWidth) / Double(dm.width))
        }
        if let did = resolvedDisplay, let hint = Self.scaleHint(forDisplay: did) { pxScale = max(pxScale, hint) }
        scale = pxScale
        let sizePoints: CGSize = {
            if let f = windowFramePoints { return f.size }
            return CGSize(width: Double(info.contentRect.width), height: Double(info.contentRect.height))
        }()
        // Gerade Pixelzahlen, sonst verweigert mancher Encoder die Arbeit.
        width = max(2, Int((Double(sizePoints.width) * pxScale).rounded()) & ~1)
        height = max(2, Int((Double(sizePoints.height) * pxScale).rounded()) & ~1)

        let cfg = SCStreamConfiguration()
        cfg.width = width
        cfg.height = height
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(maxFps))
        cfg.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        cfg.showsCursor = false
        cfg.queueDepth = 4
        cfg.scalesToFit = true
        cfg.colorMatrix = CGDisplayStream.yCbCrMatrix_ITU_R_709_2
        cfg.colorSpaceName = CGColorSpace.sRGB
        self.filter = filter
        self.config = cfg
        let s = SCStream(filter: filter, configuration: cfg, delegate: self)
        try s.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        stream = s
    }

    public func start(onFrame: @escaping (CVPixelBuffer, CMTime) -> Void, onStop: @escaping (String) -> Void) throws {
        guard stream != nil else { throw IPCError(.internalError, "Capture nicht vorbereitet") }
        self.onFrame = onFrame
        self.onStop = onStop
        // Zwei Versuche: Starten mehrere Helfer gleichzeitig, bleibt oft ein startCapture haengen, waehrend ein
        // frischer Stream kurz danach klappt (nacheinander gestartet geht es immer).
        for attempt in 1...2 {
            guard let s = stream else { throw IPCError(.internalError, "Capture nicht vorbereitet") }
            do {
                try runBlockingLocked(timeout: SCKTimeouts.start, "startCapture") { try await s.startCapture() }
                return
            } catch let t as BlockingTimeout {
                logErr("teleport: \(t) (Versuch \(attempt) von 2)")
                // Der Stream ist in unbekanntem Zustand: verwerfen und im Hintergrund anhalten.
                stream = nil
                Task.detached { try? await s.stopCapture() }
                if attempt == 2 {
                    self.onFrame = nil
                    throw CapturePermission.mapped(t)
                }
                try prepare() // frischer Filter und Stream
                self.onFrame = onFrame
            } catch {
                throw CapturePermission.mapped(error)
            }
        }
    }

    public func setMaxFps(_ fps: Int) {
        guard let cfg = config, let s = stream, fps > 0 else { return }
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        _ = try? runBlocking(timeout: SCKTimeouts.update, "updateConfiguration") { try await s.updateConfiguration(cfg) }
    }

    public func applySize(points: CGSize) -> Bool {
        guard let cfg = config, let s = stream, points.width > 0, points.height > 0 else { return false }
        let w = max(2, Int((Double(points.width) * scale).rounded()) & ~1)
        let h = max(2, Int((Double(points.height) * scale).rounded()) & ~1)
        guard w != width || h != height else { return false }
        cfg.width = w
        cfg.height = h
        do {
            try runBlocking(timeout: SCKTimeouts.update, "updateConfiguration") { try await s.updateConfiguration(cfg) }
        } catch {
            return false
        }
        width = w
        height = h
        return true
    }

    public func currentPointSize() -> CGSize? {
        switch mode {
        case .window(let id): return WindowGeometry.frame(windowID: id)?.size
        case .display(let id): return CGDisplayBounds(id).size
        }
    }

    public func refresh() -> (state: String, sizeChanged: Bool) {
        guard let cfg = config, let s = stream else { return (state, false) }
        guard let content = try? runBlockingLocked(timeout: SCKTimeouts.query, "SCShareableContent (refresh)", {
            try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        }) else { return (state, false) }
        let framePoints: CGRect
        var visible = true
        switch mode {
        case .window(let id):
            guard let win = content.windows.first(where: { $0.windowID == id }) else {
                state = "gone"
                return (state, false)
            }
            framePoints = win.frame
            visible = win.isOnScreen
        case .display(let id):
            guard let d = content.displays.first(where: { $0.displayID == id }) ?? (id == 0 ? content.displays.first : nil) else {
                state = "gone"  // z. B. der virtuelle Monitor wurde entfernt
                return (state, false)
            }
            framePoints = d.frame
        }
        stateLock.lock()
        let st = lastStatus
        stateLock.unlock()
        let hidden = !visible || st == .suspended || st == .blank
        state = hidden ? "minimized_or_other_space" : "ok"
        let w = max(2, Int((Double(framePoints.width) * scale).rounded()) & ~1)
        let h = max(2, Int((Double(framePoints.height) * scale).rounded()) & ~1)
        guard visible, w != width || h != height else { return (state, false) }
        cfg.width = w
        cfg.height = h
        do {
            try runBlocking(timeout: SCKTimeouts.update, "updateConfiguration") { try await s.updateConfiguration(cfg) }
        } catch {
            return (state, false)
        }
        width = w
        height = h
        return (state, true)
    }

    public func stop() {
        stateLock.lock()
        let already = stopped
        stopped = true
        stateLock.unlock()
        if already { return }
        let s = stream
        stream = nil
        onFrame = nil
        // Kehrt stopCapture nicht zurueck (Last), wird der Stream trotzdem verworfen: nach 1 s ist aufgeraeumt.
        if let s = s {
            do {
                try runBlocking(timeout: SCKTimeouts.stop, "stopCapture") { try await s.stopCapture() }
            } catch let t as BlockingTimeout {
                logErr("teleport: \(t); Stream wird verworfen")
            } catch {}
        }
    }

    // MARK: SCStreamOutput / SCStreamDelegate

    public func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sb.isValid, let cb = onFrame else { return }
        // Nur Frames mit neuem Inhalt ("complete"); Leerlauf-/Pausenmeldungen haben keinen Pixelpuffer.
        if let atts = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
           let raw = atts.first?[.status] as? Int, let status = SCFrameStatus(rawValue: raw) {
            noteStatus(status)
            if status != .complete { return }
        } else {
            noteStatus(.complete)
        }
        guard let pb = CMSampleBufferGetImageBuffer(sb) else { return }
        cb(pb, CMSampleBufferGetPresentationTimeStamp(sb))
    }

    /// Der letzte Frame-Status: `.suspended`/`.blank` heisst, das Fenster wird gerade nicht gezeichnet
    /// (minimiert, anderer Space, verdeckt hinter dem Dock).
    private func noteStatus(_ status: SCFrameStatus) {
        stateLock.lock()
        let changed = lastStatus != status
        lastStatus = status
        stateLock.unlock()
        if changed, ProcessInfo.processInfo.environment["BEAMER_TELEPORT_DEBUG"] == "1" {
            logErr("capture-status: \(status.rawValue)")
        }
    }

    public func stream(_ stream: SCStream, didStopWithError error: Error) {
        stateLock.lock()
        let was = stopped
        stopped = true
        stateLock.unlock()
        if was { return }
        let ns = error as NSError
        let closed: Set<Int> = [SCStreamError.noWindowList.rawValue, SCStreamError.noDisplayList.rawValue,
                                SCStreamError.noCaptureSource.rawValue]
        if ns.domain == SCStreamErrorDomain, closed.contains(ns.code) {
            onStop?("window_closed")
        } else {
            onStop?("capture_error: \(error.localizedDescription)")
        }
    }
}
