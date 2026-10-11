import CoreGraphics
import XCTest
@testable import TeleportCore

/// Zeichnet auf, was der Injektor einspielen wuerde.
final class RecordingSink: EventSink {
    enum Ev: Equatable {
        case mouse(MouseEventKind, CGPoint, MouseButton?, Int, UInt64)
        case scroll(CGPoint, Int, Int, ScrollUnit, UInt64)
        case key(UInt16, Bool, UInt64)
        case text(String, Bool)
    }
    private(set) var events: [Ev] = []
    func mouse(_ type: MouseEventKind, at p: CGPoint, button: MouseButton?, clickState: Int, flags: UInt64) {
        events.append(.mouse(type, p, button, clickState, flags))
    }
    func scroll(at p: CGPoint, wheelY: Int, wheelX: Int, unit: ScrollUnit, flags: UInt64) {
        events.append(.scroll(p, wheelY, wheelX, unit, flags))
    }
    func key(code: UInt16, down: Bool, flags: UInt64) { events.append(.key(code, down, flags)) }
    func text(_ chunk: String, down: Bool) { events.append(.text(chunk, down)) }
}

final class InputTests: XCTestCase {
    // Fenster 1512x982 pt bei (100, 50), Retina 2x: Stream 3024x1964 Pixel.
    let geo = InputGeometry(origin: CGPoint(x: 100, y: 50), size: CGSize(width: 1512, height: 982),
                            streamWidth: 3024, streamHeight: 1964)

    // MARK: Koordinaten

    func testRetinaStreamPixelsMapToPointsExactly() {
        XCTAssertEqual(geo.toGlobal(x: 0, y: 0), CGPoint(x: 100, y: 50))
        XCTAssertEqual(geo.toGlobal(x: 3023, y: 1963), CGPoint(x: 100 + 3023.0 / 2, y: 50 + 1963.0 / 2))
        XCTAssertEqual(geo.toGlobal(x: 1000, y: 600), CGPoint(x: 600, y: 350))
        XCTAssertEqual(geo.toGlobal(x: 1.0, y: 1.0), CGPoint(x: 100.5, y: 50.5), "ein Stream-Pixel sind 0,5 pt")
    }

    func testOneToOneAndNonUniformScales() {
        let g1 = InputGeometry(origin: .zero, size: CGSize(width: 800, height: 600), streamWidth: 800, streamHeight: 600)
        XCTAssertEqual(g1.toGlobal(x: 123.5, y: 77), CGPoint(x: 123.5, y: 77))
        // Negative Ursprungswerte (Monitor links vom Hauptbildschirm, wie der virtuelle Monitor).
        let g2 = InputGeometry(origin: CGPoint(x: -1200, y: 0), size: CGSize(width: 1200, height: 800), streamWidth: 2400, streamHeight: 1600)
        XCTAssertEqual(g2.toGlobal(x: 2400 - 1, y: 10), CGPoint(x: -1200 + 2399.0 / 2, y: 5))
    }

    func testOutsideTheImageIsDropped() {
        for (x, y) in [(-0.1, 5.0), (5.0, -1.0), (3024.0, 5.0), (5.0, 1964.0), (9999.0, 9999.0)] {
            XCTAssertNil(geo.toGlobal(x: x, y: y), "\(x),\(y)")
        }
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        XCTAssertFalse(inj.handle(.move(x: 4000, y: 10), geometry: geo))
        XCTAssertFalse(inj.handle(.down(x: -5, y: 10, button: .left, clicks: 1), geometry: geo))
        XCTAssertFalse(inj.handle(.scroll(x: 10, y: 5000, dx: 0, dy: 1, unit: .line), geometry: geo))
        XCTAssertTrue(sink.events.isEmpty)
    }

    func testDegenerateGeometryDropsEverything() {
        XCTAssertNil(InputGeometry(origin: .zero, size: .zero, streamWidth: 100, streamHeight: 100).toGlobal(x: 1, y: 1))
        XCTAssertNil(InputGeometry(origin: .zero, size: CGSize(width: 10, height: 10), streamWidth: 0, streamHeight: 100).toGlobal(x: 1, y: 1))
    }

    // MARK: Maus

    func testClickSequenceWithClickCountsAndButtons() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        let p = CGPoint(x: 600, y: 350)
        XCTAssertTrue(inj.handle(.move(x: 1000, y: 600), geometry: geo))
        for n in 1...2 {
            inj.handle(.down(x: 1000, y: 600, button: .left, clicks: n), geometry: geo)
            inj.handle(.up(x: 1000, y: 600, button: .left), geometry: geo)
        }
        inj.handle(.down(x: 1000, y: 600, button: .right, clicks: 1), geometry: geo)
        inj.handle(.up(x: 1000, y: 600, button: .right), geometry: geo)
        XCTAssertEqual(sink.events, [
            .mouse(.moved, p, nil, 0, 0),
            .mouse(.down, p, .left, 1, 0), .mouse(.up, p, .left, 1, 0),
            .mouse(.down, p, .left, 2, 0), .mouse(.up, p, .left, 1, 0),
            .mouse(.down, p, .right, 1, 0), .mouse(.up, p, .right, 1, 0),
        ])
    }

    func testMovingWithAButtonHeldIsADrag() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        inj.handle(.down(x: 10, y: 10, button: .left, clicks: 1), geometry: geo)
        inj.handle(.move(x: 20, y: 20), geometry: geo)
        inj.handle(.up(x: 20, y: 20, button: .left), geometry: geo)
        inj.handle(.move(x: 30, y: 30), geometry: geo)
        XCTAssertEqual(sink.events.map { e -> MouseEventKind? in if case .mouse(let k, _, _, _, _) = e { return k }; return nil },
                       [.down, .dragged, .up, .moved])
    }

    func testReleaseOutsideTheImageStillLetsGoOfTheButton() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        inj.handle(.down(x: 10, y: 10, button: .left, clicks: 1), geometry: geo)
        XCTAssertFalse(inj.handle(.up(x: 99999, y: 10, button: .left), geometry: geo))
        XCTAssertFalse(inj.isHoldingSomething, "der Knopf gilt als losgelassen")
        inj.handle(.move(x: 20, y: 20), geometry: geo)
        if case .mouse(let kind, _, _, _, _) = sink.events.last! { XCTAssertEqual(kind, .moved) } else { XCTFail() }
    }

    func testReleaseAllLetsGoOfKeysAndButtons() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        inj.handle(.key(name: "cmd", down: true), geometry: geo)
        inj.handle(.key(name: "a", down: true), geometry: geo)
        inj.handle(.down(x: 10, y: 10, button: .left, clicks: 1), geometry: geo)
        XCTAssertTrue(inj.isHoldingSomething)
        inj.releaseAll(at: nil)
        XCTAssertFalse(inj.isHoldingSomething)
        let ups = sink.events.filter { e in
            if case .key(_, false, _) = e { return true }
            if case .mouse(.up, _, _, _, _) = e { return true }
            return false
        }
        XCTAssertEqual(ups.count, 3, "a, cmd und der Mausknopf")
    }

    // MARK: Scrollen

    func testScrollMovesThePointerFirstAndKeepsFractions() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        inj.handle(.scroll(x: 1000, y: 600, dx: 0, dy: -3, unit: .line), geometry: geo)
        XCTAssertEqual(sink.events, [.mouse(.moved, CGPoint(x: 600, y: 350), nil, 0, 0),
                                     .scroll(CGPoint(x: 600, y: 350), -3, 0, .line, 0)])
        let s2 = RecordingSink()
        let i2 = Injector(sink: s2)
        // 0,4 + 0,4 + 0,4: erst beim dritten Mal ein ganzer Schritt, der Rest bleibt.
        for _ in 0..<3 { i2.handle(.scroll(x: 5, y: 5, dx: 0, dy: 0.4, unit: .line), geometry: geo) }
        let wheels = s2.events.compactMap { e -> Int? in if case .scroll(_, let y, _, _, _) = e { return y }; return nil }
        XCTAssertEqual(wheels, [1])
        // Zeilen und Pixel haben getrennte Reste.
        var acc = ScrollAccumulator()
        XCTAssertEqual(acc.add(dy: 0.6, dx: 0, unit: .line).y, 0)
        XCTAssertEqual(acc.add(dy: 0.6, dx: 0, unit: .px).y, 0)
        XCTAssertEqual(acc.add(dy: 0.6, dx: 0, unit: .line).y, 1)
        XCTAssertEqual(acc.add(dy: -2.5, dx: 1.5, unit: .px).y, -1, "Abschneiden zur Null, Rest 0,6 - 2,5 = -1,9 -> -1")
    }

    func testPixelScrollUnitIsPassedOn() {
        let sink = RecordingSink()
        Injector(sink: sink).handle(.scroll(x: 5, y: 5, dx: 8, dy: -24, unit: .px), geometry: geo)
        XCTAssertEqual(sink.events.last, .scroll(CGPoint(x: 102.5, y: 52.5), -24, 8, .px, 0))
    }

    // MARK: Tasten

    func testSpecialKeysUseBeamersNamesAndCodes() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        inj.handle(.key(name: "enter", down: true), geometry: geo)
        inj.handle(.key(name: "enter", down: false), geometry: geo)
        inj.handle(.key(name: "left", down: true), geometry: geo)
        inj.handle(.key(name: "f5", down: true), geometry: geo)
        inj.handle(.key(name: "print_screen", down: true), geometry: geo)
        XCTAssertEqual(sink.events, [
            .key(0x24, true, 0), .key(0x24, false, 0),
            .key(0x7B, true, Flags.secondaryFn | Flags.numericPad), // Pfeile tragen Fn und NumericPad
            .key(0x60, true, Flags.secondaryFn),
            .key(0x69, true, Flags.secondaryFn), // Print Screen liegt auf F13: Fn-Flag wie bei Beamers Python-Injektor
        ])
    }

    func testModifierChordAndLetterKeys() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        for (n, d) in [("ctrl", true), ("shift", true), ("a", true), ("a", false), ("shift", false), ("ctrl", false)] as [(String, Bool)] {
            inj.handle(.key(name: n, down: d), geometry: geo)
        }
        let mods = Flags.control | Flags.shift
        XCTAssertEqual(sink.events, [
            .key(0x3B, true, Flags.control),                 // ctrl traegt sein eigenes Flag
            .key(0x38, true, mods),
            .key(0x00, true, mods), .key(0x00, false, mods), // "a" auf US-Belegung = Code 0
            .key(0x38, false, Flags.control),                // shift ist beim Loslassen schon abgezogen
            .key(0x3B, false, 0),
        ])
    }

    func testCapitalAndUnknownCharactersAreTypedAsText() {
        let sink = RecordingSink()
        let inj = Injector(sink: sink)
        inj.handle(.key(name: "A", down: true), geometry: geo)   // Grossbuchstabe ohne Modifier: Text
        inj.handle(.key(name: "\u{E9}", down: true), geometry: geo) // é: US-Belegung kennt keine Taste
        inj.handle(.key(name: "nonsense_key", down: true), geometry: geo)
        XCTAssertEqual(sink.events, [.text("A", true), .text("\u{E9}", true)])
    }

    func testRepeatedKeyStaysOnTheKeyItWentDownOn() {
        struct SwitchingLayout: KeyLayout {
            let table: [String: UInt16]
            func code(for c: String) -> UInt16? { table[c] }
            func character(for code: UInt16) -> String? { nil }
        }
        var planner = KeyPlanner()
        let before = SwitchingLayout(table: ["z": 0x06])
        let after = SwitchingLayout(table: ["z": 0x10]) // Layout-Wechsel mitten im Halten
        XCTAssertEqual(planner.plan(name: "z", down: true, layout: before)?.code, 0x06)
        XCTAssertEqual(planner.plan(name: "z", down: true, layout: after)?.code, 0x06, "Wiederholung bleibt auf der Taste")
        XCTAssertEqual(planner.plan(name: "z", down: false, layout: after)?.code, 0x06)
        XCTAssertEqual(planner.plan(name: "z", down: true, layout: after)?.code, 0x10, "neuer Druck folgt der neuen Belegung")
    }

    func testTextIsSentAsUnicodeInChunksWithoutFlags() {
        let s2 = RecordingSink()
        let i2 = Injector(sink: s2)
        i2.handle(.text("\u{E4}\u{F6}\u{FC}\u{DF} \u{20AC}"), geometry: geo)
        XCTAssertEqual(s2.events, [.text("\u{E4}\u{F6}\u{FC}\u{DF} \u{20AC}", true), .text("\u{E4}\u{F6}\u{FC}\u{DF} \u{20AC}", false)])
        XCTAssertEqual(textChunks("abcdefghijklmnopqrstuvwxyz"), ["abcdefghijklmnop", "qrstuvwxyz"])
        // Ein Zeichen ausserhalb der BMP (2 UTF-16-Einheiten) wird nie geteilt.
        let emoji = String(repeating: "a", count: 15) + "\u{1F600}"
        let chunks = textChunks(emoji)
        XCTAssertEqual(chunks, [String(repeating: "a", count: 15), "\u{1F600}"])
        XCTAssertEqual(chunks.joined(), emoji)
        XCTAssertEqual(textChunks(""), [])
    }

    func testFlagValuesMatchCoreGraphics() {
        XCTAssertEqual(Flags.shift, CGEventFlags.maskShift.rawValue)
        XCTAssertEqual(Flags.control, CGEventFlags.maskControl.rawValue)
        XCTAssertEqual(Flags.alternate, CGEventFlags.maskAlternate.rawValue)
        XCTAssertEqual(Flags.command, CGEventFlags.maskCommand.rawValue)
        XCTAssertEqual(Flags.alphaShift, CGEventFlags.maskAlphaShift.rawValue)
        XCTAssertEqual(Flags.numericPad, CGEventFlags.maskNumericPad.rawValue)
        XCTAssertEqual(Flags.secondaryFn, CGEventFlags.maskSecondaryFn.rawValue)
    }
}
