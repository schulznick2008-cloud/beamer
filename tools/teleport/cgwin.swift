// Listet die Fenster auf dem Bildschirm (Besitzer, Ebene, Rahmen) als JSON-Zeilen. Hilfsprogramm fuer live_accept.py:
// erkennt z. B. ein offenes Kontextmenue (Ebene 101) einer App.
import CoreGraphics
import Foundation

let list = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]) ?? []
for w in list {
    let b = w[kCGWindowBounds as String] as? [String: Double] ?? [:]
    let o: [String: Any] = [
        "wid": w[kCGWindowNumber as String] as? Int ?? 0,
        "owner": w[kCGWindowOwnerName as String] as? String ?? "",
        "layer": w[kCGWindowLayer as String] as? Int ?? 0,
        "name": w[kCGWindowName as String] as? String ?? "",
        "x": b["X"] ?? 0, "y": b["Y"] ?? 0, "w": b["Width"] ?? 0, "h": b["Height"] ?? 0,
    ]
    if let d = try? JSONSerialization.data(withJSONObject: o), let s = String(data: d, encoding: .utf8) { print(s) }
}
