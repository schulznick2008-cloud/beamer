import CoreGraphics
import XCTest
@testable import TeleportCore

final class FakeProbe: TitleBarProbe {
    var result: TitleBarHit?
    private(set) var calls = 0
    func hit(at point: CGPoint) -> TitleBarHit? { calls += 1; return result }
}

final class DragMonitorTests: XCTestCase {
    final class Log { var events: [JSON] = []; var t: TimeInterval = 0 }

    private func make(_ probe: FakeProbe, log: Log, deferred: Bool = false) -> (DragMonitorCore, () -> Void) {
        var queued: [() -> Void] = []
        let core = DragMonitorCore(probe: probe, maxPerSecond: 30,
                                   async: { deferred ? queued.append($0) : $0() },
                                   now: { log.t }, emit: { log.events.append($0) })
        return (core, { let q = queued; queued = []; q.forEach { $0() } })
    }

    func testTitleBarDownEmitsDragStateWithWidAndPoint() {
        let probe = FakeProbe(); probe.result = TitleBarHit(wid: 42, titleBar: true)
        let log = Log(); let (core, _) = make(probe, log: log)
        core.mouseDown(at: CGPoint(x: 300, y: 120))
        XCTAssertEqual(log.events.count, 1)
        let e = log.events[0]
        XCTAssertEqual(e["ev"] as? String, "windows.drag_state")
        XCTAssertEqual(e["dragging"] as? Bool, true)
        XCTAssertEqual(e["title_bar"] as? Bool, true)
        XCTAssertEqual(e["wid"] as? Int, 42)
        XCTAssertEqual(e["x"] as? Double, 300)
        XCTAssertEqual(e["y"] as? Double, 120)
        XCTAssertEqual(e["v"] as? Int, 1)
    }

    func testContentClickEmitsNothingAtAll() {
        let probe = FakeProbe(); probe.result = TitleBarHit(wid: 42, titleBar: false)
        let log = Log(); let (core, _) = make(probe, log: log)
        core.mouseDown(at: CGPoint(x: 300, y: 400))
        core.mouseDragged(at: CGPoint(x: 310, y: 410))
        core.mouseUp(at: CGPoint(x: 310, y: 410))
        XCTAssertTrue(log.events.isEmpty)
        probe.result = nil // kein Fenster / AX-Timeout
        core.mouseDown(at: .zero); core.mouseUp(at: .zero)
        XCTAssertTrue(log.events.isEmpty)
    }

    func testUpEmitsDraggingFalseWithoutWid() {
        let probe = FakeProbe(); probe.result = TitleBarHit(wid: 7, titleBar: true)
        let log = Log(); let (core, _) = make(probe, log: log)
        core.mouseDown(at: CGPoint(x: 10, y: 10))
        core.mouseUp(at: CGPoint(x: 900, y: 12))
        XCTAssertEqual(log.events.count, 2)
        let e = log.events[1]
        XCTAssertEqual(e["dragging"] as? Bool, false)
        XCTAssertEqual(e["title_bar"] as? Bool, false)
        XCTAssertNil(e["wid"])
        XCTAssertEqual(e["x"] as? Double, 900)
    }

    func testUpdatesDuringTheDragAreThrottledTo30PerSecond() {
        let probe = FakeProbe(); probe.result = TitleBarHit(wid: 7, titleBar: true)
        let log = Log(); let (core, _) = make(probe, log: log)
        core.mouseDown(at: CGPoint(x: 0, y: 0))
        // 1 s lang Mausbewegungen alle 2 ms (500 Ereignisse): hoechstens ~30 Updates.
        for i in 1...500 {
            log.t = Double(i) * 0.002
            core.mouseDragged(at: CGPoint(x: Double(i), y: 0))
        }
        let updates = log.events.filter { $0["dragging"] as? Bool == true }.count - 1
        XCTAssertGreaterThanOrEqual(updates, 28)
        XCTAssertLessThanOrEqual(updates, 31, "gedrosselt auf 30/s, war \(updates)")
        // Das letzte Update traegt die neuere Position.
        XCTAssertGreaterThan(log.events.last?["x"] as? Double ?? 0, 400)
    }

    func testLateProbeResultAfterMouseUpIsDiscarded() {
        let probe = FakeProbe(); probe.result = TitleBarHit(wid: 7, titleBar: true)
        let log = Log(); let (core, run) = make(probe, log: log, deferred: true)
        core.mouseDown(at: CGPoint(x: 5, y: 5))
        core.mouseUp(at: CGPoint(x: 5, y: 5)) // Knopf schon wieder los, bevor AX antwortet
        run()
        XCTAssertTrue(log.events.isEmpty, "kein haengendes dragging:true")
    }

    func testEachMouseDownProbesOnlyOnceAndNotOnDrag() {
        let probe = FakeProbe(); probe.result = TitleBarHit(wid: 7, titleBar: true)
        let log = Log(); let (core, _) = make(probe, log: log)
        core.mouseDown(at: .zero)
        for i in 1...10 { log.t = Double(i); core.mouseDragged(at: CGPoint(x: i, y: 0)) }
        XCTAssertEqual(probe.calls, 1)
    }
}

final class TitleBarRulesTests: XCTestCase {
    let frame = CGRect(x: 100, y: 100, width: 800, height: 600)

    func testExplicitTitleBarRole() {
        XCTAssertTrue(TitleBarRules.isTitleBar(roles: ["AXTitleBar", "AXWindow"], point: CGPoint(x: 400, y: 400), windowFrame: frame))
    }

    func testTopFrameOfTheWindowCounts() {
        XCTAssertTrue(TitleBarRules.isTitleBar(roles: ["AXWindow"], point: CGPoint(x: 400, y: 110), windowFrame: frame))
        XCTAssertTrue(TitleBarRules.isTitleBar(roles: ["AXStaticText", "AXGroup", "AXWindow"], point: CGPoint(x: 400, y: 127), windowFrame: frame))
        XCTAssertFalse(TitleBarRules.isTitleBar(roles: ["AXWindow"], point: CGPoint(x: 400, y: 130), windowFrame: frame), "unter 28 pt: Inhalt")
    }

    func testButtonsAndContentInTheTopStripAreNotTheTitleBar() {
        XCTAssertFalse(TitleBarRules.isTitleBar(roles: ["AXButton", "AXWindow"], point: CGPoint(x: 120, y: 110), windowFrame: frame))
        XCTAssertFalse(TitleBarRules.isTitleBar(roles: ["AXTextField", "AXGroup", "AXWindow"], point: CGPoint(x: 400, y: 112), windowFrame: frame))
        XCTAssertFalse(TitleBarRules.isTitleBar(roles: ["AXWebArea", "AXScrollArea", "AXWindow"], point: CGPoint(x: 400, y: 105), windowFrame: frame))
    }

    func testPointOutsideTheWindowIsNotATitleBar() {
        XCTAssertFalse(TitleBarRules.isTitleBar(roles: ["AXWindow"], point: CGPoint(x: 50, y: 110), windowFrame: frame))
    }

    func testVisibleTitleBarIsTheAXTitleBarChild() {
        XCTAssertTrue(TitleBarRules.hasVisibleTitleBar(childRoles: ["AXTitleBar", "AXGroup"]))
        XCTAssertFalse(TitleBarRules.hasVisibleTitleBar(childRoles: ["AXButton", "AXGroup"]), "SwiftUI: Ampeln ohne AXTitleBar")
    }

    func testLightCentersAreRelativeToTheWindowOrigin() {
        let w = CGRect(x: 100, y: 200, width: 800, height: 600)
        let c = CGRect(x: 120, y: 208, width: 12, height: 12)
        let m = CGRect(x: 140, y: 208, width: 12, height: 12)
        let z = CGRect(x: 160, y: 208, width: 12, height: 12)
        let pts = TitleBarRules.lightCenters(close: c, minimize: m, zoom: z, window: w)
        XCTAssertEqual(pts.count, 3)
        XCTAssertEqual(pts[0].0, 26, accuracy: 0.01)
        XCTAssertEqual(pts[0].1, 14, accuracy: 0.01)
        XCTAssertEqual(pts[2].0, 66, accuracy: 0.01)
        XCTAssertEqual(TitleBarRules.lightCenters(close: c, minimize: nil, zoom: z, window: w).count, 0)
    }
}

final class ReturnPlacementTests: XCTestCase {
    let screen = CGRect(x: 0, y: 0, width: 1800, height: 1169)

    func testTitleBarEndsUpUnderTheMouse() {
        let o = ReturnPlacement.origin(mouse: CGPoint(x: 900, y: 400), size: CGSize(width: 800, height: 600), screen: screen, topInset: 38)
        XCTAssertEqual(o, CGPoint(x: 500, y: 386)) // Mitte der Titelleiste (14 pt) unter dem Zeiger
    }

    func testClampsToTheVisibleScreen() {
        let size = CGSize(width: 800, height: 600)
        XCTAssertEqual(ReturnPlacement.origin(mouse: CGPoint(x: 20, y: 5), size: size, screen: screen, topInset: 38), CGPoint(x: 0, y: 38))
        XCTAssertEqual(ReturnPlacement.origin(mouse: CGPoint(x: 1790, y: 1160), size: size, screen: screen, topInset: 38),
                       CGPoint(x: 1000, y: 1169 - 28 - 8))
    }

    func testSecondDisplayWithNegativeOrigin() {
        let left = CGRect(x: -1440, y: 100, width: 1440, height: 900)
        let o = ReturnPlacement.origin(mouse: CGPoint(x: -700, y: 500), size: CGSize(width: 600, height: 400), screen: left, topInset: 0)
        XCTAssertEqual(o, CGPoint(x: -1000, y: 486))
    }

    func testWindowLargerThanTheScreenStaysAtTheEdge() {
        let o = ReturnPlacement.origin(mouse: CGPoint(x: 500, y: 300), size: CGSize(width: 2500, height: 1500), screen: screen, topInset: 38)
        XCTAssertEqual(o.x, 0)
    }
}

final class StuckInputTests: XCTestCase {
    struct Held: HeldInputSource {
        var keys: Set<UInt16> = []
        var buttons: Set<MouseButton> = []
        func isKeyDown(_ code: UInt16) -> Bool { keys.contains(code) }
        func isButtonDown(_ b: MouseButton) -> Bool { buttons.contains(b) }
    }

    func testReleasesHeldKeysModifiersAndButtonsWithFlagsZero() {
        let sink = RecordingSink()
        let n = StuckInput.releaseAll(source: Held(keys: [0x37, 0x08, 0x38], buttons: [.left]), sink: sink,
                                      at: CGPoint(x: 5, y: 6))
        XCTAssertEqual(n, 4)
        XCTAssertEqual(sink.events, [
            .key(0x08, false, 0), .key(0x37, false, 0), .key(0x38, false, 0),
            .mouse(.up, CGPoint(x: 5, y: 6), .left, 1, 0),
        ])
    }

    func testNothingHeldMeansNothingSent() {
        let sink = RecordingSink()
        XCTAssertEqual(StuckInput.releaseAll(source: Held(), sink: sink, at: .zero), 0)
        XCTAssertTrue(sink.events.isEmpty)
    }

    func testCapsLockStateIsNotAHeldKey() {
        let sink = RecordingSink()
        XCTAssertEqual(StuckInput.releaseAll(source: Held(keys: [0x39]), sink: sink, at: .zero), 0)
    }
}
