import AppKit
import CoreGraphics
import Foundation

/// Wohin ein zurueckkehrendes Fenster kommt: die Titelleiste liegt unter dem Mauszeiger (SCHNITTSTELLEN.md §2.1 /
/// C05b), auf den sichtbaren Bildschirm geklemmt. Rein rechnerisch, ohne Systemaufrufe.
public enum ReturnPlacement {
    public static let titleBarHeight: CGFloat = 28

    /// - Parameters:
    ///   - mouse: Mauszeiger (globale Punkte, Ursprung oben links)
    ///   - size: Fenstergroesse
    ///   - screen: Rahmen des Bildschirms, auf dem der Zeiger steht
    ///   - topInset: Platz oben, den die Menueleiste belegt
    public static func origin(mouse: CGPoint, size: CGSize, screen: CGRect, topInset: CGFloat) -> CGPoint {
        var x = mouse.x - size.width / 2
        var y = mouse.y - titleBarHeight / 2
        let minX = screen.minX
        let maxX = max(minX, screen.maxX - min(size.width, screen.width))
        let minY = screen.minY + topInset
        let maxY = max(minY, screen.maxY - titleBarHeight - 8) // die Titelleiste bleibt sichtbar
        x = min(max(x, minX), maxX)
        y = min(max(y, minY), maxY)
        return CGPoint(x: x, y: y)
    }
}

extension ReturnPlacement {
    /// Echte Umgebung: Mauszeiger, Bildschirm darunter, Platz fuer die Menueleiste.
    public static func systemOrigin(size: CGSize) -> CGPoint? {
        guard let mouse = CGEvent(source: nil)?.location else { return nil }
        var id: CGDirectDisplayID = 0
        var count: UInt32 = 0
        CGGetDisplaysWithPoint(mouse, 1, &id, &count)
        if count == 0 { id = CGMainDisplayID() }
        let screen = CGDisplayBounds(id)
        let ns = NSScreen.screens.first { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id }
        let inset = ns.map { $0.frame.maxY - $0.visibleFrame.maxY } ?? 0
        return origin(mouse: mouse, size: size, screen: screen, topInset: inset)
    }
}
