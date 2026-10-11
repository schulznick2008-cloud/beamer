import CoreMedia
import CoreVideo
import Foundation

/// Quelle, die absichtlich haengt (Tests und die Umgebungsvariable `BEAMER_TELEPORT_TEST_SOURCE`). Sie liefert
/// keine Bilder und braucht deshalb keinen Encoder.
public final class HangingSource: FrameSource {
    public enum Mode: String {
        case hangStart = "hang-start"
        case hangStop = "hang-stop"
        case hangRefresh = "hang-refresh"
    }

    private let mode: Mode
    public init(mode: Mode) { self.mode = mode }

    public var state: String { "ok" }
    public var pixelSize: (width: Int, height: Int) { (320, 240) }
    public var scale: Double { 1 }

    private func hangForever() {
        let sem = DispatchSemaphore(value: 0)
        sem.wait()
    }

    public func start(onFrame: @escaping (CVPixelBuffer, CMTime) -> Void, onStop: @escaping (String) -> Void) throws {
        if mode == .hangStart { hangForever() }
    }

    public func stop() {
        if mode == .hangStop { hangForever() }
    }

    public func refresh() -> (state: String, sizeChanged: Bool) {
        if mode == .hangRefresh { hangForever() }
        return ("ok", false)
    }
}

public enum TestSources {
    /// `BEAMER_TELEPORT_TEST_SOURCE=hang-start|hang-stop|hang-refresh` ersetzt die Bildquelle durch eine haengende
    /// (Test der Abnahme "Helfer beendet sich auch, wenn ScreenCaptureKit nicht zurueckkehrt").
    static func fromEnvironment() -> ((CaptureMode, Int) throws -> FrameSource)? {
        guard let v = ProcessInfo.processInfo.environment["BEAMER_TELEPORT_TEST_SOURCE"],
              let m = HangingSource.Mode(rawValue: v) else { return nil }
        return { _, _ in HangingSource(mode: m) }
    }
}
