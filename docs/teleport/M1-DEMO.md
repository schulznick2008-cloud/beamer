# M1-Demo (Mac → PC)

Kurztest: ein Mac-Fenster per UDP an den Windows-Empfänger-Helfer streamen. Steuerung über
`tools/teleport/m1_demo.py` und die Helfer-IPC (§1). Schlüssel kommen aus `core/teleport_keys`
(frisches `salt16` je Lauf, kein `StreamKeys.testOnly`).

**Reihenfolge:** zuerst **PC (Empfänger)**, dann **Mac (Sender)**. Der PC öffnet UDP **47001**
(feste Portnummer, passend zur Firewall-Regel) und erzeugt pro Lauf ein frisches `salt16`. Er gibt
die fertige Mac-Befehlszeile mit `--salt-hex` aus; die Session-Datei muss **nicht** zwischen den Geräten
geteilt werden.

> **Achtung, nur Labor:** Ohne `--master-hex` nimmt das Skript den Master aus
> `docs/teleport/vectors.json`. Diese Testvektoren sind öffentlich; das ist für M1 im eigenen Netz
> in Ordnung, aber **nie in Produktion**. Ab E5 kommt der Master aus dem Beamer-Link (§4.2).

## Voraussetzungen

- Mac: `teleport/mac/build.sh` → `teleport/mac/.build/release/BeamerTeleport`; Bildschirmaufnahme für
  Terminal erlaubt.
- PC: `teleport\win` gebaut (`scripts\fetch-ffmpeg.ps1`, `cargo build --release`) →
  `teleport\win\target\release\beamer-teleport.exe`; Firewall lässt UDP 47001 bzw. das Programm durch.
- Python 3.10+ auf beiden Seiten, Repo-Root als Arbeitsverzeichnis.

## Peer und Rückkanal

Der PC öffnet den Transport mit Peer-Port 0. Der Empfänger lernt die Adresse des Macs aus dem ersten
authentifizierten Paket (Roaming, §4.1/§7); NACK, Feedback und `keyframe_req` (r2s) gehen danach dorthin.
Ein `transport.peer` nach `mac_udp_port` ist deshalb nicht nötig.

## PC (PowerShell, im Repo-Root)

```powershell
python tools/teleport/m1_demo.py --role pc --peer <MAC-IP>:0 --codec h264 --seconds 120
```

Die Ausgabe enthält eine Zeile `python3 tools/teleport/m1_demo.py --role mac --peer <PC-IP>:47001 --salt-hex …`.

## Mac (Terminal, im Repo-Root)

Die Zeile vom PC übernehmen, `<PC-IP>`, `<s>` und `<App>` ersetzen, z. B.:

```bash
python3 tools/teleport/m1_demo.py --role mac --peer 192.168.178.176:47001 --salt-hex <vom PC> --tid 1 --codec h264 --seconds 60 --app TextEdit
```

Pro Lauf neu starten (neues `salt16`). Startet ein Helfer nach einem Absturz neu (`helper_restarted`),
erzeugt diese Seite ein neues `salt16`; die Gegenseite kennt es nicht, der Stream bricht ab. Dann beide
Seiten neu starten (in M1 so gewollt; ab E5 handeln die Seiten das über `tp_*` aus).

Die Zusammenfassung zeigt pro Lauf nur die lokale Rolle (PC: Empfangswerte, Mac: Senderwerte).

## Ausgabe

Am Ende erscheinen eine Textzeile pro Seite (fps, Verlust, RTT, Bildalter) und darunter ein
JSON-Objekt mit `pc` / `mac` und Mittelwerten.

## CI / ohne Hardware

```bash
PYTHONPATH=. python3 tools/teleport/m1_demo.py --selftest-fake --seconds 2
```

Zwei Fake-Helfer auf `127.0.0.1` (nur Stdlib + bestehende Teleport-Python-Module).
