//! Eingabe im Proxy-Fenster -> `proxy.input` (SCHNITTSTELLEN.md §3). Reine Logik ohne Fenster, damit sie
//! sich ohne Windows-Nachrichten testen laesst: Koordinaten-Umrechnung, Tastennamen, gehaltene Tasten und
//! Buttons (fuer das `up` bei Fokusverlust), Klickzaehler und Text.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::gpu::letterbox;
use crate::keytable_gen::VK_NAMES;

/// Client-Pixel -> Stream-Pixel. Das Bild liegt mit gleichem Seitenverhaeltnis mittig im Client
/// (Letterbox, siehe gpu.rs); ohne Rand ist das genau `x * Streambreite / Clientbreite`.
/// Punkte ausserhalb des Bildes ergeben Koordinaten ausserhalb von 0..Stream (der Sender verwirft sie).
pub fn to_stream(x: f64, y: f64, client: (u32, u32), stream: (u32, u32)) -> (f64, f64) {
    let r = letterbox(stream, client);
    let (dw, dh) = (
        (r.right - r.left).max(1) as f64,
        (r.bottom - r.top).max(1) as f64,
    );
    (
        (x - r.left as f64) * stream.0 as f64 / dw,
        (y - r.top as f64) * stream.1 as f64 / dh,
    )
}

/// Auf zwei Nachkommastellen (Stream-Pixel sind float; mehr Stellen sind nur Rauschen im JSON).
fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_F4: u32 = 0x73;
pub const VK_PROCESSKEY: u32 = 0xE5;
const RIGHT_SHIFT_SCAN: u32 = 0x36;

/// Seitenrichtige virtuelle Taste: ein Fenster bekommt nur Shift/Strg/Alt, links und rechts stecken in
/// Scan-Code und Extended-Bit (wie `hook_vk` in win_app/capture_win.py).
pub fn sided_vk(vk: u32, scan: u32, extended: bool) -> u32 {
    match vk {
        VK_SHIFT => {
            if scan & 0xFF == RIGHT_SHIFT_SCAN {
                0xA1
            } else {
                0xA0
            }
        }
        VK_CONTROL => {
            if extended {
                0xA3
            } else {
                0xA2
            }
        }
        VK_MENU => {
            if extended {
                0xA5
            } else {
                0xA4
            }
        }
        v => v,
    }
}

/// Benannte Taste (generierte Tabelle aus Beamer), sonst `None` (Zeichentaste).
pub fn named_key(vk: u32) -> Option<&'static str> {
    VK_NAMES.iter().find(|(v, _)| *v == vk).map(|(_, n)| *n)
}

/// Bleibt bei Windows und wird nicht weitergegeben: Windows-Tasten (Start, Win+...). Alt+Tab und
/// Strg+Alt+Entf erreichen das Fenster ohnehin nicht.
pub fn is_system_key(vk: u32) -> bool {
    matches!(vk, VK_LWIN | VK_RWIN)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl Button {
    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
            Button::Back => "back",
            Button::Forward => "forward",
        }
    }
}

/// Doppel- und Dreifachklick: gleicher Button, innerhalb der Doppelklickzeit und des Doppelklick-Rechtecks.
#[derive(Debug)]
struct LastClick {
    button: Button,
    at: Instant,
    pos: (f64, f64),
    n: u8,
}

pub struct InputState {
    /// Gehaltene Tasten nach seitenrichtigem VK (oder Scan-Code) -> Name vom `down`.
    /// Der Name haengt an Shift/Feststelltaste; der VK nicht. Sonst verfehlt das `up` die Taste
    /// ("A" gedrueckt, Shift los, "a" losgelassen) und sie bleibt beim Sender unten.
    keys: BTreeMap<u32, String>,
    buttons: BTreeSet<Button>,
    pos: (f64, f64),
    last_click: Option<LastClick>,
    pub dbl_time: Duration,
    /// Halbe Breite/Hoehe des Doppelklick-Rechtecks in Client-Pixeln.
    pub dbl_slop: (f64, f64),
    /// Erste Haelfte eines UTF-16-Surrogatpaars aus WM_CHAR.
    high_surrogate: Option<u16>,
    /// AltGr gedrueckt (rechtes Alt mit kuenstlichem linkem Strg davor): Zeichen gehen als `text`.
    pub altgr: bool,
    /// Das kuenstliche linke Strg von AltGr ist gedrueckt (wird nicht gemeldet).
    pub fake_ctrl: bool,
}

impl Default for InputState {
    fn default() -> Self {
        InputState {
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            pos: (0.0, 0.0),
            last_click: None,
            dbl_time: Duration::from_millis(500),
            dbl_slop: (2.0, 2.0),
            high_surrogate: None,
            altgr: false,
            fake_ctrl: false,
        }
    }
}

impl InputState {
    pub fn any_button(&self) -> bool {
        !self.buttons.is_empty()
    }

    pub fn held_keys(&self) -> impl Iterator<Item = &String> {
        self.keys.values()
    }

    /// Shift links oder rechts ist unten. Das Fenster braucht das fuer das Zeichen beim `down`,
    /// wenn `GetKeyState` den Tastaturzustand nicht mitbekommt (Tasten per `PostMessage`).
    pub fn holds_shift(&self) -> bool {
        const VK_LSHIFT: u32 = 0xA0;
        const VK_RSHIFT: u32 = 0xA1;
        self.keys.contains_key(&VK_LSHIFT)
            || self.keys.contains_key(&VK_RSHIFT)
            || self.keys.contains_key(&VK_SHIFT)
    }

    pub fn moved(&mut self, s: (f64, f64)) -> Value {
        self.pos = s;
        json!({"k": "move", "x": r2(s.0), "y": r2(s.1)})
    }

    /// `client` dient nur dem Doppelklick-Rechteck (Client-Pixel), `s` ist die Stream-Position.
    pub fn button(
        &mut self,
        b: Button,
        down: bool,
        s: (f64, f64),
        client: (f64, f64),
        now: Instant,
    ) -> Option<Value> {
        self.pos = s;
        if down {
            let n = match &self.last_click {
                Some(l)
                    if l.button == b
                        && now.saturating_duration_since(l.at) <= self.dbl_time
                        && (client.0 - l.pos.0).abs() <= self.dbl_slop.0
                        && (client.1 - l.pos.1).abs() <= self.dbl_slop.1 =>
                {
                    if l.n >= 3 {
                        1
                    } else {
                        l.n + 1
                    }
                }
                _ => 1,
            };
            self.last_click = Some(LastClick {
                button: b,
                at: now,
                pos: client,
                n,
            });
            self.buttons.insert(b);
            Some(json!({"k": "down", "x": r2(s.0), "y": r2(s.1), "b": b.name(), "n": n}))
        } else {
            // Ein `up` ohne `down` (Button vor dem Fokus gedrueckt) wird nicht gemeldet.
            if !self.buttons.remove(&b) {
                return None;
            }
            Some(json!({"k": "up", "x": r2(s.0), "y": r2(s.1), "b": b.name()}))
        }
    }

    /// Rad in Rastungen (WHEEL_DELTA = 1); positiv = vom Nutzer weg bzw. nach rechts, wie Beamer
    /// (`wheel_notches` in win_app/capture_win.py).
    pub fn scroll(&mut self, s: (f64, f64), dy: f64, dx: f64) -> Value {
        self.pos = s;
        json!({"k": "scroll", "x": r2(s.0), "y": r2(s.1), "dx": dx, "dy": dy, "unit": "line"})
    }

    /// Taste gedrueckt/losgelassen, gefuehrt ueber `id` (seitenrichtiger VK oder Scan-Code, wie
    /// `held = vk` in `win_app/sender.py` `on_key`).
    ///
    /// Der Name wird beim `down` gespeichert und beim `up` genau so gemeldet. Shift oder
    /// Feststelltaste koennen dazwischen umspringen ("A"/"a", "2"/"\""); der Aufrufer berechnet
    /// den Namen dann neu und wuerde die Taste sonst verfehlen. `name` gilt deshalb nur beim
    /// `down`. Gedrueckt halten (gleiche `id`, auch mit anderem Namen) meldet nur ein `down`;
    /// die Wiederholung macht der Sender selbst. Ein `up` ohne vorheriges `down` wird verworfen.
    pub fn key(&mut self, id: u32, name: &str, down: bool) -> Option<Value> {
        if down {
            if self.keys.contains_key(&id) {
                return None;
            }
            self.keys.insert(id, name.to_owned());
            Some(json!({"k": "key", "key": name, "down": true}))
        } else {
            let stored = self.keys.remove(&id)?;
            Some(json!({"k": "key", "key": stored, "down": false}))
        }
    }

    /// Ein WM_CHAR-Zeichen (UTF-16-Einheit) als `text`: zusammengesetzte Zeichen (Akzent-Tasten), IME,
    /// Alt+Ziffern. Zeichen, die schon als Taste gemeldet wurden, entfernt das Fenster vorher aus der Queue.
    pub fn char_unit(&mut self, unit: u16) -> Option<Value> {
        if (0xD800..0xDC00).contains(&unit) {
            self.high_surrogate = Some(unit);
            return None;
        }
        let text = if (0xDC00..0xE000).contains(&unit) {
            let hi = self.high_surrogate.take()?;
            String::from_utf16(&[hi, unit]).ok()?
        } else {
            self.high_surrogate = None;
            let c = char::from_u32(unit as u32)?;
            if c.is_control() {
                return None;
            }
            c.to_string()
        };
        Some(json!({"k": "text", "text": text}))
    }

    /// Fokusverlust: alles Gedrueckte loslassen (erst Buttons, dann Tasten), damit beim Sender nichts haengt.
    pub fn release_all(&mut self) -> Vec<Value> {
        let (x, y) = (r2(self.pos.0), r2(self.pos.1));
        let mut out: Vec<Value> = std::mem::take(&mut self.buttons)
            .into_iter()
            .map(|b| json!({"k": "up", "x": x, "y": y, "b": b.name()}))
            .collect();
        // Reihenfolge wie bisher (nach Name), nicht nach VK.
        let mut names: Vec<String> = std::mem::take(&mut self.keys).into_values().collect();
        names.sort();
        out.extend(
            names
                .into_iter()
                .map(|k| json!({"k": "key", "key": k, "down": false})),
        );
        self.high_surrogate = None;
        self.last_click = None;
        self.altgr = false;
        self.fake_ctrl = false;
        out
    }
}

/// Entprellung fuer `proxy.resize`: hoechstens ein Wert je `every`, der letzte Wert kommt immer an.
#[derive(Debug)]
pub struct Debounce<T: PartialEq + Clone> {
    last_sent: Option<(Instant, T)>,
    pending: Option<T>,
}

impl<T: PartialEq + Clone> Default for Debounce<T> {
    fn default() -> Self {
        Debounce {
            last_sent: None,
            pending: None,
        }
    }
}

pub enum DebounceOut<T> {
    /// Jetzt senden.
    Send(T),
    /// Spaeter senden (Timer in dieser Zeit stellen).
    Later(Duration),
    /// Unveraendert, nichts zu tun.
    Nothing,
}

impl<T: PartialEq + Clone> Debounce<T> {
    pub fn offer(&mut self, v: T, now: Instant, every: Duration) -> DebounceOut<T> {
        if self.pending.is_none() && self.last_sent.as_ref().is_some_and(|(_, s)| *s == v) {
            return DebounceOut::Nothing;
        }
        match &self.last_sent {
            Some((t, _)) if now.saturating_duration_since(*t) < every => {
                self.pending = Some(v);
                DebounceOut::Later(every - now.saturating_duration_since(*t))
            }
            _ => {
                self.pending = None;
                self.last_sent = Some((now, v.clone()));
                DebounceOut::Send(v)
            }
        }
    }

    /// Timer abgelaufen oder Ziehen beendet: wartenden Wert senden.
    pub fn flush(&mut self, now: Instant) -> Option<T> {
        let v = self.pending.take()?;
        if self.last_sent.as_ref().is_some_and(|(_, s)| *s == v) {
            return None;
        }
        self.last_sent = Some((now, v.clone()));
        Some(v)
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// Groesse in Punkten aus Client-Pixeln und DPI (96 = 100 %), auf 0,01 gerundet.
pub fn px_to_points(px: (u32, u32), dpi: u32) -> (f64, f64) {
    let s = dpi.max(1) as f64 / 96.0;
    (r2(px.0 as f64 / s), r2(px.1 as f64 / s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_without_letterbox_scale_linearly() {
        // Client halb so gross wie der Stream: x * Streambreite / Clientbreite.
        assert_eq!(to_stream(0.0, 0.0, (960, 540), (1920, 1080)), (0.0, 0.0));
        assert_eq!(
            to_stream(480.0, 270.0, (960, 540), (1920, 1080)),
            (960.0, 540.0)
        );
        assert_eq!(
            to_stream(100.5, 10.0, (1920, 1080), (1920, 1080)),
            (100.5, 10.0)
        );
    }

    #[test]
    fn coordinates_with_letterbox_skip_the_border() {
        // 16:9-Stream in quadratischem Client 1000x1000: Bild 1000x563 ab y=218.
        let (x, y) = to_stream(500.0, 218.0, (1000, 1000), (1920, 1080));
        assert!((x - 960.0).abs() < 0.01 && y.abs() < 0.01, "{x} {y}");
        // Im schwarzen Rand oben: negativ, der Sender verwirft es.
        assert!(to_stream(500.0, 10.0, (1000, 1000), (1920, 1080)).1 < 0.0);
    }

    #[test]
    fn sides_of_modifiers() {
        assert_eq!(sided_vk(VK_SHIFT, 0x2A, false), 0xA0);
        assert_eq!(sided_vk(VK_SHIFT, 0x36, false), 0xA1);
        assert_eq!(sided_vk(VK_CONTROL, 0x1D, false), 0xA2);
        assert_eq!(sided_vk(VK_CONTROL, 0x1D, true), 0xA3);
        assert_eq!(sided_vk(VK_MENU, 0x38, true), 0xA5);
        assert_eq!(sided_vk(0x41, 0x1E, false), 0x41);
    }

    #[test]
    fn key_names_are_raw_physical() {
        // Roh, ohne Cmd/Strg-Umsetzung (macht Python): Strg = "ctrl", Windows-Taste = "cmd".
        assert_eq!(named_key(0xA2), Some("ctrl"));
        assert_eq!(named_key(0xA3), Some("ctrl_r"));
        assert_eq!(named_key(0x5B), Some("cmd"));
        assert_eq!(named_key(0xA4), Some("alt"));
        assert_eq!(named_key(0x0D), Some("enter"));
        assert_eq!(named_key(0x70), Some("f1"));
        assert_eq!(named_key(0x83), Some("f20"));
        assert_eq!(named_key(0x41), None); // Zeichentaste
        assert!(is_system_key(VK_LWIN) && !is_system_key(0xA2));
    }

    #[test]
    fn click_counts() {
        let mut s = InputState::default();
        let t = Instant::now();
        let p = (10.0, 10.0);
        let d = |s: &mut InputState, t: Instant, c: (f64, f64)| {
            s.button(Button::Left, true, p, c, t).unwrap()["n"].clone()
        };
        assert_eq!(d(&mut s, t, p), 1);
        s.button(Button::Left, false, p, p, t);
        assert_eq!(d(&mut s, t + Duration::from_millis(200), p), 2);
        s.button(Button::Left, false, p, p, t);
        assert_eq!(d(&mut s, t + Duration::from_millis(400), (11.0, 10.0)), 3);
        s.button(Button::Left, false, p, p, t);
        // zu spaet
        assert_eq!(d(&mut s, t + Duration::from_millis(1500), p), 1);
        s.button(Button::Left, false, p, p, t);
        // zu weit weg
        assert_eq!(d(&mut s, t + Duration::from_millis(1600), (30.0, 10.0)), 1);
    }

    #[test]
    fn release_all_lifts_everything_once() {
        let mut s = InputState::default();
        let t = Instant::now();
        s.key(0xA2, "ctrl", true);
        s.key(0x41, "a", true);
        assert!(s.key(0x41, "a", true).is_none()); // Wiederholung: kein zweites down
        s.button(Button::Right, true, (5.0, 6.0), (1.0, 1.0), t);
        let ups = s.release_all();
        assert_eq!(ups.len(), 3);
        assert_eq!(ups[0], json!({"k":"up","x":5.0,"y":6.0,"b":"right"}));
        assert!(ups
            .iter()
            .any(|v| *v == json!({"k":"key","key":"ctrl","down":false})));
        assert!(s.release_all().is_empty());
        // Das echte `up` danach wird nicht doppelt gemeldet.
        assert!(s.key(0xA2, "ctrl", false).is_none());
        assert!(s
            .button(Button::Right, false, (0.0, 0.0), (0.0, 0.0), t)
            .is_none());
    }

    #[test]
    fn shift_released_before_the_key_uses_the_down_name() {
        // Shift runter, A runter, Shift hoch, A hoch: der Aufrufer berechnet beim Loslassen "a".
        // Gemeldet wird trotzdem "A", und das naechste Shift+A ist kein verschlucktes Repeat.
        let mut s = InputState::default();
        assert_eq!(
            s.key(0xA0, "shift", true),
            Some(json!({"k": "key", "key": "shift", "down": true}))
        );
        assert!(s.holds_shift());
        assert_eq!(
            s.key(0x41, "A", true),
            Some(json!({"k": "key", "key": "A", "down": true}))
        );
        // Wiederholung nach VK, auch wenn der Name inzwischen "a" waere.
        assert!(s.key(0x41, "a", true).is_none());
        assert_eq!(
            s.key(0xA0, "shift", false),
            Some(json!({"k": "key", "key": "shift", "down": false}))
        );
        assert!(!s.holds_shift());
        assert_eq!(
            s.key(0x41, "a", false),
            Some(json!({"k": "key", "key": "A", "down": false}))
        );
        assert_eq!(
            s.key(0xA0, "shift", true),
            Some(json!({"k": "key", "key": "shift", "down": true}))
        );
        assert_eq!(
            s.key(0x41, "A", true),
            Some(json!({"k": "key", "key": "A", "down": true}))
        );
        assert_eq!(
            s.key(0xA0, "shift", false),
            Some(json!({"k": "key", "key": "shift", "down": false}))
        );
        assert_eq!(
            s.key(0x41, "a", false),
            Some(json!({"k": "key", "key": "A", "down": false}))
        );
        assert!(s.release_all().is_empty());
    }

    #[test]
    fn symbol_and_caps_keep_the_name_from_down() {
        let mut s = InputState::default();
        // Zifferntaste: down "\"", up kommt als "2" an.
        let quote = "\"";
        assert_eq!(
            s.key(0x32, quote, true),
            Some(json!({"k": "key", "key": quote, "down": true}))
        );
        assert_eq!(
            s.key(0x32, "2", false),
            Some(json!({"k": "key", "key": quote, "down": false}))
        );
        // Feststelltaste dazwischen: der Name vom down bleibt.
        assert_eq!(
            s.key(0x41, "a", true),
            Some(json!({"k": "key", "key": "a", "down": true}))
        );
        s.key(0x14, "caps_lock", true);
        assert_eq!(
            s.key(0x41, "A", false),
            Some(json!({"k": "key", "key": "a", "down": false}))
        );
        let ups = s.release_all();
        assert!(ups
            .iter()
            .any(|v| *v == json!({"k": "key", "key": "caps_lock", "down": false})));
        assert!(s.key(0x41, "A", false).is_none());
    }

    #[test]
    fn text_from_wm_char() {
        let mut s = InputState::default();
        // Zusammengesetztes Zeichen (Akzent-Taste) oder IME: als Text.
        assert_eq!(s.char_unit(0xE9), Some(json!({"k":"text","text":"\u{e9}"})));
        // Steuerzeichen (Strg+C liefert 0x03) nie als Text.
        assert!(s.char_unit(0x03).is_none());
        // Surrogatpaar: U+1F600
        assert!(s.char_unit(0xD83D).is_none());
        assert_eq!(
            s.char_unit(0xDE00),
            Some(json!({"k":"text","text":"\u{1F600}"}))
        );
    }

    #[test]
    fn scroll_sign_and_units() {
        let mut s = InputState::default();
        let v = s.scroll((1.0, 2.0), 1.0, -0.5);
        assert_eq!(
            v,
            json!({"k":"scroll","x":1.0,"y":2.0,"dx":-0.5,"dy":1.0,"unit":"line"})
        );
    }

    #[test]
    fn debounce_limits_rate_and_keeps_last() {
        let mut d = Debounce::default();
        let t = Instant::now();
        let every = Duration::from_millis(100);
        assert!(matches!(
            d.offer((1, 1), t, every),
            DebounceOut::Send((1, 1))
        ));
        assert!(matches!(
            d.offer((2, 2), t + Duration::from_millis(30), every),
            DebounceOut::Later(_)
        ));
        assert!(matches!(
            d.offer((3, 3), t + Duration::from_millis(60), every),
            DebounceOut::Later(_)
        ));
        // Ende des Ziehens: der letzte Wert kommt.
        assert_eq!(d.flush(t + Duration::from_millis(70)), Some((3, 3)));
        assert_eq!(d.flush(t + Duration::from_millis(80)), None);
        // Gleicher Wert erneut: nichts.
        assert!(matches!(
            d.offer((3, 3), t + Duration::from_millis(500), every),
            DebounceOut::Nothing
        ));
    }

    #[test]
    fn points_from_pixels() {
        assert_eq!(px_to_points((1920, 1080), 96), (1920.0, 1080.0));
        assert_eq!(px_to_points((1920, 1080), 144), (1280.0, 720.0));
    }
}
