import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

public struct EncodedFrame {
    public var annexB: [UInt8]
    public var isKeyframe: Bool
    public var ptsUS: UInt64
    public var encodeMS: Double
}

public struct EncoderError: Error, CustomStringConvertible {
    public let message: String
    public var description: String { message }
}

/// VideoToolbox-Kompression: Hardware-Encoder Pflicht, Low-Latency-Ratenregelung, keine B-Frames,
/// Keyframe alle 2 s und auf Anfrage. Ausgabe als Annex-B mit Parametersaetzen vor jedem Keyframe.
public final class VideoEncoder {
    private var session: VTCompressionSession?
    private let codec: Codec
    public let width: Int
    public let height: Int
    private let fps: Int
    private let onFrame: (EncodedFrame) -> Void
    public private(set) var usesHardware = false
    private var bitrateKbps: Int

    public init(width: Int, height: Int, codec: Codec, fps: Int, bitrateKbps: Int,
                onFrame: @escaping (EncodedFrame) -> Void) throws {
        self.width = width
        self.height = height
        self.codec = codec
        self.fps = fps
        self.bitrateKbps = bitrateKbps
        self.onFrame = onFrame

        let spec: [CFString: Any] = [
            kVTVideoEncoderSpecification_EnableLowLatencyRateControl: true,
            kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: true,
        ]
        var s: VTCompressionSession?
        let type: CMVideoCodecType = codec == .hevc ? kCMVideoCodecType_HEVC : kCMVideoCodecType_H264
        let st = VTCompressionSessionCreate(
            allocator: nil, width: Int32(width), height: Int32(height), codecType: type,
            encoderSpecification: spec as CFDictionary, imageBufferAttributes: nil,
            compressedDataAllocator: nil, outputCallback: nil, refcon: nil, compressionSessionOut: &s)
        guard st == noErr, let sess = s else {
            throw EncoderError(message: "Hardware-Encoder (\(codec.rawValue), \(width)x\(height)) nicht verfuegbar: OSStatus \(st)")
        }
        session = sess

        var hw: CFTypeRef?
        let rc = withUnsafeMutablePointer(to: &hw) { p in
            VTSessionCopyProperty(sess, key: kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                                  allocator: nil, valueOut: UnsafeMutableRawPointer(p))
        }
        if rc == noErr, let v = hw, CFGetTypeID(v) == CFBooleanGetTypeID() {
            usesHardware = CFBooleanGetValue(unsafeBitCast(v, to: CFBoolean.self))
        } else {
            usesHardware = true // Hardware war Bedingung beim Anlegen der Session
        }
        guard usesHardware else {
            VTCompressionSessionInvalidate(sess)
            session = nil
            throw EncoderError(message: "Der Encoder laeuft nicht in Hardware (Pflicht fuer Teleport)")
        }

        func set(_ key: CFString, _ value: CFTypeRef) {
            _ = VTSessionSetProperty(sess, key: key, value: value) // nicht jede Eigenschaft kennt jeder Modus
        }
        set(kVTCompressionPropertyKey_RealTime, kCFBooleanTrue)
        set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse)
        set(kVTCompressionPropertyKey_ExpectedFrameRate, fps as CFNumber)
        set(kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, 2 as CFNumber)
        set(kVTCompressionPropertyKey_MaxKeyFrameInterval, (fps * 2) as CFNumber)
        set(kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, kCFBooleanTrue)
        set(kVTCompressionPropertyKey_ProfileLevel,
            codec == .hevc ? kVTProfileLevel_HEVC_Main_AutoLevel : kVTProfileLevel_H264_High_AutoLevel)
        set(kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2)
        set(kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2)
        set(kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2)
        applyBitrate(kbps: bitrateKbps)
        VTCompressionSessionPrepareToEncodeFrames(sess)
    }

    deinit { invalidate() }

    private func applyBitrate(kbps: Int) {
        guard let sess = session else { return }
        let bps = max(100, kbps) * 1000
        _ = VTSessionSetProperty(sess, key: kVTCompressionPropertyKey_AverageBitRate, value: bps as CFNumber)
        // Hoechstens ~1,5 x Mittelwert in jeder Sekunde (Bytes je Sekunde).
        let limit = [Int(Double(bps) / 8.0 * 1.5), 1] as CFArray
        _ = VTSessionSetProperty(sess, key: kVTCompressionPropertyKey_DataRateLimits, value: limit)
    }

    /// Alle ~1 s mit `bt_target_kbps` aufrufen.
    public func setBitrate(kbps: Int) {
        guard kbps != bitrateKbps else { return }
        bitrateKbps = kbps
        applyBitrate(kbps: kbps)
    }

    public func encode(_ pixelBuffer: CVPixelBuffer, hostTime: CMTime, forceKeyframe: Bool) {
        guard let sess = session else { return }
        var props: CFDictionary?
        if forceKeyframe {
            props = [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary
        }
        let started = DispatchTime.now().uptimeNanoseconds
        let codec = self.codec
        let onFrame = self.onFrame
        let ptsUS = UInt64(max(0, hostTime.seconds * 1_000_000))
        var flags = VTEncodeInfoFlags()
        let st = VTCompressionSessionEncodeFrame(
            sess, imageBuffer: pixelBuffer, presentationTimeStamp: hostTime,
            duration: .invalid, frameProperties: props, infoFlagsOut: &flags
        ) { status, _, sample in
            guard status == noErr, let sample = sample,
                  let enc = VideoEncoder.annexB(from: sample, codec: codec, ptsUS: ptsUS) else { return }
            var e = enc
            e.encodeMS = Double(DispatchTime.now().uptimeNanoseconds - started) / 1e6
            onFrame(e)
        }
        _ = st
    }

    public func flush() {
        guard let sess = session else { return }
        VTCompressionSessionCompleteFrames(sess, untilPresentationTimeStamp: .invalid)
    }

    public func invalidate() {
        guard let sess = session else { return }
        session = nil
        VTCompressionSessionCompleteFrames(sess, untilPresentationTimeStamp: .invalid)
        VTCompressionSessionInvalidate(sess)
    }

    // MARK: Annex-B

    static func annexB(from sample: CMSampleBuffer, codec: Codec, ptsUS: UInt64) -> EncodedFrame? {
        guard let fd = CMSampleBufferGetFormatDescription(sample),
              let block = CMSampleBufferGetDataBuffer(sample) else { return nil }
        var isKey = true
        if let atts = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[CFString: Any]],
           let first = atts.first, let notSync = first[kCMSampleAttachmentKey_NotSync] as? Bool {
            isKey = !notSync
        }
        var lengthSize: Int32 = 4
        var sets: [[UInt8]] = []
        if isKey {
            sets = parameterSets(fd, codec: codec, lengthSizeOut: &lengthSize)
        } else {
            _ = parameterSets(fd, codec: codec, lengthSizeOut: &lengthSize, wantData: false)
        }
        var total = 0
        var ptr: UnsafeMutablePointer<Int8>?
        guard CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &total,
                                          dataPointerOut: &ptr) == kCMBlockBufferNoErr, let p = ptr else { return nil }
        let avcc = Array(UnsafeBufferPointer(start: UnsafeRawPointer(p).assumingMemoryBound(to: UInt8.self), count: total))
        guard let out = try? AnnexB.convert(avcc: avcc, lengthSize: Int(lengthSize), parameterSets: sets) else { return nil }
        return EncodedFrame(annexB: out, isKeyframe: isKey, ptsUS: ptsUS, encodeMS: 0)
    }

    static func parameterSets(_ fd: CMFormatDescription, codec: Codec, lengthSizeOut: inout Int32,
                              wantData: Bool = true) -> [[UInt8]] {
        var count = 0
        var nalLen: Int32 = 4
        var sets: [[UInt8]] = []
        func read(_ i: Int) -> (UnsafePointer<UInt8>?, Int, OSStatus) {
            var p: UnsafePointer<UInt8>?
            var size = 0
            let st: OSStatus
            if codec == .hevc {
                st = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                    fd, parameterSetIndex: i, parameterSetPointerOut: &p, parameterSetSizeOut: &size,
                    parameterSetCountOut: &count, nalUnitHeaderLengthOut: &nalLen)
            } else {
                st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    fd, parameterSetIndex: i, parameterSetPointerOut: &p, parameterSetSizeOut: &size,
                    parameterSetCountOut: &count, nalUnitHeaderLengthOut: &nalLen)
            }
            return (p, size, st)
        }
        let first = read(0)
        guard first.2 == noErr else { return [] }
        lengthSizeOut = nalLen
        guard wantData else { return [] }
        for i in 0..<count {
            let (p, size, st) = i == 0 ? first : read(i)
            if st == noErr, let p = p { sets.append(Array(UnsafeBufferPointer(start: p, count: size))) }
        }
        return sets
    }
}
