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
.\scripts\fetch-ffmpeg.ps1          # FFmpeg 8.1 (gepinnt, SHA-256 geprueft), LGPL-shared, nach third_party\ffmpeg
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

`fetch-ffmpeg.ps1` lädt einen fest gepinnten BtbN-Build (`autobuild-2026-08-31-13-27`,
`ffmpeg-n8.1.2-50-g1a748fe2cd-win64-lgpl-shared-8.1.zip`) und bricht ab, wenn die SHA-256-Prüfsumme nicht
`e9712ffb…a3db63` ist. Gepinnt ist ein Monatsend-Build, weil BtbN Tages-Builds nach einigen Wochen löscht.
Ein Update heißt: Tag, Dateiname und Prüfsumme im Skript ändern, neu testen, DEPENDENCIES.md anpassen.

`build.rs` kopiert die FFmpeg-DLLs (`avcodec-62`, `avutil-60`, `avformat-62`, `avdevice-62`, `avfilter-11`,
`swresample-6`, `swscale-9`) neben das Binary. Zum Weitergeben gehören sie und `third_party\ffmpeg\LICENSE.txt`
neben `beamer-teleport.exe` (LGPL, siehe `docs/teleport/DEPENDENCIES.md`).

## Aufruf

```
beamer-teleport.exe --ipc stdio [--same-clock] [--test-loss PCT | --test-netem SPEC] [--test-present-all]
beamer-teleport.exe --selftest-send <host> <port> [--tid N] [--fps N] [--size BxH] [--codec h264|hevc]
                    [--kbps N] [--secs S] [--encoder NAME] [--local-port N] [--mac-test-keys]
```

- `--ipc stdio`: JSON Lines nach §1. Befehle: `hello`, `transport.open/peer/close`, `recv.open/update/close`,
  `shutdown`. Sender- und `proxy.*`-Befehle werden mit `unsupported` beantwortet. Ereignisse: `proxy.closed`
  (Fenster geschlossen), `stats` (jede Sekunde), `error`. stdin-EOF: Aufräumen und Exit 0 (gemessen < 0,1 s).
- `stats` enthält die Transportwerte (`fps`, `kbps`, `rtt_ms`, `loss_pct`, `peer`, …) und zusätzlich
  `decode_ms`, `present_ms`, `recv_to_present_ms` (+ `_p95_ms`, Empfang des letzten Fragments bis Rückkehr von
  `Present`), `recv_to_present_max_ms`, `recv_to_present_hist` (Histogramm, Klassengrenzen 1, 2, 5, 10, 16,7, 25,
  50, 100, 250 ms und offen; daraus lässt sich das p95 über einen ganzen Lauf bilden), `queue_ms` (dekodiert bis
  Present), `frames_presented`, `frames_superseded` (vom Jitter-Puffer durch ein neueres Bild ersetzt),
  `frames_skipped`, `catchups`, `decode_errors`, `decoder`. `latency_ms` = Haltezeit im Transport + Empfang→Present.
- Verwürfe im Transport je Ursache (Summe = `frames_dropped`): `dropped_incomplete` (Frame nach der Frist
  2×RTT, mind. 30 ms, noch unvollständig), `dropped_gap` (fertige Frames nach einer Lücke bis zum nächsten
  Keyframe), `dropped_overflow` (mehr als 64 offene Frames), `dropped_audio`. Dazu `gaps_incomplete` /
  `gaps_blocked` (Auslöser der Lücken: unvollständig abgelaufen bzw. Frame kam gar nicht) und `late_packets`
  (Pakete für schon verworfene Frames – zeigt Jitter statt Verlust).
- `--same-clock` (nur Selbsttest auf einem Rechner): `stats.same_clock_latency_ms` = Alter des Bildes seit
  `pts_us` des Senders (QueryPerformanceCounter, gilt nur, wenn Sender und Empfänger dieselbe Uhr haben).
- `--test-loss`: künstlicher Paketverlust im Transport, um NACK/FEC/Keyframe-Erholung zu prüfen.
- `--test-netem "loss=1,delay=10,jitter=8,stall=50,stall_every=1000,reorder=0,seed=7"`: künstliche Störung im
  Empfänger; `stall` bildet WLAN-Aussetzer nach (alle ~`stall_every` ms hält die Strecke `stall` ms lang alles
  fest und gibt es gesammelt frei).
- `--test-present-all`: Jitter-Puffer aus (jedes Bild einzeln, blockierend) – nur für Vergleichsmessungen.
- `--selftest-send`: Testbild (Zähler und Zeitstempel in ms als große Ziffern, wandernder Balken, Rauschfeld)
  mit AMF kodieren (dann Media Foundation, zuletzt OpenH264 mit Warnung) und mit **festen Testschlüsseln**
  (`selftest::test_keys(tid)`) senden. Nur für lokale Tests: feste Schlüssel + neuer Lauf = Nonce-Wiederverwendung.
  `--mac-test-keys` nimmt stattdessen die Testschlüssel des Mac-Senders (`send_demo.py`) und simuliert so den
  Mac für `ipc_driver --external`.

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
  Fenstergröße = `w`×`h` Punkte × DPI-Skalierung des Zielmonitors (begrenzt auf die Arbeitsfläche).
  Größenänderung durch den Nutzer skaliert nur (Seitenverhältnis bleibt, Rand schwarz).
- Platzierung: Monitor unter dem Mauszeiger, außer dort liegt ein Vollbild-Programm im Vordergrund (Spiel,
  Video) – dann ein anderer Monitor. Das Fenster erscheint oben, aber **ohne Aktivierung** (kein Fokusklau).
  Gibt es nur einen Monitor mit Vollbild-Programm, blinkt das Fenster in der Taskleiste.
- Anzeige mit Jitter-Puffer von höchstens einem Bild: Jedes Frame wird sofort dekodiert, gezeigt wird das
  neueste, sobald die Present-Queue frei ist. Nach einem WLAN-Aussetzer springt die Anzeige so auf das aktuelle
  Bild, statt jedes nachgelieferte Bild einen Bildschirm-Refresh lang zu zeigen.
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
.\target\release\examples\ipc_driver.exe --streams 1 --secs 20 --netem "delay=10,jitter=8,stall=50,stall_every=1000"
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

Der Treiber gibt neben OK/FEHL die Verwürfe je Ursache, das p95 über den ganzen Lauf (aus dem Histogramm) und
das Maximum aus; `--dump` zeigt die `stats` jeder Sekunde.

## E2E-Test Mac → PC (WLAN)

Mac-Sender aus C02 (`teleport/mac`, `tools/teleport/send_demo.py`) mit dessen festen **Testschlüsseln**
(`StreamKeys.testOnly`, nur für Tests). Der PC öffnet den Empfang mit leerem Peer; das Roaming lernt die
Adresse des Macs aus dem ersten authentifizierten Paket (§4.1), `stats.peer` zeigt sie danach an.

1. PC, Firewall: Beim ersten Start fragt Windows nach. Für das WLAN „Privat“ erlauben (keine Regeln von Hand
   nötig). Die IP des PCs im WLAN merken (`ipconfig`).
2. PC (60 s Messung ab dem ersten Bild, Abnahme 03a wird mit ausgewertet):
   ```powershell
   cd teleport\win
   .\target\release\examples\ipc_driver.exe --external --codec hevc --secs 60 --port 47001
   ```
   Der Treiber öffnet tid 1 mit den Mac-Testschlüsseln, `transport.open` auf UDP 47001 mit leerem Peer und
   wartet bis zu 5 min auf das erste Bild.
3. Mac: Testfenster bauen und starten, dann den Sender auf die PC-Adresse (wie in `tools/teleport/measure_c02.sh`;
   vorher `teleport/mac/build.sh`, Bildschirmaufnahme-Freigabe für das Terminal):
   ```bash
   swiftc -O tools/teleport/anim_window.swift -o /tmp/teleport_anim && (/tmp/teleport_anim &)
   tools/teleport/send_demo.py --helper teleport/mac/.build/release/BeamerTeleport \
       --app teleport_anim --peer <PC-IP>:47001 --codec hevc --seconds 75
   ```
4. Danach dasselbe mit `--codec h264` auf beiden Seiten.

Ausgabe: `E2E: fps … >= 57`, `verworfen … < 0,5 %` (mit Ursachen), `p95 Empfang->Present … < 25 ms`,
`keine decode_errors`. Von Hand geht es auch direkt mit dem Helfer:

```
.\target\release\beamer-teleport.exe --ipc stdio
{"v":1,"id":1,"cmd":"recv.open","tid":1,"title":"Mac","app":"E2E","w":1280,"h":800,"scale":2,"codec":"hevc","keys":{"s2r_key":"ERERERERERERERERERERERERERERERERERERERERERE=","s2r_prefix":"oaKjpA==","r2s_key":"IiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiI=","r2s_prefix":"sbKztA=="}}
{"v":1,"id":2,"cmd":"transport.open","local_port":47001,"peer_host":"","peer_port":0}
```

Im Betrieb (mit Python, E5) kommen statt der Testschlüssel die abgeleiteten Schlüssel aus `recv.open`, und
Python setzt `transport.peer` bei jedem Pfadwechsel (§7). Für das Direktkabel muss die Firewall UDP auch im
Profil „Öffentlich“ für `169.254.0.0/16` und `192.168.77.0/24` erlauben (Hinweis im Log, siehe oben).
