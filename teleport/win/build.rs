//! Kopiert die FFmpeg-DLLs neben das Binary (und neben die Test-Binaries in deps/), damit
//! `cargo run` und `cargo test` ohne PATH-Aenderung laufen.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let bin = manifest.join("third_party").join("ffmpeg").join("bin");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", bin.display());
    if !bin.is_dir() {
        println!(
            "cargo:warning=FFmpeg fehlt unter {} - zuerst scripts/fetch-ffmpeg.ps1 ausfuehren",
            bin.display()
        );
        return;
    }
    // OUT_DIR = target/<profil>/build/<crate>-<hash>/out
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile) = out.ancestors().nth(3) else {
        return;
    };
    for dir in [
        profile.to_path_buf(),
        profile.join("deps"),
        profile.join("examples"),
    ] {
        let _ = std::fs::create_dir_all(&dir);
        copy_dlls(&bin, &dir);
    }
}

fn copy_dlls(from: &Path, to: &Path) {
    let Ok(rd) = std::fs::read_dir(from) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
            let dst = to.join(p.file_name().unwrap());
            let fresh = std::fs::metadata(&dst)
                .and_then(|m| m.modified())
                .ok()
                .zip(e.metadata().and_then(|m| m.modified()).ok())
                .is_some_and(|(d, s)| d >= s);
            if !fresh {
                let _ = std::fs::copy(&p, &dst);
            }
        }
    }
}
