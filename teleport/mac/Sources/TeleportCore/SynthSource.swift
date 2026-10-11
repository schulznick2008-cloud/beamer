import CoreMedia
import CoreVideo
import Foundation

/// Muster-Quelle fuer Selbsttests ohne Bildschirmaufnahme-Freigabe: bewegtes Bild mit Rauschanteil,
/// damit der Encoder echte Arbeit hat. Laeuft ueber denselben Encoder- und Transportweg wie die echte Quelle.
public final class SynthSource: FrameSource {
    private let width: Int
    private let height: Int
    private var fps: Int
    private var timer: DispatchSourceTimer?
    private var pool: CVPixelBufferPool?
    private let queue = DispatchQueue(label: "teleport.synth", qos: .userInteractive)
    private var frame = 0
    private var noise: [UInt8]

    public init(width: Int, height: Int, fps: Int) {
        self.width = width & ~1
        self.height = height & ~1
        self.fps = fps
        var g = SystemRandomNumberGenerator()
        noise = (0..<(max(width, 64) * 64)).map { _ in UInt8.random(in: 0...255, using: &g) }
    }

    public var state: String { "ok" }
    public var pixelSize: (width: Int, height: Int) { (width, height) }
    public var scale: Double { 1 }

    public func start(onFrame: @escaping (CVPixelBuffer, CMTime) -> Void, onStop: @escaping (String) -> Void) throws {
        let attrs: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferWidthKey: width, kCVPixelBufferHeightKey: height,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        var p: CVPixelBufferPool?
        CVPixelBufferPoolCreate(nil, nil, attrs as CFDictionary, &p)
        guard let pool = p else { throw IPCError(.internalError, "Pixelpuffer-Pool nicht anlegbar") }
        self.pool = pool
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now(), repeating: .nanoseconds(1_000_000_000 / fps), leeway: .microseconds(200))
        t.setEventHandler { [weak self] in
            guard let self = self, let pb = self.makeFrame() else { return }
            onFrame(pb, HostClock.nowTime())
        }
        timer = t
        t.resume()
    }

    public func setMaxFps(_ fps: Int) {
        guard fps > 0, fps != self.fps else { return }
        self.fps = fps
        timer?.schedule(deadline: .now(), repeating: .nanoseconds(1_000_000_000 / fps), leeway: .microseconds(200))
    }

    public func stop() {
        timer?.cancel()
        timer = nil
    }

    private func makeFrame() -> CVPixelBuffer? {
        guard let pool = pool else { return nil }
        var pb: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &pb) == kCVReturnSuccess, let buf = pb else { return nil }
        CVPixelBufferLockBaseAddress(buf, [])
        defer { CVPixelBufferUnlockBaseAddress(buf, []) }
        frame += 1
        if let y = CVPixelBufferGetBaseAddressOfPlane(buf, 0) {
            let stride = CVPixelBufferGetBytesPerRowOfPlane(buf, 0)
            let base = y.assumingMemoryBound(to: UInt8.self)
            let noiseRows = height / 3
            let shift = (frame * 7) % 64
            noise.withUnsafeBufferPointer { n in
                for row in 0..<height {
                    let line = base + row * stride
                    if row < noiseRows {
                        // Rauschband, das jedes Bild anders liegt
                        memcpy(line, n.baseAddress! + ((row + shift) % 64) * max(width, 64), width)
                    } else {
                        memset(line, Int32((row + frame * 3) & 0xFF), width)
                    }
                }
            }
        }
        if let c = CVPixelBufferGetBaseAddressOfPlane(buf, 1) {
            memset(c, 128, CVPixelBufferGetBytesPerRowOfPlane(buf, 1) * (height / 2))
        }
        return buf
    }
}
