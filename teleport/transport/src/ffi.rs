//! C-ABI nach SCHNITTSTELLEN.md §5. `unsafe` gibt es nur hier, an der FFI-Grenze.
//! Kein Panic verlaesst diese Funktionen (catch_unwind); Fehler stehen in `bt_last_error`.
#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::session::{Callbacks, FrameRef, Session, StreamKeys};
use crate::stream::Role;

pub struct BtSession {
    s: Session,
}

#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtRole {
    BT_ROLE_SENDER = 1,
    BT_ROLE_RECEIVER = 2,
}

#[repr(C)]
pub struct BtFrame {
    pub data: *const u8,
    pub len: usize,
    pub pts_us: u64,
    pub fflags: u8,
    pub frame_id: u32,
}

pub type BtOnFrame = Option<unsafe extern "C" fn(*mut c_void, u32, u8, *const BtFrame)>;
pub type BtOnControl = Option<unsafe extern "C" fn(*mut c_void, u32, u8)>;
pub type BtOnStats = Option<unsafe extern "C" fn(*mut c_void, u32, *const c_char)>;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(msg: impl AsRef<str>) {
    let c = CString::new(msg.as_ref().replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|l| *l.borrow_mut() = c);
}

/// Fuehrt `f` aus, faengt Panics ab und uebersetzt `Err` in -1 plus `bt_last_error`.
fn guard_int(f: impl FnOnce() -> Result<(), String>) -> c_int {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            set_error(e);
            -1
        }
        Err(_) => {
            set_error("interner Fehler (Panic abgefangen)");
            -1
        }
    }
}

#[derive(Clone, Copy)]
struct Ctx(*mut c_void);
// Der Aufrufer garantiert, dass `ctx` aus dem Transport-Thread benutzbar ist (siehe §5).
unsafe impl Send for Ctx {}
unsafe impl Sync for Ctx {}

unsafe fn array<const N: usize>(p: *const u8, name: &str) -> Result<[u8; N], String> {
    if p.is_null() {
        return Err(format!("{name} ist NULL"));
    }
    let mut a = [0u8; N];
    std::ptr::copy_nonoverlapping(p, a.as_mut_ptr(), N);
    Ok(a)
}

#[no_mangle]
pub unsafe extern "C" fn bt_open(
    local_port: u16,
    on_frame: BtOnFrame,
    on_control: BtOnControl,
    on_stats: BtOnStats,
    ctx: *mut c_void,
) -> *mut BtSession {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let ctx = Ctx(ctx);
        let cb = Callbacks {
            on_frame: Box::new(move |sid, kind, f: &FrameRef| {
                if let Some(cb) = on_frame {
                    let bf = BtFrame {
                        data: f.data.as_ptr(),
                        len: f.data.len(),
                        pts_us: f.pts_us,
                        fflags: f.fflags,
                        frame_id: f.frame_id,
                    };
                    let ctx = ctx;
                    cb(ctx.0, sid, kind, &bf);
                }
            }),
            on_control: Box::new(move |sid, ty| {
                if let Some(cb) = on_control {
                    let ctx = ctx;
                    cb(ctx.0, sid, ty);
                }
            }),
            on_stats: Box::new(move |sid, json| {
                if let (Some(cb), Ok(c)) = (on_stats, CString::new(json)) {
                    let ctx = ctx;
                    cb(ctx.0, sid, c.as_ptr());
                }
            }),
        };
        Session::open(local_port, cb)
    }));
    match r {
        Ok(Ok(s)) => Box::into_raw(Box::new(BtSession { s })),
        Ok(Err(e)) => {
            set_error(e);
            std::ptr::null_mut()
        }
        Err(_) => {
            set_error("interner Fehler (Panic abgefangen)");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn bt_local_port(s: *const BtSession) -> u16 {
    catch_unwind(AssertUnwindSafe(|| {
        s.as_ref().map_or(0, |s| s.s.local_port())
    }))
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn bt_set_peer(s: *mut BtSession, host: *const c_char, port: u16) -> c_int {
    guard_int(|| {
        let s = s.as_ref().ok_or("session ist NULL")?;
        if host.is_null() {
            return Err("host ist NULL".into());
        }
        let host = CStr::from_ptr(host)
            .to_str()
            .map_err(|_| "host ist kein UTF-8")?;
        s.s.set_peer(host, port)
    })
}

#[no_mangle]
pub unsafe extern "C" fn bt_add_stream(
    s: *mut BtSession,
    stream_id: u32,
    role: BtRole,
    s2r_key: *const u8,
    s2r_prefix: *const u8,
    r2s_key: *const u8,
    r2s_prefix: *const u8,
    max_kbps: u32,
) -> c_int {
    guard_int(|| {
        let s = s.as_ref().ok_or("session ist NULL")?;
        let keys = StreamKeys {
            s2r_key: array(s2r_key, "s2r_key")?,
            s2r_prefix: array(s2r_prefix, "s2r_prefix")?,
            r2s_key: array(r2s_key, "r2s_key")?,
            r2s_prefix: array(r2s_prefix, "r2s_prefix")?,
        };
        let role = match role {
            BtRole::BT_ROLE_SENDER => Role::Sender,
            BtRole::BT_ROLE_RECEIVER => Role::Receiver,
        };
        s.s.add_stream(stream_id, role, &keys, max_kbps)
    })
}

#[no_mangle]
pub unsafe extern "C" fn bt_remove_stream(s: *mut BtSession, stream_id: u32) -> c_int {
    guard_int(|| {
        let s = s.as_ref().ok_or("session ist NULL")?;
        s.s.remove_stream(stream_id)
    })
}

#[no_mangle]
pub unsafe extern "C" fn bt_send_frame(
    s: *mut BtSession,
    stream_id: u32,
    kind: u8,
    data: *const u8,
    len: usize,
    pts_us: u64,
    fflags: u8,
) -> c_int {
    guard_int(|| {
        let s = s.as_ref().ok_or("session ist NULL")?;
        if data.is_null() {
            return Err("data ist NULL".into());
        }
        let d = std::slice::from_raw_parts(data, len);
        s.s.send_frame(stream_id, kind, d, pts_us, fflags)
    })
}

#[no_mangle]
pub unsafe extern "C" fn bt_request_keyframe(s: *mut BtSession, stream_id: u32) -> c_int {
    guard_int(|| {
        let s = s.as_ref().ok_or("session ist NULL")?;
        s.s.request_keyframe(stream_id)
    })
}

#[no_mangle]
pub unsafe extern "C" fn bt_target_kbps(s: *const BtSession, stream_id: u32) -> u32 {
    catch_unwind(AssertUnwindSafe(|| {
        s.as_ref().map_or(0, |s| s.s.target_kbps(stream_id))
    }))
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn bt_close(s: *mut BtSession) {
    if s.is_null() {
        return;
    }
    let b = Box::from_raw(s);
    let _ = catch_unwind(AssertUnwindSafe(move || drop(b)));
}

#[no_mangle]
pub extern "C" fn bt_last_error() -> *const c_char {
    LAST_ERROR.with(|l| l.borrow().as_ptr())
}
