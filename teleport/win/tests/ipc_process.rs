//! Der Helfer als Prozess: Protokoll auf stdout, Fehlerfaelle, EOF beendet binnen 2 s mit Code 0.
//! Braucht keine GPU (es wird kein Fenster geoeffnet).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

#[test]
fn protocol_and_eof() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_beamer-teleport"))
        .args(["--ipc", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let lines = [
        r#"{"v":1,"id":1,"cmd":"hello","beamer_version":"test"}"#,
        r#"{"v":1,"id":2,"cmd":"windows.list"}"#,
        "kein json",
        r#"{"v":1,"id":3,"cmd":"recv.close","tid":5}"#,
        r#"{"v":1,"id":4,"cmd":"transport.peer","peer_host":"127.0.0.1","peer_port":9}"#,
        r#"{"v":1,"id":5,"cmd":"transport.open","local_port":0,"peer_host":"","peer_port":0}"#,
        r#"{"v":1,"id":6,"cmd":"transport.peer","peer_host":"127.0.0.1","peer_port":9}"#,
        r#"{"v":1,"id":7,"cmd":"recv.open","tid":1,"w":10,"h":10,"codec":"h264"}"#,
        r#"{"v":1,"id":8,"cmd":"transport.close"}"#,
    ];
    for l in lines {
        writeln!(stdin, "{l}").unwrap();
    }
    stdin.flush().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut msgs: Vec<Value> = Vec::new();
    for _ in 0..lines.len() {
        let mut s = String::new();
        out.read_line(&mut s).unwrap();
        msgs.push(serde_json::from_str(&s).expect("nur JSON auf stdout"));
    }
    let re = |id: i64| msgs.iter().find(|m| m["re"] == id).cloned().unwrap();
    assert_eq!(re(1)["platform"], "win");
    assert_eq!(re(1)["caps"][0], "recv");
    assert_eq!(re(2)["error"]["code"], "unsupported");
    assert!(msgs
        .iter()
        .any(|m| m["ev"] == "error" && m["code"] == "bad_request"));
    assert_eq!(re(3)["error"]["code"], "not_found");
    assert_eq!(re(4)["ok"], false);
    assert!(re(5)["local_port"].as_u64().unwrap() > 0);
    assert_eq!(re(6)["ok"], true);
    assert_eq!(re(7)["error"]["code"], "bad_request"); // keys fehlen
    assert_eq!(re(8)["ok"], true);

    let t = Instant::now();
    drop(stdin);
    let code = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s.code();
        }
        assert!(t.elapsed() < Duration::from_secs(2), "kein Ende nach EOF");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(code, Some(0));
}

#[test]
fn shutdown_replies_then_exits() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_beamer-teleport"))
        .args(["--ipc", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"v":1,"id":9,"cmd":"shutdown"}}"#).unwrap();
    stdin.flush().unwrap();
    let mut s = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut s)
        .unwrap();
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!((v["re"].as_i64(), v["ok"].as_bool()), (Some(9), Some(true)));
    assert_eq!(child.wait().unwrap().code(), Some(0));
}
