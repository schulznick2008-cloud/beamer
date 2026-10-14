//! Mac-Fensterrahmen des Proxys (macOS Sonoma/Sequoia): Geometrie, Hit-Test und die selbst gezeichnete
//! Titelleiste.
//!
//! Zwei Arten:
//! - `stream`: Die echte Mac-Titelleiste ist im Stream. Sie wird unveraendert gezeigt, hier wird nichts
//!   gezeichnet; nur Hit-Test (Ziehen, Ampel-Klicks) an den vom Mac gemeldeten Ampel-Positionen.
//! - `synthetic`: Der Stream hat keine Titelleiste (full-size content view, transparente Titelleiste, SwiftUI).
//!   Der Proxy legt oben eine Leiste von `BAR_PT` Punkten an und zeichnet Titel und Ampeln selbst.
//!
//! Das Bild fuellt den Inhaltsbereich (Client ohne Leiste) immer ganz aus: kein Letterbox, keine schwarzen
//! Raender. Passt das Seitenverhaeltnis kurz nicht (Nutzer zieht, Mac antwortet noch), wird gestreckt.
//!
//! Masse in Punkten (1 pt = 1 px bei 100 %), Vorlage: macOS-14/15-Standardfenster.

use crate::input::{
    HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT,
};

/// Hoehe der gezeichneten Titelleiste.
pub const BAR_PT: f64 = 28.0;
/// Ampel: Durchmesser, Abstand zwischen den Kreisen, Abstand des ersten Kreises vom linken Rand.
pub const LIGHT_D_PT: f64 = 12.0;
pub const LIGHT_GAP_PT: f64 = 8.0;
pub const LIGHT_LEFT_PT: f64 = 20.0;
/// Rand der Ampel-Kreise (dunkler Ring).
pub const LIGHT_RIM_PT: f64 = 0.5;
/// So weit um die Ampelgruppe herum zeigt Hover die Symbole.
const HOVER_PAD_PT: f64 = 4.0;
/// Kantenbreite fuer Groessenaenderung.
pub const RESIZE_BORDER_PT: f64 = 6.0;
/// Hoehe der Titelleiste im Stream, wenn der Mac nichts meldet.
pub const STREAM_TITLE_PT: f64 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrafficLight {
    Close,
    Minimize,
    Zoom,
}

const LIGHTS: [TrafficLight; 3] = [
    TrafficLight::Close,
    TrafficLight::Minimize,
    TrafficLight::Zoom,
];

/// Ampeln der echten Titelleiste im Stream (Mittelpunkte in Punkten, relativ zur linken oberen Fensterecke)
/// und die Hoehe des Zugbereichs. Ohne Angabe vom Mac: Standard-Titelleiste von 28 pt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamLights {
    pub centers: [(f64, f64); 3],
    pub band_pt: f64,
}

impl Default for StreamLights {
    fn default() -> Self {
        StreamLights {
            centers: [(14.0, 14.0), (34.0, 14.0), (54.0, 14.0)],
            band_pt: STREAM_TITLE_PT,
        }
    }
}

impl StreamLights {
    /// Aus `recv.open` `lights` ([[x, y], …], Punkte). Zugbereich: bis zur doppelten Hoehe der Ampel-Mitte
    /// (Toolbar-Fenster haben die Ampeln tiefer und eine hoehere Leiste).
    pub fn from_centers(c: [(f64, f64); 3]) -> StreamLights {
        let band = (c[0].1 * 2.0).clamp(STREAM_TITLE_PT, 80.0);
        StreamLights {
            centers: c,
            band_pt: band,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Stream,
    Synthetic,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Stream => "stream",
            Mode::Synthetic => "synthetic",
        }
    }
}

/// Alles, was Hit-Test und Koordinaten-Umrechnung brauchen (Client-Pixel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub client: (u32, u32),
    pub stream: Option<(u32, u32)>,
    /// Stream-Pixel je Mac-Punkt (Retina 2).
    pub stream_scale: f64,
    /// Client-Pixel je Punkt (DPI / 96).
    pub dpi_scale: f64,
    pub mode: Mode,
    pub lights: StreamLights,
    /// Kanten fuer Groessenaenderung (nicht im Vollbild oder maximiert).
    pub resizable: bool,
}

/// Hoehe der gezeichneten Leiste in Pixeln.
pub fn bar_px(dpi_scale: f64) -> u32 {
    (BAR_PT * dpi_scale).round().max(1.0) as u32
}

/// Mittelpunkt der Ampel `i` (0 = rot) in der gezeichneten Leiste, Pixel.
pub fn light_center(i: usize, scale: f64) -> (f64, f64) {
    let x = LIGHT_LEFT_PT + LIGHT_D_PT / 2.0 + i as f64 * (LIGHT_D_PT + LIGHT_GAP_PT);
    (x * scale, BAR_PT / 2.0 * scale)
}

impl Layout {
    /// Hoehe der gezeichneten Leiste (0 bei echter Titelleiste im Stream).
    pub fn bar(&self) -> u32 {
        match self.mode {
            Mode::Synthetic => bar_px(self.dpi_scale).min(self.client.1.saturating_sub(1)),
            Mode::Stream => 0,
        }
    }

    /// Inhaltsbereich (links, oben, rechts, unten), in den das Bild gestreckt wird.
    pub fn content(&self) -> (i32, i32, i32, i32) {
        (
            0,
            self.bar() as i32,
            self.client.0 as i32,
            self.client.1 as i32,
        )
    }

    /// Client-Pixel -> Stream-Pixel (linear, das Bild fuellt den Inhaltsbereich). Vor dem ersten Bild 1:1.
    /// In der gezeichneten Leiste ergibt das negative y (der Sender verwirft es).
    pub fn to_stream(&self, x: f64, y: f64) -> (f64, f64) {
        let (l, t, r, b) = self.content();
        let Some(st) = self.stream else {
            return (x - l as f64, y - t as f64);
        };
        let (dw, dh) = ((r - l).max(1) as f64, (b - t).max(1) as f64);
        (
            (x - l as f64) * st.0 as f64 / dw,
            (y - t as f64) * st.1 as f64 / dh,
        )
    }

    /// Client-Punkt in Mac-Punkten des gestreamten Fensters.
    fn window_pt(&self, x: f64, y: f64) -> (f64, f64) {
        let (sx, sy) = self.to_stream(x, y);
        let s = self.stream_scale.max(0.1);
        (sx / s, sy / s)
    }

    /// Welche Ampel liegt unter dem Client-Punkt?
    pub fn light_at(&self, x: f64, y: f64) -> Option<TrafficLight> {
        match self.mode {
            Mode::Synthetic => {
                if y < 0.0 || y >= self.bar() as f64 {
                    return None;
                }
                let half = (LIGHT_D_PT + LIGHT_GAP_PT) / 2.0 * self.dpi_scale;
                (0..3).find_map(|i| {
                    let (cx, _) = light_center(i, self.dpi_scale);
                    ((x - cx).abs() <= half).then_some(LIGHTS[i])
                })
            }
            Mode::Stream => {
                self.stream?;
                let (px, py) = self.window_pt(x, y);
                let half = (LIGHT_D_PT + LIGHT_GAP_PT) / 2.0;
                self.lights
                    .centers
                    .iter()
                    .position(|&(cx, cy)| (px - cx).abs() <= half && (py - cy).abs() <= half)
                    .map(|i| LIGHTS[i])
            }
        }
    }

    /// Ueber der Ampelgruppe (Hover zeigt × − ⤢)?
    pub fn in_light_group(&self, x: f64, y: f64) -> bool {
        match self.mode {
            Mode::Synthetic => {
                let s = self.dpi_scale;
                let x0 = (LIGHT_LEFT_PT - HOVER_PAD_PT) * s;
                let x1 = (LIGHT_LEFT_PT + 3.0 * LIGHT_D_PT + 2.0 * LIGHT_GAP_PT + HOVER_PAD_PT) * s;
                (x0..x1).contains(&x) && y >= 0.0 && y < self.bar() as f64
            }
            Mode::Stream => self.light_at(x, y).is_some(),
        }
    }

    /// Titelleiste (gezeichnet oder im Stream), Ampeln eingeschlossen.
    pub fn in_title(&self, x: f64, y: f64) -> bool {
        match self.mode {
            Mode::Synthetic => y >= 0.0 && y < self.bar() as f64,
            Mode::Stream => {
                if self.stream.is_none() {
                    return false;
                }
                let (px, py) = self.window_pt(x, y);
                let w_pt = self.stream.map_or(0.0, |s| s.0 as f64) / self.stream_scale.max(0.1);
                (0.0..self.lights.band_pt).contains(&py) && (0.0..=w_pt).contains(&px)
            }
        }
    }

    /// Zugbereich der Titelleiste ohne Ampeln (HTCAPTION).
    pub fn in_caption(&self, x: f64, y: f64) -> bool {
        self.in_title(x, y) && self.light_at(x, y).is_none()
    }

    /// WM_NCHITTEST: Kanten zuerst, dann Titelleiste ohne Ampeln, sonst Client.
    pub fn hit_test(&self, x: i32, y: i32) -> isize {
        let (cw, ch) = (self.client.0 as i32, self.client.1 as i32);
        if cw <= 0 || ch <= 0 {
            return HTCLIENT;
        }
        if self.resizable {
            let b = (RESIZE_BORDER_PT * self.dpi_scale).round().max(4.0) as i32;
            let (l, r, t, bo) = (x < b, x >= cw - b, y < b, y >= ch - b);
            let hit = match (t, bo, l, r) {
                (true, _, true, _) => Some(HTTOPLEFT),
                (true, _, _, true) => Some(HTTOPRIGHT),
                (_, true, true, _) => Some(HTBOTTOMLEFT),
                (_, true, _, true) => Some(HTBOTTOMRIGHT),
                (_, _, true, _) => Some(HTLEFT),
                (_, _, _, true) => Some(HTRIGHT),
                (true, _, _, _) => Some(HTTOP),
                (_, true, _, _) => Some(HTBOTTOM),
                _ => None,
            };
            if let Some(h) = hit {
                return h;
            }
        }
        if self.in_caption(x as f64, y as f64) {
            return HTCAPTION;
        }
        HTCLIENT
    }

    /// Griffpunkt in Client-Pixeln: Mitte der Titelleiste (fuer `recv.place`).
    pub fn grab_point(&self) -> (i32, i32) {
        let cx = self.client.0 as i32 / 2;
        let cy = match self.mode {
            Mode::Synthetic => self.bar() as i32 / 2,
            Mode::Stream => match self.stream {
                Some(st) => {
                    let (_, t, _, b) = self.content();
                    let band_px = self.lights.band_pt * self.stream_scale;
                    (t as f64 + band_px / 2.0 * (b - t) as f64 / st.1.max(1) as f64).round() as i32
                }
                None => (STREAM_TITLE_PT / 2.0 * self.dpi_scale).round() as i32,
            },
        };
        (cx, cy.max(0))
    }
}

// ---------------------------------------------------------------------------------------------------
// Zeichnen der Titelleiste

/// Was die Leiste zeigt; aendert sich etwas davon, wird sie neu gezeichnet.
#[derive(Clone, Debug, PartialEq)]
pub struct BarSpec {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub title: String,
    pub active: bool,
    pub hover: bool,
    pub dark: bool,
}

/// Farben (0xRRGGBB), macOS 14/15.
pub struct Palette {
    pub bg_active: u32,
    pub bg_inactive: u32,
    pub separator: u32,
    /// Feiner Innenrand oben (Hairline), mit Deckkraft `highlight_alpha`.
    pub highlight: u32,
    pub highlight_alpha: f64,
    pub title_active: u32,
    pub title_inactive: u32,
    pub light_off: u32,
    pub light_off_rim: u32,
    /// Aeusserer 1-px-Rand des Fensters (DWM-Rahmenfarbe).
    pub border: u32,
}

pub const DARK: Palette = Palette {
    bg_active: 0x363636,
    bg_inactive: 0x2B2B2B,
    separator: 0x1C1C1C,
    highlight: 0xFFFFFF,
    highlight_alpha: 0.12,
    title_active: 0xDFDFDF,
    title_inactive: 0x808080,
    light_off: 0x4D4D4F,
    light_off_rim: 0x3E3E40,
    border: 0x0A0A0A,
};

pub const LIGHT: Palette = Palette {
    bg_active: 0xE8E8E8,
    bg_inactive: 0xF6F6F6,
    separator: 0xD2D2D2,
    highlight: 0xFFFFFF,
    highlight_alpha: 0.55,
    title_active: 0x3C3C3C,
    title_inactive: 0xA9A9A9,
    light_off: 0xD3D3D3,
    light_off_rim: 0xBFBFBF,
    border: 0xA8A8A8,
};

pub fn palette(dark: bool) -> &'static Palette {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// Ampelfarben: Fuellung, Ring, Symbol.
pub const LIGHT_COLORS: [(u32, u32, u32); 3] = [
    (0xFF5F57, 0xE0443E, 0x4D0000),
    (0xFEBC2E, 0xDEA123, 0x995700),
    (0x28C840, 0x1AAB29, 0x006500),
];

/// BGRA-Pixel, Zeilen von oben nach unten.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: usize, h: usize, rgb: u32) -> Canvas {
        let mut px = vec![0u8; w * h * 4];
        for p in px.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&bgra(rgb));
        }
        Canvas { w, h, px }
    }

    pub fn get(&self, x: usize, y: usize) -> u32 {
        let i = (y * self.w + x) * 4;
        let p = &self.px[i..i + 4];
        (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32
    }

    fn blend(&mut self, x: usize, y: usize, rgb: u32, a: f64) {
        if x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let a = a.min(1.0);
        let i = (y * self.w + x) * 4;
        let c = bgra(rgb);
        for (k, src) in c.iter().take(3).enumerate() {
            let d = self.px[i + k] as f64;
            self.px[i + k] = (d + (*src as f64 - d) * a).round() as u8;
        }
        self.px[i + 3] = 255;
    }

    fn row(&mut self, y: usize, rgb: u32, a: f64) {
        for x in 0..self.w {
            self.blend(x, y, rgb, a);
        }
    }

    /// Form mit 4×4-Ueberabtastung kantenglatt fuellen; `inside` bekommt Pixel-Koordinaten.
    fn shape(&mut self, bbox: (f64, f64, f64, f64), rgb: u32, inside: impl Fn(f64, f64) -> bool) {
        const N: usize = 4;
        let x0 = bbox.0.floor().max(0.0) as usize;
        let y0 = bbox.1.floor().max(0.0) as usize;
        let x1 = (bbox.2.ceil().max(0.0) as usize).min(self.w);
        let y1 = (bbox.3.ceil().max(0.0) as usize).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut hits = 0;
                for j in 0..N {
                    for i in 0..N {
                        let sx = x as f64 + (i as f64 + 0.5) / N as f64;
                        let sy = y as f64 + (j as f64 + 0.5) / N as f64;
                        if inside(sx, sy) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    self.blend(x, y, rgb, hits as f64 / (N * N) as f64);
                }
            }
        }
    }
}

fn bgra(rgb: u32) -> [u8; 4] {
    [rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8, 255]
}

fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    (qx * qx + qy * qy).sqrt()
}

fn in_triangle(p: (f64, f64), t: [(f64, f64); 3]) -> bool {
    let s = |a: (f64, f64), b: (f64, f64)| (p.0 - b.0) * (a.1 - b.1) - (a.0 - b.0) * (p.1 - b.1);
    let (d1, d2, d3) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Ampel `i` mit Mittelpunkt `c` (Pixel) zeichnen.
fn draw_light(
    cv: &mut Canvas,
    i: usize,
    c: (f64, f64),
    s: f64,
    colored: bool,
    glyph: bool,
    pal: &Palette,
) {
    let r = LIGHT_D_PT / 2.0 * s;
    let rim = (LIGHT_RIM_PT * s).max(0.6);
    let (fill, ring, sym) = if colored {
        LIGHT_COLORS[i]
    } else {
        (pal.light_off, pal.light_off_rim, 0)
    };
    let bb = (c.0 - r - 1.0, c.1 - r - 1.0, c.0 + r + 1.0, c.1 + r + 1.0);
    let d = move |x: f64, y: f64| ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt();
    cv.shape(bb, ring, |x, y| d(x, y) <= r);
    cv.shape(bb, fill, |x, y| d(x, y) <= r - rim);
    if !glyph {
        return;
    }
    let p = |dx: f64, dy: f64| (c.0 + dx * s, c.1 + dy * s);
    match LIGHTS[i] {
        TrafficLight::Close => {
            let t = 1.2 * s / 2.0;
            let (a, b, e, f) = (p(-2.6, -2.6), p(2.6, 2.6), p(-2.6, 2.6), p(2.6, -2.6));
            cv.shape(bb, sym, |x, y| {
                seg_dist((x, y), a, b) <= t || seg_dist((x, y), e, f) <= t
            });
        }
        TrafficLight::Minimize => {
            let t = 1.4 * s / 2.0;
            let (a, b) = (p(-3.4, 0.0), p(3.4, 0.0));
            cv.shape(bb, sym, |x, y| seg_dist((x, y), a, b) <= t);
        }
        TrafficLight::Zoom => {
            let t1 = [p(-3.5, -3.5), p(1.5, -3.5), p(-3.5, 1.5)];
            let t2 = [p(3.5, 3.5), p(-1.5, 3.5), p(3.5, -1.5)];
            cv.shape(bb, sym, |x, y| {
                in_triangle((x, y), t1) || in_triangle((x, y), t2)
            });
        }
    }
}

/// Bereich des Titeltexts (links, rechts) in Pixeln: mittig, Ampeln frei.
pub fn title_span(width: u32, scale: f64) -> (i32, i32) {
    let inset =
        ((LIGHT_LEFT_PT + 3.0 * LIGHT_D_PT + 2.0 * LIGHT_GAP_PT + 12.0) * scale).round() as i32;
    let w = width as i32;
    if 2 * inset >= w {
        (inset.min(w), w)
    } else {
        (inset, w - inset)
    }
}

/// Die ganze Leiste zeichnen (Hintergrund, Hairline, Trennlinie, Titel, Ampeln).
pub fn render(spec: &BarSpec) -> Canvas {
    let pal = palette(spec.dark);
    let (w, h) = (spec.width.max(1) as usize, spec.height.max(1) as usize);
    let bg = if spec.active {
        pal.bg_active
    } else {
        pal.bg_inactive
    };
    let mut cv = Canvas::new(w, h, bg);
    cv.row(0, pal.highlight, pal.highlight_alpha);
    if h > 2 {
        cv.row(h - 1, pal.separator, 1.0);
    }
    if !spec.title.is_empty() {
        let color = if spec.active {
            pal.title_active
        } else {
            pal.title_inactive
        };
        text::draw(
            &mut cv,
            &spec.title,
            title_span(spec.width, spec.scale),
            spec.scale,
            color,
        );
    }
    let colored = spec.active || spec.hover;
    for i in 0..3 {
        draw_light(
            &mut cv,
            i,
            light_center(i, spec.scale),
            spec.scale,
            colored,
            spec.hover,
            pal,
        );
    }
    cv
}

/// Titeltext per GDI (Segoe UI Semibold 13 pt, Graustufen-Glaettung) in die Leiste.
mod text {
    use super::Canvas;
    use windows::core::w;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::*;

    pub fn draw(cv: &mut Canvas, title: &str, span: (i32, i32), scale: f64, rgb: u32) {
        if span.1 <= span.0 {
            return;
        }
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return;
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: cv.w as i32,
                    biHeight: -(cv.h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                let _ = DeleteDC(dc);
                return;
            };
            if bits.is_null() {
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(dc);
                return;
            }
            std::ptr::copy_nonoverlapping(cv.px.as_ptr(), bits as *mut u8, cv.px.len());
            let old_bmp = SelectObject(dc, bmp.into());
            let font = CreateFontW(
                -((13.0 * scale).round() as i32),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                w!("Segoe UI"),
            );
            let old_font = SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            let bgr = ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF);
            SetTextColor(dc, windows::Win32::Foundation::COLORREF(bgr));
            let mut r = RECT {
                left: span.0,
                top: 0,
                right: span.1,
                bottom: cv.h as i32,
            };
            let mut wide: Vec<u16> = title.encode_utf16().collect();
            DrawTextW(
                dc,
                &mut wide,
                &mut r,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            let _ = GdiFlush();
            std::ptr::copy_nonoverlapping(bits as *const u8, cv.px.as_mut_ptr(), cv.px.len());
            SelectObject(dc, old_font);
            SelectObject(dc, old_bmp);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
            for p in cv.px.as_chunks_mut::<4>().0 {
                p[3] = 255;
            }
        }
    }
}

/// 32-bit-BMP (unten nach oben) fuer `--chrome-preview`.
pub fn bmp(cv: &Canvas) -> Vec<u8> {
    let (w, h) = (cv.w as u32, cv.h as u32);
    let data = w * h * 4;
    let mut out = Vec::with_capacity(54 + data as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..cv.h).rev() {
        out.extend_from_slice(&cv.px[y * cv.w * 4..(y + 1) * cv.w * 4]);
    }
    out
}

/// `--chrome-preview <ordner> [--width PT] [--scale S] [--title T]`: alle Zustaende als BMP schreiben
/// (dunkel/hell × aktiv/inaktiv/Hover), zum 1:1-Vergleich mit einem Mac-Screenshot.
pub fn write_previews(
    dir: &std::path::Path,
    width_pt: f64,
    scale: f64,
    title: &str,
) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut names = Vec::new();
    for dark in [true, false] {
        for (state, active, hover) in [
            ("active", true, false),
            ("inactive", false, false),
            ("hover", false, true),
        ] {
            let spec = BarSpec {
                width: (width_pt * scale).round().max(1.0) as u32,
                height: bar_px(scale),
                scale,
                title: title.to_owned(),
                active,
                hover,
                dark,
            };
            let name = format!(
                "titlebar-{}-{}-{}x.bmp",
                if dark { "dark" } else { "light" },
                state,
                scale
            );
            std::fs::write(dir.join(&name), bmp(&render(&spec)))?;
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(mode: Mode) -> Layout {
        Layout {
            client: (800, 628),
            stream: Some((1600, 1200)),
            stream_scale: 2.0,
            dpi_scale: 1.0,
            mode,
            lights: StreamLights::default(),
            resizable: true,
        }
    }

    #[test]
    fn synthetic_bar_and_stretch_fill() {
        let l = layout(Mode::Synthetic);
        assert_eq!(l.bar(), 28);
        assert_eq!(l.content(), (0, 28, 800, 628));
        // Linke obere Ecke des Inhalts = Stream (0, 0), rechte untere = Streamgroesse.
        assert_eq!(l.to_stream(0.0, 28.0), (0.0, 0.0));
        assert_eq!(l.to_stream(800.0, 628.0), (1600.0, 1200.0));
        // In der Leiste: negativ (nicht an den Mac).
        assert!(l.to_stream(400.0, 10.0).1 < 0.0);
        // Gestreckt statt Letterbox: anderes Seitenverhaeltnis, trotzdem Ecke auf Ecke.
        let mut l2 = l;
        l2.client = (1000, 528);
        assert_eq!(l2.to_stream(1000.0, 528.0), (1600.0, 1200.0));
    }

    #[test]
    fn synthetic_lights_geometry() {
        // 12 pt Durchmesser, 8 pt Abstand, 20 pt vom linken Rand, mittig in 28 pt.
        assert_eq!(light_center(0, 1.0), (26.0, 14.0));
        assert_eq!(light_center(1, 1.0), (46.0, 14.0));
        assert_eq!(light_center(2, 1.0), (66.0, 14.0));
        assert_eq!(light_center(0, 2.0), (52.0, 28.0));
        let l = layout(Mode::Synthetic);
        assert_eq!(l.light_at(26.0, 14.0), Some(TrafficLight::Close));
        assert_eq!(l.light_at(46.0, 14.0), Some(TrafficLight::Minimize));
        assert_eq!(l.light_at(66.0, 14.0), Some(TrafficLight::Zoom));
        assert_eq!(l.light_at(200.0, 14.0), None);
        assert_eq!(l.light_at(26.0, 40.0), None);
        assert!(l.in_light_group(20.0, 14.0) && !l.in_light_group(120.0, 14.0));
        assert_eq!(l.hit_test(400, 14), HTCAPTION);
        assert_eq!(l.hit_test(46, 14), HTCLIENT);
        assert_eq!(l.hit_test(400, 300), HTCLIENT);
    }

    #[test]
    fn stream_mode_uses_mac_lights_and_draws_nothing() {
        let l = layout(Mode::Stream);
        assert_eq!(l.bar(), 0);
        // Client 800x628 auf Stream 1600x1200 (2x): Client-Pixel (14, 14*628/600) = Mac-Punkt (14, 14).
        let y = 14.0 * 628.0 / 600.0;
        assert_eq!(l.light_at(14.0, y), Some(TrafficLight::Close));
        assert_eq!(l.light_at(54.0, y), Some(TrafficLight::Zoom));
        assert_eq!(l.hit_test(400, 10), HTCAPTION);
        assert_eq!(l.hit_test(400, 100), HTCLIENT);
        // Toolbar-Fenster: Ampeln tiefer, Zugbereich hoeher.
        let mut t = l;
        t.lights = StreamLights::from_centers([(20.0, 26.0), (40.0, 26.0), (60.0, 26.0)]);
        assert_eq!(t.lights.band_pt, 52.0);
        assert_eq!(t.hit_test(400, 50), HTCAPTION);
        assert_eq!(
            t.light_at(20.0, 26.0 * 628.0 / 600.0),
            Some(TrafficLight::Close)
        );
    }

    #[test]
    fn edges_resize_before_title() {
        let l = layout(Mode::Synthetic);
        assert_eq!(l.hit_test(2, 300), HTLEFT);
        assert_eq!(l.hit_test(797, 300), HTRIGHT);
        assert_eq!(l.hit_test(400, 626), HTBOTTOM);
        assert_eq!(l.hit_test(400, 2), HTTOP);
        assert_eq!(l.hit_test(2, 2), HTTOPLEFT);
        assert_eq!(l.hit_test(797, 626), HTBOTTOMRIGHT);
        let mut fixed = l;
        fixed.resizable = false;
        assert_eq!(fixed.hit_test(2, 300), HTCLIENT);
    }

    #[test]
    fn grab_point_is_title_middle() {
        assert_eq!(layout(Mode::Synthetic).grab_point(), (400, 14));
        assert_eq!(layout(Mode::Stream).grab_point(), (400, 15));
    }

    fn spec(active: bool, hover: bool, dark: bool) -> BarSpec {
        BarSpec {
            width: 400,
            height: 28,
            scale: 1.0,
            title: String::new(),
            active,
            hover,
            dark,
        }
    }

    #[test]
    fn render_colors() {
        let cv = render(&spec(true, false, true));
        assert_eq!((cv.w, cv.h), (400, 28));
        // Mitte der Ampeln: Originalfarben.
        assert_eq!(cv.get(26, 14), 0xFF5F57);
        assert_eq!(cv.get(46, 14), 0xFEBC2E);
        assert_eq!(cv.get(66, 14), 0x28C840);
        // Hintergrund, Trennlinie unten.
        assert_eq!(cv.get(200, 14), DARK.bg_active);
        assert_eq!(cv.get(200, 27), DARK.separator);
        // Ring dunkler als die Fuellung.
        assert_ne!(cv.get(26, 8), 0xFF5F57);
        // Inaktiv: grau.
        let off = render(&spec(false, false, true));
        assert_eq!(off.get(26, 14), DARK.light_off);
        assert_eq!(off.get(200, 14), DARK.bg_inactive);
        let light = render(&spec(true, false, false));
        assert_eq!(light.get(200, 14), LIGHT.bg_active);
    }

    #[test]
    fn hover_shows_symbols_even_when_inactive() {
        let cv = render(&spec(false, true, true));
        // Farbig, und in der Mitte liegt das Symbol (dunkler als die Fuellung).
        assert_ne!(cv.get(26, 14), DARK.light_off);
        assert_ne!(cv.get(26, 14), 0xFF5F57);
        assert_ne!(cv.get(46, 14), 0xFEBC2E);
        // Am Rand der Fuellung (kein Symbol) die Originalfarbe.
        assert_eq!(cv.get(26, 9), 0xFF5F57);
        // Zoom: Dreiecke in den Ecken oben links / unten rechts, Mitte frei.
        assert_eq!(cv.get(66, 14), 0x28C840);
        assert_ne!(cv.get(63, 11), 0x28C840);
    }

    #[test]
    fn bmp_header() {
        let b = bmp(&render(&spec(true, false, true)));
        assert_eq!(&b[..2], b"BM");
        assert_eq!(b.len(), 54 + 400 * 28 * 4);
    }
}
