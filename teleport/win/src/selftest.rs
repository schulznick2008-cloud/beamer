//! `--selftest-send <host> <port>`: synthetisches Testbild (Zaehler und Zeitstempel als grosse Ziffern,
//! dazu ein wandernder Balken) kodieren und ueber beamer-transport senden. So laesst sich C03 ohne Mac
//! testen. Encoder der Reihe nach: AMF (Hardware), Media Foundation, OpenH264 (Software, mit Warnung).

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use beamer_transport::{Callbacks, Role, Session, StreamKeys};
use rusty_ffmpeg::ffi;

use crate::clock;
use crate::decode::{av_err, EAGAIN};
use crate::ipc::Codec;

/// Feste Testschluessel je `tid`. NUR fuer lokale Tests: Bei jedem Lauf beginnt `seq` wieder bei 1, mit
/// festen Schluesseln heisst das Nonce-Wiederverwendung. Echte Schluessel leitet Python je Teleport ab
/// (SCHNITTSTELLEN.md §4.2). Die `tid` steckt in den ersten 4 Byte, damit eine Session mehrere
/// Teststreams annimmt (sie lehnt doppelte Sendeschluessel ab).
pub fn test_keys(tid: u32) -> StreamKeys {
    let key = |fill: u8| {
        let mut k = [fill; 32];
        k[..4].copy_from_slice(&tid.to_be_bytes());
        k
    };
    StreamKeys {
        s2r_key: key(0x11),
        s2r_prefix: [0x00, 0x00, 0x00, 0x01],
        r2s_key: key(0x22),
        r2s_prefix: [0x00, 0x00, 0x00, 0x02],
    }
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub host: String,
    pub port: u16,
    pub tid: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
    pub codec: Codec,
    pub secs: f64,
    /// Encoder erzwingen (z. B. `h264_amf`, `libopenh264`).
    pub encoder: Option<String>,
    pub local_port: u16,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            host: "127.0.0.1".into(),
            port: 47001,
            tid: 1,
            width: 1920,
            height: 1080,
            fps: 60,
            kbps: 20_000,
            codec: Codec::H264,
            secs: 0.0,
            encoder: None,
            local_port: 0,
        }
    }
}

pub fn parse_args(a: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = a.iter();
    o.host = it.next().ok_or("host fehlt")?.clone();
    o.port = it
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or("port fehlt oder ungueltig")?;
    while let Some(k) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("Wert fehlt nach {k}"));
        match k.as_str() {
            "--tid" => o.tid = val()?.parse().map_err(|_| "tid ungueltig")?,
            "--fps" => o.fps = val()?.parse().map_err(|_| "fps ungueltig")?,
            "--kbps" => o.kbps = val()?.parse().map_err(|_| "kbps ungueltig")?,
            "--secs" => o.secs = val()?.parse().map_err(|_| "secs ungueltig")?,
            "--local-port" => o.local_port = val()?.parse().map_err(|_| "local-port ungueltig")?,
            "--encoder" => o.encoder = Some(val()?),
            "--codec" => {
                let c = val()?;
                o.codec = Codec::parse(&c).ok_or(format!("codec {c} unbekannt"))?;
            }
            "--size" => {
                let s = val()?;
                let (w, h) = s.split_once('x').ok_or("size: BREITExHOEHE")?;
                o.width = w.parse().map_err(|_| "Breite ungueltig")?;
                o.height = h.parse().map_err(|_| "Hoehe ungueltig")?;
            }
            x => return Err(format!("unbekannte Option {x}")),
        }
    }
    if o.tid == 0
        || o.fps == 0
        || o.width < 64
        || o.height < 64
        || o.width % 2 != 0
        || o.height % 2 != 0
    {
        return Err("tid/fps/size ungueltig (gerade Groessen ab 64)".into());
    }
    Ok(o)
}

// ------------------------------------------------------------------ Testbild

/// 7-Segment-Ziffern: Bits a..g.
const SEG: [u8; 10] = [
    0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
    0b1111111, 0b1101111,
];

fn fill(y: &mut [u8], stride: usize, x0: usize, y0: usize, w: usize, h: usize, v: u8) {
    for row in y0..y0 + h {
        let off = row * stride + x0;
        if let Some(s) = y.get_mut(off..off + w) {
            s.fill(v);
        }
    }
}

/// Ziffernfolge in die Y-Ebene zeichnen. `s` = Hoehe einer Ziffer in Pixeln.
pub fn draw_number(y: &mut [u8], stride: usize, x: usize, top: usize, s: usize, text: &str) {
    let w = s / 2;
    let t = (s / 10).max(2);
    let mut cx = x;
    for ch in text.chars() {
        if let Some(d) = ch.to_digit(10) {
            let m = SEG[d as usize];
            let half = s / 2;
            let segs = [
                (0, cx, top, w, t),
                (1, cx + w - t, top, t, half),
                (2, cx + w - t, top + half, t, half),
                (3, cx, top + s - t, w, t),
                (4, cx, top + half, t, half),
                (5, cx, top, t, half),
                (6, cx, top + half - t / 2, w, t),
            ];
            for (bit, sx, sy, sw, sh) in segs {
                if m & (1 << bit) != 0 {
                    fill(y, stride, sx, sy, sw, sh, 235);
                }
            }
        } else if ch == '.' || ch == ':' {
            fill(y, stride, cx + w / 4, top + s - t, t, t, 235);
        }
        cx += w + w / 2;
    }
}

/// Die beiden Ebenen eines NV12-Bildes.
pub struct Nv12<'a> {
    pub y: &'a mut [u8],
    pub y_stride: usize,
    pub uv: &'a mut [u8],
    pub uv_stride: usize,
    pub w: usize,
    pub h: usize,
}

/// Ein NV12-Bild: Verlauf, wandernder Balken, Zaehler und Zeitstempel (ms).
pub fn draw_frame(img: Nv12, n: u64, t_ms: u64) {
    let Nv12 {
        y,
        y_stride,
        uv,
        uv_stride,
        w,
        h,
    } = img;
    for row in 0..h {
        let v = 16 + ((row * 64) / h) as u8;
        y[row * y_stride..row * y_stride + w].fill(v);
    }
    // Rauschfeld (unten rechts, ein Viertel der Flaeche): haelt die Bitrate realistisch hoch.
    let mut r = n.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    for row in h / 2..h {
        for px in y[row * y_stride + w / 2..row * y_stride + w].chunks_mut(8) {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            px.fill(16 + (r % 200) as u8);
        }
    }
    let bar = (n as usize * 8) % w;
    fill(y, y_stride, bar, 0, (w / 60).max(4).min(w - bar), h, 200);
    let s = h / 6;
    draw_number(y, y_stride, w / 20, h / 8, s, &format!("{n:07}"));
    draw_number(
        y,
        y_stride,
        w / 20,
        h / 8 + s + s / 3,
        s,
        &format!("{}", t_ms % 100_000_000),
    );
    for row in 0..h / 2 {
        uv[row * uv_stride..row * uv_stride + w].fill(128);
    }
}

// ------------------------------------------------------------------ Encoder

pub struct Encoder {
    ctx: *mut ffi::AVCodecContext,
    frame: *mut ffi::AVFrame,
    pkt: *mut ffi::AVPacket,
    pub name: String,
    pub hardware: bool,
    extradata: Vec<u8>,
    /// NV12 (AMF, Media Foundation) oder YUV420P (OpenH264).
    planar: bool,
    codec: Codec,
}

fn set_opt(obj: *mut std::ffi::c_void, k: &str, v: &str) {
    let (k, v) = (CString::new(k).unwrap(), CString::new(v).unwrap());
    unsafe {
        let _ = ffi::av_opt_set(obj, k.as_ptr(), v.as_ptr(), 0);
    }
}

/// Enthaelt das Annex-B-Paket einen Parametersatz (SPS bzw. VPS)?
pub fn has_param_sets(data: &[u8], codec: Codec) -> bool {
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let nal = data[i + 3];
            let found = match codec {
                Codec::H264 => nal & 0x1f == 7,
                Codec::Hevc => (nal >> 1) & 0x3f == 32 || (nal >> 1) & 0x3f == 33,
            };
            if found {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

impl Encoder {
    pub fn candidates(codec: Codec) -> &'static [&'static str] {
        match codec {
            Codec::H264 => &["h264_amf", "h264_mf", "libopenh264"],
            Codec::Hevc => &["hevc_amf", "hevc_mf"],
        }
    }

    pub fn open(o: &Opts) -> Result<Encoder, String> {
        let names: Vec<String> = match &o.encoder {
            Some(n) => vec![n.clone()],
            None => Encoder::candidates(o.codec)
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        let mut errs = Vec::new();
        for n in names {
            match Encoder::open_one(o, &n) {
                Ok(e) => return Ok(e),
                Err(e) => errs.push(format!("{n}: {e}")),
            }
        }
        Err(errs.join("; "))
    }

    fn open_one(o: &Opts, name: &str) -> Result<Encoder, String> {
        let cname = CString::new(name).unwrap();
        unsafe {
            let enc = ffi::avcodec_find_encoder_by_name(cname.as_ptr());
            if enc.is_null() {
                return Err("nicht vorhanden".into());
            }
            let mut ctx = ffi::avcodec_alloc_context3(enc);
            (*ctx).width = o.width as i32;
            (*ctx).height = o.height as i32;
            // OpenH264 nimmt nur planares YUV 4:2:0, die Hardware-Encoder NV12.
            let planar = name == "libopenh264";
            let pix_fmt = if planar {
                ffi::AV_PIX_FMT_YUV420P
            } else {
                ffi::AV_PIX_FMT_NV12
            };
            (*ctx).pix_fmt = pix_fmt;
            (*ctx).time_base = ffi::AVRational {
                num: 1,
                den: o.fps as i32,
            };
            (*ctx).framerate = ffi::AVRational {
                num: o.fps as i32,
                den: 1,
            };
            (*ctx).bit_rate = o.kbps as i64 * 1000;
            (*ctx).rc_max_rate = (*ctx).bit_rate;
            (*ctx).rc_buffer_size = ((*ctx).bit_rate / o.fps as i64 * 2) as i32;
            (*ctx).gop_size = (o.fps * 2) as i32;
            (*ctx).max_b_frames = 0;
            (*ctx).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;
            (*ctx).color_range = ffi::AVCOL_RANGE_MPEG;
            (*ctx).colorspace = ffi::AVCOL_SPC_BT709;
            (*ctx).color_primaries = ffi::AVCOL_PRI_BT709;
            (*ctx).color_trc = ffi::AVCOL_TRC_BT709;
            let p = (*ctx).priv_data;
            if name.ends_with("_amf") {
                set_opt(p, "usage", "ultralowlatency");
                set_opt(p, "quality", "speed");
                set_opt(p, "rc", "cbr");
                set_opt(p, "forced_idr", "1");
                set_opt(p, "header_insertion_mode", "idr");
            } else if name.ends_with("_mf") {
                set_opt(p, "hw_encoding", "1");
                set_opt(p, "scenario", "display_remoting");
            }
            let r = ffi::avcodec_open2(ctx, enc, ptr::null_mut());
            if r < 0 {
                ffi::avcodec_free_context(&mut ctx);
                return Err(av_err(r));
            }
            let frame = ffi::av_frame_alloc();
            (*frame).format = pix_fmt;
            (*frame).width = o.width as i32;
            (*frame).height = o.height as i32;
            let r = ffi::av_frame_get_buffer(frame, 32);
            if r < 0 {
                let mut f = frame;
                ffi::av_frame_free(&mut f);
                ffi::avcodec_free_context(&mut ctx);
                return Err(av_err(r));
            }
            let extradata = if (*ctx).extradata.is_null() || (*ctx).extradata_size <= 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts((*ctx).extradata, (*ctx).extradata_size as usize)
                    .to_vec()
            };
            let caps = (*enc).capabilities;
            let hardware = caps & ffi::AV_CODEC_CAP_HARDWARE as i32 != 0 || name.ends_with("_amf");
            let long = CStr::from_ptr((*enc).long_name).to_string_lossy();
            eprintln!("beamer-teleport: Selbsttest-Encoder {name} ({long})");
            if !hardware {
                eprintln!("beamer-teleport: WARNUNG Software-Encoder {name} - kein Hardware-Encoder verfuegbar");
            }
            Ok(Encoder {
                ctx,
                frame,
                pkt: ffi::av_packet_alloc(),
                name: name.to_owned(),
                hardware,
                extradata,
                codec: o.codec,
                planar,
            })
        }
    }

    /// Bild zeichnen, kodieren; liefert (Annex-B, Keyframe) fuer jedes fertige Paket.
    pub fn encode(
        &mut self,
        n: u64,
        t_ms: u64,
        force_key: bool,
    ) -> Result<Vec<(Vec<u8>, bool)>, String> {
        unsafe {
            let f = self.frame;
            let r = ffi::av_frame_make_writable(f);
            if r < 0 {
                return Err(av_err(r));
            }
            let (w, h) = ((*f).width as usize, (*f).height as usize);
            let ys = (*f).linesize[0] as usize;
            let y = std::slice::from_raw_parts_mut((*f).data[0], ys * h);
            if self.planar {
                // Luma zeichnen, Chroma (U und V, je halbe Breite) grau.
                let mut uv = vec![0u8; w * h / 2];
                draw_frame(
                    Nv12 {
                        y,
                        y_stride: ys,
                        uv: &mut uv,
                        uv_stride: w,
                        w,
                        h,
                    },
                    n,
                    t_ms,
                );
                for p in 1..=2 {
                    let ls = (*f).linesize[p] as usize;
                    std::slice::from_raw_parts_mut((*f).data[p], ls * h / 2).fill(128);
                }
            } else {
                let uvs = (*f).linesize[1] as usize;
                let img = Nv12 {
                    y,
                    y_stride: ys,
                    uv: std::slice::from_raw_parts_mut((*f).data[1], uvs * h / 2),
                    uv_stride: uvs,
                    w,
                    h,
                };
                draw_frame(img, n, t_ms);
            }
            (*f).pts = n as i64;
            if force_key {
                (*f).pict_type = ffi::AV_PICTURE_TYPE_I;
                (*f).flags |= ffi::AV_FRAME_FLAG_KEY as i32;
            } else {
                (*f).pict_type = ffi::AV_PICTURE_TYPE_NONE;
                (*f).flags &= !(ffi::AV_FRAME_FLAG_KEY as i32);
            }
            let r = ffi::avcodec_send_frame(self.ctx, f);
            if r < 0 {
                return Err(format!("avcodec_send_frame: {}", av_err(r)));
            }
            let mut out = Vec::new();
            loop {
                let r = ffi::avcodec_receive_packet(self.ctx, self.pkt);
                if r == EAGAIN || r == ffi::AVERROR_EOF {
                    break;
                }
                if r < 0 {
                    return Err(format!("avcodec_receive_packet: {}", av_err(r)));
                }
                let p = &*self.pkt;
                let key = p.flags & ffi::AV_PKT_FLAG_KEY as i32 != 0;
                let data = std::slice::from_raw_parts(p.data, p.size as usize);
                // Vertrag: jeder Keyframe traegt VPS/SPS/PPS inline.
                let mut v = Vec::with_capacity(data.len() + self.extradata.len());
                if key && !has_param_sets(data, self.codec) {
                    v.extend_from_slice(&self.extradata);
                }
                v.extend_from_slice(data);
                out.push((v, key));
                ffi::av_packet_unref(self.pkt);
            }
            Ok(out)
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            ffi::av_packet_free(&mut self.pkt);
            ffi::av_frame_free(&mut self.frame);
            ffi::avcodec_free_context(&mut self.ctx);
        }
    }
}

// ------------------------------------------------------------------ Senden

pub struct Report {
    pub frames: u64,
    pub keyframes: u64,
    pub refused: u64,
    pub encode_ms_median: f64,
    pub encoder: String,
}

/// Sendet bis `stop` gesetzt ist bzw. `secs` (0 = endlos) abgelaufen sind.
pub fn run(o: &Opts, stop: Arc<AtomicBool>) -> Result<Report, String> {
    let want_key = Arc::new(AtomicBool::new(false));
    let wk = want_key.clone();
    let session = Session::open(
        o.local_port,
        Callbacks {
            on_control: Box::new(move |_, t| {
                if t == 5 {
                    wk.store(true, Ordering::Relaxed);
                }
            }),
            ..Callbacks::none()
        },
    )?;
    session.set_peer(&o.host, o.port)?;
    session.add_stream(o.tid, Role::Sender, &test_keys(o.tid), o.kbps.max(1000) * 2)?;
    let mut enc = Encoder::open(o)?;
    eprintln!(
        "beamer-teleport: Selbsttest sendet tid {} {}x{}@{} {} {} kbit/s an {}:{}",
        o.tid,
        o.width,
        o.height,
        o.fps,
        o.codec.name(),
        o.kbps,
        o.host,
        o.port
    );
    let period = Duration::from_secs_f64(1.0 / o.fps as f64);
    let start = Instant::now();
    let mut next = start;
    let mut n = 0u64;
    let mut rep = Report {
        frames: 0,
        keyframes: 0,
        refused: 0,
        encode_ms_median: 0.0,
        encoder: enc.name.clone(),
    };
    let mut enc_ms = Vec::new();
    while !stop.load(Ordering::Relaxed) && (o.secs <= 0.0 || start.elapsed().as_secs_f64() < o.secs)
    {
        let pts = clock::now_us();
        let force = n == 0 || want_key.swap(false, Ordering::Relaxed);
        let t0 = Instant::now();
        let pkts = enc.encode(n, pts / 1000, force)?;
        enc_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        for (data, key) in pkts {
            let fflags = key as u8 | o.codec.fflags_bits();
            if session.send_frame(o.tid, 1, &data, pts, fflags).is_err() {
                rep.refused += 1;
            }
            rep.frames += 1;
            rep.keyframes += key as u64;
        }
        n += 1;
        if n.is_multiple_of(o.fps as u64 * 5) {
            eprintln!(
                "beamer-teleport: Selbsttest {} Bilder, Ziel {} kbit/s",
                n,
                session.target_kbps(o.tid)
            );
        }
        next += period;
        let now = Instant::now();
        if next > now {
            let left = next - now;
            if left > Duration::from_millis(2) {
                std::thread::sleep(left - Duration::from_millis(1));
            }
            while Instant::now() < next {
                std::hint::spin_loop();
            }
        } else if now - next > period * 10 {
            next = now; // zu langsam: nicht nachholen
        }
    }
    rep.encode_ms_median = crate::stats::percentile(&enc_ms, 0.5);
    std::thread::sleep(Duration::from_millis(200));
    session.close();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        let a: Vec<String> = [
            "127.0.0.1",
            "47001",
            "--fps",
            "30",
            "--size",
            "1280x720",
            "--codec",
            "hevc",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let o = parse_args(&a).unwrap();
        assert_eq!(
            (o.port, o.fps, o.width, o.height, o.codec),
            (47001, 30, 1280, 720, Codec::Hevc)
        );
        assert!(parse_args(&["h".into()]).is_err());
        assert!(parse_args(&["h".into(), "1".into(), "--size".into(), "65x64".into()]).is_err());
    }

    #[test]
    fn param_set_detection() {
        assert!(has_param_sets(&[0, 0, 0, 1, 0x67, 1, 2], Codec::H264));
        assert!(!has_param_sets(&[0, 0, 0, 1, 0x65, 1, 2], Codec::H264));
        assert!(has_param_sets(&[0, 0, 1, 0x40, 1], Codec::Hevc)); // VPS
        assert!(!has_param_sets(&[0, 0, 1, 0x26, 1], Codec::Hevc)); // IDR
    }

    #[test]
    fn draws_digits() {
        let (w, h) = (320usize, 180usize);
        let mut y = vec![0u8; w * h];
        let mut uv = vec![0u8; w * h / 2];
        draw_frame(
            Nv12 {
                y: &mut y,
                y_stride: w,
                uv: &mut uv,
                uv_stride: w,
                w,
                h,
            },
            8,
            123,
        );
        assert!(y.iter().filter(|v| **v == 235).count() > 100);
        assert!(uv.iter().all(|v| *v == 128));
    }
}
