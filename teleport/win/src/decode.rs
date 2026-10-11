//! Hardware-Decode mit FFmpeg (libavcodec, D3D11VA) auf dem D3D11-Geraet des Empfaengers.
//!
//! Es gibt bewusst keinen stillen Software-Fallback: Bietet der Treiber kein D3D11VA fuer den Codec,
//! schlaegt der Decode fehl und das Log sagt warum.

use std::ffi::{c_void, CStr};
use std::ptr;

use rusty_ffmpeg::ffi;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use crate::gpu::Gpu;
use crate::ipc::Codec;

/// `AVD3D11VADeviceContext` aus `libavutil/hwcontext_d3d11va.h` (rusty_ffmpeg bindet den Header nicht).
#[repr(C)]
struct AVD3D11VADeviceContext {
    device: *mut c_void,
    device_context: *mut c_void,
    video_device: *mut c_void,
    video_context: *mut c_void,
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

pub const EAGAIN: i32 = -(ffi::EAGAIN as i32);

pub fn av_err(e: i32) -> String {
    let mut buf = [0i8; 128];
    unsafe {
        ffi::av_strerror(e, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

/// FFmpeg-Meldungen ab Warnung auf stderr (Standard-Callback). stdout bleibt frei.
pub fn init_logging() {
    unsafe { ffi::av_log_set_level(ffi::AV_LOG_WARNING as i32) };
}

pub fn codec_id(c: Codec) -> ffi::AVCodecID {
    match c {
        Codec::H264 => ffi::AV_CODEC_ID_H264,
        Codec::Hevc => ffi::AV_CODEC_ID_HEVC,
    }
}

unsafe extern "C" fn pick_d3d11(
    _ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let mut p = fmts;
    while *p != ffi::AV_PIX_FMT_NONE {
        if *p == ffi::AV_PIX_FMT_D3D11 {
            return *p;
        }
        p = p.add(1);
    }
    eprintln!("beamer-teleport: Decoder bietet kein D3D11VA an (kein Software-Fallback)");
    ffi::AV_PIX_FMT_NONE
}

/// Ein dekodiertes Bild (eigene Referenz auf den FFmpeg-Frame, die Textur bleibt so lange gueltig).
pub struct Decoded {
    frame: *mut ffi::AVFrame,
}

impl Decoded {
    pub fn texture(&self) -> &ID3D11Texture2D {
        unsafe {
            let raw = &*((&(*self.frame).data[0]) as *const *mut u8 as *const *mut c_void);
            ID3D11Texture2D::from_raw_borrowed(raw).expect("D3D11-Frame ohne Textur")
        }
    }

    pub fn slice(&self) -> u32 {
        unsafe { (*self.frame).data[1] as usize as u32 }
    }

    pub fn size(&self) -> (u32, u32) {
        unsafe { ((*self.frame).width as u32, (*self.frame).height as u32) }
    }

    pub fn is_key(&self) -> bool {
        unsafe { (*self.frame).flags & ffi::AV_FRAME_FLAG_KEY as i32 != 0 }
    }
}

impl Drop for Decoded {
    fn drop(&mut self) {
        unsafe { ffi::av_frame_free(&mut self.frame) };
    }
}

pub struct Decoder {
    ctx: *mut ffi::AVCodecContext,
    pkt: *mut ffi::AVPacket,
    hw: *mut ffi::AVBufferRef,
    pub codec: Codec,
}

impl Decoder {
    pub fn new(gpu: &Gpu, codec: Codec) -> Result<Decoder, String> {
        unsafe {
            let mut hw = ffi::av_hwdevice_ctx_alloc(ffi::AV_HWDEVICE_TYPE_D3D11VA);
            if hw.is_null() {
                return Err("av_hwdevice_ctx_alloc(d3d11va) fehlgeschlagen".into());
            }
            let dev = (*hw).data as *mut ffi::AVHWDeviceContext;
            let hwctx = (*dev).hwctx as *mut AVD3D11VADeviceContext;
            // FFmpeg gibt das Geraet beim Aufraeumen frei, deshalb eine eigene Referenz uebergeben.
            (*hwctx).device = gpu.device.clone().into_raw();
            let r = ffi::av_hwdevice_ctx_init(hw);
            if r < 0 {
                ffi::av_buffer_unref(&mut hw);
                return Err(format!("av_hwdevice_ctx_init: {}", av_err(r)));
            }
            let dec = ffi::avcodec_find_decoder(codec_id(codec));
            if dec.is_null() {
                ffi::av_buffer_unref(&mut hw);
                return Err(format!("kein Decoder fuer {}", codec.name()));
            }
            let mut ctx = ffi::avcodec_alloc_context3(dec);
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(hw);
            (*ctx).get_format = Some(pick_d3d11);
            (*ctx).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;
            // Frame-Threads wuerden Bilder zurueckhalten; mit D3D11VA rechnet ohnehin die GPU.
            (*ctx).thread_count = 1;
            // Reserve im Texturpool: das letzte gute Bild bleibt stehen, waehrend weiter dekodiert wird.
            (*ctx).extra_hw_frames = 4;
            let r = ffi::avcodec_open2(ctx, dec, ptr::null_mut());
            if r < 0 {
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut hw);
                return Err(format!("avcodec_open2: {}", av_err(r)));
            }
            Ok(Decoder {
                ctx,
                pkt: ffi::av_packet_alloc(),
                hw,
                codec,
            })
        }
    }

    /// Ein Annex-B-Frame dekodieren. `Ok(None)`: Decoder braucht mehr Daten.
    pub fn decode(&mut self, data: &[u8], key: bool) -> Result<Option<Decoded>, String> {
        if data.len() > i32::MAX as usize {
            return Err("Frame zu gross".into());
        }
        unsafe {
            // Ohne eigenen Puffer kopiert avcodec_send_packet die Daten (mit Padding).
            (*self.pkt).data = data.as_ptr() as *mut u8;
            (*self.pkt).size = data.len() as i32;
            (*self.pkt).flags = if key { ffi::AV_PKT_FLAG_KEY as i32 } else { 0 };
            let r = ffi::avcodec_send_packet(self.ctx, self.pkt);
            (*self.pkt).data = ptr::null_mut();
            (*self.pkt).size = 0;
            if r < 0 && r != EAGAIN {
                return Err(format!("avcodec_send_packet: {}", av_err(r)));
            }
            let mut out: Option<Decoded> = None;
            loop {
                let f = ffi::av_frame_alloc();
                let r = ffi::avcodec_receive_frame(self.ctx, f);
                if r < 0 {
                    let mut f = f;
                    ffi::av_frame_free(&mut f);
                    if r == EAGAIN || r == ffi::AVERROR_EOF {
                        break;
                    }
                    return Err(format!("avcodec_receive_frame: {}", av_err(r)));
                }
                if (*f).format != ffi::AV_PIX_FMT_D3D11 {
                    let mut f = f;
                    ffi::av_frame_free(&mut f);
                    return Err("Frame liegt nicht als D3D11-Textur vor".into());
                }
                if (*f).decode_error_flags != 0 {
                    let mut f = f;
                    ffi::av_frame_free(&mut f);
                    return Err("Decoder meldet ein fehlerhaftes Bild".into());
                }
                // Mehrere Bilder auf einmal (sollte ohne B-Frames nicht vorkommen): nur das neueste zeigen.
                out = Some(Decoded { frame: f });
            }
            Ok(out)
        }
    }

    /// Nach einem Fehler: Referenzen verwerfen, ab dem naechsten Keyframe neu anfangen.
    pub fn flush(&mut self) {
        unsafe { ffi::avcodec_flush_buffers(self.ctx) };
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ffi::av_packet_free(&mut self.pkt);
            ffi::avcodec_free_context(&mut self.ctx);
            ffi::av_buffer_unref(&mut self.hw);
        }
    }
}
