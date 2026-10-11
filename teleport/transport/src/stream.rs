//! Zustand eines Streams (eine `stream_id`) in einer Session: Senden mit Puffer/FEC/Bitratenregelung,
//! Empfangen mit Zusammensetzen, NACK, FEC-Reparatur, Reihenfolge und Rueckmeldungen.
//!
//! Alles hier ist rein synchron und kennt keinen Socket: Datagramme zum Senden landen in `Io::out`,
//! Ereignisse fuer den Aufrufer in `Io::events`. Dadurch laesst sich die Logik ohne Netz testen.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use crate::packet::*;
use crate::replay::ReplayWindow;

pub const RETX_WINDOW: Duration = Duration::from_millis(300);
pub const KEYFRAME_REQ_MIN_GAP: Duration = Duration::from_millis(300);
pub const FEC_GROUP: usize = 8;
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAGS: usize = MAX_FRAME_BYTES / MAX_FRAG_PAYLOAD + 1;
const MAX_OPEN_FRAMES: usize = 64;
const MAX_RETX_ENTRIES: usize = 40_000;
const MAX_NACK_TRIES: u8 = 8;
const NACK_PER_PACKET: usize = (MAX_PLAINTEXT - 2) / 8;
/// Hoechstalter eines unvollstaendigen Frames, egal wie lange noch Pakete tropfen.
/// Ab dieser Wartezeit zaehlt ein noch fehlendes Paket vorlaeufig als verloren (Rohverlust/Restverlust).
const PROVISIONAL_LOSS: Duration = Duration::from_millis(50);
const ASM_MAX_AGE: Duration = Duration::from_millis(250);
/// Pacing des Senders: Token-Bucket mit 1,5 x Zielbitrate, Burst hoechstens 64 KB.
const PACE_BURST: f64 = 64.0 * 1024.0;
const PACE_FACTOR: f64 = 1.5;
/// Was die Sendewarteschlange hoechstens an Wartezeit enthalten darf (bei aktueller Bucket-Rate). Ein
/// neues Frame wird angenommen, solange die Warteschlange davor darunter liegt, egal wie gross es ist.
const MAX_PACE_QUEUE_S: f64 = 0.15;
/// Restverlust (nach FEC/NACK) ab dem gedrosselt wird, und Rohverlust als Notbremse (je in Promille).
const RESIDUAL_LOSS_LIMIT: u16 = 10;
const RAW_LOSS_EMERGENCY: u16 = 80;
const PING_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Sender,
    Receiver,
}

/// Angaben der Session, die in die Statistik eines Streams wandern.
#[derive(Clone, Debug, Default)]
pub struct TickInfo {
    pub auth_fail: u64,
    /// Aktuelle Gegenstelle als `ip:port` (leer = unbekannt).
    pub peer: String,
    /// Tatsaechliche Socket-Puffergroessen nach set_recv/send_buffer_size.
    pub rcvbuf: u32,
    pub sndbuf: u32,
}

/// Was die Session nach einem Aufruf erledigen muss.
#[derive(Default)]
pub struct Io {
    pub out: Vec<Vec<u8>>,
    pub events: Vec<Event>,
}

pub enum Event {
    Frame {
        stream: u32,
        kind: u8,
        data: Vec<u8>,
        pts_us: u64,
        fflags: u8,
        frame_id: u32,
    },
    Control {
        stream: u32,
        ptype: u8,
    },
    Stats {
        stream: u32,
        json: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RxResult {
    Ok,
    AuthFail,
    Replay,
    Malformed,
}

#[derive(Clone, Debug, Default)]
pub struct Counters {
    pub packets_sent: u64,
    pub packets_recv: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub frames_sent: u64,
    pub frames_complete: u64,
    pub frames_delivered: u64,
    pub frames_dropped: u64,
    /// Aufschluesselung von `frames_dropped` (Summe = frames_dropped):
    /// Video-Frame nach der Frist (2 x RTT bzw. 250 ms Hoechstalter) noch unvollstaendig.
    pub dropped_incomplete: u64,
    /// Zu viele offene bzw. fertige, aber blockierte Frames (MAX_OPEN_FRAMES).
    pub dropped_overflow: u64,
    /// Fertige Frames, die nach einer Luecke bis zum naechsten Keyframe nicht ausgeliefert wurden.
    pub dropped_gap: u64,
    /// Audio-Frames (Frist oder Ueberlauf).
    pub dropped_audio: u64,
    /// Pakete fuer Frames, die schon verworfen bzw. uebersprungen waren (kamen zu spaet).
    pub late_packets: u64,
    /// Wie oft der Empfang auf "bis zum Keyframe warten" ging: Frame unvollstaendig abgelaufen ...
    pub gaps_incomplete: u64,
    /// ... bzw. ein Frame kam innerhalb der Frist gar nicht (Reihenfolge blockiert).
    pub gaps_blocked: u64,
    pub retransmits: u64,
    pub nacks_sent: u64,
    pub nack_seqs_sent: u64,
    pub fec_sent: u64,
    pub fec_recovered: u64,
    pub keyframe_reqs_sent: u64,
    pub keyframe_reqs_recv: u64,
    pub replay_dropped: u64,
    pub malformed: u64,
}

struct RetxEntry {
    at: Instant,
    last_retx: Option<Instant>,
    datagram: Option<Vec<u8>>,
}

struct Miss {
    first: Instant,
    last_nack: Option<Instant>,
    tries: u8,
    /// Der Frame ist ohne dieses Paket fertig geworden (FEC): nicht mehr nachfordern, aber weiter
    /// als Verlust bzw. spaete Ankunft verbuchen.
    settled: bool,
    /// Schon als roh verloren bzw. als Restverlust verbucht (nicht doppelt zaehlen).
    lost_counted: bool,
    resid_counted: bool,
}

struct FecPkt {
    group_start: usize,
    group_len: usize,
    len_xor: u16,
    parity: Vec<u8>,
}

struct Asm {
    cnt: usize,
    frags: Vec<Option<Vec<u8>>>,
    got: usize,
    first: Instant,
    /// Ankunft des letzten Pakets dieses Frames; die Frist laeuft ab hier, damit grosse Frames
    /// auf langsamen Leitungen nicht mitten im Empfang verworfen werden.
    last: Instant,
    pts_us: u64,
    fflags: u8,
    fec: Vec<FecPkt>,
    /// seq des jeweils empfangenen Fragments (0 = noch nicht da), fuer das Aufraeumen der NACK-Liste.
    seqs: Vec<u64>,
}

impl Asm {
    fn stale(&self, now: Instant, deadline: Duration) -> bool {
        now.saturating_duration_since(self.last) > deadline
            || now.saturating_duration_since(self.first) > ASM_MAX_AGE
    }
}

struct Done {
    data: Vec<u8>,
    pts_us: u64,
    fflags: u8,
    first: Instant,
}

#[derive(Default)]
struct Window {
    bytes_sent: u64,
    bytes_recv: u64,
    frames: u64,
    hold_sum_ms: f64,
    hold_n: u64,
    lost: u64,
    /// Verloren und weder per NACK noch per FEC zurueckgekommen.
    resid: u64,
    hi_at_start: Option<u64>,
    /// Rohverlust und Restverlust des letzten Feedbacks (Promille).
    last_loss_permille: u16,
    last_resid_permille: u16,
}

pub struct Stream {
    pub id: u32,
    pub role: Role,
    tx: Cipher,
    tx_seq: u64,
    rx: Cipher,
    replay: ReplayWindow,
    max_kbps: u32,
    pub fec_enabled: bool,
    /// Untergrenze der Frist fuer unvollstaendige Frames in ms (Vertrag: 30; hoeher nur fuer Tests).
    pub min_deadline_ms: f64,
    t0: Instant,
    pub c: Counters,
    win: Window,
    last_stats: Instant,

    // Sender
    next_vid: u32,
    next_aid: u32,
    retx: VecDeque<RetxEntry>,
    retx_first: u64,
    target_kbps: f64,
    last_adjust: Instant,
    prev_jitter_us: u32,
    jitter_rises: u8,
    fb_count: u32,
    fb_loss_permille: u16,
    fb_raw_permille: u16,
    fb_recv_kbps: u32,
    pace_q: VecDeque<Vec<u8>>,
    pace_bytes: usize,
    tokens: f64,
    last_refill: Instant,
    /// Das zuletzt verarbeitete Paket hatte die hoechste bisher gesehene seq (fuer Roaming).
    pub last_new_max: bool,

    // Empfaenger
    hi: Option<u64>,
    missing: BTreeMap<u64, Miss>,
    video: BTreeMap<u32, Asm>,
    audio: BTreeMap<u32, Asm>,
    completed: BTreeMap<u32, Done>,
    floor: Option<u32>,
    /// Zuletzt verworfene Video-frame_ids (nur fuer den Zaehler `late_packets`).
    dropped_ids: std::collections::VecDeque<u32>,
    broken: bool,
    broken_since: Option<Instant>,
    blocked_since: Option<Instant>,
    last_kf_req: Option<Instant>,
    saw_traffic: bool,
    srtt_ms: Option<f64>,
    last_ping: Option<Instant>,
    last_feedback: Option<Instant>,
    first_media: Option<Instant>,
    jitter_us: f64,
    /// Wie lange ein spaeter, aber unversehrt eingetroffenes Paket hinterherhinkt (Umordnung), in ms.
    reorder_ms: f64,
    /// (Zeit, erwartete Pakete, roh verloren, Restverlust) je Feedback-Fenster der letzten Sekunde.
    loss_hist: VecDeque<(Instant, u64, u64, u64)>,
    /// Nach einem NACK eingetroffene seqs (als verloren gebucht), um ein spaeteres Duplikat zu erkennen.
    late_nacked: BTreeMap<u64, Instant>,
    last_arrival: Option<(Instant, u64)>,
    last_frame_id: u32,
}

fn id_ge(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) >= 0
}

impl Stream {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u32,
        role: Role,
        s2r_key: &[u8; 32],
        s2r_prefix: &[u8; 4],
        r2s_key: &[u8; 32],
        r2s_prefix: &[u8; 4],
        max_kbps: u32,
        now: Instant,
    ) -> Self {
        let (tx, rx) = match role {
            Role::Sender => (
                Cipher::new(s2r_key, s2r_prefix),
                Cipher::new(r2s_key, r2s_prefix),
            ),
            Role::Receiver => (
                Cipher::new(r2s_key, r2s_prefix),
                Cipher::new(s2r_key, s2r_prefix),
            ),
        };
        let max_kbps = max_kbps.max(1);
        Stream {
            id,
            role,
            tx,
            tx_seq: 0,
            rx,
            replay: ReplayWindow::new(),
            max_kbps,
            fec_enabled: true,
            min_deadline_ms: 30.0,
            t0: now,
            c: Counters::default(),
            win: Window::default(),
            last_stats: now,
            next_vid: 1,
            next_aid: 1,
            retx: VecDeque::new(),
            retx_first: 1,
            target_kbps: max_kbps as f64,
            last_adjust: now,
            prev_jitter_us: 0,
            jitter_rises: 0,
            fb_count: 0,
            fb_loss_permille: 0,
            fb_raw_permille: 0,
            fb_recv_kbps: 0,
            pace_q: VecDeque::new(),
            pace_bytes: 0,
            tokens: PACE_BURST,
            last_refill: now,
            last_new_max: false,
            hi: None,
            missing: BTreeMap::new(),
            video: BTreeMap::new(),
            audio: BTreeMap::new(),
            completed: BTreeMap::new(),
            floor: None,
            dropped_ids: std::collections::VecDeque::new(),
            broken: true,
            broken_since: None,
            blocked_since: None,
            last_kf_req: None,
            saw_traffic: false,
            srtt_ms: None,
            last_ping: None,
            last_feedback: None,
            first_media: None,
            jitter_us: 0.0,
            reorder_ms: 0.0,
            loss_hist: VecDeque::new(),
            late_nacked: BTreeMap::new(),
            last_arrival: None,
            last_frame_id: 0,
        }
    }

    pub fn target_kbps(&self) -> u32 {
        match self.role {
            Role::Sender => self.target_kbps.round() as u32,
            Role::Receiver => self.max_kbps,
        }
    }

    pub fn needs_fast_tick(&self) -> bool {
        !self.missing.is_empty()
            || !self.video.is_empty()
            || !self.completed.is_empty()
            || !self.pace_q.is_empty()
    }

    /// Neue Obergrenze (z. B. nach Pfadwechsel). Erhoehung: Zielbitrate springt auf mindestens 50 %
    /// der neuen Obergrenze. Senkung: Zielbitrate hoechstens die neue Obergrenze.
    pub fn set_max_kbps(&mut self, max_kbps: u32) {
        let new = max_kbps.max(1);
        let old = self.max_kbps;
        self.max_kbps = new;
        if new > old {
            self.target_kbps = self.target_kbps.max(new as f64 * 0.5);
        }
        self.target_kbps = self.target_kbps.min(new as f64);
    }

    pub fn max_kbps(&self) -> u32 {
        self.max_kbps
    }

    /// Ein ping jetzt (nach Wechsel der Gegenstelle).
    pub fn ping_now(&mut self, now: Instant, io: &mut Io) {
        self.last_ping = Some(now);
        let t = self.now_us(now).to_be_bytes();
        self.packet(now, T_PING, &t, false, io);
    }

    /// Gibt wartende Sender-Pakete im Takt des Token-Buckets frei.
    pub fn pace(&mut self, now: Instant, io: &mut Io) {
        if self.role != Role::Sender {
            return;
        }
        let dt = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        self.last_refill = now;
        let rate = self.target_kbps * PACE_FACTOR * 125.0; // Byte je Sekunde
        self.tokens = (self.tokens + dt * rate).min(PACE_BURST);
        while let Some(front) = self.pace_q.front() {
            if self.tokens < front.len() as f64 {
                break;
            }
            let d = self.pace_q.pop_front().unwrap();
            self.tokens -= d.len() as f64;
            self.pace_bytes -= d.len();
            io.out.push(d);
        }
    }

    fn pace_rate(&self) -> f64 {
        self.target_kbps * PACE_FACTOR * 125.0 // Byte je Sekunde
    }

    fn pace_queue_limit(&self) -> usize {
        (self.pace_rate() * MAX_PACE_QUEUE_S) as usize
    }

    /// Wartezeit der Sendewarteschlange in ms bei aktueller Bucket-Rate.
    fn pace_queue_ms(&self) -> f64 {
        self.pace_bytes as f64 / self.pace_rate().max(1.0) * 1000.0
    }

    fn emit(&mut self, d: Vec<u8>, io: &mut Io) {
        if self.role == Role::Sender {
            self.pace_bytes += d.len();
            self.pace_q.push_back(d);
        } else {
            io.out.push(d);
        }
    }

    fn deadline(&self) -> Duration {
        let rtt = self.srtt_ms.unwrap_or(0.0);
        Duration::from_secs_f64((2.0 * rtt).clamp(self.min_deadline_ms.min(250.0), 250.0) / 1000.0)
    }

    fn now_us(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.t0).as_micros() as u64
    }

    // ---------------------------------------------------------------- Senden

    /// Baut und merkt sich ein Datagramm. `keep` = fuer erneutes Senden aufbewahren.
    fn packet(
        &mut self,
        now: Instant,
        ptype: u8,
        plain: &[u8],
        keep: bool,
        io: &mut Io,
    ) -> Option<()> {
        let seq = self.tx_seq + 1;
        let d = self.tx.seal(
            &Header {
                ptype,
                flags: 0,
                stream_id: self.id,
                seq,
            },
            plain,
        )?;
        self.tx_seq = seq;
        self.c.packets_sent += 1;
        self.c.bytes_sent += d.len() as u64;
        self.win.bytes_sent += d.len() as u64;
        while let Some(f) = self.retx.front() {
            if now.saturating_duration_since(f.at) > RETX_WINDOW
                || self.retx.len() > MAX_RETX_ENTRIES
            {
                self.retx.pop_front();
                self.retx_first += 1;
            } else {
                break;
            }
        }
        self.retx.push_back(RetxEntry {
            at: now,
            last_retx: None,
            datagram: if keep { Some(d.clone()) } else { None },
        });
        self.emit(d, io);
        Some(())
    }

    pub fn send_frame(
        &mut self,
        now: Instant,
        kind: u8,
        data: &[u8],
        pts_us: u64,
        fflags: u8,
        io: &mut Io,
    ) -> Result<(), String> {
        if self.role != Role::Sender {
            return Err("bt_send_frame nur fuer Sender-Streams".into());
        }
        if kind != 1 && kind != 2 {
            return Err("kind muss 1 (video) oder 2 (audio) sein".into());
        }
        if data.is_empty() || data.len() > MAX_FRAME_BYTES {
            return Err("Frame leer oder groesser als 16 MiB".into());
        }
        if self.pace_bytes > self.pace_queue_limit() {
            return Err("Sendewarteschlange voll (Bitrate zu hoch fuer die Leitung)".into());
        }
        let ptype = if kind == 1 { T_VIDEO } else { T_AUDIO };
        let frame_id = if kind == 1 {
            let v = self.next_vid;
            self.next_vid = self.next_vid.wrapping_add(1);
            v
        } else {
            let v = self.next_aid;
            self.next_aid = self.next_aid.wrapping_add(1);
            v
        };
        let cnt = data.len().div_ceil(MAX_FRAG_PAYLOAD);
        let use_fec = self.fec_enabled && kind == 1;
        let mut group: Vec<Vec<u8>> = Vec::new();
        for (i, chunk) in data.chunks(MAX_FRAG_PAYLOAD).enumerate() {
            let plain = encode_media(
                &MediaHead {
                    frame_id,
                    frag_idx: i as u16,
                    frag_cnt: cnt as u16,
                    pts_us,
                    fflags,
                },
                chunk,
            );
            self.packet(now, ptype, &plain, true, io)
                .ok_or("Paket zu gross")?;
            if use_fec {
                group.push(plain);
                if group.len() == FEC_GROUP || i + 1 == cnt {
                    self.send_parity(now, frame_id, i + 1 - group.len(), &group, io)?;
                    group.clear();
                }
            }
        }
        self.c.frames_sent += 1;
        self.win.frames += 1;
        self.pace(now, io);
        Ok(())
    }

    fn send_parity(
        &mut self,
        now: Instant,
        frame_id: u32,
        group_start: usize,
        group: &[Vec<u8>],
        io: &mut Io,
    ) -> Result<(), String> {
        let maxlen = group.iter().map(|p| p.len()).max().unwrap_or(0);
        let mut parity = vec![0u8; maxlen];
        let mut len_xor = 0u16;
        for p in group {
            for (a, b) in parity.iter_mut().zip(p) {
                *a ^= b;
            }
            len_xor ^= p.len() as u16;
        }
        let plain = encode_fec(
            &FecHead {
                frame_id,
                group_start: group_start as u16,
                group_len: group.len() as u16,
                len_xor,
            },
            &parity,
        );
        self.packet(now, T_FEC, &plain, true, io)
            .ok_or("FEC-Paket zu gross")?;
        self.c.fec_sent += 1;
        Ok(())
    }

    fn retransmit(&mut self, now: Instant, seq: u64, io: &mut Io) {
        if seq < self.retx_first {
            return;
        }
        let Some(e) = self.retx.get_mut((seq - self.retx_first) as usize) else {
            return;
        };
        if now.saturating_duration_since(e.at) > RETX_WINDOW {
            return;
        }
        if e.last_retx
            .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(4))
        {
            return;
        }
        if let Some(d) = e.datagram.clone() {
            e.last_retx = Some(now);
            if self.pace_bytes > self.pace_queue_limit() {
                return;
            }
            self.c.retransmits += 1;
            self.c.packets_sent += 1;
            self.c.bytes_sent += d.len() as u64;
            self.win.bytes_sent += d.len() as u64;
            self.emit(d, io);
        }
    }

    fn on_feedback(&mut self, now: Instant, fb: &[u8]) {
        if fb.len() < 4 + 2 + 4 + 4 {
            return;
        }
        let recv_kbps = u32::from_be_bytes(fb[0..4].try_into().unwrap());
        // loss_permille = Restverlust nach NACK/FEC; ein optionales Feld am Ende traegt den Rohverlust.
        let loss = u16::from_be_bytes(fb[4..6].try_into().unwrap());
        let raw = if fb.len() >= 16 {
            u16::from_be_bytes(fb[14..16].try_into().unwrap())
        } else {
            loss
        };
        self.fb_raw_permille = raw;
        let jitter = u32::from_be_bytes(fb[6..10].try_into().unwrap());
        self.fb_loss_permille = loss;
        self.fb_recv_kbps = recv_kbps;
        let dt = now
            .saturating_duration_since(self.last_adjust)
            .as_secs_f64()
            .clamp(0.05, 2.0);
        self.last_adjust = now;
        let prev = self.prev_jitter_us;
        self.prev_jitter_us = jitter;
        // Der Jitter-Schaetzer schwankt von Sekunde zu Sekunde; erst zwei Anstiege in Folge um je 15 %
        // (und mindestens 2 ms) gelten als steigend.
        if prev > 0 && jitter as f64 > prev as f64 * 1.15 && jitter > prev.saturating_add(2000) {
            self.jitter_rises = self.jitter_rises.saturating_add(1);
        } else {
            self.jitter_rises = 0;
        }
        // Der Schaetzer laeuft sich in den ersten Sekunden erst ein (Anstieg von 0): da noch nicht werten.
        self.fb_count = self.fb_count.saturating_add(1);
        let jitter_rising = self.jitter_rises >= 2 && self.fb_count > 6;
        // 4.1 (v1.1): bei Restverlust > 1 %, Rohverlust > 8 % (Notbremse) oder steigendem Jitter
        // 15 % je Sekunde runter, sonst 5 % je Sekunde rauf.
        if loss > RESIDUAL_LOSS_LIMIT || raw > RAW_LOSS_EMERGENCY || jitter_rising {
            self.target_kbps *= 0.85f64.powf(dt);
        } else {
            self.target_kbps *= 1.05f64.powf(dt);
        }
        let floor = (self.max_kbps as f64 / 20.0)
            .max(200.0)
            .min(self.max_kbps as f64);
        self.target_kbps = self.target_kbps.clamp(floor, self.max_kbps as f64);
    }

    // ---------------------------------------------------------------- Empfang

    /// Verarbeitet ein Datagramm, dessen Header schon als `h` gelesen wurde.
    pub fn receive(&mut self, now: Instant, d: &[u8], h: &Header, io: &mut Io) -> RxResult {
        let allowed = match self.role {
            Role::Sender => matches!(
                h.ptype,
                T_NACK | T_KEYFRAME_REQ | T_FEEDBACK | T_PING | T_PONG
            ),
            Role::Receiver => matches!(h.ptype, T_VIDEO | T_AUDIO | T_FEC | T_PONG | T_PING),
        };
        if !allowed {
            self.c.malformed += 1;
            return RxResult::Malformed;
        }
        if !self.replay.check(h.seq) {
            // Zu alt oder doppelt. Vor der Authentifizierung nur zaehlen, nichts aendern; nur ein
            // authentifiziertes Duplikat einer nach NACK eingetroffenen seq nimmt die Verlustbuchung zurueck.
            self.c.replay_dropped += 1;
            if self.late_nacked.contains_key(&h.seq) && self.rx.open(d, h).is_some() {
                self.late_nacked.remove(&h.seq);
                self.win.lost = self.win.lost.saturating_sub(1);
            }
            return RxResult::Replay;
        }
        let Some(plain) = self.rx.open(d, h) else {
            return RxResult::AuthFail;
        };
        self.last_new_max = h.seq > self.replay.top();
        self.replay.commit(h.seq);
        self.c.packets_recv += 1;
        self.c.bytes_recv += d.len() as u64;
        self.win.bytes_recv += d.len() as u64;
        self.saw_traffic = true;
        if self.role == Role::Receiver {
            self.note_seq(now, h.seq);
        }
        let ok = match h.ptype {
            T_VIDEO | T_AUDIO => self.on_media(now, h, plain, io),
            T_FEC => self.on_fec(now, h, plain, io),
            T_NACK => {
                self.on_nack(now, &plain, io);
                true
            }
            T_KEYFRAME_REQ => {
                self.c.keyframe_reqs_recv += 1;
                io.events.push(Event::Control {
                    stream: self.id,
                    ptype: T_KEYFRAME_REQ,
                });
                true
            }
            T_FEEDBACK => {
                self.on_feedback(now, &plain);
                true
            }
            T_PING => {
                if plain.len() >= 8 {
                    let t = plain[..8].to_vec();
                    self.packet(now, T_PONG, &t, false, io);
                    true
                } else {
                    false
                }
            }
            T_PONG if plain.len() >= 8 => {
                let t = u64::from_be_bytes(plain[..8].try_into().unwrap());
                let now_us = self.now_us(now);
                if t <= now_us {
                    let s = (now_us - t) as f64 / 1000.0;
                    self.srtt_ms = Some(match self.srtt_ms {
                        Some(p) => p * 0.875 + s * 0.125,
                        None => s,
                    });
                }
                true
            }
            _ => false,
        };
        if !ok {
            self.c.malformed += 1;
            return RxResult::Malformed;
        }
        self.pace(now, io);
        RxResult::Ok
    }

    fn on_nack(&mut self, now: Instant, p: &[u8], io: &mut Io) {
        if p.len() < 2 {
            return;
        }
        let n = u16::from_be_bytes([p[0], p[1]]) as usize;
        for i in 0..n.min((p.len() - 2) / 8) {
            let seq = u64::from_be_bytes(p[2 + i * 8..10 + i * 8].try_into().unwrap());
            self.retransmit(now, seq, io);
        }
        self.pace(now, io);
    }

    /// Luecken in der Sequenz merken (fuer NACK).
    fn note_seq(&mut self, now: Instant, seq: u64) {
        match self.hi {
            None => {
                // Beginn des Stroms: fehlen die allerersten Pakete, trotzdem nachfordern.
                if seq <= 64 {
                    for s in 1..seq {
                        self.add_missing(now, s);
                    }
                }
                self.hi = Some(seq);
                self.win.hi_at_start = Some(seq.saturating_sub(1));
            }
            Some(hi) if seq > hi => {
                if seq - hi > 2048 {
                    self.missing.clear();
                } else {
                    for s in hi + 1..seq {
                        self.add_missing(now, s);
                    }
                }
                self.hi = Some(seq);
            }
            Some(_) => {
                // Verspaetet eingetroffen. Ohne vorherigen NACK war es reine Umordnung: daraus lernen.
                // Roh verloren zaehlt eine seq nur, wenn das Original nicht mehr kommt: sie laeuft ab, oder
                // sie trifft nach einem NACK ein, und zwar so spaet, dass es die Wiederholung sein kann
                // (frueher als eine halbe RTT nach dem NACK kann nur das verspaetete Original sein).
                if let Some(m) = self.missing.remove(&seq) {
                    match m.last_nack {
                        None => {
                            let late =
                                now.saturating_duration_since(m.first).as_secs_f64() * 1000.0;
                            self.reorder_ms = late.max(self.reorder_ms * 0.98);
                        }
                        Some(t) => {
                            let since_nack =
                                now.saturating_duration_since(t).as_secs_f64() * 1000.0;
                            if since_nack >= self.srtt_ms.unwrap_or(0.0) / 2.0 {
                                // Vorlaeufig verloren. Taucht die Wiederholung spaeter als Duplikat auf,
                                // war es das verspaetete Original und die Buchung wird zurueckgenommen.
                                if !m.lost_counted {
                                    self.win.lost += 1;
                                }
                                self.late_nacked.insert(seq, now);
                            } else {
                                // Das Original war nur spaeter als die NACK-Verzoegerung: die naechsten
                                // Male laenger warten.
                                let late =
                                    now.saturating_duration_since(m.first).as_secs_f64() * 1000.0;
                                self.reorder_ms = late.max(self.reorder_ms * 0.98);
                            }
                        }
                    }
                }
            }
        }
    }

    fn add_missing(&mut self, now: Instant, seq: u64) {
        self.missing.insert(
            seq,
            Miss {
                first: now,
                last_nack: None,
                tries: 0,
                settled: false,
                lost_counted: false,
                resid_counted: false,
            },
        );
    }

    fn accepts(&self, frame_id: u32) -> bool {
        match self.floor {
            Some(f) => id_ge(frame_id, f) && frame_id.wrapping_sub(f) < 1_000_000,
            None => true,
        }
    }

    fn on_media(&mut self, now: Instant, h: &Header, plain: Vec<u8>, io: &mut Io) -> bool {
        let Some(m) = decode_media(&plain) else {
            return false;
        };
        let cnt = m.frag_cnt as usize;
        let idx = m.frag_idx as usize;
        if cnt == 0 || cnt > MAX_FRAGS || idx >= cnt {
            return false;
        }
        let video = h.ptype == T_VIDEO;
        if video {
            self.first_media.get_or_insert(now);
            if !self.accepts(m.frame_id) || self.completed.contains_key(&m.frame_id) {
                if self.dropped_ids.contains(&m.frame_id) {
                    self.c.late_packets += 1;
                }
                return true; // spaeter Nachzuegler, unauffaellig
            }
            if self.broken_since.is_none() && self.broken {
                self.broken_since = Some(now);
            }
        }
        let map = if video {
            &mut self.video
        } else {
            &mut self.audio
        };
        if !map.contains_key(&m.frame_id) && map.len() >= MAX_OPEN_FRAMES {
            let oldest = *map.keys().next().unwrap();
            map.remove(&oldest);
            self.c.frames_dropped += 1;
            if video {
                self.c.dropped_overflow += 1;
                // map leiht self.video; dropped_ids direkt (disjunktes Feld) statt note_dropped.
                if self.dropped_ids.len() >= 64 {
                    self.dropped_ids.pop_front();
                }
                self.dropped_ids.push_back(oldest);
            } else {
                self.c.dropped_audio += 1;
            }
        }
        let new_frame = map.get(&m.frame_id).is_none_or(|a| a.cnt == 0);
        let a = map.entry(m.frame_id).or_insert_with(|| Asm {
            cnt: 0,
            frags: Vec::new(),
            got: 0,
            first: now,
            last: now,
            pts_us: m.pts_us,
            fflags: m.fflags,
            fec: Vec::new(),
            seqs: Vec::new(),
        });
        a.last = now;
        if a.cnt == 0 {
            a.cnt = cnt;
            a.frags = vec![None; cnt];
            a.seqs = vec![0; cnt];
        } else if a.cnt != cnt {
            return false;
        }
        if a.frags[idx].is_none() {
            a.seqs[idx] = h.seq;
            a.frags[idx] = Some(plain);
            a.got += 1;
            a.pts_us = m.pts_us;
            a.fflags = m.fflags;
        }
        if video && new_frame {
            self.update_jitter(now, m.pts_us);
        }
        self.last_frame_id = if video {
            m.frame_id
        } else {
            self.last_frame_id
        };
        self.try_finish(now, m.frame_id, video, io);
        true
    }

    fn update_jitter(&mut self, now: Instant, pts_us: u64) {
        if let Some((t, p)) = self.last_arrival {
            let da = now.saturating_duration_since(t).as_micros() as i64;
            let dp = pts_us.wrapping_sub(p) as i64;
            let d = (da - dp).unsigned_abs().min(100_000) as f64;
            self.jitter_us += (d - self.jitter_us) / 16.0;
        }
        self.last_arrival = Some((now, pts_us));
    }

    fn on_fec(&mut self, now: Instant, _h: &Header, plain: Vec<u8>, io: &mut Io) -> bool {
        let Some((fh, parity)) = decode_fec(&plain) else {
            return false;
        };
        if fh.group_len == 0 || fh.group_len as usize > 255 {
            return false;
        }
        if !self.accepts(fh.frame_id) || self.completed.contains_key(&fh.frame_id) {
            if self.dropped_ids.contains(&fh.frame_id) {
                self.c.late_packets += 1;
            }
            return true;
        }
        if !self.video.contains_key(&fh.frame_id) && self.video.len() >= MAX_OPEN_FRAMES {
            let oldest = *self.video.keys().next().unwrap();
            self.video.remove(&oldest);
            self.c.frames_dropped += 1;
            self.c.dropped_overflow += 1;
            self.note_dropped(oldest);
        }
        let a = self.video.entry(fh.frame_id).or_insert_with(|| Asm {
            cnt: 0,
            frags: Vec::new(),
            got: 0,
            first: now,
            last: now,
            pts_us: 0,
            fflags: 0,
            fec: Vec::new(),
            seqs: Vec::new(),
        });
        a.last = now;
        a.fec.push(FecPkt {
            group_start: fh.group_start as usize,
            group_len: fh.group_len as usize,
            len_xor: fh.len_xor,
            parity: parity.to_vec(),
        });
        self.try_finish(now, fh.frame_id, true, io);
        true
    }

    /// FEC-Reparatur versuchen und, wenn vollstaendig, den Frame abliefern.
    fn try_finish(&mut self, now: Instant, frame_id: u32, video: bool, io: &mut Io) {
        let map = if video {
            &mut self.video
        } else {
            &mut self.audio
        };
        let Some(a) = map.get_mut(&frame_id) else {
            return;
        };
        if a.cnt == 0 {
            return;
        }
        let mut recovered = 0u64;
        loop {
            let mut progress = false;
            for fi in 0..a.fec.len() {
                if let Some((idx, plain)) = recover(a, fi, frame_id) {
                    a.frags[idx] = Some(plain);
                    a.got += 1;
                    recovered += 1;
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        self.c.fec_recovered += recovered;
        if a.got < a.cnt {
            return;
        }
        let Some(a) = map.remove(&frame_id) else {
            return;
        };
        let mut data = Vec::with_capacity(a.cnt * MAX_FRAG_PAYLOAD);
        for f in a.frags.iter().flatten() {
            data.extend_from_slice(&f[MEDIA_HEAD..]);
        }
        self.c.frames_complete += 1;
        // Seqs, die zu diesem Frame gehoerten und per FEC ersetzt wurden, nicht mehr nachfordern.
        let fec = !a.fec.is_empty();
        let pos = |i: usize| if fec { i + i / FEC_GROUP } else { i };
        if let Some(j) = a.seqs.iter().position(|s| *s != 0) {
            // Die Pakete eines Frames haben fortlaufende seqs (Fragmente, je Gruppe gefolgt von der Paritaet).
            let base = a.seqs[j].saturating_sub(pos(j) as u64);
            let total = if fec {
                a.cnt + a.cnt.div_ceil(FEC_GROUP)
            } else {
                a.cnt
            };
            let hi = base + total as u64 - 1;
            for (_, m) in self.missing.range_mut(base..=hi) {
                m.settled = true;
            }
        }
        let done = Done {
            data,
            pts_us: a.pts_us,
            fflags: a.fflags,
            first: a.first,
        };
        if video {
            self.completed.insert(frame_id, done);
            while self.completed.len() > MAX_OPEN_FRAMES {
                let k = *self.completed.keys().next().unwrap();
                self.completed.remove(&k);
                self.c.frames_dropped += 1;
                self.c.dropped_overflow += 1;
                self.note_dropped(k);
            }
            self.drain(now, io);
        } else {
            self.deliver(now, 2, frame_id, done, io);
        }
    }

    fn deliver(&mut self, now: Instant, kind: u8, frame_id: u32, d: Done, io: &mut Io) {
        self.c.frames_delivered += 1;
        self.win.frames += 1;
        self.win.hold_sum_ms += now.saturating_duration_since(d.first).as_secs_f64() * 1000.0;
        self.win.hold_n += 1;
        io.events.push(Event::Frame {
            stream: self.id,
            kind,
            data: d.data,
            pts_us: d.pts_us,
            fflags: d.fflags,
            frame_id,
        });
    }

    /// Fertige Video-Frames in Reihenfolge ausliefern. Nach einer Luecke nur ab dem naechsten Keyframe.
    fn drain(&mut self, now: Instant, io: &mut Io) {
        loop {
            if self.broken {
                let key = self
                    .completed
                    .iter()
                    .find(|(_, d)| d.fflags & 1 == 1)
                    .map(|(k, _)| *k);
                let Some(k) = key else { break };
                let older: Vec<u32> = self
                    .completed
                    .keys()
                    .copied()
                    .filter(|x| !id_ge(*x, k))
                    .collect();
                for o in older {
                    self.completed.remove(&o);
                    self.c.frames_dropped += 1;
                    self.c.dropped_gap += 1;
                    self.note_dropped(o);
                }
                let d = self.completed.remove(&k).unwrap();
                self.deliver(now, 1, k, d, io);
                self.floor = Some(k.wrapping_add(1));
                self.broken = false;
                self.broken_since = None;
            } else {
                let f = self.floor.unwrap_or(0);
                match self.completed.remove(&f) {
                    Some(d) => {
                        self.deliver(now, 1, f, d, io);
                        self.floor = Some(f.wrapping_add(1));
                    }
                    None => break,
                }
            }
        }
        if !self.broken && !self.completed.is_empty() {
            self.blocked_since.get_or_insert(now);
        } else {
            self.blocked_since = None;
        }
    }

    /// Keyframe anfordern, hoechstens alle 300 ms.
    pub fn request_keyframe(&mut self, now: Instant, io: &mut Io) -> bool {
        if self
            .last_kf_req
            .is_some_and(|t| now.saturating_duration_since(t) < KEYFRAME_REQ_MIN_GAP)
        {
            return false;
        }
        self.last_kf_req = Some(now);
        self.c.keyframe_reqs_sent += 1;
        self.packet(now, T_KEYFRAME_REQ, &[], false, io);
        true
    }

    fn note_dropped(&mut self, frame_id: u32) {
        if self.dropped_ids.len() >= 64 {
            self.dropped_ids.pop_front();
        }
        self.dropped_ids.push_back(frame_id);
    }

    fn go_broken(&mut self, now: Instant, io: &mut Io) {
        if !self.broken {
            self.broken = true;
            self.broken_since = Some(now);
        }
        self.request_keyframe(now, io);
    }

    // ---------------------------------------------------------------- Zeitgesteuert

    pub fn tick(&mut self, now: Instant, info: &TickInfo, io: &mut Io) {
        if self.role == Role::Receiver {
            self.tick_receiver(now, io);
        }
        // ping: Empfaenger sobald Verkehr da ist, Sender sobald er gesendet hat (misst die Sender-RTT).
        let active = match self.role {
            Role::Receiver => self.saw_traffic,
            Role::Sender => self.tx_seq > 0,
        };
        if active {
            match self.last_ping {
                None if self.role == Role::Sender => self.last_ping = Some(now), // erstes ping nach 500 ms
                Some(t) if now.saturating_duration_since(t) < PING_INTERVAL => {}
                _ => self.ping_now(now, io),
            }
        }
        self.pace(now, io);
        if now.saturating_duration_since(self.last_stats) >= Duration::from_secs(1) {
            self.emit_stats(now, info, io);
        }
    }

    fn tick_receiver(&mut self, now: Instant, io: &mut Io) {
        let deadline = self.deadline();
        // Unvollstaendige Frames nach 2 x RTT (mindestens 30 ms) verwerfen.
        let expired: Vec<u32> = self
            .video
            .iter()
            .filter(|(_, a)| a.stale(now, deadline))
            .map(|(k, _)| *k)
            .collect();
        let mut lost = false;
        let mut incomplete = false;
        for k in expired {
            self.video.remove(&k);
            self.c.frames_dropped += 1;
            self.c.dropped_incomplete += 1;
            self.note_dropped(k);
            incomplete = true;
            let nf = k.wrapping_add(1);
            if self.floor.is_none_or(|f| id_ge(nf, f)) {
                self.floor = Some(nf);
            }
            lost = true;
        }
        let expired_a: Vec<u32> = self
            .audio
            .iter()
            .filter(|(_, a)| a.stale(now, deadline))
            .map(|(k, _)| *k)
            .collect();
        for k in expired_a {
            self.audio.remove(&k);
            self.c.frames_dropped += 1;
            self.c.dropped_audio += 1;
        }
        // Wartet die Reihenfolge auf einen Frame, der nie kommt?
        let mut blocked = false;
        if !self.broken {
            if let Some(b) = self.blocked_since {
                if now.saturating_duration_since(b) > deadline {
                    lost = true;
                    blocked = true;
                }
            }
        }
        if lost && !self.broken {
            if incomplete {
                self.c.gaps_incomplete += 1;
            } else if blocked {
                self.c.gaps_blocked += 1;
            }
        }
        if lost {
            self.go_broken(now, io);
            self.drain(now, io);
        }
        if self.broken
            && self.saw_traffic
            && self
                .broken_since
                .is_some_and(|t| now.saturating_duration_since(t) > deadline)
        {
            self.request_keyframe(now, io);
        }

        self.send_nacks(now, io);

        // feedback
        if let Some(fm) = self.first_media {
            let bad = self.win.last_resid_permille > RESIDUAL_LOSS_LIMIT
                || self.win.last_loss_permille > RAW_LOSS_EMERGENCY;
            let every = if bad {
                Duration::from_millis(200)
            } else {
                Duration::from_secs(1)
            };
            let due = match self.last_feedback {
                Some(t) => now.saturating_duration_since(t) >= every,
                None => now.saturating_duration_since(fm) >= Duration::from_millis(150),
            };
            if due {
                self.send_feedback(now, io);
            }
        }
    }

    fn nack_delay(&self) -> Duration {
        let by_jitter = (self.jitter_us / 1000.0) * 2.0 + 2.0;
        let by_reorder = self.reorder_ms * 1.2 + 1.0;
        // Nie laenger warten als die halbe Frist, sonst verfaellt der Frame vor dem ersten NACK
        // (z. B. wenn der Jitter-Schaetzer wegen unregelmaessiger pts hohe Werte liefert).
        let cap = (self.deadline().as_secs_f64() * 1000.0 / 2.0).max(2.0);
        Duration::from_secs_f64(by_jitter.max(by_reorder).clamp(2.0, cap) / 1000.0)
    }

    fn send_nacks(&mut self, now: Instant, io: &mut Io) {
        if self.missing.is_empty() {
            return;
        }
        let delay = self.nack_delay();
        self.late_nacked
            .retain(|_, t| now.saturating_duration_since(*t) <= RETX_WINDOW);
        let renack = Duration::from_secs_f64(
            (self.srtt_ms.unwrap_or(0.0) * 1.5).clamp(10.0, 100.0) / 1000.0,
        );
        let mut ask: Vec<u64> = Vec::new();
        let mut lost = 0u64;
        let mut resid = 0u64;
        self.missing.retain(|seq, m| {
            let age = now.saturating_duration_since(m.first);
            // Wer nach PROVISIONAL_LOSS noch fehlt, wird schon jetzt verbucht, damit die Regelung nicht
            // bis zum Ablauf (300 ms) blind ist; spaeter nicht noch einmal.
            if age >= PROVISIONAL_LOSS {
                if !m.lost_counted {
                    m.lost_counted = true;
                    lost += 1;
                }
                if !m.settled && !m.resid_counted {
                    m.resid_counted = true;
                    resid += 1;
                }
            }
            if age > RETX_WINDOW || m.tries >= MAX_NACK_TRIES {
                if !m.lost_counted {
                    lost += 1; // Original kam nie
                }
                if !m.settled && !m.resid_counted {
                    resid += 1; // nie zurueckgekommen und nicht per FEC ersetzt
                }
                return false;
            }
            let ready = !m.settled
                && age >= delay
                && m.last_nack
                    .is_none_or(|t| now.saturating_duration_since(t) >= renack);
            if ready {
                m.last_nack = Some(now);
                m.tries += 1;
                ask.push(*seq);
            }
            true
        });
        self.win.lost += lost;
        self.win.resid += resid;
        for chunk in ask.chunks(NACK_PER_PACKET) {
            let mut p = Vec::with_capacity(2 + chunk.len() * 8);
            p.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
            for s in chunk {
                p.extend_from_slice(&s.to_be_bytes());
            }
            self.packet(now, T_NACK, &p, false, io);
            self.c.nacks_sent += 1;
            self.c.nack_seqs_sent += chunk.len() as u64;
        }
    }

    /// (Rohverlust, Restverlust) im laufenden Fenster in Promille.
    ///
    /// Gemessen ueber die letzte Sekunde (gleitend), damit haeufigere Feedbacks bei Verlust nicht von
    /// kurzen, zufaellig verlustfreien Fenstern verfaelscht werden.
    fn loss_permille_window(&mut self, now: Instant) -> (u16, u16) {
        let expected = match (self.hi, self.win.hi_at_start) {
            (Some(hi), Some(start)) => hi.saturating_sub(start),
            _ => 0,
        };
        self.loss_hist
            .push_back((now, expected, self.win.lost, self.win.resid));
        while self
            .loss_hist
            .front()
            .is_some_and(|(t, ..)| now.saturating_duration_since(*t) > Duration::from_secs(1))
        {
            self.loss_hist.pop_front();
        }
        let (e, l, r) = self
            .loss_hist
            .iter()
            .fold((0u64, 0u64, 0u64), |a, (_, e, l, r)| {
                (a.0 + e, a.1 + l, a.2 + r)
            });
        if e == 0 {
            return (0, 0);
        }
        let pm = |n: u64| ((n.min(e) as f64 / e as f64) * 1000.0).round() as u16;
        (pm(l), pm(r))
    }

    fn send_feedback(&mut self, now: Instant, io: &mut Io) {
        let secs = self
            .last_feedback
            .or(self.first_media)
            .map(|t| now.saturating_duration_since(t).as_secs_f64())
            .unwrap_or(1.0)
            .max(0.05);
        let kbps = (self.win.bytes_recv as f64 * 8.0 / 1000.0 / secs) as u32;
        let (raw, resid) = self.loss_permille_window(now);
        let mut p = Vec::with_capacity(16);
        p.extend_from_slice(&kbps.to_be_bytes());
        p.extend_from_slice(&resid.to_be_bytes());
        p.extend_from_slice(&(self.jitter_us as u32).to_be_bytes());
        p.extend_from_slice(&self.last_frame_id.to_be_bytes());
        p.extend_from_slice(&raw.to_be_bytes());
        self.packet(now, T_FEEDBACK, &p, false, io);
        self.last_feedback = Some(now);
        self.win.last_loss_permille = raw;
        self.win.last_resid_permille = resid;
        self.win.lost = 0;
        self.win.resid = 0;
        self.win.hi_at_start = self.hi;
    }

    fn emit_stats(&mut self, now: Instant, info: &TickInfo, io: &mut Io) {
        let secs = now
            .saturating_duration_since(self.last_stats)
            .as_secs_f64()
            .max(0.001);
        let fps = self.win.frames as f64 / secs;
        let (bytes, loss_pct, latency_ms, rtt) = match self.role {
            Role::Sender => (
                self.win.bytes_sent,
                self.fb_raw_permille as f64 / 10.0,
                0.0,
                self.srtt_ms.unwrap_or(0.0),
            ),
            Role::Receiver => {
                let hold = if self.win.hold_n > 0 {
                    self.win.hold_sum_ms / self.win.hold_n as f64
                } else {
                    0.0
                };
                let rtt = self.srtt_ms.unwrap_or(0.0);
                (
                    self.win.bytes_recv,
                    self.win.last_loss_permille as f64 / 10.0,
                    rtt / 2.0 + hold,
                    rtt,
                )
            }
        };
        let kbps = bytes as f64 * 8.0 / 1000.0 / secs;
        let json = format!(
            "{{\"fps\":{:.1},\"kbps\":{:.0},\"rtt_ms\":{:.1},\"loss_pct\":{:.1},\"latency_ms\":{:.1},\
\"role\":\"{}\",\"target_kbps\":{},\"jitter_ms\":{:.2},\"frames_complete\":{},\"frames_delivered\":{},\
\"frames_dropped\":{},\"dropped_incomplete\":{},\"dropped_overflow\":{},\"dropped_gap\":{},\"dropped_audio\":{},\"late_packets\":{},\"gaps_incomplete\":{},\"gaps_blocked\":{},\"retransmits\":{},\"nacks\":{},\"fec_recovered\":{},\"auth_fail\":{},\"peer_recv_kbps\":{},\"resid_loss_pct\":{:.1},\"peer\":\"{}\",\"sock_rcvbuf\":{},\"sock_sndbuf\":{},\"pace_queue_bytes\":{},\"pace_queue_ms\":{:.0}}}",
            fps,
            kbps,
            rtt,
            loss_pct,
            latency_ms,
            if self.role == Role::Sender { "sender" } else { "receiver" },
            self.target_kbps(),
            self.jitter_us / 1000.0,
            self.c.frames_complete,
            self.c.frames_delivered,
            self.c.frames_dropped,
            self.c.dropped_incomplete,
            self.c.dropped_overflow,
            self.c.dropped_gap,
            self.c.dropped_audio,
            self.c.late_packets,
            self.c.gaps_incomplete,
            self.c.gaps_blocked,
            self.c.retransmits,
            self.c.nacks_sent,
            self.c.fec_recovered,
            info.auth_fail,
            self.fb_recv_kbps,
            match self.role {
                Role::Sender => self.fb_loss_permille,
                Role::Receiver => self.win.last_resid_permille,
            } as f64
                / 10.0,
            info.peer,
            info.rcvbuf,
            info.sndbuf,
            self.pace_bytes,
            self.pace_queue_ms()
        );
        io.events.push(Event::Stats {
            stream: self.id,
            json,
        });
        self.last_stats = now;
        self.win.bytes_sent = 0;
        self.win.bytes_recv = 0;
        self.win.frames = 0;
        self.win.hold_sum_ms = 0.0;
        self.win.hold_n = 0;
    }
}

/// Versucht, in Gruppe `fi` genau ein fehlendes Fragment aus der Paritaet zu rekonstruieren.
fn recover(a: &Asm, fi: usize, frame_id: u32) -> Option<(usize, Vec<u8>)> {
    let f = &a.fec[fi];
    let end = f.group_start.checked_add(f.group_len)?;
    if end > a.cnt {
        return None;
    }
    let mut missing = None;
    for i in f.group_start..end {
        if a.frags[i].is_none() {
            if missing.is_some() {
                return None; // mehr als eins fehlt: nicht reparierbar
            }
            missing = Some(i);
        }
    }
    let idx = missing?;
    let mut par = f.parity.clone();
    let mut len = f.len_xor;
    for i in f.group_start..end {
        if let Some(p) = &a.frags[i] {
            if p.len() > par.len() {
                return None;
            }
            for (x, y) in par.iter_mut().zip(p) {
                *x ^= y;
            }
            len ^= p.len() as u16;
        }
    }
    let len = len as usize;
    if len < MEDIA_HEAD || len > par.len() {
        return None;
    }
    par.truncate(len);
    let m = decode_media(&par)?;
    if m.frame_id != frame_id || m.frag_idx as usize != idx || m.frag_cnt as usize != a.cnt {
        return None;
    }
    Some((idx, par))
}
