//! Treibt `beamer-teleport.exe --ipc stdio` wie Python es tut und startet dazu Selbsttest-Sender.
//! Prueft C03 ohne Mac: hello, recv.open (1 oder 2 Fenster), transport.open, transport.peer im
//! laufenden Stream, stats, Fenster schliessen -> proxy.closed, recv.close, EOF -> Exit 0 binnen 2 s.
//!
//!   cargo run --release --example ipc_driver -- --streams 2 --secs 10
//!
//! Optionen: --streams 1|2  --secs <s>  --fps <n>  --size BxH  --codec h264|hevc  --port <udp>
//!           --encoder <name>  --keep (Fenster am Ende offen lassen, bis Enter)
//!           --loss <%> (kuenstlicher Paketverlust im Empfaenger: Erholung per Keyframe pruefen)

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::Engine;
use beamer_teleport_win::selftest::test_keys;
use beamer_teleport_win::window::init_process_dpi;
use serde_json::{json, Value};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClientRect, PostMessageW, WM_CLOSE};

struct Opts {
    streams: u32,
    secs: f64,
    fps: u32,
    size: String,
    codec: String,
    port: u16,
    encoder: Option<String>,
    keep: bool,
    loss: f64,
}

fn parse() -> Opts {
    let mut o = Opts {
        streams: 2,
        secs: 10.0,
        fps: 60,
        size: "1920x1080".into(),
        codec: "h264".into(),
        port: 47001,
        encoder: None,
        keep: false,
        loss: 0.0,
    };
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < a.len() {
        let v = a.get(i + 1).cloned().unwrap_or_default();
        match a[i].as_str() {
            "--streams" => o.streams = v.parse().expect("streams"),
            "--secs" => o.secs = v.parse().expect("secs"),
            "--fps" => o.fps = v.parse().expect("fps"),
            "--size" => o.size = v,
            "--codec" => o.codec = v,
            "--port" => o.port = v.parse().expect("port"),
            "--encoder" => o.encoder = Some(v),
            "--loss" => o.loss = v.parse().expect("loss"),
            "--keep" => {
                o.keep = true;
                i += 1;
                continue;
            }
            x => panic!("unbekannte Option {x}"),
        }
        i += 2;
    }
    o
}

fn exe() -> PathBuf {
    // target/<profil>/examples/ipc_driver.exe -> target/<profil>/beamer-teleport.exe
    let me = std::env::current_exe().unwrap();
    me.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("beamer-teleport.exe")
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

struct Helper {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<Value>,
    next_id: i64,
    events: Vec<Value>,
}

impl Helper {
    fn start(loss: f64) -> Helper {
        let mut child = Command::new(exe())
            .args([
                "--ipc",
                "stdio",
                "--same-clock",
                "--test-loss",
                &loss.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("beamer-teleport.exe starten (vorher cargo build)");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Value>(&line) {
                    Ok(v) => {
                        let _ = tx.send(v);
                    }
                    Err(_) => panic!("stdout enthaelt keine JSON-Zeile: {line}"),
                }
            }
        });
        let stdin = child.stdin.take();
        Helper {
            child,
            stdin,
            rx,
            next_id: 1,
            events: Vec::new(),
        }
    }

    fn cmd(&mut self, mut v: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        v["v"] = json!(1);
        v["id"] = json!(id);
        let s = self.stdin.as_mut().unwrap();
        writeln!(s, "{v}").unwrap();
        s.flush().unwrap();
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let m = self.rx.recv_timeout(left).expect("keine Antwort");
            if m.get("re").and_then(Value::as_i64) == Some(id) {
                return m;
            }
            self.events.push(m);
        }
    }

    fn pump(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while let Ok(m) = self
            .rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
        {
            self.events.push(m);
        }
    }

    fn wait_event(&mut self, name: &str, d: Duration) -> Option<Value> {
        let end = Instant::now() + d;
        if let Some(i) = self.events.iter().position(|e| e["ev"] == name) {
            return Some(self.events.remove(i));
        }
        while let Ok(m) = self
            .rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
        {
            if m["ev"] == name {
                return Some(m);
            }
            self.events.push(m);
        }
        None
    }
}

fn sender(o: &Opts, tid: u32) -> Child {
    let mut c = Command::new(exe());
    c.args(["--selftest-send", "127.0.0.1", &o.port.to_string()])
        .args(["--tid", &tid.to_string(), "--fps", &o.fps.to_string()])
        .args(["--size", &o.size, "--codec", &o.codec])
        .args(["--local-port", &(47100 + tid).to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if let Some(e) = &o.encoder {
        c.args(["--encoder", e]);
    }
    c.spawn().expect("Selbsttest-Sender starten")
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn check(ok: bool, what: &str, fails: &mut Vec<String>) {
    println!("  {} {what}", if ok { "OK  " } else { "FEHL" });
    if !ok {
        fails.push(what.to_owned());
    }
}

fn main() {
    let o = parse();
    // Sonst virtualisiert Windows die Fenstergroessen des Helfers fuer diesen Prozess.
    init_process_dpi();
    let mut fails = Vec::new();
    let mut h = Helper::start(o.loss);
    let r = h.cmd(json!({"cmd":"hello","beamer_version":"driver"}));
    check(
        r["ok"] == true && r["platform"] == "win",
        "hello",
        &mut fails,
    );
    let r = h.cmd(json!({"cmd":"send.start","tid":9}));
    check(
        r["error"]["code"] == "unsupported",
        "send.start -> unsupported",
        &mut fails,
    );
    let r = h.cmd(json!({"cmd":"proxy.input","tid":9}));
    check(
        r["error"]["code"] == "unsupported",
        "proxy.input -> unsupported",
        &mut fails,
    );

    let (w, hgt): (f64, f64) = {
        let (a, b) = o.size.split_once('x').unwrap();
        (a.parse().unwrap(), b.parse().unwrap())
    };
    for tid in 1..=o.streams {
        let k = test_keys(tid);
        let r = h.cmd(
            json!({"cmd":"recv.open","tid":tid,"title":format!("Selbsttest {tid}"),"app":"Teleport",
            // Fenster halb so gross wie der Stream (in Punkten), damit zwei nebeneinander passen.
            "w": w / 2.0, "h": hgt / 2.0, "scale": 1.0, "codec": o.codec, "fullscreen": false,
            "keys": {"s2r_key": b64(&k.s2r_key), "s2r_prefix": b64(&k.s2r_prefix),
                     "r2s_key": b64(&k.r2s_key), "r2s_prefix": b64(&k.r2s_prefix)}}),
        );
        check(
            r["ok"] == true,
            &format!("recv.open tid {tid}: {r}"),
            &mut fails,
        );
    }
    let r = h.cmd(json!({"cmd":"transport.open","local_port":o.port,"peer_host":"","peer_port":0}));
    check(
        r["ok"] == true && r["local_port"] == o.port,
        "transport.open",
        &mut fails,
    );

    let mut senders: Vec<Child> = (1..=o.streams).map(|t| sender(&o, t)).collect();
    // Anlaufen lassen (Encoder-Init, erster Keyframe), Messwerte davor verwerfen.
    h.pump(Duration::from_secs(2));
    h.events.clear();

    // transport.peer im laufenden Stream (§7): auf die Quelladresse des ersten Senders.
    let half = Duration::from_secs_f64(o.secs / 2.0);
    h.pump(half);
    let r = h.cmd(json!({"cmd":"transport.peer","peer_host":"127.0.0.1","peer_port":47101}));
    check(
        r["ok"] == true,
        "transport.peer bei laufendem Stream",
        &mut fails,
    );
    h.pump(half);

    let stats: Vec<Value> = h
        .events
        .iter()
        .filter(|e| e["ev"] == "stats")
        .cloned()
        .collect();
    println!("\nstats ({} Ereignisse):", stats.len());
    for tid in 1..=o.streams {
        let s: Vec<&Value> = stats.iter().filter(|e| e["tid"] == tid).collect();
        let col = |k: &str| s.iter().filter_map(|e| e[k].as_f64()).collect::<Vec<f64>>();
        let fps = median(col("fps"));
        let r2p = median(col("recv_to_present_ms"));
        println!(
            "  tid {tid}: fps {:.1}  kbps {:.0}  Empfang->Present Median {:.2} ms (p95 {:.2})  decode {:.2} ms  present {:.2} ms  \
             Bildalter (gleiche Uhr) {:.2} ms  Decode-Fehler {}  {}",
            fps,
            median(col("kbps")),
            r2p,
            median(col("recv_to_present_p95_ms")),
            median(col("decode_ms")),
            median(col("present_ms")),
            median(col("same_clock_latency_ms")),
            col("decode_errors").iter().sum::<f64>(),
            s.last().map(|e| e["decoder"].to_string()).unwrap_or_default()
        );
        check(
            !s.is_empty(),
            &format!("tid {tid}: stats jede Sekunde"),
            &mut fails,
        );
        if o.loss > 0.0 {
            let presented: f64 = col("frames_presented").iter().sum();
            println!(
                "        Verlust {} %: angezeigt {presented:.0}, uebersprungen {:.0}, verworfen (Transport) {:.0}, FEC repariert {:.0}, NACKs {:.0}",
                o.loss,
                col("frames_skipped").iter().sum::<f64>(),
                s.last().and_then(|e| e["frames_dropped"].as_f64()).unwrap_or(0.0),
                s.last().and_then(|e| e["fec_recovered"].as_f64()).unwrap_or(0.0),
                s.last().and_then(|e| e["nacks"].as_f64()).unwrap_or(0.0),
            );
            check(
                fps > o.fps as f64 * 0.5,
                &format!("tid {tid}: Bild laeuft trotz Verlust"),
                &mut fails,
            );
        } else {
            check(
                fps > o.fps as f64 * 0.9,
                &format!("tid {tid}: fps >= 90 % von {}", o.fps),
                &mut fails,
            );
        }
        check(
            r2p < 10.0,
            &format!("tid {tid}: Median Empfang->Present < 10 ms"),
            &mut fails,
        );
        let need = [
            "fps",
            "kbps",
            "rtt_ms",
            "loss_pct",
            "latency_ms",
            "decode_ms",
            "present_ms",
        ];
        check(
            s.iter().all(|e| need.iter().all(|k| e.get(*k).is_some())),
            &format!("tid {tid}: stats-Felder vollstaendig"),
            &mut fails,
        );
    }

    if o.keep {
        println!("\nFenster bleiben offen. Enter beendet.");
        let mut l = String::new();
        let _ = std::io::stdin().read_line(&mut l);
    }

    // Fenster 1 wie ein Nutzer schliessen -> proxy.closed, danach ist recv.close kein Fehler.
    let title = HSTRING::from("Teleport \u{2013} Selbsttest 1");
    let hwnd = unsafe { FindWindowW(w!("BeamerTeleportProxy"), &title) };
    check(
        hwnd.is_ok(),
        "Fenster 1 gefunden (Titel \"<app> \u{2013} <title>\")",
        &mut fails,
    );
    if let Ok(hwnd) = hwnd {
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let ev = h.wait_event("proxy.closed", Duration::from_secs(3));
        check(
            ev.as_ref().is_some_and(|e| e["tid"] == 1),
            "Schliessen -> proxy.closed tid 1",
            &mut fails,
        );
        let r = h.cmd(json!({"cmd":"recv.close","tid":1}));
        check(r["ok"] == true, "recv.close nach proxy.closed", &mut fails);
    }
    if o.streams > 1 {
        let r = h.cmd(json!({"cmd":"recv.update","tid":2,"title":"umbenannt","w":640,"h":360}));
        check(r["ok"] == true, "recv.update tid 2", &mut fails);
        h.pump(Duration::from_millis(500));
        // Neuer Titel, Groesse = Punkte x DPI-Skalierung des Monitors (Per-Monitor-DPI-Aware V2).
        let title = HSTRING::from("Teleport \u{2013} umbenannt");
        let hwnd = unsafe { FindWindowW(w!("BeamerTeleportProxy"), &title) };
        check(hwnd.is_ok(), "recv.update: Titel geaendert", &mut fails);
        if let Ok(hwnd) = hwnd {
            let mut rc = RECT::default();
            let dpi = unsafe {
                let _ = GetClientRect(hwnd, &mut rc);
                GetDpiForWindow(hwnd)
            };
            let px = |pt: f64| (pt * dpi as f64 / 96.0).round() as i32;
            let want = (px(640.0), px(360.0));
            let got = (rc.right - rc.left, rc.bottom - rc.top);
            println!("  Fenster 2: Client {got:?} bei {dpi} dpi, erwartet {want:?}");
            check(
                got == want,
                "recv.update: Groesse = Punkte x DPI",
                &mut fails,
            );
        }
        let r = h.cmd(json!({"cmd":"recv.close","tid":2}));
        check(r["ok"] == true, "recv.close tid 2", &mut fails);
    }
    let r = h.cmd(json!({"cmd":"recv.close","tid":77}));
    check(
        r["error"]["code"] == "not_found",
        "recv.close unbekannt -> not_found",
        &mut fails,
    );

    // Ein Fenster offen lassen und per EOF beenden: muss binnen 2 s mit Code 0 enden.
    let k = test_keys(50);
    let r = h.cmd(
        json!({"cmd":"recv.open","tid":50,"title":"EOF","app":"Teleport","w":320,"h":180,
        "codec":o.codec,"keys":{"s2r_key": b64(&k.s2r_key), "s2r_prefix": b64(&k.s2r_prefix),
        "r2s_key": b64(&k.r2s_key), "r2s_prefix": b64(&k.r2s_prefix)}}),
    );
    check(r["ok"] == true, "recv.open vor EOF", &mut fails);
    for s in &mut senders {
        let _ = s.kill();
        let _ = s.wait();
    }
    let t = Instant::now();
    drop(h.stdin.take());
    let mut code = None;
    while t.elapsed() < Duration::from_secs(5) {
        if let Some(st) = h.child.try_wait().unwrap() {
            code = st.code();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let dt = t.elapsed().as_secs_f64();
    check(
        code == Some(0) && dt < 2.0,
        &format!("EOF -> Exit {code:?} nach {dt:.2} s"),
        &mut fails,
    );
    if code.is_none() {
        let _ = h.child.kill();
    }

    if fails.is_empty() {
        println!("\nALLES OK");
    } else {
        println!("\n{} FEHLER: {:?}", fails.len(), fails);
        std::process::exit(1);
    }
}
