//! Session: ein UDP-Socket, ein Transport-Thread, beliebig viele Streams.

use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::netem::{Netem, NetemConfig, Verdict};
use crate::packet::{decode_header, MAX_PACKET};
use crate::stream::{Counters, Event, Io, Role, RxResult, Stream, TickInfo};

pub struct FrameRef<'a> {
    pub data: &'a [u8],
    pub pts_us: u64,
    pub fflags: u8,
    pub frame_id: u32,
}

pub type FrameFn = Box<dyn Fn(u32, u8, &FrameRef) + Send + Sync>;
pub type ControlFn = Box<dyn Fn(u32, u8) + Send + Sync>;
pub type StatsFn = Box<dyn Fn(u32, &str) + Send + Sync>;

/// Rueckrufe. Sie laufen im Transport-Thread und duerfen nicht blockieren.
pub struct Callbacks {
    pub on_frame: FrameFn,
    pub on_control: ControlFn,
    pub on_stats: StatsFn,
}

impl Callbacks {
    pub fn none() -> Self {
        Callbacks {
            on_frame: Box::new(|_, _, _| {}),
            on_control: Box::new(|_, _| {}),
            on_stats: Box::new(|_, _| {}),
        }
    }
}

#[derive(Clone)]
pub struct StreamKeys {
    pub s2r_key: [u8; 32],
    pub s2r_prefix: [u8; 4],
    pub r2s_key: [u8; 32],
    pub r2s_prefix: [u8; 4],
}

#[derive(Clone, Debug, Default)]
pub struct SessionCounters {
    /// Pakete, die Tag/AAD nicht bestanden haben.
    pub auth_fail: u64,
    /// Kein gueltiger Header (Laenge, Version, Typ).
    pub bad_packets: u64,
    /// Gueltiger Header, aber unbekannte stream_id.
    pub unknown_stream: u64,
    pub send_errors: u64,
    /// Wie oft die Gegenstelle per Roaming auf eine neue Quelladresse umgestellt wurde.
    pub roams: u64,
    /// Tatsaechliche Socket-Puffergroessen (Byte), vom Betriebssystem bestaetigt.
    pub rcvbuf: u32,
    pub sndbuf: u32,
}

struct State {
    streams: HashMap<u32, Stream>,
    peer: Option<SocketAddr>,
    netem: Netem,
    c: SessionCounters,
    /// (Schluessel, Praefix) aller je zum Senden benutzten Richtungen: Wiederverwendung waere Nonce-Reuse.
    used_tx: HashSet<([u8; 32], [u8; 4])>,
}

struct Inner {
    socket: UdpSocket,
    state: Mutex<State>,
    stop: AtomicBool,
    cb: Callbacks,
    local_port: u16,
}

pub struct Session {
    inner: Arc<Inner>,
    thread: Mutex<Option<(JoinHandle<()>, ThreadId)>>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    fn transmit(&self, st: &mut State, out: Vec<Vec<u8>>) {
        let Some(peer) = st.peer else {
            return;
        };
        for d in out {
            if self.socket.send_to(&d, peer).is_err() {
                st.c.send_errors += 1;
            }
        }
    }

    fn dispatch(&self, events: Vec<Event>) {
        for e in events {
            let _ = catch_unwind(AssertUnwindSafe(|| match e {
                Event::Frame {
                    stream,
                    kind,
                    data,
                    pts_us,
                    fflags,
                    frame_id,
                } => (self.cb.on_frame)(
                    stream,
                    kind,
                    &FrameRef {
                        data: &data,
                        pts_us,
                        fflags,
                        frame_id,
                    },
                ),
                Event::Control { stream, ptype } => (self.cb.on_control)(stream, ptype),
                Event::Stats { stream, json } => (self.cb.on_stats)(stream, &json),
            }));
        }
    }

    fn handle(&self, st: &mut State, now: Instant, d: &[u8], src: SocketAddr, io: &mut Io) {
        let Some(h) = decode_header(d) else {
            st.c.bad_packets += 1;
            return;
        };
        match st.netem.ingest(now, d, src, h.seq) {
            Verdict::Now => self.process(st, now, d, src, io),
            Verdict::Held => {}
        }
    }

    fn process(&self, st: &mut State, now: Instant, d: &[u8], src: SocketAddr, io: &mut Io) {
        let Some(h) = decode_header(d) else {
            st.c.bad_packets += 1;
            return;
        };
        let Some(s) = st.streams.get_mut(&h.stream_id) else {
            st.c.unknown_stream += 1;
            return;
        };
        match s.receive(now, d, &h, io) {
            RxResult::Ok => {
                // Roaming (4.1): authentifiziert, kein Replay, hoechste seq dieses Streams.
                let new_max = s.last_new_max;
                if st.peer.is_none() {
                    st.peer = Some(src);
                } else if new_max && st.peer != Some(src) {
                    st.peer = Some(src);
                    st.c.roams += 1;
                    for s in st.streams.values_mut() {
                        if s.role == Role::Receiver {
                            s.ping_now(now, io);
                        }
                    }
                }
            }
            RxResult::AuthFail => st.c.auth_fail += 1,
            RxResult::Replay | RxResult::Malformed => {}
        }
    }

    fn run(self: Arc<Self>) {
        let mut buf = vec![0u8; 2048];
        let mut cur_timeout = Duration::ZERO;
        while !self.stop.load(Ordering::Relaxed) {
            let step = catch_unwind(AssertUnwindSafe(|| {
                let (fast, due) = {
                    let mut st = lock(&self.state);
                    let now = Instant::now();
                    let mut io = Io::default();
                    while let Some((data, src)) = st.netem.pop_due(now) {
                        self.process(&mut st, now, &data, src, &mut io);
                    }
                    let fast =
                        st.netem.active() || st.streams.values().any(|s| s.needs_fast_tick());
                    (fast, io)
                };
                self.finish(due);
                let want = if fast {
                    Duration::from_millis(1)
                } else {
                    Duration::from_millis(10)
                };
                if want != cur_timeout {
                    let _ = self.socket.set_read_timeout(Some(want));
                    cur_timeout = want;
                }
            }));
            if step.is_err() {
                continue;
            }
            // Fehler (Timeout, unter Windows auch ICMP-Reset) heissen einfach: weiter.
            if let Ok((n, src)) = self.socket.recv_from(&mut buf) {
                {
                    if n > MAX_PACKET {
                        lock(&self.state).c.bad_packets += 1;
                    } else {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            let mut st = lock(&self.state);
                            let mut io = Io::default();
                            self.handle(&mut st, Instant::now(), &buf[..n], src, &mut io);
                            self.after(&mut st, io)
                        }))
                        .map(|ev| self.dispatch(ev));
                    }
                }
            }
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let mut st = lock(&self.state);
                let now = Instant::now();
                let mut io = Io::default();
                let info = TickInfo {
                    auth_fail: st.c.auth_fail,
                    peer: st.peer.map(|p| p.to_string()).unwrap_or_default(),
                    rcvbuf: st.c.rcvbuf,
                    sndbuf: st.c.sndbuf,
                };
                for s in st.streams.values_mut() {
                    s.tick(now, &info, &mut io);
                }
                self.after(&mut st, io)
            }))
            .map(|ev| self.dispatch(ev));
        }
    }

    /// Sendet die Datagramme eines `Io` und gibt die Ereignisse zurueck.
    fn after(&self, st: &mut State, io: Io) -> Vec<Event> {
        self.transmit(st, io.out);
        io.events
    }

    fn finish(&self, io: Io) {
        if io.out.is_empty() && io.events.is_empty() {
            return;
        }
        let mut st = lock(&self.state);
        let ev = self.after(&mut st, io);
        drop(st);
        self.dispatch(ev);
    }
}

impl Session {
    /// Bindet einen IPv4-UDP-Socket (`local_port` 0 = beliebig) und startet den Transport-Thread.
    pub fn open(local_port: u16, cb: Callbacks) -> Result<Session, String> {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| format!("socket: {e}"))?;
        let _ = sock.set_recv_buffer_size(4 << 20);
        let _ = sock.set_send_buffer_size(4 << 20);
        // Das Betriebssystem darf kleiner liefern als gewuenscht (macOS: kern.ipc.maxsockbuf); den
        // wirklichen Wert lesen, loggen und in den Statistiken ausgeben.
        let rcvbuf = sock.recv_buffer_size().unwrap_or(0) as u32;
        let sndbuf = sock.send_buffer_size().unwrap_or(0) as u32;
        sock.bind(&SocketAddr::from(([0, 0, 0, 0], local_port)).into())
            .map_err(|e| format!("bind {local_port}: {e}"))?;
        let socket: UdpSocket = sock.into();
        socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .map_err(|e| format!("timeout: {e}"))?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        eprintln!("beamer-transport: UDP-Port {port}, Socket-Puffer empfangen {rcvbuf} B, senden {sndbuf} B");
        crate::timer::begin_fine_timer();
        let inner = Arc::new(Inner {
            socket,
            state: Mutex::new(State {
                streams: HashMap::new(),
                peer: None,
                netem: Netem::default(),
                c: SessionCounters {
                    rcvbuf,
                    sndbuf,
                    ..Default::default()
                },
                used_tx: HashSet::new(),
            }),
            stop: AtomicBool::new(false),
            cb,
            local_port: port,
        });
        let runner = inner.clone();
        let h = thread::Builder::new()
            .name("beamer-transport".into())
            .spawn(move || runner.run())
            .map_err(|e| {
                crate::timer::end_fine_timer();
                format!("thread: {e}")
            })?;
        let id = h.thread().id();
        Ok(Session {
            inner,
            thread: Mutex::new(Some((h, id))),
        })
    }

    pub fn local_port(&self) -> u16 {
        self.inner.local_port
    }

    /// `port` 0 heisst: Gegenstelle noch unbekannt (wird beim ersten gueltigen Paket gelernt).
    pub fn set_peer(&self, host: &str, port: u16) -> Result<(), String> {
        if port == 0 {
            lock(&self.inner.state).peer = None;
            return Ok(());
        }
        let addr = (host, port)
            .to_socket_addrs()
            .map_err(|e| format!("Adresse {host}:{port}: {e}"))?
            .find(|a| a.is_ipv4())
            .ok_or_else(|| format!("keine IPv4-Adresse fuer {host}"))?;
        let mut st = lock(&self.inner.state);
        st.peer = Some(addr);
        // Wirkt sofort: Empfaenger melden sich auf dem neuen Pfad (der Sender folgt mit dem naechsten Paket).
        let now = Instant::now();
        let mut io = Io::default();
        for s in st.streams.values_mut() {
            if s.role == Role::Receiver {
                s.ping_now(now, &mut io);
            }
        }
        self.inner.transmit(&mut st, io.out);
        Ok(())
    }

    /// Aktuelle Gegenstelle (nach Roaming die neue Adresse).
    pub fn peer(&self) -> Option<SocketAddr> {
        lock(&self.inner.state).peer
    }

    /// Neue Obergrenze der Bitrate eines Streams (Pfadwechsel).
    pub fn set_max_kbps(&self, stream_id: u32, max_kbps: u32) -> Result<(), String> {
        if max_kbps == 0 {
            return Err("max_kbps muss > 0 sein".into());
        }
        lock(&self.inner.state)
            .streams
            .get_mut(&stream_id)
            .map(|s| s.set_max_kbps(max_kbps))
            .ok_or_else(|| format!("stream {stream_id} unbekannt"))
    }

    pub fn add_stream(
        &self,
        stream_id: u32,
        role: Role,
        keys: &StreamKeys,
        max_kbps: u32,
    ) -> Result<(), String> {
        if stream_id == 0 {
            return Err("stream_id 0 ist ungueltig".into());
        }
        let mut st = lock(&self.inner.state);
        if st.streams.contains_key(&stream_id) {
            return Err(format!("stream {stream_id} existiert schon"));
        }
        let tx_pair = match role {
            Role::Sender => (keys.s2r_key, keys.s2r_prefix),
            Role::Receiver => (keys.r2s_key, keys.r2s_prefix),
        };
        if !st.used_tx.insert(tx_pair) {
            return Err(
                "Sendeschluessel schon benutzt (Nonce-Wiederverwendung): neue Schluessel ableiten"
                    .into(),
            );
        }
        st.streams.insert(
            stream_id,
            Stream::new(
                stream_id,
                role,
                &keys.s2r_key,
                &keys.s2r_prefix,
                &keys.r2s_key,
                &keys.r2s_prefix,
                max_kbps,
                Instant::now(),
            ),
        );
        Ok(())
    }

    pub fn remove_stream(&self, stream_id: u32) -> Result<(), String> {
        lock(&self.inner.state)
            .streams
            .remove(&stream_id)
            .map(|_| ())
            .ok_or_else(|| format!("stream {stream_id} unbekannt"))
    }

    pub fn send_frame(
        &self,
        stream_id: u32,
        kind: u8,
        data: &[u8],
        pts_us: u64,
        fflags: u8,
    ) -> Result<(), String> {
        let mut st = lock(&self.inner.state);
        if st.peer.is_none() {
            return Err("Gegenstelle nicht gesetzt (bt_set_peer)".into());
        }
        let mut io = Io::default();
        let s = st
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| format!("stream {stream_id} unbekannt"))?;
        let r = s.send_frame(Instant::now(), kind, data, pts_us, fflags, &mut io);
        self.inner.transmit(&mut st, io.out);
        r
    }

    pub fn request_keyframe(&self, stream_id: u32) -> Result<(), String> {
        let mut st = lock(&self.inner.state);
        let mut io = Io::default();
        let s = st
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| format!("stream {stream_id} unbekannt"))?;
        if s.role != Role::Receiver {
            return Err("bt_request_keyframe nur fuer Empfaenger-Streams".into());
        }
        s.request_keyframe(Instant::now(), &mut io);
        self.inner.transmit(&mut st, io.out);
        Ok(())
    }

    /// 0, wenn der Stream unbekannt ist.
    pub fn target_kbps(&self, stream_id: u32) -> u32 {
        lock(&self.inner.state)
            .streams
            .get(&stream_id)
            .map_or(0, |s| s.target_kbps())
    }

    /// XOR-FEC je Stream ein- oder ausschalten (Standard: an). Nur fuer Sender-Streams wirksam.
    pub fn set_fec_enabled(&self, stream_id: u32, on: bool) -> Result<(), String> {
        lock(&self.inner.state)
            .streams
            .get_mut(&stream_id)
            .map(|s| s.fec_enabled = on)
            .ok_or_else(|| format!("stream {stream_id} unbekannt"))
    }

    /// Untergrenze der Frist fuer unvollstaendige Frames (Standard 60 ms nach dem Vertrag v1.2, hoechstens
    /// 250 ms). Nur fuer Tests auf langsamen Rechnern gedacht.
    pub fn set_min_deadline_ms(&self, stream_id: u32, ms: f64) -> Result<(), String> {
        lock(&self.inner.state)
            .streams
            .get_mut(&stream_id)
            .map(|s| {
                s.min_deadline_ms = ms.clamp(
                    crate::stream::DEADLINE_MIN_MS,
                    crate::stream::DEADLINE_MAX_MS,
                )
            })
            .ok_or_else(|| format!("stream {stream_id} unbekannt"))
    }

    /// Kuenstliche Netzstoerung fuer eingehende Pakete (Tests, Beispiele).
    pub fn set_netem(&self, cfg: NetemConfig) {
        lock(&self.inner.state).netem.set(cfg);
    }

    pub fn counters(&self, stream_id: u32) -> Option<Counters> {
        lock(&self.inner.state)
            .streams
            .get(&stream_id)
            .map(|s| s.c.clone())
    }

    pub fn session_counters(&self) -> SessionCounters {
        lock(&self.inner.state).c.clone()
    }

    /// Beendet den Transport-Thread und gibt den Socket frei. Ruft man es aus einem Callback
    /// (also im Transport-Thread) auf, wird nicht auf sich selbst gewartet.
    pub fn close(&self) {
        let was_running = !self.inner.stop.swap(true, Ordering::Relaxed);
        if was_running {
            crate::timer::end_fine_timer();
        }
        let t = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((h, id)) = t {
            if thread::current().id() != id {
                let _ = h.join();
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}
