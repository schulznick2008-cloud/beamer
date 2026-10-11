import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

public struct DecoderError: Error, CustomStringConvertible {
    public let message: String
    public var description: String { message }
}

/// Dekodiert Annex-B-Zugriffseinheiten mit VideoToolbox (nur fuer den Selbsttest-Empfaenger, ohne Anzeige).
public final class VideoDecoder {
    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var paramKey: [[UInt8]] = []
    private let codec: Codec
    private let onDecoded: (UInt64) -> Void
    public private(set) var errors = 0

    public init(codec: Codec, onDecoded: @escaping (UInt64) -> Void) {
        self.codec = codec
        self.onDecoded = onDecoded
    }

    deinit { invalidate() }

    public func invalidate() {
        if let s = session {
            VTDecompressionSessionWaitForAsynchronousFrames(s)
            VTDecompressionSessionInvalidate(s)
        }
        session = nil
    }

    private func makeFormat(_ params: [[UInt8]]) throws {
        var fd: CMVideoFormatDescription?
        let st: OSStatus = withPointers(params) { ptrs, sizes in
            if codec == .hevc {
                return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    allocator: nil, parameterSetCount: params.count, parameterSetPointers: ptrs,
                    parameterSetSizes: sizes, nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &fd)
            }
            return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                allocator: nil, parameterSetCount: params.count, parameterSetPointers: ptrs,
                parameterSetSizes: sizes, nalUnitHeaderLength: 4, formatDescriptionOut: &fd)
        }
        guard st == noErr, let f = fd else { throw DecoderError(message: "Formatbeschreibung: OSStatus \(st)") }
        invalidate()
        var s: VTDecompressionSession?
        let attrs: [CFString: Any] = [kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
        let st2 = VTDecompressionSessionCreate(
            allocator: nil, formatDescription: f, decoderSpecification: nil,
            imageBufferAttributes: attrs as CFDictionary, outputCallback: nil, decompressionSessionOut: &s)
        guard st2 == noErr, let sess = s else { throw DecoderError(message: "Decoder-Session: OSStatus \(st2)") }
        _ = VTSessionSetProperty(sess, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        session = sess
        format = f
        paramKey = params
    }

    private func withPointers<R>(_ sets: [[UInt8]], _ body: ([UnsafePointer<UInt8>], [Int]) -> R) -> R {
        // Alle Parametersaetze am Stueck halten, solange `body` laeuft.
        let flat = sets.flatMap { $0 }
        return flat.withUnsafeBufferPointer { buf in
            var ptrs: [UnsafePointer<UInt8>] = []
            var off = 0
            for s in sets {
                ptrs.append(buf.baseAddress! + off)
                off += s.count
            }
            return body(ptrs, sets.map { $0.count })
        }
    }

    public func decode(annexB: [UInt8], ptsUS: UInt64) throws {
        let nals = AnnexB.split(annexB)
        let params = nals.filter { AnnexB.isParameterSet($0, codec: codec) }
        if !params.isEmpty, params != paramKey { try makeFormat(params) }
        guard let sess = session, let fmt = format else { throw DecoderError(message: "noch kein Keyframe") }
        let slices = nals.filter { !AnnexB.isParameterSet($0, codec: codec) }
        guard !slices.isEmpty else { return }
        let avcc = AnnexB.toAVCC(nals: slices)

        var block: CMBlockBuffer?
        var st = CMBlockBufferCreateWithMemoryBlock(
            allocator: nil, memoryBlock: nil, blockLength: avcc.count, blockAllocator: nil,
            customBlockSource: nil, offsetToData: 0, dataLength: avcc.count,
            flags: kCMBlockBufferAssureMemoryNowFlag, blockBufferOut: &block)
        guard st == noErr, let b = block else { throw DecoderError(message: "BlockBuffer: \(st)") }
        st = avcc.withUnsafeBytes { CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: b, offsetIntoDestination: 0, dataLength: avcc.count) }
        guard st == noErr else { throw DecoderError(message: "BlockBuffer fuellen: \(st)") }

        var sample: CMSampleBuffer?
        var timing = CMSampleTimingInfo(duration: .invalid,
                                        presentationTimeStamp: CMTime(value: Int64(ptsUS), timescale: 1_000_000),
                                        decodeTimeStamp: .invalid)
        var size = avcc.count
        st = CMSampleBufferCreateReady(
            allocator: nil, dataBuffer: b, formatDescription: fmt, sampleCount: 1, sampleTimingEntryCount: 1,
            sampleTimingArray: &timing, sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample)
        guard st == noErr, let sb = sample else { throw DecoderError(message: "SampleBuffer: \(st)") }

        let cb = onDecoded
        var flags = VTDecodeInfoFlags()
        st = VTDecompressionSessionDecodeFrame(
            sess, sampleBuffer: sb, flags: [._EnableAsynchronousDecompression], infoFlagsOut: &flags
        ) { status, _, image, pts, _ in
            guard status == noErr, image != nil else { return }
            cb(UInt64(max(0, pts.seconds * 1_000_000)))
        }
        if st != noErr {
            errors += 1
            throw DecoderError(message: "Decode: OSStatus \(st)")
        }
    }
}
