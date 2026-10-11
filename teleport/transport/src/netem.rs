//! Kuenstliche Netzstoerung auf der Empfangsseite (Tests und Beispiele): Verlust, Jitter, Umordnung.

use std::collections::{BinaryHeap, HashSet};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct NetemConfig {
    /// Verlust in Prozent (0..100) je eingehendem Datagramm.
    pub loss_pct: f64,
    /// Feste Verzoegerung.
    pub delay_ms: f64,
    /// Zufaellige Zusatzverzoegerung 0..jitter_ms je Datagramm. Ungleiche Werte ordnen Pakete um.
    pub jitter_ms: f64,
    /// Anteil der Pakete, die zusaetzlich um 2 x jitter_ms (mindestens 5 ms) spaeter ankommen.
    pub reorder_pct: f64,
    /// Seed fuer reproduzierbare Laeufe.
    pub seed: u64,
    /// Jeweils die erste Uebertragung dieser seq wird verworfen (deterministische Tests).
    pub drop_once: HashSet<u64>,
    /// Verwirft die erste Uebertragung jedes n-ten Video-Pakets (0 = aus). Deterministisch, unabhaengig
    /// von zwischengeschobenen ping/pong-Paketen.
    pub drop_video_every: u64,
}

impl NetemConfig {
    pub fn is_off(&self) -> bool {
        self.loss_pct <= 0.0
            && self.delay_ms <= 0.0
            && self.jitter_ms <= 0.0
            && self.reorder_pct <= 0.0
            && self.drop_once.is_empty()
            && self.drop_video_every == 0
    }
}

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Gleichverteilt in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct Delayed {
    due: Instant,
    order: u64,
    data: Vec<u8>,
    src: std::net::SocketAddr,
}

impl PartialEq for Delayed {
    fn eq(&self, o: &Self) -> bool {
        self.due == o.due && self.order == o.order
    }
}
impl Eq for Delayed {}
impl PartialOrd for Delayed {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Delayed {
    // BinaryHeap ist ein Max-Heap, wir wollen das fruehste zuerst.
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.due.cmp(&self.due).then(o.order.cmp(&self.order))
    }
}

#[derive(Default)]
pub struct Netem {
    cfg: NetemConfig,
    rng: Option<Rng>,
    heap: BinaryHeap<Delayed>,
    counter: u64,
    video_seen: u64,
    video_top: u64,
}

pub enum Verdict {
    /// Sofort verarbeiten.
    Now,
    /// Verworfen oder in die Warteschlange gelegt.
    Held,
}

impl Netem {
    pub fn set(&mut self, cfg: NetemConfig) {
        self.rng = Some(Rng::new(cfg.seed));
        self.cfg = cfg;
        // Bereits wartende Pakete laufen normal ab.
    }

    pub fn active(&self) -> bool {
        !self.cfg.is_off() || !self.heap.is_empty()
    }

    pub fn queued(&self) -> usize {
        self.heap.len()
    }

    /// `seq` ist die (unauthentifizierte) Sequenznummer aus dem Header, nur fuer `drop_once`.
    pub fn ingest(
        &mut self,
        now: Instant,
        data: &[u8],
        src: std::net::SocketAddr,
        seq: u64,
    ) -> Verdict {
        if self.cfg.is_off() {
            return Verdict::Now;
        }
        let rng = self.rng.get_or_insert_with(|| Rng::new(0));
        if self.cfg.drop_once.remove(&seq) {
            return Verdict::Held;
        }
        if self.cfg.drop_video_every > 0 && data.get(1) == Some(&1) && seq > self.video_top {
            self.video_top = seq;
            self.video_seen += 1;
            if self.video_seen.is_multiple_of(self.cfg.drop_video_every) {
                return Verdict::Held;
            }
        }
        if self.cfg.loss_pct > 0.0 && rng.next_f64() * 100.0 < self.cfg.loss_pct {
            return Verdict::Held;
        }
        let mut ms = self.cfg.delay_ms.max(0.0);
        if self.cfg.jitter_ms > 0.0 {
            ms += rng.next_f64() * self.cfg.jitter_ms;
        }
        if self.cfg.reorder_pct > 0.0 && rng.next_f64() * 100.0 < self.cfg.reorder_pct {
            ms += (2.0 * self.cfg.jitter_ms).max(5.0);
        }
        if ms <= 0.0 && self.heap.is_empty() {
            return Verdict::Now;
        }
        self.counter += 1;
        self.heap.push(Delayed {
            due: now + Duration::from_secs_f64(ms / 1000.0),
            order: self.counter,
            data: data.to_vec(),
            src,
        });
        Verdict::Held
    }

    pub fn pop_due(&mut self, now: Instant) -> Option<(Vec<u8>, std::net::SocketAddr)> {
        if self.heap.peek().is_some_and(|d| d.due <= now) {
            let d = self.heap.pop()?;
            return Some((d.data, d.src));
        }
        None
    }
}
