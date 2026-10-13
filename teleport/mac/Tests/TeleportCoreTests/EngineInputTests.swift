import CoreGraphics
import CoreMedia
import CoreVideo
import XCTest
@testable import TeleportCore

/// Bildquelle, deren Groesse der Test (oder die Fenster-Attrappe) aendern kann.
final class FakeSource: FrameSource {
    private let lock = NSLock()
    private var size: (width: Int, height: Int)
    private var pending: (width: Int, height: Int)?
    private(set) var stopped = false
    var state: String { "ok" }
    var scale: Double { 2 }
    var pixelSize: (width: Int, height: Int) { lock.lock(); defer { lock.unlock() }; return size }
    init(width: Int, height: Int) { size = (width, height) }
    func start(onFrame: @escaping (CVPixelBuffer, CMTime) -> Void, onStop: @escaping (String) -> Void) throws {}
    func stop() { stopped = true }
    func resizeWindowTo(width: Int, height: Int) { lock.lock(); pending = (width, height); lock.unlock() }
    func refresh() -> (state: String, sizeChanged: Bool) {
        lock.lock(); defer { lock.unlock() }
        guard let p = pending else { return ("ok", false) }
        pending = nil
        let changed = p.width != size.width || p.height != size.height
        size = p
        return ("ok", changed)
    }
}

final class FakeWindowController: WindowController {
    var calls: [String] = []
    var frontmost = false
    var frame = CGRect(x: 100, y: 50, width: 1200, height: 800)
    var failSetFrame: IPCError?
    var onSetFrame: ((CGPoint?, CGSize?) -> Void)?
    private let lock = NSLock()
    private func log(_ s: String) { lock.lock(); calls.append(s); lock.unlock() }
    func activate(windowID: UInt32, pid: pid_t) throws { log("activate \(windowID)") }
    func frame(windowID: UInt32, pid: pid_t) throws -> CGRect { frame }
    func setFrame(windowID: UInt32, pid: pid_t, origin: CGPoint?, size: CGSize?) throws {
        if let e = failSetFrame { throw e }
        log("setFrame \(origin.map { "\($0.x),\($0.y)" } ?? "-") \(size.map { "\($0.width)x\($0.height)" } ?? "-")")
        onSetFrame?(origin, size)
    }
    func isFrontmost(windowID: UInt32, pid: pid_t) -> Bool { frontmost }
    var snapshot: [String] { lock.lock(); defer { lock.unlock() }; return calls }
}

final class FakeVirtualDisplay: VirtualDisplayHandle {
    let displayID: UInt32 = 77
    var widthPoints: Int
    var heightPoints: Int
    var origin = CGPoint(x: -1200, y: 0)
    private(set) var destroyed = false
    private(set) var resizes: [(Int, Int)] = []
    init(w: Int, h: Int) { widthPoints = w; heightPoints = h }
    var bounds: CGRect { CGRect(x: origin.x, y: origin.y, width: CGFloat(widthPoints), height: CGFloat(heightPoints)) }
    func resize(widthPoints: Int, heightPoints: Int) -> Bool {
        resizes.append((widthPoints, heightPoints))
        self.widthPoints = widthPoints; self.heightPoints = heightPoints
        return true
    }
    func destroy() { destroyed = true }
}

final class EngineInputTests: XCTestCase {
    struct Rig {
        let engine: SenderEngine
        let sink: RecordingSink
        let ctl: FakeWindowController
        let source: FakeSource
        let events: EventLog
        var vd: () -> FakeVirtualDisplay?
    }

    final class EventLog {
        private let lock = NSLock()
        private var all: [JSON] = []
        func add(_ e: JSON) { lock.lock(); all.append(e); lock.unlock() }
        func named(_ n: String) -> [JSON] { lock.lock(); defer { lock.unlock() }; return all.filter { $0["ev"] as? String == n } }
    }

    private func rig(sourceSize: (Int, Int) = (2400, 1600)) throws -> Rig {
        let events = EventLog()
        let engine = SenderEngine { events.add($0) }
        let sink = RecordingSink()
        let ctl = FakeWindowController()
        let source = FakeSource(width: sourceSize.0, height: sourceSize.1)
        var made: FakeVirtualDisplay?
        engine.eventSink = sink
        engine.windowController = ctl
        engine.keyLayout = USKeyLayout()
        engine.accessibilityCheck = {}
        engine.windowFrameProvider = { _ in CGRect(x: 100, y: 50, width: 1200, height: 800) }
        engine.ownerPIDProvider = { _ in 4242 }
        engine.makeSource = { _, _ in source }
        engine.virtualDisplayFactory = { w, h in
            let v = FakeVirtualDisplay(w: w, h: h)
            made = v
            return v
        }
        _ = try engine.transportOpen(localPort: 0, peerHost: nil, peerPort: 0)
        return Rig(engine: engine, sink: sink, ctl: ctl, source: source, events: events, vd: { made })
    }

    private var tidCounter: UInt32 = 0
    private func params(virtual: Bool = false, mode: CaptureMode = .window(5)) -> SendParams {
        tidCounter += 1
        var k = StreamKeys.testOnly
        k.s2rKey = [UInt8](repeating: UInt8(0x40 + tidCounter), count: 32)
        return SendParams(tid: 1, mode: mode, codec: .h264, maxFps: 30, bitrateKbps: 5000, keys: k, virtualDisplay: virtual)
    }

    // MARK: Fenster (Rueckfall ohne virtuellen Monitor)

    func testWindowInputIsMappedIntoTheWindowFrame() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params())
        // Fenster 1200x800 pt bei (100,50), Stream 2400x1600 px (Retina 2x).
        try r.engine.inject(tid: 1, input: .move(x: 600, y: 400))
        try r.engine.inject(tid: 1, input: .down(x: 2000, y: 1000, button: .left, clicks: 2))
        try r.engine.inject(tid: 1, input: .up(x: 2000, y: 1000, button: .left))
        XCTAssertEqual(r.sink.events, [
            .mouse(.moved, CGPoint(x: 400, y: 250), nil, 0, 0),
            .mouse(.down, CGPoint(x: 1100, y: 550), .left, 2, 0),
            .mouse(.up, CGPoint(x: 1100, y: 550), .left, 1, 0),
        ])
        // Ausserhalb des Bildes: verworfen.
        let n = r.sink.events.count
        try r.engine.inject(tid: 1, input: .move(x: 2400, y: 10))
        XCTAssertEqual(r.sink.events.count, n)
        r.engine.shutdown()
    }

    func testTheWindowIsRaisedBeforeDownKeyAndTextButNotForMoves() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params())
        try r.engine.inject(tid: 1, input: .move(x: 5, y: 5))
        try r.engine.inject(tid: 1, input: .scroll(x: 5, y: 5, dx: 0, dy: 1, unit: .line))
        XCTAssertTrue(r.ctl.snapshot.filter { $0.hasPrefix("activate") }.isEmpty, "move und scroll aktivieren nichts")
        try r.engine.inject(tid: 1, input: .down(x: 5, y: 5, button: .left, clicks: 1))
        XCTAssertEqual(r.ctl.snapshot.filter { $0.hasPrefix("activate") }, ["activate 5"])
        // Gleich danach nicht noch einmal (Drosselung), auch nicht fuer key/text.
        try r.engine.inject(tid: 1, input: .key(name: "a", down: true))
        try r.engine.inject(tid: 1, input: .text("x"))
        XCTAssertEqual(r.ctl.snapshot.filter { $0.hasPrefix("activate") }.count, 1)
        // Liegt es schon vorne, wird nichts angefasst.
        let r2 = try rig()
        r2.ctl.frontmost = true
        _ = try r2.engine.sendStart(params())
        try r2.engine.inject(tid: 1, input: .down(x: 5, y: 5, button: .left, clicks: 1))
        XCTAssertTrue(r2.ctl.snapshot.filter { $0.hasPrefix("activate") }.isEmpty)
        r.engine.shutdown(); r2.engine.shutdown()
    }

    func testSendActivateRaisesTheWindow() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params())
        try r.engine.sendActivate(tid: 1)
        XCTAssertEqual(r.ctl.snapshot.filter { $0.hasPrefix("activate") }, ["activate 5"])
        XCTAssertThrowsError(try r.engine.sendActivate(tid: 9)) { XCTAssertEqual(($0 as? IPCError)?.code, .notFound) }
        r.engine.shutdown()
    }

    func testMissingAccessibilityPermissionIsReportedAsPermission() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params())
        r.engine.accessibilityCheck = { throw IPCError(.permission, "Bedienungshilfen nicht freigegeben") }
        for call in [{ try r.engine.inject(tid: 1, input: .move(x: 1, y: 1)) },
                     { try r.engine.sendActivate(tid: 1) },
                     { _ = try r.engine.sendResize(tid: 1, width: 800, height: 600) }] as [() throws -> Void] {
            XCTAssertThrowsError(try call()) { XCTAssertEqual(($0 as? IPCError)?.code, .permission) }
        }
        XCTAssertTrue(r.sink.events.isEmpty)
        r.engine.shutdown()
    }

    func testInjectNeedsARunningStream() throws {
        let r = try rig()
        XCTAssertThrowsError(try r.engine.inject(tid: 3, input: .move(x: 1, y: 1))) { XCTAssertEqual(($0 as? IPCError)?.code, .notFound) }
        _ = try r.engine.sendStart(params())
        r.engine.windowFrameProvider = { _ in nil } // Fenster weg
        XCTAssertThrowsError(try r.engine.inject(tid: 1, input: .move(x: 1, y: 1))) { XCTAssertEqual(($0 as? IPCError)?.code, .notFound) }
        r.engine.shutdown()
    }

    func testHeldKeysAreReleasedWhenTheStreamEnds() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params())
        try r.engine.inject(tid: 1, input: .key(name: "shift", down: true))
        try r.engine.inject(tid: 1, input: .down(x: 5, y: 5, button: .left, clicks: 1))
        try r.engine.sendStop(tid: 1, restore: true)
        let ups = r.sink.events.filter { e in
            if case .key(_, false, _) = e { return true }
            if case .mouse(.up, _, _, _, _) = e { return true }
            return false
        }
        XCTAssertEqual(ups.count, 2, "shift und Mausknopf werden losgelassen")
        r.engine.shutdown()
    }

    // MARK: send.resize

    func testResizeRoundTripReportsSizeFastAndOnce() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params())
        // Die Attrappe verhaelt sich wie macOS: nach dem AX-Setzen ist die Quelle neu gemessen.
        r.ctl.onSetFrame = { _, size in
            if let s = size { r.source.resizeWindowTo(width: Int(s.width) * 2, height: Int(s.height) * 2) }
        }
        let t0 = Date()
        let size = try r.engine.sendResize(tid: 1, width: 800, height: 600)
        let dt = Date().timeIntervalSince(t0)
        XCTAssertEqual(size.width, 1600)
        XCTAssertEqual(size.height, 1200)
        XCTAssertLessThan(dt, 0.3, "Rundlauf send.resize bis Groesse unter 300 ms (\(dt) s)")
        XCTAssertEqual(r.ctl.snapshot.filter { $0.hasPrefix("setFrame") }, ["setFrame - 800.0x600.0"])
        let ev = r.events.named("send.size")
        XCTAssertEqual(ev.count, 1, "genau ein send.size")
        XCTAssertEqual(ev.first?.int("width"), 1600)
        XCTAssertEqual(ev.first?.int("height"), 1200)
        XCTAssertEqual(ev.first?.int("tid"), 1)
        // Dieselbe Groesse noch einmal: keine zweite Meldung.
        _ = try r.engine.sendResize(tid: 1, width: 800, height: 600)
        XCTAssertEqual(r.events.named("send.size").count, 1)
        r.engine.shutdown()
    }

    func testAppInitiatedResizeIsReportedWithinAFewTicks() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params())
        r.source.resizeWindowTo(width: 1800, height: 1000) // die App aendert ihre Groesse selbst
        let t0 = Date()
        while r.events.named("send.size").isEmpty && Date().timeIntervalSince(t0) < 3 { Thread.sleep(forTimeInterval: 0.02) }
        XCTAssertEqual(r.events.named("send.size").first?.int("width"), 1800)
        r.engine.shutdown()
    }

    // MARK: virtueller Monitor

    func testVirtualDisplayLifecycle() throws {
        let r = try rig(sourceSize: (2400, 1680))
        r.ctl.frame = CGRect(x: 100, y: 50, width: 1200, height: 800)
        var madeModes: [CaptureMode] = []
        r.engine.makeSource = { mode, _ in madeModes.append(mode); return r.source }
        let info = try r.engine.sendStart(params(virtual: true))
        let vd = try XCTUnwrap(r.vd())
        XCTAssertEqual(madeModes, [.display(77)], "der ganze virtuelle Monitor wird aufgenommen")
        XCTAssertEqual(vd.widthPoints, 1200)
        XCTAssertEqual(vd.heightPoints, 800 + SenderEngine.menuBarAllowance)
        XCTAssertEqual(info.width, 2400)
        // Das Fenster sitzt unter der Menueleiste auf dem virtuellen Monitor.
        XCTAssertEqual(r.ctl.snapshot.filter { $0.hasPrefix("setFrame") }, ["setFrame -1200.0,40.0 1200.0x800.0"])
        XCTAssertTrue(r.engine.sleepGuard.isHeld)

        // Eingabe: ganze Monitorflaeche, Ursprung ist der des Monitors (negativ).
        try r.engine.inject(tid: 1, input: .move(x: 1200, y: 840))
        XCTAssertEqual(r.sink.events.last, .mouse(.moved, CGPoint(x: -1200 + 600, y: 420), nil, 0, 0))

        try r.engine.sendStop(tid: 1, restore: true)
        XCTAssertTrue(vd.destroyed, "der virtuelle Monitor ist weg")
        XCTAssertEqual(r.ctl.snapshot.last, "setFrame 100.0,50.0 1200.0x800.0", "das Fenster ist an der alten Stelle")
        XCTAssertFalse(r.engine.sleepGuard.isHeld)
        r.engine.shutdown()
    }

    func testVirtualDisplayIsRemovedOnShutdownAndEOF() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params(virtual: true))
        let vd = try XCTUnwrap(r.vd())
        r.engine.shutdown() // wie bei EOF oder shutdown
        XCTAssertTrue(vd.destroyed)
        XCTAssertEqual(r.ctl.snapshot.last, "setFrame 100.0,50.0 1200.0x800.0")
    }

    func testVirtualDisplayResizeKeepsMonitorWindowAndStreamInStep() throws {
        let r = try rig()
        _ = try r.engine.sendStart(params(virtual: true))
        let vd = try XCTUnwrap(r.vd())
        r.ctl.onSetFrame = { _, size in
            if let s = size { r.source.resizeWindowTo(width: Int(s.width) * 2, height: (Int(s.height) + SenderEngine.menuBarAllowance) * 2) }
        }
        let t0 = Date()
        let size = try r.engine.sendResize(tid: 1, width: 900, height: 500)
        XCTAssertLessThan(Date().timeIntervalSince(t0), 0.3)
        XCTAssertEqual(vd.resizes.last?.0, 900)
        XCTAssertEqual(vd.resizes.last?.1, 500 + SenderEngine.menuBarAllowance)
        XCTAssertEqual(size.width, 1800)
        XCTAssertEqual(r.events.named("send.size").count, 1)
        r.engine.shutdown()
    }

    func testFailedVirtualDisplayStartLeavesNothingBehind() throws {
        // 1. Fenster laesst sich nicht verschieben.
        let r = try rig()
        r.ctl.failSetFrame = IPCError(.internalError, "AX kaputt")
        XCTAssertThrowsError(try r.engine.sendStart(params(virtual: true)))
        XCTAssertTrue(try XCTUnwrap(r.vd()).destroyed)
        XCTAssertTrue(r.source.stopped)
        XCTAssertFalse(r.engine.sleepGuard.isHeld)
        // 2. Der Monitor laesst sich nicht anlegen.
        let r2 = try rig()
        r2.engine.virtualDisplayFactory = { _, _ in throw IPCError(.unsupported, "privates API fehlt") }
        XCTAssertThrowsError(try r2.engine.sendStart(params(virtual: true))) { XCTAssertEqual(($0 as? IPCError)?.code, .unsupported) }
        XCTAssertTrue(r2.ctl.snapshot.isEmpty, "kein Fenster wurde angefasst")
        // 3. Ohne Bedienungshilfen kein Monitor.
        let r3 = try rig()
        r3.engine.accessibilityCheck = { throw IPCError(.permission, "nein") }
        XCTAssertThrowsError(try r3.engine.sendStart(params(virtual: true))) { XCTAssertEqual(($0 as? IPCError)?.code, .permission) }
        XCTAssertNil(r3.vd())
        r.engine.shutdown(); r2.engine.shutdown(); r3.engine.shutdown()
    }

    func testVirtualDisplayNeedsAWindow() throws {
        let r = try rig()
        XCTAssertThrowsError(try r.engine.sendStart(params(virtual: true, mode: .display(1)))) { _ in }
        r.engine.shutdown()
    }

    // MARK: Schlaf

    func testSleepIsPreventedWhileATeleportRuns() throws {
        let r = try rig()
        XCTAssertFalse(r.engine.sleepGuard.isHeld)
        _ = try r.engine.sendStart(params())
        XCTAssertTrue(r.engine.sleepGuard.isHeld)
        // Die Assertion ist systemweit sichtbar.
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/pmset")
        p.arguments = ["-g", "assertions"]
        let pipe = Pipe()
        p.standardOutput = pipe
        try p.run()
        let out = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        p.waitUntilExit()
        XCTAssertTrue(out.contains("Beamer Teleport aktiv"), "pmset -g assertions:\n\(out)")
        XCTAssertTrue(out.contains("PreventUserIdleSystemSleep"))
        try r.engine.sendStop(tid: 1, restore: false)
        XCTAssertFalse(r.engine.sleepGuard.isHeld)
        r.engine.shutdown()
    }

    func testSleepGuardIsReferenceCounted() {
        let g = SleepGuard()
        g.acquire(); g.acquire()
        XCTAssertTrue(g.isHeld)
        g.release()
        XCTAssertTrue(g.isHeld)
        g.release()
        XCTAssertFalse(g.isHeld)
        g.release() // zu viel: kein Absturz, kein negativer Zaehler
        XCTAssertFalse(g.isHeld)
    }
}

/// Der echte virtuelle Monitor (privates CGVirtualDisplay-API). Braucht weder Bildschirmaufnahme noch Bedienungshilfen;
/// auf Maschinen ohne Fenster-Server-Zugriff (virtuelle CI-Maschinen) wird uebersprungen.
final class RealVirtualDisplayTests: XCTestCase {
    private func activeDisplays() -> [CGDirectDisplayID] {
        var n: UInt32 = 0
        CGGetActiveDisplayList(0, nil, &n)
        var ids = [CGDirectDisplayID](repeating: 0, count: Int(n))
        CGGetActiveDisplayList(n, &ids, &n)
        return Array(ids.prefix(Int(n)))
    }

    private func waitUntil(_ secs: Double, _ cond: () -> Bool) -> Bool {
        let end = Date().addingTimeInterval(secs)
        while Date() < end { if cond() { return true }; Thread.sleep(forTimeInterval: 0.05) }
        return cond()
    }

    func testCreateResizeAndRemoveARealVirtualDisplay() throws {
        let vd: VirtualDisplayHandle
        do {
            vd = try SystemVirtualDisplay.make(widthPoints: 800, heightPoints: 540)
        } catch {
            throw XCTSkip("virtueller Monitor auf dieser Maschine nicht moeglich: \(error)")
        }
        let id = vd.displayID
        XCTAssertNotEqual(id, 0)
        XCTAssertTrue(waitUntil(3) { self.activeDisplays().contains(id) }, "der Monitor ist aktiv")
        XCTAssertEqual(vd.bounds.width, 800)
        XCTAssertEqual(vd.bounds.height, 540)
        // HiDPI: doppelt so viele Pixel wie Punkte.
        if let mode = CGDisplayCopyDisplayMode(id) {
            XCTAssertEqual(mode.pixelWidth, 1600)
            XCTAssertEqual(mode.pixelHeight, 1080)
        }
        XCTAssertTrue(vd.resize(widthPoints: 900, heightPoints: 600))
        XCTAssertTrue(waitUntil(3) { vd.bounds.width == 900 }, "neue Groesse \(vd.bounds)")
        XCTAssertEqual(vd.bounds.height, 600)
        XCTAssertEqual(vd.displayID, id, "die Display-ID bleibt")
        vd.destroy()
        XCTAssertTrue(waitUntil(3) { !self.activeDisplays().contains(id) }, "der Monitor ist weg")
        vd.destroy() // zweites Mal: kein Absturz
    }

    func testAbsurdSizesAreRefused() {
        XCTAssertThrowsError(try SystemVirtualDisplay.make(widthPoints: 10, heightPoints: 10))
        XCTAssertThrowsError(try SystemVirtualDisplay.make(widthPoints: 100_000, heightPoints: 100_000))
    }
}
