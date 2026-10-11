# M1-Demo (Mac → PC)

Kurztest: ein Mac-Fenster per UDP an den Windows-Empfänger-Helfer streamen. Steuerung über
`tools/teleport/m1_demo.py` und die Helfer-IPC (§1). Schlüssel kommen aus `core/teleport_keys`
(frisches `salt16` je Lauf, kein `StreamKeys.testOnly`).

**Reihenfolge:** zuerst **PC (Empfänger)**, dann **Mac (Sender)**. Beide teilen sich eine
Session-Datei (`m1_demo_session.json`).

## Voraussetzungen

- Mac: `teleport/mac` gebaut → `teleport/mac/.build/release/BeamerTeleport`
- PC: `teleport/win` gebaut → `teleport/win/target/release/beamer-teleport.exe`
- Python 3.12+, Repo-Root als Arbeitsverzeichnis
- Bildschirmaufnahme auf dem Mac erlaubt

## Session-Datei

Die Datei enthält u. a. `master_hex` (32 Byte, Default aus `docs/teleport/vectors.json` für
Labore; in Produktion später aus dem Beamer-Link), `salt16_hex`, `pc_udp_port`, `mac_udp_port`,
`tid`. Nach `helper_restarted` erzeugt das Skript ein neues `salt16` und öffnet Transport/Stream
neu.

## PC (PowerShell)

Im Repo-Root, Session-Datei z. B. auf dem Desktop:

```powershell
cd C:\Pfad\zum\beamer
$session = "$env:USERPROFILE\Desktop\m1_demo_session.json"
python tools/teleport/m1_demo.py --role pc --peer 192.168.1.50:0 --session $session --codec hevc --seconds 30
```

`192.168.1.50` = IP des **Macs** (Gegenstelle für `transport.peer`). Port `0` bis der Mac
gestartet ist; danach nutzt der PC `mac_udp_port` aus der Session, sobald vorhanden.

## Mac (Terminal)

Gleiche Session-Datei (per SMB/USB kopieren oder gemeinsamer Pfad):

```bash
cd /Pfad/zum/beamer
python3 tools/teleport/m1_demo.py --role mac --peer 192.168.1.2:0 --session ~/Desktop/m1_demo_session.json --codec hevc --seconds 30 --app TextEdit
```

`192.168.1.2` = IP des **PCs**. Port `0`: das Skript wartet auf `pc_udp_port` in der Session.

Optional eigenes Master (hex, 64 Zeichen):

```bash
python3 tools/teleport/m1_demo.py --role mac --master-hex <64-hex-zeichen> ...
```

## Ausgabe

Am Ende erscheinen eine Textzeile pro Seite (fps, Verlust, RTT, Bildalter) und darunter ein
JSON-Objekt mit `pc` / `mac` und Mittelwerten.

## CI / ohne Hardware

```bash
PYTHONPATH=. python3 tools/teleport/m1_demo.py --selftest-fake --seconds 2
```

Zwei Fake-Helfer auf `127.0.0.1` (nur Stdlib + bestehende Teleport-Python-Module).
