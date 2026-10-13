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
    p.advance(70);
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

fn assert_drop_sum(s: &Stream) {
    let c = &s.c;
    assert_eq!(
        c.frames_dropped,
        c.dropped_incomplete + c.dropped_overflow + c.dropped_gap + c.dropped_audio,
        "Ursachen ergeben nicht frames_dropped: {c:?}"
    );
}

#[test]
fn drop_causes_incomplete_gap_and_late_packets_are_counted() {
    let mut p = pair(false);
    let (_, got) = {
        let k = p.send(&blob(3000), 1);
        p.feed_rx(&k)
    };
    assert_eq!(got, vec![1]);
    // Frame 2: nur ein Teil kommt rechtzeitig (Jitter-Spitze), Frame 3 und 4 vollstaendig.
    let f2 = p.send(&blob(3000), 0);
    p.feed_rx(&f2[..1]);
    let f3 = p.send(&blob(2000), 0);
    p.feed_rx(&f3);
    p.advance(70);
    p.tick_rx();
    assert_eq!(p.rx.c.dropped_incomplete, 1);
    assert_eq!(p.rx.c.gaps_incomplete, 1);
    assert_eq!(p.rx.c.gaps_blocked, 0);
    // Der Rest von Frame 2 kommt zu spaet.
    p.feed_rx(&f2[1..]);
    assert!(p.rx.c.late_packets >= 1);
    let f4 = p.send(&blob(2000), 0);
    p.feed_rx(&f4);
    // Keyframe 5: Frames 3 und 4 waren fertig, werden aber nach der Luecke nicht mehr gezeigt.
    let k5 = p.send(&blob(3000), 1);
    let (_, got) = p.feed_rx(&k5);
    assert_eq!(got, vec![5]);
    assert_eq!(p.rx.c.dropped_gap, 2);
    assert_eq!(p.rx.c.frames_dropped, 3);
    assert_drop_sum(&p.rx);
}

#[test]
fn missing_whole_frame_counts_as_blocked_gap() {
    let mut p = pair(false);
    let k = p.send(&blob(3000), 1);
    p.feed_rx(&k);
    let _f2_lost_completely = p.send(&blob(3000), 0);
    let f3 = p.send(&blob(2000), 0);
    p.feed_rx(&f3);
    p.advance(70);
    p.tick_rx();
    assert_eq!(p.rx.c.gaps_blocked, 1);
    assert_eq!(p.rx.c.dropped_incomplete, 0);
    assert_drop_sum(&p.rx);
}

/// Hoechster Wert eines Zahlenfelds ueber alle Stats-Zeilen.
fn max_field(stats: &[String], key: &str) -> f64 {
    stats
        .iter()
        .filter_map(|s| {
            s.split(&format!("\"{key}\":"))
                .nth(1)
                .and_then(|r| r.split([',', '}']).next())
                .and_then(|v| v.parse::<f64>().ok())
        })
        .fold(0.0, f64::max)
}

struct SimRun {
    raw_rx: f64,
    raw_tx: f64,
    target: u32,
    delivered: u64,
    /// Erster Zeitpunkt (ms), zu dem die Zielbitrate <= 70 % des Maximums war.
    below_70: Option<u64>,
}

/// Simulierte Strecke: 60 fps, 5000 Byte je Frame, Maximum 8000 kbit/s. Hinrichtung mit
/// 0..`jitter_ms` zufaelliger Verzoegerung und `loss_pct` Verlust, Rueckrichtung ohne Stoerung
/// (wie `set_netem` auf dem Empfaenger). Simulierte Uhr in 1-ms-Schritten, fester Seed: keine Sockets,
/// keine Wall-Clock, auf jedem Runner gleich.
fn sim_run(fec: bool, seed: u64, jitter_ms: f64, loss_pct: f64, total_ms: u64) -> SimRun {
    use beamer_transport::netem::Rng;
    use beamer_transport::stream::TickInfo;
    let mut p = pair(fec);
    let t0 = p.now;
    let mut rng = Rng::new(seed);
    let info = TickInfo::default();
    let mut to_rx: Vec<(Instant, u64, Vec<u8>)> = Vec::new();
    let mut to_tx: Vec<Vec<u8>> = Vec::new();
    let (mut rx_stats, mut tx_stats) = (Vec::new(), Vec::new());
    let mut order = 0u64;
    let mut frame = 0u64;
    let mut below_70 = None;
    let jitter = |rng: &mut Rng, now: Instant| {
        now + Duration::from_secs_f64(rng.next_f64() * jitter_ms / 1000.0)
    };
    for ms in 0..total_ms {
        let now = t0 + Duration::from_millis(ms);
        let mut io = Io::default();
        if ms % 16 == 0 {
            let _ = p.tx.send_frame(
                now,
                1,
                &blob(5000),
                ms * 1000,
                frame.is_multiple_of(120) as u8,
                &mut io,
            );
            frame += 1;
        }
        p.tx.tick(now, &info, &mut io);
        for d in to_tx.drain(..) {
            let h = decode_header(&d).unwrap();
            p.tx.receive(now, &d, &h, &mut io);
        }
        for d in io.out.drain(..) {
            if loss_pct > 0.0 && rng.next_f64() * 100.0 < loss_pct {
                continue;
            }
            order += 1;
            to_rx.push((jitter(&mut rng, now), order, d));
        }
        if below_70.is_none() && p.tx.target_kbps() <= 5600 {
            below_70 = Some(ms);
        }
        for e in io.events.drain(..) {
            if let Event::Stats { json, .. } = e {
                tx_stats.push(json);
            }
        }
        // Faellige Pakete in Ankunftsreihenfolge an den Empfaenger.
        to_rx.sort_by_key(|(due, o, _)| (*due, *o));
        let n = to_rx.iter().take_while(|(due, _, _)| *due <= now).count();
        let mut rio = Io::default();
        for (_, _, d) in to_rx.drain(..n) {
            let h = decode_header(&d).unwrap();
            p.rx.receive(now, &d, &h, &mut rio);
        }
        p.rx.tick(now, &info, &mut rio);
        to_tx.append(&mut rio.out);
        for e in rio.events.drain(..) {
            if let Event::Stats { json, .. } = e {
                rx_stats.push(json);
            }
        }
    }
    assert!(rx_stats.len() >= 3 && tx_stats.len() >= 3);
    SimRun {
        raw_rx: max_field(&rx_stats, "loss_pct"),
        raw_tx: max_field(&tx_stats, "loss_pct"),
        target: p.tx.target_kbps(),
        delivered: p.rx.c.frames_delivered,
        below_70,
    }
}

#[test]
fn reordering_without_loss_is_not_raw_loss() {
    // 0 % Verlust, aber bis zu 10 ms Umordnung: Rohverlust < 1 %, Zielbitrate bleibt am Maximum.
    // Deterministisch (simulierte Uhr, feste Seeds) statt Sockets und Wall-Clock wie frueher in
    // tests/session.rs; mehrere Seeds decken verschiedene Umordnungsmuster ab.
    for fec in [true, false] {
        for seed in 1..=4 {
            let r = sim_run(fec, seed, 10.0, 0.0, 4500);
            assert!(
                r.raw_rx < 1.0 && r.raw_tx < 1.0,
                "FEC {fec}, Seed {seed}: Umordnung zaehlt als Verlust: {} / {}",
                r.raw_rx,
                r.raw_tx
            );
            assert!(
                r.target >= 7600,
                "FEC {fec}, Seed {seed}: Zielbitrate muss (fast) am Maximum bleiben: {}",
                r.target
            );
            assert!(
                r.delivered >= 270,
                "FEC {fec}, Seed {seed}: nur {} Frames",
                r.delivered
            );
        }
    }
}

#[test]
fn bitrate_drops_at_20_percent_loss() {
    // Kriterium: bei 20 % Verlust innerhalb von 3 s um mindestens 30 % runter (Regel: -15 %/s bei
    // Restverlust > 1 % bzw. Rohverlust > 8 %). Deterministisch statt Sockets und Wall-Clock wie frueher
    // in tests/session.rs (dort auf langsamen CI-Runnern gelegentlich rot).
    for seed in 1..=4 {
        let r = sim_run(true, seed, 0.0, 20.0, 3300);
        let at = r.below_70.unwrap_or_else(|| {
            panic!(
                "Seed {seed}: Zielbitrate nie auf 70 % gesunken ({})",
                r.target
            )
        });
        eprintln!("Seed {seed}: <= 70 % nach {at} ms");
        assert!(
            at <= 3000,
            "Seed {seed}: mindestens 30 % Absenkung innerhalb von 3 s erwartet, erreicht nach {at} ms"
        );
    }
}

// ---- Frame-Frist nach Vertrag v1.2, §4.1 (2 x RTT + 4 x Jitter, 60..250 ms, ab dem letzten Paket) ----

/// Empfaenger mit vorgegebener RTT-/Jitter-Schaetzung (die Uhr steht still, nur `advance` bewegt sie).
fn pair_with_link(rtt_ms: f64, jitter_ms: f64) -> Pair {
    let mut p = pair(false);
    p.rx.set_link_estimate(rtt_ms, jitter_ms);
    p
}

/// Keyframe 1 kommt an, danach Frame 2 mit drei Paketen; vom letzten Paket an laeuft die Frist.
/// Gibt die Pakete von Frame 2 zurueck (erste zwei sind schon beim Empfaenger).
fn start_partial_frame(p: &mut Pair) -> Vec<Vec<u8>> {
    let k = p.send(&blob(3000), 1);
    let (_, got) = p.feed_rx(&k);
    assert_eq!(got, vec![1]);
    let f2 = p.send(&blob(3000), 0);
    assert!(f2.len() >= 3, "Frame 2 braucht mindestens drei Pakete");
    p.feed_rx(&f2[..2]);
    f2
}

/// Zeit in ms (Schritte von 1 ms) ab dem letzten Paket, bis der Empfaenger den unvollstaendigen Frame verwirft
/// und einen keyframe_req schickt. NACK-Antworten gibt es nie (echter Verlust).
fn drop_time_ms(p: &mut Pair) -> (u64, bool) {
    let mut kf_req = false;
    for t in 1..=400u64 {
        p.advance(1);
        let (out, _) = p.tick_rx();
        kf_req |= ptypes(&out).contains(&T_KEYFRAME_REQ);
        if p.rx.c.dropped_incomplete > 0 {
            return (t, kf_req);
        }
    }
    panic!("Frame wurde nie verworfen");
}

#[test]
fn deadline_formula_and_bounds() {
    // (c) Grenzen: unter 60 ms wird auf 60 angehoben, ueber 250 ms auf 250 gekappt.
    for (rtt, jit, want) in [
        (0.0, 0.0, 60.0),
        (5.0, 1.0, 60.0),  // 14 ms
        (20.0, 8.0, 72.0), // 2*20 + 4*8
        (10.0, 8.0, 60.0), // 52 ms -> 60
        (30.0, 10.0, 100.0),
        (100.0, 20.0, 250.0), // 280 ms -> 250
        (500.0, 100.0, 250.0),
    ] {
        let p = pair_with_link(rtt, jit);
        assert!(
            (p.rx.deadline_ms() - want).abs() < 1e-6,
            "RTT {rtt} ms, Jitter {jit} ms: Frist {} ms, erwartet {want}",
            p.rx.deadline_ms()
        );
    }
    // Ohne Messung (noch kein Pong): Untergrenze.
    assert_eq!(pair(false).rx.deadline_ms(), 60.0);
}

#[test]
fn outage_shorter_than_deadline_does_not_drop_the_frame() {
    // (a) RTT 20 ms, Jitter 8 ms -> Frist 72 ms. Ein Aussetzer von 50 ms mitten im Frame:
    let mut p = pair_with_link(20.0, 8.0);
    let f2 = start_partial_frame(&mut p);
    p.advance(50);
    p.tick_rx();
    assert_eq!(p.rx.c.dropped_incomplete, 0, "nach 50 ms noch kein Verwurf");
    let (_, got) = p.feed_rx(&f2[2..]);
    assert_eq!(got, vec![2], "der Frame wird vollstaendig ausgeliefert");
    assert_eq!(p.rx.c.frames_dropped, 0);
    assert_eq!(p.rx.c.late_packets, 0, "kein Paket kam zu spaet");

    // Gegenprobe mit der alten Frist (2 x RTT = 40 ms, ohne Jitter): derselbe Aussetzer verwirft den Frame,
    // der Rest kommt als spaetes Paket (`late_packets` > 0).
    let mut q = pair_with_link(20.0, 0.0);
    q.rx.min_deadline_ms = 30.0;
    assert_eq!(q.rx.deadline_ms(), 40.0);
    let f2 = start_partial_frame(&mut q);
    q.advance(50);
    q.tick_rx();
    assert_eq!(q.rx.c.dropped_incomplete, 1);
    q.feed_rx(&f2[2..]);
    assert!(q.rx.c.late_packets > 0);
}

#[test]
fn real_loss_without_nack_reply_drops_by_the_deadline_and_asks_for_a_keyframe() {
    // (b) Das dritte Paket kommt nie (NACK bleibt unbeantwortet).
    for (rtt, jit, nominal) in [(20.0, 8.0, 72.0), (0.0, 0.0, 60.0), (200.0, 50.0, 250.0)] {
        let mut p = pair_with_link(rtt, jit);
        let _ = start_partial_frame(&mut p);
        // Die Jitter-Schaetzung bewegt sich beim Empfang der ersten Pakete etwas: Soll ist die Frist danach.
        let want = p.rx.deadline_ms();
        assert!(
            (want - nominal).abs() <= 4.0,
            "Frist {want} ms weicht von {nominal} ab"
        );
        let (t, kf_req) = drop_time_ms(&mut p);
        assert!(
            (t as f64) >= want.floor() && (t as f64) <= want.ceil() + 1.0,
            "RTT {rtt}, Jitter {jit}: verworfen nach {t} ms, Frist {want} ms"
        );
        assert!(t <= 250 + 1, "hoechstens 250 ms");
        assert!(kf_req, "keyframe_req nach dem Verwurf");
        assert_eq!(p.rx.c.gaps_incomplete, 1);
    }
}

// ---- WLAN-Nachbildung (aus 03a): 10 ms Verzoegerung je Richtung, 0..8 ms Jitter, etwa jede Sekunde ein Aussetzer
// von 50 ms (alle Pakete beider Richtungen werden festgehalten und danach gesammelt freigegeben). ----

struct WifiRun {
    sent: u64,
    dropped_incomplete: u64,
    dropped_total: u64,
    late: u64,
}

fn wifi_run(fec: bool, seed: u64, total_ms: u64, stall_ms: u64) -> WifiRun {
    use beamer_transport::netem::Rng;
    use beamer_transport::stream::TickInfo;
    let mut p = pair(fec);
    let t0 = p.now;
    let mut rng = Rng::new(seed);
    let info = TickInfo::default();
    // (faellig, laufende Nummer, Datagramm)
    let mut to_rx: Vec<(u64, u64, Vec<u8>)> = Vec::new();
    let mut to_tx: Vec<(u64, u64, Vec<u8>)> = Vec::new();
    let mut order = 0u64;
    let mut frame = 0u64;
    let mut force_key = false;
    // Aussetzer vorab festlegen: im Mittel alle Sekunde (0,5..1,5-fach), je `stall_ms` lang.
    let mut windows: Vec<(u64, u64)> = Vec::new();
    let mut at = 500 + (rng.next_f64() * 1000.0) as u64;
    while stall_ms > 0 && at < total_ms {
        windows.push((at, at + stall_ms));
        at += stall_ms + (500.0 + rng.next_f64() * 1000.0) as u64;
    }
    // Ein Paket, das im Aussetzer ankommen wuerde, wird bis zu dessen Ende festgehalten.
    let stalled = |due: u64| {
        windows
            .iter()
            .find(|(a, b)| due >= *a && due < *b)
            .map_or(due, |(_, b)| *b)
    };
    for ms in 0..total_ms {
        let now = t0 + Duration::from_millis(ms);
        let mut io = Io::default();
        if ms % 16 == 0 {
            let key = frame.is_multiple_of(120) || force_key;
            force_key = false;
            let _ =
                p.tx.send_frame(now, 1, &blob(5000), ms * 1000, key as u8, &mut io);
            frame += 1;
        }
        p.tx.tick(now, &info, &mut io);
        let due_tx: Vec<_> = {
            let n = to_tx.iter().filter(|(d, _, _)| *d <= ms).count();
            to_tx.sort_by_key(|(d, o, _)| (*d, *o));
            to_tx.drain(..n).collect()
        };
        for (_, _, d) in due_tx {
            let h = decode_header(&d).unwrap();
            p.tx.receive(now, &d, &h, &mut io);
        }
        for e in &io.events {
            if let Event::Control { ptype, .. } = e {
                if *ptype == T_KEYFRAME_REQ {
                    force_key = true;
                }
            }
        }
        for d in io.out.drain(..) {
            order += 1;
            let due = ms + 10 + (rng.next_f64() * 8.0) as u64;
            to_rx.push((stalled(due), order, d));
        }
        let n = to_rx.iter().filter(|(d, _, _)| *d <= ms).count();
        to_rx.sort_by_key(|(d, o, _)| (*d, *o));
        let mut rio = Io::default();
        for (_, _, d) in to_rx.drain(..n) {
            let h = decode_header(&d).unwrap();
            p.rx.receive(now, &d, &h, &mut rio);
        }
        p.rx.tick(now, &info, &mut rio);
        for d in rio.out.drain(..) {
            order += 1;
            let due = ms + 10;
            to_tx.push((stalled(due), order, d));
        }
    }
    WifiRun {
        sent: p.tx.c.frames_sent,
        dropped_incomplete: p.rx.c.dropped_incomplete,
        dropped_total: p.rx.c.frames_dropped,
        late: p.rx.c.late_packets,
    }
}

#[test]
fn wifi_replica_drops_few_frames() {
    let mut results = Vec::new();
    for fec in [true, false] {
        let (mut sent, mut inc, mut tot, mut late) = (0, 0, 0, 0);
        for seed in 1..=6 {
            let r = wifi_run(fec, seed, 60_000, 50);
            sent += r.sent;
            inc += r.dropped_incomplete;
            tot += r.dropped_total;
            late += r.late;
        }
        let pct = |n: u64| 100.0 * n as f64 / sent as f64;
        eprintln!(
            "WLAN-Nachbildung FEC {fec}: {sent} Frames, unvollstaendig verworfen {inc} ({:.3} %), alle verworfenen {tot} ({:.3} %), zu spaete Pakete {late}",
            pct(inc),
            pct(tot)
        );
        results.push((fec, pct(tot)));
    }
    for (fec, pct) in results {
        assert!(pct < 0.5, "FEC {fec}: {pct:.3} % verworfen");
    }
}
