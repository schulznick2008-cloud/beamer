//! Kodieren (Selbsttest-Encoder) und mit D3D11VA dekodieren, ohne Fenster und Netz.
//! Ohne Hardware-GPU mit Video-Decode (z. B. CI-Runner) wird der Test uebersprungen.

use beamer_teleport_win::decode::Decoder;
use beamer_teleport_win::gpu::Gpu;
use beamer_teleport_win::ipc::Codec;
use beamer_teleport_win::selftest::{Encoder, Opts};

fn roundtrip(codec: Codec, encoder: Option<&str>) {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("uebersprungen: keine GPU ({e})");
            return;
        }
    };
    let mut dec = match Decoder::new(&gpu, codec) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("uebersprungen: kein D3D11VA ({e})");
            return;
        }
    };
    let o = Opts {
        width: 640,
        height: 360,
        fps: 30,
        kbps: 4000,
        codec,
        encoder: encoder.map(str::to_owned),
        ..Opts::default()
    };
    let mut enc = match Encoder::open(&o) {
        Ok(e) => e,
        // Ausdruecklich verlangter Encoder (im FFmpeg-Build enthalten) muss gehen.
        Err(e) if encoder.is_some() => panic!("{e}"),
        Err(e) => {
            eprintln!("uebersprungen: kein Hardware-Encoder ({e})");
            return;
        }
    };
    let mut decoded = 0;
    let mut keys = 0;
    for n in 0..30u64 {
        for (data, key) in enc.encode(n, n * 33, n == 0 || n == 20).unwrap() {
            keys += key as u32;
            if let Some(d) = dec.decode(&data, key).unwrap() {
                assert_eq!(d.size(), (640, 360));
                decoded += 1;
            }
        }
    }
    assert!(keys >= 2, "erzwungene Keyframes fehlen ({keys})");
    // Ohne B-Frames und mit LOW_DELAY kommt jedes Bild sofort wieder heraus.
    assert!(decoded >= 28, "nur {decoded} von 30 Bildern dekodiert");
}

#[test]
fn h264_roundtrip() {
    roundtrip(Codec::H264, None);
}

#[test]
fn hevc_roundtrip() {
    roundtrip(Codec::Hevc, None);
}

#[test]
fn h264_software_encoder_roundtrip() {
    // OpenH264 steckt im LGPL-Build und ist der letzte Ausweg des Selbsttests.
    roundtrip(Codec::H264, Some("libopenh264"));
}

#[test]
fn garbage_after_keyframe_does_not_panic() {
    let Ok(gpu) = Gpu::new() else { return };
    let Ok(mut dec) = Decoder::new(&gpu, Codec::H264) else {
        return;
    };
    // Muell darf einen Fehler oder kein Bild liefern, aber nie abstuerzen.
    let junk = [0u8, 0, 0, 1, 0x41, 0xff, 0x13, 0x37, 0, 0, 1, 0x65, 1, 2, 3];
    for _ in 0..5 {
        let _ = dec.decode(&junk, false);
    }
    dec.flush();
}
