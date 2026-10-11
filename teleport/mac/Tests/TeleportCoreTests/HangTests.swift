import XCTest
@testable import TeleportCore

/// Haengende ScreenCaptureKit-Aufrufe duerfen weder den IPC-Thread noch den Strom festhalten (C02b).
final class HangTests: XCTestCase {
    /// Je Teleport eigene Schluessel (der Transport lehnt Wiederverwendung eines Senderschluessels ab).
    private func params(_ tid: UInt32 = 1, salt: UInt8 = 0) -> SendParams {
        var k = StreamKeys.testOnly
        k.s2rKey = [UInt8](repeating: UInt8(tid) &+ salt &+ 0x30, count: 32)
        return SendParams(tid: tid, mode: .window(1), codec: .h264, maxFps: 30, bitrateKbps: 5000, keys: k)
    }

    func testSendStopReturnsInUnderTwoSecondsWhenCaptureStopNeverReturns() throws {
        let engine = SenderEngine { _ in }
        engine.makeSource = { _, _ in HangingSource(mode: .hangStop) }
        _ = try engine.transportOpen(localPort: 0, peerHost: nil, peerPort: 0)
        _ = try engine.sendStart(params())
        let t0 = Date()
        try engine.sendStop(tid: 1, restore: false)
        XCTAssertLessThan(Date().timeIntervalSince(t0), 2.0)
        let t1 = Date()
        engine.shutdown()
        XCTAssertLessThan(Date().timeIntervalSince(t1), 2.0)
    }

    func testShutdownWithSeveralHangingStreamsIsBoundedNotSequential() throws {
        let engine = SenderEngine { _ in }
        engine.makeSource = { _, _ in HangingSource(mode: .hangStop) }
        _ = try engine.transportOpen(localPort: 0, peerHost: nil, peerPort: 0)
        for tid in 1...3 { _ = try engine.sendStart(params(UInt32(tid))) }
        let t0 = Date()
        engine.shutdown()
        XCTAssertLessThan(Date().timeIntervalSince(t0), 2.0, "drei haengende Strome nacheinander wuerden 3 x warten")
    }

    func testSendStartFailsWithBusyWhenTheSourceNeverStarts() throws {
        let engine = SenderEngine { _ in }
        engine.startTimeout = 0.5
        engine.makeSource = { _, _ in HangingSource(mode: .hangStart) }
        _ = try engine.transportOpen(localPort: 0, peerHost: nil, peerPort: 0)
        let t0 = Date()
        XCTAssertThrowsError(try engine.sendStart(params())) { XCTAssertEqual(($0 as? IPCError)?.code, .busy) }
        XCTAssertLessThan(Date().timeIntervalSince(t0), 2.0)
        // Die tid ist wieder frei, der Helfer arbeitet weiter.
        engine.makeSource = { _, _ in ManualSource() }
        XCTAssertNoThrow(try engine.sendStart(params(1, salt: 9)), "neue Schluessel nach dem Fehlschlag")
        engine.shutdown()
    }

    func testAHangingRefreshDoesNotStallStatsOrStop() throws {
        let rx = try Transport(localPort: 0)
        let tx = try Transport(localPort: 0)
        try tx.addStream(id: 1, role: .sender, keys: .testOnly, maxKbps: 10_000)
        try tx.setPeer(host: "127.0.0.1", port: rx.localPort)
        let lock = NSLock()
        var stats = 0
        let pipe = SenderPipeline(tid: 1, codec: .h264, fps: 30, bitrateKbps: 5000, transport: tx,
                                  source: HangingSource(mode: .hangRefresh)) { e in
            if e["ev"] as? String == "stats" { lock.lock(); stats += 1; lock.unlock() }
        }
        try pipe.start()
        Thread.sleep(forTimeInterval: 3.5)
        lock.lock(); let n = stats; lock.unlock()
        XCTAssertGreaterThanOrEqual(n, 2, "stats kommen trotz haengender Zustandsabfrage (\(n))")
        let t0 = Date()
        pipe.stop()
        XCTAssertLessThan(Date().timeIntervalSince(t0), 2.0)
        tx.close(); rx.close()
    }
}
