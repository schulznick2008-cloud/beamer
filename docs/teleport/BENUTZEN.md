# Teleport in Beamer benutzen (Nick)

Teleport verschiebt ein **Mac-Fenster** auf den gekoppelten **Windows-PC** (Proxy dort). Zurückholen mit dem gleichen Hotkey. Verbindung läuft über den **normalen Beamer-Link** (TCP 24820) plus UDP-Medienstrom; Schlüssel kommen vom gekoppelten Link (nicht aus Test-`vectors.json`).

## Voraussetzungen

- Beamer **1.5.2** auf Mac und PC, Branch `teleport` (oder Build mit Teleport-Integration).
- Helfer gebaut und erreichbar:
  - Mac: `teleport/mac/.build/release/BeamerTeleport`
  - PC: `teleport/win/target/release/beamer-teleport.exe`
- Kopplung wie gewohnt; in `hello`/`welcome` steht `"teleport":1` (automatisch ab E5-Integration).

## Start (Normalbetrieb)

### Mac

```bash
cd /pfad/zum/beamer-fork
# App wie gewohnt (Tray) – z. B. aus Xcode/Release oder:
python3 mac_app/kvm_bridge_app.py
```

Optional Helfer-Pfad setzen, wenn nicht Standard:

```bash
export BEAMER_TELEPORT_MAC_HELPER="/pfad/zu/BeamerTeleport"
```

### Windows-PC

```bash
cd C:\pfad\zum\beamer-fork
# Beamer-Desktop-App wie gewohnt starten (kvm_bridge_win)
```

Optional:

```cmd
set BEAMER_TELEPORT_WIN_HELPER=C:\pfad\zu\beamer-teleport.exe
```

Beide Seiten: **Beamer laufen lassen**, Link verbunden (grün / „Connected“).

## Fenster teleportieren und zurückholen

| Aktion | Mac |
|--------|-----|
| **Vorderfenster zum PC** | Hotkey **Ctrl+Alt+Cmd+T** (Vorschlag, nicht im Vertrag dokumentiert) |
| **Zurückholen** | Gleicher Hotkey erneut, oder Proxy auf dem PC schließen |

Der PC zeigt ein Proxy-Fenster. Liegt der Fokus dort, gehen Tastatur und Maus über den Windows-Proxy zum Mac (Eingabe im teleportierten Fenster). Sonst bleibt Beamer-KVM wie bisher.

## Nur testen ohne volle App (M1-Demo)

```bash
# PC (Empfänger), Fake-Helfer in CI:
PYTHONPATH=. BEAMER_TELEPORT_FAKE_HELPER=1 python3 tools/teleport/m1_demo.py --role pc --selftest-fake --seconds 5

# Echter Helfer: siehe docs/teleport/M1-DEMO.md
```

## Einstellungen (geplant / teilweise)

- `teleport.path`: `auto` | `wlan` | `cable` (§7, Pfadwahl)
- Menüeinträge in der Beamer-UI: **noch nicht** in 1.5.2 – bis dahin nur Hotkey

## Bekannte Grenzen (dieser Stand)

- **Make-before-break** und volle Kabel-Umschaltung (§7) sind in Python angelegt (`core/teleport_path.py`), Link-Umschalten auf Gerät noch mit Nick/C01b zu verifizieren.
- **Menü/Hotkey-Einstellung** in der UI fehlt; Hotkey ist fest `ctrl+alt+cmd+t` im Code (`mac_app/bridge.py`).
- Volle **native** Proxy-Eingabe auf Windows (C05) kann auf dem Gerät noch fehlen; ohne Fokus im Proxy bleibt KVM normal.

## Hilfe / Logs

- Helfer-Logs auf **stderr** (nicht stdout).
- Bei `helper_restarted`: Teleport endet, Schlüssel neu – erneut teleportieren.
