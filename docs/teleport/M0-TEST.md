# M0 – WLAN-Machbarkeitstest (Mac-Bild auf den PC)

**Frage:** Ist ein Mac-Bild, das per WLAN auf den Windows-PC gestreamt wird, schnell und scharf genug für den Fenster-Teleport?
Getestet wird mit fertigen Werkzeugen (**Sunshine** auf dem Mac, **Moonlight** auf dem PC, beide GPL-3.0). Eigener Code ist noch nicht nötig.
Dauer: etwa 60–90 Minuten. Alles lässt sich danach wieder entfernen.

**Was gebraucht wird:** MacBook (am Netzteil, Deckel offen) und Windows-PC, beide im selben WLAN bzw. Heimnetz (Fritzbox).
Dazu Nicks iPhone für Fotos sowie die Dateien aus `tools/teleport/m0/` auf beiden Rechnern.

**Ampel (Entscheidung am Ende)**
- 🟢 weiter mit C01: Glass-to-Glass-Median ≤ 60 ms, p95 ≤ 80 ms, 11-px-Text bei ≤ 40 Mbit/s lesbar, ≤ 1 sichtbarer Ruckler pro Minute.
- 🟡 weiter mit Auflagen (PC per LAN-Kabel, AWDL aus, höhere Bitrate): Median 60–80 ms oder Text nur bei 80 Mbit/s lesbar.
- 🔴 Plan überdenken: Median > 80 ms, Text unlesbar oder ständige Aussetzer.

---

## Teil A – Vorbereitung (einmalig, ca. 20 min)

**A1. IP-Adressen notieren**
- Mac: Systemeinstellungen → WLAN → (i) neben dem Netz → „IP-Adresse“.
- PC: `ipconfig` in der Eingabeaufforderung, „IPv4-Adresse“. Notieren, ob der PC über **WLAN oder LAN-Kabel** verbunden ist.

**A2. Dateien holen** (Ordner `tools/teleport/m0/` aus dem Fork, Branch `teleport`, z. B. als ZIP von GitHub, oder vom Entwickler kopiert)
- Mac: Ordner nach `~/m0`, dann im Terminal `cd ~/m0 && chmod +x *.sh`.
- PC: Ordner nach `C:\m0`.

**A3. Mac: Sunshine installieren**
- `Sunshine-macOS-arm64.dmg` von https://github.com/LizardByte/Sunshine/releases/latest laden und installieren. Die macOS-Version ist laut Projekt **experimentell**.
- Beim ersten Start **Bildschirmaufnahme** (und Mikrofon) erlauben, danach Sunshine neu starten.
- Im Browser `https://localhost:47990` öffnen, die Zertifikatswarnung bestätigen und Benutzer und Passwort anlegen (starkes Passwort, das Web-UI ist im Heimnetz erreichbar).

**A4. PC: Moonlight installieren und koppeln**
- Installer von https://github.com/moonlight-stream/moonlight-qt/releases/latest.
- Moonlight öffnen. Der Mac erscheint automatisch, sonst „+“ und die Mac-IP eingeben. Auf den Mac klicken, Moonlight zeigt eine **PIN**.
- Auf dem Mac im Sunshine-Web-UI unter **PIN** die PIN eintragen und senden.

**A5. Moonlight einstellen** (Zahnrad)
- Auflösung = Auflösung des PC-Monitors, **60 FPS**, Bitrate **40 Mbit/s**.
- Anzeigemodus **Fenster**. Videocodec **HEVC**. **V-Sync aus**, **Bildtempo (Frame Pacing) aus**.
- Falls angeboten: „YUV 4:4:4“ zunächst **aus** (wird in B4 getestet).

**A6. Mac wach halten:** Im Terminal `caffeinate -dimsu` starten und laufen lassen, bis der Test vorbei ist (Strg+C beendet es).

---

## Teil B – Messungen

**B1. Netz ohne Stream (2 min)**
- Mac: `cd ~/m0 && ./mac_netz.sh <PC-IP> 60`
- PC (PowerShell): `cd C:\m0; powershell -ExecutionPolicy Bypass -File .\pc_netz.ps1 -Mac <Mac-IP>`
- → p50, p95, max und Verlust beider Skripte notieren, dazu Kanal und Rate aus der WLAN-Anzeige.
- Antwortet der Mac nicht auf Ping, ist vermutlich der Tarnmodus der Mac-Firewall aktiv. Dann nur den Mac-Wert nehmen.

**B2. Stream starten (5 min laufen lassen)**
- Moonlight → Mac → **Desktop**. Statistik mit **Strg+Alt+Shift+S** einblenden.
- Auf dem Mac normal arbeiten (Safari scrollen, Finanz-App öffnen).
- Nach 5 min notieren: Netzwerklatenz (Mittel/Schwankung), Decodier- und Renderzeit, „Frames dropped by network“ bzw. Jitter. Sichtbare Ruckler zählen.

**B3. Glass-to-Glass-Latenz (10 Fotos)**
- Auf dem Mac `stoppuhr.html` in Safari öffnen und Vollbild schalten (Strg+Cmd+F).
- Das MacBook so stellen, dass der Mac-Bildschirm und der PC-Monitor mit dem Stream **zusammen auf ein Foto** passen.
- Mit dem iPhone 10 Fotos machen. Pro Foto **Mac-Zeit minus PC-Zeit** = Latenz. Median und höchsten Wert notieren.

**B4. Schärfe**
- Auf dem Mac `schaerfe.html` öffnen (100 % Zoom), danach die Finanz-App.
- Bewerten am PC-Monitor mit Note 1–5: Ab welcher Textgröße ist der Text flüssig lesbar? Ist farbiger Text auf Farbe sauber? Sind die 1-Pixel-Linien ohne Moiré?
- Durchgänge: (a) HEVC 20 / 40 / 80 Mbit/s, (b) H.264 40 Mbit/s, (c) falls angeboten YUV 4:4:4 bei 40 Mbit/s.
- Einstellung jeweils in Moonlight ändern und den Stream neu starten.

**B5. AWDL-Effekt (2 × 5 min)**
- Lauf 1 normal: `./awdl.sh status` und 5 min Stream, Ruckler und Drops notieren.
- Lauf 2: `./awdl.sh down --watch` (Mac-Passwort nötig) und 5 min Stream. Mit Strg+C wird AWDL wieder eingeschaltet.
- Während Lauf 2 gehen AirDrop, Handoff, Sidecar und Universal Control nicht. Nach einem Neustart ist ohnehin alles wieder normal.

**B6. Netz unter Last**
- Während der Stream läuft, `pc_netz.ps1` noch einmal starten. Den Anstieg von p95/max gegenüber B1 notieren.

**B7. Gefühl (2 min)**
- Moonlight-Fenster etwa so groß wie ein App-Fenster ziehen. Die Finanz-App über Moonlight bedienen: tippen, scrollen, klicken.
- Note 1–5: „Fühlt sich das wie ein lokales Fenster an?“

**Optional B7b, falls WLAN schwach:** PC per LAN-Kabel an die Fritzbox, B2 bis B3 wiederholen. Das ist der wichtigste Hebel.

**Optional B8. Direktkabel Mac↔PC (ca. 20 min)**
Ein LAN-Kabel direkt vom PC zum USB-C-LAN-Adapter am Mac. Das WLAN bleibt auf beiden Rechnern an. Feste IPs werden empfohlen, weil sie sofort und immer gleich da sind.
1. **Mac:** Adapter und Kabel einstecken. Systemeinstellungen → Netzwerk → *USB-LAN* (Name je nach Adapter) → Details → TCP/IP → „IPv4 konfigurieren: Manuell“, IP `192.168.77.1`, Teilnetzmaske `255.255.255.0`, Router leer lassen → OK.
2. **PC:** Einstellungen → Netzwerk und Internet → Ethernet → IP-Zuweisung „Bearbeiten“ → Manuell, IPv4 an: IP `192.168.77.2`, Subnetzpräfixlänge `24`, Gateway und DNS leer → Speichern. Auf derselben Seite den Netzwerkprofiltyp auf **Privat** stellen, sonst blockt die Firewall.
3. **Prüfen:** Auf dem Mac `ping -c 5 192.168.77.2`. Antworten kommen in unter 1 ms.
- Alternative ohne Einstellungen: beide Seiten auf DHCP lassen. Nach etwa 1 min hat jeder eine Link-Local-Adresse `169.254.x.y` (Mac: Details → TCP/IP, PC: `ipconfig`). Diese Adressen dann statt `.1`/`.2` nehmen.
- **Messen wie oben**, nur mit den Kabel-IPs:
  - B1: `./mac_netz.sh 192.168.77.2 60` und `pc_netz.ps1 -Mac 192.168.77.1`.
  - B2/B3: Sunshine auf dem Mac in Moonlight über „+“ mit `192.168.77.1` hinzufügen und koppeln, dann wie bei B2/B3 messen.
  - B4: HEVC 80 und 150 Mbit/s.
  - B6 und B7 wie oben.
- **Kabelwechsel (Info):** Kabel während des Streams ziehen. Moonlight wechselt nicht auf WLAN, der Abbruch ist hier normal. Nur notieren, dass es so ist. Den nahtlosen Wechsel baut erst Teleport (`SCHNITTSTELLEN.md` §7).
- **Rückbau:** Auf beiden Seiten wieder „DHCP“ bzw. „Automatisch“ einstellen, oder die Einstellungen einfach lassen. Sie stören das WLAN nicht.

---

## Teil C – Aufräumen
- AWDL läuft wieder? Mit `./awdl.sh status` prüfen, sonst `./awdl.sh up`.
- `caffeinate` mit Strg+C beenden.
- Sunshine beenden. Wird es nicht mehr gebraucht: App löschen und unter Systemeinstellungen → Datenschutz die Freigaben entfernen. Moonlight darf bleiben.

---

## Falls Sunshine auf dem Mac nicht läuft
Die macOS-Version ist experimentell. Ersatz **nur zum Messen**: Parsec (Closed Source, Mac-Hosting möglich). Es wird nicht in den Fork eingebunden. Die Werte sind dann ein Richtwert, kein exakter Vergleich.

---

## Ergebnisbogen (ausfüllen und an den Entwickler geben)

| Punkt | Wert |
|---|---|
| PC-Anbindung (WLAN/LAN), Mac-Kanal/PHY/Rate | |
| B1 Ping Mac→Fritzbox p50/p95/max | |
| B1 Ping Mac→PC p50/p95/max/Verlust | |
| B1 Ping PC→Mac p50/p95/max/Verlust | |
| B2 Moonlight Netzwerklatenz Ø / Schwankung | |
| B2 Decodier-/Renderzeit | |
| B2 Drops / Ruckler pro 5 min | |
| B3 Glass-to-Glass Median / max (10 Fotos) | |
| B4 kleinste lesbare Textgröße bei HEVC 20/40/80 | |
| B4 H.264 40 / YUV 4:4:4 (falls vorhanden) | |
| B4 Farbtext / Linien (1–5) | |
| B5 Ruckler mit AWDL an / aus | |
| B6 Ping unter Last p95/max | |
| B7 Gefühl (1–5) | |
| B8 Kabel: Ping Mac→PC p50/p95/max/Verlust | |
| B8 Kabel: Moonlight Netzwerklatenz Ø / Schwankung, Drops pro 5 min | |
| B8 Kabel: Glass-to-Glass Median / max | |
| B8 Kabel: Text bei HEVC 80 / 150, Gefühl (1–5) | |
| Ampel | 🟢 / 🟡 / 🔴 |
| Bemerkungen | |
