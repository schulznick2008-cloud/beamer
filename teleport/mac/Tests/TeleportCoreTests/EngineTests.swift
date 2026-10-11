import XCTest
@testable import TeleportCore

/// Ende-zu-Ende mit echtem VideoToolbox-Encoder, echtem Transport und der Muster-Quelle.
/// Ohne Hardware-Encoder (z. B. in virtuellen CI-Maschinen) werden die Tests uebersprungen.
final class EngineTests: XCTestCase {
    private func requireHardwareEncoder() throws {
        do {
            _ = try VideoEncoder(width: 320, height: 240, codec: .h264, fps: 30, bitrateKbps: 2000) { _ in }
        } catch {
            throw XCTSkip("kein Hardware-Encoder verfuegbar: \(error)")
        }
    }

    private func makeEngine() -> (SenderEngine, EventBox) {
        let box = EventBox()
        let engine = SenderEngine { box.add($0) }
        engine.makeSource = { _, fps in SynthSource(width: 320, height: 240, fps: fps) }
        return (engine, box)
    }

    final class EventBox {
        private let lock = NSLock()
        private var events: [JSON] = []
        func add(_ e: JSON) { lock.lock(); events.append(e); lock.unlock() }
        func all() -> [JSON] { lock.lock(); defer { lock.unlock() }; return events }
    }

    final class FrameCounter {
        private let lock = NSLock()
        private(set) var count = 0
        private(set) var keyframes = 0
        func add(_ f: ReceivedFrame) {
            lock.lock(); count += 1; if f.fflags & 1 == 1 { keyframes += 1 }; lock.unlock()
        }
        var value: Int { lock.lock(); defer { lock.unlock() }; return count }
    }

    private func waitFor(_ secs: Double, _ cond: () -> Bool) -> Bool {
        let end = Date().addingTimeInterval(secs)
        while Date() < end { if cond() { return true }; Thread.sleep(forTimeInterval: 0.02) }
        return cond()
    }

    private func receiver() throws -> (Transport, FrameCounter) {
        let rx = try Transport(localPort: 0)
        try rx.addStream(id: 1, role: .receiver, keys: .testOnly, maxKbps: 100_000)
        let c = FrameCounter()
        rx.onFrame = { c.add($0) }
        return (rx, c)
    }

    private func startParams(codec: Codec = .h264, kbps: UInt32 = 8000) -> SendParams {
        SendParams(tid: 1, mode: .window(1), codec: codec, maxFps: 30, bitrateKbps: kbps, keys: .testOnly)
    }

    func testStreamsFramesAndStopsFast() throws {
        try requireHardwareEncoder()
        let (engine, box) = makeEngine()
        let (rx, counter) = try receiver()
        _ = try engine.transportOpen(localPort: 0, peerHost: "127.0.0.1", peerPort: rx.localPort)
        let info = try engine.sendStart(startParams())
        XCTAssertEqual(info.width, 320)
        XCTAssertEqual(info.height, 240)
        XCTAssertTrue(waitFor(8) { counter.value >= 20 }, "Frames kommen an: \(counter.value)")
        XCTAssertGreaterThanOrEqual(counter.keyframes, 1)
        XCTAssertTrue(waitFor(4) { box.all().contains { $0["ev"] as? String == "stats" } }, "stats-Ereignis jede Sekunde")
        let stats = box.all().last { $0["ev"] as? String == "stats" }
        XCTAssertEqual(stats?.int("tid"), 1)
        for k in ["fps", "kbps", "rtt_ms", "loss_pct", "latency_ms", "state"] { XCTAssertNotNil(stats?[k], k) }

        XCTAssertThrowsError(try engine.sendStart(startParams())) { XCTAssertEqual(($0 as? IPCError)?.code, .busy) }
        let t0 = Date()
        try engine.sendStop(tid: 1, restore: false)
        engine.shutdown()
        XCTAssertLessThan(Date().timeIntervalSince(t0), 2.0, "send.stop und Aufraeumen unter 2 s")
        XCTAssertThrowsError(try engine.sendStop(tid: 1, restore: false)) { XCTAssertEqual(($0 as? IPCError)?.code, .notFound) }
        rx.close()
    }

    func testKeyframeRequestsProduceKeyframes() throws {
        try requireHardwareEncoder()
        let (engine, _) = makeEngine()
        let (rx, counter) = try receiver()
        _ = try engine.transportOpen(localPort: 0, peerHost: "127.0.0.1", peerPort: rx.localPort)
        _ = try engine.sendStart(startParams())
        XCTAssertTrue(waitFor(8) { counter.value >= 10 })
        let before = counter.keyframes
        try engine.sendKeyframe(tid: 1)
        XCTAssertTrue(waitFor(3) { counter.keyframes > before }, "send.keyframe liefert einen Keyframe")
        // keyframe_req vom Empfaenger
        let before2 = counter.keyframes
        rx.requestKeyframe(stream: 1)
        XCTAssertTrue(waitFor(3) { counter.keyframes > before2 }, "keyframe_req liefert einen Keyframe")
        engine.shutdown()
        rx.close()
    }

    func testTransportPeerWorksWhileStreaming() throws {
        try requireHardwareEncoder()
        let (engine, _) = makeEngine()
        let (rx1, c1) = try receiver()
        let (rx2, c2) = try receiver()
        _ = try engine.transportOpen(localPort: 0, peerHost: "127.0.0.1", peerPort: rx1.localPort)
        _ = try engine.sendStart(startParams())
        XCTAssertTrue(waitFor(8) { c1.value >= 10 })
        XCTAssertEqual(c2.value, 0)

        // Pfadwechsel mitten im Strom (SCHNITTSTELLEN §7): neue Adresse, Stream laeuft weiter. Der alte
        // Pfad ist nach dem Wechsel tot (sonst wuerde das Roaming des Transports dem alten Absender folgen).
        rx1.close()
        try engine.transportPeer(host: "127.0.0.1", port: rx2.localPort)
        try engine.sendKeyframe(tid: 1)
        XCTAssertTrue(waitFor(8) { c2.value >= 10 }, "Frames kommen am neuen Ziel an: \(c2.value)")
        XCTAssertThrowsError(try engine.transportPeer(host: "nicht-aufloesbar.invalid", port: 5)) {
            XCTAssertEqual(($0 as? IPCError)?.code, .badRequest)
        }
        engine.shutdown()
        rx2.close()
    }

    func testSendBitrateChangesTheCeiling() throws {
        try requireHardwareEncoder()
        let (engine, _) = makeEngine()
        let (rx, counter) = try receiver()
        _ = try engine.transportOpen(localPort: 0, peerHost: "127.0.0.1", peerPort: rx.localPort)
        _ = try engine.sendStart(startParams(kbps: 8000))
        XCTAssertTrue(waitFor(8) { counter.value >= 5 })
        XCTAssertEqual(engine.targetKbps(tid: 1), 8000)
        try engine.sendBitrate(tid: 1, kbps: 3000)
        XCTAssertEqual(engine.targetKbps(tid: 1), 3000, "Senkung: hoechstens die neue Obergrenze")
        try engine.sendBitrate(tid: 1, kbps: 150_000)
        XCTAssertEqual(engine.targetKbps(tid: 1), 75_000, "Erhoehung: Zielbitrate springt auf 50 % der neuen Obergrenze")
        XCTAssertThrowsError(try engine.sendBitrate(tid: 7, kbps: 1000)) { XCTAssertEqual(($0 as? IPCError)?.code, .notFound) }
        engine.shutdown()
        rx.close()
    }

    func testStartNeedsOpenTransportAndReportsSourceErrors() throws {
        let (engine, _) = makeEngine()
        XCTAssertThrowsError(try engine.sendStart(startParams())) { XCTAssertEqual(($0 as? IPCError)?.code, .badRequest) }
        _ = try engine.transportOpen(localPort: 0, peerHost: nil, peerPort: 0)
        engine.makeSource = { _, _ in throw IPCError(.permission, "Bildschirmaufnahme nicht freigegeben") }
        XCTAssertThrowsError(try engine.sendStart(startParams())) { XCTAssertEqual(($0 as? IPCError)?.code, .permission) }
        XCTAssertThrowsError(try engine.transportOpen(localPort: 0, peerHost: nil, peerPort: 0)) {
            XCTAssertEqual(($0 as? IPCError)?.code, .busy)
        }
        engine.shutdown()
    }
}

final class EncoderBehaviourTests: XCTestCase {
    func testFrameRateAndBitrateChangesKeepTheEncoderRunning() throws {
        let done = NSLock()
        var outputs = 0
        var nils = 0
        let enc: VideoEncoder
        do {
            enc = try VideoEncoder(width: 1280, height: 720, codec: .hevc, fps: 60, bitrateKbps: 8000) { f in
                done.lock(); if f == nil { nils += 1 } else { outputs += 1 }; done.unlock()
            }
        } catch {
            throw XCTSkip("kein Hardware-Encoder verfuegbar: \(error)")
        }
        let src = SynthSource(width: 1280, height: 720, fps: 60)
        var sent = 0
        let sentLock = NSLock()
        try src.start(onFrame: { pb, t in
            sentLock.lock(); sent += 1; sentLock.unlock()
            enc.encode(pb, hostTime: t, forceKeyframe: false)
        }, onStop: { _ in })
        func counts() -> (Int, Int) { done.lock(); defer { done.unlock() }; return (outputs, nils) }
        /// Wartet, bis seit `base` mindestens `n` Frames dazugekommen sind (Polling, die CI-Maschinen sind langsam).
        func waitForMore(_ n: Int, since base: Int) -> Bool {
            let end = Date().addingTimeInterval(8)
            while Date() < end { if counts().0 - base >= n { return true }; Thread.sleep(forTimeInterval: 0.02) }
            return counts().0 - base >= n
        }
        XCTAssertTrue(waitForMore(10, since: 0), "Encoder liefert Frames")
        let a = counts().0
        enc.setFrameRate(51)
        XCTAssertTrue(waitForMore(10, since: a), "nach setFrameRate laeuft der Encoder weiter")
        let b = counts().0
        enc.setBitrate(kbps: 3000)
        enc.setFrameRate(24)
        XCTAssertTrue(waitForMore(10, since: b), "nach setBitrate/setFrameRate laeuft der Encoder weiter")
        src.stop()
        enc.flush()
        enc.invalidate()
    }
}
