//! Die C-ABI so benutzen, wie Swift/C es tut: nur ueber die `bt_*`-Funktionen und rohe Zeiger.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use beamer_transport::ffi::*;

type Rx = (u32, u8, Vec<u8>, u64, u8, u32);

#[derive(Default)]
struct Ctx {
    frames: Mutex<Vec<Rx>>,
    controls: Mutex<Vec<(u32, u8)>>,
    stats: Mutex<Vec<String>>,
}

unsafe extern "C" fn on_frame(ctx: *mut c_void, sid: u32, kind: u8, f: *const BtFrame) {
    let c = &*(ctx as *const Ctx);
    let f = &*f;
    c.frames.lock().unwrap().push((
        sid,
        kind,
        std::slice::from_raw_parts(f.data, f.len).to_vec(),
        f.pts_us,
        f.fflags,
        f.frame_id,
    ));
}
unsafe extern "C" fn on_control(ctx: *mut c_void, sid: u32, t: u8) {
    (*(ctx as *const Ctx))
        .controls
        .lock()
        .unwrap()
        .push((sid, t));
}
unsafe extern "C" fn on_stats(ctx: *mut c_void, _sid: u32, json: *const c_char) {
    let s = CStr::from_ptr(json).to_str().unwrap().to_string();
    (*(ctx as *const Ctx)).stats.lock().unwrap().push(s);
}

fn last_error() -> String {
    unsafe {
        CStr::from_ptr(bt_last_error())
            .to_str()
            .unwrap()
            .to_string()
    }
}

#[test]
fn c_abi_end_to_end() {
    let (tx_ctx, rx_ctx) = (Box::new(Ctx::default()), Box::new(Ctx::default()));
    let tx_p = &*tx_ctx as *const Ctx as *mut c_void;
    let rx_p = &*rx_ctx as *const Ctx as *mut c_void;
    let (s2r_key, r2s_key) = ([3u8; 32], [4u8; 32]);
    let (s2r_pfx, r2s_pfx) = ([1u8, 1, 1, 1], [2u8, 2, 2, 2]);
    unsafe {
        let tx = bt_open(0, Some(on_frame), Some(on_control), Some(on_stats), tx_p);
        let rx = bt_open(0, Some(on_frame), Some(on_control), Some(on_stats), rx_p);
        assert!(!tx.is_null() && !rx.is_null());
        let (tp, rp) = (bt_local_port(tx), bt_local_port(rx));
        assert!(tp != 0 && rp != 0 && tp != rp);
        let lo = CString::new("127.0.0.1").unwrap();
        assert_eq!(bt_set_peer(tx, lo.as_ptr(), rp), 0);
        assert_eq!(bt_set_peer(rx, lo.as_ptr(), tp), 0);
        assert_eq!(
            bt_add_stream(
                tx,
                5,
                BtRole::BT_ROLE_SENDER,
                s2r_key.as_ptr(),
                s2r_pfx.as_ptr(),
                r2s_key.as_ptr(),
                r2s_pfx.as_ptr(),
                6000
            ),
            0
        );
        assert_eq!(
            bt_add_stream(
                rx,
                5,
                BtRole::BT_ROLE_RECEIVER,
                s2r_key.as_ptr(),
                s2r_pfx.as_ptr(),
                r2s_key.as_ptr(),
                r2s_pfx.as_ptr(),
                6000
            ),
            0
        );
        assert_eq!(bt_target_kbps(tx, 5), 6000);

        let data: Vec<u8> = (0..9000u32).map(|i| (i % 253) as u8).collect();
        assert_eq!(
            bt_send_frame(tx, 5, 1, data.as_ptr(), data.len(), 777, 1),
            0
        );
        wait(|| !rx_ctx.frames.lock().unwrap().is_empty());
        {
            let f = rx_ctx.frames.lock().unwrap();
            assert_eq!(f[0], (5, 1, data.clone(), 777, 1, 1));
        }

        assert_eq!(bt_request_keyframe(rx, 5), 0);
        wait(|| !tx_ctx.controls.lock().unwrap().is_empty());
        assert_eq!(tx_ctx.controls.lock().unwrap()[0], (5, 5));

        wait(|| !rx_ctx.stats.lock().unwrap().is_empty());
        assert!(rx_ctx.stats.lock().unwrap()[0].starts_with('{'));

        // Fehlerpfade: nie Absturz, immer < 0 und eine Meldung.
        assert!(bt_send_frame(tx, 99, 1, data.as_ptr(), data.len(), 0, 0) < 0);
        assert!(last_error().contains("99"), "{}", last_error());
        assert!(bt_send_frame(tx, 5, 1, std::ptr::null(), 10, 0, 0) < 0);
        assert!(bt_send_frame(std::ptr::null_mut(), 5, 1, data.as_ptr(), 1, 0, 0) < 0);
        assert!(
            bt_add_stream(
                tx,
                6,
                BtRole::BT_ROLE_SENDER,
                std::ptr::null(),
                s2r_pfx.as_ptr(),
                r2s_key.as_ptr(),
                r2s_pfx.as_ptr(),
                1
            ) < 0
        );
        assert!(last_error().contains("s2r_key"));
        assert!(bt_set_peer(tx, std::ptr::null(), 1) < 0);
        assert!(bt_remove_stream(tx, 77) < 0);
        assert_eq!(bt_target_kbps(tx, 77), 0);
        assert_eq!(bt_local_port(std::ptr::null()), 0);
        bt_close(std::ptr::null_mut());

        // Port belegt: NULL plus Fehlertext.
        let dup = bt_open(tp, None, None, None, std::ptr::null_mut());
        assert!(dup.is_null());
        assert!(!last_error().is_empty());

        let t = Instant::now();
        bt_close(tx);
        bt_close(rx);
        assert!(t.elapsed() < Duration::from_millis(500));
    }
}

fn wait(mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end && !f() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(f(), "Zeitueberschreitung");
}
