import CoreMedia
import Darwin
import Foundation

/// Gemeinsame Uhr fuer `pts_us` und Latenzmessung: Host-Zeit-Uhr in Mikrosekunden. ScreenCaptureKit
/// stempelt Frames mit derselben Uhr, deshalb ist "Capture bis dekodiert" auf einem Mac direkt messbar.
public enum HostClock {
    public static func nowUS() -> UInt64 {
        let t = CMClockGetTime(CMClockGetHostTimeClock())
        return UInt64(max(0, t.seconds * 1_000_000))
    }

    public static func nowTime() -> CMTime { CMClockGetTime(CMClockGetHostTimeClock()) }
}

/// CPU-Zeit des eigenen Prozesses (User + System), fuer die CPU-Last in den Statistiken.
public struct CPUMeter {
    private var lastCPU: Double
    private var lastWall: Double

    public init() {
        lastCPU = CPUMeter.cpuSeconds()
        lastWall = CPUMeter.wallSeconds()
    }

    static func cpuSeconds() -> Double {
        var ru = rusage()
        getrusage(RUSAGE_SELF, &ru)
        func s(_ t: timeval) -> Double { Double(t.tv_sec) + Double(t.tv_usec) / 1e6 }
        return s(ru.ru_utime) + s(ru.ru_stime)
    }

    static func wallSeconds() -> Double {
        Double(clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW)) / 1e9
    }

    /// Prozent eines Kerns seit dem letzten Aufruf (100 = ein Kern voll ausgelastet).
    public mutating func sample() -> Double {
        let c = CPUMeter.cpuSeconds()
        let w = CPUMeter.wallSeconds()
        defer { lastCPU = c; lastWall = w }
        let dw = w - lastWall
        return dw > 0 ? (c - lastCPU) / dw * 100 : 0
    }
}
