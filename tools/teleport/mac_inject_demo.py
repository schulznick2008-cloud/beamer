#!/usr/bin/env python3
"""Abnahme-Skript fuer Paket C04 (Mac-Sender: Eingabe, Fenstersteuerung, virtueller Monitor).

Startet das Pruef-Fenster `tools/teleport/input_probe.swift`, den Selbsttest-Empfaenger und den Helfer
(`BeamerTeleport --ipc stdio`), schickt `send.start`, eine Folge von `inject`-Befehlen (Maus, Doppelklick, Rechtsklick,
Scrollen, Tippen, Umlaut ueber `text`) und `send.resize`, und prueft anhand des Protokolls des Pruef-Fensters, dass alles
pixelgenau im Zielfenster angekommen ist.

  tools/teleport/mac_inject_demo.py                 # Fenster-Rueckfall (ohne virtuellen Monitor)
  tools/teleport/mac_inject_demo.py --virtual       # mit virtuellem Monitor
  tools/teleport/mac_inject_demo.py --app TextEdit --no-launch --sequence-only   # nur Eingaben schicken

Voraussetzungen: Freigabe Bildschirmaufnahme UND Bedienungshilfen fuer das Programm, das dieses Skript startet. Fehlt die
Bedienungshilfen-Freigabe, antwortet der Helfer mit `permission` (Exit-Code 2).
Die Schluessel sind feste TEST-Schluessel (siehe StreamKeys.testOnly).
"""
import argparse
import base64
import json
import os
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HELPER = os.path.join(ROOT, "teleport", "mac", ".build", "release", "BeamerTeleport")
PROBE_SRC = os.path.join(ROOT, "tools", "teleport", "input_probe.swift")
TEST_KEYS = {
    "s2r_key": base64.b64encode(bytes([0x11] * 32)).decode(),
    "s2r_prefix": base64.b64encode(bytes([0xA1, 0xA2, 0xA3, 0xA4])).decode(),
    "r2s_key": base64.b64encode(bytes([0x22] * 32)).decode(),
    "r2s_prefix": base64.b64encode(bytes([0xB1, 0xB2, 0xB3, 0xB4])).decode(),
}
MENU_ALLOWANCE = 40  # SenderEngine.menuBarAllowance


class Helper:
    def __init__(self, path):
        self.p = subprocess.Popen([path, "--ipc", "stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=sys.stderr, text=True, bufsize=1)
        self.next_id = 1
        self.replies = {}
        self.events = []
        self.cv = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            line = line.strip()
            if not line:
                continue
            m = json.loads(line)
            with self.cv:
                if "re" in m:
                    self.replies[m["re"]] = m
                else:
                    m["_at"] = time.time()
                    self.events.append(m)
                self.cv.notify_all()

    def call(self, cmd, timeout=15, **kw):
        i = self.next_id
        self.next_id += 1
        t0 = time.time()
        self.p.stdin.write(json.dumps({"v": 1, "id": i, "cmd": cmd, **kw}) + "\n")
        self.p.stdin.flush()
        end = t0 + timeout
        with self.cv:
            while i not in self.replies:
                left = end - time.time()
                if left <= 0:
                    raise TimeoutError(cmd)
                self.cv.wait(left)
            r = self.replies.pop(i)
        r["_ms"] = (time.time() - t0) * 1000
        return r

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=3)
        except Exception:
            self.p.kill()

    def events_named(self, name, since=0.0):
        with self.cv:
            return [e for e in self.events if e.get("ev") == name and e["_at"] >= since]


class Probe:
    """Liest das JSON-Lines-Protokoll des Pruef-Fensters."""

    def __init__(self, path):
        self.path = path

    def events(self):
        out = []
        try:
            for line in open(self.path, encoding="utf-8"):
                line = line.strip()
                if line:
                    out.append(json.loads(line))
        except FileNotFoundError:
            pass
        return out

    def wait_for(self, pred, timeout=3.0):
        end = time.time() + timeout
        while time.time() < end:
            hit = [e for e in self.events() if pred(e)]
            if hit:
                return hit
            time.sleep(0.02)
        return []


class Report:
    def __init__(self):
        self.rows = []

    def check(self, name, ok, detail=""):
        self.rows.append((name, bool(ok), detail))
        print(("  OK    " if ok else "  FEHLER ") + name + (f"  [{detail}]" if detail else ""))
        return ok

    @property
    def failed(self):
        return [r for r in self.rows if not r[1]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--helper", default=HELPER)
    ap.add_argument("--virtual", action="store_true", help="virtual_display:true")
    ap.add_argument("--no-launch", action="store_true", help="Pruef-Fenster nicht selbst starten")
    ap.add_argument("--app", default="teleport_probe", help="App-Name des Zielfensters")
    ap.add_argument("--probe-log", default="/tmp/teleport_probe.log")
    ap.add_argument("--port", type=int, default=47030)
    ap.add_argument("--dump-dir", default=None, help="Selbsttest-Empfaenger schreibt hier PNGs des Streams hin")
    ap.add_argument("--codec", default="h264", choices=["h264", "hevc"])
    a = ap.parse_args()

    procs = []
    probe_proc = None
    if not a.no_launch:
        subprocess.run(["swiftc", "-O", PROBE_SRC, "-o", "/tmp/teleport_probe"], check=True)
        probe_proc = subprocess.Popen(["/tmp/teleport_probe", a.probe_log])
        procs.append(probe_proc)
    probe = Probe(a.probe_log)
    if not a.no_launch and not probe.wait_for(lambda e: e.get("ev") == "ready", 8):
        print("Pruef-Fenster startet nicht")
        return 1
    time.sleep(2.0)  # das Fenster setzt sich beim Start erst noch zurecht

    recv_args = [a.helper, "--selftest-recv", str(a.port), "--seconds", "120"]
    if a.dump_dir:
        os.makedirs(a.dump_dir, exist_ok=True)
        recv_args += ["--dump-dir", a.dump_dir]
    recv = subprocess.Popen(recv_args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    procs.append(recv)

    h = Helper(a.helper)
    rep = Report()
    code = 0
    try:
        h.call("hello", beamer_version="demo")
        r = h.call("transport.open", local_port=0, peer_host="127.0.0.1", peer_port=a.port)
        assert r["ok"], r
        r = h.call("windows.list")
        if not r.get("ok"):
            print("windows.list:", r)
            return 1
        cands = [w for w in r["windows"] if a.app.lower() in w["app"].lower() or "TeleportInputProbe" in w["title"]]
        if not cands:
            print("Zielfenster nicht gefunden")
            return 1
        win = max(cands, key=lambda w: w["w"] * w["h"])
        orig_frame = (win["x"], win["y"])
        start = h.call("send.start", tid=1, mode="window", wid=win["wid"], codec=a.codec, max_fps=30,
                       bitrate_kbps=20000, keys=TEST_KEYS, virtual_display=a.virtual)
        if not start.get("ok"):
            err = start.get("error", {})
            print("send.start:", start)
            return 2 if err.get("code") == "permission" else 1
        W, H, scale = start["width"], start["height"], start["scale"]
        print(f"Stream {W}x{H} px, Skalierung {scale}, Modus: {'virtueller Monitor' if a.virtual else 'Fenster'}")
        time.sleep(1.5)

        def inject(**inp):
            r = h.call("inject", tid=1, input=inp)
            if not r.get("ok"):
                err = r.get("error", {})
                raise RuntimeError(f"inject {inp}: {err}")
            time.sleep(0.04)

        # --- Kalibrieren: wo liegt der Fensterinhalt im Stream (Titelleiste)? ---
        inject(k="move", x=1000.0, y=800.0)
        mv = probe.wait_for(lambda e: e.get("ev") == "move" and abs(e["screen"][0] - 0) >= 0, 3)
        if not mv:
            print("Das Pruef-Fenster hat die Bewegung nicht gesehen: Bedienungshilfen/Fokus? (Maus bewegt sich nicht ins Fenster)")
            rep.check("Mausbewegung kommt an", False)
            return 1
        last = mv[-1]
        wx, wy = last["win"][0], last["win"][1]
        off_y = MENU_ALLOWANCE if a.virtual else 0.0
        # erwartet: Fensterbezogene Position = Stream-Pixel / Skalierung (- Menueleisten-Platz bei virtuellem Monitor)
        def expected(px, py):
            return (px / scale, py / scale - off_y)

        def rel(e):
            return (e["screen"][0] - e["win"][0], e["screen"][1] - e["win"][1])

        def near(a_, b_, tol=0.75):
            return abs(a_[0] - b_[0]) <= tol and abs(a_[1] - b_[1]) <= tol

        title_bar_est = rel(last)[1] - last["view"][1]
        print("1. Maus")
        t_mark = time.time()
        n0 = len(probe.events())
        inject(k="move", x=600.0, y=400.0)
        inject(k="move", x=1201.0, y=701.0)
        evs = probe.wait_for(lambda e: e.get("ev") == "move" and near(rel(e), expected(1201, 701)), 2)
        rep.check("Bewegung pixelgenau (Stream 1201,701)", bool(evs),
                  f"erwartet {expected(1201, 701)}, gesehen {rel(evs[-1]) if evs else '-'}")

        # Einfacher Klick
        inject(k="down", x=1000.0, y=600.0, b="left", n=1)
        inject(k="up", x=1000.0, y=600.0, b="left")
        d = probe.wait_for(lambda e: e.get("ev") == "down" and e["button"] == 0 and near(rel(e), expected(1000, 600)), 2)
        rep.check("Linksklick pixelgenau", bool(d), f"erwartet {expected(1000, 600)}, gesehen {rel(d[-1]) if d else '-'}")
        rep.check("Linksklick: Klickzahl 1", bool(d) and d[-1]["clicks"] == 1)

        # Doppelklick
        inject(k="down", x=800.0, y=500.0, b="left", n=1)
        inject(k="up", x=800.0, y=500.0, b="left")
        inject(k="down", x=800.0, y=500.0, b="left", n=2)
        inject(k="up", x=800.0, y=500.0, b="left")
        dd = probe.wait_for(lambda e: e.get("ev") == "down" and e.get("clicks") == 2 and near(rel(e), expected(800, 500)), 2)
        rep.check("Doppelklick: zweiter Druck mit Klickzahl 2", bool(dd))

        # Rechtsklick: Kontextmenue
        inject(k="down", x=1400.0, y=900.0, b="right", n=1)
        inject(k="up", x=1400.0, y=900.0, b="right")
        rc = probe.wait_for(lambda e: e.get("ev") == "down" and e.get("button") == 1 and near(rel(e), expected(1400, 900)), 2)
        rep.check("Rechtsklick (Button 1) pixelgenau", bool(rc), f"gesehen {rel(rc[-1]) if rc else '-'}")
        time.sleep(0.6)
        if a.dump_dir:
            h.call("send.keyframe", tid=1)
            print(f"   (Kontextmenue offen: Stream-Bild in {a.dump_dir} ansehen)")
            time.sleep(2.5)
        # Kontextmenue bedienen: Pfeil runter + Return waehlt einen Eintrag
        for key in ("down", "down", "enter"):
            inject(k="key", key=key, down=True)
            inject(k="key", key=key, down=False)
        menu_hit = probe.wait_for(lambda e: e.get("ev") == "menu", 2)
        rep.check("Kontextmenue ist bedienbar (Eintrag gewaehlt)", bool(menu_hit), menu_hit[-1]["title"] if menu_hit else "kein Menueeintrag")
        time.sleep(0.3)

        print("1b. Dropdown")
        # Popup bei View (20..260, 70..98): Klick in die Mitte oeffnet das Menue
        popup_px = 140.0 * scale, (84.0 + title_bar_est + off_y) * scale
        inject(k="down", x=popup_px[0], y=popup_px[1], b="left", n=1)
        inject(k="up", x=popup_px[0], y=popup_px[1], b="left")
        time.sleep(0.6)
        if a.dump_dir:
            h.call("send.keyframe", tid=1)
            print(f"   (Dropdown offen: Stream-Bild in {a.dump_dir} ansehen)")
            time.sleep(2.5)
        for key in ("down", "enter"):
            inject(k="key", key=key, down=True)
            inject(k="key", key=key, down=False)
        dd_hit = probe.wait_for(lambda e: e.get("ev") == "dropdown", 2)
        rep.check("Dropdown ist bedienbar (Eintrag gewaehlt)", bool(dd_hit), dd_hit[-1]["title"] if dd_hit else "keine Auswahl")
        time.sleep(0.3)

        print("2. Scrollen")
        inject(k="scroll", x=800.0, y=500.0, dx=0.0, dy=-3.0, unit="line")
        sc = probe.wait_for(lambda e: e.get("ev") == "scroll", 2)
        rep.check("Scrollen in Zeilen kommt an", bool(sc))
        before = len(probe.events())
        inject(k="scroll", x=800.0, y=500.0, dx=0.0, dy=24.0, unit="px")
        sc2 = [e for e in probe.wait_for(lambda e: e.get("ev") == "scroll", 2)]
        rep.check("Scrollen in Pixeln kommt an", len(sc2) >= 2)

        print("3. Tippen")
        # Textfeld: View-Koordinaten (20..420, 20..48) -> Stream-Pixel
        ready = [e for e in probe.events() if e.get("ev") == "ready"]
        title_bar = 0.0
        # Aus dem Klick weiter oben: Fensterinhalt-Versatz = (rel - view)
        if d:
            title_bar = rel(d[-1])[1] - d[-1]["view"][1]
        fx, fy = 220.0, 34.0
        px, py = fx * scale, (fy + title_bar + off_y) * scale
        inject(k="down", x=px, y=py, b="left", n=1)
        inject(k="up", x=px, y=py, b="left")
        time.sleep(0.2)
        for ch in "hallo":
            inject(k="key", key=ch, down=True)
            inject(k="key", key=ch, down=False)
        inject(k="text", text="äöü ß €")
        t = probe.wait_for(lambda e: e.get("ev") == "text" and "äöü" in e.get("value", ""), 3)
        value = t[-1]["value"] if t else (probe.wait_for(lambda e: e.get("ev") == "text", 1) or [{"value": ""}])[-1]["value"]
        rep.check("Tippen und Umlaute ueber text", "hallo" in value and "äöü ß €" in value, repr(value))
        inject(k="key", key="cmd", down=True)
        inject(k="key", key="a", down=True)
        inject(k="key", key="a", down=False)
        inject(k="key", key="cmd", down=False)
        inject(k="key", key="backspace", down=True)
        inject(k="key", key="backspace", down=False)
        cleared = probe.wait_for(lambda e: e.get("ev") == "text" and e.get("value") == "", 3)
        rep.check("Kuerzel Cmd+A und Rueckschritt", bool(cleared))

        print("4. send.resize")
        probe_before = [e for e in probe.events() if "win" in e][-1]["win"]
        t0 = time.time()
        r = h.call("send.resize", tid=1, w=1200.0, h=800.0)
        t_reply = (time.time() - t0) * 1000
        size_ev = h.events_named("send.size", since=t0)
        t_ev = (size_ev[0]["_at"] - t0) * 1000 if size_ev else None
        rep.check("send.resize beantwortet", r.get("ok"), str(r.get("error", "")))
        if r.get("ok"):
            rep.check("Rundlauf send.resize bis Antwort unter 300 ms", t_reply < 300, f"{t_reply:.0f} ms")
            rep.check("send.size gemeldet", bool(size_ev), f"nach {t_ev:.0f} ms" if t_ev else "kein Ereignis")
            if size_ev:
                rep.check("send.size unter 300 ms", t_ev < 300, f"{t_ev:.0f} ms")
            print(f"   Antwort: {r['width']}x{r['height']} px")
            ok_size = probe.wait_for(lambda e: e.get("ev") == "resized" and abs(e["win"][2] - 1200) < 2, 3)
            wnow = ([e for e in probe.events() if "win" in e][-1]["win"])
            rep.check("Fenster hat die neue Groesse (1200x800 pt)", bool(ok_size) and abs(wnow[3] - 800) < 2, str(wnow))
            exp_w = int(round(1200 * scale))
            rep.check("Antwort nennt die neue Stream-Groesse", r["width"] == exp_w or a.virtual, f"{r['width']} (erwartet ~{exp_w})")

        print("5. Beenden")
        t0 = time.time()
        r = h.call("send.stop", tid=1, restore=True)
        rep.check("send.stop", r.get("ok") and (time.time() - t0) < 2, f"{(time.time() - t0) * 1000:.0f} ms")
        time.sleep(0.8)
        # Die endgueltige Position nicht aus dem Protokoll (das nur bei Eingaben schreibt), sondern frisch lesen.
        now = [w for w in h.call("windows.list")["windows"] if w["wid"] == win["wid"]]
        if a.virtual:
            ok = bool(now) and abs(now[0]["x"] - orig_frame[0]) < 4 and abs(now[0]["y"] - orig_frame[1]) < 4
            rep.check("Fenster ist zurueck auf dem echten Monitor (alte Position)", ok,
                      f"vorher {list(orig_frame)}, nachher {[now[0]['x'], now[0]['y']] if now else '-'}")
            rep.check("Fenster hat die alte Groesse zurueck", bool(now) and abs(now[0]["w"] - win["w"]) < 3 and abs(now[0]["h"] - win["h"]) < 3,
                      f"vorher {win['w']}x{win['h']}, nachher {now[0]['w']}x{now[0]['h']}" if now else "-")
        code = 0 if not rep.failed else 1
    except RuntimeError as e:
        text = str(e)
        print("FEHLER:", text)
        code = 2 if "permission" in text else 1
        if "permission" in text:
            print("-> Bedienungshilfen freigeben (Systemeinstellungen > Datenschutz & Sicherheit > Bedienungshilfen) und neu starten.")
    finally:
        try:
            h.p.stdin.close()
            h.p.wait(timeout=5)
        except Exception:
            h.p.kill()
        for p in procs:
            p.terminate()
    print()
    print(f"{len(rep.rows) - len(rep.failed)} von {len(rep.rows)} Pruefungen bestanden")
    return code or (1 if rep.failed else 0)


if __name__ == "__main__":
    sys.exit(main())
