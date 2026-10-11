//! Der eingecheckte C-Header muss dem entsprechen, was cbindgen aus src/ffi.rs erzeugt.
//! Aktualisieren: `BEAMER_UPDATE_HEADER=1 cargo build`.

#[test]
fn committed_header_is_current() {
    let generated =
        std::fs::read_to_string(env!("BT_GENERATED_HEADER")).expect("erzeugter Header fehlt");
    let committed = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/include/beamer_transport.h"
    ))
    .expect("include/beamer_transport.h fehlt");
    let norm = |s: &str| s.replace("\r\n", "\n");
    assert_eq!(
        norm(&generated),
        norm(&committed),
        "include/beamer_transport.h ist veraltet: BEAMER_UPDATE_HEADER=1 cargo build"
    );
    for sig in [
        "bt_set_max_kbps(BtSession *s, uint32_t stream_id, uint32_t max_kbps)",
        "bt_open(uint16_t local_port",
        "uint32_t role",
    ] {
        assert!(committed.contains(sig), "{sig} fehlt im Header");
    }
}
