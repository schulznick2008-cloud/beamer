//! Kanten-Drag (SCHNITTSTELLEN.md §2.1): Proxy an die Maus legen (`recv.place`), ihr folgen, bis die linke
//! Taste los ist, und die Position als `proxy.drag` melden. Hier nur die Rechnung, ohne Fenster: Punkte und
//! Pixel je Monitor-DPI, Klemmen auf die Arbeitsflaeche, Drosselung und das Ende des Zugs.

use std::time::{Duration, Instant};

/// Rechteck in physischen Bildschirm-Pixeln (virtueller Desktop): links, oben, rechts, unten.
pub type Px = (i32, i32, i32, i32);

/// Ein Monitor: Rechteck in Pixeln und seine DPI (96 = 100 %).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mon {
    pub rect: Px,
    pub dpi: u32,
}

impl Mon {
    fn scale(&self) -> f64 {
        self.dpi.max(1) as f64 / 96.0
    }

    /// Rechteck in Punkten: Pixel / Skalierung (bei 100 % gleich).
    fn rect_pt(&self) -> (f64, f64, f64, f64) {
        let s = self.scale();
        (
            self.rect.0 as f64 / s,
            self.rect.1 as f64 / s,
            self.rect.2 as f64 / s,
            self.rect.3 as f64 / s,
        )
    }
}

fn dist2(p: (f64, f64), r: (f64, f64, f64, f64)) -> f64 {
    let dx = if p.0 < r.0 {
        r.0 - p.0
    } else if p.0 >= r.2 {
        p.0 - r.2 + 1.0
    } else {
        0.0
    };
    let dy = if p.1 < r.1 {
        r.1 - p.1
    } else if p.1 >= r.3 {
        p.1 - r.3 + 1.0
    } else {
        0.0
    };
    dx * dx + dy * dy
}

/// Monitor, auf dem der Punkt liegt, sonst der naechste.
fn nearest<F: Fn(&Mon) -> (f64, f64, f64, f64)>(
    p: (f64, f64),
    mons: &[Mon],
    rect: F,
) -> Option<Mon> {
    mons.iter()
        .copied()
        .min_by(|a, b| dist2(p, rect(a)).total_cmp(&dist2(p, rect(b))))
}

/// Bildschirm-Punkte -> Pixel: Punkte gelten je Monitor (Pixel = Punkte x DPI/96).
pub fn points_to_px(pt: (f64, f64), mons: &[Mon]) -> (i32, i32) {
    match nearest(pt, mons, Mon::rect_pt) {
        Some(m) => {
            let s = m.scale();
            ((pt.0 * s).round() as i32, (pt.1 * s).round() as i32)
        }
        None => (pt.0.round() as i32, pt.1.round() as i32),
    }
}

/// Pixel -> Bildschirm-Punkte (Umkehrung von `points_to_px`).
pub fn px_to_points(px: (i32, i32), mons: &[Mon]) -> (f64, f64) {
    let p = (px.0 as f64, px.1 as f64);
    let s = nearest(p, mons, |m| {
        (
            m.rect.0 as f64,
            m.rect.1 as f64,
            m.rect.2 as f64,
            m.rect.3 as f64,
        )
    })
    .map_or(1.0, |m| m.scale());
    let r = |v: f64| (v / s * 100.0).round() / 100.0;
    (r(p.0), r(p.1))
}

/// Linke obere Ecke des Fensters (Aussengroesse `outer`), damit der Griffpunkt `grab` (relativ zum Fenster)
/// auf `point` liegt, geklemmt auf die Arbeitsflaeche `work`. Passt das Fenster nicht, gewinnt links/oben.
pub fn place_origin(
    point: (i32, i32),
    outer: (i32, i32),
    grab: (i32, i32),
    work: Px,
) -> (i32, i32) {
    let clamp = |v: i32, lo: i32, hi: i32| if hi < lo { lo } else { v.clamp(lo, hi) };
    (
        clamp(point.0 - grab.0, work.0, work.2 - outer.0),
        clamp(point.1 - grab.1, work.1, work.3 - outer.1),
    )
}

/// Griffpunkt in der Titelleiste: Mitte der Breite, Mitte der Hoehe des oberen Rahmens (Titel + Rand).
pub fn title_grab(outer_w: i32, top_inset: i32) -> (i32, i32) {
    (outer_w / 2, (top_inset / 2).max(1))
}

/// Hoechstens ein Ereignis je `every` (proxy.drag <= 30/s).
#[derive(Debug)]
pub struct Throttle {
    last: Option<Instant>,
    every: Duration,
}

impl Throttle {
    pub fn new(every: Duration) -> Self {
        Throttle { last: None, every }
    }

    pub fn ready(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < self.every)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

pub const DRAG_EVERY: Duration = Duration::from_millis(34);
/// Ist die linke Taste beim Start nicht unten (z. B. noch nicht vom KVM eingespielt), endet das Folgen
/// spaetestens nach dieser Zeit.
pub const NO_BUTTON_GRACE: Duration = Duration::from_millis(1000);

/// Dem Mauszeiger folgen, bis die linke Taste los ist.
#[derive(Debug)]
pub struct Follow {
    pub grab: (i32, i32),
    started: Instant,
    seen_down: bool,
}

impl Follow {
    pub fn new(grab: (i32, i32), now: Instant) -> Self {
        Follow {
            grab,
            started: now,
            seen_down: false,
        }
    }

    /// Ein Schritt: neue linke obere Ecke und ob der Zug zu Ende ist.
    pub fn tick(
        &mut self,
        cursor: (i32, i32),
        button_down: bool,
        now: Instant,
    ) -> ((i32, i32), bool) {
        if button_down {
            self.seen_down = true;
        }
        let ended = if self.seen_down {
            !button_down
        } else {
            now.saturating_duration_since(self.started) > NO_BUTTON_GRACE
        };
        ((cursor.0 - self.grab.0, cursor.1 - self.grab.1), ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Mon = Mon {
        rect: (-1920, 0, 0, 1080),
        dpi: 96,
    };
    const MAIN_150: Mon = Mon {
        rect: (0, 0, 2880, 1620),
        dpi: 144,
    };

    #[test]
    fn points_and_pixels_at_100_and_150_percent() {
        // 100 %: identisch, auch links vom Hauptmonitor (negative Koordinaten).
        assert_eq!(
            points_to_px((-500.0, 300.0), &[LEFT, MAIN_150]),
            (-500, 300)
        );
        // 150 %: 1000 Punkte = 1500 Pixel.
        assert_eq!(
            points_to_px((1000.0, 200.0), &[LEFT, MAIN_150]),
            (1500, 300)
        );
        assert_eq!(
            px_to_points((1500, 300), &[LEFT, MAIN_150]),
            (1000.0, 200.0)
        );
        assert_eq!(
            px_to_points((-500, 300), &[LEFT, MAIN_150]),
            (-500.0, 300.0)
        );
        // Ausserhalb aller Monitore: naechster Monitor bestimmt die Skalierung.
        assert_eq!(points_to_px((5000.0, 100.0), &[MAIN_150]), (7500, 150));
        assert_eq!(points_to_px((7.4, 3.0), &[]), (7, 3));
    }

    #[test]
    fn placement_puts_the_title_under_the_point_and_clamps() {
        let work = (0, 0, 1920, 1040);
        let grab = title_grab(800, 32);
        assert_eq!(grab, (400, 16));
        // Mitten auf dem Bildschirm: Titelmitte unter dem Punkt.
        assert_eq!(place_origin((960, 300), (800, 600), grab, work), (560, 284));
        // Am rechten Rand: Fenster bleibt ganz auf der Arbeitsflaeche.
        assert_eq!(
            place_origin((1910, 300), (800, 600), grab, work),
            (1120, 284)
        );
        // Oben/links ueber den Rand: auf 0 geklemmt.
        assert_eq!(place_origin((5, 2), (800, 600), grab, work), (0, 0));
        // Unten: Unterkante an der Taskleiste.
        assert_eq!(
            place_origin((960, 1030), (800, 600), grab, work),
            (560, 440)
        );
        // Groesser als die Arbeitsflaeche: links/oben gewinnt.
        assert_eq!(place_origin((960, 300), (3000, 2000), grab, work), (0, 0));
    }

    #[test]
    fn throttle_30_per_second() {
        let mut t = Throttle::new(DRAG_EVERY);
        let t0 = Instant::now();
        let n = (0..100)
            .filter(|i| t.ready(t0 + Duration::from_millis(*i * 10)))
            .count();
        // 1 s in 10-ms-Schritten: hoechstens 30.
        assert!((25..=30).contains(&n), "{n}");
    }

    #[test]
    fn follow_until_the_left_button_is_up() {
        let t0 = Instant::now();
        let mut f = Follow::new((400, 16), t0);
        // Taste unten: folgen.
        assert_eq!(f.tick((1000, 500), true, t0), ((600, 484), false));
        assert_eq!(
            f.tick((1100, 520), true, t0 + Duration::from_millis(50)),
            ((700, 504), false)
        );
        // Losgelassen: Ende, letzte Position.
        assert_eq!(
            f.tick((1105, 521), false, t0 + Duration::from_millis(60)),
            ((705, 505), true)
        );
    }

    #[test]
    fn follow_without_button_ends_after_grace() {
        let t0 = Instant::now();
        let mut f = Follow::new((10, 10), t0);
        assert!(!f.tick((0, 0), false, t0 + Duration::from_millis(500)).1);
        assert!(f.tick((0, 0), false, t0 + Duration::from_millis(1100)).1);
    }
}
