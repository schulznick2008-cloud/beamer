# Teleport – Abhängigkeiten und Lizenzen

Der Fork bleibt **GPL-3.0**. Hier wird jede neue Abhängigkeit der Teleport-Teile eingetragen (Pflicht im selben PR).

Erlaubt: MIT, BSD-2/3-Clause, Apache-2.0, MPL-2.0, ISC, Zlib, LGPL-2.1+/3, GPL-2.0-or-later, GPL-3.0.
Verboten: AGPL, SSPL, proprietär, „source-available“, Code ohne klare Lizenz.

| Name | Version | Lizenz | Teil (transport/mac/win/python/tools) | Wofür | Eingebunden wie (crate/SwiftPM/DLL/Systemframework/Werkzeug) |
|---|---|---|---|---|---|
| chacha20poly1305 | 0.10 | Apache-2.0 OR MIT | transport | AEAD ChaCha20-Poly1305 (IETF) für den UDP-Strom | crate (RustCrypto) |
| socket2 | 0.5 | MIT OR Apache-2.0 | transport | UDP-Socket mit großen Puffern binden | crate |
| cbindgen | 0.27 | MPL-2.0 | transport (nur Build) | erzeugt `include/beamer_transport.h` aus `src/ffi.rs`, landet nicht im Binary | crate (build-dependency) |
| windows (windows-rs) | 0.62 | MIT OR Apache-2.0 | win | Win32/D3D11/DXGI/Firewall-COM-Bindings | crate |
| rusty_ffmpeg | 0.17.0+ffmpeg.8.1 | MIT | win | FFI-Bindings an libavcodec/libavutil (Decode D3D11VA, Selbsttest-Encoder) | crate |
| bindgen (über rusty_ffmpeg) | 0.72 | BSD-3-Clause | win (nur Build) | erzeugt die FFmpeg-Bindings aus den Headern, landet nicht im Binary | crate (build-dependency) |
| LLVM/libclang | 18+ (getestet 23.1) | Apache-2.0 WITH LLVM-exception | win (nur Build) | von bindgen geladen | Werkzeug |
| serde_json | 1 | MIT OR Apache-2.0 | win | JSON Lines (IPC) | crate |
| base64 | 0.22 | MIT OR Apache-2.0 | win | Schlüssel in `recv.open` | crate |
| FFmpeg (BtbN-Build `ffmpeg-n8.1-latest-win64-lgpl-shared-8.1`) | 8.1 | LGPL-3.0-or-later (`--enable-version3`, ohne `--enable-gpl`/`--enable-nonfree`) | win | D3D11VA-Decode (H.264/HEVC); im Selbsttest AMF-/Media-Foundation-Encoder | DLL, dynamisch gelinkt: `avcodec-62.dll`, `avutil-60.dll`, `avformat-62.dll`, `avdevice-62.dll`, `avfilter-11.dll`, `swresample-6.dll`, `swscale-9.dll` neben `beamer-teleport.exe`; geholt per `teleport/win/scripts/fetch-ffmpeg.ps1`, nicht im Repo |
| OpenH264 (in den FFmpeg-DLLs) | 2.x | BSD-2-Clause | win (nur Selbsttest) | Software-Encoder als letzter Ausweg von `--selftest-send` (mit Warnung) | Teil der FFmpeg-DLLs |

Systemframeworks (ScreenCaptureKit, VideoToolbox, Windows Graphics Capture, D3D11, Media Foundation) fallen unter die Systembibliotheks-Ausnahme der GPL und werden nur der Vollständigkeit halber genannt.

Transitive Abhängigkeiten von `beamer-transport` (aead, cipher, chacha20, poly1305, universal-hash, crypto-common, generic-array, subtle, zeroize, typenum, libc u. a.) stehen unter MIT, Apache-2.0, BSD-3-Clause oder Zlib-nahen Lizenzen; für den Build-Teil von cbindgen kommen serde, syn, toml u. a. (MIT/Apache-2.0) dazu. Stand `cargo tree -e normal,build` vom 2026-10-11, keine AGPL/SSPL/proprietär.

**win (C03):** Transitive Crates von `beamer-teleport-win` (windows-core, windows-targets, serde, itoa, ryu, memchr u. a.; im Build zusätzlich bindgen, clang-sys, cexpr, regex, libloading, unicode-ident) stehen unter MIT, Apache-2.0, BSD-3-Clause, ISC, Unlicense-OR-MIT (MIT gewählt) bzw. Unicode-3.0 (nur Build). Stand `cargo metadata` vom 2026-10-11, keine AGPL/SSPL/proprietär.

**FFmpeg-DLLs (LGPL):** dynamisch gelinkt und austauschbar. Wer `beamer-teleport.exe` mit den DLLs weitergibt, legt `LICENSE.txt` aus dem FFmpeg-Ordner bei und nennt die Bezugsquelle (Quellcode: https://ffmpeg.org, Build-Skripte: https://github.com/BtbN/FFmpeg-Builds). Die DLLs enthalten weitere statisch gebaute Bibliotheken des LGPL-Builds (u. a. dav1d, libvpx, opus, OpenH264, AMF-Header), alle unter LGPL-kompatiblen Lizenzen; der „gpl“-Build von BtbN wird nicht verwendet.
