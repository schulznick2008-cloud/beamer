import CVirtualDisplay
import CoreGraphics
import Foundation
import IOKit.pwr_mgt

/// Ein virtueller Monitor (privates CGVirtualDisplay-API ueber `CVirtualDisplay`).
public protocol VirtualDisplayHandle: AnyObject {
    var displayID: UInt32 { get }
    /// Groesse in Punkten.
    var widthPoints: Int { get }
    var heightPoints: Int { get }
    /// Globale Rechteck des Monitors in Punkten (Ursprung oben links).
    var bounds: CGRect { get }
    func resize(widthPoints: Int, heightPoints: Int) -> Bool
    func destroy()
}

public typealias VirtualDisplayFactory = (_ widthPoints: Int, _ heightPoints: Int) throws -> VirtualDisplayHandle

public final class SystemVirtualDisplay: VirtualDisplayHandle {
    private var ref: OpaquePointer?
    public private(set) var widthPoints: Int
    public private(set) var heightPoints: Int
    private static var serialCounter: UInt32 = 0
    private static let counterLock = NSLock()

    public static func make(widthPoints: Int, heightPoints: Int) throws -> VirtualDisplayHandle {
        guard widthPoints >= 200, heightPoints >= 100, widthPoints <= 3840, heightPoints <= 2160 else {
            throw IPCError(.badRequest, "Fenstergroesse \(widthPoints)x\(heightPoints) pt passt nicht auf einen virtuellen Monitor")
        }
        counterLock.lock()
        serialCounter += 1
        let serial = serialCounter
        counterLock.unlock()
        guard let r = TPVirtualDisplayCreate(UInt32(widthPoints), UInt32(heightPoints), true, "Beamer Teleport", serial) else {
            throw IPCError(.unsupported, "Der virtuelle Monitor liess sich nicht anlegen (privates CGVirtualDisplay-API nicht verfuegbar oder abgelehnt).")
        }
        return SystemVirtualDisplay(ref: r, w: widthPoints, h: heightPoints)
    }

    private init(ref: OpaquePointer, w: Int, h: Int) {
        self.ref = ref
        widthPoints = w
        heightPoints = h
    }

    deinit { destroy() }

    public var displayID: UInt32 { ref.map { TPVirtualDisplayID($0) } ?? 0 }
    public var bounds: CGRect { CGDisplayBounds(displayID) }

    public func resize(widthPoints: Int, heightPoints: Int) -> Bool {
        guard let r = ref, widthPoints >= 200, heightPoints >= 100 else { return false }
        guard TPVirtualDisplayResize(r, UInt32(widthPoints), UInt32(heightPoints), true) else { return false }
        self.widthPoints = widthPoints
        self.heightPoints = heightPoints
        return true
    }

    public func destroy() {
        guard let r = ref else { return }
        ref = nil
        TPVirtualDisplayDestroy(r)
    }
}

/// Verhindert den Leerlauf-Schlaf des Systems, solange mindestens ein Teleport laeuft (IOPMAssertion
/// PreventUserIdleSystemSleep). Zugeklappter Deckel: siehe README (die Assertion haelt den Mac dann nur im
/// Clamshell-Betrieb mit Netzteil und externem Monitor wach).
public final class SleepGuard {
    private let lock = NSLock()
    private var id: IOPMAssertionID = 0
    private var users = 0
    public init() {}

    public var isHeld: Bool { lock.lock(); defer { lock.unlock() }; return id != 0 }

    public func acquire(reason: String = "Beamer Teleport aktiv") {
        lock.lock(); defer { lock.unlock() }
        users += 1
        guard id == 0 else { return }
        var newID: IOPMAssertionID = 0
        let rc = IOPMAssertionCreateWithName("PreventUserIdleSystemSleep" as CFString,
                                             IOPMAssertionLevel(kIOPMAssertionLevelOn), reason as CFString, &newID)
        if rc == kIOReturnSuccess { id = newID } else { logErr("teleport: IOPMAssertion fehlgeschlagen (\(rc))") }
    }

    public func release() {
        lock.lock(); defer { lock.unlock() }
        users = max(0, users - 1)
        guard users == 0, id != 0 else { return }
        IOPMAssertionRelease(id)
        id = 0
    }
}
