import CBeamerTransport
import Foundation

public struct ReceivedFrame {
    public var streamId: UInt32
    public var kind: UInt8
    public var data: [UInt8]
    public var ptsUS: UInt64
    public var fflags: UInt8
    public var frameId: UInt32
}

public struct TransportError: Error, CustomStringConvertible {
    public let message: String
    public var description: String { message }
}

/// Duenne Swift-Huelle um `beamer_transport.h` (C-ABI aus `teleport/transport`).
/// Die Rueckrufe laufen im Transport-Thread und muessen schnell zurueckkehren.
public final class Transport {
    public enum Role: UInt32 {
        case sender = 1
        case receiver = 2
    }

    private var session: OpaquePointer?
    public var onFrame: ((ReceivedFrame) -> Void)?
    public var onControl: ((UInt32, UInt8) -> Void)?
    public var onStats: ((UInt32, String) -> Void)?

    static func lastError() -> String {
        guard let p = bt_last_error() else { return "unbekannter Fehler" }
        let s = String(cString: p)
        return s.isEmpty ? "unbekannter Fehler" : s
    }

    public init(localPort: UInt16) throws {
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        let frameCB: BtOnFrame = { ctx, sid, kind, f in
            guard let ctx = ctx, let f = f?.pointee else { return }
            let t = Unmanaged<Transport>.fromOpaque(ctx).takeUnretainedValue()
            guard let cb = t.onFrame else { return }
            let bytes = f.data.map { Array(UnsafeBufferPointer(start: $0, count: f.len)) } ?? []
            cb(ReceivedFrame(streamId: sid, kind: kind, data: bytes, ptsUS: f.pts_us, fflags: f.fflags, frameId: f.frame_id))
        }
        let controlCB: BtOnControl = { ctx, sid, type in
            guard let ctx = ctx else { return }
            Unmanaged<Transport>.fromOpaque(ctx).takeUnretainedValue().onControl?(sid, type)
        }
        let statsCB: BtOnStats = { ctx, sid, json in
            guard let ctx = ctx, let json = json else { return }
            Unmanaged<Transport>.fromOpaque(ctx).takeUnretainedValue().onStats?(sid, String(cString: json))
        }
        guard let s = bt_open(localPort, frameCB, controlCB, statsCB, ctx) else {
            throw TransportError(message: "bt_open: \(Transport.lastError())")
        }
        session = s
    }

    deinit { close() }

    public var localPort: UInt16 { session.map { bt_local_port($0) } ?? 0 }

    private func check(_ rc: Int32, _ what: String) throws {
        if rc < 0 { throw TransportError(message: "\(what): \(Transport.lastError())") }
    }

    private func live() throws -> OpaquePointer {
        guard let s = session else { throw TransportError(message: "Transport ist geschlossen") }
        return s
    }

    /// Port 0 heisst: Gegenstelle noch unbekannt. Darf auch bei laufenden Streams aufgerufen werden.
    public func setPeer(host: String, port: UInt16) throws {
        try check(bt_set_peer(try live(), host, port), "bt_set_peer")
    }

    public func addStream(id: UInt32, role: Role, keys: StreamKeys, maxKbps: UInt32) throws {
        let s = try live()
        try check(
            keys.s2rKey.withUnsafeBufferPointer { a in
                keys.s2rPrefix.withUnsafeBufferPointer { b in
                    keys.r2sKey.withUnsafeBufferPointer { c in
                        keys.r2sPrefix.withUnsafeBufferPointer { d in
                            bt_add_stream(s, id, role.rawValue, a.baseAddress, b.baseAddress,
                                          c.baseAddress, d.baseAddress, maxKbps)
                        }
                    }
                }
            }, "bt_add_stream")
    }

    public func removeStream(id: UInt32) {
        guard let s = session else { return }
        _ = bt_remove_stream(s, id)
    }

    /// `false`, wenn der Transport das Frame ablehnt (z. B. "Sendewarteschlange voll"); dann auslassen.
    @discardableResult
    public func sendFrame(stream: UInt32, kind: UInt8 = 1, data: [UInt8], ptsUS: UInt64, fflags: UInt8) -> Bool {
        guard let s = session else { return false }
        return data.withUnsafeBufferPointer { p in
            bt_send_frame(s, stream, kind, p.baseAddress, p.count, ptsUS, fflags) == 0
        }
    }

    public func requestKeyframe(stream: UInt32) {
        guard let s = session else { return }
        _ = bt_request_keyframe(s, stream)
    }

    public func targetKbps(stream: UInt32) -> UInt32 {
        session.map { bt_target_kbps($0, stream) } ?? 0
    }

    public func setMaxKbps(stream: UInt32, kbps: UInt32) throws {
        try check(bt_set_max_kbps(try live(), stream, kbps), "bt_set_max_kbps")
    }

    public func close() {
        guard let s = session else { return }
        session = nil
        bt_close(s)
    }
}
