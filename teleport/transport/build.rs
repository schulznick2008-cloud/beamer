// Erzeugt include/beamer_transport.h aus src/ffi.rs. Scheitert die Erzeugung (z. B. weil cbindgen
// den Quelltext gerade nicht parsen kann), bleibt der eingecheckte Header stehen und der Build geht weiter.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let cfg = cbindgen::Config::from_file(dir.join("cbindgen.toml")).unwrap_or_default();
    match cbindgen::Builder::new()
        .with_src(dir.join("src").join("ffi.rs"))
        .with_config(cfg)
        .generate()
    {
        Ok(b) => {
            b.write_to_file(dir.join("include").join("beamer_transport.h"));
        }
        Err(e) => println!("cargo:warning=cbindgen: {e}"),
    }
}
