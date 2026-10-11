import AppKit
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

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
}

extension FrameSource {
    public func refresh() -> (state: String, sizeChanged: Bool) { (state, false) }
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
        let ns = error as NSError
        if ns.domain == SCStreamErrorDomain, ns.code == SCStreamError.userDeclined.rawValue {
            return IPCError(.permission, "Bildschirmaufnahme nicht freigegeben (SCStreamError.userDeclined). In den Systemeinstellungen unter Datenschutz & Sicherheit freigeben.")
        }
        return IPCError(.internalError, "ScreenCaptureKit: \(error.localizedDescription)")
    }
}

/// Blockiert einen Hintergrund-Thread, bis die async-Aufgabe fertig ist (nie auf dem Hauptthread aufrufen).
func runBlocking<T>(_ work: @escaping () async throws -> T) throws -> T {
    let sem = DispatchSemaphore(value: 0)
    var result: Result<T, Error>?
    Task.detached {
        do { result = .success(try await work()) } catch { result = .failure(error) }
        sem.signal()
    }
    sem.wait()
    return try result!.get()
}

public enum WindowCatalog {
    public static func list() throws -> [WindowInfo] {
        try CapturePermission.check()
        let content: SCShareableContent
        do {
            content = try runBlocking {
                try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: false)
            }
        } catch {
            throw CapturePermission.mapped(error)
        }
        let displays = content.displays
        return content.windows.compactMap { w -> WindowInfo? in
            // Menueleisten-/Overlay-Fenster und winzige Hilfsfenster sind keine Teleport-Kandidaten.
            guard w.windowLayer == 0, w.frame.width >= 100, w.frame.height >= 50 else { return nil }
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
    private let mode: CaptureMode
    private let maxFps: Int
    private var stream: SCStream?
    private var filter: SCContentFilter?
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
            content = try runBlocking {
                try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: false)
            }
        } catch {
            throw CapturePermission.mapped(error)
        }
        let filter: SCContentFilter
        switch mode {
        case .window(let id):
            guard let w = content.windows.first(where: { $0.windowID == id }) else {
                throw IPCError(.notFound, "Fenster \(id) nicht gefunden")
            }
            filter = SCContentFilter(desktopIndependentWindow: w)
        case .display(let id):
            guard let d = content.displays.first(where: { $0.displayID == id }) ?? (id == 0 ? content.displays.first : nil) else {
                throw IPCError(.notFound, "Bildschirm \(id) nicht gefunden")
            }
            filter = SCContentFilter(display: d, excludingWindows: [])
        }
        let info = SCShareableContent.info(for: filter)
        let pxScale = Double(info.pointPixelScale)
        scale = pxScale
        // Gerade Pixelzahlen, sonst verweigert mancher Encoder die Arbeit.
        width = max(2, Int((Double(info.contentRect.width) * pxScale).rounded()) & ~1)
        height = max(2, Int((Double(info.contentRect.height) * pxScale).rounded()) & ~1)

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
        guard let s = stream else { throw IPCError(.internalError, "Capture nicht vorbereitet") }
        self.onFrame = onFrame
        self.onStop = onStop
        do {
            try runBlocking { try await s.startCapture() }
        } catch {
            throw CapturePermission.mapped(error)
        }
    }

    public func refresh() -> (state: String, sizeChanged: Bool) {
        guard case .window(let id) = mode, let cfg = config, let s = stream else { return (state, false) }
        guard let content = try? runBlocking({
            try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        }) else { return (state, false) }
        guard let win = content.windows.first(where: { $0.windowID == id }) else {
            state = "gone"
            return (state, false)
        }
        stateLock.lock()
        let st = lastStatus
        stateLock.unlock()
        let hidden = !win.isOnScreen || st == .suspended || st == .blank
        state = hidden ? "minimized_or_other_space" : "ok"
        let w = max(2, Int((Double(win.frame.width) * scale).rounded()) & ~1)
        let h = max(2, Int((Double(win.frame.height) * scale).rounded()) & ~1)
        guard win.isOnScreen, w != width || h != height else { return (state, false) }
        cfg.width = w
        cfg.height = h
        do {
            try runBlocking { try await s.updateConfiguration(cfg) }
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
        if let s = s { _ = try? runBlocking { try await s.stopCapture() } }
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
