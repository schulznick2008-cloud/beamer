import AppKit
import CoreGraphics
import Foundation

/// Initialisiert die CoreGraphics-/SkyLight-Verbindung des Prozesses (CGS). Ein Kommandozeilenprozess hat sie nicht
/// von selbst; rufen ScreenCaptureKit oder CoreGraphics dann von einem Hintergrund-Thread auf (IPC-Befehle laufen
/// auf einer Queue), bricht der Prozess zeitweise mit
/// `Assertion failed: (did_initialize), function CGS_REQUIRE_INIT` ab (C02d). Deshalb einmal beim Start auf dem
/// Hauptthread aufrufen, vor jeder Capture- und Fensterarbeit.
public enum CoreGraphicsInit {
    nonisolated(unsafe) private static var done = false

    /// Muss auf dem Hauptthread laufen. Mehrfaches Aufrufen ist harmlos.
    public static func ensure() {
        precondition(Thread.isMainThread, "CoreGraphicsInit.ensure() gehoert auf den Hauptthread")
        guard !done else { return }
        done = true
        // NSApplication.shared richtet die Verbindung zum Window Server ein; ohne Dock-Symbol und Menueleiste.
        let app = NSApplication.shared
        app.setActivationPolicy(.prohibited)
        _ = CGMainDisplayID()
        var count: UInt32 = 0
        CGGetActiveDisplayList(0, nil, &count)
    }

    public static var isInitialized: Bool { done }
}
