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
| `Sources/BeamerTeleport` | `main.swift`: Argumente, IPC-Schleife |
| `Tests/TeleportCoreTests` | 37 Tests (Hardware-Encoder-Tests werden ohne Hardware übersprungen) |

## IPC (Helfer-Seite)

Befehle: `hello`, `transport.open`, `transport.peer` (auch bei laufendem Strom), `transport.close`,
`windows.list`, `send.start`, `send.stop`, `send.keyframe`, `send.bitrate` (→ `bt_set_max_kbps` und
Encoder-Obergrenze), `shutdown`. Alles andere (`recv.*`, `inject`, `send.resize`, `send.activate`) antwortet mit
`unsupported`. `send.start` mit `virtual_display:true` ebenfalls (kommt in einem späteren Paket); `restore` bei
`send.stop` hat auf dem Mac keine Wirkung.

Ereignisse: `send.gone` (`reason`: `window_closed` oder `capture_error: …`), `send.size` (Größe oder Skalierung
geändert; der Encoder startet mit einem Keyframe neu), `stats` jede Sekunde, `error` (nur für Zeilen, die sich
keiner Anfrage zuordnen lassen). Nach `send.gone` ist ein `send.stop` zu derselben `tid` kein Fehler.

`stats` enthält die Felder aus §1.2 (`fps`, `kbps`, `rtt_ms`, `loss_pct`, `latency_ms`) und zusätzlich `state`
(`ok`, `minimized_or_other_space`), `target_kbps`, `capture_fps`, `encode_fps`, `encode_ms`, `refused`, `cpu_pct`
(Prozent eines Kerns), `pace_queue_ms`. `latency_ms` ist beim Sender 0.

Pipeline: Capture (Retina-Pixel, 420v, kein Cursor, `max_fps`) → VideoToolbox (Low-Latency-Ratenregelung, keine
B-Frames, Keyframe alle 2 s und auf `keyframe_req`/`send.keyframe`) → Annex-B mit Parametersätzen vor jedem
Keyframe → `bt_send_frame`. Jede Sekunde wird die Encoder-Bitrate auf `min(bt_target_kbps, Obergrenze)` gesetzt. Lehnt der
Transport ein Frame ab („Sendewarteschlange voll“), werden die Folgebilder bis zum nächsten Keyframe nicht gesendet
und sofort ein Keyframe angefordert. Ändert sich nichts im Bild, geht spätestens alle 2 s trotzdem ein Keyframe mit dem
letzten Bild raus. Ist das Fenster minimiert oder auf einem anderen Space, bleibt das letzte Bild stehen und `state`
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
