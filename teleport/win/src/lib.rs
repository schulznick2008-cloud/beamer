//! Windows-Helfer fuer Beamer-Teleport (Paket C03: Empfaenger).
//!
//! Binary `beamer-teleport.exe --ipc stdio` nach `docs/teleport/SCHNITTSTELLEN.md` §1. Empfaengt Streams
//! ueber `beamer-transport`, dekodiert sie mit D3D11VA (FFmpeg) und zeigt jeden `tid` in einem eigenen
//! Fenster. `--selftest-send <host> <port>` sendet ein synthetisches Testbild.

pub mod app;
pub mod clock;
pub mod decode;
pub mod firewall;
pub mod gpu;
pub mod ipc;
pub mod receiver;
pub mod selftest;
pub mod stats;
pub mod window;
