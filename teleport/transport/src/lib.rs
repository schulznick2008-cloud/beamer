//! beamer-transport: verschluesselter UDP-Medienstrom fuer Beamer-Teleport.
//!
//! Spezifikation: `docs/teleport/SCHNITTSTELLEN.md` §4 (Strom) und §5 (C-ABI).
//! Rust-Nutzer verwenden [`Session`]; C/Swift-Nutzer die Funktionen aus `ffi` (Header in `include/`).

pub mod ffi;
pub mod netem;
pub mod packet;
pub mod replay;
pub mod session;
pub mod stream;
mod timer;

pub use netem::NetemConfig;
pub use session::{Callbacks, FrameRef, Session, SessionCounters, StreamKeys};
pub use stream::{Counters, Role};
