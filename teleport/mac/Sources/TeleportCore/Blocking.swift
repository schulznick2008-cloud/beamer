import Foundation

/// Ein blockierender Aufruf ist nicht rechtzeitig zurueckgekehrt.
public struct BlockingTimeout: Error, CustomStringConvertible {
    public let label: String
    public let seconds: TimeInterval
    public var description: String { "\(label) hat nach \(seconds) s nicht geantwortet" }
}

/// Ergebnis-Kasten zwischen dem Arbeiter und dem Wartenden. Genau einer schreibt, der andere liest, beide
/// unter Lock: ein Ergebnis, das nach dem Zeitlimit eintrifft, wird nur abgelegt und von niemandem mehr gelesen.
final class ResultBox<T> {
    private let lock = NSLock()
    private let sem = DispatchSemaphore(value: 0)
    private var value: Result<T, Error>?

    func set(_ r: Result<T, Error>) {
        lock.lock()
        if value == nil { value = r }
        lock.unlock()
        sem.signal()
    }

    /// `nil` bei Zeitueberschreitung.
    func wait(timeout: TimeInterval) -> Result<T, Error>? {
        guard sem.wait(timeout: .now() + timeout) == .success else { return nil }
        lock.lock(); defer { lock.unlock() }
        return value
    }
}

/// Fuehrt eine async-Aufgabe aus und wartet hoechstens `timeout` Sekunden darauf (nie auf dem Hauptthread der
/// Cooperative Pool). Bei Zeitueberschreitung wird die Aufgabe abgebrochen (soweit sie das zulaesst) und
/// `BlockingTimeout` geworfen; ihr spaetes Ergebnis geht verloren. ScreenCaptureKit-Aufrufe kehren unter Last
/// manchmal nie zurueck; ohne Limit haengt dann der aufrufende Thread fuer immer.
func runBlocking<T>(timeout: TimeInterval, _ label: String,
                    _ work: @escaping @Sendable () async throws -> T) throws -> T {
    let box = ResultBox<T>()
    let task = Task.detached {
        do { box.set(.success(try await work())) } catch { box.set(.failure(error)) }
    }
    guard let r = box.wait(timeout: timeout) else {
        task.cancel()
        throw BlockingTimeout(label: label, seconds: timeout)
    }
    return try r.get()
}

/// Dasselbe fuer einen normalen (synchronen) Aufruf, der haengen koennte. Er laeuft auf einem eigenen Thread
/// weiter, wenn das Limit ueberschritten wird; sein Ergebnis wird dann verworfen.
func runBounded<T>(timeout: TimeInterval, _ label: String, _ work: @escaping () throws -> T) throws -> T {
    let box = ResultBox<T>()
    DispatchQueue.global(qos: .userInitiated).async {
        do { box.set(.success(try work())) } catch { box.set(.failure(error)) }
    }
    guard let r = box.wait(timeout: timeout) else { throw BlockingTimeout(label: label, seconds: timeout) }
    return try r.get()
}

/// Prozessuebergreifende Sperre fuer ScreenCaptureKit-Aufrufe. Mehrere Helfer gleichzeitig (zwei Fenster, ein
/// Bildschirm ...) fuehren sonst dazu, dass ein Teil der gleichzeitigen `SCShareableContent`-/`startCapture`-Aufrufe
/// nie zurueckkehrt (im Test: von 3 gleichzeitigen Abfragen kommt nur eine durch, die anderen laufen in den
/// Timeout). Die Aufrufe selbst dauern nur Millisekunden; die Sperre haelt jeweils nur einen Aufruf, nie einen Strom.
enum SCKLock {
    static let path = (ProcessInfo.processInfo.environment["TMPDIR"].map { $0.hasSuffix("/") ? $0 : $0 + "/" } ?? "/tmp/")
        + "beamer-teleport-sck.lock"

    /// Fuehrt `body` unter der Sperre aus. Bekommt der Aufrufer sie nicht innerhalb von `wait` Sekunden (ein anderer
    /// Helfer haengt), laeuft `body` trotzdem, damit ein toter Sperrhalter nichts blockiert.
    static func with<T>(wait: TimeInterval = 4, _ body: () throws -> T) rethrows -> T {
        let fd = open(path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else { return try body() }
        defer { close(fd) } // schliessen gibt die Sperre frei, auch wenn der Prozess stirbt
        let end = Date().addingTimeInterval(wait)
        var locked = false
        while true {
            if flock(fd, LOCK_EX | LOCK_NB) == 0 { locked = true; break }
            if Date() >= end { break }
            usleep(5_000)
        }
        defer { if locked { flock(fd, LOCK_UN) } }
        if ProcessInfo.processInfo.environment["BEAMER_TELEPORT_DEBUG"] == "1" {
            let t0 = Date()
            logErr("sck-lock pid \(getpid()): \(locked ? "gesperrt" : "OHNE Sperre") nach \(Int(Date().timeIntervalSince(end.addingTimeInterval(-wait)) * 1000)) ms")
            defer { logErr("sck-lock pid \(getpid()): Aufruf dauerte \(Int(Date().timeIntervalSince(t0) * 1000)) ms") }
            return try body()
        }
        return try body()
    }
}

/// Wie `runBlocking`, aber der Aufruf selbst (nicht das Warten) steht unter `SCKLock`.
func runBlockingLocked<T>(timeout: TimeInterval, _ label: String,
                          _ work: @escaping @Sendable () async throws -> T) throws -> T {
    try runBlocking(timeout: timeout, label) {
        // Das Sperren blockiert kurz; es darf deshalb nicht im Cooperative Pool stehen.
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<T, Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                SCKLock.with {
                    let sem = DispatchSemaphore(value: 0)
                    let box = ResultBox<T>()
                    Task.detached {
                        do { box.set(.success(try await work())) } catch { box.set(.failure(error)) }
                        sem.signal()
                    }
                    // Die Sperre haelt hoechstens so lange wie das Zeitlimit des Aufrufs.
                    _ = sem.wait(timeout: .now() + timeout)
                    if let r = box.wait(timeout: 0) { c.resume(with: r) } else { c.resume(throwing: BlockingTimeout(label: label, seconds: timeout)) }
                }
            }
        }
    }
}
