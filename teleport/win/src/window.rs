//! Proxy-Fenster: ein Top-Level-Fenster je `tid`, Per-Monitor-DPI-Aware V2.
//!
//! Alle Fenster gehoeren dem Hauptthread (Nachrichtenschleife). Gerendert wird im Empfaenger-Thread;
//! die Fensterprozedur meldet ihm Groessenaenderungen nur ueber atomare Werte. Schliessen schickt
//! eine Thread-Nachricht an den Hauptthread, der dann `proxy.closed` meldet und aufraeumt.
//!
//! C05: Eingabe (Maus, Rad, Tasten, Text/IME) geht als `proxy.input` raus, Aktivierung als `proxy.focus`,
//! Groessenaenderungen durch den Nutzer als `proxy.resize` (entprellt). Dazu Taskleisten-Identitaet
//! (AppUserModelID je App, Icon) und randloses Vollbild (Strg+Alt+Shift+F schaltet um).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use windows::core::BOOL;
use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, GetStockObject, MonitorFromPoint, MonitorFromWindow,
    ScreenToClient, ValidateRect, BLACK_BRUSH, HBRUSH, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, GetKeyState, GetKeyboardLayout, ReleaseCapture,
    SetCapture, ToUnicodeEx, VK_CAPITAL, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::drag::{self, Follow, Throttle};
use crate::input::{self, Button, Debounce, DebounceOut, InputState};
use crate::ipc::{self, Out};

/// Hauptthread-Nachrichten (PostThreadMessage, hwnd = 0).
pub const WM_APP_CMD: u32 = WM_APP + 1;
pub const WM_APP_EOF: u32 = WM_APP + 2;
/// wparam = tid
pub const WM_APP_CLOSED: u32 = WM_APP + 3;
/// wparam = tid: Empfaenger-Thread ist mit einem Fehler beendet.
pub const WM_APP_FAILED: u32 = WM_APP + 4;

const CLASS: PCWSTR = w!("BeamerTeleportProxy");
/// Rahmenlos wie ein Mac-Fenster auf dem Desktop; WS_THICKFRAME für Kanten-Resize, Min/Max für Ampel gelb/grün.
const STYLE: WINDOW_STYLE = WINDOW_STYLE(
    WS_POPUP.0 | WS_THICKFRAME.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0 | WS_CLIPCHILDREN.0,
);
const EX_STYLE: WINDOW_EX_STYLE = WS_EX_APPWINDOW;

/// Gemeldete Groesse fuer `proxy.resize`: Client-Pixel und DPI (Punkte = Pixel * 96 / DPI).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeVal {
    pub px: (u32, u32),
    pub dpi: u32,
}

/// Fensterlage vor dem Vollbild (zum Zuruecksetzen).
struct Windowed {
    style: WINDOW_STYLE,
    rect: RECT,
}

const RESIZE_TIMER: usize = 1;
/// Kanten-Drag: der Maus folgen (`recv.place` mit `dragging`).
const FOLLOW_TIMER: usize = 2;
const FOLLOW_TICK_MS: u32 = 8;
/// `proxy.resize` hoechstens 10/s.
const RESIZE_EVERY: Duration = Duration::from_millis(100);
const VK_F: u32 = 0x46;

/// Zustand, den Fensterprozedur (Hauptthread) und Empfaenger-Thread teilen.
pub struct Shared {
    pub tid: u32,
    main_thread: u32,
    /// Client-Groesse in physischen Pixeln, (w << 32) | h.
    size: AtomicU64,
    /// Groesse des dekodierten Bildes (Stream-Pixel), (w << 32) | h; 0 = noch kein Bild.
    stream: AtomicU64,
    /// Groesse geaendert oder Neuzeichnen noetig.
    pub dirty: AtomicBool,
    pub dpi: AtomicU32,
    out: Out,
    input: Mutex<InputState>,
    resize: Mutex<Debounce<ResizeVal>>,
    /// Groessenaenderung kommt von uns (recv.update): kein `proxy.resize` zurueck.
    programmatic: AtomicBool,
    windowed: Mutex<Option<Windowed>>,
    /// Zuletzt gemeldeter Fokus (WM_ACTIVATE kommt auch ohne Wechsel, gemeldet wird nur der Wechsel).
    focused: AtomicBool,
    /// Kanten-Drag: Fenster folgt der Maus, bis die linke Taste los ist.
    follow: Mutex<Option<Follow>>,
    /// Nutzer zieht das Fenster an der Titelleiste (modale Verschiebe-Schleife).
    moving: AtomicBool,
    drag_throttle: Mutex<Throttle>,
}

impl Shared {
    pub fn size(&self) -> (u32, u32) {
        let v = self.size.load(Ordering::Acquire);
        ((v >> 32) as u32, v as u32)
    }

    fn set_size(&self, w: u32, h: u32) {
        self.size
            .store(((w as u64) << 32) | h as u64, Ordering::Release);
        self.dirty.store(true, Ordering::Release);
    }

    /// Vom Empfaenger-Thread: Groesse des zuletzt dekodierten Bildes (fuer die Eingabe-Umrechnung).
    pub fn set_stream(&self, w: u32, h: u32) {
        self.stream
            .store(((w as u64) << 32) | h as u64, Ordering::Release);
    }

    pub fn stream(&self) -> Option<(u32, u32)> {
        let v = self.stream.load(Ordering::Acquire);
        (v != 0).then_some(((v >> 32) as u32, v as u32))
    }

    fn send_input(&self, v: Value) {
        self.out.send(&ipc::event(
            "proxy.input",
            json!({"tid": self.tid, "input": v}),
        ));
    }

    /// Client-Pixel -> Stream-Pixel; vor dem ersten Bild 1:1.
    fn to_stream(&self, x: i32, y: i32) -> (f64, f64) {
        match self.stream() {
            Some(st) => input::to_stream(x as f64, y as f64, self.size(), st),
            None => (x as f64, y as f64),
        }
    }
}

/// Einmal pro Prozess, vor dem ersten Fenster.
pub fn init_process_dpi() {
    unsafe {
        // Schlaegt fehl, wenn das Manifest schon etwas setzt; dann gilt das.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn register_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

/// Physische Client-Groesse fuer `w`×`h` Punkte bei `dpi` (96 = 100 %).
pub fn client_px(w_pt: f64, h_pt: f64, dpi: u32) -> (u32, u32) {
    let s = dpi.max(1) as f64 / 96.0;
    (
        (w_pt * s).round().max(1.0) as u32,
        (h_pt * s).round().max(1.0) as u32,
    )
}

/// Auf die Arbeitsflaeche (abzueglich Rahmen) begrenzen, Seitenverhaeltnis bleibt.
pub fn fit_into(c: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    if c.0 <= max.0 && c.1 <= max.1 {
        return c;
    }
    let s = (max.0 as f64 / c.0 as f64).min(max.1 as f64 / c.1 as f64);
    (
        ((c.0 as f64 * s).floor() as u32).max(1),
        ((c.1 as f64 * s).floor() as u32).max(1),
    )
}

fn monitor_dpi(m: HMONITOR) -> u32 {
    let (mut x, mut y) = (96u32, 96u32);
    unsafe {
        if GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_err() {
            return 96;
        }
    }
    x
}

fn work_area(m: HMONITOR) -> RECT {
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(m, &mut mi);
    }
    mi.rcWork
}

/// Abgerundete Ecken und Schatten (DWM); nach Stilwechsel (Vollbild) erneut aufrufen.
fn apply_proxy_chrome(hwnd: HWND) {
    unsafe {
        let round = DWMWCP_ROUND.0 as u32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        );
        let margins = windows::Win32::Graphics::Dwm::MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

fn frame_rect(cw: u32, ch: u32, dpi: u32) -> RECT {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: cw as i32,
        bottom: ch as i32,
    };
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut r, STYLE, false, EX_STYLE, dpi);
    }
    r
}

/// Aussengroesse fuer `w`×`h` Punkte auf einem Monitor mit `dpi`, begrenzt auf `work`.
fn outer_size(w_pt: f64, h_pt: f64, dpi: u32, work: &RECT) -> (i32, i32, (u32, u32)) {
    let f = frame_rect(0, 0, dpi);
    let frame_w = (f.right - f.left) as u32;
    let frame_h = (f.bottom - f.top) as u32;
    let max = (
        ((work.right - work.left) as u32)
            .saturating_sub(frame_w)
            .max(64),
        ((work.bottom - work.top) as u32)
            .saturating_sub(frame_h)
            .max(64),
    );
    let c = fit_into(client_px(w_pt, h_pt, dpi), max);
    let r = frame_rect(c.0, c.1, dpi);
    (r.right - r.left, r.bottom - r.top, c)
}

/// Monitor fuer ein neues Fenster: der unter dem Mauszeiger, ausser dort liegt ein Vollbild-Programm im
/// Vordergrund (Spiel, Video); dann ein anderer Monitor. Gibt es keinen anderen, bleibt es beim Monitor
/// unter dem Mauszeiger (`true` = das Fenster liegt dort wahrscheinlich verdeckt).
pub fn choose_monitor<M: Copy + PartialEq>(
    cursor: M,
    all: &[M],
    fullscreen: Option<M>,
) -> (M, bool) {
    if fullscreen != Some(cursor) {
        return (cursor, false);
    }
    match all.iter().copied().find(|m| Some(*m) != fullscreen) {
        Some(m) => (m, false),
        None => (cursor, true),
    }
}

/// Monitor, den das Vordergrundfenster ganz bedeckt (Vollbild), falls es eins ist.
fn fullscreen_monitor() -> Option<HMONITOR> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == GetDesktopWindow() || fg == GetShellWindow() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(fg, &mut r).ok()?;
        let m = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
        if m.is_invalid() {
            return None;
        }
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(m, &mut mi);
        let mr = mi.rcMonitor;
        (r.left <= mr.left && r.top <= mr.top && r.right >= mr.right && r.bottom >= mr.bottom)
            .then_some(m)
    }
}

fn all_monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        (*(lp.0 as *mut Vec<HMONITOR>)).push(m);
        true.into()
    }
    let mut v: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

pub struct ProxyWindow {
    pub hwnd: HWND,
    pub shared: Arc<Shared>,
}

/// Fenster auf dem Monitor unter dem Mauszeiger erzeugen, `index` versetzt mehrere Fenster.
#[allow(clippy::too_many_arguments)]
pub fn create(
    tid: u32,
    title: &str,
    w_pt: f64,
    h_pt: f64,
    main_thread: u32,
    index: usize,
    out: Out,
    fullscreen: bool,
) -> Result<ProxyWindow> {
    register_class();
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let cursor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mon, covered) = choose_monitor(cursor, &all_monitors(), fullscreen_monitor());
        let dpi = monitor_dpi(mon);
        let work = work_area(mon);
        let (ow, oh, c) = outer_size(w_pt, h_pt, dpi, &work);
        let step = (32 * dpi / 96) as i32 * (index % 8) as i32;
        let x = work.left + ((work.right - work.left - ow) / 2).max(0) + step;
        let y = work.top + ((work.bottom - work.top - oh) / 2).max(0) + step;
        let shared = Arc::new(Shared {
            tid,
            main_thread,
            size: AtomicU64::new(((c.0 as u64) << 32) | c.1 as u64),
            stream: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            dpi: AtomicU32::new(dpi),
            out,
            input: Mutex::new(InputState::default()),
            resize: Mutex::new(Debounce::default()),
            // Erzeugen und erstes Anzeigen melden nichts (Python kennt die Groesse aus recv.open).
            programmatic: AtomicBool::new(true),
            windowed: Mutex::new(None),
            focused: AtomicBool::new(false),
            follow: Mutex::new(None),
            moving: AtomicBool::new(false),
            drag_throttle: Mutex::new(Throttle::new(drag::DRAG_EVERY)),
        });
        let raw = Arc::into_raw(shared.clone());
        let hinst = GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            EX_STYLE,
            CLASS,
            &HSTRING::from(title),
            STYLE,
            x,
            y,
            ow,
            oh,
            None,
            None,
            Some(hinst.into()),
            Some(raw as *const _),
        );
        let hwnd = match hwnd {
            Ok(h) => h,
            Err(e) => {
                drop(Arc::from_raw(raw));
                return Err(e);
            }
        };
        // Doppelklick-Zeit und -Rechteck des Systems fuer den Klickzaehler (`n`).
        if let Some(s) = shared_of(hwnd) {
            let mut st = s.input.lock().unwrap();
            st.dbl_time = Duration::from_millis(GetDoubleClickTime() as u64);
            st.dbl_slop = (
                GetSystemMetrics(SM_CXDOUBLECLK) as f64 / 2.0,
                GetSystemMetrics(SM_CYDOUBLECLK) as f64 / 2.0,
            );
        }
        if fullscreen {
            // Fuer den Vollbild-Stream (mode "display"): randlos auf dem gewaehlten Monitor.
            set_fullscreen(hwnd, true, Some(mon));
        } else {
            apply_proxy_chrome(hwnd);
        }
        // Sichtbar und oben, aber ohne Aktivierung: kein Fokusklau (Tastatur bleibt beim Nutzer).
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        if covered {
            // Nur ein Monitor und dort laeuft ein Vollbild-Programm: in der Taskleiste blinken.
            eprintln!("beamer-teleport: tid {tid} Fenster liegt unter einem Vollbild-Programm, Taskleiste blinkt");
            let fi = FLASHWINFO {
                cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                hwnd,
                dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
                uCount: 0,
                dwTimeout: 0,
            };
            let _ = FlashWindowEx(&fi);
        }
        shared.programmatic.store(false, Ordering::Release);
        Ok(ProxyWindow { hwnd, shared })
    }
}

impl ProxyWindow {
    pub fn set_title(&self, title: &str) {
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title));
        }
    }

    /// Neue Groesse in Punkten (recv.update, also die Antwort auf ein `proxy.resize`). Meldet selbst kein
    /// `proxy.resize`. Im Vollbild bleibt das Fenster, wie es ist (das Bild wird skaliert).
    pub fn resize_points(&self, w_pt: f64, h_pt: f64) {
        if self.shared.windowed.lock().unwrap().is_some() {
            return;
        }
        self.shared.programmatic.store(true, Ordering::Release);
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd).max(96);
            let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let (ow, oh, _) = outer_size(w_pt, h_pt, dpi, &work_area(mon));
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                ow,
                oh,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.shared.programmatic.store(false, Ordering::Release);
    }

    pub fn is_fullscreen(&self) -> bool {
        self.shared.windowed.lock().unwrap().is_some()
    }

    pub fn set_fullscreen(&self, on: bool) {
        set_fullscreen(self.hwnd, on, None);
    }

    /// `recv.place` (§2.1): Titelleiste unter (x, y) Bildschirm-Punkte legen, auf die Arbeitsflaeche geklemmt.
    /// Mit `dragging` folgt das Fenster danach der echten Maus, bis die linke Taste los ist, und meldet
    /// `proxy.drag` (<= 30/s, am Ende `dragging: false`). Im Vollbild bleibt das Fenster, wo es ist.
    pub fn place(&self, x: f64, y: f64, dragging: bool) {
        if self.is_fullscreen() {
            eprintln!(
                "beamer-teleport: tid {} recv.place im Vollbild ignoriert",
                self.shared.tid
            );
            return;
        }
        let hwnd = self.hwnd;
        unsafe {
            let mons = monitors();
            let px = drag::points_to_px((x, y), &mons);
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let outer = (wr.right - wr.left, wr.bottom - wr.top);
            let grab = match self.shared.stream() {
                Some(st) => input::title_grab_client(self.shared.size(), st),
                None => drag::title_grab(outer.0, 0),
            };
            let mon = MonitorFromPoint(POINT { x: px.0, y: px.1 }, MONITOR_DEFAULTTONEAREST);
            let w = work_area(mon);
            let (left, top) =
                drag::place_origin(px, outer, grab, (w.left, w.top, w.right, w.bottom));
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                left,
                top,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            if dragging {
                *self.shared.follow.lock().unwrap() = Some(Follow::new(grab, Instant::now()));
                SetTimer(Some(hwnd), FOLLOW_TIMER, FOLLOW_TICK_MS, None);
                emit_drag(&self.shared, cursor_px(), true, true);
            }
        }
    }

    pub fn destroy(&self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Alle Monitore mit Rechteck (physische Pixel) und DPI.
fn monitors() -> Vec<drag::Mon> {
    all_monitors()
        .into_iter()
        .map(|m| {
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            unsafe {
                let _ = GetMonitorInfoW(m, &mut mi);
            }
            let r = mi.rcMonitor;
            drag::Mon {
                rect: (r.left, r.top, r.right, r.bottom),
                dpi: monitor_dpi(m),
            }
        })
        .collect()
}

fn cursor_px() -> (i32, i32) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

/// Linke (primaere) Maustaste unten, auch bei vertauschten Tasten.
fn primary_button_down() -> bool {
    unsafe {
        let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        GetAsyncKeyState(vk.0 as i32) < 0
    }
}

/// `proxy.drag` (§2.1): `x`/`y` in Bildschirm-Punkten, dazu `px_x`/`px_y` (physische Pixel des
/// virtuellen Desktops, Zusatzfelder) fuer Pythons Kantenpruefung. `force`: ohne Drosselung (Ende des Zugs).
fn emit_drag(s: &Shared, px: (i32, i32), dragging: bool, force: bool) {
    if !force && !s.drag_throttle.lock().unwrap().ready(Instant::now()) {
        return;
    }
    let (x, y) = drag::px_to_points(px, &monitors());
    s.out.send(&ipc::event(
        "proxy.drag",
        json!({"tid": s.tid, "x": x, "y": y, "dragging": dragging, "px_x": px.0, "px_y": px.1}),
    ));
}

/// Ein Schritt der Mausfolge (Timer).
fn follow_tick(hwnd: HWND, s: &Shared) {
    let cur = cursor_px();
    let down = primary_button_down();
    let step = s
        .follow
        .lock()
        .unwrap()
        .as_mut()
        .map(|f| f.tick(cur, down, Instant::now()));
    let Some(((left, top), ended)) = step else {
        unsafe {
            let _ = KillTimer(Some(hwnd), FOLLOW_TIMER);
        }
        return;
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            left,
            top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        if ended {
            let _ = KillTimer(Some(hwnd), FOLLOW_TIMER);
        }
    }
    if ended {
        *s.follow.lock().unwrap() = None;
    }
    emit_drag(s, cur, !ended, ended);
}

/// Randloses Vollbild an/aus. `mon`: Zielmonitor (sonst der des Fensters).
fn set_fullscreen(hwnd: HWND, on: bool, mon: Option<HMONITOR>) {
    unsafe {
        let Some(s) = shared_of(hwnd) else { return };
        let mut saved = s.windowed.lock().unwrap();
        if on == saved.is_some() {
            return;
        }
        if on {
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
            *saved = Some(Windowed { style, rect });
            drop(saved);
            let m = mon.unwrap_or_else(|| MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(m, &mut mi);
            let r = mi.rcMonitor;
            SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                ((style.0 & !(WS_OVERLAPPEDWINDOW.0 | STYLE.0)) | WS_POPUP.0) as isize,
            );
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        } else {
            let w = saved.take().unwrap();
            drop(saved);
            SetWindowLongPtrW(hwnd, GWL_STYLE, w.style.0 as isize);
            let r = w.rect;
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            apply_proxy_chrome(hwnd);
        }
        eprintln!(
            "beamer-teleport: tid {} Vollbild {}",
            s.tid,
            if on { "an" } else { "aus" }
        );
    }
}

/// `proxy.resize` anbieten (entprellt); `final_` = Ziehen beendet, wartenden Wert sofort senden.
fn offer_resize(hwnd: HWND, s: &Shared, final_: bool) {
    let now = Instant::now();
    let val = ResizeVal {
        px: s.size(),
        dpi: s.dpi.load(Ordering::Relaxed).max(1),
    };
    let mut d = s.resize.lock().unwrap();
    let send = if final_ {
        let r = match d.offer(val, now, Duration::ZERO) {
            DebounceOut::Send(v) => Some(v),
            _ => d.flush(now),
        };
        unsafe {
            let _ = KillTimer(Some(hwnd), RESIZE_TIMER);
        }
        r
    } else {
        match d.offer(val, now, RESIZE_EVERY) {
            DebounceOut::Send(v) => Some(v),
            DebounceOut::Later(wait) => {
                unsafe {
                    SetTimer(
                        Some(hwnd),
                        RESIZE_TIMER,
                        wait.as_millis().max(1) as u32,
                        None,
                    );
                }
                None
            }
            DebounceOut::Nothing => None,
        }
    };
    drop(d);
    if let Some(v) = send {
        send_resize(s, v);
    }
}

fn send_resize(s: &Shared, v: ResizeVal) {
    let (w, h) = input::px_to_points(v.px, v.dpi);
    // w/h in Punkten (Vertrag); px_w/px_h/dpi als Zusatzfelder (v1.3), damit der Sender so kodieren kann,
    // dass ein Stream-Pixel ein Bildschirm-Pixel ist.
    s.out.send(&ipc::event(
        "proxy.resize",
        json!({"tid": s.tid, "w": w, "h": h, "px_w": v.px.0, "px_h": v.px.1, "dpi": v.dpi}),
    ));
}

fn client_point(hwnd: HWND, screen_x: i32, screen_y: i32) -> (i32, i32) {
    let mut pt = POINT {
        x: screen_x,
        y: screen_y,
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut pt);
    }
    (pt.x, pt.y)
}

fn client_in_stream_title_bar(s: &Shared, client_x: i32, client_y: i32) -> bool {
    match s.stream() {
        Some(st) => input::in_stream_title_bar(client_x as f64, client_y as f64, s.size(), st),
        None => false,
    }
}

fn toggle_maximize(hwnd: HWND) {
    unsafe {
        if IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
    }
}

/// Ampel-Klick lokal (rot → zurück auf den Mac, gelb minimiert, grün maximiert).
fn on_traffic_light(hwnd: HWND, s: &Shared, light: input::TrafficLight) {
    match light {
        input::TrafficLight::Close => {
            let _ = PostThreadMessageW(
                s.main_thread,
                WM_APP_CLOSED,
                WPARAM(s.tid as usize),
                LPARAM(0),
            );
        }
        input::TrafficLight::Minimize => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
        }
        input::TrafficLight::Zoom => toggle_maximize(hwnd),
    }
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    (
        (lp.0 & 0xffff) as i16 as i32,
        ((lp.0 >> 16) & 0xffff) as i16 as i32,
    )
}

fn key_down(vk: u32) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

/// Zeichen einer Zeichentaste: nur Shift und Feststelltaste zaehlen (wie win_app/capture_win.py),
/// Strg/Alt nicht. Mit `altgr` zusaetzlich Strg+Alt (so erzeugt Windows AltGr-Zeichen wie "@").
/// Tote Tasten (Akzente) liefern nichts; ihr Ergebnis kommt als WM_CHAR -> `text`.
/// `shift` ist der Tastaturzustand oder eine von uns selbst gesehene Shift-Taste (PostMessage
/// setzt `GetKeyState` nicht).
fn key_char(vk: u32, scan: u32, altgr: bool, shift: bool) -> Option<String> {
    unsafe {
        let mut state = [0u8; 256];
        if shift {
            state[input::VK_SHIFT as usize] = 0x80;
        }
        if altgr {
            for k in [input::VK_CONTROL, input::VK_MENU, 0xA2, 0xA5] {
                state[k as usize] = 0x80;
            }
        }
        if GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 {
            state[VK_CAPITAL.0 as usize] = 0x01;
        }
        let mut buf = [0u16; 8];
        // Flag 4: Tastaturzustand (tote Tasten) nicht veraendern, sonst stoert es das echte WM_CHAR.
        let n = ToUnicodeEx(vk, scan, &state, &mut buf, 4, Some(GetKeyboardLayout(0)));
        if n <= 0 {
            return None;
        }
        let s = String::from_utf16(&buf[..n as usize]).ok()?;
        let mut cs = s.chars();
        let c = cs.next()?;
        (cs.next().is_none() && !c.is_control()).then(|| c.to_string())
    }
}

/// AltGr: Windows schickt ein kuenstliches linkes Strg (nicht erweitert) mit derselben Nachrichtenzeit
/// unmittelbar vor dem rechten Alt (erweitert). Auf Layouts ohne AltGr (US) fehlt dieses Strg.
fn is_altgr_fake_ctrl(hwnd: HWND) -> bool {
    unsafe {
        let mut m = MSG::default();
        if !PeekMessageW(&mut m, Some(hwnd), WM_KEYFIRST, WM_KEYLAST, PM_NOREMOVE).as_bool() {
            return false;
        }
        (m.message == WM_KEYDOWN || m.message == WM_SYSKEYDOWN)
            && m.wParam.0 as u32 == input::VK_MENU
            && (m.lParam.0 >> 24) & 1 == 1
            && (m.time as i64 - GetMessageTime() as i64).abs() <= 1
    }
}

/// Entfernt das WM_CHAR, das TranslateMessage zu dieser Taste schon in die Queue gelegt hat (gleicher
/// Scan-Code im lParam): die Taste wurde als `key` bzw. AltGr-`text` gemeldet, das Zeichen soll nicht noch
/// einmal als `text` kommen. Fremde Zeichen (IME, zusammengesetzte Akzente) bleiben unberuehrt.
/// Ohne gehaltene Sperren aufrufen: PeekMessage kann gesendete Nachrichten zustellen.
fn drop_translated_char(hwnd: HWND, scan: u32) {
    unsafe {
        let mut m = MSG::default();
        if PeekMessageW(&mut m, Some(hwnd), WM_CHAR, WM_CHAR, PM_NOREMOVE).as_bool()
            && ((m.lParam.0 >> 16) & 0xff) as u32 == scan
        {
            let _ = PeekMessageW(&mut m, Some(hwnd), WM_CHAR, WM_CHAR, PM_REMOVE);
        }
    }
}

fn on_key(hwnd: HWND, s: &Shared, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let mut drop_char = false;
    let r = on_key_inner(hwnd, s, msg, wp, lp, &mut drop_char);
    if drop_char {
        drop_translated_char(hwnd, ((lp.0 >> 16) & 0xff) as u32);
    }
    r
}

fn on_key_inner(
    hwnd: HWND,
    s: &Shared,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    drop_char: &mut bool,
) -> Option<LRESULT> {
    let vk = wp.0 as u32;
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let scan = ((lp.0 >> 16) & 0xff) as u32;
    let extended = (lp.0 >> 24) & 1 == 1;
    if vk == input::VK_PROCESSKEY || input::is_system_key(vk) {
        return None; // IME bzw. Windows-Taste: Windows ueberlassen
    }
    // Windows-Taste mit anderen Tasten (Win+...) bleibt bei Windows (v1.4 §3): kein `down` weitergeben.
    // Das `up` einer vorher gedrueckten Taste laeuft normal weiter.
    if down && (key_down(input::VK_LWIN) || key_down(input::VK_RWIN)) {
        return None;
    }
    // Strg+Alt+Shift+F: Vollbild umschalten, nicht weitergeben.
    if vk == VK_F
        && key_down(input::VK_CONTROL)
        && key_down(input::VK_MENU)
        && key_down(input::VK_SHIFT)
    {
        if down && (lp.0 >> 30) & 1 == 0 {
            let on = s.windowed.lock().unwrap().is_none();
            set_fullscreen(hwnd, on, None);
        }
        *drop_char = true;
        return Some(LRESULT(0));
    }
    // Alt+F4 schliesst den Proxy (proxy.closed), F4 geht nicht zum Sender.
    if vk == input::VK_F4 && msg == WM_SYSKEYDOWN {
        return None;
    }
    let svk = input::sided_vk(vk, scan, extended);
    let repeat = down && (lp.0 >> 30) & 1 == 1;
    // Vor der Sperre: PeekMessage kann gesendete Nachrichten zustellen (Wiedereintritt).
    let altgr_ctrl = svk == 0xA2 && down && is_altgr_fake_ctrl(hwnd);
    let mut st = s.input.lock().unwrap();
    // AltGr-Paar (kuenstliches Strg + rechtes Alt) nicht melden; Zeichen mit AltGr gehen als `text`.
    if svk == 0xA2 {
        if altgr_ctrl && !st.fake_ctrl {
            st.fake_ctrl = true;
            return Some(LRESULT(0));
        }
        if st.fake_ctrl {
            if !down {
                st.fake_ctrl = false;
            }
            return Some(LRESULT(0));
        }
    }
    if svk == 0xA5 && (st.fake_ctrl || st.altgr) {
        st.altgr = down;
        return Some(LRESULT(0));
    }
    // GetKeyState sieht per PostMessage eingespieltes Shift nicht; die gehaltene Taste schon.
    let shift = key_down(input::VK_SHIFT) || st.holds_shift();
    // Nur das Druecken wird unter AltGr zu Text; das Loslassen laeuft ueber den normalen Weg, damit eine vor
    // AltGr gedrueckte Zeichentaste ihr `up` bekommt (sonst haengt sie beim Sender).
    if st.altgr && down && input::named_key(svk).is_none() {
        let c = key_char(vk, scan, true, shift);
        // Das WM_CHAR zur Taste darf nicht doppelt als Text raus. Tote Taste (kein Zeichen): nichts
        // entfernen, das zusammengesetzte Zeichen kommt spaeter als WM_CHAR.
        *drop_char = c.is_some();
        drop(st);
        if let Some(c) = c.filter(|_| !repeat) {
            s.send_input(json!({"k": "text", "text": c}));
        }
        return Some(LRESULT(0));
    }
    let computed = match input::named_key(svk) {
        Some(n) => Some(n.to_owned()),
        None => key_char(vk, scan, false, shift),
    };
    // down: Name von jetzt. up: Name vom down (InputState, nach VK), auch wenn Shift/Caps ihn
    // inzwischen geaendert haben oder die Taste tot geworden ist. Sonst bleibt sie beim Sender unten.
    let ev = match (down, computed) {
        (true, Some(name)) => {
            if input::named_key(svk).is_none() {
                *drop_char = true;
            }
            st.key(svk, &name, true)
        }
        (true, None) => None,
        (false, name) => st.key(svk, name.as_deref().unwrap_or(""), false),
    };
    drop(st);
    if let Some(v) = ev {
        s.send_input(v);
    }
    if msg == WM_SYSKEYDOWN || msg == WM_SYSKEYUP {
        // Alt+Leertaste (Systemmenue) bleibt bei Windows; den Rest nicht an DefWindowProc (kein Menue-Modus).
        if vk == 0x20 {
            return None;
        }
        return Some(LRESULT(0));
    }
    Some(LRESULT(0))
}

fn on_button(hwnd: HWND, s: &Shared, b: Button, down: bool, lp: LPARAM) {
    let (x, y) = lparam_xy(lp);
    if down && b == Button::Left {
        if let Some(st) = s.stream() {
            let (sx, sy) = input::to_stream(x as f64, y as f64, s.size(), st);
            if let Some(light) = input::traffic_light_hit(sx, sy) {
                on_traffic_light(hwnd, s, light);
                return;
            }
        }
    }
    if client_in_stream_title_bar(s, x, y) {
        return;
    }
    let pos = s.to_stream(x, y);
    let mut st = s.input.lock().unwrap();
    let ev = st.button(b, down, pos, (x as f64, y as f64), Instant::now());
    let any = st.any_button();
    drop(st);
    unsafe {
        // Ziehen ueber den Fensterrand hinaus: Maus festhalten, bis alle Buttons oben sind.
        if down {
            SetCapture(hwnd);
        } else if !any {
            let _ = ReleaseCapture();
        }
    }
    if let Some(v) = ev {
        s.send_input(v);
    }
}

fn on_wheel(hwnd: HWND, s: &Shared, wp: WPARAM, lp: LPARAM, horizontal: bool) {
    let delta = ((wp.0 >> 16) & 0xffff) as i16 as f64 / WHEEL_DELTA as f64;
    // Rad-Nachrichten tragen Bildschirmkoordinaten.
    let (sx, sy) = lparam_xy(lp);
    let mut pt = POINT { x: sx, y: sy };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
    }
    let pos = s.to_stream(pt.x, pt.y);
    let (dy, dx) = if horizontal {
        (0.0, delta)
    } else {
        (delta, 0.0)
    };
    let v = s.input.lock().unwrap().scroll(pos, dy, dx);
    s.send_input(v);
}

unsafe fn shared_of(hwnd: HWND) -> Option<&'static Shared> {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
    p.as_ref()
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCREATE => {
                let cs = &*(lp.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_NCHITTEST => {
                if let Some(s) = shared_of(hwnd) {
                    let sx = (lp.0 & 0xffff) as i16 as i32;
                    let sy = ((lp.0 >> 16) & 0xffff) as i16 as i32;
                    let fullscreen = s.windowed.lock().unwrap().is_some();
                    let (cx, cy) = client_point(hwnd, sx, sy);
                    let hit = input::client_hit_test(
                        cx,
                        cy,
                        s.size(),
                        s.stream(),
                        !fullscreen,
                    );
                    if hit != input::HTCLIENT {
                        return LRESULT(hit);
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SIZE => {
                if let Some(s) = shared_of(hwnd) {
                    let w = (lp.0 & 0xffff) as u32;
                    let h = ((lp.0 >> 16) & 0xffff) as u32;
                    if w > 0 && h > 0 {
                        s.set_size(w, h);
                        if wp.0 as u32 != SIZE_MINIMIZED && !s.programmatic.load(Ordering::Acquire)
                        {
                            offer_resize(hwnd, s, false);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOVING => {
                // Nutzer zieht an der Titelleiste: Position fuer Pythons Rueckweg-Kante melden.
                if let Some(s) = shared_of(hwnd) {
                    s.moving.store(true, Ordering::Release);
                    emit_drag(s, cursor_px(), true, false);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_EXITSIZEMOVE => {
                if let Some(s) = shared_of(hwnd) {
                    offer_resize(hwnd, s, true);
                    if s.moving.swap(false, Ordering::AcqRel) {
                        emit_drag(s, cursor_px(), false, true);
                    }
                    // Waehrend der modalen Schleife verwirft Windows Thread-Nachrichten: wartende
                    // IPC-Befehle jetzt abarbeiten lassen.
                    let _ = PostThreadMessageW(s.main_thread, WM_APP_CMD, WPARAM(0), LPARAM(0));
                }
                LRESULT(0)
            }
            WM_TIMER if wp.0 == FOLLOW_TIMER => {
                if let Some(s) = shared_of(hwnd) {
                    follow_tick(hwnd, s);
                }
                LRESULT(0)
            }
            WM_TIMER if wp.0 == RESIZE_TIMER => {
                let _ = KillTimer(Some(hwnd), RESIZE_TIMER);
                if let Some(s) = shared_of(hwnd) {
                    let v = s.resize.lock().unwrap().flush(Instant::now());
                    if let Some(v) = v {
                        send_resize(s, v);
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some(s) = shared_of(hwnd) {
                    let (x, y) = lparam_xy(lp);
                    if !client_in_stream_title_bar(s, x, y) {
                        let pos = s.to_stream(x, y);
                        let v = s.input.lock().unwrap().moved(pos);
                        s.send_input(v);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
            | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
                if let Some(s) = shared_of(hwnd) {
                    let (b, down) = match msg {
                        WM_LBUTTONDOWN => (Button::Left, true),
                        WM_LBUTTONUP => (Button::Left, false),
                        WM_RBUTTONDOWN => (Button::Right, true),
                        WM_RBUTTONUP => (Button::Right, false),
                        WM_MBUTTONDOWN => (Button::Middle, true),
                        WM_MBUTTONUP => (Button::Middle, false),
                        _ => {
                            let b = if (wp.0 >> 16) & 0xffff == 1 {
                                Button::Back
                            } else {
                                Button::Forward
                            };
                            (b, msg == WM_XBUTTONDOWN)
                        }
                    };
                    on_button(hwnd, s, b, down, lp);
                }
                if msg == WM_XBUTTONDOWN || msg == WM_XBUTTONUP {
                    LRESULT(1)
                } else {
                    LRESULT(0)
                }
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                if let Some(s) = shared_of(hwnd) {
                    on_wheel(hwnd, s, wp, lp, msg == WM_MOUSEHWHEEL);
                }
                LRESULT(0)
            }
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                match shared_of(hwnd).and_then(|s| on_key(hwnd, s, msg, wp, lp)) {
                    Some(r) => r,
                    None => DefWindowProcW(hwnd, msg, wp, lp),
                }
            }
            WM_CHAR => {
                if let Some(s) = shared_of(hwnd) {
                    let v = s.input.lock().unwrap().char_unit(wp.0 as u16);
                    if let Some(v) = v {
                        s.send_input(v);
                    }
                }
                LRESULT(0)
            }
            WM_SYSCHAR => {
                // Alt+Taste wurde schon als Taste gemeldet; kein Menue-Kuerzel (Piepton). Alt+Leertaste:
                // Systemmenue bleibt bei Windows.
                if wp.0 == 0x20 {
                    DefWindowProcW(hwnd, msg, wp, lp)
                } else {
                    LRESULT(0)
                }
            }
            WM_SYSCOMMAND if (wp.0 & 0xfff0) as u32 == SC_KEYMENU && lp.0 == 0 => {
                // Alt allein loest sonst den Menue-Modus aus und schluckt die naechsten Tasten.
                LRESULT(0)
            }
            WM_ACTIVATE => {
                if let Some(s) = shared_of(hwnd) {
                    let focused = (wp.0 & 0xffff) as u32 != WA_INACTIVE;
                    if s.focused.swap(focused, Ordering::AcqRel) != focused {
                        s.out.send(&ipc::event(
                            "proxy.focus",
                            json!({"tid": s.tid, "focused": focused}),
                        ));
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_KILLFOCUS => {
                // Fokus weg (Alt+Tab, Klick woanders): alles Gedrueckte loslassen, nichts darf haengen.
                if let Some(s) = shared_of(hwnd) {
                    let ups = s.input.lock().unwrap().release_all();
                    for v in ups {
                        s.send_input(v);
                    }
                }
                let _ = ReleaseCapture();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_CAPTURECHANGED => {
                // Maus-Capture verloren (z. B. Fokuswechsel waehrend des Ziehens): Buttons loslassen.
                if let Some(s) = shared_of(hwnd) {
                    if HWND(lp.0 as *mut _) != hwnd {
                        let mut st = s.input.lock().unwrap();
                        if st.any_button() {
                            let ups: Vec<Value> = st
                                .release_all()
                                .into_iter()
                                .filter(|v| v["k"] == "up")
                                .collect();
                            drop(st);
                            for v in ups {
                                s.send_input(v);
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                if let Some(s) = shared_of(hwnd) {
                    s.dpi.store((wp.0 & 0xffff) as u32, Ordering::Relaxed);
                }
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                // Monitor mit anderer DPI: Python erfaehrt die neue Pixelgroesse (Punkte bleiben gleich).
                if let Some(s) = shared_of(hwnd) {
                    offer_resize(hwnd, s, true);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                // Gezeichnet wird im Empfaenger-Thread (Swapchain); nur neu praesentieren lassen.
                let _ = ValidateRect(Some(hwnd), None);
                if let Some(s) = shared_of(hwnd) {
                    s.dirty.store(true, Ordering::Release);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_CLOSE => {
                // Nicht selbst zerstoeren: der Hauptthread meldet proxy.closed und raeumt geordnet auf
                // (erst Empfaenger-Thread beenden, dann Fenster).
                if let Some(s) = shared_of(hwnd) {
                    let _ = PostThreadMessageW(
                        s.main_thread,
                        WM_APP_CLOSED,
                        WPARAM(s.tid as usize),
                        LPARAM(0),
                    );
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
                if !p.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Arc::from_raw(p));
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_scaling() {
        assert_eq!(client_px(800.0, 600.0, 96), (800, 600));
        assert_eq!(client_px(800.0, 600.0, 144), (1200, 900));
        assert_eq!(client_px(1000.0, 500.0, 120), (1250, 625));
        assert_eq!(client_px(0.1, 0.1, 96), (1, 1));
    }

    #[test]
    fn monitor_choice_avoids_fullscreen() {
        // kein Vollbild: Monitor unter dem Mauszeiger
        assert_eq!(choose_monitor(1, &[1, 2], None), (1, false));
        // Vollbild auf einem anderen Monitor: egal
        assert_eq!(choose_monitor(1, &[1, 2], Some(2)), (1, false));
        // Vollbild unter dem Mauszeiger: anderer Monitor
        assert_eq!(choose_monitor(1, &[1, 2], Some(1)), (2, false));
        // nur ein Monitor mit Vollbild: bleibt, aber verdeckt
        assert_eq!(choose_monitor(1, &[1], Some(1)), (1, true));
    }

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit_into((800, 600), (1920, 1080)), (800, 600));
        assert_eq!(fit_into((3840, 2160), (1920, 1080)), (1920, 1080));
        assert_eq!(fit_into((2000, 1000), (1000, 1000)), (1000, 500));
    }
}
