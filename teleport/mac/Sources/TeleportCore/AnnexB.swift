import Foundation

/// Umwandlung zwischen den Laenge-Praefix-Paketen (AVCC/HVCC) von VideoToolbox und Annex-B
/// (Startcodes), wie es SCHNITTSTELLEN §4.1 fuer den Strom verlangt.
public enum AnnexB {
    public static let startCode: [UInt8] = [0, 0, 0, 1]

    public enum Failure: Error, Equatable {
        case badLengthSize
        case truncated
        case empty
    }

    /// Wandelt ein AVCC-Sample (`lengthSize`-Byte lange Big-Endian-Laengen vor jeder NAL) in Annex-B.
    /// Bei `parameterSets` (nur bei Keyframes uebergeben) stehen diese vor den NALs, jeweils mit Startcode.
    public static func convert(avcc: [UInt8], lengthSize: Int, parameterSets: [[UInt8]] = []) throws -> [UInt8] {
        guard (1...4).contains(lengthSize) else { throw Failure.badLengthSize }
        var out: [UInt8] = []
        out.reserveCapacity(avcc.count + parameterSets.reduce(0) { $0 + $1.count + 4 } + 16)
        for ps in parameterSets where !ps.isEmpty {
            out.append(contentsOf: startCode)
            out.append(contentsOf: ps)
        }
        var i = 0
        var nals = 0
        while i < avcc.count {
            guard i + lengthSize <= avcc.count else { throw Failure.truncated }
            var len = 0
            for k in 0..<lengthSize { len = (len << 8) | Int(avcc[i + k]) }
            i += lengthSize
            guard len > 0, i + len <= avcc.count else { throw Failure.truncated }
            out.append(contentsOf: startCode)
            out.append(contentsOf: avcc[i..<(i + len)])
            i += len
            nals += 1
        }
        guard nals > 0 else { throw Failure.empty }
        return out
    }

    /// Zerlegt Annex-B in NALs (ohne Startcodes). Akzeptiert 3- und 4-Byte-Startcodes.
    public static func split(_ data: [UInt8]) -> [[UInt8]] {
        var starts: [(code: Int, payload: Int)] = []
        var i = 0
        let n = data.count
        while i + 3 <= n {
            if data[i] == 0, data[i + 1] == 0, data[i + 2] == 1 {
                starts.append((i, i + 3))
                i += 3
            } else {
                i += 1
            }
        }
        var nals: [[UInt8]] = []
        for (idx, s) in starts.enumerated() {
            var end = idx + 1 < starts.count ? starts[idx + 1].code : n
            // Ein vierter Startcode-Byte (00 00 00 01) gehoert nicht mehr zur vorigen NAL.
            while end > s.payload, data[end - 1] == 0, idx + 1 < starts.count { end -= 1 }
            if end > s.payload { nals.append(Array(data[s.payload..<end])) }
        }
        return nals
    }

    /// Wandelt Annex-B-NALs (ohne Parametersaetze) in AVCC mit 4-Byte-Laengen, fuer den Decoder.
    public static func toAVCC(nals: [[UInt8]]) -> [UInt8] {
        var out: [UInt8] = []
        for nal in nals {
            let l = UInt32(nal.count)
            out.append(contentsOf: [UInt8(l >> 24), UInt8((l >> 16) & 0xFF), UInt8((l >> 8) & 0xFF), UInt8(l & 0xFF)])
            out.append(contentsOf: nal)
        }
        return out
    }

    /// NAL-Typ: H.264 = Bits 0-4 des ersten Bytes, HEVC = Bits 1-6.
    public static func nalType(_ nal: [UInt8], codec: Codec) -> Int {
        guard let b = nal.first else { return -1 }
        return codec == .h264 ? Int(b & 0x1F) : Int((b >> 1) & 0x3F)
    }

    public static func isParameterSet(_ nal: [UInt8], codec: Codec) -> Bool {
        let t = nalType(nal, codec: codec)
        return codec == .h264 ? (t == 7 || t == 8) : (t >= 32 && t <= 34)
    }

    /// Enthaelt die Zugriffseinheit ein IDR-Bild?
    public static func containsKeyframe(_ nals: [[UInt8]], codec: Codec) -> Bool {
        nals.contains { n in
            let t = nalType(n, codec: codec)
            return codec == .h264 ? t == 5 : (t >= 16 && t <= 21)
        }
    }
}
