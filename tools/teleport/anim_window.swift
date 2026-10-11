// Test-Fenster fuer die Mac-Sender-Messung (Paket C02): ein bewegtes Bild mit 60 Hz in einem Fenster
// von 1512x982 pt (Inhalt 1512x950 plus Titelleiste). Bauen und starten:
//   swiftc -O tools/teleport/anim_window.swift -o /tmp/teleport_anim && /tmp/teleport_anim &
// Danach: tools/teleport/send_demo.py --app teleport_anim ...
// Mit --dance aendert es nach 6 s die Groesse, minimiert bei 11 s, holt zurueck bei 16 s und schliesst bei 21 s.
import AppKit

final class AnimView: NSView {
    var frame_ = 0
    override func draw(_ r: NSRect) {
        NSColor(calibratedHue: CGFloat(frame_ % 360) / 360, saturation: 0.5, brightness: 0.8, alpha: 1).setFill()
        r.fill()
        for i in 0..<60 {
            let x = CGFloat((frame_ * 7 + i * 53) % max(1, Int(r.width)))
            let y = CGFloat((frame_ * 3 + i * 97) % max(1, Int(r.height)))
            NSColor(calibratedWhite: CGFloat(i % 5) / 5, alpha: 1).setFill()
            NSRect(x: x, y: y, width: 90, height: 60).fill()
        }
        ("Teleport Test \(frame_)" as NSString).draw(
            at: NSPoint(x: 40, y: 40), withAttributes: [.font: NSFont.systemFont(ofSize: 48)])
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let win = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 1512, height: 950),
                   styleMask: [.titled, .resizable, .closable, .miniaturizable], backing: .buffered, defer: false)
win.title = "TeleportAnimTest"
let view = AnimView(frame: win.contentView!.bounds)
view.autoresizingMask = [.width, .height]
win.contentView!.addSubview(view)
win.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { _ in
    view.frame_ += 1
    view.needsDisplay = true
}
if CommandLine.arguments.contains("--dance") {
    let q = DispatchQueue.main
    q.asyncAfter(deadline: .now() + 6) { win.setFrame(NSRect(x: 100, y: 100, width: 1200, height: 800), display: true) }
    q.asyncAfter(deadline: .now() + 11) { win.miniaturize(nil) }
    q.asyncAfter(deadline: .now() + 16) { win.deminiaturize(nil) }
    q.asyncAfter(deadline: .now() + 21) { win.close() }
}
app.run()
