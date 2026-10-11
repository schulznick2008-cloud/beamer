import CoreMedia
import CoreVideo
import Foundation

/// Ein Teleport-Strom auf der Senderseite: Bildquelle -> VideoToolbox -> `beamer-transport`.
/// Haelt die Regeln aus SCHNITTSTELLEN §4: Keyframe auf Anfrage und alle 2 s, Bitrate jede Sekunde
/// auf `bt_target_kbps`, bei abgelehntem Frame bis zum naechsten Keyframe weitermachen.
public final class SenderPipeline {
    public let tid: UInt32
    private let codec: Codec
    private let transport: Transport
    private let source: FrameSource
    private let emit: (JSON) -> Void
    private let queue = DispatchQueue(label: "teleport.pipeline")
    private var timer: DispatchSourceTimer?

    private let lock = NSLock()
    private var encoder: VideoEncoder?
    private var forceKey = true
    private var awaitingKey = false
    private var lastBuffer: CVPixelBuffer?
    private var lastFrameHost = HostClock.nowUS()
    private var lastSent = HostClock.nowUS()
    private var capCeilingKbps: UInt32
    private var fps: Int
    private var transportStats: JSON = [:]
    private var stopped = false
    private var goneSent = false
    private var tickCount = 0
    private var cpu = CPUMeter()

    // Zaehler (unter `lock`)
    private var captured = 0, encoded = 0, refused = 0
    private var encodeMSSum = 0.0, encodeMSCount = 0
    private var bytesOut = 0
    private var failure: String?

    public init(tid: UInt32, codec: Codec, fps: Int, bitrateKbps: UInt32, transport: Transport,
                source: FrameSource, emit: @escaping (JSON) -> Void) {
        self.tid = tid
        self.codec = codec
        self.fps = fps
        self.capCeilingKbps = bitrateKbps
        self.transport = transport
        self.source = source
        self.emit = emit
    }

    public func start() throws {
        try source.start(
            onFrame: { [weak self] pb, t in self?.handleFrame(pb, t) },
            onStop: { [weak self] reason in self?.sourceStopped(reason) })
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + .milliseconds(250), repeating: .milliseconds(250))
        t.setEventHandler { [weak self] in self?.tick() }
        timer = t
        t.resume()
    }

    public func stop() {
        lock.lock()
        let was = stopped
        stopped = true
        let enc = encoder
        encoder = nil
        lock.unlock()
        if was { return }
        timer?.cancel()
        timer = nil
        source.stop()
        enc?.flush()
        enc?.invalidate()
    }

    public func requestKeyframe() {
        lock.lock()
        forceKey = true
        lock.unlock()
    }

    public func setCeiling(kbps: UInt32) {
        lock.lock()
        capCeilingKbps = kbps
        lock.unlock()
    }

    public func updateTransportStats(_ json: String) {
        guard let d = json.data(using: .utf8), let o = try? JSONSerialization.jsonObject(with: d) as? JSON else { return }
        lock.lock()
        transportStats = o
        lock.unlock()
    }

    // MARK: Frames

    private func handleFrame(_ pb: CVPixelBuffer, _ t: CMTime) {
        lock.lock()
        if stopped { lock.unlock(); return }
        captured += 1
        lastBuffer = pb
        lastFrameHost = HostClock.nowUS()
        let w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb)
        var enc = encoder
        var sizeChanged = false
        var replaced: VideoEncoder?
        if enc == nil || enc!.width != w || enc!.height != h {
            replaced = encoder
            sizeChanged = replaced != nil
            do {
                enc = try VideoEncoder(
                    width: w, height: h, codec: codec, fps: fps,
                    bitrateKbps: Int(min(max(transport.targetKbps(stream: tid), 100), capCeilingKbps)),
                    onFrame: { [weak self] f in self?.handleEncoded(f) })
            } catch {
                failure = "\(error)"
                lock.unlock()
                sourceStopped("\(error)")
                return
            }
            encoder = enc
            forceKey = true
        }
        let key = forceKey
        forceKey = false
        lock.unlock()
        if let old = replaced {
            old.flush()
            old.invalidate()
        }
        if sizeChanged {
            emit(["v": 1, "ev": "send.size", "tid": Int(tid), "width": w, "height": h, "scale": source.scale])
        }
        enc?.encode(pb, hostTime: t, forceKeyframe: key)
    }

    private func handleEncoded(_ f: EncodedFrame) {
        lock.lock()
        if stopped { lock.unlock(); return }
        if awaitingKey && !f.isKeyframe {
            lock.unlock()
            return // nach einer Ablehnung gehoeren Folgebilder zu einem Bild, das nie ankam
        }
        encoded += 1
        encodeMSSum += f.encodeMS
        encodeMSCount += 1
        lock.unlock()

        let fflags = (f.isKeyframe ? 1 : 0) | codec.fflagsBits
        let ok = transport.sendFrame(stream: tid, kind: 1, data: f.annexB, ptsUS: f.ptsUS, fflags: fflags)
        lock.lock()
        if ok {
            if f.isKeyframe { awaitingKey = false }
            bytesOut += f.annexB.count
            lastSent = HostClock.nowUS()
        } else {
            // "Sendewarteschlange voll": Frame ausgelassen, ab jetzt erst wieder ab dem naechsten Keyframe.
            refused += 1
            awaitingKey = true
            forceKey = true
        }
        lock.unlock()
    }

    private func sourceStopped(_ reason: String) {
        lock.lock()
        let was = stopped || goneSent
        goneSent = true
        lock.unlock()
        if was { return }
        emit(["v": 1, "ev": "send.gone", "tid": Int(tid), "reason": reason])
        queue.async { [weak self] in self?.stop() } // nicht auf dem Capture-Thread anhalten
    }

    // MARK: Takt (250 ms)

    private func tick() {
        lock.lock()
        if stopped { lock.unlock(); return }
        tickCount += 1
        let idleUS = HostClock.nowUS() - max(lastFrameHost, lastSent)
        let buf = lastBuffer
        let enc = encoder
        let second = tickCount % 4 == 0
        lock.unlock()

        // Nichts Neues im Bild: spaetestens alle 2 s trotzdem ein (Schluessel-)Bild senden.
        if idleUS > 1_700_000, let buf = buf, let enc = enc {
            enc.encode(buf, hostTime: HostClock.nowTime(), forceKeyframe: true)
        }
        guard second else { return }

        // Zustand (geschlossen / minimiert) und Fenstergroesse einmal pro Sekunde neu lesen. Aendert sich die
        // Groesse, wird der Encoder mit dem ersten Bild neuer Groesse neu angelegt und `send.size` gemeldet.
        let (state, resized) = source.refresh()
        if state == "gone" {
            lock.lock()
            let sent = goneSent
            goneSent = true
            lock.unlock()
            if !sent { emit(["v": 1, "ev": "send.gone", "tid": Int(tid), "reason": "window_closed"]) }
            stop()
            return
        }
        if resized { requestKeyframe() }

        // Bitrate jede Sekunde auf den Transport-Zielwert (nie ueber die Obergrenze) nachfuehren.
        let target = transport.targetKbps(stream: tid)
        lock.lock()
        let ceiling = capCeilingKbps
        lock.unlock()
        let eff = Int(min(max(target, 100), ceiling))
        enc?.setBitrate(kbps: eff)

        emitStats(state: state, effectiveKbps: eff)
    }

    private func emitStats(state: String, effectiveKbps: Int) {
        lock.lock()
        let t = transportStats
        let cap = captured, enc = encoded, ref = refused
        let ms = encodeMSCount > 0 ? encodeMSSum / Double(encodeMSCount) : 0
        captured = 0; encoded = 0; refused = 0; encodeMSSum = 0; encodeMSCount = 0
        bytesOut = 0
        let cpuPct = cpu.sample()
        lock.unlock()

        var e: JSON = [
            "v": 1, "ev": "stats", "tid": Int(tid),
            "fps": t["fps"] ?? 0, "kbps": t["kbps"] ?? 0, "rtt_ms": t["rtt_ms"] ?? 0,
            "loss_pct": t["loss_pct"] ?? 0, "latency_ms": t["latency_ms"] ?? 0,
            "state": state, "target_kbps": effectiveKbps,
            "capture_fps": cap, "encode_fps": enc, "refused": ref,
            "encode_ms": (ms * 10).rounded() / 10, "cpu_pct": (cpuPct * 10).rounded() / 10,
        ]
        if let q = t["pace_queue_ms"] { e["pace_queue_ms"] = q }
        emit(e)
    }
}
