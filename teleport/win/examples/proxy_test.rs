//! C05-Abnahme ohne Mac: Helfer + Selbsttest-Sender starten, dann Fenster-Nachrichten an das Proxy-Fenster
//! schicken (PostMessage, ohne echte Maus/Tastatur des Nutzers) und die Ereignisse pruefen:
//! `proxy.input` (move/down/up/scroll/key/text, Stream-Pixel), Loslassen bei Fokusverlust, `proxy.focus`,
//! `proxy.resize` (entprellt, letzter Wert), Vollbild bei `recv.open`, Icon und AppUserModelID.
//!
//!   cargo run --release --example proxy_test

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::Engine;
use beamer_teleport_win::selftest::test_keys;
use serde_json::{json, Value};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

fn exe() -> PathBuf {
    let me = std::env::current_exe().unwrap();
    me.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("beamer-teleport.exe")
}

struct Helper {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<Value>,
    next_id: i64,
}

impl Helper {
    fn start() -> Helper {
        let mut child = Command::new(exe())
            .args(["--ipc", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("beamer-teleport.exe (vorher cargo build --release)");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line).expect("nur JSON auf stdout");
                let _ = tx.send(v);
            }
        });
        let stdin = child.stdin.take();
        Helper {
            child,
            stdin,
            rx,
            next_id: 1,
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
        loop {
            let m = self
                .rx
                .recv_timeout(Duration::from_secs(15))
                .expect("keine Antwort");
            if m.get("re").and_then(Value::as_i64) == Some(id) {
                return m;
            }
        }
    }

    /// Alle Ereignisse `ev` (ausser stats) in `d` einsammeln.
    fn collect(&mut self, d: Duration) -> Vec<Value> {
        let end = Instant::now() + d;
        let mut out = Vec::new();
        while let Ok(m) = self
            .rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
        {
            if m["ev"].is_string() && m["ev"] != "stats" {
                out.push(m);
            }
        }
        out
    }
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn keys_json(tid: u32) -> Value {
    let k = test_keys(tid);
    json!({"s2r_key": b64(&k.s2r_key), "s2r_prefix": b64(&k.s2r_prefix),
           "r2s_key": b64(&k.r2s_key), "r2s_prefix": b64(&k.r2s_prefix)})
}

/// 1x1-PNG (rot).
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn find(title: &str) -> Option<HWND> {
    unsafe { FindWindowW(w!("BeamerTeleportProxy"), &HSTRING::from(title)).ok() }
}

fn post(h: HWND, msg: u32, wp: usize, lp: isize) {
    unsafe {
        let _ = PostMessageW(Some(h), msg, WPARAM(wp), LPARAM(lp));
    }
}

fn xy(x: i32, y: i32) -> isize {
    ((y as u16 as isize) << 16) | (x as u16 as isize)
}

/// lParam einer Tastennachricht: Scan-Code, Bit 24 erweitert, Bit 30 vorher gedrueckt (Wiederholung).
fn key_lp_ext(scan: u32, extended: bool, repeat: bool) -> isize {
    let mut v = 1 | ((scan as isize) << 16);
    if extended {
        v |= 1 << 24;
    }
    if repeat {
        v |= 1 << 30;
    }
    v
}

/// Zeichen, das das aktuelle Layout fuer AltGr+Taste liefert (None: kein AltGr-Zeichen).
fn altgr_char(vk: u32, scan: u32) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, ToUnicodeEx};
    let mut state = [0u8; 256];
    for k in [0x11usize, 0x12, 0xA2, 0xA5] {
        state[k] = 0x80;
    }
    let mut buf = [0u16; 8];
    let n = unsafe { ToUnicodeEx(vk, scan, &state, &mut buf, 4, Some(GetKeyboardLayout(0))) };
    (n == 1).then(|| String::from_utf16_lossy(&buf[..1]))
}

fn key_lp(scan: u32, up: bool) -> isize {
    let mut v = 1 | ((scan as isize) << 16);
    if up {
        v |= (1 << 30) | (1 << 31);
    }
    v
}

fn inputs(evs: &[Value]) -> Vec<Value> {
    evs.iter()
        .filter(|e| e["ev"] == "proxy.input")
        .map(|e| e["input"].clone())
        .collect()
}

fn check(ok: bool, what: &str, fails: &mut Vec<String>) {
    println!("  {} {what}", if ok { "OK  " } else { "FEHL" });
    if !ok {
        fails.push(what.to_owned());
    }
}

fn client_size(h: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    (r.right, r.bottom)
}

fn main() {
    beamer_teleport_win::window::init_process_dpi();
    let mut fails = Vec::new();
    let mut h = Helper::start();
    // Fenster 960x540 Punkte fuer einen 1920x1080-Stream: Stream-Pixel = 2 x Client-Pixel bei 96 dpi.
    let r = h.cmd(
        json!({"cmd":"recv.open","tid":1,"title":"Eingabe","app":"Finanzen","w":960,"h":540,
        "scale":1,"codec":"h264","icon_png": b64(PNG),"keys":keys_json(1)}),
    );
    check(r["ok"] == true, "recv.open mit icon_png", &mut fails);
    let r = h.cmd(json!({"cmd":"transport.open","local_port":47001,"peer_host":"","peer_port":0}));
    check(r["ok"] == true, "transport.open", &mut fails);
    let mut sender = Command::new(exe())
        .args(["--selftest-send", "127.0.0.1", "47001", "--tid", "1"])
        .args([
            "--fps",
            "30",
            "--size",
            "1920x1080",
            "--local-port",
            "47160",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    h.collect(Duration::from_secs(2)); // erstes Bild, Stream-Groesse bekannt
    let hw = find("Finanzen \u{2013} Eingabe").expect("Proxy-Fenster");
    let (cw, ch) = client_size(hw);
    let sx = 1920.0 / cw as f64;
    println!("  Client {cw}x{ch}, Stream 1920x1080, Faktor {sx:.3}");

    // --- Taskleiste: Icon und AppUserModelID
    let icon = unsafe {
        SendMessageW(
            hw,
            WM_GETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(0)),
        )
    };
    check(icon.0 != 0, "Icon aus icon_png gesetzt", &mut fails);
    let aumid = unsafe {
        SHGetPropertyStoreForWindow::<IPropertyStore>(hw)
            .and_then(|s| s.GetValue(&PKEY_AppUserModel_ID))
            .map(|pv| pv.to_string())
            .unwrap_or_default()
    };
    check(
        aumid == "Beamer.Teleport.Finanzen",
        &format!("AppUserModelID je App ({aumid})"),
        &mut fails,
    );

    // --- Maus
    post(hw, WM_MOUSEMOVE, 0, xy(100, 50));
    post(hw, WM_LBUTTONDOWN, 1, xy(100, 50));
    post(hw, WM_LBUTTONUP, 0, xy(100, 50));
    post(hw, WM_LBUTTONDOWN, 1, xy(100, 50));
    post(hw, WM_LBUTTONUP, 0, xy(100, 50));
    post(hw, WM_RBUTTONDOWN, 2, xy(10, 10));
    post(hw, WM_RBUTTONUP, 0, xy(10, 10));
    post(hw, WM_XBUTTONDOWN, 1 << 16, xy(10, 10));
    post(hw, WM_XBUTTONUP, 1 << 16, xy(10, 10));
    let mut pt = POINT { x: 200, y: 100 };
    unsafe {
        let _ = ClientToScreen(hw, &mut pt);
    }
    post(hw, WM_MOUSEWHEEL, (120usize) << 16, xy(pt.x, pt.y));
    post(
        hw,
        WM_MOUSEWHEEL,
        ((-240i16) as u16 as usize) << 16,
        xy(pt.x, pt.y),
    );
    post(hw, WM_MOUSEHWHEEL, (120usize) << 16, xy(pt.x, pt.y));
    let ev = inputs(&h.collect(Duration::from_millis(400)));
    let want_x = (100.0 * sx * 100.0).round() / 100.0;
    check(
        ev.first()
            .is_some_and(|v| v["k"] == "move" && v["x"] == want_x && v["y"] == want_x / 2.0),
        &format!("move in Stream-Pixeln (x {want_x}): {:?}", ev.first()),
        &mut fails,
    );
    let downs: Vec<&Value> = ev.iter().filter(|v| v["k"] == "down").collect();
    check(
        downs.len() == 4
            && downs[0]["b"] == "left"
            && downs[0]["n"] == 1
            && downs[1]["n"] == 2
            && downs[2]["b"] == "right"
            && downs[3]["b"] == "back",
        "down left n=1, Doppelklick n=2, right, back",
        &mut fails,
    );
    check(
        ev.iter().filter(|v| v["k"] == "up").count() == 4,
        "4x up",
        &mut fails,
    );
    let sc: Vec<&Value> = ev.iter().filter(|v| v["k"] == "scroll").collect();
    check(
        sc.len() == 3
            && sc[0]["dy"] == 1.0
            && sc[1]["dy"] == -2.0
            && sc[2]["dx"] == 1.0
            && sc[0]["unit"] == "line"
            && (sc[0]["x"].as_f64().unwrap() - 200.0 * sx).abs() < 0.6,
        &format!("scroll dy +1/-2, dx +1, unit line, Position: {sc:?}"),
        &mut fails,
    );

    // --- Tastatur (roh, Strg = "ctrl"), Text, Loslassen bei Fokusverlust
    post(hw, WM_KEYDOWN, 0x11, key_lp(0x1D, false)); // Strg links
    post(hw, WM_KEYDOWN, 0x41, key_lp(0x1E, false)); // A
    post(hw, WM_KEYUP, 0x41, key_lp(0x1E, true));
    post(hw, WM_KEYDOWN, 0x0D, key_lp(0x1C, false)); // Enter
    post(hw, WM_CHAR, 0xE9, 0); // zusammengesetztes "e" mit Akut (tote Taste/IME)
    post(hw, WM_KEYDOWN, 0x5B, key_lp(0x5B, false)); // Windows-Taste: bleibt bei Windows
    post(hw, WM_KILLFOCUS, 0, 0); // Alt+Tab: alles loslassen
    let ev = inputs(&h.collect(Duration::from_millis(400)));
    println!("  Tastatur: {ev:?}");
    let keys: Vec<(String, bool)> = ev
        .iter()
        .filter(|v| v["k"] == "key")
        .map(|v| (v["key"].as_str().unwrap().to_owned(), v["down"] == true))
        .collect();
    println!("  Tasten: {keys:?}");
    check(
        keys.starts_with(&[
            ("ctrl".into(), true),
            ("a".into(), true),
            ("a".into(), false),
            ("enter".into(), true),
        ]),
        "Strg roh als \"ctrl\", Zeichentaste \"a\", Enter",
        &mut fails,
    );
    check(
        !keys.iter().any(|(k, _)| k == "cmd"),
        "Windows-Taste nicht weitergegeben",
        &mut fails,
    );
    check(
        ev.iter().any(|v| *v == json!({"k":"text","text":"\u{e9}"}))
            && !ev.iter().any(|v| v["text"] == "\u{1}"),
        "Text \"e-Akut\" als text, Steuerzeichen nicht",
        &mut fails,
    );
    let ups_after: Vec<&(String, bool)> = keys.iter().skip(4).collect();
    check(
        ups_after.contains(&&("ctrl".to_string(), false))
            && ups_after.contains(&&("enter".to_string(), false)),
        "Fokusverlust: ctrl und enter bekommen up (kein haengender Modifier)",
        &mut fails,
    );

    // WM_CHAR erzeugt der Helfer selbst (TranslateMessage in seiner Nachrichtenschleife), hier nicht posten.
    // --- Gedrueckt halten: nur ein down (Wiederholung macht der Sender)
    post(hw, WM_ACTIVATE, 1, 0);
    post(hw, WM_KEYDOWN, 0x42, key_lp_ext(0x30, false, false)); // B
    post(hw, WM_KEYDOWN, 0x42, key_lp_ext(0x30, false, true)); // Wiederholung
    post(hw, WM_KEYDOWN, 0x42, key_lp_ext(0x30, false, true));
    post(hw, WM_KEYUP, 0x42, key_lp(0x30, true));
    let ev = inputs(&h.collect(Duration::from_millis(300)));
    println!("  Halten: {ev:?}");
    let b: Vec<&Value> = ev.iter().filter(|v| v["key"] == "b").collect();
    check(
        b.len() == 2
            && b[0]["down"] == true
            && b[1]["down"] == false
            && !ev.iter().any(|v| v["k"] == "text"),
        &format!(
            "Gedrueckt halten: ein down, ein up, kein Text ({} Ereignisse)",
            ev.len()
        ),
        &mut fails,
    );

    // --- Shift vor der Taste loslassen: up traegt den Namen vom down ("A"), nicht "a".
    // Sonst bleibt A beim Sender unten und das naechste Shift+A gilt als Wiederholung.
    post(hw, WM_KEYDOWN, 0x10, key_lp(0x2A, false)); // Shift links
    post(hw, WM_KEYDOWN, 0x41, key_lp(0x1E, false)); // A
    post(hw, WM_KEYUP, 0x10, key_lp(0x2A, true));
    post(hw, WM_KEYUP, 0x41, key_lp(0x1E, true));
    post(hw, WM_KEYDOWN, 0x10, key_lp(0x2A, false));
    post(hw, WM_KEYDOWN, 0x41, key_lp(0x1E, false));
    post(hw, WM_KEYUP, 0x41, key_lp(0x1E, true));
    post(hw, WM_KEYUP, 0x10, key_lp(0x2A, true));
    let ev = inputs(&h.collect(Duration::from_millis(400)));
    println!("  Shift vor A: {ev:?}");
    let shift_keys: Vec<(String, bool)> = ev
        .iter()
        .filter(|v| v["k"] == "key")
        .map(|v| (v["key"].as_str().unwrap().to_owned(), v["down"] == true))
        .collect();
    check(
        shift_keys
            == vec![
                ("shift".into(), true),
                ("A".into(), true),
                ("shift".into(), false),
                ("A".into(), false),
                ("shift".into(), true),
                ("A".into(), true),
                ("A".into(), false),
                ("shift".into(), false),
            ]
            && !ev.iter().any(|v| v["k"] == "text"),
        &format!(
            "Shift vor A losgelassen: up bleibt \"A\", zweites Shift+A kommt an: {shift_keys:?}"
        ),
        &mut fails,
    );

    // --- AltGr: kuenstliches linkes Strg + rechtes Alt (erweitert), dann Q -> Text (z. B. "@" auf Deutsch)
    match altgr_char(0x51, 0x10) {
        Some(want) => {
            post(hw, WM_KEYDOWN, 0x11, key_lp_ext(0x1D, false, false));
            post(hw, WM_KEYDOWN, 0x12, key_lp_ext(0x38, true, false));
            post(hw, WM_KEYDOWN, 0x51, key_lp_ext(0x10, false, false));
            post(hw, WM_KEYUP, 0x51, key_lp(0x10, true));
            post(hw, WM_KEYUP, 0x11, key_lp(0x1D, true));
            post(hw, WM_KEYUP, 0x12, key_lp(0x38, true) | (1 << 24));
            post(hw, WM_KEYDOWN, 0x41, key_lp(0x1E, false)); // danach normal: a
            post(hw, WM_KEYUP, 0x41, key_lp(0x1E, true));
            let ev = inputs(&h.collect(Duration::from_millis(300)));
            println!("  AltGr+Q: {ev:?}");
            let texts: Vec<&Value> = ev.iter().filter(|v| v["k"] == "text").collect();
            check(
                texts.len() == 1
                    && texts[0]["text"] == want.as_str()
                    && !ev
                        .iter()
                        .any(|v| v["key"] == "ctrl" || v["key"] == "alt_r" || v["key"] == "q")
                    && ev.iter().any(|v| v["key"] == "a" && v["down"] == true),
                &format!(
                    "AltGr+Q als text {want:?}, ohne ctrl/alt_r/q; danach wieder normale Tasten"
                ),
                &mut fails,
            );
        }
        None => println!("  (AltGr-Test uebersprungen: Layout ohne AltGr-Zeichen auf Q)"),
    }

    // --- Fokus
    post(hw, WM_ACTIVATE, 1, 0);
    post(hw, WM_ACTIVATE, 0, 0);
    let ev = h.collect(Duration::from_millis(300));
    let foc: Vec<bool> = ev
        .iter()
        .filter(|e| e["ev"] == "proxy.focus")
        .map(|e| e["focused"] == true)
        .collect();
    // Nur Wechsel werden gemeldet: Werte wechseln sich ab, am Ende inaktiv.
    check(
        foc.last() == Some(&false) && foc.windows(2).all(|w| w[0] != w[1]),
        &format!("proxy.focus nur bei Wechsel, am Ende false: {foc:?}"),
        &mut fails,
    );

    // --- Resize: 15 Groessen in 300 ms, dann Ende des Ziehens
    let t = Instant::now();
    for i in 0..15 {
        unsafe {
            let _ = SetWindowPos(
                hw,
                None,
                0,
                0,
                700 + i * 20,
                400 + i * 10,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    post(hw, WM_EXITSIZEMOVE, 0, 0);
    let ev = h.collect(Duration::from_millis(500));
    let rs: Vec<&Value> = ev.iter().filter(|e| e["ev"] == "proxy.resize").collect();
    let (fw, fh) = client_size(hw);
    let span = t.elapsed().as_secs_f64();
    println!(
        "  proxy.resize: {} Meldungen in {span:.2} s, letzte {:?}",
        rs.len(),
        rs.last()
    );
    check(
        !rs.is_empty() && rs.len() as f64 <= span * 10.0 + 2.0,
        "proxy.resize entprellt (<= 10/s)",
        &mut fails,
    );
    check(
        rs.last()
            .is_some_and(|e| e["px_w"] == fw && e["px_h"] == fh && e["w"].is_number()),
        &format!("letzter Wert = Endgroesse {fw}x{fh}"),
        &mut fails,
    );
    // Antwort des Senders (recv.update) darf kein proxy.resize ausloesen.
    let r = h.cmd(json!({"cmd":"recv.update","tid":1,"w":800,"h":450}));
    let echo = h
        .collect(Duration::from_millis(400))
        .into_iter()
        .filter(|e| e["ev"] == "proxy.resize")
        .count();
    check(
        r["ok"] == true && echo == 0,
        "recv.update ohne proxy.resize-Echo",
        &mut fails,
    );

    // --- Vollbild bei recv.open (Vollbild-Stream)
    let r = h.cmd(
        json!({"cmd":"recv.open","tid":2,"title":"Bildschirm","app":"Mac","w":1280,"h":720,
        "codec":"h264","fullscreen":true,"keys":keys_json(2)}),
    );
    check(r["ok"] == true, "recv.open fullscreen:true", &mut fails);
    std::thread::sleep(Duration::from_millis(300));
    if let Some(fs) = find("Mac \u{2013} Bildschirm") {
        let mut wr = RECT::default();
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let style = unsafe {
            let _ = GetWindowRect(fs, &mut wr);
            let _ = GetMonitorInfoW(MonitorFromWindow(fs, MONITOR_DEFAULTTONEAREST), &mut mi);
            GetWindowLongPtrW(fs, GWL_STYLE) as u32
        };
        let m = mi.rcMonitor;
        check(
            (wr.left, wr.top, wr.right, wr.bottom) == (m.left, m.top, m.right, m.bottom)
                && style & WS_POPUP.0 != 0
                && style & WS_CAPTION.0 != WS_CAPTION.0,
            &format!("randlos ueber den ganzen Monitor {m:?}"),
            &mut fails,
        );
    } else {
        check(false, "Vollbild-Fenster gefunden", &mut fails);
    }
    let r = h.cmd(json!({"cmd":"recv.close","tid":2}));
    check(r["ok"] == true, "recv.close Vollbild", &mut fails);

    let _ = sender.kill();
    let _ = sender.wait();
    drop(h.stdin.take());
    let _ = h.child.wait();
    if fails.is_empty() {
        println!("\nALLES OK");
    } else {
        println!("\n{} FEHLER: {fails:?}", fails.len());
        std::process::exit(1);
    }
}
