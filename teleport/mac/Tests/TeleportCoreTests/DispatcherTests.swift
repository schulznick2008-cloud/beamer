import XCTest
@testable import TeleportCore

/// Attrappe: zeichnet Aufrufe auf, liefert vorgegebene Antworten.
final class FakeBackend: TeleportBackend {
    var calls: [String] = []
    var transportOpened = false
    var failWindows: IPCError?
    var failStart: IPCError?
    var running: Set<UInt32> = []

    func transportOpen(localPort: UInt16, peerHost: String?, peerPort: UInt16) throws -> UInt16 {
        if transportOpened { throw IPCError(.busy, "schon offen") }
        transportOpened = true
        calls.append("open \(localPort) \(peerHost ?? "-") \(peerPort)")
        return localPort == 0 ? 50_000 : localPort
    }
    func transportPeer(host: String, port: UInt16) throws {
        guard transportOpened else { throw IPCError(.badRequest, "transport.open fehlt") }
        calls.append("peer \(host) \(port)")
    }
    func transportClose() { transportOpened = false; calls.append("close") }
    func listWindows() throws -> [WindowInfo] {
        if let e = failWindows { throw e }
        return [WindowInfo(wid: 7, pid: 99, app: "TextEdit", title: "Ohne Titel", x: 10, y: 20, w: 800, h: 600, display: 1, scale: 2)]
    }
    func sendStart(_ p: SendParams) throws -> SendStarted {
        if let e = failStart { throw e }
        running.insert(p.tid)
        calls.append("start \(p.tid) \(p.codec.rawValue) \(p.bitrateKbps)")
        return SendStarted(width: 3024, height: 1964, scale: 2)
    }
    func sendStop(tid: UInt32, restore: Bool) throws {
        guard running.remove(tid) != nil else { throw IPCError(.notFound, "tid unbekannt") }
        calls.append("stop \(tid) \(restore)")
    }
    func sendKeyframe(tid: UInt32) throws {
        guard running.contains(tid) else { throw IPCError(.notFound, "tid unbekannt") }
        calls.append("key \(tid)")
    }
    func sendBitrate(tid: UInt32, kbps: UInt32) throws {
        guard running.contains(tid) else { throw IPCError(.notFound, "tid unbekannt") }
        calls.append("bitrate \(tid) \(kbps)")
    }
    func shutdown() { calls.append("shutdown") }
}

final class DispatcherTests: XCTestCase {
    var backend = FakeBackend()
    var d: Dispatcher!

    override func setUp() {
        backend = FakeBackend()
        d = Dispatcher(backend: backend)
    }

    private func send(_ obj: JSON) -> JSON {
        try! JSONLine.parse(d.handle(line: JSONLine.encode(obj)).line)
    }

    private func startMsg(id: Int = 10, tid: Int = 1) -> JSON {
        ["v": 1, "id": id, "cmd": "send.start", "tid": tid, "mode": "window", "wid": 7, "codec": "hevc",
         "max_fps": 60, "bitrate_kbps": 20_000, "keys": IPCTests.keysJSON(), "virtual_display": false]
    }

    func testHello() {
        let r = send(["v": 1, "id": 1, "cmd": "hello", "beamer_version": "1.5.1"])
        XCTAssertEqual(r.bool("ok"), true)
        XCTAssertEqual(r.int("re"), 1)
        XCTAssertEqual(r.int("v"), 1)
        XCTAssertEqual(r.string("platform"), "mac")
        XCTAssertEqual(r["caps"] as? [String], ["send_window", "send_display"])
        XCTAssertNotNil(r.string("helper_version"))
    }

    func testUnsupportedCommandsAnswerUnsupported() {
        for cmd in ["recv.open", "recv.update", "recv.close", "inject", "send.resize", "send.activate", "bogus"] {
            let r = send(["v": 1, "id": 2, "cmd": cmd])
            XCTAssertEqual(r.bool("ok"), false, cmd)
            XCTAssertEqual(r.dict("error")?.string("code"), "unsupported", cmd)
            XCTAssertEqual(r.int("re"), 2)
        }
    }

    func testBadRequests() {
        XCTAssertEqual(send(["v": 1, "id": 3])["error"] as? JSON != nil, true)           // cmd fehlt
        XCTAssertEqual(send(["v": 2, "id": 3, "cmd": "hello"]).dict("error")?.string("code"), "bad_request")
        let noId = try! JSONLine.parse(d.handle(line: "{\"v\":1,\"cmd\":\"hello\"}").line)
        XCTAssertEqual(noId.string("ev"), "error")
        XCTAssertEqual(noId.string("code"), "bad_request")
        let garbage = try! JSONLine.parse(d.handle(line: "kein json").line)
        XCTAssertEqual(garbage.string("ev"), "error")
    }

    func testTransportOpenPeerClose() {
        var r = send(["v": 1, "id": 4, "cmd": "transport.open", "local_port": 0, "peer_host": "127.0.0.1", "peer_port": 47000])
        XCTAssertEqual(r.int("local_port"), 50_000)
        XCTAssertEqual(send(["v": 1, "id": 5, "cmd": "transport.open", "local_port": 0]).dict("error")?.string("code"), "busy")
        r = send(["v": 1, "id": 6, "cmd": "transport.peer", "peer_host": "192.168.77.2", "peer_port": 47001])
        XCTAssertEqual(r.bool("ok"), true)
        XCTAssertEqual(send(["v": 1, "id": 7, "cmd": "transport.peer", "peer_port": 1]).dict("error")?.string("code"), "bad_request")
        XCTAssertEqual(send(["v": 1, "id": 8, "cmd": "transport.open", "local_port": 70000]).dict("error")?.string("code"), "bad_request")
        XCTAssertEqual(send(["v": 1, "id": 9, "cmd": "transport.close"]).bool("ok"), true)
        XCTAssertEqual(backend.calls, ["open 0 127.0.0.1 47000", "peer 192.168.77.2 47001", "close"])
    }

    func testPeerBeforeOpenIsAnError() {
        let r = send(["v": 1, "id": 1, "cmd": "transport.peer", "peer_host": "1.2.3.4", "peer_port": 5])
        XCTAssertEqual(r.dict("error")?.string("code"), "bad_request")
    }

    func testWindowsList() {
        let r = send(["v": 1, "id": 1, "cmd": "windows.list"])
        let w = (r["windows"] as? [JSON])?.first
        XCTAssertEqual(w?.int("wid"), 7)
        XCTAssertEqual(w?.string("app"), "TextEdit")
        XCTAssertEqual(w?.int("display"), 1)
        for k in ["pid", "title", "x", "y", "w", "h", "scale"] { XCTAssertNotNil(w?[k], k) }
    }

    func testPermissionErrorIsReportedNotCrashed() {
        backend.failWindows = IPCError(.permission, "Bildschirmaufnahme nicht freigegeben")
        let r = send(["v": 1, "id": 1, "cmd": "windows.list"])
        XCTAssertEqual(r.bool("ok"), false)
        XCTAssertEqual(r.dict("error")?.string("code"), "permission")
        XCTAssertTrue(r.dict("error")?.string("msg")?.contains("Bildschirmaufnahme") ?? false)
        backend.failStart = IPCError(.permission, "nein")
        XCTAssertEqual(send(startMsg()).dict("error")?.string("code"), "permission")
    }

    func testSendLifecycle() {
        let r = send(startMsg())
        XCTAssertEqual(r.int("width"), 3024)
        XCTAssertEqual(r.int("height"), 1964)
        XCTAssertEqual(r.int("scale"), 2)
        XCTAssertEqual(send(["v": 1, "id": 11, "cmd": "send.keyframe", "tid": 1]).bool("ok"), true)
        XCTAssertEqual(send(["v": 1, "id": 12, "cmd": "send.keyframe", "tid": 9]).dict("error")?.string("code"), "not_found")
        XCTAssertEqual(send(["v": 1, "id": 13, "cmd": "send.stop", "tid": 1, "restore": true]).bool("ok"), true)
        XCTAssertEqual(send(["v": 1, "id": 14, "cmd": "send.stop", "tid": 1]).dict("error")?.string("code"), "not_found")
        XCTAssertEqual(backend.calls, ["start 1 hevc 20000", "key 1", "stop 1 true"])
    }

    func testSendBitrateAndValidation() {
        _ = send(startMsg())
        XCTAssertEqual(send(["v": 1, "id": 2, "cmd": "send.bitrate", "tid": 1, "bitrate_kbps": 150_000]).bool("ok"), true)
        XCTAssertEqual(send(["v": 1, "id": 3, "cmd": "send.bitrate", "tid": 1, "bitrate_kbps": 5]).dict("error")?.string("code"), "bad_request")
        XCTAssertEqual(send(["v": 1, "id": 4, "cmd": "send.bitrate", "tid": 2, "bitrate_kbps": 5000]).dict("error")?.string("code"), "not_found")
        XCTAssertEqual(send(["v": 1, "id": 5, "cmd": "send.bitrate", "bitrate_kbps": 5000]).dict("error")?.string("code"), "bad_request")
        XCTAssertTrue(backend.calls.contains("bitrate 1 150000"))
    }

    func testStartRejectsBadFields() {
        var m = startMsg(); m["codec"] = "av1"
        XCTAssertEqual(send(m).dict("error")?.string("code"), "bad_request")
        m = startMsg(); m["virtual_display"] = true
        XCTAssertEqual(send(m).dict("error")?.string("code"), "unsupported")
        XCTAssertTrue(backend.calls.isEmpty)
    }

    func testShutdownQuits() {
        let r = d.handle(line: JSONLine.encode(["v": 1, "id": 99, "cmd": "shutdown"]))
        XCTAssertTrue(r.quit)
        XCTAssertEqual(backend.calls, ["shutdown"])
    }
}
