# Proxy-Chrome 1:1 (macOS Sonoma/Sequoia)

Vergleich des Windows-Proxys mit einem echten Mac-Fenster. Die gezeichnete Leiste gilt nur, wenn der Stream **keine** AX-Titelleiste hat (`title_bar:false`). Sonst bleibt die Mac-Leiste unverändert.

## Masse (Punkte, 1 pt = 1 px bei 100 %)

| Teil | Soll |
|---|---|
| Titelleiste | 28 pt hoch |
| Ampel-Durchmesser | 12 pt |
| Abstand zwischen Ampeln | 8 pt |
| Erste Ampel vom linken Rand | 20 pt (Mittelpunkt bei x = 26) |
| Ampeln vertikal | mittig (y = 14) |
| Farben | `#FF5F57`, `#FEBC2E`, `#28C840`, dunkler Ring 0,5 pt |
| Hover auf der Gruppe | Symbole × − ⤢ |
| Inaktiv, kein Hover | Ampeln grau |
| Ecken | Windows-11-DWM rund (~8 pt, nächstliegend zu 10 pt) |
| Schatten | DWM-Drop-Shadow |
| Hairline | 1 px heller Innenrand oben, 1 px Trenner unter der Leiste |
| Windows-Rahmen | keiner (WS_POPUP, `WM_NCCALCSIZE` 0, kein heller 1-px-Streifen oben) |

## BMP-Vorschau (ohne Live-Stream)

Auf dem Windows-PC, nach dem Bauen des Helfers:

```
beamer-teleport.exe --chrome-preview %TEMP%\teleport-chrome --width 480 --scale 1 --title Safari
beamer-teleport.exe --chrome-preview %TEMP%\teleport-chrome --width 480 --scale 2 --title Safari
```

Dateien: `titlebar-{dark|light}-{active|inactive|hover}-{1|2}x.bmp`.

## Checkliste am echten Fenster

1. **SwiftUI / transparente Titelleiste** (wie im Test-Screenshot): Proxy hat eigene Leiste mit Titel und Ampeln; Stream beginnt darunter; keine doppelte Leiste.
2. **TextEdit / Standard-AppKit** mit `AXTitleBar`: keine zweite Leiste, Ampeln im Stream, Klick rot holt zurück, gelb minimiert, grün maximiert.
3. **Hover** über die Ampelgruppe: × − ⤢ erscheinen; wegziehen blendet sie aus.
4. **Inaktives Proxy** (anderes Fenster vorn): Ampeln grau, Titel gedimmt.
5. **Oben kein heller Windows-Streifen**; abgerundete Ecken und Schatten sichtbar.
6. **Resize** an den Kanten: keine schwarzen Balken; Mac-Fenster ändert die Größe; nach der Antwort stimmt das Seitenverhältnis.
7. **Ziehen über den Beamer-Rand** (Mac unter dem PC = oben, Mac links = rechts): Cursor am Übergang teleportiert, auch während des Titelleisten-Zugs. Log `Teleport-Kante:` auf stderr der Mac-App.

Lizenz der Masse: eigene Nachbildung (keine übernommene GUI-Datei); Farben und Abstände nach öffentlich dokumentiertem macOS-Window-Chrome, kompatibel mit GPL-3.0.
