//! Hauptthread: Nachrichtenschleife, IPC-Befehle, Proxy-Fenster, Transport-Session.
//!
//! stdin liest ein eigener Thread und weckt den Hauptthread per `PostThreadMessage`. Alle Fenster und
//! die Session-Verwaltung bleiben so auf einem Thread; Transport-Callbacks reichen Frames nur an die
//! Empfaenger-Threads weiter und schreiben `stats` direkt.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use beamer_transport::{Callbacks, NetemConfig, Role, Session};
use serde_json::{json, Value};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW, TranslateMessage, MSG,
    PM_NOREMOVE, WM_USER,
};

use crate::ipc::{self, Cmd, Out, RecvOpen};
use crate::receiver::{self, FrameIn, SessionSlot};
use crate::stats;
use crate::window::{self, ProxyWindow, WM_APP_CLOSED, WM_APP_CMD, WM_APP_EOF};

/// Obergrenze fuer Empfaenger-Streams: fuer den Empfaenger ohne Wirkung (Bitrate regelt der Sender).
const RECV_MAX_KBPS: u32 = 150_000;

struct Route {
    handle: receiver::Handle,
}

type Routes = Arc<Mutex<HashMap<u32, Route>>>;

struct Recv {
    open: RecvOpen,
    win: ProxyWindow,
}

pub struct Options {
    pub same_clock: bool,
    /// Nur Tests: Jitter-Puffer aus (jedes Bild zeigen, blockierend warten).
    pub present_all: bool,
    /// Nur fuer Tests: kuenstliche Netzstoerung auf der Empfangsseite des Transports.
    pub test_netem: Option<NetemConfig>,
}

/// `--test-netem "loss=2,delay=5,jitter=3,stall=40,stall_every=1500,reorder=1,seed=7"` (alle optional).
pub fn parse_netem(spec: &str) -> Result<NetemConfig, String> {
    let mut c = NetemConfig {
        seed: 7,
        ..Default::default()
    };
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| format!("netem: {part} ist nicht schluessel=wert"))?;
        let x: f64 = v
            .parse()
            .map_err(|_| format!("netem: {v} ist keine Zahl"))?;
        if !x.is_finite() || x < 0.0 {
            return Err(format!("netem: {k} muss >= 0 sein"));
        }
        match k {
            "loss" => c.loss_pct = x,
            "delay" => c.delay_ms = x,
            "jitter" => c.jitter_ms = x,
            "reorder" => c.reorder_pct = x,
            "stall" => c.stall_ms = x,
            "stall_every" => c.stall_every_ms = x,
            "seed" => c.seed = x as u64,
            _ => return Err(format!("netem: unbekannt {k}")),
        }
    }
    Ok(c)
}

pub struct App {
    out: Out,
    opts: Options,
    main_thread: u32,
    session: Option<Arc<Session>>,
    slot: SessionSlot,
    routes: Routes,
    recvs: HashMap<u32, Recv>,
    /// Vom Nutzer geschlossene Proxies: ein spaeteres `recv.close` ist dann kein Fehler.
    user_closed: HashSet<u32>,
    opened: usize,
    queue: Arc<Mutex<VecDeque<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl App {
    pub fn new(out: Out, opts: Options) -> App {
        App {
            out,
            opts,
            main_thread: unsafe { GetCurrentThreadId() },
            session: None,
            slot: Arc::new(Mutex::new(None)),
            routes: Arc::new(Mutex::new(HashMap::new())),
            recvs: HashMap::new(),
            user_closed: HashSet::new(),
            opened: 0,
            queue: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Nachrichtenschleife bis `shutdown` oder stdin-EOF. Rueckgabe: Exit-Code.
    pub fn run(mut self) -> i32 {
        unsafe {
            // Nachrichtenwarteschlange des Threads anlegen, bevor der stdin-Thread hineinschreibt.
            let mut m = MSG::default();
            let _ = PeekMessageW(&mut m, None, WM_USER, WM_USER, PM_NOREMOVE);
        }
        let q = self.queue.clone();
        let tid = self.main_thread;
        std::thread::Builder::new()
            .name("stdin".into())
            .spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    lock(&q).push_back(line);
                    unsafe {
                        let _ = PostThreadMessageW(tid, WM_APP_CMD, WPARAM(0), LPARAM(0));
                    }
                }
                unsafe {
                    let _ = PostThreadMessageW(tid, WM_APP_EOF, WPARAM(0), LPARAM(0));
                }
            })
            .expect("stdin-Thread");

        let mut msg = MSG::default();
        loop {
            let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if r.0 <= 0 {
                break;
            }
            if msg.hwnd.is_invalid() {
                match msg.message {
                    WM_APP_CMD => {
                        if self.drain_commands() {
                            break;
                        }
                        continue;
                    }
                    WM_APP_EOF => {
                        // Erst noch Wartendes abarbeiten, dann aufraeumen (Vertrag: binnen 2 s, Code 0).
                        if !self.drain_commands() {
                            eprintln!("beamer-teleport: stdin geschlossen, raeume auf");
                        }
                        break;
                    }
                    WM_APP_CLOSED => {
                        self.user_close(msg.wParam.0 as u32);
                        continue;
                    }
                    _ => {}
                }
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let t = Instant::now();
        self.cleanup();
        eprintln!(
            "beamer-teleport: beendet (Aufraeumen {:.0} ms)",
            t.elapsed().as_secs_f64() * 1000.0
        );
        0
    }

    /// true = beenden (shutdown).
    fn drain_commands(&mut self) -> bool {
        loop {
            let Some(line) = lock(&self.queue).pop_front() else {
                return false;
            };
            if self.handle_line(&line) {
                return true;
            }
        }
    }

    fn handle_line(&mut self, line: &str) -> bool {
        let req = match ipc::parse_line(line) {
            Ok(r) => r,
            Err(e) => {
                match e.id {
                    Some(id) => self.out.send(&ipc::err(id, e.code, &e.msg)),
                    None => self
                        .out
                        .send(&ipc::event("error", json!({"code": e.code, "msg": e.msg}))),
                }
                return false;
            }
        };
        let id = req.id;
        let reply = match req.cmd {
            Cmd::Hello => Ok(ipc::hello_reply(id)),
            Cmd::Shutdown => {
                self.out.send(&ipc::ok(id, json!({})));
                return true;
            }
            Cmd::Unsupported(name) => Err((
                "unsupported",
                format!("{name} kann dieser Helfer (noch) nicht"),
            )),
            Cmd::TransportOpen {
                local_port,
                peer_host,
                peer_port,
            } => self.transport_open(local_port, peer_host, peer_port),
            Cmd::TransportPeer { host, port } => self.transport_peer(&host, port),
            Cmd::TransportClose => {
                self.transport_close();
                Ok(json!({}))
            }
            Cmd::RecvOpen(o) => self.recv_open(*o),
            Cmd::RecvUpdate(u) => match self.recvs.get_mut(&u.tid) {
                None => Err(("not_found", format!("tid {} unbekannt", u.tid))),
                Some(r) => {
                    if let Some(t) = u.title {
                        r.open.title = t;
                        r.win
                            .set_title(&ipc::window_title(&r.open.app, &r.open.title));
                    }
                    if u.w.is_some() || u.h.is_some() {
                        r.open.w = u.w.unwrap_or(r.open.w);
                        r.open.h = u.h.unwrap_or(r.open.h);
                        r.win.resize_points(r.open.w, r.open.h);
                    }
                    Ok(json!({}))
                }
            },
            Cmd::RecvClose { tid } => {
                if self.teardown(tid) || self.user_closed.remove(&tid) {
                    Ok(json!({}))
                } else {
                    Err(("not_found", format!("tid {tid} unbekannt")))
                }
            }
        };
        let v = match reply {
            Ok(v) if v.get("re").is_some() => v,
            Ok(v) => ipc::ok(id, v),
            Err((code, msg)) => ipc::err(id, code, &msg),
        };
        self.out.send(&v);
        false
    }

    fn transport_open(
        &mut self,
        local_port: u16,
        peer_host: Option<String>,
        peer_port: u16,
    ) -> Result<Value, (&'static str, String)> {
        if self.session.is_some() {
            return Err(("busy", "Transport ist schon offen".into()));
        }
        let routes = self.routes.clone();
        let routes2 = self.routes.clone();
        let out = self.out.clone();
        let cb = Callbacks {
            on_frame: Box::new(move |sid, kind, f| {
                if kind != 1 {
                    return; // Audio kommt mit C09
                }
                let t_recv = Instant::now();
                if let Some(r) = lock(&routes).get(&sid) {
                    r.handle.push(FrameIn {
                        data: f.data.to_vec(),
                        pts_us: f.pts_us,
                        fflags: f.fflags,
                        frame_id: f.frame_id,
                        t_recv,
                    });
                }
            }),
            on_control: Box::new(|_, _| {}),
            on_stats: Box::new(move |sid, j| {
                let (sum, dec) = match lock(&routes2).get(&sid) {
                    Some(r) => (
                        lock(&r.handle.shared.stats).take(),
                        lock(&r.handle.shared.decoder).clone(),
                    ),
                    None => return,
                };
                out.send(&stats::stats_event(sid, j, &sum, &dec));
            }),
        };
        let s = Arc::new(Session::open(local_port, cb).map_err(|e| ("internal", e))?);
        if let (Some(h), p) = (peer_host.as_deref(), peer_port) {
            if p != 0 {
                if let Err(e) = s.set_peer(h, p) {
                    s.close();
                    return Err(("bad_request", e));
                }
            }
        }
        for (tid, r) in &self.recvs {
            if let Err(e) = s.add_stream(*tid, Role::Receiver, &r.open.keys, RECV_MAX_KBPS) {
                eprintln!("beamer-teleport: tid {tid} add_stream: {e}");
            }
        }
        if let Some(n) = &self.opts.test_netem {
            eprintln!("beamer-teleport: TEST: kuenstliche Netzstoerung {n:?}");
            s.set_netem(n.clone());
        }
        let port = s.local_port();
        *lock(&self.slot) = Some(s.clone());
        self.session = Some(s);
        crate::firewall::check_in_background(port);
        Ok(json!({"local_port": port}))
    }

    fn transport_peer(&mut self, host: &str, port: u16) -> Result<Value, (&'static str, String)> {
        let Some(s) = &self.session else {
            return Err(("not_found", "Transport ist nicht offen".into()));
        };
        // §7: darf bei laufenden Streams kommen; seq, Schluessel und Puffer laufen weiter.
        s.set_peer(host, port).map_err(|e| ("bad_request", e))?;
        eprintln!("beamer-teleport: Gegenstelle jetzt {host}:{port}");
        Ok(json!({}))
    }

    fn transport_close(&mut self) {
        *lock(&self.slot) = None;
        if let Some(s) = self.session.take() {
            s.close();
        }
    }

    fn recv_open(&mut self, o: RecvOpen) -> Result<Value, (&'static str, String)> {
        let tid = o.tid;
        if self.recvs.contains_key(&tid) {
            return Err(("busy", format!("tid {tid} ist schon offen")));
        }
        if o.fullscreen {
            eprintln!("beamer-teleport: tid {tid} fullscreen kommt mit C05, zeige als Fenster");
        }
        self.user_closed.remove(&tid);
        let title = ipc::window_title(&o.app, &o.title);
        let win = window::create(tid, &title, o.w, o.h, self.main_thread, self.opened)
            .map_err(|e| ("internal", format!("Fenster: {e}")))?;
        let handle = match receiver::spawn(receiver::Config {
            tid,
            codec: o.codec,
            hwnd: win.hwnd.0 as isize,
            win: win.shared.clone(),
            session: self.slot.clone(),
            same_clock: self.opts.same_clock,
            present_all: self.opts.present_all,
        }) {
            Ok(h) => h,
            Err(e) => {
                win.destroy();
                return Err(("internal", e));
            }
        };
        if let Some(s) = &self.session {
            if let Err(e) = s.add_stream(tid, Role::Receiver, &o.keys, RECV_MAX_KBPS) {
                drop(handle);
                win.destroy();
                return Err(("bad_request", e));
            }
        }
        lock(&self.routes).insert(tid, Route { handle });
        self.opened += 1;
        self.recvs.insert(tid, Recv { open: o, win });
        Ok(json!({}))
    }

    /// Empfaenger beenden und Fenster schliessen. false, wenn `tid` nicht offen war.
    fn teardown(&mut self, tid: u32) -> bool {
        let Some(r) = self.recvs.remove(&tid) else {
            return false;
        };
        if let Some(s) = &self.session {
            let _ = s.remove_stream(tid);
        }
        // Route entfernen (Transport liefert nichts mehr), dann Thread stoppen, dann Fenster zerstoeren.
        let route = lock(&self.routes).remove(&tid);
        drop(route);
        r.win.destroy();
        true
    }

    fn user_close(&mut self, tid: u32) {
        if self.teardown(tid) {
            self.user_closed.insert(tid);
            self.out
                .send(&ipc::event("proxy.closed", json!({"tid": tid})));
        }
    }

    fn cleanup(&mut self) {
        let tids: Vec<u32> = self.recvs.keys().copied().collect();
        for t in tids {
            self.teardown(t);
        }
        self.transport_close();
    }
}

#[cfg(test)]
mod tests {
    use super::parse_netem;

    #[test]
    fn netem_spec() {
        let c = parse_netem("loss=2.5, delay=5,stall=40,stall_every=1500").unwrap();
        assert_eq!(
            (c.loss_pct, c.delay_ms, c.stall_ms, c.stall_every_ms),
            (2.5, 5.0, 40.0, 1500.0)
        );
        assert_eq!(c.seed, 7);
        assert!(parse_netem("loss").is_err());
        assert!(parse_netem("loss=-1").is_err());
        assert!(parse_netem("foo=1").is_err());
        assert!(parse_netem("").unwrap().is_off());
    }
}
