//! Tests ueber echte UDP-Sockets auf 127.0.0.1.

use std::collections::HashSet;
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

#[test]
fn every_datagram_is_at_most_1200_bytes() {
    let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let tx = Session::open(0, Callbacks::none()).unwrap();
    tx.set_peer("127.0.0.1", peer.local_addr().unwrap().port())
        .unwrap();
    tx.add_stream(SID, Role::Sender, &keys(), 8000).unwrap();
    for (i, n) in [1usize, 1141, 1142, 2282, 30_000, 400_000]
        .iter()
        .enumerate()
    {
        tx.send_frame(SID, 1, &blob(*n, i), 1, 1).unwrap();
    }
    tx.send_frame(SID, 2, &blob(5000, 1), 1, 3 << 1).unwrap();
    let (mut n, mut max) = (0, 0);
    let mut buf = [0u8; 4096];
    while let Ok(len) = peer.recv(&mut buf) {
        n += 1;
        max = max.max(len);
        assert!(len <= MAX_PACKET, "Datagramm mit {len} Byte");
        assert!(decode_header(&buf[..len]).is_some());
    }
    assert!(n > 400, "es muessen viele Pakete gekommen sein, waren {n}");
    assert_eq!(max, MAX_PACKET, "volle Pakete nutzen die 1200 Byte aus");
}

#[test]
fn nack_recovers_losses() {
    let l = link(8000);
    l.tx.set_fec_enabled(SID, false).unwrap();
    // Die ersten Uebertragungen dieser seqs gehen verloren (Fragmente von Frame 1..4).
    let mut drop_once = HashSet::new();
    drop_once.extend([2u64, 3, 7, 12, 13, 14, 19]);
    l.rx.set_netem(NetemConfig {
        drop_once,
        ..Default::default()
    });
    let n = 8;
    for i in 0..n {
        l.tx.send_frame(SID, 1, &blob(5000, i), i as u64, if i == 0 { 1 } else { 0 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(
        wait_until(3.0, || l.rx_sink.frames.lock().unwrap().len() == n),
        "geliefert: {}",
        l.rx_sink.frames.lock().unwrap().len()
    );
    let fr = l.rx_sink.frames.lock().unwrap();
    for i in 0..n {
        assert_eq!(fr[i].data, blob(5000, i));
    }
    let c = l.tx.counters(SID).unwrap();
    assert!(c.retransmits >= 7, "Wiederholungen: {}", c.retransmits);
    assert!(l.rx.counters(SID).unwrap().nacks_sent > 0);
}

#[test]
fn fec_repairs_single_loss_per_group() {
    let l = link(8000);
    // Jeder Frame hat 5 Fragmente + 1 Paritaet = 6 seqs. Je Frame faellt genau ein Fragment aus.
    let mut drop_once = HashSet::new();
    let n = 8usize;
    for f in 0..n as u64 {
        drop_once.insert(f * 6 + 2);
    }
    l.rx.set_netem(NetemConfig {
        drop_once,
        ..Default::default()
    });
    for i in 0..n {
        l.tx.send_frame(SID, 1, &blob(5000, i), i as u64, if i == 0 { 1 } else { 0 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(wait_until(3.0, || l.rx_sink.frames.lock().unwrap().len() == n));
    let rc = l.rx.counters(SID).unwrap();
    assert_eq!(
        rc.fec_recovered, n as u64,
        "jeder Verlust per FEC repariert: {rc:?}"
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
    let start = Instant::now();
    let mut i = 0u64;
    while start.elapsed() < Duration::from_millis(2500) {
        l.tx.send_frame(SID, 1, &blob(3000, 0), i * 16_666, 0)
            .unwrap();
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
    let low = l.tx.target_kbps(SID);
    assert!(low < 6000);
    l.rx.set_netem(NetemConfig::default());
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(2500) {
        l.tx.send_frame(SID, 1, &blob(3000, 0), i * 16_666, 0)
            .unwrap();
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(l.tx.target_kbps(SID) > low, "Bitrate muss wieder steigen");
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
    assert!(l.tx_sink.stats.lock().unwrap().len() >= 2);
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
