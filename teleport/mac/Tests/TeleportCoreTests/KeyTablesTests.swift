import XCTest
@testable import TeleportCore

/// Die Swift-Tabellen muessen den Tabellen der Python-Seite entsprechen (nichts neu erfunden).
final class KeyTablesTests: XCTestCase {
    private func repoFile(_ rel: String) throws -> String {
        var dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        for _ in 0..<8 {
            let candidate = dir.appendingPathComponent(rel)
            if let s = try? String(contentsOf: candidate, encoding: .utf8) { return s }
            dir.deleteLastPathComponent()
        }
        throw XCTSkip("\(rel) nicht gefunden (Test laeuft ausserhalb des Repos)")
    }

    /// Zeilen `0xNN: "name",` aus dem Block `NAME = { ... }`.
    private func table(_ source: String, _ name: String) -> [UInt16: String] {
        guard let start = source.range(of: "\(name) = {") else { return [:] }
        let rest = source[start.upperBound...]
        let end = rest.range(of: "\n}")!.lowerBound
        var out: [UInt16: String] = [:]
        for line in rest[..<end].split(separator: "\n") {
            let parts = line.trimmingCharacters(in: .whitespaces).split(separator: ":", maxSplits: 1)
            guard parts.count == 2, parts[0].hasPrefix("0x"), let code = UInt16(parts[0].dropFirst(2), radix: 16) else { continue }
            var v = parts[1].trimmingCharacters(in: .whitespaces)
            if v.hasSuffix(",") { v.removeLast() }
            // Python-Literal "x" oder "\\" -> Zeichen
            v.removeFirst(); v.removeLast()
            out[code] = v.replacingOccurrences(of: "\\\\", with: "\\")
        }
        return out
    }

    func testSpecialAndPrintableTablesEqualTheirPythonSource() throws {
        let py = try repoFile("mac_app/key_codes.py")
        let special = table(py, "SPECIAL_KEY_NAMES")
        let printable = table(py, "PRINTABLE_KEY_FALLBACKS")
        XCTAssertGreaterThan(special.count, 40)
        XCTAssertGreaterThan(printable.count, 40)
        XCTAssertEqual(KeyTables.specialKeyNames, special)
        XCTAssertEqual(KeyTables.printableFallbacks, printable)
    }

    func testNameLookupFollowsKeyNameToCode() throws {
        // KEY_NAME_TO_CODE.setdefault: bei doppelten Namen (enter) gilt der kleinste Code.
        XCTAssertEqual(KeyTables.keyNameToCode["enter"], 0x24)
        XCTAssertEqual(KeyTables.keyNameToCode["f1"], 0x7A)
        XCTAssertEqual(KeyTables.keyNameToCode["left"], 0x7B)
        XCTAssertEqual(KeyTables.keyNameToCode["cmd"], 0x37)
        XCTAssertEqual(KeyTables.windowsOnlyKeyCodes["print_screen"], 0x69)
        XCTAssertEqual(KeyTables.modifierFlagNames["cmd_r"], "Command")
        XCTAssertEqual(KeyTables.mediaNXKeys["volume_up"], 0)
        XCTAssertEqual(KeyTables.browserKeyChords["browser_back"], "[")
        for (name, code) in KeyTables.keyNameToCode {
            XCTAssertEqual(KeyTables.specialKeyNames[code], name, "\(name)")
        }
    }

    func testGeneratedFileIsCurrent() throws {
        let generated = try repoFile("teleport/mac/Sources/TeleportCore/KeyTables.swift")
        let py = try repoFile("mac_app/key_codes.py")
        // Jeder Sondertasten-Eintrag der Python-Quelle steht in der erzeugten Datei.
        for (code, name) in table(py, "SPECIAL_KEY_NAMES") {
            XCTAssertTrue(generated.contains(String(format: "0x%02X: \"%@\",", code, name)), "\(name) fehlt: gen_swift_keycodes.py ausfuehren")
        }
    }
}
