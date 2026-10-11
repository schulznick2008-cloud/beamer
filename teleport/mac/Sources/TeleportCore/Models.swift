import Foundation

public enum Codec: String {
    case h264
    case hevc

    /// Codec-Bits in `fflags` (Bit 1-3): 1 = h264, 2 = hevc (SCHNITTSTELLEN §4.1).
    public var fflagsBits: UInt8 { self == .h264 ? (1 << 1) : (2 << 1) }
}

public struct WindowInfo: Equatable {
    public var wid: UInt32
    public var pid: Int32
    public var app: String
    public var title: String
    public var x: Double, y: Double, w: Double, h: Double
    public var display: UInt32
    public var scale: Double

    public init(wid: UInt32, pid: Int32, app: String, title: String, x: Double, y: Double,
                w: Double, h: Double, display: UInt32, scale: Double) {
        self.wid = wid; self.pid = pid; self.app = app; self.title = title
        self.x = x; self.y = y; self.w = w; self.h = h; self.display = display; self.scale = scale
    }

    public var json: JSON {
        ["wid": Int(wid), "pid": Int(pid), "app": app, "title": title,
         "x": x, "y": y, "w": w, "h": h, "display": Int(display), "scale": scale]
    }
}

/// Schluessel einer Richtung-Paarung, fertig abgeleitet (SCHNITTSTELLEN §4.2).
public struct StreamKeys: Equatable {
    public var s2rKey: [UInt8]
    public var s2rPrefix: [UInt8]
    public var r2sKey: [UInt8]
    public var r2sPrefix: [UInt8]

    public init(s2rKey: [UInt8], s2rPrefix: [UInt8], r2sKey: [UInt8], r2sPrefix: [UInt8]) {
        self.s2rKey = s2rKey; self.s2rPrefix = s2rPrefix; self.r2sKey = r2sKey; self.r2sPrefix = r2sPrefix
    }

    /// Liest das `keys`-Objekt von `send.start`. Base64 mit Padding, 32/4/32/4 Byte.
    public static func parse(_ j: JSON) throws -> StreamKeys {
        func field(_ name: String, _ len: Int) throws -> [UInt8] {
            guard let s = j.string(name) else {
                throw IPCError(.badRequest, "keys.\(name) fehlt")
            }
            guard let d = Data(base64Encoded: s), d.count == len else {
                throw IPCError(.badRequest, "keys.\(name) muss Base64 von genau \(len) Byte sein")
            }
            return [UInt8](d)
        }
        return StreamKeys(
            s2rKey: try field("s2r_key", 32), s2rPrefix: try field("s2r_prefix", 4),
            r2sKey: try field("r2s_key", 32), r2sPrefix: try field("r2s_prefix", 4))
    }

    /// NUR FUER TESTS: feste Dummy-Schluessel fuer Selbsttests und `tools/teleport/send_demo.py`.
    /// Nie fuer echte Teleports verwenden; die kommen von Python (`core/teleport_keys.py`).
    public static let testOnly = StreamKeys(
        s2rKey: [UInt8](repeating: 0x11, count: 32), s2rPrefix: [0xA1, 0xA2, 0xA3, 0xA4],
        r2sKey: [UInt8](repeating: 0x22, count: 32), r2sPrefix: [0xB1, 0xB2, 0xB3, 0xB4])
}

public enum CaptureMode: Equatable {
    case window(UInt32)
    case display(UInt32)
}

public struct SendParams: Equatable {
    public var tid: UInt32
    public var mode: CaptureMode
    public var codec: Codec
    public var maxFps: Int
    public var bitrateKbps: UInt32
    public var keys: StreamKeys
    /// Fenster auf einen eigenen virtuellen Monitor holen und diesen ganz aufnehmen (Menues und Popover kommen mit).
    public var virtualDisplay: Bool

    public init(tid: UInt32, mode: CaptureMode, codec: Codec, maxFps: Int, bitrateKbps: UInt32, keys: StreamKeys,
                virtualDisplay: Bool = false) {
        self.tid = tid; self.mode = mode; self.codec = codec
        self.maxFps = maxFps; self.bitrateKbps = bitrateKbps; self.keys = keys
        self.virtualDisplay = virtualDisplay
    }

    /// Prueft die Felder von `send.start` (§1.1).
    public static func parse(_ j: JSON) throws -> SendParams {
        guard let tidRaw = j.int("tid"), tidRaw > 0, tidRaw <= Int(UInt32.max) else {
            throw IPCError(.badRequest, "tid muss eine u32 ungleich 0 sein")
        }
        guard let modeStr = j.string("mode") else { throw IPCError(.badRequest, "mode fehlt") }
        let mode: CaptureMode
        switch modeStr {
        case "window":
            guard let w = j.int("wid"), w > 0, w <= Int(UInt32.max) else {
                throw IPCError(.badRequest, "wid fehlt oder ungueltig")
            }
            mode = .window(UInt32(w))
        case "display":
            guard let d = j.int("display"), d >= 0, d <= Int(UInt32.max) else {
                throw IPCError(.badRequest, "display fehlt oder ungueltig")
            }
            mode = .display(UInt32(d))
        default:
            throw IPCError(.badRequest, "mode muss window oder display sein")
        }
        guard let codecStr = j.string("codec"), let codec = Codec(rawValue: codecStr) else {
            throw IPCError(.badRequest, "codec muss hevc oder h264 sein")
        }
        let fps = j.int("max_fps") ?? 60
        guard (1...240).contains(fps) else { throw IPCError(.badRequest, "max_fps muss 1...240 sein") }
        let kbps = j.int("bitrate_kbps") ?? 20_000
        guard (100...1_000_000).contains(kbps) else {
            throw IPCError(.badRequest, "bitrate_kbps muss 100...1000000 sein")
        }
        let virtual = j.bool("virtual_display") ?? false
        if virtual, case .display = mode {
            throw IPCError(.badRequest, "virtual_display gilt nur fuer mode window")
        }
        guard let keysObj = j.dict("keys") else { throw IPCError(.badRequest, "keys fehlt") }
        return SendParams(tid: UInt32(tidRaw), mode: mode, codec: codec, maxFps: fps,
                          bitrateKbps: UInt32(kbps), keys: try StreamKeys.parse(keysObj), virtualDisplay: virtual)
    }
}

public struct SendStarted: Equatable {
    public var width: Int
    public var height: Int
    public var scale: Double
    public init(width: Int, height: Int, scale: Double) {
        self.width = width; self.height = height; self.scale = scale
    }
}
