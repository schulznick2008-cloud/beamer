import XCTest
@testable import TeleportCore

/// Startet das fertige `BeamerTeleport` und spricht JSON Lines mit ihm (Fake-IPC aus Sicht von Python).
final class ProcessTests: XCTestCase {
    private func binary() throws -> URL {
        var dir = Bundle(for: ProcessTests.self).bundleURL.deletingLastPathComponent()
        for _ in 0..<2 {
            let c = dir.appendingPathComponent("BeamerTeleport")
            if FileManager.default.isExecutableFile(atPath: c.path) { return c }
            dir.deleteLastPathComponent()
        }
        throw XCTSkip("BeamerTeleport-Binary nicht gefunden (swift build zuerst)")
    }

    final class Session {
        let p = Process()
        let inPipe = Pipe(), outPipe = Pipe()
        private var buffer = Data()

        init(_ url: URL, env: [String: String] = [:]) throws {
            p.executableURL = url
            p.arguments = ["--ipc", "stdio"]
            p.standardInput = inPipe
            p.standardOutput = outPipe
            p.standardError = FileHandle.nullDevice
            var e = ProcessInfo.processInfo.environment
            env.forEach { e[$0.key] = $0.value }
            p.environment = e
            try p.run()
        }

        func send(_ obj: JSON) {
            inPipe.fileHandleForWriting.write(Data((JSONLine.encode(obj) + "\n").utf8))
        }

        func readLine(timeout: TimeInterval = 10) -> String? {
            let end = Date().addingTimeInterval(timeout)
            let fd = outPipe.fileHandleForReading.fileDescriptor
            while Date() < end {
                if let i = buffer.firstIndex(of: 0x0A) {
                    let line = String(data: buffer[buffer.startIndex..<i], encoding: .utf8)
                    buffer.removeSubrange(buffer.startIndex...i)
                    return line
                }
                var pfd = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
                if poll(&pfd, 1, 100) > 0 {
                    var tmp = [UInt8](repeating: 0, count: 4096)
                    let n = read(fd, &tmp, tmp.count)
                    if n <= 0 { return nil }
                    buffer.append(contentsOf: tmp[0..<n])
                }
            }
            return nil
        }

        struct NoReply: Error, CustomStringConvertible {
            let cmd: String
            var description: String { "keine Antwort auf \(cmd) (haengt der Helfer?)" }
        }

        func call(_ cmd: String, id: Int, timeout: TimeInterval = 10, _ extra: JSON = [:]) throws -> JSON {
            var m: JSON = ["v": 1, "id": id, "cmd": cmd]
            extra.forEach { m[$0.key] = $0.value }
            send(m)
            let end = Date().addingTimeInterval(timeout)
            while Date() < end, let line = readLine(timeout: max(0.1, end.timeIntervalSinceNow)) {
                let j = try JSONLine.parse(line) // jede stdout-Zeile muss gueltiges JSON sein
                if j.int("re") == id { return j }
            }
            throw NoReply(cmd: cmd)
        }

        func closeInputAndWait(timeout: TimeInterval) -> (exited: Bool, seconds: TimeInterval, code: Int32) {
            let t0 = Date()
            try? inPipe.fileHandleForWriting.close()
            while p.isRunning && Date().timeIntervalSince(t0) < timeout { Thread.sleep(forTimeInterval: 0.01) }
            return (!p.isRunning, Date().timeIntervalSince(t0), p.isRunning ? -1 : p.terminationStatus)
        }
    }

    func testHelloUnsupportedAndCleanShutdown() throws {
        let s = try Session(try binary())
        let hello = try s.call("hello", id: 1, ["beamer_version": "test"])
        XCTAssertEqual(hello.bool("ok"), true)
        XCTAssertEqual(hello.string("platform"), "mac")
        let un = try s.call("recv.open", id: 2)
        XCTAssertEqual(un.dict("error")?.string("code"), "unsupported")
        let open = try s.call("transport.open", id: 3, ["local_port": 0])
        XCTAssertGreaterThan(open.int("local_port") ?? 0, 0)
        let down = try s.call("shutdown", id: 4)
        XCTAssertEqual(down.bool("ok"), true)
        let r = s.closeInputAndWait(timeout: 2)
        XCTAssertTrue(r.exited)
        XCTAssertEqual(r.code, 0)
    }

    func testEOFOnStdinEndsInUnderTwoSecondsWithOpenTransport() throws {
        let s = try Session(try binary())
        _ = try s.call("transport.open", id: 1, ["local_port": 0])
        let r = s.closeInputAndWait(timeout: 2)
        XCTAssertTrue(r.exited, "Prozess endet nach EOF")
        XCTAssertLessThan(r.seconds, 2.0)
        XCTAssertEqual(r.code, 0)
    }

    func testMissingScreenRecordingPermissionIsAnErrorNotACrash() throws {
        let s = try Session(try binary(), env: ["BEAMER_TELEPORT_FAKE_NO_PERMISSION": "1"])
        let list = try s.call("windows.list", id: 1)
        XCTAssertEqual(list.bool("ok"), false)
        XCTAssertEqual(list.dict("error")?.string("code"), "permission")
        XCTAssertTrue(list.dict("error")?.string("msg")?.contains("Bildschirm") ?? false)

        _ = try s.call("transport.open", id: 2, ["local_port": 0])
        let keys: JSON = IPCTests.keysJSON()
        let start = try s.call("send.start", id: 3, ["tid": 1, "mode": "window", "wid": 5, "codec": "hevc",
                                                      "max_fps": 60, "bitrate_kbps": 20000, "keys": keys])
        XCTAssertEqual(start.dict("error")?.string("code"), "permission")
        XCTAssertTrue(s.p.isRunning, "der Helfer laeuft nach dem Fehler weiter")
        let again = try s.call("hello", id: 4)
        XCTAssertEqual(again.bool("ok"), true)
        let r = s.closeInputAndWait(timeout: 2)
        XCTAssertTrue(r.exited)
        XCTAssertEqual(r.code, 0)
    }

    func testGarbageLinesAreAnsweredWithAnErrorEventAndDoNotKillTheProcess() throws {
        let s = try Session(try binary())
        s.inPipe.fileHandleForWriting.write(Data("das ist kein json\n{\"v\":1,\"cmd\":\"hello\"}\n".utf8))
        let l1 = try JSONLine.parse(try XCTUnwrap(s.readLine()))
        let l2 = try JSONLine.parse(try XCTUnwrap(s.readLine()))
        XCTAssertEqual(l1.string("ev"), "error")
        XCTAssertEqual(l2.string("ev"), "error") // id fehlt
        XCTAssertEqual(try s.call("hello", id: 9).bool("ok"), true)
        _ = s.closeInputAndWait(timeout: 2)
    }

    // MARK: Haenger in ScreenCaptureKit (C02b)

    private func startParams(wid: Int = 1) -> JSON {
        ["tid": 1, "mode": "window", "wid": wid, "codec": "h264", "max_fps": 30, "bitrate_kbps": 5000,
         "keys": IPCTests.keysJSON()]
    }

    func testStopAnswersAndEOFEndsInUnderTwoSecondsEvenIfCaptureStopNeverReturns() throws {
        let s = try Session(try binary(), env: ["BEAMER_TELEPORT_TEST_SOURCE": "hang-stop"])
        _ = try s.call("transport.open", id: 1, ["local_port": 0])
        let start = try s.call("send.start", id: 2, startParams())
        XCTAssertEqual(start.bool("ok"), true, "\(start)")

        let t0 = Date()
        let stop = try s.call("send.stop", id: 3, timeout: 5, ["tid": 1])
        let dt = Date().timeIntervalSince(t0)
        XCTAssertEqual(stop.bool("ok"), true)
        XCTAssertLessThan(dt, 2.0, "send.stop-Antwort nach \(dt) s")
        XCTAssertEqual(try s.call("hello", id: 4).bool("ok"), true, "der Helfer antwortet danach weiter")

        let r = s.closeInputAndWait(timeout: 3)
        XCTAssertTrue(r.exited, "Prozess endet nach EOF")
        XCTAssertLessThan(r.seconds, 2.0, "EOF-Ende nach \(r.seconds) s")
        XCTAssertEqual(r.code, 0)
    }

    func testEOFEndsInUnderTwoSecondsWithAHangingStrean() throws {
        let s = try Session(try binary(), env: ["BEAMER_TELEPORT_TEST_SOURCE": "hang-stop"])
        _ = try s.call("transport.open", id: 1, ["local_port": 0])
        XCTAssertEqual(try s.call("send.start", id: 2, startParams()).bool("ok"), true)
        let r = s.closeInputAndWait(timeout: 3)
        XCTAssertTrue(r.exited)
        XCTAssertLessThan(r.seconds, 2.0, "EOF-Ende nach \(r.seconds) s")
        XCTAssertEqual(r.code, 0)
    }

    func testEOFWhileACommandIsHangingEndsInUnderTwoSeconds() throws {
        // send.start haengt (Quelle startet nie): der Helfer bemerkt EOF trotzdem und beendet sich per Wachhund.
        let s = try Session(try binary(), env: ["BEAMER_TELEPORT_TEST_SOURCE": "hang-start"])
        _ = try s.call("transport.open", id: 1, ["local_port": 0])
        var m: JSON = ["v": 1, "id": 2, "cmd": "send.start"]
        startParams().forEach { m[$0.key] = $0.value }
        s.send(m)
        Thread.sleep(forTimeInterval: 0.4)
        let r = s.closeInputAndWait(timeout: 3)
        XCTAssertTrue(r.exited, "Prozess endet trotz haengendem Befehl")
        XCTAssertLessThan(r.seconds, 2.0, "EOF-Ende nach \(r.seconds) s")
        XCTAssertEqual(r.code, 0)
    }

    func testThreeHelpersStartedAtTheSameTimeDoNotHang() throws {
        let url = try binary()
        let results = NSMutableArray()
        let lock = NSLock()
        let group = DispatchGroup()
        for n in 0..<3 {
            group.enter()
            DispatchQueue.global().async {
                defer { group.leave() }
                var outcome = "?"
                do {
                    let s = try Session(url)
                    _ = try s.call("hello", id: 1, timeout: 20)
                    _ = try s.call("transport.open", id: 2, timeout: 20, ["local_port": 0])
                    var p: JSON = ["tid": 1, "mode": "display", "display": 0, "codec": "h264", "max_fps": 30,
                                   "bitrate_kbps": 5000, "keys": IPCTests.keysJSON()]
                    p["virtual_display"] = false
                    let r = try s.call("send.start", id: 3, timeout: 25, p)
                    let code = r.dict("error")?.string("code") ?? "ok"
                    outcome = code
                    if code == "ok" {
                        let t0 = Date()
                        let stop = try s.call("send.stop", id: 4, timeout: 5, ["tid": 1])
                        if Date().timeIntervalSince(t0) >= 2 || stop.bool("ok") != true { outcome = "stop-langsam" }
                    }
                    let e = s.closeInputAndWait(timeout: 3)
                    if !e.exited || e.seconds >= 2 { outcome = "haengt-nach-EOF" }
                    if e.exited && e.code != 0 { outcome = "exit-code-\(e.code)" }
                } catch {
                    outcome = "FEHLER \(error)"
                }
                lock.lock(); results.add("Helfer \(n): \(outcome)"); lock.unlock()
            }
        }
        group.wait()
        // Scheitern mit busy/permission ist erlaubt, Haengen nicht.
        let allowed: Set<String> = ["ok", "busy", "permission"]
        for case let r as String in results {
            let outcome = String(r.split(separator: ":", maxSplits: 1).last ?? "").trimmingCharacters(in: .whitespaces)
            XCTAssertTrue(allowed.contains(outcome), r)
        }
        XCTAssertEqual(results.count, 3)
    }
}
