//! Tests ueber echte UDP-Sockets auf 127.0.0.1.

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beamer_transport::packet::*;
use beamer_transport::{Callbacks, NetemConfig, Role, Session, StreamKeys};

const SID: u32 = 42;

#[derive(Clone, Debug, PartialEq)]
struct Got {
    kind: u8,
    data: Vec<u8>,
    pts: u64,
    fflags: u8,
    frame_id: u32,
}

#[derive(Default)]
struct Sink {
    frames: Mutex<Vec<Got>>,
    controls: Mutex<Vec<(u32, u8)>>,
    stats: Mutex<Vec<String>>,
}

fn callbacks(sink: &Arc<Sink>) -> Callbacks {
    let (a, b, c) = (sink.clone(), sink.clone(), sink.clone());
    Callbacks {
        on_frame: Box::new(move |_sid, kind, f| {
            a.frames.lock().unwrap().push(Got {
                kind,
                data: f.data.to_vec(),
                pts: f.pts_us,
                fflags: f.fflags,
                frame_id: f.frame_id,
            });
        }),
        on_control: Box::new(move |sid, t| b.controls.lock().unwrap().push((sid, t))),
        on_stats: Box::new(move |_sid, j| c.stats.lock().unwrap().push(j.to_string())),
    }
}

fn keys() -> StreamKeys {
    StreamKeys {
        s2r_key: [0x11; 32],
        s2r_prefix: [1, 2, 3, 4],
        r2s_key: [0x22; 32],
        r2s_prefix: [5, 6, 7, 8],
    }
}

struct Link {
    tx: Session,
    rx: Session,
    rx_sink: Arc<Sink>,
    tx_sink: Arc<Sink>,
}

fn link(max_kbps: u32) -> Link {
    let (tx_sink, rx_sink) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
    let tx = Session::open(0, callbacks(&tx_sink)).unwrap();
    let rx = Session::open(0, callbacks(&rx_sink)).unwrap();
    tx.set_peer("127.0.0.1", rx.local_port()).unwrap();
    rx.set_peer("127.0.0.1", tx.local_port()).unwrap();
    tx.add_stream(SID, Role::Sender, &keys(), max_kbps).unwrap();
    rx.add_stream(SID, Role::Receiver, &keys(), max_kbps)
        .unwrap();
    Link {
        tx,
        rx,
        rx_sink,
        tx_sink,
    }
}

fn wait_until(secs: f64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    f()
}

fn blob(n: usize, seed: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + seed * 7) % 251) as u8).collect()
}

#[test]
fn roundtrip_various_sizes() {
    let l = link(8000);
    let sizes = [
        1usize,
        100,
        MAX_FRAG_PAYLOAD,
        MAX_FRAG_PAYLOAD + 1,
        5000,
        30_000,
        250_000,
    ];
    for (i, n) in sizes.iter().enumerate() {
        l.tx.send_frame(
            SID,
            1,
            &blob(*n, i),
            1_000 + i as u64,
            if i == 0 { 1 } else { 0 } | (2 << 1),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let ok = wait_until(3.0, || {
        l.rx_sink.frames.lock().unwrap().len() == sizes.len()
    });
    if !ok {
        eprintln!(
            "{:?} {:?} {:?}",
            l.rx_sink
                .frames
                .lock()
                .unwrap()
                .iter()
                .map(|g| g.data.len())
                .collect::<Vec<_>>(),
            l.rx.counters(SID),
            l.rx.session_counters()
        );
    }
    assert!(ok);
    let fr = l.rx_sink.frames.lock().unwrap();
    for (i, n) in sizes.iter().enumerate() {
        assert_eq!(fr[i].data, blob(*n, i), "Nutzdaten von Frame {i}");
        assert_eq!(fr[i].pts, 1_000 + i as u64);
        assert_eq!(fr[i].fflags, if i == 0 { 1 } else { 0 } | (2 << 1));
        assert_eq!(fr[i].kind, 1);
        assert_eq!(fr[i].frame_id, i as u32 + 1);
    }
}

#[test]
fn audio_roundtrip() {
    let l = link(8000);
    l.tx.send_frame(SID, 2, &blob(120, 3), 5, 3 << 1).unwrap();
    assert!(wait_until(2.0, || !l
        .rx_sink
        .frames
        .lock()
        .unwrap()
        .is_empty()));
    let fr = l.rx_sink.frames.lock().unwrap();
    assert_eq!(fr[0].kind, 2);
    assert_eq!(fr[0].data, blob(120, 3));
}

/// Roher Absender mit dem richtigen Schluessel, um Pakete von Hand zu bauen.
struct Raw {
    sock: UdpSocket,
    cipher: Cipher,
}

fn raw_to(port: u16, key: [u8; 32]) -> Raw {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.connect(("127.0.0.1", port)).unwrap();
    Raw {
        sock,
        cipher: Cipher::new(&key, &[1, 2, 3, 4]),
    }
}

impl Raw {
    fn video(&self, seq: u64, frame_id: u32, payload: &[u8]) -> Vec<u8> {
        let plain = encode_media(
            &MediaHead {
                frame_id,
                frag_idx: 0,
                frag_cnt: 1,
                pts_us: 9,
                fflags: 1,
            },
            payload,
        );
        self.cipher
            .seal(
                &Header {
                    ptype: T_VIDEO,
                    flags: 0,
                    stream_id: SID,
                    seq,
                },
                &plain,
            )
            .unwrap()
    }
}

fn receiver_only() -> (Session, Arc<Sink>) {
    let sink = Arc::new(Sink::default());
    let rx = Session::open(0, callbacks(&sink)).unwrap();
    rx.add_stream(SID, Role::Receiver, &keys(), 8000).unwrap();
    (rx, sink)
}

#[test]
fn tampered_byte_is_dropped_and_does_not_burn_the_seq() {
    let (rx, sink) = receiver_only();
    let raw = raw_to(rx.local_port(), keys().s2r_key);
    let good = raw.video(1, 1, b"hallo welt");
    let mut dropped = 0;
    for i in 0..good.len() {
        let mut bad = good.clone();
        bad[i] ^= 0x01;
        raw.sock.send(&bad).unwrap();
        dropped += 1;
    }
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        sink.frames.lock().unwrap().is_empty(),
        "manipuliertes Paket ausgeliefert"
    );
    let c = rx.session_counters();
    assert!(
        c.auth_fail > 0,
        "Authentifizierungsfehler muessen gezaehlt werden: {c:?}"
    );
    assert!(c.auth_fail + c.bad_packets + c.unknown_stream >= dropped as u64 - 1);
    // Das unveraenderte Original (gleiche seq!) kommt trotzdem durch: Fehlversuche belegen kein seq.
    raw.sock.send(&good).unwrap();
    assert!(wait_until(2.0, || sink.frames.lock().unwrap().len() == 1));
    assert_eq!(sink.frames.lock().unwrap()[0].data, b"hallo welt");
}

#[test]
fn replay_is_dropped() {
    let (rx, sink) = receiver_only();
    let raw = raw_to(rx.local_port(), keys().s2r_key);
    let p1 = raw.video(1, 1, b"eins");
    raw.sock.send(&p1).unwrap();
    raw.sock.send(&p1).unwrap();
    raw.sock.send(&p1).unwrap();
    raw.sock.send(&raw.video(2, 2, b"zwei")).unwrap();
    assert!(wait_until(2.0, || sink.frames.lock().unwrap().len() >= 2));
    std::thread::sleep(Duration::from_millis(100));
    let fr = sink.frames.lock().unwrap();
    assert_eq!(
        fr.len(),
        2,
        "Duplikat darf nicht doppelt ausgeliefert werden"
    );
    assert_eq!(fr[0].data, b"eins");
    assert_eq!(fr[1].data, b"zwei");
    assert!(rx.counters(SID).unwrap().replay_dropped >= 2);
}

#[test]
fn wrong_key_is_dropped() {
    let (rx, sink) = receiver_only();
    let raw = raw_to(rx.local_port(), [0x99; 32]);
    for s in 1..20 {
        raw.sock.send(&raw.video(s, s as u32, b"fremd")).unwrap();
    }
    std::thread::sleep(Duration::from_millis(150));
    assert!(sink.frames.lock().unwrap().is_empty());
    assert_eq!(rx.session_counters().auth_fail, 19);
}

#[test]
fn sender_key_does_not_open_the_return_direction() {
    // Ein Paket in Richtung s2r darf der Empfaenger nicht als r2s-Paket am Sender durchgehen lassen.
    let tx_sink = Arc::new(Sink::default());
    let tx = Session::open(0, callbacks(&tx_sink)).unwrap();
    tx.add_stream(SID, Role::Sender, &keys(), 8000).unwrap();
    let raw = raw_to(tx.local_port(), keys().s2r_key);
    raw.sock.send(&raw.video(1, 1, b"x")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(tx_sink.controls.lock().unwrap().is_empty());
}

/// Roher Empfaenger mit grossem Puffer und eigenem Lese-Thread; sammelt (Laenge, Ankunftszeit).
type Seen = (usize, Instant, Vec<u8>);

struct Collector {
    port: u16,
    got: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    th: Option<std::thread::JoinHandle<()>>,
}

fn collector() -> Collector {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .unwrap();
    let _ = sock.set_recv_buffer_size(8 << 20);
    sock.bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into())
        .unwrap();
    let sock: UdpSocket = sock.into();
    sock.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let port = sock.local_addr().unwrap().port();
    let got = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (g, s) = (got.clone(), stop.clone());
    let th = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while !s.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok(n) = sock.recv(&mut buf) {
                g.lock()
                    .unwrap()
                    .push((n, Instant::now(), buf[..n].to_vec()));
            }
        }
    });
    Collector {
        port,
        got,
        stop,
        th: Some(th),
    }
}

impl Collector {
    /// Wartet, bis 400 ms lang nichts mehr ankommt (hoechstens `max` Sekunden).
    fn settle(&self, max: f64) {
        let end = Instant::now() + Duration::from_secs_f64(max);
        let mut last = (0, Instant::now());
        while Instant::now() < end {
            let n = self.got.lock().unwrap().len();
            if n != last.0 {
                last = (n, Instant::now());
            } else if n > 0 && last.1.elapsed() > Duration::from_millis(400) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn every_datagram_is_at_most_1200_bytes() {
    let c = collector();
    let tx = Session::open(0, Callbacks::none()).unwrap();
    tx.set_peer("127.0.0.1", c.port).unwrap();
    tx.add_stream(SID, Role::Sender, &keys(), 20_000).unwrap();
    for (i, n) in [1usize, 1141, 1142, 2282, 30_000, 400_000]
        .iter()
        .enumerate()
    {
        tx.send_frame(SID, 1, &blob(*n, i), 1, 1).unwrap();
    }
    tx.send_frame(SID, 2, &blob(5000, 1), 1, 3 << 1).unwrap();
    c.settle(15.0);
    let got = c.got.lock().unwrap();
    let max = got.iter().map(|g| g.0).max().unwrap_or(0);
    for (len, _, d) in got.iter() {
        assert!(*len <= MAX_PACKET, "Datagramm mit {len} Byte");
        assert!(decode_header(d).is_some());
    }
    assert!(
        got.len() > 400,
        "es muessen viele Pakete gekommen sein, waren {}",
        got.len()
    );
    assert_eq!(max, MAX_PACKET, "volle Pakete nutzen die 1200 Byte aus");
}

#[test]
fn sender_paces_instead_of_bursting() {
    let c = collector();
    let tx = Session::open(0, Callbacks::none()).unwrap();
    tx.set_peer("127.0.0.1", c.port).unwrap();
    // 2000 kbit/s Ziel -> Bucket fuellt sich mit 1,5 x = 375 kB/s, Burst 64 KB.
    tx.add_stream(SID, Role::Sender, &keys(), 2000).unwrap();
    let start = Instant::now();
    tx.send_frame(SID, 1, &blob(200_000, 1), 1, 1).unwrap();
    c.settle(10.0);
    let got = c.got.lock().unwrap();
    let total: usize = got.iter().map(|g| g.0).sum();
    assert!(total > 200_000);
    // Innerhalb von 50 ms nach dem ersten Paket: Burst (64 KB, genau geprueft in tests/streams.rs) plus
    // Fuellrate (375 kB/s) mit grosszuegiger Reserve fuer den Lese-Thread; ohne Pacing waeren es 235 KB.
    let first = got.first().unwrap().1;
    let early: usize = got
        .iter()
        .filter(|g| g.1.duration_since(first) < Duration::from_millis(50))
        .map(|g| g.0)
        .sum();
    assert!(
        early <= 64 * 1024 + 45_000,
        "Burst {early} B groesser als 64 KB"
    );
    let spread = got.last().unwrap().1.duration_since(got.first().unwrap().1);
    // (total - 64 KB) / 375 kB/s ~ 0,38 s; grosszuegig nach unten.
    assert!(
        spread >= Duration::from_millis(200),
        "Pakete in {spread:?} statt gleichmaessig verteilt"
    );
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn nack_recovers_losses() {
    let l = link(8000);
    l.tx.set_fec_enabled(SID, false).unwrap();
    // Die erste Uebertragung jedes 3. Video-Pakets geht verloren.
    l.rx.set_netem(NetemConfig {
        drop_video_every: 3,
        ..Default::default()
    });
    let n = 8;
    for i in 0..n {
        l.tx.send_frame(SID, 1, &blob(5000, i), i as u64, if i == 0 { 1 } else { 0 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(
        wait_until(8.0, || l.rx_sink.frames.lock().unwrap().len() == n),
        "geliefert: {}",
        l.rx_sink.frames.lock().unwrap().len()
    );
    let fr = l.rx_sink.frames.lock().unwrap();
    for i in 0..n {
        assert_eq!(fr[i].data, blob(5000, i));
    }
    let c = l.tx.counters(SID).unwrap();
    assert!(c.retransmits >= 5, "Wiederholungen: {}", c.retransmits);
    assert!(l.rx.counters(SID).unwrap().nacks_sent > 0);
}

#[test]
fn fec_repairs_single_loss_per_group() {
    let l = link(8000);
    // Jeder Frame hat 5 Fragmente: das 5. Video-Paket jedes Frames faellt aus, also genau eins je Frame.
    l.rx.set_netem(NetemConfig {
        drop_video_every: 5,
        ..Default::default()
    });
    let n = 8usize;
    for i in 0..n {
        l.tx.send_frame(SID, 1, &blob(5000, i), i as u64, if i == 0 { 1 } else { 0 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(wait_until(8.0, || l.rx_sink.frames.lock().unwrap().len() == n));
    let rc = l.rx.counters(SID).unwrap();
    assert!(
        rc.fec_recovered >= 6,
        "FEC sollte fast jeden Verlust reparieren: {rc:?}"
    );
}

#[test]
fn bitrate_drops_at_20_percent_loss() {
    let l = link(8000);
    l.rx.set_netem(NetemConfig {
        loss_pct: 20.0,
        seed: 7,
        ..Default::default()
    });
    assert_eq!(l.tx.target_kbps(SID), 8000);
    let start = Instant::now();
    let mut i = 0u64;
    let mut at3 = None;
    while start.elapsed() < Duration::from_millis(3100) {
        l.tx.send_frame(
            SID,
            1,
            &blob(5000, i as usize),
            i * 16_666,
            if i.is_multiple_of(120) { 1 } else { 0 },
        )
        .unwrap();
        i += 1;
        if at3.is_none() && start.elapsed() >= Duration::from_secs(3) {
            at3 = Some(l.tx.target_kbps(SID));
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let t = at3.unwrap_or_else(|| l.tx.target_kbps(SID));
    eprintln!("target_kbps nach 3 s bei 20 % Verlust: {t} von 8000");
    assert!(t <= 5600, "mindestens 30 % Absenkung erwartet, ist {t}");
}

#[test]
fn bitrate_recovers_without_loss() {
    let l = link(8000);
    l.rx.set_netem(NetemConfig {
        loss_pct: 20.0,
        seed: 3,
        ..Default::default()
    });
    let mut i = 0u64;
    let mut pump = |ms: u64| {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(ms) {
            l.tx.send_frame(SID, 1, &blob(3000, 0), i * 16_666, 0)
                .unwrap();
            i += 1;
            std::thread::sleep(Duration::from_millis(16));
        }
    };
    pump(2500);
    let low = l.tx.target_kbps(SID);
    assert!(low < 6000, "Absenkung erwartet, ist {low}");
    l.rx.set_netem(NetemConfig::default());
    // Erholung: bis 6 s warten statt fester Zeit.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(6) && l.tx.target_kbps(SID) <= low + low / 20 {
        pump(200);
    }
    assert!(
        l.tx.target_kbps(SID) > low,
        "Bitrate muss wieder steigen (war {low})"
    );
}

#[test]
fn moderate_loss_repaired_by_fec_and_nack_does_not_throttle() {
    // 3 % Rohverlust, aber FEC + NACK heilen fast alles: Restverlust < 1 % -> keine Drosselung (4.1 v1.1).
    let l = link(8000);
    l.rx.set_netem(NetemConfig {
        loss_pct: 3.0,
        seed: 11,
        ..Default::default()
    });
    let start = Instant::now();
    let mut i = 0u64;
    while start.elapsed() < Duration::from_millis(5000) {
        l.tx.send_frame(
            SID,
            1,
            &blob(5000, 0),
            i * 16_666,
            if i.is_multiple_of(120) { 1 } else { 0 },
        )
        .unwrap();
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
    let t = l.tx.target_kbps(SID);
    eprintln!("target_kbps nach 5 s bei 3 % Rohverlust: {t} von 8000");
    assert!(
        t >= 4800,
        "keine harte Drosselung bei geheiltem Verlust erwartet, ist {t}"
    );
}

#[test]
fn keyframe_request_reaches_sender() {
    let l = link(8000);
    l.tx.send_frame(SID, 1, &blob(500, 0), 1, 1).unwrap();
    assert!(wait_until(2.0, || !l
        .rx_sink
        .frames
        .lock()
        .unwrap()
        .is_empty()));
    l.rx.request_keyframe(SID).unwrap();
    assert!(wait_until(2.0, || l
        .tx_sink
        .controls
        .lock()
        .unwrap()
        .contains(&(SID, 5))));
}

#[test]
fn stats_and_rtt_are_reported() {
    let l = link(8000);
    let start = Instant::now();
    let mut i = 0u64;
    while start.elapsed() < Duration::from_millis(2300) {
        l.tx.send_frame(SID, 1, &blob(3000, 0), i * 16_666, 0)
            .unwrap();
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
    let st = l.rx_sink.stats.lock().unwrap();
    assert!(st.len() >= 2, "Statistik jede Sekunde");
    let last = st.last().unwrap();
    for k in [
        "\"fps\":",
        "\"kbps\":",
        "\"rtt_ms\":",
        "\"loss_pct\":",
        "\"latency_ms\":",
    ] {
        assert!(last.contains(k), "{k} fehlt in {last}");
    }
    assert!(last.contains("\"peer\":\"127.0.0.1:"), "peer fehlt: {last}");
    let tx_st = l.tx_sink.stats.lock().unwrap();
    assert!(tx_st.len() >= 2);
    let tl = tx_st.last().unwrap();
    let rtt: f64 = tl
        .split("\"rtt_ms\":")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    assert!(rtt > 0.0, "Sender-RTT aus ping/pong fehlt: {tl}");
    assert!(tl.contains("\"sock_rcvbuf\":"));
    let c = l.tx.session_counters();
    assert!(
        c.rcvbuf > 0 && c.sndbuf > 0,
        "Socket-Puffer nicht gelesen: {c:?}"
    );
}

#[test]
fn close_is_fast_and_releases_the_port() {
    for round in 0..25 {
        let l = link(8000);
        let port = l.rx.local_port();
        for i in 0..5 {
            l.tx.send_frame(SID, 1, &blob(5000, i), i as u64, 1)
                .unwrap();
        }
        let t = Instant::now();
        l.tx.close();
        l.rx.close();
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "Runde {round}: close dauerte {:?}",
            t.elapsed()
        );
        drop(l);
        // Der Port ist wieder frei (Socket wirklich geschlossen).
        UdpSocket::bind(("0.0.0.0", port)).expect("Port nach close nicht frei");
    }
}

#[test]
fn errors_for_bad_usage() {
    let s = Session::open(0, Callbacks::none()).unwrap();
    assert!(s.send_frame(1, 1, b"x", 0, 0).is_err(), "ohne Peer/Stream");
    s.set_peer("127.0.0.1", 9).unwrap();
    assert!(
        s.send_frame(1, 1, b"x", 0, 0).is_err(),
        "unbekannter Stream"
    );
    assert!(s.add_stream(0, Role::Sender, &keys(), 1000).is_err());
    s.add_stream(1, Role::Receiver, &keys(), 1000).unwrap();
    assert!(s.add_stream(1, Role::Receiver, &keys(), 1000).is_err());
    assert!(
        s.send_frame(1, 1, b"x", 0, 0).is_err(),
        "Empfaenger darf nicht senden"
    );
    s.add_stream(2, Role::Sender, &keys(), 1000).unwrap();
    assert!(s.send_frame(2, 3, b"x", 0, 0).is_err(), "kind 3 ungueltig");
    assert!(s.send_frame(2, 1, b"", 0, 0).is_err(), "leer");
    assert!(
        s.request_keyframe(2).is_err(),
        "Sender fordert keinen Keyframe an"
    );
    assert!(s.set_peer("127.0.0.1", 0).is_ok());
    assert_eq!(s.target_kbps(99), 0);
    s.remove_stream(1).unwrap();
    assert!(s.remove_stream(1).is_err());
}

#[test]
fn reusing_send_keys_is_refused() {
    let s = Session::open(0, Callbacks::none()).unwrap();
    s.add_stream(1, Role::Sender, &keys(), 1000).unwrap();
    s.remove_stream(1).unwrap();
    let e = s.add_stream(1, Role::Sender, &keys(), 1000).unwrap_err();
    assert!(e.contains("Nonce"), "{e}");
    // Auch unter anderer stream_id: gleiches Schluesselpaar = gleiche Nonces.
    assert!(s.add_stream(2, Role::Sender, &keys(), 1000).is_err());
    // Frische Schluessel gehen.
    let mut k = keys();
    k.s2r_key = [0x77; 32];
    s.add_stream(1, Role::Sender, &k, 1000).unwrap();
}

#[test]
fn max_kbps_up_and_down() {
    let l = link(8000);
    assert_eq!(l.tx.target_kbps(SID), 8000);
    l.tx.set_max_kbps(SID, 2000).unwrap();
    assert_eq!(
        l.tx.target_kbps(SID),
        2000,
        "Senkung: hoechstens die neue Obergrenze"
    );
    l.tx.set_max_kbps(SID, 20_000).unwrap();
    assert_eq!(
        l.tx.target_kbps(SID),
        10_000,
        "Erhoehung: 50 % der neuen Obergrenze"
    );
    l.tx.set_max_kbps(SID, 40_000).unwrap();
    assert_eq!(l.tx.target_kbps(SID), 20_000);
    l.tx.set_max_kbps(SID, 5000).unwrap();
    assert_eq!(l.tx.target_kbps(SID), 5000);
    assert!(l.tx.set_max_kbps(SID, 0).is_err());
    assert!(l.tx.set_max_kbps(999, 1000).is_err());
}

#[test]
fn set_peer_while_streaming_moves_the_stream() {
    let (s1, s2, s3) = (
        Arc::new(Sink::default()),
        Arc::new(Sink::default()),
        Arc::new(Sink::default()),
    );
    let tx = Session::open(0, callbacks(&s1)).unwrap();
    let r1 = Session::open(0, callbacks(&s2)).unwrap();
    let r2 = Session::open(0, callbacks(&s3)).unwrap();
    for r in [&r1, &r2] {
        r.add_stream(SID, Role::Receiver, &keys(), 8000).unwrap();
    }
    tx.add_stream(SID, Role::Sender, &keys(), 8000).unwrap();
    tx.set_peer("127.0.0.1", r1.local_port()).unwrap();
    for i in 0..5 {
        tx.send_frame(SID, 1, &blob(3000, i), i as u64, (i == 0) as u8)
            .unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(wait_until(3.0, || s2.frames.lock().unwrap().len() == 5));
    // Umstellen mitten im Strom: seq, Schluessel und Puffer laufen weiter.
    tx.set_peer("127.0.0.1", r2.local_port()).unwrap();
    for i in 5..10 {
        tx.send_frame(SID, 1, &blob(3000, i), i as u64, (i == 5) as u8)
            .unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(
        wait_until(3.0, || s3.frames.lock().unwrap().len() == 5),
        "r2 hat {}",
        s3.frames.lock().unwrap().len()
    );
    assert_eq!(s2.frames.lock().unwrap().len(), 5, "r1 bekommt nichts mehr");
    // Der neue Empfaenger meldet sich (ping/feedback) beim Sender zurueck.
    assert!(wait_until(3.0, || tx.counters(SID).unwrap().packets_recv > 0));
}

#[test]
fn foreign_address_cannot_steal_the_peer() {
    let (rx, sink) = receiver_only();
    let a = raw_to(rx.local_port(), keys().s2r_key);
    let b = raw_to(rx.local_port(), keys().s2r_key);
    let a_addr = a.sock.local_addr().unwrap();
    let b_addr = b.sock.local_addr().unwrap();
    let p100 = a.video(100, 1, b"von A");
    a.sock.send(&p100).unwrap();
    assert!(wait_until(2.0, || sink.frames.lock().unwrap().len() == 1));
    assert_eq!(rx.peer(), Some(a_addr));

    // 1. manipuliert (gueltiger Aufbau, hohe seq, ein Bit gekippt)
    let mut bad = b.video(200, 2, b"manipuliert");
    let last = bad.len() - 1;
    bad[last] ^= 1;
    b.sock.send(&bad).unwrap();
    // 2. wiederholt (bereits gesehenes Paket 100, von B erneut gesendet)
    b.sock.send(&p100).unwrap();
    // 3. veraltet (gueltig und unbekannt, aber seq 50 < 100)
    b.sock.send(&b.video(50, 2, b"alt")).unwrap();
    assert!(
        wait_until(2.0, || sink.frames.lock().unwrap().len() == 2),
        "das alte, aber gueltige Paket wird ausgeliefert"
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        rx.peer(),
        Some(a_addr),
        "Ziel darf sich nicht geaendert haben"
    );
    assert_eq!(rx.session_counters().roams, 0);
    assert!(rx.session_counters().auth_fail >= 1);

    // Gegenprobe: gueltig, neu, hoechste seq -> Roaming.
    b.sock.send(&b.video(101, 3, b"von B")).unwrap();
    assert!(wait_until(2.0, || rx.peer() == Some(b_addr)));
    assert_eq!(rx.session_counters().roams, 1);
}

/// Mini-Relais, das die Quelladresse des Senders gegenueber dem Empfaenger umschaltet (Pfadwechsel).
struct Relay {
    front_port: u16,
    out_addrs: [std::net::SocketAddr; 2],
    active: Arc<std::sync::atomic::AtomicUsize>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    th: Option<std::thread::JoinHandle<()>>,
}

fn relay(target: std::net::SocketAddr) -> Relay {
    use std::sync::atomic::Ordering::Relaxed;
    let front = UdpSocket::bind("127.0.0.1:0").unwrap();
    let out0 = UdpSocket::bind("127.0.0.1:0").unwrap();
    // 127.0.0.2 gibt es unter Linux/Windows; macOS kennt es ohne Alias nicht -> anderer Port reicht.
    let out1 = UdpSocket::bind("127.0.0.2:0")
        .or_else(|_| UdpSocket::bind("127.0.0.1:0"))
        .unwrap();
    let out_addrs = [out0.local_addr().unwrap(), out1.local_addr().unwrap()];
    let front_port = front.local_addr().unwrap().port();
    for s in [&front, &out0, &out1] {
        s.set_nonblocking(true).unwrap();
    }
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (a, st) = (active.clone(), stop.clone());
    let th = std::thread::spawn(move || {
        let outs = [out0, out1];
        let mut sender = None;
        let mut buf = [0u8; 4096];
        while !st.load(Relaxed) {
            let mut idle = true;
            while let Ok((n, from)) = front.recv_from(&mut buf) {
                sender = Some(from);
                let _ = outs[a.load(Relaxed)].send_to(&buf[..n], target);
                idle = false;
            }
            for o in &outs {
                while let Ok((n, _)) = o.recv_from(&mut buf) {
                    if let Some(s) = sender {
                        let _ = front.send_to(&buf[..n], s);
                    }
                    idle = false;
                }
            }
            if idle {
                std::thread::sleep(Duration::from_micros(200));
            }
        }
    });
    Relay {
        front_port,
        out_addrs,
        active,
        stop,
        th: Some(th),
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn roaming_follows_a_changed_source_address() {
    let (tx_sink, rx_sink) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
    let tx = Session::open(0, callbacks(&tx_sink)).unwrap();
    let rx = Session::open(0, callbacks(&rx_sink)).unwrap();
    let rel = relay(format!("127.0.0.1:{}", rx.local_port()).parse().unwrap());
    tx.set_peer("127.0.0.1", rel.front_port).unwrap();
    rx.set_peer("127.0.0.1", rel.out_addrs[0].port()).unwrap();
    tx.add_stream(SID, Role::Sender, &keys(), 8000).unwrap();
    rx.add_stream(SID, Role::Receiver, &keys(), 8000).unwrap();

    let key_every = 30u64; // Keyframe-Intervall = 0,5 s
    let i = std::cell::Cell::new(0u64);
    let seen_req = std::cell::Cell::new(0usize);
    let send_for = |ms: u64| {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(ms) {
            let reqs = tx_sink.controls.lock().unwrap().len();
            let key = i.get().is_multiple_of(key_every) || reqs > seen_req.get();
            seen_req.set(reqs);
            tx.send_frame(
                SID,
                1,
                &blob(4000, (i.get() % 7) as usize),
                i.get() * 16_666,
                key as u8,
            )
            .unwrap();
            i.set(i.get() + 1);
            std::thread::sleep(Duration::from_millis(16));
        }
    };
    send_for(1500);
    wait_until(1.0, || {
        rx_sink.frames.lock().unwrap().len() as u64 + 1 >= i.get()
    });
    let before = rx_sink.frames.lock().unwrap().len() as u64;
    assert!(
        before + 2 >= i.get(),
        "vor dem Wechsel kommt alles an: {before} von {}",
        i.get()
    );
    assert_eq!(rx.peer().unwrap().port(), rel.out_addrs[0].port());
    let sent_before_switch = i.get();
    let tx_recv_before = tx.counters(SID).unwrap().packets_recv;

    // Pfadwechsel: ab jetzt kommen die Pakete von der zweiten Adresse.
    rel.active.store(1, std::sync::atomic::Ordering::Relaxed);
    send_for(2500);
    wait_until(1.0, || {
        rx_sink.frames.lock().unwrap().len() as u64 + 1 >= i.get()
    });
    let i = i.get();
    let delivered = rx_sink.frames.lock().unwrap().len() as u64;
    let lost = i - delivered;
    eprintln!("Roaming: {i} gesendet, {delivered} geliefert, {lost} verloren (Keyframe-Intervall {key_every})");
    assert!(
        lost <= key_every,
        "hoechstens ein Keyframe-Intervall darf verloren gehen, waren {lost}"
    );
    // Danach 100 %: alle Frames der letzten Sekunde sind da.
    let tail_start = sent_before_switch + 100;
    let tail = rx_sink
        .frames
        .lock()
        .unwrap()
        .iter()
        .filter(|g| g.pts / 16_666 >= tail_start)
        .count() as u64;
    assert!(
        tail + 2 >= i - tail_start,
        "nach dem Wechsel 100 %: {tail} von {}",
        i - tail_start
    );
    assert_eq!(rx.peer().unwrap(), rel.out_addrs[1]);
    assert_eq!(rx.session_counters().roams, 1);
    assert!(
        tx.counters(SID).unwrap().packets_recv > tx_recv_before + 3,
        "Rueckkanal laeuft ueber den neuen Pfad"
    );
}
