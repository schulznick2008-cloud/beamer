import CoreMedia
import CoreVideo
import XCTest
@testable import TeleportCore

/// Attrappe: ein Encoder, der pro Frame `delay` Sekunden braucht und Frames nacheinander abarbeitet.
/// Zeichnet auf, wie viele Frames gleichzeitig darin stecken.
final class SlowEncoder: FrameEncoder {
    let width: Int
    let height: Int
    private let delay: TimeInterval
    private let onDone: (EncodedFrame?) -> Void
    private let queue = DispatchQueue(label: "test.slowencoder")
    private let lock = NSLock()
    private var concurrent = 0
    private(set) var maxConcurrent = 0
    private(set) var submitted = 0
    private(set) var frameRates: [Int] = []
    private var count = 0
    private var busySum = 0.0
    /// Tatsaechliche mittlere Bearbeitungszeit je Frame in ms (der Scheduler schlaeft auf kleinen Runnern laenger).
    var serviceMS: Double { lock.lock(); defer { lock.unlock() }; return count > 0 ? busySum / Double(max(1, doneCount)) : 0 }
    private var doneCount = 0

    init(width: Int, height: Int, delay: TimeInterval, onDone: @escaping (EncodedFrame?) -> Void) {
        self.width = width; self.height = height; self.delay = delay; self.onDone = onDone
    }

    func encode(_ pixelBuffer: CVPixelBuffer, hostTime: CMTime, forceKeyframe: Bool) {
        let started = DispatchTime.now().uptimeNanoseconds
        lock.lock()
        submitted += 1
        concurrent += 1
        maxConcurrent = max(maxConcurrent, concurrent)
        let key = forceKeyframe || count == 0
        count += 1
        lock.unlock()
        let pts = UInt64(max(0, hostTime.seconds * 1_000_000))
        queue.async { [self] in
            let b0 = DispatchTime.now().uptimeNanoseconds
            Thread.sleep(forTimeInterval: delay)
            lock.lock()
            concurrent -= 1
            busySum += Double(DispatchTime.now().uptimeNanoseconds - b0) / 1e6
            doneCount += 1
            lock.unlock()
            let ms = Double(DispatchTime.now().uptimeNanoseconds - started) / 1e6
            let nal: [UInt8] = key ? [0, 0, 0, 1, 0x65, 1, 2, 3] : [0, 0, 0, 1, 0x41, 1, 2, 3]
            onDone(EncodedFrame(annexB: nal, isKeyframe: key, ptsUS: pts, encodeMS: ms))
        }
    }

    func setBitrate(kbps: Int) {}
    func setFrameRate(_ fps: Int) { lock.lock(); frameRates.append(fps); lock.unlock() }
    func flush() { queue.sync {} }
    func invalidate() {}
}

/// Bildquelle, die der Test von Hand fuettert (stehendes Bild).
final class ManualSource: FrameSource {
    private var onFrame: ((CVPixelBuffer, CMTime) -> Void)?
    var state: String { "ok" }
    var pixelSize: (width: Int, height: Int) { (320, 240) }
    var scale: Double { 1 }
    func start(onFrame: @escaping (CVPixelBuffer, CMTime) -> Void, onStop: @escaping (String) -> Void) throws {
        self.onFrame = onFrame
    }
    func stop() { onFrame = nil }
    func push() {
        var pb: CVPixelBuffer?
        CVPixelBufferCreate(nil, 320, 240, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, nil, &pb)
        if let pb = pb { onFrame?(pb, HostClock.nowTime()) }
    }
}

/// Attrappe, die die ersten Frames verschluckt (nie `onDone`), wie ein haengender VideoToolbox-Encoder.
final class SwallowingEncoder: FrameEncoder {
    let width: Int
    let height: Int
    private let swallow: Bool
    private let onDone: (EncodedFrame?) -> Void
    private var n = 0
    init(width: Int, height: Int, swallow: Bool, onDone: @escaping (EncodedFrame?) -> Void) {
        self.width = width; self.height = height; self.swallow = swallow; self.onDone = onDone
    }
    func encode(_ pixelBuffer: CVPixelBuffer, hostTime: CMTime, forceKeyframe: Bool) {
        if swallow { return }
        n += 1
        let pts = UInt64(max(0, hostTime.seconds * 1_000_000))
        onDone(EncodedFrame(annexB: [0, 0, 0, 1, 0x65, 1], isKeyframe: true, ptsUS: pts, encodeMS: 1))
    }
    func setBitrate(kbps: Int) {}
    func setFrameRate(_ fps: Int) {}
    func flush() {}
    func invalidate() {}
}

final class PipelineTests: XCTestCase {
    private final class Rx {
        private let lock = NSLock()
        private(set) var lat: [Double] = []
        private(set) var keyTimes: [UInt64] = []
        private(set) var frames = 0
        func add(_ f: ReceivedFrame) {
            let now = HostClock.nowUS()
            lock.lock()
            frames += 1
            lat.append(Double(Int64(now) - Int64(f.ptsUS)) / 1000)
            if f.fflags & 1 == 1 { keyTimes.append(now) }
            lock.unlock()
        }
        var snapshot: (frames: Int, lat: [Double], keys: [UInt64]) {
            lock.lock(); defer { lock.unlock() }
            return (frames, lat, keyTimes)
        }
    }

    private final class Events {
        private let lock = NSLock()
        private var all: [JSON] = []
        func add(_ e: JSON) { lock.lock(); all.append(e); lock.unlock() }
        func stats() -> [JSON] { lock.lock(); defer { lock.unlock() }; return all.filter { $0["ev"] as? String == "stats" } }
    }

    private struct Rig {
        let tx: Transport
        let rx: Transport
        let received: Rx
        let events: Events
        let pipe: SenderPipeline
        let encoder: () -> SlowEncoder?
    }

    private func rig(delay: TimeInterval, fps: Int, source: FrameSource, lowPower: Bool = false) throws -> Rig {
        let rx = try Transport(localPort: 0)
        try rx.addStream(id: 1, role: .receiver, keys: .testOnly, maxKbps: 100_000)
        let got = Rx()
        rx.onFrame = { got.add($0) }
        let tx = try Transport(localPort: 0)
        try tx.setPeer(host: "127.0.0.1", port: rx.localPort)
        try tx.addStream(id: 1, role: .sender, keys: .testOnly, maxKbps: 100_000)
        let events = Events()
        var made: SlowEncoder?
        let pipe = SenderPipeline(
            tid: 1, codec: .h264, fps: fps, bitrateKbps: 100_000, transport: tx, source: source,
            makeEncoder: { w, h, _, _, _, done in
                let e = SlowEncoder(width: w, height: h, delay: delay, onDone: done)
                made = e
                return e
            },
            lowPower: { lowPower },
            emit: { events.add($0) })
        tx.onControl = { [weak pipe] _, type in if type == 5 { pipe?.requestKeyframe() } }
        tx.onStats = { [weak pipe] _, j in pipe?.updateTransportStats(j) }
        try pipe.start()
        return Rig(tx: tx, rx: rx, received: got, events: events, pipe: pipe, encoder: { made })
    }

    private func finish(_ r: Rig) {
        r.pipe.stop()
        r.tx.close()
        r.rx.close()
    }

    func testSlowEncoderBacklogIsBoundedAndLatencyStaysUnderThreeEncoderFrames() throws {
        let delay = 0.05 // 50 ms je Frame bei 60 fps Eingang: der Encoder schafft nur 20 fps
        let r = try rig(delay: delay, fps: 60, source: SynthSource(width: 320, height: 240, fps: 60))
        Thread.sleep(forTimeInterval: 4)
        let enc = r.encoder()
        let snap = r.received.snapshot
        finish(r)

        XCTAssertLessThanOrEqual(enc?.maxConcurrent ?? 99, 2, "hoechstens 2 Frames gleichzeitig im Encoder")
        XCTAssertGreaterThan(snap.frames, 30, "Frames kommen an: \(snap.frames)")
        let lat = snap.lat.sorted()
        let service = enc?.serviceMS ?? delay * 1000 // tatsaechliche Bearbeitungszeit (>= delay)
        // Ein serieller Attrappen-Encoder braucht mit einem Frame davor in der Schlange 2 Bearbeitungszeiten plus
        // Alter des Bilds und Scheduler. Auf langsamen Runnern wird alles um denselben Faktor laenger, deshalb
        // wird an der gemessenen Bearbeitungszeit gemessen, nicht an der eingestellten.
        let median = lat[lat.count / 2]
        let p95 = lat[Int(Double(lat.count - 1) * 0.95)]
        XCTAssertLessThan(median, 3 * service, "Latenz Capture bis Ausgabe, Median \(median) ms bei \(service) ms je Frame")
        XCTAssertLessThan(p95, 3.6 * service, "Latenz p95 \(p95) ms bei \(service) ms je Frame")
        XCTAssertLessThan(lat.last ?? 0, 4.4 * service, "Latenz max \(lat.last ?? 0) ms bei \(service) ms je Frame")
        // Kein Wachstum: die letzten Frames sind nicht spaeter dran als die ersten (ohne Begrenzung wuchse die Latenz).
        let early = snap.lat.dropFirst(5).prefix(10).reduce(0, +) / 10
        let late = snap.lat.suffix(10).reduce(0, +) / 10
        XCTAssertLessThan(late, early + service, "Latenz waechst: \(early) -> \(late) ms")
        // Hat die Quelle mehr Frames geliefert, als der Encoder schaffte, muessen die ueberzaehligen verworfen
        // worden sein. (Auf einem sehr langsamen Runner liefert schon die Quelle weniger als der Encoder frisst.)
        func sum(_ k: String) -> Int { r.events.stats().compactMap { ($0[k] as? NSNumber)?.intValue }.reduce(0, +) }
        let drops = sum("backlog_drops"), captured = sum("capture_fps"), encoded = sum("encode_fps")
        if captured > encoded + 6 {
            XCTAssertGreaterThan(drops, 0, "ueberzaehlige Capture-Frames werden verworfen (\(captured) erfasst, \(encoded) kodiert)")
        }
    }

    func testKeyframeComesImmediatelyEvenWithAStillPicture() throws {
        let src = ManualSource()
        let r = try rig(delay: 0.002, fps: 30, source: src)
        src.push() // ein einziges Bild, danach steht die Quelle
        let t0 = Date()
        while r.received.snapshot.frames < 1 && Date().timeIntervalSince(t0) < 5 { Thread.sleep(forTimeInterval: 0.005) }
        XCTAssertEqual(r.received.snapshot.frames, 1)
        Thread.sleep(forTimeInterval: 0.3)

        // 1. send.keyframe / Engine-Pfad
        var before = r.received.snapshot.keys.count
        var t = HostClock.nowUS()
        r.pipe.requestKeyframe()
        var ok = wait(1) { r.received.snapshot.keys.count > before }
        XCTAssertTrue(ok, "Keyframe nach Anfrage")
        var delta = Double(r.received.snapshot.keys.last! &- t) / 1000
        XCTAssertLessThan(delta, 100, "Keyframe bei stehendem Bild nach \(delta) ms")

        // 2. keyframe_req vom Empfaenger ueber den Transport
        Thread.sleep(forTimeInterval: 0.4)
        before = r.received.snapshot.keys.count
        t = HostClock.nowUS()
        r.rx.requestKeyframe(stream: 1)
        ok = wait(1) { r.received.snapshot.keys.count > before }
        XCTAssertTrue(ok, "Keyframe nach keyframe_req")
        delta = Double(r.received.snapshot.keys.last! &- t) / 1000
        XCTAssertLessThan(delta, 100, "Keyframe nach keyframe_req bei stehendem Bild nach \(delta) ms")
        finish(r)
    }

    func testSlowEncoderIsReportedAndFrameRateReduced() throws {
        let r = try rig(delay: 0.04, fps: 60, source: SynthSource(width: 320, height: 240, fps: 60), lowPower: true)
        Thread.sleep(forTimeInterval: 6.5)
        let stats = r.events.stats()
        let enc = r.encoder()
        finish(r)
        XCTAssertGreaterThanOrEqual(stats.count, 4)
        XCTAssertTrue(stats.contains { ($0["encoder_slow"] as? Bool) == true }, "encoder_slow wird gemeldet")
        let last = stats.last!
        let eff = (last["effective_fps"] as? NSNumber)?.intValue ?? 0
        XCTAssertLessThan(eff, 60, "effektive Bildrate gesenkt")
        XCTAssertGreaterThanOrEqual(eff, SenderPipeline.minEffectiveFps, "nicht unter die Untergrenze")
        XCTAssertEqual(last["low_power"] as? Bool, true)
        XCTAssertFalse(enc?.frameRates.isEmpty ?? true, "Encoder bekommt die neue Bildrate")
    }

    func testFastEncoderIsNotReportedSlow() throws {
        let r = try rig(delay: 0.002, fps: 60, source: SynthSource(width: 320, height: 240, fps: 60))
        Thread.sleep(forTimeInterval: 3.5)
        let stats = r.events.stats()
        finish(r)
        XCTAssertGreaterThanOrEqual(stats.count, 2)
        XCTAssertTrue(stats.allSatisfy { ($0["encoder_slow"] as? Bool) == false })
        XCTAssertEqual((stats.last?["effective_fps"] as? NSNumber)?.intValue, 60)
        XCTAssertEqual(stats.last?["low_power"] as? Bool, false)
    }

    func testPipelineRecoversWhenTheEncoderSwallowsFrames() throws {
        let rx = try Transport(localPort: 0)
        try rx.addStream(id: 1, role: .receiver, keys: .testOnly, maxKbps: 100_000)
        let got = Rx()
        rx.onFrame = { got.add($0) }
        let tx = try Transport(localPort: 0)
        try tx.setPeer(host: "127.0.0.1", port: rx.localPort)
        try tx.addStream(id: 1, role: .sender, keys: .testOnly, maxKbps: 100_000)
        var created = 0
        let createdLock = NSLock()
        let pipe = SenderPipeline(
            tid: 1, codec: .h264, fps: 60, bitrateKbps: 10_000, transport: tx,
            source: SynthSource(width: 320, height: 240, fps: 60),
            makeEncoder: { w, h, _, _, _, done in
                createdLock.lock(); created += 1; let n = created; createdLock.unlock()
                return SwallowingEncoder(width: w, height: h, swallow: n == 1, onDone: done) // der erste haengt
            },
            emit: { _ in })
        try pipe.start()
        let ok = wait(6) { got.snapshot.frames > 10 }
        pipe.stop(); tx.close(); rx.close()
        XCTAssertTrue(ok, "der Strom laeuft nach dem Haenger weiter: \(got.snapshot.frames) Frames")
        XCTAssertGreaterThanOrEqual(pipe.encoderResets, 1)
        XCTAssertGreaterThanOrEqual(created, 2, "Encoder wurde neu angelegt")
    }

    private func wait(_ secs: Double, _ cond: () -> Bool) -> Bool {
        let end = Date().addingTimeInterval(secs)
        while Date() < end { if cond() { return true }; Thread.sleep(forTimeInterval: 0.002) }
        return cond()
    }
}
