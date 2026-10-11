import CoreMedia
import CoreVideo
import Foundation

/// Ein Teleport-Strom auf der Senderseite: Bildquelle -> Encoder -> `beamer-transport`.
/// Haelt die Regeln aus SCHNITTSTELLEN §4: Keyframe auf Anfrage und alle 2 s, Bitrate jede Sekunde
/// auf `bt_target_kbps`, bei abgelehntem Frame bis zum naechsten Keyframe weitermachen.
///
/// Rueckstau: Hoechstens `maxInFlight` Frames stecken gleichzeitig im Encoder. Weitere Capture-Frames
/// ersetzen das eine wartende (das neueste zaehlt), damit die Latenz nicht mit dem Rueckstau waechst.
/// Schafft der Encoder die Bildrate nicht, wird die effektive Bildrate gesenkt (`encoder_slow`).
public final class SenderPipeline {
    public static let maxInFlight = 2
    public static let minEffectiveFps = 20

    public let tid: UInt32
    private let codec: Codec
    private let transport: Transport
    private let source: FrameSource
    private let emit: (JSON) -> Void
    private let makeEncoder: EncoderFactory
    private let lowPower: () -> Bool
    private let queue = DispatchQueue(label: "teleport.pipeline")
    /// Alle ScreenCaptureKit-Abfragen (Zustand, Groesse, Bildrate) laufen hier, nie auf der Pipeline-Queue:
    /// Kehrt ein Aufruf nicht zurueck, steht nur diese Queue, der Strom und `stop` laufen weiter.
    private let sckQueue = DispatchQueue(label: "teleport.pipeline.sck")
    private var refreshRunning = false
    /// Zuletzt gemeldete Stream-Groesse (damit `send.size` nicht doppelt kommt).
    private var reportedSize: (w: Int, h: Int)?
    private var lastRefresh: (state: String, resized: Bool) = ("ok", false)
    private var timer: DispatchSourceTimer?

    private let lock = NSLock()
    private var encoder: FrameEncoder?
    private var inFlight = 0
    /// Wann zuletzt ein Frame in den Encoder ging oder fertig wurde (Wachhund gegen verschluckte Frames).
    private var lastProgress = DispatchTime.now().uptimeNanoseconds
    public private(set) var encoderResets = 0
    private var pending: (pb: CVPixelBuffer, time: CMTime, force: Bool)?
    private var forceKey = true            // gilt fuer das naechste Bild, solange es noch kein letztes gibt
    private var awaitingKey = false
    private var lastBuffer: CVPixelBuffer?
    private var lastFrameHost = HostClock.nowUS()
    private var lastSent = HostClock.nowUS()
    private var capCeilingKbps: UInt32
    private let maxFps: Int
    private var effectiveFps: Int
    private var transportStats: JSON = [:]
    private var stopped = false
    private var goneSent = false
    private var tickCount = 0
    private var cpu = CPUMeter()

    // Zaehler je Sekunde (unter `lock`)
    private var captured = 0, encoded = 0, refused = 0, backlogDrops = 0
    private var encodeMSSum = 0.0, encodeMSCount = 0
    private var slowSeconds = 0, fastSeconds = 0
    private var encoderSlow = false

    public init(tid: UInt32, codec: Codec, fps: Int, bitrateKbps: UInt32, transport: Transport,
                source: FrameSource, makeEncoder: EncoderFactory? = nil,
                lowPower: @escaping () -> Bool = { ProcessInfo.processInfo.isLowPowerModeEnabled },
                emit: @escaping (JSON) -> Void) {
        self.tid = tid
        self.codec = codec
        self.maxFps = fps
        self.effectiveFps = fps
        self.capCeilingKbps = bitrateKbps
        self.transport = transport
        self.source = source
        self.emit = emit
        self.lowPower = lowPower
        self.makeEncoder = makeEncoder ?? { w, h, c, f, k, done in
            try VideoEncoder(width: w, height: h, codec: c, fps: f, bitrateKbps: k, onFrame: done)
        }
    }

    public func start() throws {
        let px = source.pixelSize
        lock.lock()
        if reportedSize == nil, px.width > 0 { reportedSize = (px.width, px.height) }
        lock.unlock()
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
        pending = nil
        lock.unlock()
        if was { return }
        timer?.cancel()
        timer = nil
        // Hoechstens ~1,2 s auf die Quelle warten; kehrt `stop` nicht zurueck, laeuft es im Hintergrund weiter.
        let src = source
        do {
            try runBounded(timeout: 1.2, "Capture stop") { src.stop() }
        } catch {
            logErr("teleport: \(error); Quelle wird im Hintergrund weiter angehalten")
        }
        enc?.retire()
    }

    /// Keyframe sofort: auch bei stehendem Bild wird das letzte Bild gleich als Keyframe kodiert.
    public func requestKeyframe() {
        lock.lock()
        let buf = lastBuffer
        if buf == nil { forceKey = true }
        lock.unlock()
        guard let b = buf else { return }
        queue.async { [weak self] in self?.submit(b, HostClock.nowTime(), force: true) }
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
        let force = forceKey
        forceKey = false
        if reportedSize == nil { reportedSize = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb)) }
        lock.unlock()
        submit(pb, t, force: force)
    }

    /// Gibt ein Bild an den Encoder, solange hoechstens `maxInFlight` darin stecken, sonst merken (das neueste).
    private func submit(_ pb: CVPixelBuffer, _ t: CMTime, force: Bool) {
        lock.lock()
        if stopped { lock.unlock(); return }
        let w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb)
        var replaced: FrameEncoder?
        var sizeChanged = false
        var forceNow = force
        if encoder == nil || encoder!.width != w || encoder!.height != h {
            replaced = encoder
            sizeChanged = replaced != nil
            do {
                let kbps = Int(min(max(transport.targetKbps(stream: tid), 100), capCeilingKbps))
                encoder = try makeEncoder(w, h, codec, effectiveFps, kbps) { [weak self] f in self?.encoderDone(f) }
            } catch {
                lock.unlock()
                sourceStopped("\(error)")
                return
            }
            forceNow = true
        }
        var enc: FrameEncoder?
        if inFlight >= Self.maxInFlight {
            if pending != nil { backlogDrops += 1 }
            pending = (pb, t, force || forceNow || (pending?.force ?? false))
        } else {
            if inFlight == 0 { lastProgress = DispatchTime.now().uptimeNanoseconds }
            inFlight += 1
            enc = encoder
        }
        lock.unlock()
        if let old = replaced { old.retire() }
        if sizeChanged { reportSize(w, h) }
        enc?.encode(pb, hostTime: t, forceKeyframe: forceNow)
    }

    /// Der Encoder ist mit einem Frame fertig (mit oder ohne Ergebnis): Platz frei, ggf. das wartende nachreichen.
    private func encoderDone(_ f: EncodedFrame?) {
        lock.lock()
        inFlight = max(0, inFlight - 1)
        lastProgress = DispatchTime.now().uptimeNanoseconds
        let next = stopped ? nil : pending
        if next != nil { pending = nil }
        lock.unlock()
        if let n = next { submit(n.pb, n.time, force: n.force) }
        guard let f = f else { return }
        handleEncoded(f)
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
        let retry: Bool
        if ok {
            if f.isKeyframe { awaitingKey = false }
            lastSent = HostClock.nowUS()
            retry = false
        } else {
            // "Sendewarteschlange voll": Frame ausgelassen, ab jetzt erst wieder ab dem naechsten Keyframe.
            refused += 1
            awaitingKey = true
            retry = true
        }
        lock.unlock()
        if retry { requestKeyframe() }
    }

    /// `send.size` melden, wenn sich die Groesse seit der letzten Meldung geaendert hat.
    private func reportSize(_ w: Int, _ h: Int) {
        lock.lock()
        let changed = reportedSize.map { $0.w != w || $0.h != h } ?? false
        reportedSize = (w, h)
        lock.unlock()
        if changed {
            emit(["v": 1, "ev": "send.size", "tid": Int(tid), "width": w, "height": h, "scale": source.scale])
        }
    }

    /// Nach einer Groessenaenderung von aussen (`send.resize`): Quelle sofort neu lesen, `send.size` melden und einen
    /// Keyframe anfordern. Wartet hoechstens `timeout` Sekunden (ScreenCaptureKit kann haengen) und liefert die
    /// aktuelle Stream-Groesse in Pixeln.
    @discardableResult
    public func syncSize(timeout: TimeInterval = 1.5) -> (width: Int, height: Int) {
        let src = source
        let r = try? runBounded(timeout: timeout, "Groesse neu lesen") { src.refresh() }
        let size = src.pixelSize
        if r?.sizeChanged == true {
            reportSize(size.width, size.height)
            requestKeyframe()
        }
        return size
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
        let second = tickCount % 4 == 0
        lock.unlock()

        watchdog()

        // Nichts Neues im Bild: spaetestens alle 2 s trotzdem ein (Schluessel-)Bild senden.
        if idleUS > 1_700_000, let buf = buf {
            submit(buf, HostClock.nowTime(), force: true)
        }
        // Groessenaenderung der App selbst schnell bemerken (billige Abfrage alle 250 ms) und dann sofort neu lesen.
        if let pt = source.currentPointSize() {
            let px = source.pixelSize
            let sc = source.scale
            let w = max(2, Int((Double(pt.width) * sc).rounded()) & ~1), h = max(2, Int((Double(pt.height) * sc).rounded()) & ~1)
            if (w != px.width || h != px.height) && pt.width > 0 { scheduleRefresh() }
        }
        consumeRefreshResult()
        guard second else { return }
        scheduleRefresh()
        lock.lock()
        let state = lastRefresh.state
        lock.unlock()

        // Bitrate jede Sekunde auf den Transport-Zielwert (nie ueber die Obergrenze) nachfuehren.
        let target = transport.targetKbps(stream: tid)
        lock.lock()
        let ceiling = capCeilingKbps
        let enc = encoder
        lock.unlock()
        let eff = Int(min(max(target, 100), ceiling))
        enc?.setBitrate(kbps: eff)

        adaptFrameRate()
        emitStats(state: state, effectiveKbps: eff)
    }

    /// Ergebnis der letzten Zustandsabfrage uebernehmen (geschlossen, Groesse geaendert).
    private func consumeRefreshResult() {
        lock.lock()
        let (state, resized) = lastRefresh
        lastRefresh.resized = false
        lock.unlock()
        if state == "gone" {
            lock.lock()
            let sent = goneSent
            goneSent = true
            lock.unlock()
            if !sent { emit(["v": 1, "ev": "send.gone", "tid": Int(tid), "reason": "window_closed"]) }
            stop()
            return
        }
        if resized {
            let px = source.pixelSize
            reportSize(px.width, px.height)
            requestKeyframe()
        }
    }

    private func scheduleRefresh() {
        lock.lock()
        if refreshRunning || stopped { lock.unlock(); return }
        refreshRunning = true
        lock.unlock()
        sckQueue.async { [weak self] in
            guard let self = self else { return }
            let r = self.source.refresh()
            self.lock.lock()
            self.lastRefresh = (r.state, self.lastRefresh.resized || r.sizeChanged)
            self.refreshRunning = false
            self.lock.unlock()
        }
    }

    /// Steckt ein Frame laenger als 0,6 s im Encoder, ohne dass irgendein Frame fertig wird, hat VideoToolbox
    /// Frames verschluckt (z. B. nach einer Eigenschaftsaenderung unter Last). Dann den Encoder verwerfen und
    /// beim naechsten Bild neu anlegen, sonst bliebe der Strom fuer immer stehen.
    private func watchdog() {
        lock.lock()
        let stuck = inFlight > 0
            && Double(DispatchTime.now().uptimeNanoseconds &- lastProgress) / 1e9 > 0.6
        var old: FrameEncoder?
        if stuck {
            old = encoder
            encoder = nil
            inFlight = 0
            pending = nil
            awaitingKey = false
            forceKey = true
            encoderResets += 1
        }
        let buf = lastBuffer
        lock.unlock()
        guard stuck else { return }
        logErr("teleport: Encoder hat Frames verschluckt, wird neu gestartet")
        queue.async { old?.retire(timeout: 0.2) }
        if let b = buf { submit(b, HostClock.nowTime(), force: true) }
    }

    /// Liegt die Encode-Zeit 2 s lang ueber einer Bildzeit, gilt der Encoder als zu langsam. Kam dabei
    /// Rueckstau auf (Frames mussten verworfen werden), sinkt die effektive Bildrate auf den Durchsatz
    /// (nicht unter `minEffectiveFps`). Ist der Encoder 3 s lang wieder schnell, geht es schrittweise zurueck.
    private func adaptFrameRate() {
        lock.lock()
        let ms = encodeMSCount > 0 ? encodeMSSum / Double(encodeMSCount) : 0
        let frameTimeMS = 1000.0 / Double(maxFps)
        let slowNow = encodeMSCount > 0 && ms > frameTimeMS
        slowSeconds = slowNow ? slowSeconds + 1 : 0
        fastSeconds = (encodeMSCount > 0 && ms < 0.6 * 1000.0 / Double(effectiveFps)) ? fastSeconds + 1 : 0
        encoderSlow = slowSeconds >= 2
        var newFps = effectiveFps
        if encoderSlow && backlogDrops > 0 {
            let throughput = Int(Double(encoded) * 0.95)
            newFps = max(Self.minEffectiveFps, min(effectiveFps, throughput))
        } else if fastSeconds >= 3 && effectiveFps < maxFps {
            newFps = min(maxFps, effectiveFps + max(1, effectiveFps / 5))
            fastSeconds = 0
        }
        let changed = newFps != effectiveFps
        effectiveFps = newFps
        let enc = encoder
        lock.unlock()
        if changed {
            let src = source
            sckQueue.async { src.setMaxFps(newFps) }
            enc?.setFrameRate(newFps)
        }
    }

    private func emitStats(state: String, effectiveKbps: Int) {
        lock.lock()
        let t = transportStats
        let cap = captured, enc = encoded, ref = refused, drops = backlogDrops
        let ms = encodeMSCount > 0 ? encodeMSSum / Double(encodeMSCount) : 0
        let slow = encoderSlow
        let eff = effectiveFps
        let resets = encoderResets
        captured = 0; encoded = 0; refused = 0; backlogDrops = 0; encodeMSSum = 0; encodeMSCount = 0
        let cpuPct = cpu.sample()
        lock.unlock()

        var e: JSON = [
            "v": 1, "ev": "stats", "tid": Int(tid),
            "fps": t["fps"] ?? 0, "kbps": t["kbps"] ?? 0, "rtt_ms": t["rtt_ms"] ?? 0,
            "loss_pct": t["loss_pct"] ?? 0, "latency_ms": t["latency_ms"] ?? 0,
            "state": state, "target_kbps": effectiveKbps,
            "capture_fps": cap, "encode_fps": enc, "refused": ref, "backlog_drops": drops,
            "encode_ms": (ms * 10).rounded() / 10, "cpu_pct": (cpuPct * 10).rounded() / 10,
            "encoder_slow": slow, "effective_fps": eff, "low_power": lowPower(), "encoder_resets": resets,
        ]
        if let q = t["pace_queue_ms"] { e["pace_queue_ms"] = q }
        emit(e)
    }
}
