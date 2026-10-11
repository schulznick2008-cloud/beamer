import CoreImage
import CoreVideo
import Foundation
import ImageIO
import UniformTypeIdentifiers

private func percentile(_ v: [Double], _ p: Double) -> Double {
    guard !v.isEmpty else { return .nan }
    let s = v.sorted()
    return s[min(s.count - 1, Int((Double(s.count - 1) * p).rounded()))]
}

private func fmt(_ x: Double) -> String { x.isNaN ? "n/a" : String(format: "%.1f", x) }

/// Stoppsignal fuer die Selbsttests (SIGINT/SIGTERM).
public enum StopSignal {
    nonisolated(unsafe) static var flag = false
    public static func install() {
        signal(SIGINT) { _ in StopSignal.flag = true }
        signal(SIGTERM) { _ in StopSignal.flag = true }
    }
    public static var requested: Bool { flag }
}

/// `BeamerTeleport --selftest-recv <port>`: Empfaenger ohne Anzeige. Dekodiert mit VideoToolbox und
/// protokolliert je Sekunde fps, Latenz (Capture bis dekodiert, gleiche Host-Uhr), Verlust und CPU.
/// Nur sinnvoll, wenn Sender und Empfaenger auf demselben Mac laufen (gemeinsame Uhr).
public final class SelfTestReceiver {
    private let port: UInt16
    private let tid: UInt32
    private let seconds: Int
    private let out: (String) -> Void
    /// Wenn gesetzt, wird alle 2 s das zuletzt dekodierte Bild als PNG dorthin geschrieben (Sichtpruefung des Streams).
    public var dumpDir: String?
    private var latestImage: CVPixelBuffer?
    private let decodeQueue = DispatchQueue(label: "teleport.selftest.decode")
    private let lock = NSLock()
    private var decoder: VideoDecoder?
    private var decoderCodec: Codec?

    private var latWindow: [Double] = []
    private var latAll: [(sec: Int, ms: Double)] = []
    private var currentSec = 0
    private var decodedWindow = 0, decodedTotal = 0
    private var bytesWindow = 0
    private var lastFrameId: UInt32?
    private var lostWindow = 0, lostTotal = 0
    private var decodeErrors = 0
    private var cpuSamples: [Double] = []
    private var kbpsSamples: [Double] = []

    public init(port: UInt16, tid: UInt32 = 1, seconds: Int = 0, out: @escaping (String) -> Void) {
        self.port = port
        self.tid = tid
        self.seconds = seconds
        self.out = out
    }

    public func run() throws -> JSON {
        let t = try Transport(localPort: port)
        try t.addStream(id: tid, role: .receiver, keys: .testOnly, maxKbps: 100_000)
        out("selftest-recv: lausche auf UDP-Port \(t.localPort), tid \(tid) (Test-Schluessel)")
        t.onFrame = { [weak self] f in self?.onFrame(f) }
        var cpu = CPUMeter()
        var tick = 0
        while !StopSignal.requested {
            Thread.sleep(forTimeInterval: 1)
            tick += 1
            report(t: tick, cpu: cpu.sample())
            if tick % 2 == 0 { dumpLatest(tick) }
            if seconds > 0 && tick >= seconds { break }
        }
        t.close()
        return summary()
    }

    private func onFrame(_ f: ReceivedFrame) {
        guard f.kind == 1 else { return }
        decodeQueue.async { [weak self] in self?.decode(f) }
    }

    private func decode(_ f: ReceivedFrame) {
        let codec: Codec = ((f.fflags >> 1) & 7) == 2 ? .hevc : .h264
        lock.lock()
        if let last = lastFrameId, f.frameId != last &+ 1 { lostWindow += Int(f.frameId &- last &- 1); lostTotal += Int(f.frameId &- last &- 1) }
        lastFrameId = f.frameId
        bytesWindow += f.data.count
        if decoder == nil || decoderCodec != codec {
            decoder = VideoDecoder(codec: codec, onImage: { [weak self] px in
                self?.lock.lock(); self?.latestImage = px; self?.lock.unlock()
            }) { [weak self] pts in self?.decoded(pts) }
            decoderCodec = codec
        }
        let dec = decoder!
        lock.unlock()
        do {
            try dec.decode(annexB: f.data, ptsUS: f.ptsUS)
        } catch {
            lock.lock(); decodeErrors += 1; lock.unlock()
        }
    }

    private func dumpLatest(_ tick: Int) {
        guard let dir = dumpDir else { return }
        lock.lock(); let img = latestImage; lock.unlock()
        guard let px = img else { return }
        let ci = CIImage(cvPixelBuffer: px)
        guard let cg = CIContext().createCGImage(ci, from: ci.extent) else { return }
        let url = URL(fileURLWithPath: dir).appendingPathComponent(String(format: "stream_%03d.png", tick))
        guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else { return }
        CGImageDestinationAddImage(dest, cg, nil)
        CGImageDestinationFinalize(dest)
    }

    private func decoded(_ ptsUS: UInt64) {
        let now = HostClock.nowUS()
        let ms = Double(Int64(now) - Int64(ptsUS)) / 1000.0
        lock.lock()
        latWindow.append(ms)
        latAll.append((currentSec, ms))
        decodedWindow += 1
        decodedTotal += 1
        lock.unlock()
    }

    private func report(t: Int, cpu: Double) {
        lock.lock()
        currentSec = t
        let lat = latWindow, n = decodedWindow, lost = lostWindow, bytes = bytesWindow, errs = decodeErrors
        latWindow = []; decodedWindow = 0; lostWindow = 0; bytesWindow = 0
        if t > 1 { cpuSamples.append(cpu); kbpsSamples.append(Double(bytes) * 8 / 1000) }
        lock.unlock()
        out("t=\(t)s fps=\(n) kbps=\(Int(Double(bytes) * 8 / 1000)) lat_ms median=\(fmt(percentile(lat, 0.5))) p95=\(fmt(percentile(lat, 0.95))) verloren=\(lost) dekodierfehler=\(errs) cpu_empfaenger=\(fmt(cpu))%")
    }

    private func summary() -> JSON {
        lock.lock(); defer { lock.unlock() }
        // Die ersten 3 s (Anlauf der Encoder-Session, erste grosse Keyframes) zaehlen nicht fuer Median und p95;
        // `latency_all_*` enthaelt alles.
        let all = latAll.map { $0.ms }
        let lat = latAll.filter { $0.sec > 3 }.map { $0.ms }
        return [
            "selftest": "recv", "decoded": decodedTotal, "lost": lostTotal, "decode_errors": decodeErrors,
            "latency_median_ms": percentile(lat, 0.5), "latency_p95_ms": percentile(lat, 0.95),
            "latency_max_ms": lat.max() ?? 0,
            "latency_all_median_ms": percentile(all, 0.5), "latency_all_p95_ms": percentile(all, 0.95),
            "kbps_avg": kbpsSamples.isEmpty ? 0 : kbpsSamples.reduce(0, +) / Double(kbpsSamples.count),
            "recv_cpu_pct_avg": cpuSamples.isEmpty ? 0 : cpuSamples.reduce(0, +) / Double(cpuSamples.count),
        ]
    }
}

/// `BeamerTeleport --selftest-synth host:port`: Sender mit Muster-Quelle ueber denselben Encoder- und
/// Transportweg, ohne Bildschirmaufnahme-Freigabe. Misst die CPU des Senders.
public final class SelfTestSynthSender {
    private let host: String
    private let port: UInt16
    private let width: Int, height: Int, fps: Int
    private let codec: Codec
    private let kbps: UInt32
    private let seconds: Int
    private let out: (String) -> Void

    public init(host: String, port: UInt16, width: Int, height: Int, fps: Int, codec: Codec, kbps: UInt32,
                seconds: Int, out: @escaping (String) -> Void) {
        self.host = host; self.port = port; self.width = width; self.height = height; self.fps = fps
        self.codec = codec; self.kbps = kbps; self.seconds = seconds; self.out = out
    }

    public func run() throws -> JSON {
        let t = try Transport(localPort: 0)
        try t.setPeer(host: host, port: port)
        try t.addStream(id: 1, role: .sender, keys: .testOnly, maxKbps: kbps)
        let src = SynthSource(width: width, height: height, fps: fps)
        let lock = NSLock()
        var cpuSamples: [Double] = [], kbpsSamples: [Double] = []
        var sec = 0
        let pipe = SenderPipeline(tid: 1, codec: codec, fps: fps, bitrateKbps: kbps, transport: t, source: src) { ev in
            guard ev["ev"] as? String == "stats" else {
                self.out("sender-ereignis: \(JSONLine.encode(ev))")
                return
            }
            lock.lock()
            sec += 1
            if sec > 1 {
                cpuSamples.append((ev["cpu_pct"] as? Double) ?? 0)
                kbpsSamples.append(((ev["kbps"] as? NSNumber)?.doubleValue) ?? 0)
            }
            lock.unlock()
            self.out("sender: fps=\(ev["fps"] ?? 0) kbps=\(ev["kbps"] ?? 0) encode_ms=\(ev["encode_ms"] ?? 0) cpu_sender=\(ev["cpu_pct"] ?? 0)% target=\(ev["target_kbps"] ?? 0) refused=\(ev["refused"] ?? 0)")
        }
        t.onControl = { _, type in if type == 5 { pipe.requestKeyframe() } }
        t.onStats = { _, j in pipe.updateTransportStats(j) }
        try pipe.start()
        let end = Date().addingTimeInterval(Double(seconds))
        while !StopSignal.requested && (seconds == 0 || Date() < end) { Thread.sleep(forTimeInterval: 0.2) }
        pipe.stop()
        t.close()
        lock.lock(); defer { lock.unlock() }
        return [
            "selftest": "synth-send",
            "sender_cpu_pct_avg": cpuSamples.isEmpty ? 0 : cpuSamples.reduce(0, +) / Double(cpuSamples.count),
            "kbps_avg": kbpsSamples.isEmpty ? 0 : kbpsSamples.reduce(0, +) / Double(kbpsSamples.count),
        ]
    }
}
