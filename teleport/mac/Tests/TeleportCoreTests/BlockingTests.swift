import XCTest
@testable import TeleportCore

final class BlockingTests: XCTestCase {
    func testResultAndErrorPropagate() throws {
        XCTAssertEqual(try runBlocking(timeout: 2, "ok") { 42 }, 42)
        struct Boom: Error {}
        XCTAssertThrowsError(try runBlocking(timeout: 2, "fehler") { () async throws -> Int in throw Boom() }) {
            XCTAssertTrue($0 is Boom)
        }
    }

    func testTimeoutReturnsPromptlyAndThrowsBlockingTimeout() {
        let t0 = Date()
        XCTAssertThrowsError(try runBlocking(timeout: 0.2, "haengt") { () async throws -> Int in
            try await Task.sleep(nanoseconds: 30_000_000_000)
            return 1
        }) { XCTAssertTrue($0 is BlockingTimeout) }
        XCTAssertLessThan(Date().timeIntervalSince(t0), 1.0)
    }

    func testBoundedCallTimesOutOnAThreadThatNeverReturns() {
        let t0 = Date()
        XCTAssertThrowsError(try runBounded(timeout: 0.2, "haengt") { () throws -> Int in
            DispatchSemaphore(value: 0).wait()
            return 1
        }) { XCTAssertTrue(($0 as? BlockingTimeout)?.label == "haengt") }
        XCTAssertLessThan(Date().timeIntervalSince(t0), 1.0)
        XCTAssertEqual(try runBounded(timeout: 2, "ok") { 7 }, 7)
    }

    /// Ergebnis und Zeitlimit treffen sich: kein Absturz, kein Datenwettlauf (Thread Sanitizer-tauglich),
    /// jedes Mal entweder das Ergebnis oder ein Timeout.
    func testResultRacingTheDeadlineIsSafe() {
        var results = 0, timeouts = 0
        for i in 0..<300 {
            let delay = Double(i % 7) * 0.0005
            do {
                let v = try runBlocking(timeout: 0.002, "wettlauf") { () async throws -> Int in
                    try await Task.sleep(nanoseconds: UInt64(delay * 1e9))
                    return i
                }
                XCTAssertEqual(v, i)
                results += 1
            } catch is BlockingTimeout {
                timeouts += 1
            } catch {
                XCTFail("\(error)")
            }
        }
        XCTAssertEqual(results + timeouts, 300)
    }
}

final class SCKLockTests: XCTestCase {
    func testLockSerializesConcurrentCallers() {
        let lock = NSLock()
        var inside = 0, maxInside = 0
        let group = DispatchGroup()
        for _ in 0..<6 {
            group.enter()
            DispatchQueue.global().async {
                SCKLock.with {
                    lock.lock(); inside += 1; maxInside = max(maxInside, inside); lock.unlock()
                    Thread.sleep(forTimeInterval: 0.03)
                    lock.lock(); inside -= 1; lock.unlock()
                }
                group.leave()
            }
        }
        group.wait()
        XCTAssertEqual(maxInside, 1, "hoechstens ein Aufruf gleichzeitig unter der Sperre")
    }

    func testAHolderThatNeverReleasesOnlyDelaysOthersByTheWaitLimit() {
        let holding = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            SCKLock.with(wait: 1) {
                holding.signal()
                release.wait() // haengt (bis der Test fertig ist)
            }
        }
        holding.wait()
        let t0 = Date()
        var ran = false
        SCKLock.with(wait: 0.3) { ran = true }
        XCTAssertTrue(ran, "laeuft nach dem Wartelimit auch ohne Sperre")
        XCTAssertLessThan(Date().timeIntervalSince(t0), 1.0)
        release.signal()
    }
}

final class CoreGraphicsInitTests: XCTestCase {
    func testEnsureInitializesOnTheMainThreadAndIsIdempotent() {
        XCTAssertTrue(Thread.isMainThread)
        CoreGraphicsInit.ensure()
        CoreGraphicsInit.ensure()
        XCTAssertTrue(CoreGraphicsInit.isInitialized)
        // Danach duerfen SkyLight-nahe Aufrufe auch von Hintergrund-Threads kommen (CGS_REQUIRE_INIT bleibt still).
        let exp = expectation(description: "bg")
        DispatchQueue.global().async {
            _ = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
            var n: UInt32 = 0
            CGGetActiveDisplayList(0, nil, &n)
            exp.fulfill()
        }
        wait(for: [exp], timeout: 5)
    }
}
