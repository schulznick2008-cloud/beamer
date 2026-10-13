//! IPC Python <-> Helfer (SCHNITTSTELLEN.md §1): JSON Lines auf stdin/stdout.
//!
//! Hier steht nur das Protokoll (Parsen, Antworten, Ereignisse), ohne Fenster und GPU, damit es
//! sich ohne Grafikkarte testen laesst.

use std::io::Write;
use std::sync::{Arc, Mutex};

use base64::Engine;
use beamer_transport::StreamKeys;
use serde_json::{json, Map, Value};

pub const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Befehle, die zu einer anderen Seite oder einem spaeteren Paket gehoeren. Sie werden mit
/// `unsupported` beantwortet (C03: Empfaenger ohne Rueckkanal; `proxy.*` kommt mit C05).
pub const LATER_CMDS: &[&str] = &[
    "windows.list",
    "send.start",
    "send.stop",
    "send.resize",
    "send.activate",
    "send.keyframe",
    "send.bitrate",
    "inject",
    "proxy.input",
    "proxy.focus",
    "proxy.resize",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

impl Codec {
    pub fn parse(s: &str) -> Option<Codec> {
        match s {
            "h264" => Some(Codec::H264),
            "hevc" => Some(Codec::Hevc),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
        }
    }

    /// Codec-Bits 1-3 in `fflags` (SCHNITTSTELLEN.md §4.1).
    pub fn fflags_bits(self) -> u8 {
        match self {
            Codec::H264 => 1 << 1,
            Codec::Hevc => 2 << 1,
        }
    }

    pub fn from_fflags(fflags: u8) -> Option<Codec> {
        match (fflags >> 1) & 0b111 {
            1 => Some(Codec::H264),
            2 => Some(Codec::Hevc),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct RecvOpen {
    pub tid: u32,
    pub title: String,
    pub app: String,
    /// Groesse in Punkten (logische Pixel der Senderseite).
    pub w: f64,
    pub h: f64,
    pub scale: f64,
    pub codec: Codec,
    pub fullscreen: bool,
    pub keys: StreamKeys,
    /// App-Icon (PNG) fuer die Taskleiste, optional.
    pub icon_png: Option<Vec<u8>>,
}

impl std::fmt::Debug for RecvOpen {
    // Schluessel nie ausgeben (auch nicht in Panics oder Logs).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecvOpen")
            .field("tid", &self.tid)
            .field("title", &self.title)
            .field("app", &self.app)
            .field("w", &self.w)
            .field("h", &self.h)
            .field("scale", &self.scale)
            .field("codec", &self.codec)
            .field("fullscreen", &self.fullscreen)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecvUpdate {
    pub tid: u32,
    pub title: Option<String>,
    pub w: Option<f64>,
    pub h: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecvPlace {
    pub tid: u32,
    pub x: f64,
    pub y: f64,
    pub dragging: bool,
}

#[derive(Debug)]
pub enum Cmd {
    Hello,
    TransportOpen {
        local_port: u16,
        peer_host: Option<String>,
        peer_port: u16,
    },
    TransportPeer {
        host: String,
        port: u16,
    },
    TransportClose,
    RecvOpen(Box<RecvOpen>),
    RecvUpdate(RecvUpdate),
    RecvPlace(RecvPlace),
    RecvClose {
        tid: u32,
    },
    Shutdown,
    /// Bekannter Befehl, den dieser Helfer (noch) nicht kann.
    Unsupported(String),
}

#[derive(Debug)]
pub struct Request {
    pub id: i64,
    pub cmd: Cmd,
}

/// Fehler beim Parsen. `id` fehlt, wenn die Zeile kein auswertbares `id` hatte.
#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub id: Option<i64>,
    pub code: &'static str,
    pub msg: String,
}

fn bad(id: i64, msg: impl Into<String>) -> ParseError {
    ParseError {
        id: Some(id),
        code: "bad_request",
        msg: msg.into(),
    }
}

fn get_str(o: &Map<String, Value>, k: &str) -> Option<String> {
    o.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn get_u32(o: &Map<String, Value>, k: &str, id: i64) -> Result<u32, ParseError> {
    o.get(k)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| bad(id, format!("{k} fehlt oder ist keine u32")))
}

fn get_port(o: &Map<String, Value>, k: &str, id: i64) -> Result<u16, ParseError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(0),
        Some(v) => v
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| bad(id, format!("{k} ist kein Port"))),
    }
}

fn get_dim(o: &Map<String, Value>, k: &str, id: i64) -> Result<Option<f64>, ParseError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(x) if x.is_finite() && x > 0.0 && x <= 32768.0 => Ok(Some(x)),
            _ => Err(bad(id, format!("{k} ist keine gueltige Groesse"))),
        },
    }
}

fn get_tid(o: &Map<String, Value>, id: i64) -> Result<u32, ParseError> {
    let t = get_u32(o, "tid", id)?;
    if t == 0 {
        return Err(bad(id, "tid 0 ist ungueltig"));
    }
    Ok(t)
}

fn b64_fixed<const N: usize>(
    o: &Map<String, Value>,
    k: &str,
    id: i64,
) -> Result<[u8; N], ParseError> {
    let s = o
        .get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(id, format!("keys.{k} fehlt")))?;
    let v = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| bad(id, format!("keys.{k} ist kein Base64")))?;
    v.try_into()
        .map_err(|_| bad(id, format!("keys.{k} muss {N} Byte lang sein")))
}

pub fn parse_keys(v: Option<&Value>, id: i64) -> Result<StreamKeys, ParseError> {
    let o = v
        .and_then(Value::as_object)
        .ok_or_else(|| bad(id, "keys fehlt"))?;
    Ok(StreamKeys {
        s2r_key: b64_fixed::<32>(o, "s2r_key", id)?,
        s2r_prefix: b64_fixed::<4>(o, "s2r_prefix", id)?,
        r2s_key: b64_fixed::<32>(o, "r2s_key", id)?,
        r2s_prefix: b64_fixed::<4>(o, "r2s_prefix", id)?,
    })
}

/// Hoechstgroesse von `icon_png` (dekodiert).
pub const MAX_ICON_BYTES: usize = 1 << 20;

fn parse_icon(v: Option<&Value>, id: i64) -> Result<Option<Vec<u8>>, ParseError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => {
            let b = base64::engine::general_purpose::STANDARD
                .decode(s)
                .map_err(|_| bad(id, "icon_png ist kein Base64"))?;
            if b.len() > MAX_ICON_BYTES {
                return Err(bad(id, "icon_png ist zu gross"));
            }
            Ok(Some(b))
        }
        Some(_) => Err(bad(id, "icon_png muss ein String sein")),
    }
}

pub fn parse_line(line: &str) -> Result<Request, ParseError> {
    let v: Value = serde_json::from_str(line).map_err(|e| ParseError {
        id: None,
        code: "bad_request",
        msg: format!("kein JSON: {e}"),
    })?;
    let o = v.as_object().ok_or(ParseError {
        id: None,
        code: "bad_request",
        msg: "kein JSON-Objekt".into(),
    })?;
    let id = o.get("id").and_then(Value::as_i64).ok_or(ParseError {
        id: None,
        code: "bad_request",
        msg: "id fehlt".into(),
    })?;
    if o.get("v").and_then(Value::as_i64) != Some(1) {
        return Err(bad(id, "v muss 1 sein"));
    }
    let name = o
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(id, "cmd fehlt"))?;
    let cmd = match name {
        "hello" => Cmd::Hello,
        "transport.open" => Cmd::TransportOpen {
            local_port: get_port(o, "local_port", id)?,
            peer_host: get_str(o, "peer_host").filter(|s| !s.is_empty()),
            peer_port: get_port(o, "peer_port", id)?,
        },
        "transport.peer" => Cmd::TransportPeer {
            host: get_str(o, "peer_host").ok_or_else(|| bad(id, "peer_host fehlt"))?,
            port: get_port(o, "peer_port", id)?,
        },
        "transport.close" => Cmd::TransportClose,
        "recv.open" => {
            let tid = get_tid(o, id)?;
            let codec_s = get_str(o, "codec").ok_or_else(|| bad(id, "codec fehlt"))?;
            let codec = Codec::parse(&codec_s)
                .ok_or_else(|| bad(id, format!("codec {codec_s} unbekannt")))?;
            let w = get_dim(o, "w", id)?.ok_or_else(|| bad(id, "w fehlt"))?;
            let h = get_dim(o, "h", id)?.ok_or_else(|| bad(id, "h fehlt"))?;
            let scale = match o.get("scale").and_then(Value::as_f64) {
                Some(s) if s.is_finite() && s > 0.0 => s,
                None => 1.0,
                Some(_) => return Err(bad(id, "scale ungueltig")),
            };
            Cmd::RecvOpen(Box::new(RecvOpen {
                tid,
                title: get_str(o, "title").unwrap_or_default(),
                app: get_str(o, "app").unwrap_or_default(),
                w,
                h,
                scale,
                codec,
                fullscreen: o
                    .get("fullscreen")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                keys: parse_keys(o.get("keys"), id)?,
                icon_png: parse_icon(o.get("icon_png"), id)?,
            }))
        }
        "recv.update" => Cmd::RecvUpdate(RecvUpdate {
            tid: get_tid(o, id)?,
            title: get_str(o, "title"),
            w: get_dim(o, "w", id)?,
            h: get_dim(o, "h", id)?,
        }),
        "recv.place" => {
            let x = o
                .get("x")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .ok_or_else(|| bad(id, "x fehlt"))?;
            let y = o
                .get("y")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .ok_or_else(|| bad(id, "y fehlt"))?;
            Cmd::RecvPlace(RecvPlace {
                tid: get_tid(o, id)?,
                x,
                y,
                dragging: o
                    .get("dragging")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        }
        "recv.close" => Cmd::RecvClose {
            tid: get_tid(o, id)?,
        },
        "shutdown" => Cmd::Shutdown,
        other if LATER_CMDS.contains(&other) => Cmd::Unsupported(other.to_owned()),
        other => return Err(bad(id, format!("unbekannter Befehl {other}"))),
    };
    Ok(Request { id, cmd })
}

/// Fenstertitel nach C03: `<app> – <title>` (Halbgeviertstrich).
pub fn window_title(app: &str, title: &str) -> String {
    match (app.is_empty(), title.is_empty()) {
        (false, false) => format!("{app} \u{2013} {title}"),
        (false, true) => app.to_owned(),
        (true, false) => title.to_owned(),
        (true, true) => "Beamer Teleport".to_owned(),
    }
}

pub fn ok(id: i64, extra: Value) -> Value {
    let mut m = Map::new();
    m.insert("v".into(), json!(1));
    m.insert("re".into(), json!(id));
    m.insert("ok".into(), json!(true));
    if let Value::Object(e) = extra {
        m.extend(e);
    }
    Value::Object(m)
}

pub fn err(id: i64, code: &str, msg: &str) -> Value {
    json!({"v":1,"re":id,"ok":false,"error":{"code":code,"msg":msg}})
}

pub fn event(name: &str, fields: Value) -> Value {
    let mut m = Map::new();
    m.insert("v".into(), json!(1));
    m.insert("ev".into(), json!(name));
    if let Value::Object(e) = fields {
        m.extend(e);
    }
    Value::Object(m)
}

pub fn hello_reply(id: i64) -> Value {
    ok(
        id,
        json!({"helper_version": HELPER_VERSION, "platform": "win", "caps": ["recv"]}),
    )
}

/// Gemeinsamer Ausgang fuer stdout. Jede Nachricht ist genau eine Zeile; mehrere Threads
/// (Hauptthread, Transport-Thread fuer `stats`) schreiben ueber denselben Mutex.
#[derive(Clone)]
pub struct Out {
    w: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Out {
    pub fn new(w: Box<dyn Write + Send>) -> Self {
        Out {
            w: Arc::new(Mutex::new(w)),
        }
    }

    pub fn stdout() -> Self {
        Out::new(Box::new(std::io::stdout()))
    }

    pub fn send(&self, v: &Value) {
        let mut line = v.to_string();
        line.push('\n');
        let mut w = self.w.lock().unwrap_or_else(|e| e.into_inner());
        // Ein Schreibfehler heisst: Python ist weg. Das Aufraeumen kommt dann ueber stdin-EOF.
        let _ = w.write_all(line.as_bytes());
        let _ = w.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_json() -> Value {
        let b = |n: usize, x: u8| base64::engine::general_purpose::STANDARD.encode(vec![x; n]);
        json!({"s2r_key": b(32, 1), "s2r_prefix": b(4, 2), "r2s_key": b(32, 3), "r2s_prefix": b(4, 4)})
    }

    #[test]
    fn hello() {
        let r = parse_line(r#"{"v":1,"id":7,"cmd":"hello","beamer_version":"9"}"#).unwrap();
        assert_eq!(r.id, 7);
        assert!(matches!(r.cmd, Cmd::Hello));
        let a = hello_reply(7);
        assert_eq!(a["re"], 7);
        assert_eq!(a["ok"], true);
        assert_eq!(a["platform"], "win");
        assert_eq!(a["caps"], json!(["recv"]));
    }

    #[test]
    fn recv_place_parses() {
        let line = json!({"v":1,"id":2,"cmd":"recv.place","tid":9,"x":120.5,"y":80.0,"dragging":true})
            .to_string();
        let Cmd::RecvPlace(p) = parse_line(&line).unwrap().cmd else {
            panic!("expected RecvPlace")
        };
        assert_eq!(p.tid, 9);
        assert_eq!(p.x, 120.5);
        assert_eq!(p.y, 80.0);
        assert!(p.dragging);
    }

    #[test]
    fn recv_open_full() {
        let line = json!({"v":1,"id":3,"cmd":"recv.open","tid":5,"title":"Doc","app":"Safari",
            "w":800,"h":600.5,"scale":2.0,"codec":"hevc","fullscreen":false,"keys":keys_json()})
        .to_string();
        let Cmd::RecvOpen(o) = parse_line(&line).unwrap().cmd else {
            panic!()
        };
        assert_eq!(o.tid, 5);
        assert_eq!(o.codec, Codec::Hevc);
        assert_eq!(o.w, 800.0);
        assert_eq!(o.h, 600.5);
        assert_eq!(o.keys.s2r_key, [1; 32]);
        assert_eq!(o.keys.r2s_prefix, [4; 4]);
        // Debug-Ausgabe verraet keine Schluessel
        assert!(!format!("{o:?}").contains("s2r_key"));
    }

    #[test]
    fn recv_open_rejects_bad_keys_and_codec() {
        let mut k = keys_json();
        k["s2r_key"] = json!(base64::engine::general_purpose::STANDARD.encode([0u8; 31]));
        let line =
            json!({"v":1,"id":3,"cmd":"recv.open","tid":5,"w":8,"h":6,"codec":"h264","keys":k})
                .to_string();
        let e = parse_line(&line).unwrap_err();
        assert_eq!(e.id, Some(3));
        assert_eq!(e.code, "bad_request");
        let line = json!({"v":1,"id":4,"cmd":"recv.open","tid":5,"w":8,"h":6,"codec":"vp9","keys":keys_json()})
            .to_string();
        assert!(parse_line(&line).unwrap_err().msg.contains("vp9"));
        let line = json!({"v":1,"id":4,"cmd":"recv.open","tid":0,"w":8,"h":6,"codec":"h264","keys":keys_json()})
            .to_string();
        assert!(parse_line(&line).is_err());
    }

    #[test]
    fn icon_png_optional_and_limited() {
        let base = json!({"v":1,"id":3,"cmd":"recv.open","tid":5,"w":8,"h":6,"codec":"h264","keys":keys_json()});
        let with = |icon: Value| {
            let mut v = base.clone();
            v["icon_png"] = icon;
            parse_line(&v.to_string())
        };
        let get = |r: Result<Request, ParseError>| match r.unwrap().cmd {
            Cmd::RecvOpen(o) => o.icon_png,
            _ => panic!(),
        };
        assert_eq!(get(parse_line(&base.to_string())), None);
        assert_eq!(get(with(json!(""))), None);
        assert_eq!(
            get(with(json!("iVBORw0K"))),
            Some(vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A])
        );
        assert!(with(json!("%%%")).is_err());
        assert!(with(json!(7)).is_err());
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_ICON_BYTES + 1]);
        assert!(with(json!(big)).is_err());
    }

    #[test]
    fn transport_and_update() {
        let r = parse_line(r#"{"v":1,"id":1,"cmd":"transport.open","local_port":47001,"peer_host":"","peer_port":0}"#)
            .unwrap();
        match r.cmd {
            Cmd::TransportOpen {
                local_port,
                peer_host,
                peer_port,
            } => {
                assert_eq!((local_port, peer_host, peer_port), (47001, None, 0));
            }
            _ => panic!(),
        }
        let r = parse_line(
            r#"{"v":1,"id":2,"cmd":"transport.peer","peer_host":"169.254.1.2","peer_port":5000}"#,
        )
        .unwrap();
        assert!(
            matches!(r.cmd, Cmd::TransportPeer { ref host, port: 5000 } if host == "169.254.1.2")
        );
        let r = parse_line(r#"{"v":1,"id":3,"cmd":"recv.update","tid":9,"title":"x"}"#).unwrap();
        match r.cmd {
            Cmd::RecvUpdate(u) => assert_eq!(
                u,
                RecvUpdate {
                    tid: 9,
                    title: Some("x".into()),
                    w: None,
                    h: None
                }
            ),
            _ => panic!(),
        }
        assert!(parse_line(r#"{"v":1,"id":3,"cmd":"transport.open","local_port":70000}"#).is_err());
    }

    #[test]
    fn unsupported_and_unknown() {
        for c in LATER_CMDS {
            let r = parse_line(&format!(r#"{{"v":1,"id":1,"cmd":"{c}"}}"#)).unwrap();
            assert!(matches!(r.cmd, Cmd::Unsupported(ref n) if n == c));
        }
        let e = parse_line(r#"{"v":1,"id":1,"cmd":"frobnicate"}"#).unwrap_err();
        assert_eq!(e.code, "bad_request");
        assert_eq!(parse_line("nope").unwrap_err().id, None);
        assert_eq!(
            parse_line(r#"{"v":2,"id":5,"cmd":"hello"}"#)
                .unwrap_err()
                .id,
            Some(5)
        );
    }

    #[test]
    fn title_and_messages() {
        assert_eq!(window_title("Safari", "Start"), "Safari \u{2013} Start");
        assert_eq!(window_title("", "Start"), "Start");
        assert_eq!(window_title("Safari", ""), "Safari");
        let e = err(4, "not_found", "x");
        assert_eq!(e["error"]["code"], "not_found");
        assert_eq!(e["ok"], false);
        let ev = event("proxy.closed", json!({"tid": 3}));
        assert_eq!(ev, json!({"v":1,"ev":"proxy.closed","tid":3}));
    }

    #[test]
    fn codec_bits() {
        for c in [Codec::H264, Codec::Hevc] {
            assert_eq!(Codec::from_fflags(c.fflags_bits() | 1), Some(c));
        }
        assert_eq!(Codec::from_fflags(3 << 1), None);
    }

    #[test]
    fn out_writes_one_line_per_message() {
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let b = Buf::default();
        let out = Out::new(Box::new(b.clone()));
        out.send(&event("stats", json!({"tid": 1, "title": "a\nb"})));
        out.send(&ok(2, json!({})));
        let s = String::from_utf8(b.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .all(|l| serde_json::from_str::<Value>(l).is_ok()));
    }
}
