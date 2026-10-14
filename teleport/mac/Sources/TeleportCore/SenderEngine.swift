import CoreGraphics
import Foundation

/// Was zu einem laufenden Strom ausser der Pipeline gehoert: Fenster, virtueller Monitor, Wiederherstellung.
final class StreamControl {
    let windowID: UInt32?
    let pid: pid_t?
    let displayID: UInt32?
    let source: FrameSource
    var virtual: VirtualDisplayHandle?
    /// Rahmen des Fensters vor dem Verschieben auf den virtuellen Monitor (Punkte, AX-Koordinaten).
    var restoreFrame: CGRect?
    var lastActivation = Date.distantPast
    /// `send.stop` mit `restore:true`: das Fenster kommt unter den Mauszeiger zurueck, nicht an die alte Stelle.
    var placeAtMouse = false

    init(windowID: UInt32?, pid: pid_t?, displayID: UInt32?, source: FrameSource) {
        self.windowID = windowID; self.pid = pid; self.displayID = displayID; self.source = source
    }
}

/// Die echte Umsetzung der Befehle: ein Transport, mehrere Sender-Strome.
public final class SenderEngine: TeleportBackend {
    private let emit: (JSON) -> Void
    private let lock = NSLock()
    private var transport: Transport?
    private var pipelines: [UInt32: SenderPipeline] = [:]
    private var controls: [UInt32: StreamControl] = [:]
    /// Von selbst beendete Strome (send.gone): ein spaeteres send.stop dazu ist kein Fehler.
    private var ended: Set<UInt32> = []
    /// Erzeugt die Bildquelle; die Tests und `--selftest-synth` tauschen sie aus.
    public var makeSource: (CaptureMode, Int) throws -> FrameSource = { mode, fps in
        let s = ScreenCaptureSource(mode: mode, maxFps: fps)
        try s.prepare()
        return s
    }

    // Eingabe, Fenstersteuerung, virtueller Monitor (C04). Die Tests tauschen diese Teile aus.
    public var windowController: WindowController = AXWindowController()
    public var eventSink: EventSink = CGEventSink()
    public var keyLayout: KeyLayout = SystemKeyLayout()
    public var virtualDisplayFactory: VirtualDisplayFactory = { w, h in
        try SystemVirtualDisplay.make(widthPoints: w, heightPoints: h)
    }
    /// Wirft `permission`, wenn die Bedienungshilfen-Freigabe fehlt.
    public var accessibilityCheck: () throws -> Void = { try AccessibilityPermission.check() }
    public var windowFrameProvider: (UInt32) -> CGRect? = { WindowGeometry.frame(windowID: $0) }
    public var displayFrameProvider: (UInt32) -> CGRect = { CGDisplayBounds($0) }
    public var ownerPIDProvider: (UInt32) -> pid_t? = { WindowGeometry.ownerPID(windowID: $0) }
    public let sleepGuard = SleepGuard()
    /// Wohin ein zurueckkehrendes Fenster der Groesse `size` kommt (Titelleiste unter der Maus); `nil`: alte Stelle.
    public var returnOriginProvider: (CGSize) -> CGPoint? = { ReturnPlacement.systemOrigin(size: $0) }
    /// Oberer Rand des virtuellen Monitors, den die Menueleiste belegt: das Fenster sitzt darunter (Punkte).
    public static let menuBarAllowance = 40
    private lazy var injector = Injector(sink: eventSink, layout: { [unowned self] in self.keyLayout }, repeatTiming: SystemKeyRepeat.timing)

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
        lock.lock()
        let remaining = Array(controls.keys)
        lock.unlock()
        for tid in remaining { teardownControl(tid) }
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

        if p.virtualDisplay, case .display = p.mode {
            throw IPCError(.badRequest, "virtual_display gilt nur fuer mode window")
        }
        // Fenster-Daten und (bei `virtual_display`) der virtuelle Monitor.
        var captureMode = p.mode
        var windowID: UInt32?
        var pid: pid_t?
        var displayID: UInt32?
        var virtual: VirtualDisplayHandle?
        var restoreFrame: CGRect?
        switch p.mode {
        case .window(let wid):
            windowID = wid
            pid = ownerPIDProvider(wid)
            if p.virtualDisplay {
                try accessibilityCheck()
                guard let pid = pid else { throw IPCError(.notFound, "Fenster \(wid) nicht gefunden") }
                let frame = try windowController.frame(windowID: wid, pid: pid)
                windowController.prepare(windowID: wid, pid: pid)
                let vd = try virtualDisplayFactory(Int(frame.width.rounded()),
                                                   Int(frame.height.rounded()) + Self.menuBarAllowance)
                virtual = vd
                restoreFrame = frame
                captureMode = .display(vd.displayID)
                displayID = vd.displayID
                ScreenCaptureSource.setScaleHint(2, forDisplay: vd.displayID) // der virtuelle Monitor ist HiDPI
            }
        case .display(let id):
            displayID = id
        }

        func cleanupVirtual() {
            if let vd = virtual {
                if let wid = windowID, let pid = pid, let f = restoreFrame {
                    try? windowController.setFrame(windowID: wid, pid: pid, origin: f.origin, size: f.size)
                }
                vd.destroy()
            }
        }

        let source: FrameSource
        do {
            source = try makeSourceWaiting(captureMode, p.maxFps, patient: virtual != nil)   // wirft permission / not_found
        } catch {
            cleanupVirtual()
            throw error
        }
        if let vd = virtual, let wid = windowID, let pid = pid, let f = restoreFrame {
            // Das Fenster auf den virtuellen Monitor holen, unter die Menueleiste.
            do {
                try windowController.setFrame(
                    windowID: wid, pid: pid,
                    origin: CGPoint(x: vd.bounds.minX, y: vd.bounds.minY + CGFloat(Self.menuBarAllowance)), size: f.size)
            } catch {
                _ = try? runBounded(timeout: 1.2, "Capture stop") { source.stop() }
                cleanupVirtual()
                throw error
            }
        }
        let size = source.pixelSize
        do {
            try t.addStream(id: p.tid, role: .sender, keys: p.keys, maxKbps: p.bitrateKbps)
        } catch {
            // Auch hier nicht unbegrenzt auf die Quelle warten.
            _ = try? runBounded(timeout: 1.2, "Capture stop") { source.stop() }
            cleanupVirtual()
            throw IPCError(.badRequest, "\(error)")
        }
        let tid = p.tid
        let control = StreamControl(windowID: windowID, pid: pid, displayID: displayID, source: source)
        control.virtual = virtual
        control.restoreFrame = restoreFrame
        if virtual != nil, let wid = windowID, let pid = pid, let f = restoreFrame {
            CrashRestore.register(windowID: wid, pid: pid, frame: f)
        }
        let pipe = SenderPipeline(tid: tid, codec: p.codec, fps: p.maxFps, bitrateKbps: p.bitrateKbps,
                                  transport: t, source: source) { [weak self] ev in
            self?.emit(ev)
            if ev["ev"] as? String == "send.gone" { self?.dropPipeline(tid, ended: true) }
        }
        lock.lock()
        pipelines[tid] = pipe
        controls[tid] = control
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
        sleepGuard.acquire()
        return SendStarted(width: size.width, height: size.height, scale: source.scale)
    }

    /// `makeSource`, bei einem frisch angelegten virtuellen Monitor mit Geduld: ScreenCaptureKit listet ihn erst
    /// nach einem Moment.
    private func makeSourceWaiting(_ mode: CaptureMode, _ fps: Int, patient: Bool) throws -> FrameSource {
        let deadline = Date().addingTimeInterval(patient ? 4 : 0)
        while true {
            do {
                return try makeSource(mode, fps)
            } catch let e as IPCError where e.code == .notFound && Date() < deadline {
                Thread.sleep(forTimeInterval: 0.2)
            }
        }
    }

    // MARK: Eingabe, Aktivierung, Groesse

    private func control(_ tid: UInt32) -> StreamControl? {
        lock.lock(); defer { lock.unlock() }
        return controls[tid]
    }

    /// Welcher Bildschirmausschnitt (Punkte) das uebertragene Bild gerade zeigt.
    private func geometry(of c: StreamControl) -> InputGeometry? {
        let px = c.source.pixelSize
        let rect: CGRect?
        if let vd = c.virtual {
            rect = vd.bounds
        } else if let wid = c.windowID {
            rect = windowFrameProvider(wid)
        } else if let did = c.displayID {
            rect = displayFrameProvider(did)
        } else {
            rect = nil
        }
        guard let r = rect, r.width > 0, r.height > 0 else { return nil }
        return InputGeometry(origin: r.origin, size: r.size, streamWidth: px.width, streamHeight: px.height)
    }

    public func inject(tid: UInt32, input: InputEvent) throws {
        guard let c = control(tid) else { throw IPCError(.notFound, "tid \(tid) unbekannt") }
        try accessibilityCheck()
        // Vor down, key und text muss das Fenster vorne sein (hoechstens alle 0,5 s pruefen).
        if input.needsActivation, let wid = c.windowID, let pid = c.pid,
           Date().timeIntervalSince(c.lastActivation) > 0.5 {
            if !windowController.isFrontmost(windowID: wid, pid: pid) {
                try? windowController.activate(windowID: wid, pid: pid)
            }
            c.lastActivation = Date()
        }
        guard let g = geometry(of: c) else {
            throw IPCError(.notFound, "Das Fenster oder der Bildschirm ist nicht mehr vorhanden")
        }
        injector.handle(input, geometry: g) // ausserhalb des Bildes: verworfen (§3)
    }

    public func sendActivate(tid: UInt32) throws {
        guard let c = control(tid) else { throw IPCError(.notFound, "tid \(tid) unbekannt") }
        try accessibilityCheck()
        guard let wid = c.windowID, let pid = c.pid else { return } // Bildschirm: nichts zu aktivieren
        try windowController.activate(windowID: wid, pid: pid)
        c.lastActivation = Date()
    }

    public func sendResize(tid: UInt32, width: Double, height: Double) throws -> (width: Int, height: Int) {
        guard let c = control(tid), let pipe = pipeline(tid) else { throw IPCError(.notFound, "tid \(tid) unbekannt") }
        try accessibilityCheck()
        guard let wid = c.windowID, let pid = c.pid else {
            throw IPCError(.unsupported, "send.resize gilt nur fuer Fenster")
        }
        let size = CGSize(width: width.rounded(), height: height.rounded())
        let debug = ProcessInfo.processInfo.environment["BEAMER_TELEPORT_DEBUG"] == "1"
        var mark = Date()
        func lap(_ what: String) {
            if debug { logErr("resize: \(what) \(Int(Date().timeIntervalSince(mark) * 1000)) ms") }
            mark = Date()
        }
        var captureChanged = false
        let captureGroup = DispatchGroup()
        if let vd = c.virtual {
            let newMonitorH = Int(size.height) + Self.menuBarAllowance
            let shrinking = Int(size.width) <= vd.widthPoints && newMonitorH <= vd.heightPoints
            // Das Bild ist der ganze Monitor: seine Zielgroesse steht fest. Die Capture-Umstellung (ScreenCaptureKit,
            // 60-180 ms) laeuft deshalb gleichzeitig mit Monitor- und Fensterwechsel statt danach.
            captureGroup.enter()
            DispatchQueue.global(qos: .userInitiated).async {
                captureChanged = pipe.applySizeEarly(points: CGSize(width: size.width, height: CGFloat(newMonitorH)))
                captureGroup.leave()
            }
            func moveWindow() throws {
                // Der Monitor sitzt nach der Groessenaenderung an einer neuen Stelle (macOS legt ihn buendig links vom
                // Hauptbildschirm ab): Fenster samt Position neu setzen.
                try windowController.setFrame(
                    windowID: wid, pid: pid,
                    origin: CGPoint(x: vd.bounds.minX, y: vd.bounds.minY + CGFloat(Self.menuBarAllowance)), size: size)
            }
            if shrinking {
                // Verkleinern: Fenster und Monitor gleichzeitig (das Fenster passt auf beide Groessen). macOS legt den
                // Monitor buendig an derselben rechten Kante ab; mit dieser vorhergesagten Position setzt der AX-Aufruf
                // Groesse und Position in einem Rutsch. Stimmt die Vorhersage nicht, kommt eine Korrektur.
                let old = vd.bounds
                let predicted = CGPoint(x: old.maxX - size.width, y: old.minY + CGFloat(Self.menuBarAllowance))
                var winError: Error?
                let g = DispatchGroup()
                g.enter()
                DispatchQueue.global(qos: .userInitiated).async {
                    do { try self.windowController.setFrame(windowID: wid, pid: pid, origin: predicted, size: size) } catch { winError = error }
                    g.leave()
                }
                let ok = vd.resize(widthPoints: Int(size.width), heightPoints: newMonitorH)
                g.wait()
                guard ok else { throw IPCError(.internalError, "Der virtuelle Monitor liess sich nicht auf \(Int(size.width))x\(Int(size.height)) bringen") }
                if let e = winError { throw e }
                lap("Monitor+Fenster")
                let want = CGPoint(x: vd.bounds.minX, y: vd.bounds.minY + CGFloat(Self.menuBarAllowance))
                if abs(want.x - predicted.x) > 1 || abs(want.y - predicted.y) > 1 {
                    try moveWindow()
                    lap("Korrektur")
                }
            } else {
                // Vergroessern: erst der Monitor, sonst kappt macOS das Fenster am alten Rand.
                guard vd.resize(widthPoints: Int(size.width), heightPoints: newMonitorH) else {
                    throw IPCError(.internalError, "Der virtuelle Monitor liess sich nicht auf \(Int(size.width))x\(Int(size.height)) bringen")
                }
                lap("Monitor")
                try moveWindow()
                lap("Fenster")
            }
        } else {
            try windowController.setFrame(windowID: wid, pid: pid, origin: nil, size: size)
            lap("Fenster")
        }
        let expected = c.virtual != nil ? CGSize(width: size.width, height: size.height + CGFloat(Self.menuBarAllowance)) : size
        captureGroup.wait()
        let px = pipe.syncSize(expecting: expected, alreadyChanged: captureChanged)
        lap("Capture")
        return (px.width, px.height)
    }

    /// Raeumt die Fenster- und Monitor-Reste eines Stroms auf: Fenster zurueck an die alte Stelle, virtuellen
    /// Monitor entfernen, Tasten loslassen, Schlaf wieder erlauben.
    private func teardownControl(_ tid: UInt32) {
        lock.lock()
        let c = controls.removeValue(forKey: tid)
        lock.unlock()
        guard let c = c else { return }
        // Zurueck unter den Mauszeiger (restore:true) oder an die alte Stelle.
        func target(_ size: CGSize, old: CGPoint?) -> CGPoint? {
            c.placeAtMouse ? (returnOriginProvider(size) ?? old) : old
        }
        if let vd = c.virtual {
            if let wid = c.windowID { CrashRestore.unregister(windowID: wid) }
            ScreenCaptureSource.clearScaleHint(forDisplay: vd.displayID)
            // Erst den Monitor entfernen, dann das Fenster zurueckstellen: Solange der kleine virtuelle Monitor noch
            // existiert, kappt macOS die Fenstergroesse an seinem Rand.
            vd.destroy()
            if let wid = c.windowID, let pid = c.pid, let f = c.restoreFrame, windowFrameProvider(wid) != nil {
                let origin = target(f.size, old: f.origin) ?? f.origin
                for attempt in 0..<2 {
                    Thread.sleep(forTimeInterval: attempt == 0 ? 0.2 : 0.1)
                    _ = try? runBounded(timeout: 1.0, "Fenster zurueckschieben") {
                        try self.windowController.setFrame(windowID: wid, pid: pid, origin: origin, size: f.size)
                    }
                    if let now = windowFrameProvider(wid), abs(now.width - f.width) <= 2, abs(now.height - f.height) <= 2,
                       abs(now.minX - origin.x) <= 2, abs(now.minY - origin.y) <= 2 { break }
                }
            }
        } else if c.placeAtMouse, let wid = c.windowID, let pid = c.pid, let now = windowFrameProvider(wid),
                  let origin = returnOriginProvider(now.size) {
            // Fenster-Rueckfall mit restore:true: nur die Position, die Groesse bleibt.
            _ = try? runBounded(timeout: 1.0, "Fenster zurueckschieben") {
                try self.windowController.setFrame(windowID: wid, pid: pid, origin: origin, size: nil)
            }
        }
        injector.releaseAll(at: nil)
        sleepGuard.release()
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
        teardownControl(tid)
    }

    private func dropPipelineAsync(_ tid: UInt32) {
        lock.lock()
        let pipe = pipelines.removeValue(forKey: tid)
        let t = transport
        lock.unlock()
        t?.removeStream(id: tid)
        if let p = pipe {
            DispatchQueue.global().async { p.stop(); self.teardownControl(tid) }
        } else {
            teardownControl(tid)
        }
    }

    public func sendStop(tid: UInt32, restore: Bool) throws {
        lock.lock()
        let wasEnded = ended.remove(tid) != nil
        lock.unlock()
        guard pipeline(tid) != nil else {
            if wasEnded { return } // schon per send.gone beendet
            throw IPCError(.notFound, "tid \(tid) unbekannt")
        }
        if restore { control(tid)?.placeAtMouse = true }
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
