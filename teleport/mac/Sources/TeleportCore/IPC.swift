import Foundation

/// JSON-Objekt der IPC-Nachrichten (SCHNITTSTELLEN.md §1).
public typealias JSON = [String: Any]

public enum IPCErrorCode: String {
    case badRequest = "bad_request"
    case notFound = "not_found"
    case permission
    case unsupported
    case busy
    case internalError = "internal"
}

/// Fehler, der als `{"ok":false,"error":{...}}` zurueckgeht.
public struct IPCError: Error, Equatable {
    public let code: IPCErrorCode
    public let msg: String
    public init(_ code: IPCErrorCode, _ msg: String) {
        self.code = code
        self.msg = msg
    }
}

extension Dictionary where Key == String, Value == Any {
    /// Ganzzahl aus einem JSON-Wert (Zahlen, die genau ganzzahlig sind; keine Bools).
    public func int(_ key: String) -> Int? {
        guard let v = self[key] else { return nil }
        if let n = v as? NSNumber {
            if CFGetTypeID(n) == CFBooleanGetTypeID() { return nil }
            let d = n.doubleValue
            guard d.isFinite, d == d.rounded(), abs(d) < 9.0e15 else { return nil }
            return n.intValue
        }
        return nil
    }

    public func string(_ key: String) -> String? { self[key] as? String }

    public func bool(_ key: String) -> Bool? {
        guard let n = self[key] as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else { return nil }
        return n.boolValue
    }

    public func dict(_ key: String) -> JSON? { self[key] as? JSON }
}

public enum JSONLine {
    /// Parst genau ein JSON-Objekt aus einer Zeile.
    public static func parse(_ line: String) throws -> JSON {
        guard let data = line.data(using: .utf8) else {
            throw IPCError(.badRequest, "Zeile ist kein UTF-8")
        }
        let obj: Any
        do {
            obj = try JSONSerialization.jsonObject(with: data, options: [])
        } catch {
            throw IPCError(.badRequest, "kein gueltiges JSON: \(error.localizedDescription)")
        }
        guard let dict = obj as? JSON else {
            throw IPCError(.badRequest, "JSON-Objekt erwartet")
        }
        return dict
    }

    /// Eine Zeile ohne Zeilenumbruch. NaN und Unendlich (nicht erlaubt) werden zu 0.
    public static func encode(_ obj: JSON) -> String {
        let clean = sanitize(obj)
        guard JSONSerialization.isValidJSONObject(clean),
              let data = try? JSONSerialization.data(
                withJSONObject: clean, options: [.sortedKeys, .withoutEscapingSlashes]),
              let s = String(data: data, encoding: .utf8)
        else {
            return "{\"v\":1,\"ev\":\"error\",\"code\":\"internal\",\"msg\":\"Nachricht nicht serialisierbar\"}"
        }
        return s
    }

    static func sanitize(_ v: Any) -> Any {
        if let d = v as? [String: Any] { return d.mapValues { sanitize($0) } }
        if let a = v as? [Any] { return a.map { sanitize($0) } }
        if let d = v as? Double, !d.isFinite { return 0 }
        if let f = v as? Float, !f.isFinite { return 0 }
        return v
    }
}

/// Zerlegt einen Bytestrom in Zeilen (`\n`, ein davor stehendes `\r` faellt weg). Teilzeilen bleiben stehen.
public struct LineSplitter {
    public static let maxLine = 1 << 20
    private var buffer = Data()
    /// Zeile zu lang: der Rest bis zum naechsten Zeilenende wird verworfen.
    private var discarding = false
    public private(set) var oversized = 0

    public init() {}

    public mutating func feed(_ data: Data) -> [String] {
        var lines: [String] = []
        for byte in data {
            if byte == 0x0A {
                if discarding {
                    discarding = false
                } else if let s = String(data: buffer, encoding: .utf8) {
                    lines.append(s.hasSuffix("\r") ? String(s.dropLast()) : s)
                } else {
                    lines.append("\u{FFFD}") // ungueltiges UTF-8: der Parser meldet bad_request
                }
                buffer.removeAll(keepingCapacity: true)
            } else if !discarding {
                buffer.append(byte)
                if buffer.count > Self.maxLine {
                    buffer.removeAll()
                    discarding = true
                    oversized += 1
                }
            }
        }
        return lines
    }
}
