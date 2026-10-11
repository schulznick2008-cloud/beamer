import XCTest
@testable import TeleportCore

final class AnnexBTests: XCTestCase {
    let sps: [UInt8] = [0x67, 0x64, 0x00, 0x1F]
    let pps: [UInt8] = [0x68, 0xEE, 0x3C]
    let idr: [UInt8] = [0x65, 0x88, 0x84, 0x00, 0x10]
    let p: [UInt8] = [0x41, 0x9A, 0x24]

    private func avcc(_ nals: [[UInt8]], lengthSize: Int) -> [UInt8] {
        var out: [UInt8] = []
        for n in nals {
            for i in (0..<lengthSize).reversed() { out.append(UInt8((n.count >> (8 * i)) & 0xFF)) }
            out += n
        }
        return out
    }

    func testConvertKeyframeWithParameterSets() throws {
        let out = try AnnexB.convert(avcc: avcc([idr], lengthSize: 4), lengthSize: 4, parameterSets: [sps, pps])
        XCTAssertEqual(out, [0, 0, 0, 1] + sps + [0, 0, 0, 1] + pps + [0, 0, 0, 1] + idr)
    }

    func testConvertPFrameHasNoParameterSets() throws {
        let out = try AnnexB.convert(avcc: avcc([p], lengthSize: 4), lengthSize: 4)
        XCTAssertEqual(out, [0, 0, 0, 1] + p)
    }

    func testConvertMultipleNALsAndLengthSizes() throws {
        for ls in [1, 2, 3, 4] {
            let out = try AnnexB.convert(avcc: avcc([p, p], lengthSize: ls), lengthSize: ls)
            XCTAssertEqual(out, [0, 0, 0, 1] + p + [0, 0, 0, 1] + p, "lengthSize \(ls)")
        }
    }

    func testConvertRejectsBrokenInput() {
        XCTAssertThrowsError(try AnnexB.convert(avcc: [0, 0, 0, 9, 1, 2], lengthSize: 4)) { XCTAssertEqual($0 as? AnnexB.Failure, .truncated) }
        XCTAssertThrowsError(try AnnexB.convert(avcc: [0, 0], lengthSize: 4)) { XCTAssertEqual($0 as? AnnexB.Failure, .truncated) }
        XCTAssertThrowsError(try AnnexB.convert(avcc: [], lengthSize: 4)) { XCTAssertEqual($0 as? AnnexB.Failure, .empty) }
        XCTAssertThrowsError(try AnnexB.convert(avcc: [0, 0, 0, 0], lengthSize: 4)) // NAL der Laenge 0
        XCTAssertThrowsError(try AnnexB.convert(avcc: [1, 2], lengthSize: 5)) { XCTAssertEqual($0 as? AnnexB.Failure, .badLengthSize) }
    }

    func testSplitHandlesThreeAndFourByteStartCodes() {
        let data: [UInt8] = [0, 0, 0, 1] + sps + [0, 0, 1] + pps + [0, 0, 0, 1] + idr
        XCTAssertEqual(AnnexB.split(data), [sps, pps, idr])
        XCTAssertEqual(AnnexB.split([1, 2, 3]), [])
    }

    func testRoundTripThroughAVCC() throws {
        let annexB = try AnnexB.convert(avcc: avcc([idr, p], lengthSize: 4), lengthSize: 4, parameterSets: [sps, pps])
        let nals = AnnexB.split(annexB)
        XCTAssertEqual(nals, [sps, pps, idr, p])
        let slices = nals.filter { !AnnexB.isParameterSet($0, codec: .h264) }
        XCTAssertEqual(AnnexB.toAVCC(nals: slices), avcc([idr, p], lengthSize: 4))
    }

    func testNalClassification() {
        XCTAssertEqual(AnnexB.nalType(sps, codec: .h264), 7)
        XCTAssertTrue(AnnexB.isParameterSet(pps, codec: .h264))
        XCTAssertTrue(AnnexB.containsKeyframe([idr], codec: .h264))
        XCTAssertFalse(AnnexB.containsKeyframe([p], codec: .h264))
        // HEVC: VPS=32 (0x40), SPS=33 (0x42), PPS=34 (0x44), IDR_W_RADL=19 (0x26), TRAIL_R=1 (0x02)
        for b: UInt8 in [0x40, 0x42, 0x44] { XCTAssertTrue(AnnexB.isParameterSet([b, 1], codec: .hevc)) }
        XCTAssertTrue(AnnexB.containsKeyframe([[0x26, 1]], codec: .hevc))
        XCTAssertFalse(AnnexB.containsKeyframe([[0x02, 1]], codec: .hevc))
    }

    func testCodecFflagsBits() {
        XCTAssertEqual(Codec.h264.fflagsBits, 2)
        XCTAssertEqual(Codec.hevc.fflagsBits, 4)
    }
}
