//! Stream-Logik ohne Socket und ohne echte Zeit: Datagramme werden von Hand zwischen einem
//! Sender- und einem Empfaenger-Stream weitergereicht, die Uhr ist ein selbst gefuehrter `Instant`.

use std::time::{Duration, Instant};

use beamer_transport::packet::{decode_header, T_KEYFRAME_REQ, T_NACK};
use beamer_transport::stream::{Event, Io, Role, Stream};

const SID: u32 = 7;

struct Pair {
    tx: Stream,
    rx: Stream,
    now: Instant,
}

fn pair(fec: bool) -> Pair {
    let now = Instant::now();
    let (k1, k2) = ([1u8; 32], [2u8; 32]);
    let (p1, p2) = ([1u8; 4], [2u8; 4]);
    let mut tx = Stream::new(SID, Role::Sender, &k1, &p1, &k2, &p2, 8000, now);
    let rx = Stream::new(SID, Role::Receiver, &k1, &p1, &k2, &p2, 8000, now);
    tx.fec_enabled = fec;
    Pair { tx, rx, now }
}

impl Pair {
    fn advance(&mut self, ms: u64) {
        self.now += Duration::from_millis(ms);
    }

    fn send(&mut self, data: &[u8], fflags: u8) -> Vec<Vec<u8>> {
        let mut io = Io::default();
        self.tx
            .send_frame(self.now, 1, data, 1000, fflags, &mut io)
            .unwrap();
        io.out
    }

    /// Reicht Datagramme an den Empfaenger und gibt (Antworten, ausgelieferte frame_ids) zurueck.
    fn feed_rx(&mut self, ds: &[Vec<u8>]) -> (Vec<Vec<u8>>, Vec<u32>) {
        let mut io = Io::default();
        for d in ds {
            let h = decode_header(d).unwrap();
            self.rx.receive(self.now, d, &h, &mut io);
        }
        let f = frames(&io);
        (io.out, f)
    }

    fn feed_tx(&mut self, ds: &[Vec<u8>]) -> (Vec<Vec<u8>>, usize) {
        let mut io = Io::default();
        let mut controls = 0;
        for d in ds {
            let h = decode_header(d).unwrap();
            self.tx.receive(self.now, d, &h, &mut io);
        }
        for e in &io.events {
            if matches!(e, Event::Control { .. }) {
                controls += 1;
            }
        }
        (io.out, controls)
    }

    fn tick_rx(&mut self) -> (Vec<Vec<u8>>, Vec<u32>) {
        let mut io = Io::default();
        self.rx.tick(self.now, &Default::default(), &mut io);
        let f = frames(&io);
        (io.out, f)
    }
}

fn frames(io: &Io) -> Vec<u32> {
    io.events
        .iter()
        .filter_map(|e| match e {
            Event::Frame { frame_id, .. } => Some(*frame_id),
            _ => None,
        })
        .collect()
}

fn ptypes(ds: &[Vec<u8>]) -> Vec<u8> {
    ds.iter().map(|d| decode_header(d).unwrap().ptype).collect()
}

fn blob(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 31 % 251) as u8).collect()
}

#[test]
fn nack_restores_a_lost_packet() {
    let mut p = pair(false);
    let ds = p.send(&blob(5000), 1);
    assert_eq!(ds.len(), 5);
    let kept: Vec<_> = ds
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 2)
        .map(|(_, d)| d.clone())
        .collect();
    let (_, got) = p.feed_rx(&kept);
    assert!(got.is_empty(), "Frame darf nicht vollstaendig sein");

    p.advance(10);
    let (out, _) = p.tick_rx();
    assert!(
        ptypes(&out).contains(&T_NACK),
        "NACK erwartet: {:?}",
        ptypes(&out)
    );

    let nack: Vec<_> = out
        .into_iter()
        .filter(|d| decode_header(d).unwrap().ptype == T_NACK)
        .collect();
    let (retx, _) = p.feed_tx(&nack);
    assert_eq!(retx.len(), 1);
    assert_eq!(
        retx[0], ds[2],
        "erneut gesendet wird das identische Datagramm"
    );
    let (_, got) = p.feed_rx(&retx);
    assert_eq!(got, vec![1]);
    assert_eq!(p.tx.c.retransmits, 1);
}

#[test]
fn fec_repairs_one_loss_per_group_without_nack() {
    let mut p = pair(true);
    let data = blob(5000);
    let ds = p.send(&data, 1);
    assert_eq!(ds.len(), 6, "5 Fragmente + 1 Paritaet");
    for lost in 0..5 {
        let mut q = pair(true);
        let ds = q.send(&data, 1);
        let kept: Vec<_> = ds
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != lost)
            .map(|(_, d)| d.clone())
            .collect();
        let mut io = Io::default();
        for d in &kept {
            let h = decode_header(d).unwrap();
            q.rx.receive(q.now, d, &h, &mut io);
        }
        assert_eq!(
            frames(&io),
            vec![1],
            "Fragment {lost} muss repariert werden"
        );
        match &io.events[0] {
            Event::Frame { data: got, .. } => assert_eq!(got, &data),
            _ => unreachable!(),
        }
        assert_eq!(q.rx.c.fec_recovered, 1);
        q.advance(20);
        let (out, _) = q.tick_rx();
        assert!(
            !ptypes(&out).contains(&T_NACK),
            "kein NACK nach FEC-Reparatur"
        );
    }
}

#[test]
fn fec_groups_of_eight_each_repair_one() {
    // 20 Fragmente = Gruppen 8 + 8 + 4, je ein Paritaetspaket.
    let mut p = pair(true);
    let data = blob(20 * 1141);
    let ds = p.send(&data, 1);
    assert_eq!(ds.len(), 23);
    // Verlust: Fragment 3 (Gruppe 1), 10 (Gruppe 2), 17 (Gruppe 3). Datagramm-Index = Fragment + Paritaeten davor.
    let idx = |frag: usize| frag + frag / 8;
    let drop = [idx(3), idx(10), idx(17)];
    let kept: Vec<_> = ds
        .iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, d)| d.clone())
        .collect();
    let (_, got) = p.feed_rx(&kept);
    assert_eq!(got, vec![1]);
    assert_eq!(p.rx.c.fec_recovered, 3);
}

#[test]
fn two_losses_in_one_group_need_nack() {
    let mut p = pair(true);
    let data = blob(5000);
    let ds = p.send(&data, 1);
    let kept: Vec<_> = ds
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1 && *i != 3)
        .map(|(_, d)| d.clone())
        .collect();
    let (_, got) = p.feed_rx(&kept);
    assert!(got.is_empty());
    p.advance(10);
    let (out, _) = p.tick_rx();
    let nack: Vec<_> = out
        .into_iter()
        .filter(|d| decode_header(d).unwrap().ptype == T_NACK)
        .collect();
    assert!(!nack.is_empty());
    let (retx, _) = p.feed_tx(&nack);
    assert_eq!(retx.len(), 2);
    let (_, got) = p.feed_rx(&retx);
    assert_eq!(got, vec![1]);
}

#[test]
fn reordering_inside_nack_delay_causes_no_nack() {
    let mut p = pair(false);
    let ds = p.send(&blob(5000), 1);
    let mut swapped = ds.clone();
    swapped.swap(1, 3);
    let (_, got) = p.feed_rx(&swapped);
    assert_eq!(got, vec![1]);
    p.advance(20);
    let (out, _) = p.tick_rx();
    assert!(!ptypes(&out).contains(&T_NACK));
}

#[test]
fn lost_frame_drops_dependents_and_requests_keyframe_at_most_every_300ms() {
    let mut p = pair(false);
    // Frame 1 (Keyframe) kommt an.
    let k = p.send(&blob(3000), 1);
    let (_, got) = p.feed_rx(&k);
    assert_eq!(got, vec![1]);
    // Frame 2 verliert ein Paket und wird nie nachgeliefert.
    let f2 = p.send(&blob(3000), 0);
    let kept: Vec<_> = f2.iter().take(2).cloned().collect();
    p.feed_rx(&kept);
    // Frame 3 ist vollstaendig, darf aber nicht vor dem fehlenden Frame 2 ausgeliefert werden.
    let f3 = p.send(&blob(2000), 0);
    let (_, got) = p.feed_rx(&f3);
    assert!(got.is_empty());

    // Nach der Frist: Frame 2 verworfen, keyframe_req raus, Frame 3 (abhaengig) bleibt weg.
    p.advance(40);
    let (out, got) = p.tick_rx();
    assert!(got.is_empty());
    assert_eq!(
        ptypes(&out)
            .iter()
            .filter(|t| **t == T_KEYFRAME_REQ)
            .count(),
        1
    );
    assert!(p.rx.c.frames_dropped >= 1);

    // Innerhalb von 300 ms kein zweiter keyframe_req.
    p.advance(100);
    let (out, _) = p.tick_rx();
    assert_eq!(
        ptypes(&out)
            .iter()
            .filter(|t| **t == T_KEYFRAME_REQ)
            .count(),
        0
    );

    // Der Sender bekommt den keyframe_req als Control-Ereignis.
    let reqs: Vec<_> = {
        p.advance(250);
        let (out, _) = p.tick_rx();
        out.into_iter()
            .filter(|d| decode_header(d).unwrap().ptype == T_KEYFRAME_REQ)
            .collect()
    };
    assert_eq!(reqs.len(), 1, "nach >300 ms wird erneut angefragt");
    let (_, controls) = p.feed_tx(&reqs);
    assert_eq!(controls, 1);

    // Neuer Keyframe heilt den Strom; danach laufen P-Frames wieder.
    let k2 = p.send(&blob(3000), 1);
    let (_, got) = p.feed_rx(&k2);
    assert_eq!(got, vec![4]);
    let f5 = p.send(&blob(2000), 0);
    let (_, got) = p.feed_rx(&f5);
    assert_eq!(got, vec![5]);
}

#[test]
fn keyframe_request_api_is_rate_limited() {
    let mut p = pair(false);
    let mut io = Io::default();
    assert!(p.rx.request_keyframe(p.now, &mut io));
    p.advance(299);
    assert!(!p.rx.request_keyframe(p.now, &mut io));
    p.advance(2);
    assert!(p.rx.request_keyframe(p.now, &mut io));
    assert_eq!(io.out.len(), 2);
}

#[test]
fn frames_are_delivered_in_order_even_if_packets_arrive_interleaved() {
    let mut p = pair(false);
    let a = p.send(&blob(4000), 1);
    let b = p.send(&blob(4000), 0);
    let mut mixed = Vec::new();
    for i in 0..a.len().max(b.len()) {
        if let Some(d) = b.get(i) {
            mixed.push(d.clone());
        }
        if let Some(d) = a.get(i) {
            mixed.push(d.clone());
        }
    }
    let (_, got) = p.feed_rx(&mixed);
    assert_eq!(got, vec![1, 2]);
}

fn out_bytes(ds: &[Vec<u8>]) -> usize {
    ds.iter().map(|d| d.len()).sum()
}

#[test]
fn pacing_limits_burst_and_rate() {
    let now = Instant::now();
    let (k1, k2) = ([1u8; 32], [2u8; 32]);
    let mut tx = Stream::new(SID, Role::Sender, &k1, &[1; 4], &k2, &[2; 4], 2000, now);
    let mut io = Io::default();
    tx.send_frame(now, 1, &blob(200_000), 1, 1, &mut io)
        .unwrap();
    let burst = out_bytes(&io.out);
    assert!(burst <= 64 * 1024, "Burst {burst}");
    assert!(burst > 60 * 1024, "Burst nutzt den Bucket aus: {burst}");
    // 100 ms spaeter: 1,5 x 2000 kbit/s = 375 kB/s -> ~37,5 kB neu.
    let mut io = Io::default();
    tx.tick(
        now + Duration::from_millis(100),
        &Default::default(),
        &mut io,
    );
    let more = out_bytes(&io.out);
    assert!((35_000..=40_000).contains(&more), "nach 100 ms: {more}");
    // Irgendwann ist alles raus.
    let mut total = burst + more;
    for k in 2..40 {
        let mut io = Io::default();
        tx.tick(
            now + Duration::from_millis(100 * k),
            &Default::default(),
            &mut io,
        );
        total += out_bytes(&io.out);
    }
    assert!(
        total > 200_000 && !tx.needs_fast_tick(),
        "Warteschlange leer: {total}"
    );
}

fn feedback_dgram(seq: u64, resid: u16, raw: u16, with_raw: bool) -> Vec<u8> {
    let c = beamer_transport::packet::Cipher::new(&[2u8; 32], &[2; 4]);
    let mut p = Vec::new();
    p.extend_from_slice(&4000u32.to_be_bytes());
    p.extend_from_slice(&resid.to_be_bytes());
    p.extend_from_slice(&1000u32.to_be_bytes());
    p.extend_from_slice(&1u32.to_be_bytes());
    if with_raw {
        p.extend_from_slice(&raw.to_be_bytes());
    }
    c.seal(
        &beamer_transport::packet::Header {
            ptype: beamer_transport::packet::T_FEEDBACK,
            flags: 0,
            stream_id: SID,
            seq,
        },
        &p,
    )
    .unwrap()
}

/// Schickt jede Sekunde ein Feedback und gibt die Zielbitrate danach zurueck.
fn run_feedback(
    p: &mut Pair,
    seconds: u64,
    resid: u16,
    raw: u16,
    with_raw: bool,
    seq0: u64,
) -> u32 {
    for k in 0..seconds {
        p.advance(1000);
        let d = feedback_dgram(seq0 + k, resid, raw, with_raw);
        p.feed_tx(&[d]);
    }
    p.tx.target_kbps()
}

#[test]
fn rate_follows_residual_loss_not_raw_loss() {
    // 5 % Rohverlust, aber 0 Restverlust (FEC/NACK haben alles repariert): keine Drosselung.
    let mut p = pair(true);
    assert_eq!(run_feedback(&mut p, 3, 0, 50, true, 1), 8000);

    // Restverlust 2 % (> 1 %): runter, ca. 15 % je Sekunde.
    let mut p = pair(true);
    let t = run_feedback(&mut p, 3, 20, 20, true, 1);
    assert!((4800..=5300).contains(&t), "nach 3 s Restverlust: {t}");

    // Restverlust knapp unter der Schwelle (0,9 %): keine Drosselung.
    let mut p = pair(true);
    assert_eq!(run_feedback(&mut p, 3, 9, 30, true, 1), 8000);

    // Notbremse: 10 % Rohverlust trotz 0 Restverlust.
    let mut p = pair(true);
    let t = run_feedback(&mut p, 3, 0, 100, true, 1);
    assert!(t < 5400, "Notbremse greift: {t}");

    // Genau 8 % roh ist noch keine Notbremse.
    let mut p = pair(true);
    assert_eq!(run_feedback(&mut p, 3, 0, 80, true, 1), 8000);

    // Altes Feedback ohne Rohverlust-Feld: loss_permille gilt auch als Rohwert.
    let mut p = pair(true);
    let t = run_feedback(&mut p, 3, 90, 0, false, 1);
    assert!(t < 5400, "{t}");
}

#[test]
fn rate_recovers_after_throttling() {
    let mut p = pair(true);
    let low = run_feedback(&mut p, 4, 50, 50, true, 1);
    assert!(low < 4800);
    let up = run_feedback(&mut p, 4, 0, 0, true, 100);
    assert!(up > low, "{low} -> {up}");
}

#[test]
fn send_queue_is_limited_by_time_not_bytes() {
    let now = Instant::now();
    let (k1, k2) = ([1u8; 32], [2u8; 32]);
    // 1000 kbit/s -> Bucket-Rate 187,5 kB/s -> 150 ms = ~28 KB Warteschlange.
    let mut tx = Stream::new(SID, Role::Sender, &k1, &[1; 4], &k2, &[2; 4], 1000, now);
    let mut io = Io::default();
    // Ein einzelnes grosses Keyframe wird bei leerer Warteschlange immer angenommen.
    tx.send_frame(now, 1, &blob(150_000), 1, 1, &mut io)
        .unwrap();
    // Die Warteschlange hat jetzt weit mehr als 150 ms: das naechste Frame wird abgelehnt.
    let mut io2 = Io::default();
    let e = tx
        .send_frame(now, 1, &blob(3000), 2, 0, &mut io2)
        .unwrap_err();
    assert!(e.contains("Sendewarteschlange"), "{e}");
    // Nach dem Abfliessen unter 150 ms geht es wieder: Rest ~90 KB, 187,5 kB/s -> nach 0,6 s < 28 KB.
    let mut io3 = Io::default();
    // Wie im Betrieb in kleinen Schritten ticken (ein Tick gibt hoechstens den Burst frei).
    for k in 1..=6 {
        tx.tick(
            now + Duration::from_millis(100 * k),
            &Default::default(),
            &mut io3,
        );
    }
    assert!(tx
        .send_frame(
            now + Duration::from_millis(600),
            1,
            &blob(3000),
            3,
            0,
            &mut io3
        )
        .is_ok());
    // Stats nennen die Wartezeit.
    let mut io4 = Io::default();
    tx.tick(
        now + Duration::from_millis(1100),
        &Default::default(),
        &mut io4,
    );
    let json = io4
        .events
        .iter()
        .find_map(|e| match e {
            Event::Stats { json, .. } => Some(json.clone()),
            _ => None,
        })
        .expect("Stats nach 1 s");
    assert!(json.contains("\"pace_queue_ms\":"), "{json}");
}
