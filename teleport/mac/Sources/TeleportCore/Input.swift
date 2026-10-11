import CoreGraphics
import Foundation

// Eingabe nach SCHNITTSTELLEN.md §3: Stream-Pixel in globale Koordinaten umrechnen und als CGEvents einspielen.
// Alles Rechnen und Planen ist reiner Code ohne Systemaufrufe (testbar); der `EventSink` postet.

/// Ein `input`-Objekt (§3).
public enum InputEvent: Equatable {
    case move(x: Double, y: Double)
    case down(x: Double, y: Double, button: MouseButton, clicks: Int)
    case up(x: Double, y: Double, button: MouseButton)
    case scroll(x: Double, y: Double, dx: Double, dy: Double, unit: ScrollUnit)
    case key(name: String, down: Bool)
    case text(String)

    public static func parse(_ j: JSON) throws -> InputEvent {
        guard let k = j.string("k") else { throw IPCError(.badRequest, "input.k fehlt") }
        func coord(_ name: String) throws -> Double {
            guard let v = j[name] as? NSNumber, CFGetTypeID(v) != CFBooleanGetTypeID(), v.doubleValue.isFinite else {
                throw IPCError(.badRequest, "input.\(name) muss eine Zahl sein")
            }
            return v.doubleValue
        }
        func button() throws -> MouseButton {
            guard let b = j.string("b").flatMap(MouseButton.init(rawValue:)) else {
                throw IPCError(.badRequest, "input.b muss left, right, middle, back oder forward sein")
            }
            return b
        }
        switch k {
        case "move":
            return .move(x: try coord("x"), y: try coord("y"))
        case "down":
            let n = j.int("n") ?? 1
            guard (1...3).contains(n) else { throw IPCError(.badRequest, "input.n muss 1...3 sein") }
            return .down(x: try coord("x"), y: try coord("y"), button: try button(), clicks: n)
        case "up":
            return .up(x: try coord("x"), y: try coord("y"), button: try button())
        case "scroll":
            let unit = ScrollUnit(rawValue: j.string("unit") ?? "line")
            guard let u = unit else { throw IPCError(.badRequest, "input.unit muss line oder px sein") }
            return .scroll(x: try coord("x"), y: try coord("y"), dx: try coord("dx"), dy: try coord("dy"), unit: u)
        case "key":
            guard let name = j.string("key"), !name.isEmpty else { throw IPCError(.badRequest, "input.key fehlt") }
            guard let down = j.bool("down") else { throw IPCError(.badRequest, "input.down (bool) fehlt") }
            return .key(name: name, down: down)
        case "text":
            guard let t = j.string("text"), !t.isEmpty else { throw IPCError(.badRequest, "input.text fehlt") }
            return .text(t)
        default:
            throw IPCError(.badRequest, "unbekannte Eingabeart \(k)")
        }
    }

    /// Braucht dieses Ereignis ein aktiviertes (vorne liegendes) Fenster? (§3: vor `down`, `key` und `text`)
    public var needsActivation: Bool {
        switch self {
        case .down, .key, .text: return true
        default: return false
        }
    }
}

public enum MouseButton: String {
    case left, right, middle, back, forward
}

public enum ScrollUnit: String {
    case line
    case px
}

/// Welcher Teil des Bildschirms (in Punkten, globale Koordinaten mit Ursprung oben links) das uebertragene Bild zeigt.
public struct InputGeometry: Equatable {
    public var origin: CGPoint
    public var size: CGSize          // Punkte
    public var streamWidth: Int      // Stream-Pixel
    public var streamHeight: Int

    public init(origin: CGPoint, size: CGSize, streamWidth: Int, streamHeight: Int) {
        self.origin = origin; self.size = size; self.streamWidth = streamWidth; self.streamHeight = streamHeight
    }

    /// Stream-Pixel in globale Bildschirmpunkte. `nil`, wenn der Punkt ausserhalb des Bildes liegt (§3: verwerfen).
    public func toGlobal(x: Double, y: Double) -> CGPoint? {
        guard streamWidth > 0, streamHeight > 0, size.width > 0, size.height > 0 else { return nil }
        guard x >= 0, y >= 0, x < Double(streamWidth), y < Double(streamHeight) else { return nil }
        return CGPoint(x: origin.x + x * Double(size.width) / Double(streamWidth),
                       y: origin.y + y * Double(size.height) / Double(streamHeight))
    }
}

/// Wohin das Zeichen einer Taste auf der aktuellen Tastaturbelegung fuehrt.
public protocol KeyLayout {
    func code(for character: String) -> UInt16?
    func character(for code: UInt16) -> String?
}

/// US-Belegung aus Beamers Tabelle (Rueckfall und Tests).
public struct USKeyLayout: KeyLayout {
    public init() {}
    public func code(for character: String) -> UInt16? {
        KeyTables.printableFallbacks.first(where: { $0.value == character })?.key
    }
    public func character(for code: UInt16) -> String? { KeyTables.printableFallbacks[code] }
}

/// Planung einer Taste (Port von `plan_key_event`): (Tastencode, Text oder nil) oder nichts zu senden.
public struct KeyPlanner {
    public private(set) var modsDown: Set<String> = []
    private var held: [String: UInt16] = [:]
    public private(set) var warned: Set<String> = []

    public init() {}

    public var heldKeyNames: [String] { Array(held.keys).sorted() }

    public mutating func plan(name: String, down: Bool, layout: KeyLayout) -> (code: UInt16, text: String?)? {
        let lowered = name.lowercased()
        if KeyTables.modifierFlagNames[lowered] != nil {
            // Vor dem Ereignis eintragen, damit das Modifier-Ereignis sein eigenes Flag schon traegt.
            if down { modsDown.insert(lowered) } else { modsDown.remove(lowered) }
        }
        if let code = KeyTables.keyNameToCode[lowered] ?? KeyTables.windowsOnlyKeyCodes[lowered] {
            return (code, nil)
        }
        guard name.count == 1 else {
            warned.insert(name)
            return nil
        }
        // Eine Wiederholung bleibt auf der Taste, auf der der Druck begann (auch wenn die Belegung wechselte).
        if let code = held[name] {
            if !down { held.removeValue(forKey: name) }
            return (code, nil)
        }
        let isLower = name == name.lowercased() && name != name.uppercased()
        let isAlpha = name != name.lowercased() || name != name.uppercased()
        if let code = layout.code(for: name.lowercased()), isLower || !isAlpha || !modsDown.isDisjoint(with: Set(["cmd", "cmd_r", "alt", "alt_r", "ctrl", "ctrl_r", "shift", "shift_r", "caps_lock"])) {
            if down { held[name] = code }
            return (code, nil)
        }
        // Ein Zeichen, das diese Belegung nicht platzieren kann (Akzent, Gedankenstrich, Grossbuchstabe ohne Shift):
        // als Text tippen, das landet im Textfeld richtig.
        return (0, name)
    }

    public mutating func clear() {
        modsDown.removeAll()
        held.removeAll()
    }
}

/// Wie viele Klicks zaehlen als Folge: das Protokoll liefert `n` mit, hier nur die Tastenzustaende.
public enum Flags {
    /// CGEventFlags-Rohwerte (CGEventTypes.h), damit der Code ohne CoreGraphics-Konstanten testbar bleibt.
    public static let alphaShift: UInt64 = 0x10000
    public static let shift: UInt64 = 0x20000
    public static let control: UInt64 = 0x40000
    public static let alternate: UInt64 = 0x80000
    public static let command: UInt64 = 0x100000
    public static let numericPad: UInt64 = 0x200000
    public static let secondaryFn: UInt64 = 0x800000

    public static func forModifiers(_ mods: Set<String>) -> UInt64 {
        var f: UInt64 = 0
        for m in mods {
            switch KeyTables.modifierFlagNames[m] {
            case "Command": f |= command
            case "Shift": f |= shift
            case "Alternate": f |= alternate
            case "Control": f |= control
            case "AlphaShift": f |= alphaShift
            default: break
            }
        }
        return f
    }

    static let arrowCodes: Set<UInt16> = Set(["left", "right", "up", "down"].compactMap { KeyTables.keyNameToCode[$0] })
    static let fnCodes: Set<UInt16> = {
        var s = arrowCodes
        for n in ["home", "end", "page_up", "page_down", "delete"] { if let c = KeyTables.keyNameToCode[n] { s.insert(c) } }
        for n in 1...20 { if let c = KeyTables.keyNameToCode["f\(n)"] { s.insert(c) } }
        return s
    }()

    /// Pfeile, Funktionstasten, Pos1/Ende/Bild und Entf tragen das Fn-Flag (Pfeile zusaetzlich NumericPad), wie
    /// auf echter Hardware; Systemkuerzel (Ctrl+Pfeil fuer Spaces) pruefen darauf.
    public static func forKey(_ code: UInt16) -> UInt64 {
        var f: UInt64 = 0
        if fnCodes.contains(code) { f |= secondaryFn }
        if arrowCodes.contains(code) { f |= numericPad }
        return f
    }
}

/// Text in Stuecke zu hoechstens 16 UTF-16-Einheiten, ohne ein Zeichen ausserhalb der BMP zu teilen.
public func textChunks(_ text: String, maxUnits: Int = 16) -> [String] {
    var chunks: [String] = []
    var current = ""
    var units = 0
    for ch in text.unicodeScalars {
        let size = ch.value > 0xFFFF ? 2 : 1
        if units + size > maxUnits {
            chunks.append(current)
            current = ""
            units = 0
        }
        current.unicodeScalars.append(ch)
        units += size
    }
    if !current.isEmpty { chunks.append(current) }
    return chunks
}

/// Rest-Bruchteile des Scrollens je Einheit und Achse: ein Rad-Ereignis traegt nur ganze Zahlen.
public struct ScrollAccumulator {
    private var remainders: [ScrollUnit: (y: Double, x: Double)] = [.line: (0, 0), .px: (0, 0)]
    public init() {}

    /// (ganze Schritte vertikal, horizontal); Rest bleibt fuer das naechste Mal (Abschneiden zur Null, damit ein Rest
    /// nie die Richtung umkehrt).
    public mutating func add(dy: Double, dx: Double, unit: ScrollUnit) -> (y: Int, x: Int) {
        var r = remainders[unit] ?? (0, 0)
        r.y += dy
        r.x += dx
        let wy = Int(r.y), wx = Int(r.x)
        r.y -= Double(wy)
        r.x -= Double(wx)
        remainders[unit] = r
        return (wy, wx)
    }

    public mutating func reset() {
        remainders = [.line: (0, 0), .px: (0, 0)]
    }
}

/// Wohin gebaute Ereignisse gehen: das echte `CGEventSink` oder in Tests eine Attrappe.
public protocol EventSink: AnyObject {
    func mouse(_ type: MouseEventKind, at p: CGPoint, button: MouseButton?, clickState: Int, flags: UInt64)
    func scroll(at p: CGPoint, wheelY: Int, wheelX: Int, unit: ScrollUnit, flags: UInt64)
    func key(code: UInt16, down: Bool, flags: UInt64)
    func text(_ chunk: String, down: Bool)
}

public enum MouseEventKind: Equatable {
    case moved, dragged, down, up
}

/// Spielt `InputEvent`s ueber einen `EventSink` ein. Merkt sich gehaltene Tasten und Knoepfe, damit
/// `releaseAll()` beim Beenden alles loslaesst.
public final class Injector {
    private let sink: EventSink
    private let layout: () -> KeyLayout
    private let lock = NSLock()
    private var planner = KeyPlanner()
    private var buttonsDown: [MouseButton] = []
    private var scroll = ScrollAccumulator()
    private var lastPoint: CGPoint?

    public init(sink: EventSink, layout: @escaping () -> KeyLayout = { USKeyLayout() }) {
        self.sink = sink
        self.layout = layout
    }

    /// `true`, wenn das Ereignis eingespielt wurde, `false`, wenn es ausserhalb des Bildes lag (verworfen).
    @discardableResult
    public func handle(_ event: InputEvent, geometry: InputGeometry) -> Bool {
        lock.lock(); defer { lock.unlock() }
        switch event {
        case .move(let x, let y):
            guard let p = geometry.toGlobal(x: x, y: y) else { return false }
            lastPoint = p
            let held = buttonsDown.last
            sink.mouse(held == nil ? .moved : .dragged, at: p, button: held, clickState: 0, flags: Flags.forModifiers(planner.modsDown))
        case .down(let x, let y, let b, let n):
            guard let p = geometry.toGlobal(x: x, y: y) else { return false }
            lastPoint = p
            buttonsDown.removeAll { $0 == b }
            buttonsDown.append(b)
            sink.mouse(.down, at: p, button: b, clickState: n, flags: Flags.forModifiers(planner.modsDown))
        case .up(let x, let y, let b):
            guard let p = geometry.toGlobal(x: x, y: y) else {
                // Das Loslassen darf nie verloren gehen, sonst bliebe der Knopf unten: am letzten Ort loslassen.
                buttonsDown.removeAll { $0 == b }
                return false
            }
            buttonsDown.removeAll { $0 == b }
            sink.mouse(.up, at: p, button: b, clickState: 1, flags: Flags.forModifiers(planner.modsDown))
        case .scroll(let x, let y, let dx, let dy, let unit):
            guard let p = geometry.toGlobal(x: x, y: y) else { return false }
            let (wy, wx) = scroll.add(dy: dy, dx: dx, unit: unit)
            guard wy != 0 || wx != 0 else { return true }
            // Der Zeiger muss ueber dem Ziel stehen, sonst scrollt das Fenster darunter.
            sink.mouse(.moved, at: p, button: nil, clickState: 0, flags: Flags.forModifiers(planner.modsDown))
            sink.scroll(at: p, wheelY: wy, wheelX: wx, unit: unit, flags: Flags.forModifiers(planner.modsDown))
        case .key(let name, let down):
            guard let plan = planner.plan(name: name, down: down, layout: layout()) else { return false }
            if let text = plan.text {
                // Flags bewusst leer: der Text sagt, was getippt wird.
                sink.text(text, down: down)
            } else {
                sink.key(code: plan.code, down: down, flags: Flags.forModifiers(planner.modsDown) | Flags.forKey(plan.code))
            }
        case .text(let t):
            for chunk in textChunks(t) {
                sink.text(chunk, down: true)
                sink.text(chunk, down: false)
            }
        }
        return true
    }

    /// Alles loslassen, was noch gehalten wird (Verbindungsabbruch, Beenden).
    public func releaseAll(at p: CGPoint?) {
        lock.lock(); defer { lock.unlock() }
        for name in planner.heldKeyNames + planner.modsDown.sorted() {
            if let plan = planner.plan(name: name, down: false, layout: layout()), plan.text == nil {
                sink.key(code: plan.code, down: false, flags: 0)
            }
        }
        if let p = p ?? lastPoint {
            for b in buttonsDown { sink.mouse(.up, at: p, button: b, clickState: 1, flags: 0) }
        }
        buttonsDown.removeAll()
        planner.clear()
        scroll.reset()
    }

    public var isHoldingSomething: Bool {
        lock.lock(); defer { lock.unlock() }
        return !buttonsDown.isEmpty || !planner.modsDown.isEmpty || !planner.heldKeyNames.isEmpty
    }
}
