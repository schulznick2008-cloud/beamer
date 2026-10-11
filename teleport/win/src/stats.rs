//! Messwerte des Empfaengers je `tid` und das `stats`-Ereignis (SCHNITTSTELLEN.md §1.2).
//!
//! Der Transport liefert jede Sekunde sein Stats-JSON (fps, kbps, rtt_ms, loss_pct, latency_ms, ...).
//! Der Helfer haengt Decode- und Present-Zeiten an und gibt alles als ein `stats`-Ereignis aus.

use serde_json::{json, Map, Value};

/// Messfenster bis zur naechsten Ausgabe.
#[derive(Default, Debug, Clone)]
pub struct Window {
    /// Dauer von "Frame an den Decoder" bis "dekodiertes Bild da" (ms).
    pub decode_ms: Vec<f64>,
    /// Dauer von "Bild da" bis Rueckkehr von `Present` (ms, inkl. Video-Processor-Blt).
    pub present_ms: Vec<f64>,
    /// Empfang des letzten Fragments (Transport-Callback) bis Rueckkehr von `Present` (ms).
    pub recv_to_present_ms: Vec<f64>,
    /// Nur mit gemeinsamer Uhr (Selbsttest): `pts_us` des Senders bis Present (ms).
    pub pts_age_ms: Vec<f64>,
    pub presented: u64,
    pub decode_errors: u64,
    /// Frames, die nach einem Fehler bis zum naechsten Keyframe nicht angezeigt wurden.
    pub skipped: u64,
    /// Wie oft der Empfaenger einen Rueckstau abgebaut hat (Sprung zum Keyframe bzw. Verwerfen).
    pub catchups: u64,
    /// Dekodiert, aber vor dem Zeigen von einem neueren Bild ersetzt (Jitter-Puffer, 1 Bild).
    pub superseded: u64,
    /// Dekodiert bis Beginn des Present (Warten auf die Present-Queue, ms).
    pub queue_ms: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub decode_ms: f64,
    pub present_ms: f64,
    pub recv_to_present_ms: f64,
    pub recv_to_present_p95_ms: f64,
    pub recv_to_present_max_ms: f64,
    pub recv_to_present_hist: [u64; 10],
    /// Median, `None` ohne gemeinsame Uhr.
    pub pts_age_ms: Option<f64>,
    pub presented: u64,
    pub decode_errors: u64,
    pub skipped: u64,
    pub catchups: u64,
    pub superseded: u64,
    pub queue_ms: f64,
}

/// Obere Klassengrenzen (ms) des Histogramms `recv_to_present_hist`; die letzte Klasse ist offen.
/// So laesst sich das p95 ueber einen ganzen Lauf aus den Sekundenwerten zusammensetzen.
pub const HIST_EDGES_MS: [f64; 9] = [1.0, 2.0, 5.0, 10.0, 16.7, 25.0, 50.0, 100.0, 250.0];

pub fn histogram(v: &[f64]) -> [u64; 10] {
    let mut h = [0u64; 10];
    for x in v {
        let i = HIST_EDGES_MS.iter().position(|e| x <= e).unwrap_or(9);
        h[i] += 1;
    }
    h
}

pub fn percentile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * p).round() as usize]
}

impl Window {
    pub fn summary(&self) -> Summary {
        Summary {
            decode_ms: percentile(&self.decode_ms, 0.5),
            present_ms: percentile(&self.present_ms, 0.5),
            recv_to_present_ms: percentile(&self.recv_to_present_ms, 0.5),
            recv_to_present_p95_ms: percentile(&self.recv_to_present_ms, 0.95),
            recv_to_present_max_ms: percentile(&self.recv_to_present_ms, 1.0),
            recv_to_present_hist: histogram(&self.recv_to_present_ms),
            pts_age_ms: (!self.pts_age_ms.is_empty()).then(|| percentile(&self.pts_age_ms, 0.5)),
            presented: self.presented,
            decode_errors: self.decode_errors,
            skipped: self.skipped,
            catchups: self.catchups,
            superseded: self.superseded,
            queue_ms: percentile(&self.queue_ms, 0.5),
        }
    }

    /// Zusammenfassen und das Fenster leeren.
    pub fn take(&mut self) -> Summary {
        let s = self.summary();
        *self = Window::default();
        s
    }
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Baut das `stats`-Ereignis aus dem Transport-JSON und den eigenen Messwerten.
/// `latency_ms` (geschaetzt) = Haltezeit im Transport + Empfang bis Present.
pub fn stats_event(tid: u32, transport_json: &str, s: &Summary, decoder: &str) -> Value {
    let mut m: Map<String, Value> = match serde_json::from_str(transport_json) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let t_lat = m.get("latency_ms").and_then(Value::as_f64).unwrap_or(0.0);
    m.insert("v".into(), json!(1));
    m.insert("ev".into(), json!("stats"));
    m.insert("tid".into(), json!(tid));
    for k in ["fps", "kbps", "rtt_ms", "loss_pct"] {
        m.entry(k).or_insert(json!(0));
    }
    m.insert("transport_latency_ms".into(), json!(r2(t_lat)));
    m.insert("latency_ms".into(), json!(r2(t_lat + s.recv_to_present_ms)));
    m.insert("decode_ms".into(), json!(r2(s.decode_ms)));
    m.insert("present_ms".into(), json!(r2(s.present_ms)));
    m.insert("recv_to_present_ms".into(), json!(r2(s.recv_to_present_ms)));
    m.insert(
        "recv_to_present_p95_ms".into(),
        json!(r2(s.recv_to_present_p95_ms)),
    );
    m.insert(
        "recv_to_present_max_ms".into(),
        json!(r2(s.recv_to_present_max_ms)),
    );
    m.insert("recv_to_present_hist".into(), json!(s.recv_to_present_hist));
    if let Some(a) = s.pts_age_ms {
        m.insert("same_clock_latency_ms".into(), json!(r2(a)));
    }
    m.insert("frames_presented".into(), json!(s.presented));
    m.insert("decode_errors".into(), json!(s.decode_errors));
    m.insert("frames_skipped".into(), json!(s.skipped));
    m.insert("catchups".into(), json!(s.catchups));
    m.insert("frames_superseded".into(), json!(s.superseded));
    m.insert("queue_ms".into(), json!(r2(s.queue_ms)));
    m.insert("decoder".into(), json!(decoder));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[3.0, 1.0, 2.0], 0.5), 2.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 100.0], 1.0), 100.0);
    }

    #[test]
    fn window_take_resets() {
        let mut w = Window::default();
        w.decode_ms.extend([1.0, 2.0, 3.0]);
        w.recv_to_present_ms.extend([4.0, 5.0, 6.0]);
        w.presented = 3;
        let s = w.take();
        assert_eq!(s.decode_ms, 2.0);
        assert_eq!(s.recv_to_present_ms, 5.0);
        assert_eq!(s.presented, 3);
        assert!(w.decode_ms.is_empty());
        assert_eq!(w.presented, 0);
    }

    #[test]
    fn event_merges_transport_fields() {
        let t = r#"{"fps":59.9,"kbps":12000,"rtt_ms":3.5,"loss_pct":0.1,"latency_ms":2.0,"peer":"1.2.3.4:5"}"#;
        let s = Summary {
            decode_ms: 1.234,
            present_ms: 0.5,
            recv_to_present_ms: 3.0,
            recv_to_present_p95_ms: 4.0,
            recv_to_present_max_ms: 4.0,
            recv_to_present_hist: [0; 10],
            pts_age_ms: None,
            presented: 60,
            decode_errors: 0,
            skipped: 0,
            catchups: 0,
            superseded: 0,
            queue_ms: 0.0,
        };
        let e = stats_event(7, t, &s, "d3d11va");
        assert_eq!(e["v"], 1);
        assert_eq!(e["ev"], "stats");
        assert_eq!(e["tid"], 7);
        assert_eq!(e["fps"], 59.9);
        assert_eq!(e["peer"], "1.2.3.4:5");
        assert_eq!(e["latency_ms"], 5.0);
        assert_eq!(e["decode_ms"], 1.23);
        assert_eq!(e["decoder"], "d3d11va");
        // Kaputtes Transport-JSON: Pflichtfelder trotzdem da
        let e = stats_event(7, "nope", &s, "d3d11va");
        assert_eq!(e["fps"], 0);
        assert_eq!(e["latency_ms"], 3.0);
    }
}

#[cfg(test)]
mod hist_tests {
    use super::*;

    #[test]
    fn histogram_bins() {
        let h = histogram(&[0.5, 1.0, 1.5, 16.0, 17.0, 30.0, 300.0]);
        assert_eq!(h, [2, 1, 0, 0, 1, 1, 1, 0, 0, 1]);
        assert_eq!(h.iter().sum::<u64>(), 7);
    }
}
