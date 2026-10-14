import Foundation

/// Was der Befehlsverteiler vom eigentlichen Helfer braucht. Die echte Umsetzung ist `SenderEngine`,
/// die Tests benutzen eine Attrappe.
public protocol TeleportBackend: AnyObject {
    func transportOpen(localPort: UInt16, peerHost: String?, peerPort: UInt16) throws -> UInt16
    func transportPeer(host: String, port: UInt16) throws
    func transportClose()
    func listWindows() throws -> [WindowInfo]
    func sendStart(_ p: SendParams) throws -> SendStarted
    func sendStop(tid: UInt32, restore: Bool) throws
    func sendKeyframe(tid: UInt32) throws
    func sendBitrate(tid: UInt32, kbps: UInt32) throws
    func inject(tid: UInt32, input: InputEvent) throws
    func sendActivate(tid: UInt32) throws
    /// Groesse in Punkten; liefert die neue Stream-Groesse in Pixeln.
    func sendResize(tid: UInt32, width: Double, height: Double) throws -> (width: Int, height: Int)
    func shutdown()
}

/// Verarbeitet eine IPC-Zeile (SCHNITTSTELLEN §1) und liefert die Antwortzeile.
public final class Dispatcher {
    public static let protocolVersion = 1
    public static let helperVersion = "0.1.0"
    public static let caps = ["send_window", "send_display", "inject"]

    public struct Result: Equatable {
        public var line: String
        public var quit: Bool
    }

    private let backend: TeleportBackend
    private let lowPower: () -> Bool

    public init(backend: TeleportBackend,
                lowPower: @escaping () -> Bool = { ProcessInfo.processInfo.isLowPowerModeEnabled }) {
        self.backend = backend
        self.lowPower = lowPower
    }

    /// Fehlerereignis fuer Zeilen, die sich keiner Anfrage zuordnen lassen.
    public static func errorEvent(code: IPCErrorCode, msg: String, tid: UInt32? = nil) -> String {
        var e: JSON = ["v": protocolVersion, "ev": "error", "code": code.rawValue, "msg": msg]
        if let tid = tid { e["tid"] = Int(tid) }
        return JSONLine.encode(e)
    }

    public func handle(line: String) -> Result {
        let msg: JSON
        do {
            msg = try JSONLine.parse(line)
        } catch let e as IPCError {
            return Result(line: Self.errorEvent(code: e.code, msg: e.msg), quit: false)
        } catch {
            return Result(line: Self.errorEvent(code: .badRequest, msg: "\(error)"), quit: false)
        }
        guard let id = msg.int("id") else {
            return Result(line: Self.errorEvent(code: .badRequest, msg: "id (Ganzzahl) fehlt"), quit: false)
        }
        if let v = msg.int("v"), v != Self.protocolVersion {
            return reply(id, error: IPCError(.badRequest, "unbekannte Protokollversion \(v)"))
        }
        guard let cmd = msg.string("cmd") else {
            return reply(id, error: IPCError(.badRequest, "cmd fehlt"))
        }
        do {
            var quit = false
            let extra = try run(cmd: cmd, msg: msg, quit: &quit)
            return Result(line: okLine(id, extra), quit: quit)
        } catch let e as IPCError {
            return reply(id, error: e)
        } catch {
            return reply(id, error: IPCError(.internalError, "\(error)"))
        }
    }

    private func run(cmd: String, msg: JSON, quit: inout Bool) throws -> JSON {
        switch cmd {
        case "hello":
            // low_power (Zusatzfeld): Energiesparmodus an. Die Codec-Wahl trifft Python, der Helfer meldet nur.
            return ["helper_version": Self.helperVersion, "platform": "mac", "caps": Self.caps, "low_power": lowPower()]

        case "transport.open":
            guard let lp = msg.int("local_port"), (0...65535).contains(lp) else {
                throw IPCError(.badRequest, "local_port muss 0...65535 sein")
            }
            let host = msg.string("peer_host").flatMap { $0.isEmpty ? nil : $0 }
            let pp = msg.int("peer_port") ?? 0
            guard (0...65535).contains(pp) else { throw IPCError(.badRequest, "peer_port muss 0...65535 sein") }
            return ["local_port": Int(try backend.transportOpen(localPort: UInt16(lp), peerHost: host, peerPort: UInt16(pp)))]

        case "transport.peer":
            guard let host = msg.string("peer_host"), !host.isEmpty else {
                throw IPCError(.badRequest, "peer_host fehlt")
            }
            guard let pp = msg.int("peer_port"), (0...65535).contains(pp) else {
                throw IPCError(.badRequest, "peer_port muss 0...65535 sein")
            }
            try backend.transportPeer(host: host, port: UInt16(pp))
            return [:]

        case "transport.close":
            backend.transportClose()
            return [:]

        case "windows.list":
            return ["windows": try backend.listWindows().map { $0.json }]

        case "windows.chrome":
            guard let w = msg.int("wid"), w > 0, w <= Int(UInt32.max) else {
                throw IPCError(.badRequest, "wid muss eine u32 ungleich 0 sein")
            }
            if let engine = backend as? SenderEngine {
                return engine.windowChrome(windowID: UInt32(w))
            }
            return ["title_bar": false, "dark": true]

        case "send.start":
            let p = try SendParams.parse(msg)
            let s = try backend.sendStart(p)
            return ["width": s.width, "height": s.height, "scale": s.scale]

        case "send.stop":
            try backend.sendStop(tid: try tid(msg), restore: msg.bool("restore") ?? false)
            return [:]

        case "send.keyframe":
            try backend.sendKeyframe(tid: try tid(msg))
            return [:]

        case "send.bitrate":
            guard let k = msg.int("bitrate_kbps"), (100...1_000_000).contains(k) else {
                throw IPCError(.badRequest, "bitrate_kbps muss 100...1000000 sein")
            }
            try backend.sendBitrate(tid: try tid(msg), kbps: UInt32(k))
            return [:]

        case "inject":
            guard let input = msg.dict("input") else { throw IPCError(.badRequest, "input fehlt") }
            try backend.inject(tid: try tid(msg), input: try InputEvent.parse(input))
            return [:]

        case "send.activate":
            try backend.sendActivate(tid: try tid(msg))
            return [:]

        case "send.resize":
            func points(_ k: String) throws -> Double {
                guard let n = msg[k] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID(), n.doubleValue.isFinite,
                      n.doubleValue >= 50, n.doubleValue <= 8000 else {
                    throw IPCError(.badRequest, "\(k) muss eine Zahl zwischen 50 und 8000 (Punkte) sein")
                }
                return n.doubleValue
            }
            let r = try backend.sendResize(tid: try tid(msg), width: try points("w"), height: try points("h"))
            return ["width": r.width, "height": r.height]

        case "shutdown":
            quit = true // das Aufraeumen (backend.shutdown) macht der IPC-Server nach der Antwort, mit Zeitlimit
            return [:]

        default:
            throw IPCError(.unsupported, "Befehl \(cmd) wird vom Mac-Sender-Helfer nicht unterstuetzt")
        }
    }

    private func tid(_ msg: JSON) throws -> UInt32 {
        guard let t = msg.int("tid"), t > 0, t <= Int(UInt32.max) else {
            throw IPCError(.badRequest, "tid muss eine u32 ungleich 0 sein")
        }
        return UInt32(t)
    }

    private func okLine(_ id: Int, _ extra: JSON) -> String {
        var r: JSON = extra
        r["v"] = Self.protocolVersion
        r["re"] = id
        r["ok"] = true
        return JSONLine.encode(r)
    }

    private func reply(_ id: Int, error: IPCError) -> Result {
        let r: JSON = [
            "v": Self.protocolVersion, "re": id, "ok": false,
            "error": ["code": error.code.rawValue, "msg": error.msg],
        ]
        return Result(line: JSONLine.encode(r), quit: false)
    }
}
