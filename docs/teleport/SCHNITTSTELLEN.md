# Beamer-Teleport – Schnittstellen-Verträge (v1.5)

Verbindlich für alle Beteiligten (Claude Code und Cursor). Liegt im Fork unter
`docs/teleport/SCHNITTSTELLEN.md`. Änderungen nur per PR mit dem Label `vertrag`, und beide Seiten
passen dann an. Unklarheiten nicht selbst auflösen, sondern im PR als Frage melden.

Alle Ganzzahlen auf dem Draht sind **Big-Endian**. JSON ist UTF-8, ohne NaN. Zeiten sind in µs
(monotone Uhr des Senders).

Änderungen: v1.5 – §1.1: `restore` gilt nur für den Fenster-Rückfall, im virtuellen Modus geht das Fenster immer zurück; virtueller Modus ist Standard, Fenster-Rückfall nur Notlösung (ohne Menüs/Popups); bekannte Punkte (Akzentmenü, gedrückte Taste nach Helfer-Absturz). v1.4 – §3: Vorzeichen und Einheit von `scroll` wie Beamer, gehaltene Tasten nur ein `down` (Wiederholung macht der Sender), AltGr-Zeichen als `text`, System-Kürzel (Windows-Taste, Alt+Tab, Strg+Alt+Entf) werden nicht weitergeleitet; §1.2: `proxy.resize` mit optionalen `px_w`/`px_h`/`dpi` und Pixel-Regel (1 Stream-Pixel = 1 Bildschirm-Pixel). v1.3 – §1: optionale Zusatzfelder in `hello` und `stats` zugelassen (z. B. `low_power`, `encoder_slow`), Python-seitige Ereignisse wie `helper_restarted` abgegrenzt; §4.2: Einmal-Regel für `salt16`/`tid` und Testvektoren nur für Python. v1.2 – §4.1 Frame-Frist bei unvollständigen Frames: `2×RTT + 4×Jitter`, mindestens 60 ms (statt 30 ms), Maximum 250 ms unverändert. v1.1 – §7 Pfadwahl / Kabel (Direktkabel Mac↔PC), dazu `send.bitrate`, `bt_set_max_kbps` und Roaming in §4/§5. C01a (mit Freigabe von Nick): Bitratenregel nach Restverlust statt Rohverlust, `feedback` mit optionalem Rohverlust-Feld, `ping`/`pong` in beide Richtungen, Pacing im Sender, `BtRole`-Parameter als `uint32_t`. C01c: FEC-Längenfeld als `len_xor` (Vertrag an die Implementierung angepasst).

## 0. Begriffe und Komponenten

| Name | Sprache | Ort im Repo | Aufgabe |
|---|---|---|---|
| Beamer (Python) | Python | `core/`, `mac_app/`, `win_app/` | Kopplung, Schlüssel, Steuerung, Beamer-Link (TCP), UI |
| `beamer-transport` | Rust (lib, `staticlib` + `rlib`) | `teleport/transport/` | UDP-Medienstrom: Verschlüsselung, Fragmentierung, NACK, FEC, Statistik |
| Mac-Helfer `BeamerTeleport` | Swift (SwiftPM, CLI-Executable) | `teleport/mac/` | Capture/Encode/Senden, Empfangen/Decode/Anzeige, Fenstersteuerung, Eingabe einspielen |
| Win-Helfer `beamer-teleport.exe` | Rust (windows-rs) | `teleport/win/` | dasselbe für Windows |

- **Sender** ist die Seite, auf der das echte Fenster läuft. **Empfänger** ist die Seite, die das Stellvertreter-Fenster (Proxy) zeigt.
- `tid`: Teleport-ID, eine u32 ungleich 0. Die Sender-Seite vergibt sie und hält sie pro Link eindeutig. Sie ist gleichzeitig die `stream_id` im UDP-Header.
- Koordinaten für die Eingabe sind **Stream-Pixel** (float, Ursprung oben links im übertragenen Bild).

## 1. IPC Python ↔ Helfer

- Python startet den Helfer als Kindprozess: `BeamerTeleport --ipc stdio` bzw. `beamer-teleport.exe --ipc stdio`.
- **stdin/stdout: JSON Lines** (eine Nachricht pro Zeile, `\n`). **stderr: nur Log-Text.** Auf stdout darf nichts außer Protokollzeilen stehen.
- Endet stdin (EOF), räumt der Helfer innerhalb von 2 s auf (Fenster zurückschieben, virtuelle Monitore entfernen) und beendet sich mit Code 0.
- Befehl: `{"v":1,"id":<int>,"cmd":"<name>", ...}`
  Antwort: `{"v":1,"re":<id>,"ok":true, ...}` oder `{"v":1,"re":<id>,"ok":false,"error":{"code":"<code>","msg":"..."}}`.
- Ereignis (vom Helfer, ohne Anfrage): `{"v":1,"ev":"<name>", ...}`.
- Fehlercodes: `bad_request`, `not_found`, `permission` (Bildschirmaufnahme/Accessibility fehlt), `unsupported`, `busy`, `internal`.
- Binärwerte als Base64 (Standard-Alphabet, mit Padding).

### 1.1 Befehle (Python → Helfer)

| cmd | Felder | Antwort-Felder | Seite |
|---|---|---|---|
| `hello` | `beamer_version` | `helper_version`, `platform` (`mac`/`win`), `caps` (Liste aus `send_window`,`send_display`,`recv`,`inject`,`audio`), optional `low_power` (bool, Energiesparmodus des Senders) | beide |
| `transport.open` | `local_port` (0 = beliebig), `peer_host`, `peer_port` (0 = später) | `local_port` | beide |
| `transport.peer` | `peer_host`, `peer_port` | – (darf bei laufenden Streams kommen, siehe §7) | beide |
| `transport.close` | – | – | beide |
| `windows.list` | – | `windows`: `[{wid,pid,app,title,x,y,w,h,display,scale}]` (x..h in Punkten bzw. logischen Pixeln) | Sender |
| `send.start` | `tid`, `mode` (`window`/`display`), `wid` oder `display`, `codec` (`hevc`/`h264`), `max_fps`, `bitrate_kbps`, `keys` (siehe 4.2), `virtual_display` (bool) | `width`, `height` (Stream-Pixel), `scale` | Sender |
| `send.stop` | `tid`, `restore` (bool: Fenster zurück auf echten Monitor; gilt nur im Fenster-Rückfall, siehe unten) | – | Sender |
| `send.resize` | `tid`, `w`, `h` (Punkte) | `width`, `height` | Sender |
| `send.activate` | `tid` | – | Sender |
| `send.keyframe` | `tid` | – | Sender |
| `send.bitrate` | `tid`, `bitrate_kbps` | – (neue Obergrenze, siehe §7) | Sender |
| `inject` | `tid`, `input` (siehe 3) | – | Sender |
| `recv.open` | `tid`, `title`, `app`, `icon_png` (b64, optional), `w`, `h` (Punkte), `scale`, `codec`, `fullscreen` (bool), `keys` | – | Empfänger |
| `recv.update` | `tid`, optional `title`, `w`, `h` | – | Empfänger |
| `recv.close` | `tid` | – | Empfänger |
| `shutdown` | – | – | beide |

- Modi von `send.start` (v1.5): **Standard ist der virtuelle Modus** (`virtual_display:true`). Der Fenster-Rückfall (`virtual_display:false` bzw. wenn kein virtueller Monitor angelegt werden kann) ist nur eine Notlösung: Beim Einzelfenster-Capture fehlen Kontextmenüs, Popups und andere eigene Fenster der App im Strom. Python wählt den Rückfall nur, wenn der virtuelle Modus scheitert, und zeigt dem Nutzer einen Hinweis.
- `restore` (v1.5) gilt nur für den Fenster-Rückfall. Im virtuellen Modus geht das Fenster bei `send.stop`, EOF, `shutdown` und Absturz-Handler **immer** an alte Position und Größe zurück, weil der virtuelle Monitor danach verschwindet; `restore:false` wird dort ignoriert.
- Bekannte Punkte (v1.5): (a) Das Akzentmenü beim Gedrückthalten (Press-and-Hold von macOS) wird beim Senden nicht nachgebildet; Akzente kommen über tote Tasten bzw. `text`. (b) Stirbt der Sender-Helfer, während eine Taste oder ein Button gedrückt ist, kann sie im Zielsystem gedrückt bleiben (SIGKILL ist nicht abfangbar). Abhilfe: Nach `helper_restarted` lässt der neu gestartete Helfer bzw. Python alle Tasten und Buttons los (Folgepaket).

### 1.2 Ereignisse (Helfer → Python)

| ev | Felder | Bedeutung |
|---|---|---|
| `proxy.input` | `tid`, `input` (siehe 3) | Eingabe im Proxy-Fenster, die Python an den Sender weiterleitet |
| `proxy.focus` | `tid`, `focused` | Proxy hat den Fokus bekommen oder verloren |
| `proxy.resize` | `tid`, `w`, `h`, optional `px_w`, `px_h`, `dpi` | Nutzer hat die Proxy-Größe geändert (`w`/`h` in Punkten, `px_w`/`px_h` in Bildschirm-Pixeln der Client-Fläche, `dpi` des Monitors; Pixel-Regel siehe unten) |
| `proxy.closed` | `tid` | Nutzer hat den Proxy geschlossen (bedeutet: zurückholen) |
| `send.gone` | `tid`, `reason` | Fenster geschlossen bzw. Capture beendet |
| `send.size` | `tid`, `width`, `height`, `scale` | Stream-Größe hat sich geändert |
| `stats` | `tid`, `fps`, `kbps`, `rtt_ms`, `loss_pct`, `latency_ms` (geschätzt); dazu optionale Zusatzfelder (siehe unten) | alle 1 s |
| `error` | `code`, `msg`, optional `tid` | asynchroner Fehler |

- Zusatzfelder (v1.3): Helfer dürfen in Antworten und Ereignissen weitere Felder melden; Python ignoriert unbekannte Felder und verlässt sich nur auf die Felder dieser Tabelle. Dokumentiert werden Zusatzfelder in der README des Helfers (`teleport/mac/README.md`: u. a. `encoder_slow`, `effective_fps`, `low_power`, `backlog_drops`; `teleport/win/README.md`: u. a. `recv_to_present_ms`, `frames_presented`, `dropped_*`). Wird Python von einem Zusatzfeld abhängig (z. B. Codec-Wahl nach `low_power`/`encoder_slow`), wandert es zuerst per Vertrags-PR in diese Tabelle.
- Pixel-Regel für `proxy.resize` (v1.4): Ziel ist **1 Stream-Pixel = 1 Bildschirm-Pixel** auf dem Empfänger. Meldet der Proxy `px_w`/`px_h`, rechnet Python daraus die Sendegröße: `send.resize` mit `w = round(px_w / s)`, `h = round(px_h / s)`, wobei `s` der `scale` des Senders aus der letzten `send.start`-Antwort bzw. `send.size` ist. Fehlen `px_w`/`px_h`, gilt `px = w × dpi / 96` (Windows) bzw. `px = w × scale` des Empfängers. Der Windows-Proxy meldet `w`/`h` = Client-Pixel × 96 / `dpi`. Weicht die Streamgröße danach leicht ab (Encoder-Ausrichtung, Mindest- oder Höchstgröße), skaliert der Proxy ohne Verzerrung (Letterbox). Ein Monitorwechsel mit anderer DPI löst ein `proxy.resize` aus; die Antwort `recv.update` löst keines aus.
- Python-seitige Ereignisse: `core/teleport_ipc.py` erzeugt selbst `helper_restarted` (`code` = Exit-Code; Helfer nach Absturz neu gestartet, alle Transporte und Streams sind weg und müssen mit **neuen** Schlüsseln neu geöffnet werden) und `error`/`bad_request` bei unlesbaren Zeilen. Sie kommen nie vom Helfer und gehören nicht zum IPC-Draht.

## 2. Beamer-Link: neue Nachrichten (Python ↔ Python, über den bestehenden verschlüsselten TCP-Link)

- Fähigkeit: Im `hello`/`welcome` kommt das Feld `"teleport":1` dazu. Ohne dieses Feld werden keine `tp_*`-Nachrichten gesendet. Die Wire-Version bleibt 6.
- Nachrichten (JSON wie die übrigen Beamer-Nachrichten, `type`-Feld):

| type | Richtung | Felder |
|---|---|---|
| `tp_offer` | Sender→Empf. | `tid`, `app`, `title`, `icon_png`, `w`, `h`, `scale`, `codec`, `mode`, `udp_port`, `salt` (b64, 16 B zufällig) |
| `tp_accept` | Empf.→Sender | `tid`, `udp_port` |
| `tp_reject` | Empf.→Sender | `tid`, `reason` |
| `tp_input` | Empf.→Sender | `tid`, `input` |
| `tp_resize` | Empf.→Sender | `tid`, `w`, `h` |
| `tp_focus` | Empf.→Sender | `tid` |
| `tp_update` | Sender→Empf. | `tid`, optional `title`, `w`, `h`, `scale` |
| `tp_close` | beide | `tid`, `reason` (`user`,`gone`,`error`,`return`) |

## 3. Fenster-Eingabe (`input`-Objekt)

```json
{"k":"move","x":512.0,"y":300.5}
{"k":"down","x":512.0,"y":300.5,"b":"left","n":1}
{"k":"up","x":512.0,"y":300.5,"b":"left"}
{"k":"scroll","x":512.0,"y":300.5,"dx":0,"dy":-3.0,"unit":"line"}
{"k":"key","key":"a","down":true}
{"k":"text","text":"ä"}
```

- `b`: `left`/`right`/`middle`/`back`/`forward`. `n`: Klickzahl (1/2/3). `unit`: `line` oder `px`.
- `scroll` (v1.4) wie Beamer (`core/protocol.py` `scroll_msg`, `win_app/capture_win.py` `wheel_notches`, `win_app/input_injector.py`): `dy` positiv = vom Nutzer weg (Rad nach vorn, Inhalt nach oben), `dx` positiv = nach rechts. `unit:"line"`: Rastungen, 1.0 = eine Rastung (`WHEEL_DELTA` 120 unter Windows), Bruchteile erlaubt. `unit:"px"`: Pixel (Trackpad). Die Seite, die erfasst, meldet das Rad roh; Bildlaufrichtung und -tempo stellt nur Python nach Beamers Einstellungen um, kein Helfer.
- `key`: **Beamer-Tastennamen**, also dieselben Namen wie bei `keydown`/`keyup` in `core/protocol.py` und `core/keytable.py`. Die Modifier-Umsetzung Cmd↔Strg macht **Python** nach Beamers Tastatur-Einstellung, bevor `inject` an den Helfer geht. Der Helfer spielt die Namen 1:1 ein.
- Der Sender rechnet Stream-Pixel in Bildschirmkoordinaten des echten bzw. virtuellen Monitors um. Liegt eine Eingabe außerhalb des Bildes, wird sie verworfen.
- Vor `down`, `key` und `text` aktiviert der Sender das Fenster, falls es nicht vorne ist.
- Gehaltene Tasten (v1.4): Der Proxy meldet **ein** `down` und beim Loslassen ein `up`, keine Auto-Repeat-`down`s. Die Wiederholung erzeugt der Sender selbst nach der Wiederholverzögerung und -rate seines Systems, solange kein `up` kam. Bei Fokusverlust des Proxys, `send.stop` oder Ende des Teleports endet jede Wiederholung (der Proxy schickt zuvor die `up`s).
- AltGr (v1.4): Zeichen, die über AltGr entstehen (z. B. AltGr+Q auf deutschem Layout), gehen als `text` („@“). Das AltGr-Paar selbst (unter Windows rechtes Alt mit dem künstlichen linken Strg) wird nicht als `key` gemeldet. Auf Layouts ohne AltGr bleibt rechtes Alt `alt_r`.
- Nicht weitergeleitet (v1.4): System-Kürzel bleiben beim Empfänger-System und erzeugen kein `proxy.input`. Unter Windows: Windows-Taste (allein und mit anderen Tasten), Alt+Tab, Strg+Alt+Entf, Alt+Leertaste; Alt+F4 schließt den Proxy (`proxy.closed`). Strg+Alt+Shift+F schaltet im Proxy das Vollbild um und geht ebenfalls nicht zum Sender.

## 4. UDP-Medienstrom

### 4.1 Paket

```
Offset Größe Feld (Header = AAD, unverschlüsselt)
0      1     version = 1
1      1     type: 1=video 2=audio 3=fec 4=nack 5=keyframe_req 6=feedback 7=ping 8=pong
2      2     flags (reserviert, 0)
4      4     stream_id (= tid)
8      8     seq (u64, pro Richtung und stream_id ab 1, +1 je Paket)
16     ...   ChaCha20-Poly1305-IETF-Ciphertext inkl. 16-B-Tag
```

- Nonce (12 B) = `prefix_dir` (4 B) ‖ `seq` (8 B, Big-Endian). AAD = die 16 Header-Bytes.
- Ein Paket ist höchstens **1200 Byte** groß, inklusive Header und Tag.
- Replay-Schutz: Fenster von 4096 seq-Werten, Duplikate und zu alte Pakete werden verworfen. Pakete, die die Authentifizierung nicht bestehen, werden still verworfen und gezählt.
- Roaming: Die Gegenstelle ist an `stream_id` und Schlüssel gebunden, nicht an die IP. Kommt ein Paket von einer neuen Quelladresse, das die Authentifizierung besteht, kein Replay ist und dessen `seq` größer ist als jede bisher gesehene dieses Streams, übernimmt der Transport diese Adresse als Ziel für alle Streams der Session.
- Medien laufen vom Sender zum Empfänger (Richtung `s2r`). `nack`, `keyframe_req`, `feedback`, `ping` und `pong` laufen in Gegenrichtung (`r2s`), `pong` als Antwort auch `s2r`. Der Sender schickt zusätzlich eigene `ping` (`s2r`, ab 500 ms nach dem ersten Paket, dann alle 500 ms), der Empfänger antwortet mit `pong` (`r2s`); so kennt auch der Sender die RTT.

Klartext je Typ:
- **video/audio**: `frame_id` u32 ‖ `frag_idx` u16 ‖ `frag_cnt` u16 ‖ `pts_us` u64 ‖ `fflags` u8 (Bit0 = Keyframe, Bit1–3 Codec: 1=h264, 2=hevc, 3=opus) ‖ Nutzdaten. Video ist Annex-B. Jeder Keyframe enthält VPS/SPS/PPS inline.
- **fec**: `frame_id` u32 ‖ `group_start` u16 (`frag_idx` des ersten Fragments) ‖ `group_len` u16 (1–8 aufeinanderfolgende Fragmente desselben Frames) ‖ `len_xor` u16 (XOR der Klartextlängen der Gruppe) ‖ XOR-Parität über die Klartexte der Gruppe. Klartext heißt hier das ganze video-Klartextpaket inklusive Media-Kopf. Kürzere Klartexte werden mit Nullen aufgefüllt, die Parität ist so lang wie der längste. Rekonstruktion bei genau einem fehlenden Fragment: Klartext = Parität XOR übrige Klartexte, gekürzt auf `len_xor` XOR übrige Längen. Standard ist eine Parität je 8 Fragmente, nur für Video, abschaltbar. Die Nutzlast eines Fragments ist höchstens 1141 Byte, damit auch das FEC-Paket in 1200 Byte passt.
- **nack**: Anzahl u16 ‖ n × seq u64.
- **keyframe_req**: leer.
- **feedback**: `recv_kbps` u32 ‖ `loss_permille` u16 ‖ `jitter_us` u32 ‖ `last_frame_id` u32 ‖ optional `raw_loss_permille` u16. `loss_permille` ist der **Restverlust** nach NACK/FEC-Reparatur (Pakete, die weder nachgeliefert noch per FEC ersetzt wurden), `raw_loss_permille` der Rohverlust im Fenster. Fehlt das Feld (alte Gegenstelle), gilt `loss_permille` für beides.
- **ping/pong**: `t_us` u64 (pong spiegelt das t_us des pings).

Verhalten:
- Der Empfänger schickt bei Lücken einen NACK. Ist ein Frame nach 2 × RTT + 4 × Jitter (mindestens 60 ms, höchstens 250 ms, gerechnet ab dem letzten Paket des Frames) noch unvollständig, wird er verworfen und ein `keyframe_req` geschickt (höchstens alle 300 ms).
- Der Sender hält einen Puffer zum erneuten Senden von 300 ms.
- Bitratenregelung (Sender, v1.1/C01a): Zielbitrate um 15 % pro Sekunde senken bei **Restverlust** `loss_permille` > 10 (1 %), bei **Rohverlust** `raw_loss_permille` > 80 (8 %, Notbremse) oder bei steigendem Jitter (zwei Anstiege in Folge um je ≥ 15 % und ≥ 2 ms). Sonst um 5 % pro Sekunde anheben, bis zur Obergrenze (`bitrate_kbps`, `bt_set_max_kbps`). Untergrenze: Obergrenze/20, mindestens 200 kbit/s. Grund: FEC und NACK heilen Verluste unterhalb dieser Schwellen, eine Drosselung nach Rohverlust würde nur Qualität kosten.
- Pacing (Sender): Token-Bucket mit 1,5 × Zielbitrate, Burst höchstens 64 KB; Wiederholungen und Steuerpakete laufen durch denselben Bucket. Die Sendewarteschlange ist zeitlich begrenzt: Liegt die Wartezeit der bereits wartenden Pakete bei aktueller Bucket-Rate über 150 ms, lehnt `bt_send_frame` mit einem Fehler ab (der Encoder soll das Frame auslassen); darunter wird auch ein einzelnes großes Keyframe angenommen.

### 4.2 Schlüssel

Python leitet die Schlüssel ab. Die Helfer bekommen nur fertige Schlüssel, im Feld `keys` von `send.start`/`recv.open`:

```json
"keys": {"s2r_key":"<b64 32B>","s2r_prefix":"<b64 4B>","r2s_key":"<b64 32B>","r2s_prefix":"<b64 4B>"}
```

Ableitung (Python, `core/teleport_keys.py`, HKDF-SHA256 wie `protocol.hkdf_sha256`):
- `T` = `link_info(key_id, prefix_i, prefix_r, share_i, share_r, 1)` ohne das führende `b"beamer-frames"` und ohne das letzte Richtungs-Byte. Das ist das Handshake-Transkript des aktuellen Links.
- `master = HKDF(ikm = link_ikm(token, shared), salt = b"beamer-teleport-v1", info = b"tp-master" + T)`. Wird beim Link-Aufbau einmal berechnet und gehalten. `shared` wird danach verworfen. Daraus folgt: Fällt der Link weg, enden alle Teleports. Ausnahme: ein Pfadwechsel nach §7 (neuer Link steht innerhalb von 5 s). Laufende Teleports behalten dann ihre Schlüssel. Neue Teleports nutzen den Master des neuen Links.
- Pro Teleport, mit `salt16` aus `tp_offer` und `sid = tid` als u32 BE:
  - `s2r_key = HKDF(master, salt16 + sid, b"tp-key-s2r")`
  - `r2s_key = HKDF(master, salt16 + sid, b"tp-key-r2s")`
  - `s2r_prefix = HKDF(master, salt16 + sid, b"tp-pfx-s2r")[:4]`
  - `r2s_prefix = HKDF(master, salt16 + sid, b"tp-pfx-r2s")[:4]`
- Einmal-Regel (v1.3): `salt16` ist pro Teleport frisch zufällig; ein Paar (`salt16`, `tid`) wird je Master höchstens einmal abgeleitet. Weil alle vier Werte aus demselben Paar entstehen, schützt das beide Richtungen (s2r und r2s) vor Nonce-Wiederverwendung; eine eigene r2s-Sperre ist nicht nötig. Die Sperre in `bt_add_stream` (§5) ist nur das zweite Netz.
- Testvektoren legt Cursor in `docs/teleport/vectors.json` ab. Sie gelten nur für Python: Die Helfer leiten nichts ab, sie bekommen fertige Schlüssel (deshalb keine Rust-Prüfung der Vektoren).

## 5. Rust-API `beamer-transport` (C-ABI, Header per cbindgen: `teleport/transport/include/beamer_transport.h`)

```c
typedef struct BtSession BtSession;
typedef enum { BT_ROLE_SENDER = 1, BT_ROLE_RECEIVER = 2 } BtRole;
typedef struct { const uint8_t *data; size_t len; uint64_t pts_us; uint8_t fflags; uint32_t frame_id; } BtFrame;
typedef void (*BtOnFrame)(void *ctx, uint32_t stream_id, uint8_t kind /*1 video 2 audio*/, const BtFrame *f);
typedef void (*BtOnControl)(void *ctx, uint32_t stream_id, uint8_t type /*5 keyframe_req*/);
typedef void (*BtOnStats)(void *ctx, uint32_t stream_id, const char *json);

BtSession *bt_open(uint16_t local_port, BtOnFrame, BtOnControl, BtOnStats, void *ctx); // NULL bei Fehler
uint16_t   bt_local_port(const BtSession *);
int        bt_set_peer(BtSession *, const char *host, uint16_t port);
int        bt_add_stream(BtSession *, uint32_t stream_id, uint32_t role /* BtRole */,
                         const uint8_t s2r_key[32], const uint8_t s2r_prefix[4],
                         const uint8_t r2s_key[32], const uint8_t r2s_prefix[4],
                         uint32_t max_kbps);
int        bt_remove_stream(BtSession *, uint32_t stream_id);
int        bt_send_frame(BtSession *, uint32_t stream_id, uint8_t kind, const uint8_t *data, size_t len,
                         uint64_t pts_us, uint8_t fflags);               // fragmentiert, FEC, Puffer
int        bt_request_keyframe(BtSession *, uint32_t stream_id);
uint32_t   bt_target_kbps(const BtSession *, uint32_t stream_id);       // für den Encoder
int        bt_set_max_kbps(BtSession *, uint32_t stream_id, uint32_t max_kbps); // v1.1, §7
void       bt_close(BtSession *);
const char*bt_last_error(void);                                         // thread-lokal
```

- Rückgabe 0 bedeutet OK, <0 einen Fehler. `role` ist ein `uint32_t` mit den Werten von `BtRole`; andere Werte liefern einen Fehler (kein undefiniertes Verhalten).
- `bt_add_stream` lehnt ein Schlüsselpaar (Schlüssel + Präfix der Senderichtung) ab, das die Session schon benutzt hat, auch nach `bt_remove_stream`: gleiche Nonces mit neuem `seq` wären Nonce-Wiederverwendung. Die Sperre gilt nur innerhalb einer Session (sie merkt sich die Schlüssel nicht über einen Neustart hinaus): Frische Schlüssel pro Teleport sind Pflicht der Python-Seite (§4.2).
- Die Callbacks kommen aus einem Transport-Thread. Der Aufrufer muss sie schnell verarbeiten (in eine Queue legen).
- Ein Socket pro Session, beliebig viele Streams. Der Socket ist an `0.0.0.0` gebunden (alle Interfaces), damit derselbe Port über Kabel und WLAN erreichbar ist.
- `bt_set_peer` darf jederzeit kommen, auch bei laufenden Streams. Es wirkt sofort, `seq`, Schlüssel und Puffer laufen weiter. Danach schickt der Transport sofort ein `ping` (Empfänger) bzw. das nächste Paket (Sender) an die neue Adresse.
- Die Stats-JSON enthält zusätzlich `peer` (`"ip:port"`).

## 6. Repo-Struktur und Branches

```
teleport/transport/   Rust-Crate beamer-transport (+ include/, tests/, examples/)
teleport/mac/         SwiftPM-Paket BeamerTeleport
teleport/win/         Rust-Crate beamer-teleport-win
core/teleport*.py     Python-Seite (Cursor)
docs/teleport/        SCHNITTSTELLEN.md, AUFTEILUNG.md, DEPENDENCIES.md, M0-TEST.md, vectors.json
tools/teleport/       Test-/Demo-Skripte
```

- Integrations-Branch: `teleport`. Upstream-`main` bleibt unverändert, damit Upstream-Releases sauber gemergt werden können.
- Arbeits-Branches: `claude/NN-kurzname` bzw. `cursor/kurzname`. PR immer nach `teleport`. **Kein Force-Push, kein Rebase von gepushten Commits, kein Push auf `main` oder `teleport` direkt.**
- Lizenz: Der Fork bleibt GPL-3.0. Erlaubt sind nur GPL-3-kompatible Abhängigkeiten (MIT, BSD, Apache-2.0, MPL-2.0, ISC, Zlib, LGPL, GPL-2.0-or-later, GPL-3.0). **Verboten** sind AGPL, proprietäre Teile, SSPL und „source-available“. Neue Abhängigkeiten kommen mit Lizenz in `docs/teleport/DEPENDENCIES.md`.

## 7. Pfadwahl / Kabel (v1.1)

Ziel: Steckt ein direktes LAN-Kabel zwischen Mac (USB-C-LAN-Adapter) und PC, laufen Beamer-Link und Medienstrom darüber. Ohne Kabel läuft alles über WLAN. Der Wechsel geschieht im laufenden Stream, ohne dass ein Teleport endet.

**Pfade**
- `cable`: Ethernet direkt Mac↔PC. Adressen entweder Link-Local `169.254.0.0/16` (ohne Konfiguration) oder feste IPs im Netz `teleport.cable_subnet` (Einstellung, Standard `192.168.77.0/24`, Mac `.1`, PC `.2`, ohne Gateway).
- `wlan`: Heimnetz über die Fritzbox (auch wenn der PC per Kabel an der Fritzbox hängt).
- VPN, Tunnel, `/31` und `/32` werden ignoriert, wie in `core/pairing.py` (`_interfaces()`).

**Erkennung (Python, `core/teleport_path.py`)**
- Ein Interface ist Kabel-Kandidat, wenn es up ist, eine Broadcast-Adresse hat und seine IPv4-Adresse in `169.254/16` oder in `cable_subnet` liegt.
- Als Kabel gilt es erst, wenn die Gegenstelle darüber antwortet (Probe, siehe unten). Grund: Windows vergibt `169.254` auch an ein WLAN ohne DHCP.
- Interfaces werden alle 2 s neu gelesen und zusätzlich bei Netzwechsel-Ereignissen des Systems, falls vorhanden.

**Discovery**
- Das Beacon des PCs geht schon heute an jedes Interface, auch an `169.254/16` (`broadcast_targets`). Der Mac sammelt pro Peer-ID alle Absenderadressen als Kandidaten: `{addr, path, last_seen, rtt_ms}`.
- Probe: TCP-Connect auf 24820 an jeden Kandidaten, alle 2 s, ohne Handshake. RTT ist die Zeit bis zum Connect.
- Kandidaten mit `cable` werden mit fester IP auch ohne Beacon direkt geprüft (Gegenstelle `.1`↔`.2`).

**Wahl und Hysterese**
- Bester Pfad: `cable`, wenn die letzten 2 Proben erfolgreich waren. Sonst `wlan`.
- Wechsel auf `cable` nach 2 erfolgreichen Proben in Folge. Wechsel weg von `cable` sofort bei Link-down des Interfaces, bei 2 verlorenen Proben oder wenn 1 s lang kein Medienpaket über Kabel kam (Stats).
- Die Einstellung `teleport.path` kann `auto` (Standard), `cable` oder `wlan` sein. Bei `cable` gibt es keinen Rückfall, nur eine Warnung.

**Steuerkanal (Beamer-Link, TCP 24820)**
- Der Link läuft über den besten Pfad. Beim Wechsel: neuen Link zur neuen Adresse aufbauen (normales `hello`/`welcome`, gleiche Kopplung), erst danach auf ihn umschalten und den alten schließen (make-before-break).
- Laufende Teleports bleiben bestehen (§4.2, Ausnahme). Ihr `tid` gilt weiter. `tp_*`-Nachrichten gehen ab dem Umschalten über den neuen Link.
- Steht nach 5 s kein Link, enden alle Teleports mit `tp_close` `reason:"error"`.

**Medienstrom (UDP)**
- Bei jedem Pfadwechsel schickt Python beiden Helfern `transport.peer` mit der neuen Adresse. Der UDP-Port bleibt gleich (Socket an `0.0.0.0`).
- Kommt `transport.peer` zu spät oder geht verloren, greift das Roaming (§4.1): Das erste gültige, authentifizierte Paket von der neuen Adresse stellt das Ziel um.
- Ziel: Einstecken des Kabels ohne sichtbare Unterbrechung. Ziehen des Kabels höchstens 1 s Aussetzer, danach ein Keyframe.

**Bitrate**
- Obergrenze `bitrate_kbps` je Pfad: `wlan` 40 000, `cable` 150 000 (Einstellungen `teleport.max_kbps_wlan` / `teleport.max_kbps_cable`).
- Beim Wechsel schickt Python dem Sender `send.bitrate`. Der Helfer ruft `bt_set_max_kbps` und stellt den Encoder um.
- Wechsel auf `cable`: Zielbitrate springt sofort auf 50 % der neuen Obergrenze, danach normal +5 %/s. Wechsel auf `wlan`: Zielbitrate sofort auf höchstens die WLAN-Obergrenze.
- Paketgröße bleibt bei 1200 Byte (keine Jumbo-Frames), FEC und NACK bleiben an.

**Sicherheit und Windows**
- Der Kabelpfad bringt kein neues Vertrauen mit. Link und Medien sind genauso gekoppelt und verschlüsselt. Roaming folgt nur Paketen, die die Authentifizierung bestehen und kein Replay sind.
- Windows stuft das Kabelnetz als „Nicht identifiziertes Netzwerk“ (Öffentlich) ein. Entweder das Netzwerkprofil auf „Privat“ stellen oder die Beamer-Firewallregel auch für „Öffentlich“, beschränkt auf die Remote-Adressen `169.254.0.0/16` und `cable_subnet`, zulassen.

**Tests (Abnahme)**
- Transport: Loopback mit zwei lokalen Adressen (z. B. `127.0.0.1` und `127.0.0.2`). Während eines Streams wechselt die Quelladresse, es geht kein Frame über einen Keyframe hinaus verloren. Ein manipuliertes oder wiederholtes Paket von einer fremden Adresse ändert das Ziel nicht.
- Python: Pfadwahl mit gefälschten Interfaces und Proben (Einstecken, Ziehen, Flattern), Make-before-break des Links mit einem Fake-Peer.
- Gerät: B8 aus `M0-TEST.md` plus Kabel ziehen und stecken während eines Teleports.
