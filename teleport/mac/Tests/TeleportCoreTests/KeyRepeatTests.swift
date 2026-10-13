import CoreGraphics
import XCTest
@testable import TeleportCore

/// Fake-Uhr: `advance` laesst die Zeit vergehen und ruft die Zeitgeber zu den richtigen Zeitpunkten.
final class FakeTicker: RepeatTicker {
    private struct Entry { var next: TimeInterval; let interval: TimeInterval; let fire: () -> Void; var live = true }
    private var entries: [Int: Entry] = [:]
    private var nextID = 0
    private(set) var now: TimeInterval = 0
    var activeCount: Int { entries.values.filter(\.live).count }

    func start(delay: TimeInterval, interval: TimeInterval, _ fire: @escaping () -> Void) -> () -> Void {
        let id = nextID
        nextID += 1
        entries[id] = Entry(next: now + delay, interval: interval, fire: fire)
        return { [weak self] in self?.entries[id]?.live = false }
    }

    func advance(by dt: TimeInterval) {
        let end = now + dt
        while true {
            let due = entries.filter { $0.value.live && $0.value.next <= end + 1e-9 }.min { $0.value.next < $1.value.next }
            guard let (id, e) = due else { break }
            now = e.next
            entries[id]?.next += e.interval
            e.fire()
        }
        now = end
    }
}

final class RepeatSink: EventSink {
    enum Ev: Equatable { case down(UInt16, UInt64), up(UInt16), rep(UInt16, UInt64, String?), text(String, Bool) }
    private(set) var events: [Ev] = []
    func mouse(_ type: MouseEventKind, at p: CGPoint, button: MouseButton?, clickState: Int, flags: UInt64) {}
    func scroll(at p: CGPoint, wheelY: Int, wheelX: Int, unit: ScrollUnit, flags: UInt64) {}
    func key(code: UInt16, down: Bool, flags: UInt64) { events.append(down ? .down(code, flags) : .up(code)) }
    func text(_ chunk: String, down: Bool) { events.append(.text(chunk, down)) }
    func repeatKey(code: UInt16, flags: UInt64, chars: String?) { events.append(.rep(code, flags, chars)) }
    var repeats: Int { events.filter { if case .rep = $0 { return true } else { return false } }.count }
}

final class KeyRepeatTests: XCTestCase {
    let geo = InputGeometry(origin: .zero, size: CGSize(width: 800, height: 600), streamWidth: 800, streamHeight: 600)

    private func make(delay: TimeInterval = 0.4, interval: TimeInterval = 0.1) -> (Injector, RepeatSink, FakeTicker) {
        let sink = RepeatSink(), ticker = FakeTicker()
        return (Injector(sink: sink, ticker: ticker, repeatTiming: { (delay, interval) }), sink, ticker)
    }

    private func key(_ inj: Injector, _ n: String, _ down: Bool) { inj.handle(.key(name: n, down: down), geometry: geo) }

    func testRepeatStartsAfterDelayThenRunsAtInterval() {
        let (inj, sink, clock) = make()
        key(inj, "a", true)
        clock.advance(by: 0.39)
        XCTAssertEqual(sink.repeats, 0, "vor der Verzoegerung nichts")
        clock.advance(by: 0.02)
        XCTAssertEqual(sink.repeats, 1)
        clock.advance(by: 1.0)
        XCTAssertEqual(sink.repeats, 11, "danach alle 100 ms: 1 s = 10 weitere")
        key(inj, "a", false)
        clock.advance(by: 2)
        XCTAssertEqual(sink.repeats, 11, "nach dem up keine Wiederholung mehr")
        XCTAssertEqual(clock.activeCount, 0)
        // Genau ein down am Anfang, ein up am Ende, dazwischen nur Wiederholungen.
        XCTAssertEqual(sink.events.filter { if case .down = $0 { return true } else { return false } }.count, 1)
        XCTAssertEqual(sink.events.filter { if case .up = $0 { return true } else { return false } }.count, 1)
    }

    func testOneSecondHoldMatchesSystemRate() {
        // Systemrate wie macOS-Standard: 0,5 s Verzoegerung, 83 ms Intervall: 1 s halten = 1 + (0,5/0,083) = 7 Wiederholungen.
        let (inj, sink, clock) = make(delay: 0.5, interval: 0.083)
        key(inj, "a", true)
        clock.advance(by: 1.0)
        key(inj, "a", false)
        XCTAssertEqual(sink.repeats, 1 + Int(0.5 / 0.083))
    }

    func testModifiersDoNotRepeatAndFlagsCarryOver() {
        let (inj, sink, clock) = make()
        key(inj, "shift", true)
        clock.advance(by: 5)
        XCTAssertEqual(sink.repeats, 0, "Modifier wiederholen nicht")
        key(inj, "a", true)
        clock.advance(by: 0.5)
        XCTAssertEqual(sink.repeats, 2)
        if case .rep(_, let flags, _)? = sink.events.last { XCTAssertEqual(flags & Flags.shift, Flags.shift) } else { XCTFail() }
        // Shift loslassen stoppt die Wiederholung von "a" nicht.
        key(inj, "shift", false)
        clock.advance(by: 0.3)
        XCTAssertEqual(sink.repeats, 5)
        key(inj, "a", false)
    }

    func testOnlyTheLastPressedKeyRepeats() {
        let (inj, sink, clock) = make()
        key(inj, "a", true)
        clock.advance(by: 1)
        let afterA = sink.repeats
        key(inj, "b", true)
        let before = sink.events.count
        clock.advance(by: 0.5)
        let codes = Set(sink.events[before...].compactMap { e -> UInt16? in if case .rep(let c, _, _) = e { return c } else { return nil } })
        XCTAssertEqual(codes.count, 1, "nur eine Taste wiederholt")
        XCTAssertGreaterThan(afterA, 0)
        XCTAssertEqual(clock.activeCount, 1)
        // Loslassen von "a" (nicht die wiederholte) aendert nichts, "b" wiederholt weiter.
        key(inj, "a", false)
        let n = sink.repeats
        clock.advance(by: 0.3)
        XCTAssertGreaterThan(sink.repeats, n)
        key(inj, "b", false)
        XCTAssertEqual(clock.activeCount, 0)
    }

    func testSecondDownDuringRepeatDoesNotStartASecondRepeat() {
        let (inj, sink, clock) = make()
        key(inj, "a", true)
        clock.advance(by: 0.6)
        let n0 = sink.events.count
        key(inj, "a", true) // aelterer Weg: wiederholtes down
        key(inj, "a", true)
        XCTAssertEqual(sink.events.count, n0, "wiederholte downs werden verworfen")
        XCTAssertEqual(clock.activeCount, 1, "ein Zeitgeber")
        let before = sink.repeats
        clock.advance(by: 1.0)
        XCTAssertEqual(sink.repeats - before, 10, "Rate bleibt 10/s, nicht verdoppelt")
        key(inj, "a", false)
        clock.advance(by: 1)
        XCTAssertEqual(sink.events.filter { if case .up = $0 { return true } else { return false } }.count, 1)
        XCTAssertEqual(clock.activeCount, 0)
    }

    func testReleaseAllStopsRepeat() {
        let (inj, sink, clock) = make()
        key(inj, "a", true)
        clock.advance(by: 1)
        inj.releaseAll(at: nil)
        let n = sink.repeats
        clock.advance(by: 3)
        XCTAssertEqual(sink.repeats, n)
        XCTAssertEqual(clock.activeCount, 0)
        XCTAssertFalse(inj.isHoldingSomething)
    }

    func testTextKeyRepeatsAsText() {
        let (inj, sink, clock) = make(delay: 0.2, interval: 0.1)
        key(inj, "é", true) // kein Tastencode auf US-Belegung: wird als Text getippt
        clock.advance(by: 0.5)
        let texts = sink.events.filter { if case .text("é", true) = $0 { return true } else { return false } }.count
        XCTAssertEqual(texts, 1 + 4, "ein down plus Wiederholungen (0,2 / 0,3 / 0,4 / 0,5 s)")
        key(inj, "é", false)
    }

    func testDisabledRepeatWhenIntervalIsZero() {
        let (inj, sink, clock) = make(delay: 0.4, interval: 0)
        key(inj, "a", true)
        clock.advance(by: 3)
        XCTAssertEqual(sink.repeats, 0)
        key(inj, "a", false)
    }
}

final class KeyRepeatCharsTests: XCTestCase {
    let geo = InputGeometry(origin: .zero, size: CGSize(width: 800, height: 600), streamWidth: 800, streamHeight: 600)

    func testRepeatCarriesTheCharacterOnlyForPrintableKeys() {
        let sink = RepeatSink(), ticker = FakeTicker()
        let inj = Injector(sink: sink, ticker: ticker, repeatTiming: { (0.1, 0.1) })
        for (name, expected) in [("x", "x"), ("space", " "), ("backspace", nil), ("left", nil)] as [(String, String?)] {
            inj.handle(.key(name: name, down: true), geometry: geo)
            ticker.advance(by: 0.15)
            inj.handle(.key(name: name, down: false), geometry: geo)
            guard case .rep(_, _, let chars)? = sink.events.last(where: { if case .rep = $0 { return true } else { return false } }) else {
                XCTFail("keine Wiederholung fuer \(name)"); continue
            }
            XCTAssertEqual(chars, expected, name)
        }
    }
}
