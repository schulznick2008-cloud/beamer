import Foundation

/// Die echte Umsetzung der Befehle: ein Transport, mehrere Sender-Strome.
public final class SenderEngine: TeleportBackend {
    private let emit: (JSON) -> Void
    private let lock = NSLock()
    private var transport: Transport?
    private var pipelines: [UInt32: SenderPipeline] = [:]
    /// Von selbst beendete Strome (send.gone): ein spaeteres send.stop dazu ist kein Fehler.
    private var ended: Set<UInt32> = []
    /// Erzeugt die Bildquelle; die Tests und `--selftest-synth` tauschen sie aus.
    public var makeSource: (CaptureMode, Int) throws -> FrameSource = { mode, fps in
        let s = ScreenCaptureSource(mode: mode, maxFps: fps)
        try s.prepare()
        return s
    }

    /// Wie lange `send.start` auf die Quelle wartet, bevor es mit `busy` scheitert (Sekunden).
    public var startTimeout: TimeInterval = 12
    /// Wie lange `send.stop` hoechstens auf das Aufraeumen wartet (die Antwort kommt immer rechtzeitig).
    public var stopTimeout: TimeInterval = 1.5

    public init(emit: @escaping (JSON) -> Void) {
        self.emit = emit
        if let test = TestSources.fromEnvironment() { makeSource = test }
    }

    // MARK: Transport

    public func transportOpen(localPort: UInt16, peerHost: String?, peerPort: UInt16) throws -> UInt16 {
        lock.lock(); defer { lock.unlock() }
        guard transport == nil else { throw IPCError(.busy, "Transport ist schon offen") }
        let t: Transport
        do {
            t = try Transport(localPort: localPort)
        } catch {
            throw IPCError(.internalError, "\(error)")
        }
        t.onControl = { [weak self] sid, type in
            if type == 5 { self?.pipeline(sid)?.requestKeyframe() }
        }
        t.onStats = { [weak self] sid, json in self?.pipeline(sid)?.updateTransportStats(json) }
        if let host = peerHost, peerPort > 0 {
            do { try t.setPeer(host: host, port: peerPort) } catch {
                t.close()
                throw IPCError(.badRequest, "\(error)")
            }
        }
        transport = t
        return t.localPort
    }

    public func transportPeer(host: String, port: UInt16) throws {
        lock.lock(); defer { lock.unlock() }
        guard let t = transport else { throw IPCError(.badRequest, "transport.open fehlt") }
        do { try t.setPeer(host: host, port: port) } catch { throw IPCError(.badRequest, "\(error)") }
    }

    public func transportClose() {
        lock.lock()
        let ps = Array(pipelines.values)
        pipelines.removeAll()
        let t = transport
        transport = nil
        lock.unlock()
        // Alle Strome gleichzeitig anhalten, damit ein haengender nicht die anderen aufhaelt; hoechstens
        // `stopTimeout` warten, danach wird der Transport geschlossen und der Rest laeuft im Hintergrund aus.
        let group = DispatchGroup()
        for p in ps {
            group.enter()
            DispatchQueue.global().async { p.stop(); group.leave() }
        }
        _ = group.wait(timeout: .now() + stopTimeout)
        t?.close()
    }

    private func pipeline(_ tid: UInt32) -> SenderPipeline? {
        lock.lock(); defer { lock.unlock() }
        return pipelines[tid]
    }

    // MARK: Fenster und Strome

    public func listWindows() throws -> [WindowInfo] {
        try WindowCatalog.list()
    }

    public func sendStart(_ p: SendParams) throws -> SendStarted {
        lock.lock()
        guard let t = transport else {
            lock.unlock()
            throw IPCError(.badRequest, "transport.open fehlt")
        }
        ended.remove(p.tid)
        guard pipelines[p.tid] == nil else {
            lock.unlock()
            throw IPCError(.busy, "tid \(p.tid) laeuft schon")
        }
        lock.unlock()

        let source = try makeSource(p.mode, p.maxFps)   // wirft permission / not_found
        let size = source.pixelSize
        do {
            try t.addStream(id: p.tid, role: .sender, keys: p.keys, maxKbps: p.bitrateKbps)
        } catch {
            // Auch hier nicht unbegrenzt auf die Quelle warten.
            _ = try? runBounded(timeout: 1.2, "Capture stop") { source.stop() }
            throw IPCError(.badRequest, "\(error)")
        }
        let tid = p.tid
        let pipe = SenderPipeline(tid: tid, codec: p.codec, fps: p.maxFps, bitrateKbps: p.bitrateKbps,
                                  transport: t, source: source) { [weak self] ev in
            self?.emit(ev)
            if ev["ev"] as? String == "send.gone" { self?.dropPipeline(tid, ended: true) }
        }
        lock.lock()
        pipelines[tid] = pipe
        lock.unlock()
        do {
            try runBounded(timeout: startTimeout, "send.start (Quelle)") { try pipe.start() }
        } catch let e as IPCError {
            dropPipelineAsync(tid)
            throw e
        } catch let t as BlockingTimeout {
            dropPipelineAsync(tid)
            throw IPCError(.busy, "Die Bildquelle startet nicht (\(t)). Mac ueberlastet? Spaeter erneut versuchen.")
        } catch {
            dropPipelineAsync(tid)
            throw IPCError(.internalError, "\(error)")
        }
        return SendStarted(width: size.width, height: size.height, scale: source.scale)
    }

    /// Nimmt den Strom aus der Verwaltung und raeumt auf. Wartet hoechstens `stopTimeout` darauf; haengt die Quelle,
    /// laeuft das Aufraeumen im Hintergrund weiter, der Aufrufer (IPC-Thread) kommt trotzdem rechtzeitig zurueck.
    private func dropPipeline(_ tid: UInt32, ended wasEnded: Bool = false) {
        lock.lock()
        if wasEnded { ended.insert(tid) }
        let pipe = pipelines.removeValue(forKey: tid)
        let t = transport
        lock.unlock()
        t?.removeStream(id: tid)
        guard let p = pipe else { return }
        _ = try? runBounded(timeout: stopTimeout, "Strom beenden") { p.stop() }
    }

    private func dropPipelineAsync(_ tid: UInt32) {
        lock.lock()
        let pipe = pipelines.removeValue(forKey: tid)
        let t = transport
        lock.unlock()
        t?.removeStream(id: tid)
        if let p = pipe { DispatchQueue.global().async { p.stop() } }
    }

    public func sendStop(tid: UInt32, restore: Bool) throws {
        lock.lock()
        let wasEnded = ended.remove(tid) != nil
        lock.unlock()
        guard pipeline(tid) != nil else {
            if wasEnded { return } // schon per send.gone beendet
            throw IPCError(.notFound, "tid \(tid) unbekannt")
        }
        dropPipeline(tid)
    }

    public func sendKeyframe(tid: UInt32) throws {
        guard let p = pipeline(tid) else { throw IPCError(.notFound, "tid \(tid) unbekannt") }
        p.requestKeyframe()
    }

    public func sendBitrate(tid: UInt32, kbps: UInt32) throws {
        guard let p = pipeline(tid) else { throw IPCError(.notFound, "tid \(tid) unbekannt") }
        lock.lock()
        let t = transport
        lock.unlock()
        do { try t?.setMaxKbps(stream: tid, kbps: kbps) } catch { throw IPCError(.internalError, "\(error)") }
        p.setCeiling(kbps: kbps)
    }

    /// Aktuelle Zielbitrate des Transports fuer einen Strom (Tests).
    public func targetKbps(tid: UInt32) -> UInt32 {
        lock.lock(); defer { lock.unlock() }
        return transport?.targetKbps(stream: tid) ?? 0
    }

    public func shutdown() {
        transportClose()
    }
}
