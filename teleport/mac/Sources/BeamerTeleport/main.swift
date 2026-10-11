import Darwin
import Foundation
import TeleportCore

setvbuf(stdout, nil, _IOLBF, 0)
signal(SIGPIPE, SIG_IGN)

func usage() -> Never {
    let text = """
    BeamerTeleport \(Dispatcher.helperVersion) (Mac-Sender, SCHNITTSTELLEN.md v1.1)

      BeamerTeleport --ipc stdio
          JSON-Lines-Protokoll auf stdin/stdout (hello, transport.*, windows.list, send.*, shutdown)
      BeamerTeleport --selftest-recv <port> [--seconds N] [--tid N] [--dump-dir DIR]
          Empfaenger ohne Anzeige: dekodiert mit VideoToolbox, protokolliert fps, Latenz, Verlust
      BeamerTeleport --selftest-synth <host:port> [--seconds N] [--size WxH] [--fps N] [--codec hevc|h264] [--kbps N]
          Sender mit Muster-Quelle (ohne Bildschirmaufnahme-Freigabe) ueber denselben Encoder-/Transportweg
      BeamerTeleport --version

    """
    FileHandle.standardError.write(Data(text.utf8))
    exit(2)
}

let args = Array(CommandLine.arguments.dropFirst())

func option(_ name: String) -> String? {
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { return nil }
    return args[i + 1]
}

func printJSON(_ j: JSON) { print(JSONLine.encode(j)) }

guard let mode = args.first else { usage() }

switch mode {
case "--version":
    print(Dispatcher.helperVersion)

case "--ipc":
    guard args.count >= 2, args[1] == "stdio" else { usage() }
    let writer = LineWriter()
    let engine = SenderEngine { writer.write(JSONLine.encode($0)) }
    // SIGTERM/SIGINT/SIGHUP: Fenster zurueckschieben, virtuelle Monitore entfernen, dann beenden.
    var signalSources: [DispatchSourceSignal] = []
    for sig in [SIGTERM, SIGINT, SIGHUP] {
        signal(sig, SIG_IGN)
        let src = DispatchSource.makeSignalSource(signal: sig, queue: .global(qos: .userInitiated))
        src.setEventHandler {
            DispatchQueue.global().asyncAfter(deadline: .now() + 1.8) { exit(0) }
            engine.shutdown()
            exit(0)
        }
        src.resume()
        signalSources.append(src)
    }
    CrashRestore.install()
    IPCServer(engine: engine, writer: writer).run()
    exit(0)

case "--selftest-recv":
    guard args.count >= 2, let port = UInt16(args[1]) else { usage() }
    StopSignal.install()
    let r = SelfTestReceiver(port: port, tid: UInt32(option("--tid") ?? "1") ?? 1,
                             seconds: Int(option("--seconds") ?? "0") ?? 0) { print($0) }
    r.dumpDir = option("--dump-dir")
    do { printJSON(try r.run()) } catch { logErr("selftest-recv: \(error)"); exit(1) }

case "--selftest-synth":
    guard args.count >= 2 else { usage() }
    let parts = args[1].split(separator: ":")
    guard parts.count == 2, let port = UInt16(parts[1]) else { usage() }
    let size = (option("--size") ?? "1512x982").split(separator: "x").compactMap { Int($0) }
    guard size.count == 2 else { usage() }
    StopSignal.install()
    let s = SelfTestSynthSender(
        host: String(parts[0]), port: port, width: size[0], height: size[1],
        fps: Int(option("--fps") ?? "60") ?? 60, codec: Codec(rawValue: option("--codec") ?? "hevc") ?? .hevc,
        kbps: UInt32(option("--kbps") ?? "20000") ?? 20_000,
        seconds: Int(option("--seconds") ?? "10") ?? 10) { print($0) }
    do { printJSON(try s.run()) } catch { logErr("selftest-synth: \(error)"); exit(1) }

default:
    usage()
}
