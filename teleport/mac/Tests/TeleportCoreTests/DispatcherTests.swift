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
    var failInject: IPCError?
    func inject(tid: UInt32, input: InputEvent) throws {
        if let e = failInject { throw e }
        guard running.contains(tid) else { throw IPCError(.notFound, "tid unbekannt") }
        calls.append("inject \(tid) \(input)")
    }
    func sendActivate(tid: UInt32) throws {
        guard running.contains(tid) else { throw IPCError(.notFound, "tid unbekannt") }
        calls.append("activate \(tid)")
    }
    func sendResize(tid: UInt32, width: Double, height: Double) throws -> (width: Int, height: Int) {
        guard running.contains(tid) else { throw IPCError(.notFound, "tid unbekannt") }
        calls.append("resize \(tid) \(width) \(height)")
        return (Int(width) * 2, Int(height) * 2)
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
        XCTAssertEqual(r["caps"] as? [String], ["send_window", "send_display", "inject"])
        XCTAssertNotNil(r.string("helper_version"))
    }

    func testWindowsChromeWithoutSenderEngineIsSynthetic() {
        let r = send(["v": 1, "id": 21, "cmd": "windows.chrome", "wid": 7])
        XCTAssertEqual(r.bool("ok"), true)
        XCTAssertEqual(r.bool("title_bar"), false)
        XCTAssertEqual(r.bool("dark"), true)
    }

    func testUnsupportedCommandsAnswerUnsupported() {
        for cmd in ["recv.open", "recv.update", "recv.close", "bogus"] {
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
        m = startMsg(); m["virtual_display"] = true; m["mode"] = "display"; m["display"] = 1
        XCTAssertEqual(send(m).dict("error")?.string("code"), "bad_request")
        XCTAssertTrue(backend.calls.isEmpty)
    }

    func testShutdownQuits() {
        let r = d.handle(line: JSONLine.encode(["v": 1, "id": 99, "cmd": "shutdown"]))
        XCTAssertTrue(r.quit)
        XCTAssertTrue(backend.calls.isEmpty, "Aufraeumen macht der IPC-Server nach der Antwort")
    }
}

final class HelloAndFilterTests: XCTestCase {
    func testHelloReportsLowPower() throws {
        for lp in [true, false] {
            let d = Dispatcher(backend: FakeBackend(), lowPower: { lp })
            let r = try JSONLine.parse(d.handle(line: JSONLine.encode(["v": 1, "id": 1, "cmd": "hello"])).line)
            XCTAssertEqual(r.bool("low_power"), lp)
            XCTAssertEqual(r.bool("ok"), true)
        }
    }

    func testWindowFilter() {
        func keep(layer: Int = 0, w: Double = 800, h: Double = 600, on: Bool = true, title: String = "Fenster",
                  regular: Bool = true) -> Bool {
            WindowFilter.keep(layer: layer, width: w, height: h, isOnScreen: on, title: title, regularApp: regular)
        }
        XCTAssertTrue(keep())
        XCTAssertTrue(keep(on: false, title: "Minimiert"), "minimiertes Fenster mit Titel bleibt")
        XCTAssertTrue(keep(title: ""), "sichtbares Fenster einer normalen App ohne Titel bleibt")
        XCTAssertFalse(keep(w: 500, h: 500, on: false, title: ""), "unsichtbares Fenster ohne Titel (Claude 500x500)")
        XCTAssertFalse(keep(w: 500, h: 500, on: false, title: "", regular: false), "Open and Save Panel Service")
        XCTAssertFalse(keep(on: true, title: "Gesture Blocking Overlay", regular: false), "WindowManager")
        XCTAssertFalse(keep(layer: 25), "Menueleiste / Overlays")
        XCTAssertFalse(keep(w: 1800, h: 39), "Menueleisten-Streifen")
        XCTAssertFalse(keep(w: 64, h: 64), "winziges Hilfsfenster")
    }
}

final class InputCommandTests: XCTestCase {
    var backend = FakeBackend()
    var d: Dispatcher!
    override func setUp() { backend = FakeBackend(); d = Dispatcher(backend: backend); backend.running = [1] }
    private func send(_ obj: JSON) -> JSON { try! JSONLine.parse(d.handle(line: JSONLine.encode(obj)).line) }

    func testInjectParsesEveryKindOfInput() {
        let inputs: [JSON] = [
            ["k": "move", "x": 512.0, "y": 300.5],
            ["k": "down", "x": 512.0, "y": 300.5, "b": "left", "n": 2],
            ["k": "up", "x": 1, "y": 2, "b": "right"],
            ["k": "scroll", "x": 5, "y": 6, "dx": 0, "dy": -3.0, "unit": "line"],
            ["k": "key", "key": "a", "down": true],
            ["k": "text", "text": "\u{E4}"],
        ]
        for (i, input) in inputs.enumerated() {
            let r = send(["v": 1, "id": i + 1, "cmd": "inject", "tid": 1, "input": input])
            XCTAssertEqual(r.bool("ok"), true, "\(input) -> \(r)")
        }
        XCTAssertEqual(backend.calls.count, 6)
        XCTAssertTrue(backend.calls[1].contains("down(x: 512.0, y: 300.5, button: TeleportCore.MouseButton.left, clicks: 2)"), backend.calls[1])
    }

    func testInjectRejectsBadInput() {
        let bad: [JSON] = [
            ["k": "move", "x": "1", "y": 2], ["k": "down", "x": 1, "y": 2, "b": "left", "n": 5],
            ["k": "down", "x": 1, "y": 2, "b": "laser"], ["k": "scroll", "x": 1, "y": 2, "dx": 0, "dy": 1, "unit": "mm"],
            ["k": "key", "key": "a"], ["k": "text", "text": ""], ["k": "tap"], [:],
        ]
        for (i, input) in bad.enumerated() {
            let r = send(["v": 1, "id": i + 1, "cmd": "inject", "tid": 1, "input": input])
            XCTAssertEqual(r.dict("error")?.string("code"), "bad_request", "\(input)")
        }
        XCTAssertEqual(send(["v": 1, "id": 99, "cmd": "inject", "tid": 1]).dict("error")?.string("code"), "bad_request")
        XCTAssertEqual(send(["v": 1, "id": 98, "cmd": "inject", "tid": 7, "input": ["k": "move", "x": 1, "y": 1]]).dict("error")?.string("code"), "not_found")
        XCTAssertTrue(backend.calls.isEmpty)
    }

    func testPermissionErrorFromInject() {
        backend.failInject = IPCError(.permission, "Bedienungshilfen nicht freigegeben")
        let r = send(["v": 1, "id": 1, "cmd": "inject", "tid": 1, "input": ["k": "move", "x": 1, "y": 1]])
        XCTAssertEqual(r.dict("error")?.string("code"), "permission")
    }

    func testActivateAndResize() {
        XCTAssertEqual(send(["v": 1, "id": 1, "cmd": "send.activate", "tid": 1]).bool("ok"), true)
        let r = send(["v": 1, "id": 2, "cmd": "send.resize", "tid": 1, "w": 800.0, "h": 600])
        XCTAssertEqual(r.int("width"), 1600)
        XCTAssertEqual(r.int("height"), 1200)
        for bad in [["w": 10, "h": 600], ["w": 800, "h": 99999], ["w": "x", "h": 1], ["h": 600]] as [JSON] {
            var m: JSON = ["v": 1, "id": 3, "cmd": "send.resize", "tid": 1]
            bad.forEach { m[$0.key] = $0.value }
            XCTAssertEqual(send(m).dict("error")?.string("code"), "bad_request", "\(bad)")
        }
        XCTAssertEqual(send(["v": 1, "id": 4, "cmd": "send.resize", "tid": 5, "w": 800, "h": 600]).dict("error")?.string("code"), "not_found")
        XCTAssertEqual(send(["v": 1, "id": 5, "cmd": "send.activate", "tid": 5]).dict("error")?.string("code"), "not_found")
    }
}
