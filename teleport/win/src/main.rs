use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use beamer_teleport_win::{app, decode, ipc, selftest, window};

const USAGE: &str = "\
beamer-teleport.exe --ipc stdio [--same-clock] [--test-loss PCT]
beamer-teleport.exe --selftest-send <host> <port> [--tid N] [--fps N] [--size BxH] [--codec h264|hevc]
                    [--kbps N] [--secs S] [--encoder NAME] [--local-port N]

--ipc stdio        JSON Lines auf stdin/stdout (docs/teleport/SCHNITTSTELLEN.md), Log auf stderr.
--same-clock       Sender teilt die Uhr (Selbsttest auf einem Rechner): stats enthaelt same_clock_latency_ms.
--test-loss PCT    Nur Tests: kuenstlicher Paketverlust im Transport (Keyframe-Anforderung pruefen).
--selftest-send    Testbild mit festen Testschluesseln senden (nur fuer lokale Tests).";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    decode::init_logging();
    let code = match args.first().map(String::as_str) {
        Some("--ipc") if args.get(1).map(String::as_str) == Some("stdio") => {
            let rest = &args[2..];
            let same_clock = rest.iter().any(|a| a == "--same-clock");
            let test_loss_pct = rest
                .iter()
                .position(|a| a == "--test-loss")
                .and_then(|i| rest.get(i + 1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            window::init_process_dpi();
            app::App::new(
                ipc::Out::stdout(),
                app::Options {
                    same_clock,
                    test_loss_pct,
                },
            )
            .run()
        }
        Some("--selftest-send") => match selftest::parse_args(&args[1..]) {
            Ok(o) => match selftest::run(&o, Arc::new(AtomicBool::new(false))) {
                Ok(r) => {
                    eprintln!(
                        "beamer-teleport: Selbsttest fertig: {} Frames ({} Keyframes), {} abgelehnt, Encoder {} (Median {:.2} ms)",
                        r.frames, r.keyframes, r.refused, r.encoder, r.encode_ms_median
                    );
                    0
                }
                Err(e) => {
                    eprintln!("beamer-teleport: Selbsttest: {e}");
                    1
                }
            },
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                2
            }
        },
        Some("--help") | Some("-h") => {
            eprintln!("{USAGE}");
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}
