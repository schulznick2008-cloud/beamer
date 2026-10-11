# Teleport – Abhängigkeiten und Lizenzen

Der Fork bleibt **GPL-3.0**. Hier wird jede neue Abhängigkeit der Teleport-Teile eingetragen (Pflicht im selben PR).

Erlaubt: MIT, BSD-2/3-Clause, Apache-2.0, MPL-2.0, ISC, Zlib, LGPL-2.1+/3, GPL-2.0-or-later, GPL-3.0.
Verboten: AGPL, SSPL, proprietär, „source-available“, Code ohne klare Lizenz.

| Name | Version | Lizenz | Teil (transport/mac/win/python/tools) | Wofür | Eingebunden wie (crate/SwiftPM/DLL/Systemframework/Werkzeug) |
|---|---|---|---|---|---|
| chacha20poly1305 | 0.10 | Apache-2.0 OR MIT | transport | AEAD ChaCha20-Poly1305 (IETF) für den UDP-Strom | crate (RustCrypto) |
| socket2 | 0.5 | MIT OR Apache-2.0 | transport | UDP-Socket mit großen Puffern binden | crate |
| cbindgen | 0.27 | MPL-2.0 | transport (nur Build) | erzeugt `include/beamer_transport.h` aus `src/ffi.rs`, landet nicht im Binary | crate (build-dependency) |

Systemframeworks (ScreenCaptureKit, VideoToolbox, Windows Graphics Capture, D3D11, Media Foundation) fallen unter die Systembibliotheks-Ausnahme der GPL und werden nur der Vollständigkeit halber genannt.

Transitive Abhängigkeiten von `beamer-transport` (aead, cipher, chacha20, poly1305, universal-hash, crypto-common, generic-array, subtle, zeroize, typenum, libc u. a.) stehen unter MIT, Apache-2.0, BSD-3-Clause oder Zlib-nahen Lizenzen; für den Build-Teil von cbindgen kommen serde, syn, toml u. a. (MIT/Apache-2.0) dazu. Stand `cargo tree -e normal,build` vom 2026-10-11, keine AGPL/SSPL/proprietär.
