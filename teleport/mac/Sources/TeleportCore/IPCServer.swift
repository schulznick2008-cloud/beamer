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
public final class IPCServer {
    private let engine: SenderEngine
    private let dispatcher: Dispatcher
    private let writer: LineWriter

    public init(engine: SenderEngine, writer: LineWriter) {
        self.engine = engine
        self.writer = writer
        self.dispatcher = Dispatcher(backend: engine)
    }

    /// Blockiert bis EOF oder `shutdown`. Raeumt danach auf (Strome stoppen, Transport schliessen).
    public func run(input: Int32 = STDIN_FILENO) {
        var splitter = LineSplitter()
        var buf = [UInt8](repeating: 0, count: 64 * 1024)
        var quit = false
        while !quit {
            let n = read(input, &buf, buf.count)
            if n < 0 {
                if errno == EINTR { continue }
                break
            }
            if n == 0 { break } // EOF
            for line in splitter.feed(Data(buf[0..<n])) {
                if line.trimmingCharacters(in: .whitespaces).isEmpty { continue }
                let r = dispatcher.handle(line: line)
                writer.write(r.line)
                if r.quit { quit = true; break }
            }
            if splitter.oversized > 0 {
                writer.write(Dispatcher.errorEvent(code: .badRequest, msg: "Zeile laenger als 1 MiB verworfen"))
            }
        }
        engine.shutdown()
    }
}
