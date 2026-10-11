//! Sender und Empfaenger in einem Prozess ueber 127.0.0.1, mit kuenstlicher Netzstoerung auf der
//! Empfangsseite beider Sessions. Sendet synthetische Frames (30 KB Keyframe alle 2 s, sonst 3-8 KB, 60 fps).
//!
//!   cargo run --release --example loopback -- --loss 3 --jitter 10 --secs 30
//!
//! Optionen: --loss <%> --jitter <ms> --reorder <%> --secs <s> --fps <n> --max-kbps <n> --no-fec
//!           --stats (Statistik-JSON jede Sekunde ausgeben) --one-way (Stoerung nur fuer Medien, nicht fuer NACK/Feedback) --no-baseline --seed <n>

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beamer_transport::netem::Rng;
use beamer_transport::{Callbacks, NetemConfig, Role, Session, StreamKeys};

const SID: u32 = 1;

struct Opts {
    loss: f64,
    jitter: f64,
    reorder: f64,
    secs: f64,
    fps: f64,
    max_kbps: u32,
    fec: bool,
    one_way: bool,
    baseline: bool,
    seed: u64,
    stats: bool,
}

fn parse() -> Opts {
    let mut o = Opts {
        loss: 0.0,
        jitter: 0.0,
        reorder: 0.0,
        secs: 10.0,
        fps: 60.0,
        max_kbps: 20_000,
        fec: true,
        one_way: false,
        baseline: true,
        seed: 1,
        stats: false,
    };
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let val = |i: &mut usize| -> f64 {
        *i += 1;
        a.get(*i).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
            eprintln!("Wert fehlt oder ungueltig nach {}", a[*i - 1]);
            std::process::exit(2)
        })
    };
    while i < a.len() {
        match a[i].as_str() {
            "--loss" => o.loss = val(&mut i),
            "--jitter" => o.jitter = val(&mut i),
            "--reorder" => o.reorder = val(&mut i),
            "--secs" => o.secs = val(&mut i),
            "--fps" => o.fps = val(&mut i),
            "--max-kbps" => o.max_kbps = val(&mut i) as u32,
            "--seed" => o.seed = val(&mut i) as u64,
            "--stats" => o.stats = true,
            "--no-fec" => o.fec = false,
            "--one-way" => o.one_way = true,
            "--no-baseline" => o.baseline = false,
            x => {
                eprintln!("unbekannte Option {x}");
                std::process::exit(2)
            }
        }
        i += 1;
    }
    o
}

struct Report {
    sent: u64,
    complete: u64,
    delivered: u64,
    lat_ms: Vec<f64>,
    retransmits: u64,
    nacks: u64,
    fec_recovered: u64,
    fec_sent: u64,
    dropped: u64,
    keyframe_reqs: u64,
    min_target: u32,
    refused: u64,
    final_target: u32,
    kbps: f64,
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * p).round() as usize]
}

fn run(o: &Opts, loss: f64, secs: f64) -> Report {
    let keys = StreamKeys {
        s2r_key: [0x5a; 32],
        s2r_prefix: [1, 2, 3, 4],
        r2s_key: [0xa5; 32],
        r2s_prefix: [9, 8, 7, 6],
    };
    let t0 = Instant::now();
    let lat = Arc::new(Mutex::new(Vec::<f64>::new()));
    let want_key = Arc::new(AtomicBool::new(false));
    let l2 = lat.clone();
    let show_stats = o.stats;
    let rx = Session::open(
        0,
        Callbacks {
            on_stats: Box::new(move |_s, j| {
                if show_stats {
                    println!("  rx-stats {j}");
                }
            }),
            on_frame: Box::new(move |_s, _k, f| {
                let now = t0.elapsed().as_micros() as u64;
                l2.lock()
                    .unwrap()
                    .push(now.saturating_sub(f.pts_us) as f64 / 1000.0);
            }),
            ..Callbacks::none()
        },
    )
    .unwrap();
    let wk = want_key.clone();
    let tx = Session::open(
        0,
        Callbacks {
            on_control: Box::new(move |_s, t| {
                if t == 5 {
                    wk.store(true, Ordering::Relaxed);
                }
            }),
            ..Callbacks::none()
        },
    )
    .unwrap();
    tx.set_peer("127.0.0.1", rx.local_port()).unwrap();
    rx.set_peer("127.0.0.1", tx.local_port()).unwrap();
    tx.add_stream(SID, Role::Sender, &keys, o.max_kbps).unwrap();
    rx.add_stream(SID, Role::Receiver, &keys, o.max_kbps)
        .unwrap();
    tx.set_fec_enabled(SID, o.fec).unwrap();
    let cfg = |seed| NetemConfig {
        loss_pct: loss,
        jitter_ms: o.jitter,
        reorder_pct: o.reorder,
        seed,
        ..Default::default()
    };
    rx.set_netem(cfg(o.seed));
    if !o.one_way {
        tx.set_netem(cfg(o.seed + 1));
    }

    let payload = vec![0xabu8; 16 * 1024];
    let big = vec![0xcdu8; 30_000];
    let mut rng = Rng::new(o.seed + 99);
    let period = Duration::from_secs_f64(1.0 / o.fps);
    let key_every = (o.fps * 2.0) as u64;
    let mut i = 0u64;
    let mut min_target = u32::MAX;
    let start = Instant::now();
    let mut next = start;
    let mut bytes = 0u64;
    let mut refused = 0u64;
    while start.elapsed().as_secs_f64() < secs {
        let key = i.is_multiple_of(key_every) || want_key.swap(false, Ordering::Relaxed);
        // Wie ein echter Encoder die Zielbitrate beachten: Nennbitrate des Beispiels ca. 3000 kbit/s.
        let scale = (tx.target_kbps(SID) as f64 / 3000.0).clamp(0.05, 1.0);
        let data: &[u8] = if key {
            &big[..(big.len() as f64 * scale) as usize]
        } else {
            let n = (3000.0 + rng.next_f64() * 5000.0) * scale;
            &payload[..(n as usize).max(200)]
        };
        let pts = t0.elapsed().as_micros() as u64;
        if tx.send_frame(SID, 1, data, pts, key as u8).is_err() {
            refused += 1; // Sendewarteschlange voll
        }
        bytes += data.len() as u64;
        min_target = min_target.min(tx.target_kbps(SID));
        i += 1;
        next += period;
        while Instant::now() < next {
            let left = next.saturating_duration_since(Instant::now());
            if left > Duration::from_millis(2) {
                std::thread::sleep(left - Duration::from_millis(1));
            } else {
                std::hint::spin_loop();
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    std::thread::sleep(Duration::from_millis(500));
    let tc = tx.counters(SID).unwrap();
    let rc = rx.counters(SID).unwrap();
    let r = Report {
        sent: i,
        complete: rc.frames_complete,
        delivered: rc.frames_delivered,
        lat_ms: lat.lock().unwrap().clone(),
        retransmits: tc.retransmits,
        nacks: rc.nacks_sent,
        fec_recovered: rc.fec_recovered,
        fec_sent: tc.fec_sent,
        dropped: rc.frames_dropped,
        keyframe_reqs: rc.keyframe_reqs_sent,
        min_target,
        refused,
        final_target: tx.target_kbps(SID),
        kbps: bytes as f64 * 8.0 / 1000.0 / elapsed,
    };
    let t = std::time::Instant::now();
    tx.close();
    rx.close();
    assert!(t.elapsed() < Duration::from_millis(500), "close zu langsam");
    r
}

fn main() {
    let o = parse();
    let mut base_median = None;
    if o.baseline && (o.loss > 0.0 || o.reorder > 0.0) {
        let b = run(&o, 0.0, o.secs.min(5.0));
        base_median = Some(pct(&b.lat_ms, 0.5));
        println!(
            "Basis (0 % Verlust, {} ms Jitter, {:.0} s): Latenz Median {:.2} ms, p99 {:.2} ms",
            o.jitter,
            o.secs.min(5.0),
            base_median.unwrap(),
            pct(&b.lat_ms, 0.99)
        );
    }
    let r = run(&o, o.loss, o.secs);
    let med = pct(&r.lat_ms, 0.5);
    println!(
        "Lauf: {:.1} % Verlust, {} ms Jitter, {} fps, FEC {}, {:.0} s, Nutzlast {:.0} kbit/s",
        o.loss,
        o.jitter,
        o.fps,
        if o.fec { "an" } else { "aus" },
        o.secs,
        r.kbps
    );
    println!("  gesendet:          {} Frames", r.sent);
    println!(
        "  vollstaendig:      {} ({:.2} %)",
        r.complete,
        100.0 * r.complete as f64 / r.sent as f64
    );
    println!(
        "  ausgeliefert:      {} ({:.2} %)   verworfen: {}   Keyframe-Anfragen: {}",
        r.delivered,
        100.0 * r.delivered as f64 / r.sent as f64,
        r.dropped,
        r.keyframe_reqs
    );
    println!(
        "  Latenz (Sendeaufruf bis Callback): Median {:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        med,
        pct(&r.lat_ms, 0.95),
        pct(&r.lat_ms, 0.99),
        pct(&r.lat_ms, 1.0)
    );
    if let Some(b) = base_median {
        println!("  Zusatzlatenz (Median gegen Basis): {:.2} ms", med - b);
    }
    println!(
        "  NACKs: {}  Wiederholungen: {}  FEC gesendet: {}  FEC repariert: {}",
        r.nacks, r.retransmits, r.fec_sent, r.fec_recovered
    );
    if r.refused > 0 {
        println!(
            "  abgelehnt (Sendewarteschlange voll): {} Frames",
            r.refused
        );
    }
    println!(
        "  Zielbitrate: min {} kbit/s, am Ende {} kbit/s (Maximum {})",
        r.min_target, r.final_target, o.max_kbps
    );
}
