//! Empfaenger-Thread je `tid`: Frames aus dem Transport dekodieren (D3D11VA) und im Proxy-Fenster
//! praesentieren. Bei Decode-Fehlern oder Luecken bleibt das letzte gute Bild stehen, ein Keyframe wird
//! angefordert und erst ab ihm geht es weiter.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use beamer_transport::Session;
use windows::Win32::Foundation::HWND;

use crate::clock;
use crate::decode::{Decoded, Decoder};
use crate::gpu::{Gpu, Presenter};
use crate::ipc::Codec;
use crate::stats;
use crate::window::Shared as WinShared;

/// Ab so vielen wartenden Frames holt der Empfaenger auf: er springt zum neuesten Keyframe in der
/// Warteschlange bzw. verwirft alles und fordert einen an (Latenz vor Vollstaendigkeit).
pub const MAX_BACKLOG: usize = 6;

pub struct FrameIn {
    pub data: Vec<u8>,
    pub pts_us: u64,
    pub fflags: u8,
    pub frame_id: u32,
    /// Zeitpunkt des Transport-Callbacks = Empfang des letzten Fragments.
    pub t_recv: Instant,
}

impl FrameIn {
    pub fn is_key(&self) -> bool {
        self.fflags & 1 == 1
    }
}

pub enum Msg {
    Frame(FrameIn),
    Stop,
}

/// Session, die gerade offen ist (fuer `request_keyframe` aus dem Empfaenger-Thread).
pub type SessionSlot = Arc<Mutex<Option<Arc<Session>>>>;

/// Was Transport-Callbacks und Hauptthread vom Empfaenger brauchen.
pub struct Shared {
    pub stats: Mutex<stats::Window>,
    pub decoder: Mutex<String>,
    pub backlog: AtomicUsize,
}

pub struct Handle {
    pub tx: Sender<Msg>,
    pub shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// Frame aus dem Transport-Thread einreihen (blockiert nie).
    pub fn push(&self, f: FrameIn) {
        self.shared.backlog.fetch_add(1, Ordering::AcqRel);
        if self.tx.send(Msg::Frame(f)).is_err() {
            self.shared.backlog.fetch_sub(1, Ordering::AcqRel);
        }
    }

    pub fn stop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct Config {
    pub tid: u32,
    pub codec: Codec,
    /// Fensterhandle als Zahl (HWND ist nicht Send; benutzt wird es nur ueber DXGI).
    pub hwnd: isize,
    pub win: Arc<WinShared>,
    pub session: SessionSlot,
    /// Sender und Empfaenger teilen die Uhr (Selbsttest auf einem Rechner): Alter des Bildes messen.
    pub same_clock: bool,
}

/// Thread starten. Kehrt zurueck, sobald GPU, Swapchain und Decoder stehen (oder mit dem Fehler).
pub fn spawn(cfg: Config) -> Result<Handle, String> {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        stats: Mutex::new(stats::Window::default()),
        decoder: Mutex::new(String::new()),
        backlog: AtomicUsize::new(0),
    });
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let sh = shared.clone();
    let tid = cfg.tid;
    let thread = std::thread::Builder::new()
        .name(format!("recv-{tid}"))
        .spawn(move || {
            let hwnd = HWND(cfg.hwnd as *mut _);
            run(cfg, hwnd, rx, sh, ready_tx);
        })
        .map_err(|e| format!("thread: {e}"))?;
    let mut h = Handle {
        tx,
        shared,
        thread: Some(thread),
    };
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(h),
        Ok(Err(e)) => {
            h.stop();
            Err(e)
        }
        Err(_) => {
            h.stop();
            Err("Empfaenger-Thread startet nicht".into())
        }
    }
}

struct State {
    gpu: Gpu,
    presenter: Presenter,
    decoder: Decoder,
    last: Option<Decoded>,
    /// Nach Fehler oder Luecke: nichts anzeigen bis zum naechsten Keyframe.
    waiting_key: bool,
    last_frame_id: Option<u32>,
}

fn request_key(slot: &SessionSlot, tid: u32) {
    let s = slot.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(s) = s {
        let _ = s.request_keyframe(tid);
    }
}

fn run(
    cfg: Config,
    hwnd: HWND,
    rx: Receiver<Msg>,
    shared: Arc<Shared>,
    ready: Sender<Result<(), String>>,
) {
    let init = || -> Result<State, String> {
        let gpu = Gpu::new().map_err(|e| format!("D3D11: {e}"))?;
        let (w, h) = cfg.win.size();
        let presenter = Presenter::new(&gpu, hwnd, w, h).map_err(|e| format!("Swapchain: {e}"))?;
        let decoder = Decoder::new(&gpu, cfg.codec)?;
        eprintln!(
            "beamer-teleport: tid {} empfaengt {} auf \"{}\" (D3D11VA){}",
            cfg.tid,
            cfg.codec.name(),
            gpu.adapter_name,
            if gpu.tearing { ", Tearing erlaubt" } else { "" }
        );
        Ok(State {
            gpu,
            presenter,
            decoder,
            last: None,
            waiting_key: true,
            last_frame_id: None,
        })
    };
    let mut st = match init() {
        Ok(s) => {
            *shared.decoder.lock().unwrap() = format!("d3d11va:{}", cfg.codec.name());
            let _ = ready.send(Ok(()));
            s
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut pending: Vec<FrameIn> = Vec::new();
    loop {
        let msg = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match msg {
            Some(Msg::Stop) => break,
            Some(Msg::Frame(f)) => {
                shared.backlog.fetch_sub(1, Ordering::AcqRel);
                pending.push(f);
                // Alles schon Wartende mitnehmen, um ggf. aufzuholen.
                while let Ok(m) = rx.try_recv() {
                    match m {
                        Msg::Frame(f) => {
                            shared.backlog.fetch_sub(1, Ordering::AcqRel);
                            pending.push(f);
                        }
                        Msg::Stop => return,
                    }
                }
                if pending.len() > MAX_BACKLOG {
                    catch_up(&mut pending, &mut st, &cfg, &shared);
                }
                for f in pending.drain(..) {
                    handle_frame(f, &mut st, &cfg, &shared);
                }
            }
            None => {}
        }
        if cfg.win.dirty.swap(false, Ordering::AcqRel) {
            let (w, h) = cfg.win.size();
            if let Err(e) = st.presenter.resize(w, h) {
                eprintln!("beamer-teleport: tid {} ResizeBuffers: {e}", cfg.tid);
            }
            // Letztes gutes Bild in neuer Groesse zeigen.
            if let Some(d) = st.last.as_ref() {
                let _ = st
                    .presenter
                    .present(&st.gpu, d.texture(), d.slice(), d.size());
            }
        }
    }
}

/// Rueckstau abbauen: ab dem neuesten Keyframe weitermachen, sonst alles verwerfen und einen anfordern.
fn catch_up(pending: &mut Vec<FrameIn>, st: &mut State, cfg: &Config, shared: &Shared) {
    let n = pending.len();
    let mut w = shared.stats.lock().unwrap();
    match pending.iter().rposition(FrameIn::is_key) {
        Some(k) => {
            w.skipped += k as u64;
            pending.drain(..k);
        }
        None => {
            w.skipped += n as u64;
            pending.clear();
            st.waiting_key = true;
            st.decoder.flush();
            drop(w);
            request_key(&cfg.session, cfg.tid);
        }
    }
    eprintln!(
        "beamer-teleport: tid {} Rueckstau von {n} Frames abgebaut",
        cfg.tid
    );
}

fn handle_frame(f: FrameIn, st: &mut State, cfg: &Config, shared: &Shared) {
    let key = f.is_key();
    // Luecke in den frame_ids ohne Keyframe: Referenzen fehlen, Bild waere kaputt.
    if let Some(prev) = st.last_frame_id {
        if f.frame_id != prev.wrapping_add(1) && !key && !st.waiting_key {
            st.waiting_key = true;
            st.decoder.flush();
            request_key(&cfg.session, cfg.tid);
        }
    }
    st.last_frame_id = Some(f.frame_id);
    if st.waiting_key && !key {
        shared.stats.lock().unwrap().skipped += 1;
        return;
    }
    if let Some(c) = Codec::from_fflags(f.fflags) {
        if c != st.decoder.codec {
            eprintln!(
                "beamer-teleport: tid {} Codec wechselt auf {}",
                cfg.tid,
                c.name()
            );
            match Decoder::new(&st.gpu, c) {
                Ok(d) => st.decoder = d,
                Err(e) => {
                    eprintln!("beamer-teleport: tid {} {e}", cfg.tid);
                    return;
                }
            }
            *shared.decoder.lock().unwrap() = format!("d3d11va:{}", c.name());
        }
    }
    let t0 = Instant::now();
    let r = st.decoder.decode(&f.data, key);
    let t1 = Instant::now();
    let d = match r {
        Ok(Some(d)) => d,
        Ok(None) => return,
        Err(e) => {
            eprintln!(
                "beamer-teleport: tid {} Frame {}: {e} - Keyframe angefordert",
                cfg.tid, f.frame_id
            );
            shared.stats.lock().unwrap().decode_errors += 1;
            st.waiting_key = true;
            st.decoder.flush();
            request_key(&cfg.session, cfg.tid);
            return;
        }
    };
    st.waiting_key = false;
    if let Err(e) = st
        .presenter
        .present(&st.gpu, d.texture(), d.slice(), d.size())
    {
        eprintln!("beamer-teleport: tid {} Present: {e}", cfg.tid);
        st.last = Some(d);
        return;
    }
    let t2 = Instant::now();
    let mut w = shared.stats.lock().unwrap();
    w.decode_ms.push((t1 - t0).as_secs_f64() * 1000.0);
    w.present_ms.push((t2 - t1).as_secs_f64() * 1000.0);
    w.recv_to_present_ms
        .push((t2 - f.t_recv).as_secs_f64() * 1000.0);
    if cfg.same_clock {
        let now = clock::now_us();
        if now >= f.pts_us {
            w.pts_age_ms.push((now - f.pts_us) as f64 / 1000.0);
        }
    }
    w.presented += 1;
    drop(w);
    st.last = Some(d);
}
