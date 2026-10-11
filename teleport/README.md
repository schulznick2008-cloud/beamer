# Teleport (Beamer-Fork)

Einzelne Fenster in beide Richtungen zwischen Mac und Windows streamen und bedienen, plus Vollbild-Stream.
Plan und Verträge: [`docs/teleport/`](../docs/teleport/) – verbindlich ist `SCHNITTSTELLEN.md`.

| Ordner | Inhalt | Paket |
|---|---|---|
| `transport/` | Rust-Crate `beamer-transport` (UDP, AEAD, NACK, FEC, C-ABI) | C01 |
| `mac/` | SwiftPM-Paket `BeamerTeleport` (Capture, Encode, Anzeige, Eingabe) | C02, C04, C07, C08 |
| `win/` | Rust-Crate `beamer-teleport-win` (WGC, AMF, D3D11, Eingabe) | C03, C05, C06, C09 |
