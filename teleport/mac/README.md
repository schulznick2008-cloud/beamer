# BeamerTeleport (Mac-Sender)

SwiftPM-Paket für macOS 14+ (arm64). Executable `BeamerTeleport` streamt ein einzelnes Fenster oder einen
Bildschirm per ScreenCaptureKit und VideoToolbox (H.264/HEVC, Hardware-Encoder Pflicht) über
`beamer-transport` (`../transport`). Gesteuert per JSON Lines über stdio, siehe `docs/teleport/SCHNITTSTELLEN.md`
(§1 IPC, §4 Strom, §5 C-ABI, §7 Pfadwechsel).

```
cd teleport/mac
./build.sh            # baut ../transport (cargo, release) und das Swift-Paket, ohne Warnungen
swift test            # IPC-Parser, Befehls-Verteiler, Annex-B, Transport, Engine, Prozess
.build/release/BeamerTeleport --ipc stdio
```

Voraussetzungen: Xcode/Swift 5.10+, Rust (`cargo`), Freigabe **Bildschirmaufnahme** für das Programm, das den
Helfer startet (Systemeinstellungen > Datenschutz & Sicherheit). Ohne Freigabe antworten `windows.list` und
`send.start` mit `error.code = "permission"`; der Helfer läuft weiter.

## Aufbau

| Ziel | Inhalt |
|---|---|
| `Sources/CBeamerTransport` | C-Target; `include/beamer_transport.h` ist ein Symlink auf `../transport/include` |
| `Sources/TeleportCore` | `IPC` (JSON, Zeilen), `Dispatcher` (Befehle), `SenderEngine`/`SenderPipeline` (Strom), `Capture` (ScreenCaptureKit), `VideoEncoder` (VideoToolbox), `AnnexB`, `Transport` (Swift-Hülle der C-ABI), Selbsttests |
| `Sources/CVirtualDisplay` | ObjC-Target fuer den virtuellen Monitor (privates CGVirtualDisplay-API, eigene Deklarationen) |
| `Sources/BeamerTeleport` | `main.swift`: Argumente, IPC-Schleife |
| `Tests/TeleportCoreTests` | 99 Tests (Hardware-Encoder-Tests werden ohne Hardware übersprungen; die Pipeline-Tests laufen mit einer Attrappe als langsamem Encoder und brauchen keine) |

## IPC (Helfer-Seite)

Befehle: `hello`, `transport.open`, `transport.peer` (auch bei laufendem Strom), `transport.close`,
`windows.list`, `send.start`, `send.stop`, `send.keyframe`, `send.bitrate` (→ `bt_set_max_kbps` und
Encoder-Obergrenze), `shutdown`. Alles andere (`recv.*`, `inject`, `send.resize`, `send.activate`) antwortet mit
`unsupported`. `send.start` mit `virtual_display:true` ebenfalls (kommt in einem späteren Paket); `restore` bei
`send.stop` hat auf dem Mac keine Wirkung.

Ereignisse: `send.gone` (`reason`: `window_closed` oder `capture_error: …`), `send.size` (Größe oder Skalierung
geändert; der Encoder startet mit einem Keyframe neu), `stats` jede Sekunde, `error` (nur für Zeilen, die sich
keiner Anfrage zuordnen lassen). Nach `send.gone` ist ein `send.stop` zu derselben `tid` kein Fehler.

`hello` meldet zusätzlich `low_power` (Energiesparmodus an). Die Codec-Wahl (HEVC → H.264) trifft Python; der Helfer meldet nur.

`stats` enthält die Felder aus §1.2 (`fps`, `kbps`, `rtt_ms`, `loss_pct`, `latency_ms`) und zusätzlich `state`
(`ok`, `minimized_or_other_space`), `target_kbps`, `capture_fps`, `encode_fps`, `encode_ms`, `refused`, `cpu_pct`
(Prozent eines Kerns), `pace_queue_ms`, `backlog_drops` (wegen Encoder-Rückstau verworfene Capture-Frames), `encoder_slow`, `effective_fps`, `low_power`, `encoder_resets`. `latency_ms` ist beim Sender 0.

Pipeline: Capture (Retina-Pixel, 420v, kein Cursor, `max_fps`) → VideoToolbox (Low-Latency-Ratenregelung, keine
B-Frames, Keyframe alle 2 s und auf `keyframe_req`/`send.keyframe`) → Annex-B mit Parametersätzen vor jedem
Keyframe → `bt_send_frame`. Jede Sekunde wird die Encoder-Bitrate auf `min(bt_target_kbps, Obergrenze)` gesetzt. Lehnt der
Transport ein Frame ab („Sendewarteschlange voll“), werden die Folgebilder bis zum nächsten Keyframe nicht gesendet
und sofort ein Keyframe angefordert. Ändert sich nichts im Bild, geht spätestens alle 2 s trotzdem ein Keyframe mit dem
letzten Bild raus. Ein Keyframe-Wunsch (`keyframe_req`, `send.keyframe`, abgelehntes Frame) kodiert das letzte Bild sofort, auch bei stehender Quelle.

**Encoder-Rückstau (C02a):** Höchstens 2 Frames stecken gleichzeitig im Encoder; weitere Capture-Frames ersetzen das eine wartende (das neueste zählt). Liegt die Encode-Zeit 2 s lang über einer Bildzeit (`1000/max_fps`), meldet `stats` `encoder_slow:true`; gab es dabei Rückstau, sinkt die effektive Bildrate auf den Durchsatz (nicht unter 20) und geht nach 3 s ohne Engpass schrittweise zurück. Verschluckt VideoToolbox Frames (0,6 s lang kein Fortschritt), legt ein Wachhund den Encoder neu an. `send.stop` und das Beenden bei EOF warten höchstens 0,5 s auf den Encoder. Ist das Fenster minimiert oder auf einem anderen Space, bleibt das letzte Bild stehen und `state`
zeigt es an.

## Selbsttests und Messung

```
.build/release/BeamerTeleport --selftest-recv 47000 [--seconds N]      # Empfänger ohne Anzeige
tools/teleport/send_demo.py --list                                     # Fenster auflisten
tools/teleport/send_demo.py --app TextEdit --peer 127.0.0.1:47000      # Sender per IPC
.build/release/BeamerTeleport --selftest-synth 127.0.0.1:47000         # Sender mit Muster-Quelle (ohne Freigabe)
tools/teleport/measure_c02.sh hevc 30                                  # komplette Messung (Test-Fenster 1512x982 pt)
```

`--selftest-recv` dekodiert mit VideoToolbox und gibt je Sekunde fps, Latenz (Capture bis dekodiert, gleiche
Host-Uhr, also nur auf demselben Mac sinnvoll), Verlust und CPU aus, am Ende eine JSON-Zusammenfassung (Median/p95 ohne
die ersten 3 s Anlauf, `latency_all_*` mit). Die Test-Schlüssel (`StreamKeys.testOnly`) sind feste Dummy-Werte nur für
diese Selbsttests; echte Teleports bekommen ihre Schlüssel von Python (§4.2).

`BEAMER_TELEPORT_FAKE_NO_PERMISSION=1` lässt den Helfer so tun, als fehle die Freigabe (Test der Fehlerpfade),
`BEAMER_TELEPORT_DEBUG=1` loggt Capture-Statuswechsel nach stderr.

`windows.list` liefert nur sichtbare oder minimierte Fenster (Layer 0, mindestens 100×50 pt, mit Titel) normaler Apps; Hilfsprozesse (z. B. WindowManager, „Open and Save Panel Service“) und unsichtbare Fenster ohne Titel fallen heraus (`WindowFilter`).

## Haengende ScreenCaptureKit-Aufrufe (C02b)

ScreenCaptureKit-Aufrufe kehren unter Last manchmal nie zurueck (besonders, wenn mehrere Helfer gleichzeitig starten).
Der Helfer haelt dafuer folgende Regeln ein:

- **Jeder blockierende SCK-Aufruf hat ein Zeitlimit** (`runBlocking(timeout:)`, kein Datenwettlauf auf dem Ergebnis):
  `SCShareableContent` 2,5 s (zwei Versuche), `startCapture` 3,5 s (zwei Versuche mit frischem Stream), `stopCapture` und
  `updateConfiguration` 1 s. Bei einem Timeout antwortet der Befehl mit `busy` (klare Meldung); `stop` raeumt trotzdem
  auf und verwirft den Stream. Meldet SCK kurz `userDeclined`, obwohl `CGPreflightScreenCaptureAccess()` die Freigabe
  bestaetigt, gilt das als `busy`, nicht als `permission`.
- **Prozessuebergreifende Sperre** (`flock` auf `$TMPDIR/beamer-teleport-sck.lock`) nur um die kurzen Aufrufe, nie um
  einen Strom: gleichzeitig gestartete Helfer fragen SCK nacheinander. Wer die Sperre nicht in 4 s bekommt, laeuft ohne.
- **Der IPC-Thread blockiert nie:** Befehle laufen auf einer Queue, gelesen wird auf eigenem Thread. `send.stop` antwortet
  nach hoechstens 1,5 s, auch wenn die Quelle haengt; `shutdown`/EOF raeumt alle Strome gleichzeitig auf, und ein Wachhund
  beendet den Prozess nach 1,8 s mit `exit(0)`, falls das Aufraeumen nicht fertig wird. `send.start` wartet hoechstens 12 s.
- **Kein SCK-Aufruf auf der Pipeline-Queue:** Zustand, Fenstergroesse und Bildrate werden auf einer eigenen Queue
  abgefragt; die Pipeline nutzt beim naechsten Takt das letzte Ergebnis.
- Test-Haken: `BEAMER_TELEPORT_TEST_SOURCE=hang-start|hang-stop|hang-refresh` ersetzt die Quelle durch eine haengende.
- Lasttest: `tools/teleport/measure_c02_load.sh hevc 30 3` (Messung plus zwei Helfer mit `--display 0`).

## Eingabe, Fenstersteuerung, virtueller Monitor (C04)

Neue Befehle: `inject` (§3), `send.activate`, `send.resize` (Antwort `width`/`height` in Stream-Pixeln, danach `send.size`);
`hello` meldet jetzt `inject` in `caps`; `send.start` mit `virtual_display:true` ist umgesetzt (nur fuer `mode:"window"`).
Ohne Bedienungshilfen-Freigabe antworten `inject`, `send.activate`, `send.resize` und `send.start` mit
`virtual_display` mit `error.code = "permission"` (Test: `BEAMER_TELEPORT_FAKE_NO_ACCESSIBILITY=1`).

- **Eingabe:** Stream-Pixel werden in globale Bildschirmpunkte umgerechnet (`InputGeometry`: Fensterrahmen oder
  Monitorflaeche geteilt durch die Stream-Groesse, also bei Retina 2x ein Stream-Pixel = 0,5 pt). Eingaben ausserhalb des
  Bildes werden verworfen; ein `up` ausserhalb laesst den Knopf trotzdem los. Maus (Bewegung, Drag bei gehaltenem Knopf,
  Klickzahl aus `n`, Rechts-/Mitteltaste, Zusatztasten), Scrollen in Zeilen oder Pixeln (Bruchteile werden aufgespart;
  der Zeiger wird vorher an die Stelle gesetzt; die Vorzeichen gibt der Helfer unveraendert an CoreGraphics weiter wie
  Beamers Python-Injektor), Tasten ueber Beamers Tastennamen (Port von `plan_key_event`) und `text` als Unicode-Ereignis
  (Stuecke zu 16 UTF-16-Einheiten, ohne Flags, damit ein gehaltener Modifier den Text nicht zum Kuerzel macht).
  Die Tabellen kommen aus `mac_app/key_codes.py` und `mac_app/input_injector_mac.py`: `tools/teleport/gen_swift_keycodes.py`
  erzeugt `KeyTables.swift`, und ein Test vergleicht sie mit dem Python-Quelltext. Zeichentasten folgen der aktuellen
  Tastaturbelegung (aus Tastenereignissen gelesen, alle 10 s erneuert), Zeichen ohne Taste gehen als Text raus.
- **Aktivierung:** vor `down`, `key` und `text` (hoechstens alle 0,5 s geprueft) und bei `send.activate`: AXFrontmost an
  der App, `NSRunningApplication.activate()`, `AXRaise` auf genau dieses Fenster (gefunden ueber `_AXUIElementGetWindow`,
  Rueckfall ueber den Rahmen).
- **`send.resize`:** Groesse per `kAXSizeAttribute` setzen, Capture sofort neu lesen und `send.size` melden (Ziel unter
  300 ms). Aendert die App ihre Groesse selbst, bemerkt das ein billiger Check alle 250 ms (CoreGraphics-Rahmen) und
  stoesst dasselbe an.
- **Virtueller Monitor:** Das Fenster wird per AX auf einen neuen virtuellen Monitor in Fenstergroesse mit HiDPI
  geschoben (unter eine Platzreserve fuer die Menueleiste von 40 pt), der **ganze Monitor** wird aufgenommen (Menues,
  Popover, Sheets kommen mit; `send.start` meldet dann die Monitorgroesse). Eingaben gelten fuer die Monitorflaeche.
  `send.stop` (auch `restore:false`, sonst bliebe das Fenster auf einem verschwindenden Monitor) schiebt das Fenster an die
  alte Stelle zurueck und entfernt den Monitor; ebenso EOF, `shutdown` und SIGTERM/SIGINT/SIGHUP. Bei einem Absturz
  (SIGSEGV usw.) versucht ein Handler, die Fenster zurueckzuschieben; der Monitor verschwindet mit dem Prozess.
- **Schlaf:** Solange ein Teleport laeuft, haelt eine `IOPMAssertion` (`PreventUserIdleSystemSleep`, Name „Beamer
  Teleport aktiv“) den Mac wach (`pmset -g assertions`). **Zugeklappter Deckel:** diese Assertion verhindert nur den
  Leerlauf-Schlaf. Klappt man den Deckel zu, schlaeft ein MacBook trotzdem, ausser im Clamshell-Betrieb (Netzteil und
  externer Monitor angeschlossen). Dann stoppt der Strom, die Verbindung bricht ab und die Teleports enden.

Abnahme: `tools/teleport/mac_inject_demo.py [--virtual] [--dump-dir DIR]` (startet das Pruef-Fenster
`tools/teleport/input_probe.swift` mit Textfeld, Dropdown und Kontextmenue, den Selbsttest-Empfaenger, den Helfer und
prueft Position, Klickzahl, Scrollen, Tippen und `send.resize` am Protokoll des Pruef-Fensters). Braucht die
Bedienungshilfen- und die Bildschirmaufnahme-Freigabe fuer das startende Programm.
