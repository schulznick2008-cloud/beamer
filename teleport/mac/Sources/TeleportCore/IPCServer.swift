import Foundation

/// stdout-Schreiber: genau eine Zeile je Nachricht, von mehreren Threads sicher. Auf stdout steht nichts
/// ausser Protokollzeilen; Logs gehen nach stderr.
public final class LineWriter {
    private let lock = NSLock()
    private let fd: Int32
    public init(fd: Int32 = STDOUT_FILENO) { self.fd = fd }

    public func write(_ line: String) {
        var bytes = Array(line.utf8)
        bytes.append(0x0A)
        lock.lock(); defer { lock.unlock() }
        var off = 0
        while off < bytes.count {
            let n = bytes.withUnsafeBytes { Darwin.write(fd, $0.baseAddress! + off, bytes.count - off) }
            if n < 0 {
                if errno == EINTR { continue }
                return // Leser weg (EPIPE): nichts mehr zu tun
            }
            off += n
        }
    }
}

public func logErr(_ s: String) {
    FileHandle.standardError.write(Data((s + "\n").utf8))
}

/// IPC-Schleife: liest Zeilen von `input`, antwortet ueber `writer`. Endet bei EOF oder `shutdown`.
///
/// Gelesen wird auf einem eigenen Thread, die Befehle laufen nacheinander auf einer Queue. So bemerkt der Helfer
/// EOF auch dann, wenn gerade ein Befehl haengt (ScreenCaptureKit kehrt unter Last manchmal nicht zurueck). Beim
/// Beenden (EOF oder `shutdown`) laeuft ein Wachhund: Ist nach `watchdogSeconds` nicht aufgeraeumt, endet der
/// Prozess trotzdem mit `exit(0)`. Vorgabe aus SCHNITTSTELLEN §1: nach EOF Aufraeumen und Ende in unter 2 s.
public final class IPCServer {
    private let engine: SenderEngine
    private let dispatcher: Dispatcher
    private let writer: LineWriter
    private let exec = DispatchQueue(label: "teleport.ipc.exec")
    private let finished = DispatchSemaphore(value: 0)
    private let stateLock = NSLock()
    private var shuttingDown = false

    public var watchdogSeconds = 1.8
    public var exitHandler: (Int32) -> Void = { Darwin.exit($0) }

    public init(engine: SenderEngine, writer: LineWriter) {
        self.engine = engine
        self.writer = writer
        self.dispatcher = Dispatcher(backend: engine)
    }

    /// Blockiert bis aufgeraeumt ist (oder der Wachhund zuschlaegt).
    public func run(input: Int32 = STDIN_FILENO) {
        let reader = Thread { [self] in readLoop(input) }
        reader.name = "teleport.ipc.reader"
        reader.start()
        finished.wait()
    }

    private func readLoop(_ input: Int32) {
        var splitter = LineSplitter()
        var buf = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let n = read(input, &buf, buf.count)
            if n < 0 {
                if errno == EINTR { continue }
                break
            }
            if n == 0 { break } // EOF
            for line in splitter.feed(Data(buf[0..<n])) {
                if line.trimmingCharacters(in: .whitespaces).isEmpty { continue }
                exec.async { [self] in
                    if isShuttingDown() { return }
                    let r = dispatcher.handle(line: line)
                    writer.write(r.line)
                    if r.quit { beginShutdown() }
                }
            }
            if splitter.oversized > 0 {
                writer.write(Dispatcher.errorEvent(code: .badRequest, msg: "Zeile laenger als 1 MiB verworfen"))
            }
        }
        beginShutdown()
    }

    private func isShuttingDown() -> Bool {
        stateLock.lock(); defer { stateLock.unlock() }
        return shuttingDown
    }

    private func beginShutdown() {
        stateLock.lock()
        let already = shuttingDown
        shuttingDown = true
        stateLock.unlock()
        if already { return }
        // Wachhund zuerst: kommt der normale Weg nicht zum Ende (haengender Befehl in der Queue), endet der Prozess hier.
        let handler = exitHandler
        DispatchQueue.global().asyncAfter(deadline: .now() + watchdogSeconds) { handler(0) }
        // Aufraeumen auf eigenem Thread, nicht hinter einem haengenden Befehl in `exec`.
        DispatchQueue.global().async { [self] in
            engine.shutdown()
            finished.signal()
        }
    }
}
