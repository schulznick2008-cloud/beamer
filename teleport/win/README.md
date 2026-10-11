# beamer-teleport für Windows (C03: Empfänger)

Rust-Crate `beamer-teleport-win` (windows-rs), Binary `beamer-teleport.exe`. Spezifikation:
`docs/teleport/SCHNITTSTELLEN.md` §1, §4, §5, §7.

Stand C03: Empfangen über `beamer-transport` (rlib), Hardware-Decode H.264/HEVC mit D3D11VA (FFmpeg),
Anzeige je `tid` in einem eigenen Fenster. Eingabe, Fokus und Größen-Rückkanal (`proxy.input/focus/resize`)
kommen mit C05, der Sender mit C06.

## Bauen

Voraussetzungen: Rust (stable, MSVC), Visual Studio Build Tools (C++ und Windows SDK), LLVM/libclang
(für bindgen; Standardpfad `C:\Program Files\LLVM\bin`, sonst `LIBCLANG_PATH` setzen).

```powershell
cd teleport\win
.\scripts\fetch-ffmpeg.ps1          # FFmpeg 8.1, LGPL-shared, nach third_party\ffmpeg (nicht im Repo)
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

`build.rs` kopiert die FFmpeg-DLLs (`avcodec-62`, `avutil-60`, `avformat-62`, `avdevice-62`, `avfilter-11`,
`swresample-6`, `swscale-9`) neben das Binary. Zum Weitergeben gehören sie und `third_party\ffmpeg\LICENSE.txt`
neben `beamer-teleport.exe` (LGPL, siehe `docs/teleport/DEPENDENCIES.md`).

## Aufruf

```
beamer-teleport.exe --ipc stdio [--same-clock] [--test-loss PCT]
beamer-teleport.exe --selftest-send <host> <port> [--tid N] [--fps N] [--size BxH] [--codec h264|hevc]
                    [--kbps N] [--secs S] [--encoder NAME] [--local-port N]
```

- `--ipc stdio`: JSON Lines nach §1. Befehle: `hello`, `transport.open/peer/close`, `recv.open/update/close`,
  `shutdown`. Sender- und `proxy.*`-Befehle werden mit `unsupported` beantwortet. Ereignisse: `proxy.closed`
  (Fenster geschlossen), `stats` (jede Sekunde), `error`. stdin-EOF: Aufräumen und Exit 0 (gemessen < 0,1 s).
- `stats` enthält die Transportwerte (`fps`, `kbps`, `rtt_ms`, `loss_pct`, `peer`, …) und zusätzlich
  `decode_ms`, `present_ms`, `recv_to_present_ms` (+ `_p95_ms`, Empfang des letzten Fragments bis Rückkehr von
  `Present`), `frames_presented`, `frames_skipped`, `decode_errors`, `decoder`. `latency_ms` = Haltezeit im
  Transport + Empfang→Present.
- `--same-clock` (nur Selbsttest auf einem Rechner): `stats.same_clock_latency_ms` = Alter des Bildes seit
  `pts_us` des Senders (QueryPerformanceCounter, gilt nur, wenn Sender und Empfänger dieselbe Uhr haben).
- `--test-loss`: künstlicher Paketverlust im Transport, um NACK/FEC/Keyframe-Erholung zu prüfen.
- `--selftest-send`: Testbild (Zähler und Zeitstempel in ms als große Ziffern, wandernder Balken, Rauschfeld)
  mit AMF kodieren (dann Media Foundation, zuletzt OpenH264 mit Warnung) und mit **festen Testschlüsseln**
  (`selftest::test_keys(tid)`) senden. Nur für lokale Tests: feste Schlüssel + neuer Lauf = Nonce-Wiederverwendung.

## Aufbau

| Datei | Inhalt |
|---|---|
| `src/app.rs` | Hauptthread: Nachrichtenschleife, IPC-Befehle, Session, Fenster |
| `src/ipc.rs` | Protokoll (Parsen, Antworten, Ereignisse), ohne GPU testbar |
| `src/window.rs` | Proxy-Fenster, Per-Monitor-DPI-Aware V2, Titel `<app> – <title>` |
| `src/receiver.rs` | Thread je `tid`: Decode, Present, Keyframe-Anforderung, Rückstau |
| `src/decode.rs` | FFmpeg D3D11VA auf dem eigenen D3D11-Gerät, kein stiller Software-Fallback |
| `src/gpu.rs` | D3D11-Gerät (leistungsstärkste GPU), Flip-Swapchain, V-Sync aus, Latenz 1, Video-Processor (NV12→BGRA, Letterbox) |
| `src/stats.rs` | Messfenster und `stats`-Ereignis |
| `src/firewall.rs` | prüft die Firewall-Regel für „Öffentlich“ (§7), ändert nichts, nur Hinweis im Log |
| `src/selftest.rs` | Testbild, Encoder, Sender |
| `examples/ipc_driver.rs` | treibt den Helfer wie Python und startet Selbsttest-Sender (Abnahme C03) |

Verhalten:
- Jeder `tid` hat ein eigenes Fenster und einen eigenen Thread mit eigenem D3D11-Gerät und Swapchain.
  Fenstergröße = `w`×`h` Punkte × DPI-Skalierung des Monitors unter dem Mauszeiger (begrenzt auf die
  Arbeitsfläche). Größenänderung durch den Nutzer skaliert nur (Seitenverhältnis bleibt, Rand schwarz).
- Decode-Fehler oder Lücke: letztes gutes Bild bleibt stehen, `bt_request_keyframe`, bis zum nächsten
  Keyframe wird nichts angezeigt. Stauen sich mehr als 6 Frames, springt der Empfänger zum neuesten Keyframe
  bzw. verwirft alles und fordert einen an.
- Schließen des Fensters: `proxy.closed`, Stream und Fenster werden abgebaut; ein folgendes `recv.close` ist ok.
- `transport.peer` wirkt auch bei laufenden Streams (`Session::set_peer`).
- Firewall: Beim `transport.open` prüft ein Hintergrund-Thread die Regeln. Fehlt eine Erlaubnis für UDP im
  Profil „Öffentlich“ (Direktkabel), steht ein Hinweis mit dem passenden `New-NetFirewallRule`-Befehl im Log.

## Abnahmetest ohne Mac

```powershell
cd teleport\win
cargo build --release --bins --examples
.\target\release\examples\ipc_driver.exe --streams 2 --secs 10            # 2 Fenster, 1080p60 H.264
.\target\release\examples\ipc_driver.exe --streams 1 --secs 8 --codec hevc
.\target\release\examples\ipc_driver.exe --streams 1 --secs 10 --loss 5   # Erholung bei Verlust
.\target\release\examples\ipc_driver.exe --streams 1 --keep               # Fenster ansehen, Enter beendet
```

Von Hand (zwei Terminals), wie in C03 beschrieben:

```powershell
cargo run --release -- --selftest-send 127.0.0.1 47001
# zweites Terminal: Empfänger starten und JSON-Zeilen eingeben (Testschlüssel für tid 1):
.\target\release\beamer-teleport.exe --ipc stdio --same-clock
{"v":1,"id":1,"cmd":"recv.open","tid":1,"title":"Test","app":"Selbsttest","w":960,"h":540,"scale":1,"codec":"h264","keys":{"s2r_key":"AAAAARERERERERERERERERERERERERERERERERERERE=","s2r_prefix":"AAAAAQ==","r2s_key":"AAAAASIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiI=","r2s_prefix":"AAAAAg=="}}
{"v":1,"id":2,"cmd":"transport.open","local_port":47001,"peer_host":"","peer_port":0}
```

Mit dem Mac-Sender (C02): `recv.open` mit den von Python abgeleiteten Schlüsseln, `transport.open` und
`transport.peer` auf die Adresse des Macs; die Firewall muss UDP auf dem gewählten Port durchlassen
(WLAN: Profil „Privat“, Kabel: siehe oben).
