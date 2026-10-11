// Erzeugt den C-Header aus src/ffi.rs nach $OUT_DIR. In den Quellbaum (include/beamer_transport.h)
// wird nur geschrieben, wenn BEAMER_UPDATE_HEADER=1 gesetzt ist; sonst vergleicht tests/header.rs den
// eingecheckten Header mit dem frisch erzeugten (so faellt ein vergessenes Aktualisieren in der CI auf).
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-env-changed=BEAMER_UPDATE_HEADER");
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("beamer_transport.h");
    let cfg = cbindgen::Config::from_file(dir.join("cbindgen.toml")).unwrap_or_default();
    match cbindgen::Builder::new()
        .with_src(dir.join("src").join("ffi.rs"))
        .with_config(cfg)
        .generate()
    {
        Ok(b) => {
            b.write_to_file(&out);
            if std::env::var("BEAMER_UPDATE_HEADER").is_ok_and(|v| v == "1") {
                b.write_to_file(dir.join("include").join("beamer_transport.h"));
            }
        }
        Err(e) => println!("cargo:warning=cbindgen: {e}"),
    }
    println!("cargo:rustc-env=BT_GENERATED_HEADER={}", out.display());
}
