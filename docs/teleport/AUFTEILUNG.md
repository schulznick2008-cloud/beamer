# Beamer-Teleport – Aufteilung der Arbeit

Ziel: einzelne Fenster in beide Richtungen zwischen Mac (M5) und Windows-PC (RX 6600) teleportieren, plus Vollbild-Stream. Verbindung über WLAN, optional über ein Direktkabel Mac↔PC (`SCHNITTSTELLEN.md` §7).
Verträge: `SCHNITTSTELLEN.md`, im Fork unter `docs/teleport/`. Gearbeitet wird erst nach Nicks Okay zum Fork.

## Rollen
- **Claude Code (C)**, lokal auf Mac bzw. PC: native, hardwarenahe und schwierige Teile. Prompts liegen in `claude-code/`.
- **Cursor/Entwickler (E)**: Fork und Struktur, M0, die Python-Seite in Beamer, Idee 4, CI, Doku, Reviews jedes Claude-PRs.
- **Nick (N)**: Freigaben, M0-Messung, Tests am Gerät, Merges.

## Pakete
| # | Wer | Rechner | Inhalt | Meilenstein | hängt ab von |
|---|---|---|---|---|---|
| E0 | E | – | M0-Anleitung und Skripte (Sunshine/Moonlight plus virtueller Monitor, Messprotokoll) | M0 | – |
| E1 | E | – | Fork, Branch `teleport`, Repo-Struktur, `docs/teleport/*`, CI-Gerüst (macOS, Windows, Python), DEPENDENCIES.md | Setup | Nicks Fork-Okay |
| E2 | E | – | `core/teleport_keys.py` und `vectors.json` | M1 | E1 |
| E3 | E | – | `core/teleport_ipc.py`: Helfer starten, JSON-Lines, Fake-Helfer für Tests | M1 | E1 |
| C01 | C | Mac | `beamer-transport` (Rust): UDP, AEAD, Fragmente, NACK, FEC, Bitrate, C-ABI | M1 | E1 |
| C01b | C | Mac | Transport für Pfadwechsel nach §7: Roaming nach gültigem, authentifiziertem Paket, `bt_set_peer` bei laufenden Streams, `bt_set_max_kbps`, Socket an `0.0.0.0`, `peer` in Stats, Tests mit Adresswechsel. Kann an die C01-Nacharbeit angehängt werden. | M1 | C01 |
| C02 | C | Mac | Mac-Sender: ScreenCaptureKit (Fenster/Display), VideoToolbox, Senden | M1 | C01 |
| C03 | C | Win | Win-Empfänger: Dekodieren (D3D11VA), Anzeigefenster, einfacher Proxy | M1 | C01 |
| E4 | E | – | `tools/teleport/m1_demo.py`: verbindet beide Helfer mit statischen Schlüsseln | M1 | C02, C03, E3 |
| E5 | E | – | Beamer-Protokoll: `tp_*`-Nachrichten und `teleport`-Fähigkeit, Routing, Eingabe nach Fensterfokus, Hotkeys „Fenster teleportieren“ und „zurückholen“. Dazu Pfadwahl/Kabel nach §7 (`core/teleport_path.py`, Make-before-break des Links, `transport.peer`/`send.bitrate` an die Helfer) | M2 | E2, E3, C01b |
| C04 | C | Mac | Mac-Sender ausbauen: Eingabe einspielen, Fenster aktivieren, Größe, CGVirtualDisplay pro App | M2/M3 | C02 |
| C05 | C | Win | Win-Proxy nativ: Eingabe abgreifen → `proxy.input`, Fokus, Größe, Titel/Icon/Taskleiste, DPI, Vollbild | M2/M3 | C03 |
| C06 | C | Win | Win-Sender: WGC, AMF (FFmpeg), virtueller Monitor (VDD, MIT), SendInput, Fenstersteuerung | M4 | C01, C05 |
| C07 | C | Mac | Mac-Empfänger: VideoToolbox-Decode, natives NSWindow als Proxy, Eingabe, DPI, Vollbild | M4 | C01, C04 |
| E6 | E | – | Vollbild-Modus in Python/UI (Bildschirm wählen, `mode:"display"`), Integrationstests | M5 | C02, C05, C06, C07 |
| E7 | E | – | Idee 4: Hotkey „Beam“ (URL, Text, Datei) über den Beamer-Link | Nebenschritt | E5 |
| E8 | E | – | Kanten-Geste (M6), Python-Seite: Fenster-Drag an der Kante → Teleport | M6 | E5, C04, C06 |
| C08 | C | Mac | Idee 5: Audio pro App (ScreenCaptureKit-Audio, Opus) senden und empfangen | Nebenschritt | C02, C07 |
| C09 | C | Win | Idee 5: Audio pro Prozess (Process-Loopback, Opus) senden und empfangen | Nebenschritt | C03, C06 |
| C10 | C | Mac + SSH zum Pi 4 | Idee 2: Notanker mit dem Pi 4 als USB-HID-Gadget | Nebenschritt | E1 |
| E9 | E | – | Reviews (jeder C-PR), Doku, Härtung (M7), Releases | laufend | – |

## Reihenfolge
1. **E0**, Nick misst M0 (1–2 Tage). Abbruchkriterium: über 80 ms oder unlesbare Schrift → Plan überdenken.
2. **E1**, nach dem Fork-Okay, danach **C01**. Parallel laufen **E2** und **E3**.
3. **C02** (Mac) und **C03** (Windows) parallel, dann **E4**. **M1 fertig:** Mac-Fenster sichtbar auf dem PC.
4. **E5**, **C04** und **C05**. **M2/M3 fertig:** Mac-Fenster auf dem PC bedienbar, fühlt sich nativ an.
5. **C06** und **C07** parallel. **M4 fertig:** Rückrichtung.
6. **E6** (M5 Vollbild), dann **E8** (M6 Geste), dann Härtung.
- Jederzeit möglich: **E7** (ab E5), **C10** (unabhängig), **C08/C09** (ab M4).

## Regeln
- Jedes Paket ist ein Branch, ein PR nach `teleport`, mit Review durch E und Test durch Nick am Gerät.
- Ändert sich ein Vertrag, gibt es zuerst einen PR auf `SCHNITTSTELLEN.md`.
- Am Ende schreibt jedes Paket einen kurzen `REPORT`-Abschnitt in die PR-Beschreibung (Vorlage in jedem Prompt).
- Nicht force-pushen, keine Secrets ins Repo, nur GPL-3-kompatible Lizenzen.
- Kabel (§7) in den Helfern: `transport.peer` bei laufenden Streams und `send.bitrate` setzen C02/C03 (bzw. C04/C05) und C06/C07 als kleine Pflichtpunkte mit um. Die Logik liegt in C01b und E5.
