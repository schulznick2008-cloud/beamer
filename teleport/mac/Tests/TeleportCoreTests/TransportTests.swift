import XCTest
@testable import TeleportCore

final class TransportTests: XCTestCase {
    func testLoopbackFrameRoundtripThroughTheCLibrary() throws {
        let tx = try Transport(localPort: 0)
        let rx = try Transport(localPort: 0)
        try rx.addStream(id: 9, role: .receiver, keys: .testOnly, maxKbps: 10_000)
        try tx.addStream(id: 9, role: .sender, keys: .testOnly, maxKbps: 10_000)
        try tx.setPeer(host: "127.0.0.1", port: rx.localPort)
        let got = expectation(description: "Frame")
        var received: ReceivedFrame?
        rx.onFrame = { f in received = f; got.fulfill() }
        let payload = (0..<5000).map { UInt8($0 % 251) }
        XCTAssertTrue(tx.sendFrame(stream: 9, kind: 1, data: payload, ptsUS: 1234, fflags: 1 | Codec.hevc.fflagsBits))
        wait(for: [got], timeout: 5)
        XCTAssertEqual(received?.data, payload)
        XCTAssertEqual(received?.ptsUS, 1234)
        XCTAssertEqual(received?.fflags, 5)
        XCTAssertEqual(received?.streamId, 9)
        XCTAssertEqual(tx.targetKbps(stream: 9), 10_000)
        try tx.setMaxKbps(stream: 9, kbps: 3000)
        XCTAssertEqual(tx.targetKbps(stream: 9), 3000)
        tx.close()
        rx.close()
    }

    func testErrorsAreReportedNotFatal() throws {
        let t = try Transport(localPort: 0)
        XCTAssertThrowsError(try t.setPeer(host: "nicht-erreichbar.invalid", port: 1))
        XCTAssertFalse(t.sendFrame(stream: 5, data: [1, 2, 3], ptsUS: 0, fflags: 0), "unbekannter Stream")
        XCTAssertThrowsError(try t.setMaxKbps(stream: 5, kbps: 1000))
        try t.addStream(id: 1, role: .sender, keys: .testOnly, maxKbps: 1000)
        XCTAssertThrowsError(try t.addStream(id: 1, role: .sender, keys: .testOnly, maxKbps: 1000))
        t.close()
        XCTAssertFalse(t.sendFrame(stream: 1, data: [1], ptsUS: 0, fflags: 0))
    }
}
