import CoreGraphics
import Foundation

// Nach einem Absturz oder Neustart des Helfers kann das System noch Tasten, Modifier oder Maustasten als gedrueckt
// fuehren, die der Helfer eingespielt hatte (der Proxy schickt das `up` nur an den alten Prozess). Beim Start laesst
// der Helfer deshalb alles los, was das System als unten meldet: ein `up` mit Flags 0, ohne Wiederholung.

public protocol HeldInputSource {
    func isKeyDown(_ code: UInt16) -> Bool
    func isButtonDown(_ button: MouseButton) -> Bool
}

public struct SystemHeldInput: HeldInputSource {
    public init() {}
    public func isKeyDown(_ code: UInt16) -> Bool { CGEventSource.keyState(.hidSystemState, key: CGKeyCode(code)) }
    public func isButtonDown(_ button: MouseButton) -> Bool {
        let b: CGMouseButton
        switch button {
        case .left: b = .left
        case .right: b = .right
        case .middle: b = .center
        case .back: b = CGMouseButton(rawValue: 3)!
        case .forward: b = CGMouseButton(rawValue: 4)!
        }
        return CGEventSource.buttonState(.hidSystemState, button: b)
    }
}

public enum StuckInput {
    /// Alle Tastencodes 0...127 ausser Caps Lock (dessen "unten" ist der Schaltzustand, nicht eine gehaltene Taste).
    static let capsLockCode: UInt16 = 0x39

    /// Gibt die Anzahl losgelassener Tasten und Knoepfe zurueck.
    @discardableResult
    public static func releaseAll(source: HeldInputSource = SystemHeldInput(), sink: EventSink,
                                  at location: CGPoint? = CGEvent(source: nil)?.location) -> Int {
        var n = 0
        for code in UInt16(0)...127 where code != capsLockCode && source.isKeyDown(code) {
            sink.key(code: code, down: false, flags: 0)
            n += 1
        }
        if let p = location {
            for b in [MouseButton.left, .right, .middle, .back, .forward] where source.isButtonDown(b) {
                sink.mouse(.up, at: p, button: b, clickState: 1, flags: 0)
                n += 1
            }
        }
        return n
    }
}

extension StuckInput {
    /// Beim Start des Helfers: nur mit Bedienungshilfen-Freigabe (sonst wuerde macOS die Ereignisse verwerfen).
    public static func releaseAtStartup() {
        guard AccessibilityPermission.trusted else { return }
        let n = releaseAll(sink: CGEventSink())
        if n > 0 { logErr("teleport: \(n) haengende Taste(n)/Knopf/Knoepfe losgelassen") }
    }
}
