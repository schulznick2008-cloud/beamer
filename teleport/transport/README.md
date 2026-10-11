# beamer-transport

Rust-Bibliothek für den verschlüsselten UDP-Medienstrom von Beamer-Teleport.
Spezifikation: `docs/teleport/SCHNITTSTELLEN.md` §4 (Strom) und §5 (C-ABI). Crate-Typ `rlib` + `staticlib`.

- **Paket:** 16-Byte-Header (= AAD), ChaCha20-Poly1305, Nonce = Praefix ‖ seq, höchstens 1200 Byte je Datagramm,
  Replay-Fenster 4096.
- **Fragmentieren/Zusammensetzen**, Puffer zum erneuten Senden (300 ms), **NACK**, **XOR-FEC** (1 Parität je 8 Fragmente),
  `keyframe_req` (höchstens alle 300 ms), `feedback` jede Sekunde, `ping`/`pong` für die RTT, Bitratenregelung
  (`bt_target_kbps`).
- Ein UDP-Socket (IPv4) und ein Transport-Thread je Session, beliebig viele Streams. Callbacks laufen im Transport-Thread,
  nie unter einem Lock, damit sie `bt_send_frame` aufrufen dürfen.
- Video wird in Reihenfolge ausgeliefert. Geht ein Frame verloren (Frist 2 × RTT, mindestens 30 ms ab dem letzten Paket des
  Frames, höchstens 500 ms), gibt es `keyframe_req`, und bis zum nächsten Keyframe werden keine abhängigen Frames geliefert.

## Benutzen

C/Swift: `include/beamer_transport.h` (von cbindgen erzeugt, beim Build aktualisiert) und `libbeamer_transport.a`
aus `target/release/`. Rust: `beamer_transport::Session`.

```
cd teleport/transport
cargo test
cargo clippy --all-targets -- -D warnings
cargo run --release --example loopback -- --loss 3 --jitter 10 --secs 30
cargo build --release --target x86_64-pc-windows-msvc   # Windows (rustup target add x86_64-pc-windows-msvc)
```

`loopback` misst Zustellquote und Latenz über 127.0.0.1 mit künstlicher Stoerung auf der Empfangsseite. Optionen:
`--loss <%> --jitter <ms> --reorder <%> --secs <s> --fps <n> --max-kbps <n> --no-fec --one-way --no-baseline --stats --seed <n>`.
Die Zusatzlatenz ist der Median gegen einen verlustfreien Basislauf mit demselben Jitter.

## Hinweise für Nutzer der C-ABI

- Rückgabe 0 = OK, < 0 = Fehler, Text über `bt_last_error()` (thread-lokal, gültig bis zum nächsten Aufruf im selben Thread).
- `bt_close` nicht aus einem Callback aufrufen, wenn man auf das Ende warten will; es wartet dann nicht auf sich selbst.
- Der `BtFrame`-Zeiger und `data` sind nur während des Callbacks gültig. Daten kopieren und zurückkehren.
- Der Stats-JSON enthält die Felder aus §1.2 (`fps`, `kbps`, `rtt_ms`, `loss_pct`, `latency_ms`) und zusätzlich
  `role`, `target_kbps`, `jitter_ms`, `frames_*`, `retransmits`, `nacks`, `fec_recovered`, `auth_fail`, `peer_recv_kbps`.
  Beim Sender sind `rtt_ms` und `latency_ms` 0 (die RTT misst nur der Empfänger).
- Nur im Rust-API, nicht in der C-ABI: `Session::set_fec_enabled`, `Session::set_netem`, `Session::counters`.

## Auslegung des Vertrags (siehe REPORT im PR)

- FEC-Paket: `frame_id` ‖ `group_start` ‖ `group_len` ‖ **XOR der Klartextlängen (u16)** ‖ XOR-Parität. §4.1 sagt
  „die Länge als u16 je Quelle“; hier wird die Länge einmal mitge-XORt, das reicht für die Rekonstruktion eines Verlusts
  und spart 2 Byte je Quelle.
- `frame_id` zählt je Stream und Art (Video/Audio) getrennt ab 1. FEC gibt es nur für Video.
- Fragment-Nutzlast höchstens 1141 Byte, damit das zugehörige FEC-Paket noch in 1200 Byte passt.
