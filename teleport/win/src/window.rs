//! Proxy-Fenster: ein Top-Level-Fenster je `tid`, Per-Monitor-DPI-Aware V2.
//!
//! Alle Fenster gehoeren dem Hauptthread (Nachrichtenschleife). Gerendert wird im Empfaenger-Thread;
//! die Fensterprozedur meldet ihm Groessenaenderungen nur ueber atomare Werte. Schliessen schickt
//! eine Thread-Nachricht an den Hauptthread, der dann `proxy.closed` meldet und aufraeumt.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Once};

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetStockObject, MonitorFromPoint, MonitorFromWindow, ValidateRect,
    BLACK_BRUSH, HBRUSH, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Hauptthread-Nachrichten (PostThreadMessage, hwnd = 0).
pub const WM_APP_CMD: u32 = WM_APP + 1;
pub const WM_APP_EOF: u32 = WM_APP + 2;
/// wparam = tid
pub const WM_APP_CLOSED: u32 = WM_APP + 3;
/// wparam = tid: Empfaenger-Thread ist mit einem Fehler beendet.
pub const WM_APP_FAILED: u32 = WM_APP + 4;

const CLASS: PCWSTR = w!("BeamerTeleportProxy");
const STYLE: WINDOW_STYLE = WS_OVERLAPPEDWINDOW;
const EX_STYLE: WINDOW_EX_STYLE = WS_EX_APPWINDOW;

/// Zustand, den Fensterprozedur (Hauptthread) und Empfaenger-Thread teilen.
pub struct Shared {
    pub tid: u32,
    main_thread: u32,
    /// Client-Groesse in physischen Pixeln, (w << 32) | h.
    size: AtomicU64,
    /// Groesse geaendert oder Neuzeichnen noetig.
    pub dirty: AtomicBool,
    pub dpi: AtomicU32,
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

pub struct ProxyWindow {
    pub hwnd: HWND,
    pub shared: Arc<Shared>,
}

/// Fenster auf dem Monitor unter dem Mauszeiger erzeugen, `index` versetzt mehrere Fenster.
pub fn create(
    tid: u32,
    title: &str,
    w_pt: f64,
    h_pt: f64,
    main_thread: u32,
    index: usize,
) -> Result<ProxyWindow> {
    register_class();
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
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
            dirty: AtomicBool::new(false),
            dpi: AtomicU32::new(dpi),
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
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        Ok(ProxyWindow { hwnd, shared })
    }
}

impl ProxyWindow {
    pub fn set_title(&self, title: &str) {
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title));
        }
    }

    /// Neue Groesse in Punkten (recv.update). Der Inhalt wird nur skaliert (Rueckkanal: C05).
    pub fn resize_points(&self, w_pt: f64, h_pt: f64) {
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
    }

    pub fn destroy(&self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
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
            WM_SIZE => {
                if let Some(s) = shared_of(hwnd) {
                    let w = (lp.0 & 0xffff) as u32;
                    let h = ((lp.0 >> 16) & 0xffff) as u32;
                    if w > 0 && h > 0 {
                        s.set_size(w, h);
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
    fn fit_keeps_aspect() {
        assert_eq!(fit_into((800, 600), (1920, 1080)), (800, 600));
        assert_eq!(fit_into((3840, 2160), (1920, 1080)), (1920, 1080));
        assert_eq!(fit_into((2000, 1000), (1000, 1000)), (1000, 500));
    }
}
