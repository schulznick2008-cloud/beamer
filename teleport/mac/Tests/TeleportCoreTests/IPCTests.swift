import XCTest
@testable import TeleportCore

final class IPCTests: XCTestCase {
    func testLineSplitterHandlesPartialLinesAndCRLF() {
        var s = LineSplitter()
        XCTAssertEqual(s.feed(Data("{\"a\":1}\n{\"b\"".utf8)), ["{\"a\":1}"])
        XCTAssertEqual(s.feed(Data(":2}\r\n\n".utf8)), ["{\"b\":2}", ""])
        XCTAssertEqual(s.feed(Data("x".utf8)), [])
    }

    func testLineSplitterDropsOversizedLineAndRecovers() {
        var s = LineSplitter()
        let big = Data(repeating: 0x61, count: LineSplitter.maxLine + 10)
        XCTAssertEqual(s.feed(big), [])
        XCTAssertEqual(s.feed(Data("rest\nok\n".utf8)), ["ok"])
        XCTAssertEqual(s.oversized, 1)
    }

    func testParseRejectsNonObjectsAndGarbage() {
        XCTAssertThrowsError(try JSONLine.parse("[1,2]")) { XCTAssertEqual(($0 as? IPCError)?.code, .badRequest) }
        XCTAssertThrowsError(try JSONLine.parse("{nope")) { XCTAssertEqual(($0 as? IPCError)?.code, .badRequest) }
        XCTAssertNoThrow(try JSONLine.parse("{\"v\":1,\"id\":3,\"cmd\":\"hello\"}"))
    }

    func testEncodeIsOneLineWithoutNaN() {
        let line = JSONLine.encode(["x": Double.nan, "y": [Double.infinity], "s": "a/b\n"])
        XCTAssertFalse(line.contains("\n") && line.hasSuffix("\n"))
        XCTAssertFalse(line.contains("nan"))
        let back = try! JSONLine.parse(line)
        XCTAssertEqual(back.int("x"), 0)
        XCTAssertEqual(back.string("s"), "a/b\n")
    }

    func testIntRejectsBoolsAndFractions() throws {
        let j = try JSONLine.parse("{\"a\":true,\"b\":1.5,\"c\":7,\"d\":7.0,\"e\":\"7\"}")
        XCTAssertNil(j.int("a"))
        XCTAssertNil(j.int("b"))
        XCTAssertEqual(j.int("c"), 7)
        XCTAssertEqual(j.int("d"), 7)
        XCTAssertNil(j.int("e"))
        XCTAssertEqual(j.bool("a"), true)
        XCTAssertNil(j.bool("c"))
    }

    // MARK: send.start

    static func keysJSON() -> JSON {
        func b64(_ n: Int, _ v: UInt8) -> String { Data(repeating: v, count: n).base64EncodedString() }
        return ["s2r_key": b64(32, 1), "s2r_prefix": b64(4, 2), "r2s_key": b64(32, 3), "r2s_prefix": b64(4, 4)]
    }

    func testKeysParse() throws {
        let k = try StreamKeys.parse(Self.keysJSON())
        XCTAssertEqual(k.s2rKey, [UInt8](repeating: 1, count: 32))
        XCTAssertEqual(k.r2sPrefix, [UInt8](repeating: 4, count: 4))
        var bad = Self.keysJSON()
        bad["s2r_key"] = Data(repeating: 1, count: 31).base64EncodedString()
        XCTAssertThrowsError(try StreamKeys.parse(bad)) { XCTAssertEqual(($0 as? IPCError)?.code, .badRequest) }
        bad = Self.keysJSON()
        bad["r2s_prefix"] = "%%%"
        XCTAssertThrowsError(try StreamKeys.parse(bad))
        bad = Self.keysJSON()
        bad.removeValue(forKey: "s2r_prefix")
        XCTAssertThrowsError(try StreamKeys.parse(bad))
    }

    func testSendParamsParse() throws {
        var m: JSON = ["tid": 5, "mode": "window", "wid": 42, "codec": "hevc", "max_fps": 30, "bitrate_kbps": 8000,
                       "keys": Self.keysJSON(), "virtual_display": false]
        let p = try SendParams.parse(m)
        XCTAssertEqual(p.tid, 5)
        XCTAssertEqual(p.mode, .window(42))
        XCTAssertEqual(p.codec, .hevc)
        XCTAssertEqual(p.maxFps, 30)
        XCTAssertEqual(p.bitrateKbps, 8000)

        m["mode"] = "display"; m["display"] = 1; m.removeValue(forKey: "wid")
        XCTAssertEqual(try SendParams.parse(m).mode, .display(1))

        for (k, v) in [("tid", 0), ("max_fps", 0), ("max_fps", 500), ("bitrate_kbps", 10)] as [(String, Int)] {
            var bad = m; bad[k] = v
            XCTAssertThrowsError(try SendParams.parse(bad), "\(k)=\(v)") { XCTAssertEqual(($0 as? IPCError)?.code, .badRequest) }
        }
        var bad = m; bad["codec"] = "vp9"
        XCTAssertThrowsError(try SendParams.parse(bad))
        bad = m; bad["mode"] = "window"; bad.removeValue(forKey: "wid")
        XCTAssertThrowsError(try SendParams.parse(bad))
        // virtual_display: fuer Fenster erlaubt, fuer Bildschirme ein Fehler.
        var win = m; win["mode"] = "window"; win["wid"] = 9; win.removeValue(forKey: "display"); win["virtual_display"] = true
        XCTAssertEqual(try SendParams.parse(win).virtualDisplay, true)
        win["virtual_display"] = false
        XCTAssertEqual(try SendParams.parse(win).virtualDisplay, false)
        bad = m; bad["virtual_display"] = true // m ist hier mode display
        XCTAssertThrowsError(try SendParams.parse(bad)) { XCTAssertEqual(($0 as? IPCError)?.code, .badRequest) }
    }
}
